use tokio::sync::watch;
use url::Url;

use crate::{
    agent::Agent,
    config::{
        ForwarderBuilder,
        HttpTunnelBuilder,
        TcpTunnelBuilder,
        TlsTunnelBuilder,
        TunnelBuilder,
    },
    endpoint::{
        EndpointForwarder,
        EndpointListener,
        TunnelInner,
    },
    error::NgrokError,
    tunnel::{
        EndpointInfo as TunnelEndpointInfo,
        TunnelInfo,
    },
    upstream::Upstream,
};

/// Options for configuring an endpoint.
#[derive(Default)]
pub(crate) struct EndpointOptions {
    pub(crate) url: Option<String>,
    pub(crate) traffic_policy: Option<String>,
    pub(crate) metadata: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) bindings: Vec<String>,
    pub(crate) pooling_enabled: Option<bool>,
    pub(crate) app_protocol: Option<String>,
    pub(crate) verify_upstream_tls: Option<bool>,
    pub(crate) forwards_to: Option<String>,
}

/// A builder for creating an [`EndpointListener`].
pub struct EndpointListenBuilder<'a> {
    agent: &'a Agent,
    opts: EndpointOptions,
}

impl<'a> EndpointListenBuilder<'a> {
    pub(crate) fn new(agent: &'a Agent) -> Self {
        Self {
            agent,
            opts: EndpointOptions::default(),
        }
    }

    /// Set the URL for this endpoint.
    ///
    /// The URL scheme determines the protocol:
    /// - `https://` or `http://` → HTTP endpoint
    /// - `tcp://` → TCP endpoint
    /// - `tls://` → TLS endpoint
    ///
    /// The host portion is used as the domain or address.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.opts.url = Some(url.into());
        self
    }

    /// Set the traffic policy for this endpoint (YAML or JSON string).
    pub fn traffic_policy(mut self, policy: impl Into<String>) -> Self {
        self.opts.traffic_policy = Some(policy.into());
        self
    }

    /// Set opaque metadata for this endpoint.
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.opts.metadata = Some(meta.into());
        self
    }

    /// Set a human-readable description for this endpoint.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.opts.description = Some(desc.into());
        self
    }

    /// Set the bindings for this endpoint.
    pub fn bindings(mut self, bindings: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.opts.bindings = bindings.into_iter().map(|b| b.into()).collect();
        self
    }

    /// Enable or disable connection pooling for this endpoint.
    pub fn pooling_enabled(mut self, enabled: bool) -> Self {
        self.opts.pooling_enabled = Some(enabled);
        self
    }

    /// Set the application protocol for this endpoint.
    pub fn app_protocol(mut self, proto: impl Into<String>) -> Self {
        self.opts.app_protocol = Some(proto.into());
        self
    }

    /// Set the forwarding target description for this endpoint.
    pub fn forwards_to(mut self, addr: impl Into<String>) -> Self {
        self.opts.forwards_to = Some(addr.into());
        self
    }

    /// Start the endpoint listener.
    ///
    /// This will connect to the ngrok service if not already connected
    /// (when `auto_connect` is enabled).
    pub async fn start(self) -> Result<EndpointListener, NgrokError> {
        let session = self.agent.ensure_connected().await?;
        let (closed_tx, closed_rx) = watch::channel(false);

        let tunnel_inner = start_listener(&session, &self.opts).await?;

        Ok(EndpointListener {
            inner: tunnel_inner,
            agent: self.agent.clone(),
            traffic_policy: self.opts.traffic_policy.unwrap_or_default(),
            description: self.opts.description.unwrap_or_default(),
            bindings: self.opts.bindings,
            pooling_enabled: self.opts.pooling_enabled.unwrap_or(false),
            closed_tx,
            closed_rx,
        })
    }
}

/// A builder for creating an [`EndpointForwarder`].
pub struct EndpointForwardBuilder<'a> {
    agent: &'a Agent,
    upstream: Upstream,
    opts: EndpointOptions,
}

impl<'a> EndpointForwardBuilder<'a> {
    pub(crate) fn new(agent: &'a Agent, upstream: Upstream) -> Self {
        Self {
            agent,
            upstream,
            opts: EndpointOptions::default(),
        }
    }

    /// Set the URL for this endpoint.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.opts.url = Some(url.into());
        self
    }

    /// Set the traffic policy for this endpoint.
    pub fn traffic_policy(mut self, policy: impl Into<String>) -> Self {
        self.opts.traffic_policy = Some(policy.into());
        self
    }

    /// Set opaque metadata for this endpoint.
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.opts.metadata = Some(meta.into());
        self
    }

    /// Set a human-readable description for this endpoint.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.opts.description = Some(desc.into());
        self
    }

    /// Set the bindings for this endpoint.
    pub fn bindings(mut self, bindings: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.opts.bindings = bindings.into_iter().map(|b| b.into()).collect();
        self
    }

    /// Enable or disable connection pooling for this endpoint.
    pub fn pooling_enabled(mut self, enabled: bool) -> Self {
        self.opts.pooling_enabled = Some(enabled);
        self
    }

    /// Set the application protocol for this endpoint.
    pub fn app_protocol(mut self, proto: impl Into<String>) -> Self {
        self.opts.app_protocol = Some(proto.into());
        self
    }

    /// Start the endpoint forwarder.
    pub async fn start(self) -> Result<EndpointForwarder, NgrokError> {
        let session = self.agent.ensure_connected().await?;
        let upstream_url_str = self.upstream.addr.clone();
        let upstream_protocol = self.upstream.protocol.clone();
        let (closed_tx, closed_rx) = watch::channel(false);

        // Parse upstream URL for forwarding
        let to_url: Url = if upstream_url_str.contains("://") {
            upstream_url_str
                .parse()
                .map_err(|_| NgrokError::InvalidUrl {
                    url: upstream_url_str.clone(),
                    reason: "invalid upstream URL".into(),
                })?
        } else {
            format!("tcp://{upstream_url_str}")
                .parse()
                .map_err(|_| NgrokError::InvalidUrl {
                    url: upstream_url_str.clone(),
                    reason: "invalid upstream address".into(),
                })?
        };

        let mut opts = self.opts;
        if opts.forwards_to.is_none() {
            opts.forwards_to = Some(upstream_url_str.clone());
        }
        opts.verify_upstream_tls = Some(self.upstream.verify_upstream_tls);

        let (scheme, domain, addr) = parse_endpoint_url(opts.url.as_deref())?;

        let (id, url, protocol, metadata_val, join) = match scheme.as_str() {
            "https" | "http" | "" => {
                let mut b: HttpTunnelBuilder = session.clone().into();
                if let Some(ref d) = domain {
                    b.domain(d);
                }
                apply_http_opts(&mut b, &opts);
                let fwd = b
                    .listen_and_forward(to_url)
                    .await
                    .map_err(NgrokError::from)?;
                let id = TunnelInfo::id(&fwd).to_string();
                let url = TunnelEndpointInfo::url(&fwd).to_string();
                let protocol = TunnelEndpointInfo::proto(&fwd).to_string();
                let metadata_val = TunnelInfo::metadata(&fwd).to_string();
                (id, url, protocol, metadata_val, fwd.join)
            }
            "tcp" => {
                let mut b: TcpTunnelBuilder = session.clone().into();
                if let Some(ref a) = addr {
                    b.remote_addr(a);
                }
                apply_tcp_opts(&mut b, &opts);
                let fwd = b
                    .listen_and_forward(to_url)
                    .await
                    .map_err(NgrokError::from)?;
                let id = TunnelInfo::id(&fwd).to_string();
                let url = TunnelEndpointInfo::url(&fwd).to_string();
                let protocol = TunnelEndpointInfo::proto(&fwd).to_string();
                let metadata_val = TunnelInfo::metadata(&fwd).to_string();
                (id, url, protocol, metadata_val, fwd.join)
            }
            "tls" => {
                let mut b: TlsTunnelBuilder = session.clone().into();
                if let Some(ref d) = domain {
                    b.domain(d);
                }
                apply_tls_opts(&mut b, &opts);
                let fwd = b
                    .listen_and_forward(to_url)
                    .await
                    .map_err(NgrokError::from)?;
                let id = TunnelInfo::id(&fwd).to_string();
                let url = TunnelEndpointInfo::url(&fwd).to_string();
                let protocol = TunnelEndpointInfo::proto(&fwd).to_string();
                let metadata_val = TunnelInfo::metadata(&fwd).to_string();
                (id, url, protocol, metadata_val, fwd.join)
            }
            other => {
                return Err(NgrokError::UnsupportedScheme {
                    scheme: other.to_string(),
                })
            }
        };

        Ok(EndpointForwarder {
            id,
            url,
            protocol,
            metadata_val,
            upstream_url: upstream_url_str,
            upstream_protocol,
            agent: self.agent.clone(),
            traffic_policy: opts.traffic_policy.unwrap_or_default(),
            description: opts.description.unwrap_or_default(),
            bindings: opts.bindings,
            pooling_enabled: opts.pooling_enabled.unwrap_or(false),
            join,
            closed_tx,
            closed_rx,
        })
    }
}

/// Parse the URL and dispatch to the appropriate protocol builder for listening.
async fn start_listener(
    session: &crate::session::Session,
    opts: &EndpointOptions,
) -> Result<TunnelInner, NgrokError> {
    let (scheme, domain, addr) = parse_endpoint_url(opts.url.as_deref())?;

    match scheme.as_str() {
        "https" | "http" | "" => {
            let mut b: HttpTunnelBuilder = session.clone().into();
            if let Some(ref domain) = domain {
                b.domain(domain);
            }
            apply_http_opts(&mut b, opts);
            let tunnel = b.listen().await.map_err(NgrokError::from)?;
            Ok(TunnelInner::Http(tunnel))
        }
        "tcp" => {
            let mut b: TcpTunnelBuilder = session.clone().into();
            if let Some(ref addr) = addr {
                b.remote_addr(addr);
            }
            apply_tcp_opts(&mut b, opts);
            let tunnel = b.listen().await.map_err(NgrokError::from)?;
            Ok(TunnelInner::Tcp(tunnel))
        }
        "tls" => {
            let mut b: TlsTunnelBuilder = session.clone().into();
            if let Some(ref domain) = domain {
                b.domain(domain);
            }
            apply_tls_opts(&mut b, opts);
            let tunnel = b.listen().await.map_err(NgrokError::from)?;
            Ok(TunnelInner::Tls(tunnel))
        }
        other => Err(NgrokError::UnsupportedScheme {
            scheme: other.to_string(),
        }),
    }
}

/// Parse an endpoint URL into (scheme, domain, address) components.
fn parse_endpoint_url(
    url: Option<&str>,
) -> Result<(String, Option<String>, Option<String>), NgrokError> {
    let url_str = match url {
        Some(u) => u,
        None => return Ok(("https".to_string(), None, None)),
    };

    // Handle the case where it's just a domain name (no scheme)
    let effective_url = if !url_str.contains("://") {
        format!("https://{url_str}")
    } else {
        url_str.to_string()
    };

    let parsed: Url = effective_url.parse().map_err(|_| NgrokError::InvalidUrl {
        url: url_str.to_string(),
        reason: "failed to parse URL".into(),
    })?;

    let scheme = parsed.scheme().to_string();
    let host = parsed.host_str().map(|h| {
        if let Some(port) = parsed.port() {
            format!("{h}:{port}")
        } else {
            h.to_string()
        }
    });

    // For TCP, the host:port is the remote address
    // For HTTP/TLS, the host is the domain
    let (domain, addr) = match scheme.as_str() {
        "tcp" => (None, host),
        _ => (host, None),
    };

    Ok((scheme, domain, addr))
}

fn apply_http_opts(b: &mut HttpTunnelBuilder, opts: &EndpointOptions) {
    if let Some(ref tp) = opts.traffic_policy {
        b.traffic_policy(tp);
    }
    if let Some(ref meta) = opts.metadata {
        b.metadata(meta);
    }
    if let Some(ref ft) = opts.forwards_to {
        b.forwards_to(ft);
    }
    // Only the first binding is used; the underlying builder panics on multiple calls.
    if let Some(binding) = opts.bindings.first() {
        b.binding(binding);
    }
    if let Some(pe) = opts.pooling_enabled {
        b.pooling_enabled(pe);
    }
    if let Some(ref ap) = opts.app_protocol {
        b.app_protocol(ap);
    }
    if let Some(vut) = opts.verify_upstream_tls {
        b.verify_upstream_tls(vut);
    }
}

fn apply_tcp_opts(b: &mut TcpTunnelBuilder, opts: &EndpointOptions) {
    if let Some(ref tp) = opts.traffic_policy {
        b.traffic_policy(tp);
    }
    if let Some(ref meta) = opts.metadata {
        b.metadata(meta);
    }
    if let Some(ref ft) = opts.forwards_to {
        b.forwards_to(ft);
    }
    // Only the first binding is used; the underlying builder panics on multiple calls.
    if let Some(binding) = opts.bindings.first() {
        b.binding(binding);
    }
    if let Some(pe) = opts.pooling_enabled {
        b.pooling_enabled(pe);
    }
    if let Some(vut) = opts.verify_upstream_tls {
        b.verify_upstream_tls(vut);
    }
}

fn apply_tls_opts(b: &mut TlsTunnelBuilder, opts: &EndpointOptions) {
    if let Some(ref tp) = opts.traffic_policy {
        b.traffic_policy(tp);
    }
    if let Some(ref meta) = opts.metadata {
        b.metadata(meta);
    }
    if let Some(ref ft) = opts.forwards_to {
        b.forwards_to(ft);
    }
    // Only the first binding is used; the underlying builder panics on multiple calls.
    if let Some(binding) = opts.bindings.first() {
        b.binding(binding);
    }
    if let Some(pe) = opts.pooling_enabled {
        b.pooling_enabled(pe);
    }
    if let Some(vut) = opts.verify_upstream_tls {
        b.verify_upstream_tls(vut);
    }
}
