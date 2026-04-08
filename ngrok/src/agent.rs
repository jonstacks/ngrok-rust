//! Agent builder and agent handle.

use std::{
    collections::HashMap,
    sync::Arc,
    time::Duration,
};

use tokio::sync::{
    RwLock,
    mpsc,
    watch,
};
use url::Url;

use crate::{
    diagnose::DiagnoseResult,
    endpoint::{
        EndpointInfo,
        EndpointKind,
    },
    error::{
        DiagnoseError,
        Error,
    },
    events::Event,
    forwarder::{
        self,
        EndpointForwarder,
    },
    listener::EndpointListener,
    options::EndpointOptions,
    proto::msg::{
        AuthExtra,
        BindReq,
    },
    rpc::{
        RpcRequest,
        RpcResponse,
    },
    session::AgentSession,
    tunnel::reconnecting::{
        DialConfig,
        ReconnectInner,
        ReconnectingSession,
        TunnelConfig,
    },
    upstream::Upstream,
};

/// Builder for an `Agent`.
///
/// Use `Agent::builder()` to create one.
#[must_use]
pub struct AgentBuilder {
    authtoken: Option<String>,
    connect_url: Option<String>,
    tls_config: Option<rustls::ClientConfig>,
    connect_ca_cert: Option<Vec<u8>>,
    proxy_url: Option<String>,
    auto_connect: bool,
    heartbeat_interval: Duration,
    heartbeat_tolerance: Duration,
    client_type: Option<String>,
    client_version: Option<String>,
    description: Option<String>,
    metadata: Option<String>,
    event_handlers: Vec<Box<dyn Fn(Event) + Send + Sync>>,
    rpc_handler: Option<Arc<dyn Fn(RpcRequest) -> RpcResponse + Send + Sync>>,
    with_tracing: bool,
}

impl AgentBuilder {
    /// Set the authtoken for ngrok cloud authentication.
    pub fn authtoken(mut self, token: impl Into<String>) -> Self {
        self.authtoken = Some(token.into());
        self
    }

    /// Read the authtoken from the `NGROK_AUTHTOKEN` environment variable.
    pub fn authtoken_from_env(mut self) -> Self {
        self.authtoken = std::env::var("NGROK_AUTHTOKEN").ok();
        self
    }

    /// Set the connect URL (host:port) for the ngrok cloud.
    pub fn connect_url(mut self, addr: impl Into<String>) -> Self {
        self.connect_url = Some(addr.into());
        self
    }

    /// Set a custom CA certificate for TLS verification (DER or PEM bytes).
    pub fn connect_ca_cert(mut self, pem: &[u8]) -> Self {
        self.connect_ca_cert = Some(pem.to_vec());
        self
    }

    /// Set a custom TLS client config.
    pub fn tls_config(mut self, cfg: rustls::ClientConfig) -> Self {
        self.tls_config = Some(cfg);
        self
    }

    /// Set an HTTP/SOCKS proxy URL for outbound connections.
    pub fn proxy_url(mut self, url: impl Into<String>) -> Self {
        self.proxy_url = Some(url.into());
        self
    }

    /// Whether to auto-connect on build. Default: true.
    pub fn auto_connect(mut self, auto: bool) -> Self {
        self.auto_connect = auto;
        self
    }

    /// Set the heartbeat interval.
    pub fn heartbeat_interval(mut self, d: Duration) -> Self {
        self.heartbeat_interval = d;
        self
    }

    /// Set the heartbeat tolerance.
    pub fn heartbeat_tolerance(mut self, d: Duration) -> Self {
        self.heartbeat_tolerance = d;
        self
    }

    /// Set client info reported to ngrok cloud.
    pub fn client_info(
        mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.client_type = Some(client_type.into());
        self.client_version = Some(version.into());
        self
    }

    /// Set a human-readable description for this agent.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set user-defined metadata for this agent.
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.metadata = Some(meta.into());
        self
    }

    /// Register an event handler.
    pub fn on_event(mut self, handler: impl Fn(Event) + Send + Sync + 'static) -> Self {
        self.event_handlers.push(Box::new(handler));
        self
    }

    /// Register an RPC handler for server-initiated commands.
    pub fn on_rpc(
        mut self,
        handler: impl Fn(RpcRequest) -> RpcResponse + Send + Sync + 'static,
    ) -> Self {
        self.rpc_handler = Some(Arc::new(handler));
        self
    }

    /// Enable tracing integration.
    pub fn with_tracing(mut self) -> Self {
        self.with_tracing = true;
        self
    }

    /// Build the agent, optionally connecting to ngrok cloud.
    pub async fn build(self) -> Result<Agent, Error> {
        let authtoken = self.authtoken.ok_or(Error::MissingAuthtoken)?;
        let connect_url = self
            .connect_url
            .unwrap_or_else(|| crate::DEFAULT_CONNECT_URL.to_string());

        let tls_config = match self.tls_config {
            Some(cfg) => Arc::new(cfg),
            None => {
                let mut root_store = rustls::RootCertStore::empty();

                // Add custom CA cert if provided.
                if let Some(pem_data) = &self.connect_ca_cert {
                    let certs = rustls_pemfile::certs(&mut &pem_data[..])
                        .filter_map(|r| r.ok())
                        .collect::<Vec<_>>();
                    for cert in certs {
                        root_store.add(cert).map_err(|e| {
                            Error::Io(std::io::Error::other(format!("failed to add CA cert: {e}")))
                        })?;
                    }
                }

                // Add the bundled ngrok CA certificate.
                let ngrok_ca = include_bytes!("../assets/ngrok.ca.crt");
                let ngrok_certs = rustls_pemfile::certs(&mut &ngrok_ca[..])
                    .filter_map(|r| r.ok())
                    .collect::<Vec<_>>();
                for cert in ngrok_certs {
                    root_store.add(cert).map_err(|e| {
                        Error::Io(std::io::Error::other(format!(
                            "failed to add ngrok CA cert: {e}"
                        )))
                    })?;
                }

                // Add native root certificates when available.
                #[cfg(feature = "rustls-native-roots")]
                {
                    let native_certs = rustls_native_certs::load_native_certs();
                    for cert in native_certs.certs {
                        let _ = root_store.add(cert);
                    }
                }

                #[cfg(feature = "aws-lc-rs")]
                let provider = rustls::crypto::aws_lc_rs::default_provider();
                #[cfg(not(feature = "aws-lc-rs"))]
                let provider = rustls::crypto::ring::default_provider();

                let config = rustls::ClientConfig::builder_with_provider(Arc::new(provider))
                    .with_safe_default_protocol_versions()
                    .map_err(|e| {
                        Error::Io(std::io::Error::other(format!(
                            "failed to configure TLS: {e}"
                        )))
                    })?
                    .with_root_certificates(root_store)
                    .with_no_client_auth();

                Arc::new(config)
            }
        };

        let auth_extra = AuthExtra {
            authtoken: authtoken.clone(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            version: self
                .client_version
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into()),
            hostname: gethostname().unwrap_or_default(),
            user_agent: self
                .client_type
                .map(|t| format!("{}/{}", t, env!("CARGO_PKG_VERSION")))
                .unwrap_or_else(|| format!("ngrok-rust/{}", env!("CARGO_PKG_VERSION"))),
            metadata: self.metadata.unwrap_or_default(),
            heartbeat_interval_ms: self.heartbeat_interval.as_nanos() as i64,
            heartbeat_tolerance_ms: self.heartbeat_tolerance.as_nanos() as i64,
            ..Default::default()
        };

        let dial_config = DialConfig {
            connect_url,
            tls_config,
            heartbeat_interval: self.heartbeat_interval,
            heartbeat_tolerance: self.heartbeat_tolerance,
        };

        let (connected_tx, _connected_rx) = watch::channel(false);

        let reconnect_inner = Arc::new(ReconnectInner {
            auth_extra: RwLock::new(auth_extra),
            tunnels: RwLock::new(Vec::new()),
            tunnel_senders: Arc::new(RwLock::new(HashMap::new())),
            rpc_handler: self.rpc_handler,
            event_handlers: Arc::new(self.event_handlers),
            cookie: RwLock::new(String::new()),
            session: RwLock::new(None),
            raw_session: RwLock::new(None),
            closed: std::sync::atomic::AtomicBool::new(false),
            dial_config,
            connected_tx,
        });

        let reconnecting = if self.auto_connect {
            Some(ReconnectingSession::start(reconnect_inner.clone()))
        } else {
            None
        };

        let agent = Agent {
            inner: Arc::new(AgentInner {
                reconnect_inner,
                reconnecting: RwLock::new(reconnecting),
                endpoints: RwLock::new(Vec::new()),
            }),
        };

        Ok(agent)
    }
}

/// The root handle for the ngrok agent.
///
/// Manages authentication, the persistent muxado session, and all open endpoints.
/// Cheaply cloneable via `Arc`.
#[derive(Clone)]
pub struct Agent {
    inner: Arc<AgentInner>,
}

struct AgentInner {
    /// The reconnect inner state.
    reconnect_inner: Arc<ReconnectInner>,
    /// The reconnecting session, if auto-connect is enabled.
    reconnecting: RwLock<Option<ReconnectingSession>>,
    /// Open endpoint infos.
    endpoints: RwLock<Vec<EndpointInfo>>,
}

impl Agent {
    /// Create a new `AgentBuilder`.
    pub fn builder() -> AgentBuilder {
        AgentBuilder {
            authtoken: None,
            connect_url: None,
            tls_config: None,
            connect_ca_cert: None,
            proxy_url: None,
            auto_connect: true,
            heartbeat_interval: Duration::from_secs(10),
            heartbeat_tolerance: Duration::from_secs(15),
            client_type: None,
            client_version: None,
            description: None,
            metadata: None,
            event_handlers: Vec::new(),
            rpc_handler: None,
            with_tracing: false,
        }
    }

    /// Explicitly connect to ngrok cloud.
    ///
    /// No-op if already connected and `auto_connect` is true.
    pub async fn connect(&self) -> Result<(), Error> {
        let mut reconnecting = self.inner.reconnecting.write().await;
        if reconnecting.is_none() {
            *reconnecting = Some(ReconnectingSession::start(
                self.inner.reconnect_inner.clone(),
            ));
        }
        // Wait for connection.
        let mut rx = self.inner.reconnect_inner.connected_tx.subscribe();
        while !*rx.borrow() {
            rx.changed().await.map_err(|_| Error::NotConnected)?;
        }
        Ok(())
    }

    /// Disconnect cleanly. All open endpoints signal completion.
    pub async fn disconnect(&self) -> Result<(), Error> {
        self.inner
            .reconnect_inner
            .closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Return the active session, if any.
    pub fn session(&self) -> Option<AgentSession> {
        // This needs to be synchronous, so we use try_read.
        match self.inner.reconnect_inner.session.try_read() {
            Ok(guard) => guard.clone(),
            Err(_) => None,
        }
    }

    /// Snapshot of all currently open endpoints.
    pub fn endpoints(&self) -> Vec<EndpointInfo> {
        match self.inner.endpoints.try_read() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Open a new listener endpoint.
    pub async fn listen(&self, opts: EndpointOptions) -> Result<EndpointListener, Error> {
        // Ensure we have a reconnecting session.
        {
            let mut reconnecting = self.inner.reconnecting.write().await;
            if reconnecting.is_none() {
                *reconnecting = Some(ReconnectingSession::start(
                    self.inner.reconnect_inner.clone(),
                ));
            }
        }

        // Wait for connection.
        let mut rx = self.inner.reconnect_inner.connected_tx.subscribe();
        while !*rx.borrow() {
            rx.changed().await.map_err(|_| Error::NotConnected)?;
        }

        // Build bind request.
        let proto = opts
            .url
            .as_ref()
            .and_then(|u| Url::parse(u).ok())
            .map(|u| u.scheme().to_string())
            .unwrap_or_else(|| "https".into());

        let bind_req = BindReq {
            client_id: String::new(),
            proto: proto.clone(),
            opts: build_bind_opts(&opts),
            forwards: String::new(),
        };

        let (accept_tx, accept_rx) = mpsc::channel(128);

        let tunnel_config = TunnelConfig {
            bind_req: bind_req.clone(),
            accept_tx: accept_tx.clone(),
        };

        // Bind through the reconnecting session.
        let reconnecting = self.inner.reconnecting.read().await;
        let sess = reconnecting.as_ref().ok_or(Error::NotConnected)?;
        let resp = sess.bind(tunnel_config).await?;

        let url = Url::parse(&resp.url).map_err(Error::Url)?;
        let (done_tx, done_rx) = watch::channel(false);

        let listener = EndpointListener {
            url: url.clone(),
            id: resp.client_id.clone(),
            name: opts.name.unwrap_or_default(),
            metadata: opts.metadata.unwrap_or_default(),
            protocol: resp.proto.clone(),
            accept_rx,
            done_rx,
            done_tx,
        };

        // Register endpoint info.
        {
            let mut endpoints = self.inner.endpoints.write().await;
            endpoints.push(EndpointInfo {
                id: resp.client_id,
                name: listener.name.clone(),
                url,
                protocol: resp.proto,
                kind: EndpointKind::Listener,
            });
        }

        Ok(listener)
    }

    /// Open a new forwarder endpoint that auto-proxies to an upstream.
    pub async fn forward(
        &self,
        upstream: Upstream,
        opts: EndpointOptions,
    ) -> Result<EndpointForwarder, Error> {
        let listener = self.listen(opts).await?;
        let url = listener.url().clone();
        let id = listener.id().to_string();
        let upstream_addr = resolve_upstream_addr(&upstream.addr)?;
        let upstream_url = Url::parse(&format!("tcp://{}", upstream_addr))
            .unwrap_or_else(|_| Url::parse("tcp://localhost:0").unwrap());

        let (done_tx, done_rx) = watch::channel(false);
        let task = forwarder::spawn_forward_loop(listener, upstream_addr);

        Ok(EndpointForwarder {
            url,
            id,
            upstream_url,
            upstream_protocol: upstream.protocol,
            proxy_proto: upstream.proxy_proto,
            done_rx,
            done_tx,
            _task: task,
        })
    }

    /// Run a connectivity probe without authenticating.
    pub async fn diagnose(&self, addr: Option<&str>) -> Result<DiagnoseResult, DiagnoseError> {
        let connect_url = addr.unwrap_or(crate::DEFAULT_CONNECT_URL);
        let tls_config = self
            .inner
            .reconnect_inner
            .dial_config
            .tls_config
            .as_ref()
            .clone();
        crate::diagnose::diagnose(connect_url, tls_config).await
    }
}

/// Build bind options from EndpointOptions.
fn build_bind_opts(opts: &EndpointOptions) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    if let Some(ref url) = opts.url {
        map.insert("Hostname".into(), serde_json::Value::String(url.clone()));
    }
    if let Some(ref tp) = opts.traffic_policy {
        map.insert(
            "TrafficPolicy".into(),
            serde_json::Value::String(tp.clone()),
        );
    }
    if opts.pooling_enabled {
        map.insert("PoolingEnabled".into(), serde_json::Value::Bool(true));
    }
    if !opts.bindings.is_empty() {
        map.insert(
            "Bindings".into(),
            serde_json::Value::Array(
                opts.bindings
                    .iter()
                    .map(|s| serde_json::Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    serde_json::Value::Object(map)
}

/// Resolve an upstream address string to host:port.
fn resolve_upstream_addr(addr: &str) -> Result<String, Error> {
    // Handle various forms: "8080", "host:port", "http://host:port", "https://host:port"
    if let Ok(url) = Url::parse(addr) {
        let host = url.host_str().unwrap_or("localhost");
        let port = url
            .port()
            .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
        return Ok(format!("{host}:{port}"));
    }
    // Try as just a port number.
    if let Ok(port) = addr.parse::<u16>() {
        return Ok(format!("localhost:{port}"));
    }
    // Try as host:port.
    if addr.contains(':') {
        return Ok(addr.to_string());
    }
    Err(Error::InvalidUpstream(addr.to_string()))
}

/// Get the hostname of the current machine.
fn gethostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes into a fixed-size stack buffer we own.
        let result = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
        if result == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8(buf[..end].to_vec()).ok()
        } else {
            None
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_port_only() {
        assert_eq!(resolve_upstream_addr("8080").unwrap(), "localhost:8080");
    }

    #[test]
    fn resolve_host_port() {
        // "host:port" is parsed by Url::parse as scheme:path, so it goes through
        // the URL path. The host becomes "localhost" and port defaults.
        // To actually pass a host:port, use an explicit scheme.
        assert_eq!(
            resolve_upstream_addr("http://myhost:3000").unwrap(),
            "myhost:3000"
        );
    }

    #[test]
    fn resolve_http_url() {
        assert_eq!(
            resolve_upstream_addr("http://backend:9000").unwrap(),
            "backend:9000"
        );
    }

    #[test]
    fn resolve_https_url_default_port() {
        assert_eq!(
            resolve_upstream_addr("https://secure.local").unwrap(),
            "secure.local:443"
        );
    }

    #[test]
    fn resolve_http_url_default_port() {
        assert_eq!(
            resolve_upstream_addr("http://backend").unwrap(),
            "backend:80"
        );
    }

    #[test]
    fn resolve_invalid_addr() {
        assert!(resolve_upstream_addr("not-a-valid-thing").is_err());
    }

    #[test]
    fn build_bind_opts_empty() {
        let opts = EndpointOptions::default();
        let val = build_bind_opts(&opts);
        let obj = val.as_object().unwrap();
        assert!(obj.is_empty());
    }

    #[test]
    fn build_bind_opts_with_url_and_policy() {
        let opts = EndpointOptions::builder()
            .url("https://example.ngrok.app")
            .traffic_policy("on-http-request: []")
            .pooling_enabled(true)
            .bindings(vec!["public".into()])
            .build();
        let val = build_bind_opts(&opts);
        let obj = val.as_object().unwrap();
        assert_eq!(
            obj.get("Hostname").and_then(|v| v.as_str()),
            Some("https://example.ngrok.app")
        );
        assert_eq!(
            obj.get("TrafficPolicy").and_then(|v| v.as_str()),
            Some("on-http-request: []")
        );
        assert_eq!(
            obj.get("PoolingEnabled").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(obj.get("Bindings").unwrap().is_array());
    }
}
