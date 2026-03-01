use std::time::Instant;

use tokio::sync::RwLock;

use crate::{
    config::{
        ForwarderBuilder,
        HttpTunnelBuilder,
        Scheme,
        TcpTunnelBuilder,
        TlsTunnelBuilder,
        TunnelBuilder,
    },
    forwarder::Forwarder,
    session::{ConnectError, RpcError, SessionBuilder},
    tunnel::{
        EndpointInfo,
        HttpTunnel,
        TcpTunnel,
        TlsTunnel,
        TunnelCloser,
        TunnelInfo,
    },
    Session,
};

use super::{
    builder::AgentBuilder,
    endpoint_options::EndpointOpts,
    events::{Event, EventHandler},
    upstream::Upstream,
    EndpointOption,
};

/// An ngrok agent.
///
/// The `Agent` is the main interface for interacting with the ngrok service.
/// It manages the session lifecycle and provides methods for creating endpoints.
///
/// # Example
///
/// ```rust,no_run
/// use ngrok::agent::Agent;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = Agent::builder()
///     .authtoken("your-authtoken")
///     .build()?;
///
/// agent.connect().await?;
///
/// // Create a listener
/// use ngrok::agent::with_url;
/// let listener = agent.listen(vec![with_url("https://example.ngrok.app")]).await?;
/// # Ok(())
/// # }
/// ```
pub struct Agent {
    pub(crate) session_builder: SessionBuilder,
    pub(crate) session: RwLock<Option<Session>>,
    pub(crate) auto_connect: bool,
    pub(crate) event_handlers: Vec<EventHandler>,
    pub(crate) endpoints: RwLock<Vec<EndpointRecord>>,
}

/// A record of an endpoint created by the agent.
pub(crate) struct EndpointRecord {
    pub(crate) id: String,
    pub(crate) url: Option<String>,
}

/// Information about an active endpoint.
#[derive(Debug, Clone)]
pub struct EndpointRef {
    /// The endpoint's unique ID.
    pub id: String,
    /// The endpoint's URL, if available.
    pub url: Option<String>,
}

/// Information about the agent's current session.
pub struct AgentSession {
    session: Session,
}

impl AgentSession {
    /// Returns the session ID.
    pub fn id(&self) -> String {
        self.session.id()
    }

    /// Returns a reference to the underlying session.
    pub fn session(&self) -> &Session {
        &self.session
    }
}

/// Errors that can occur when using the agent.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// A connection error occurred.
    #[error("connection error: {0}")]
    Connect(#[from] ConnectError),
    /// An RPC error occurred.
    #[error("rpc error: {0}")]
    Rpc(#[from] RpcError),
    /// The agent is not connected.
    #[error("agent not connected, call connect() first")]
    NotConnected,
    /// The agent is already connected.
    #[error("agent already connected")]
    AlreadyConnected,
    /// An invalid URL was provided.
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    /// An unsupported URL scheme was provided.
    #[error("unsupported endpoint URL scheme: {0}")]
    UnsupportedScheme(String),
}

impl Agent {
    /// Create a new [`AgentBuilder`] to configure an agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// Connect to the ngrok service.
    ///
    /// Establishes a session with the ngrok cloud service. If the agent
    /// is already connected, returns an error.
    pub async fn connect(&self) -> Result<(), AgentError> {
        let mut session_guard = self.session.write().await;
        if session_guard.is_some() {
            return Err(AgentError::AlreadyConnected);
        }

        let session = self.session_builder.connect().await?;
        *session_guard = Some(session);

        self.emit_event(Event::AgentConnectSucceeded {
            timestamp: Instant::now(),
        });

        Ok(())
    }

    /// Disconnect from the ngrok service.
    ///
    /// Terminates the current session. If the agent is not connected,
    /// this is a no-op.
    pub async fn disconnect(&self) -> Result<(), AgentError> {
        let mut session_guard = self.session.write().await;
        if let Some(mut session) = session_guard.take() {
            // Clear endpoints
            self.endpoints.write().await.clear();
            session.close().await?;
        }
        Ok(())
    }

    /// Returns information about the current session.
    ///
    /// Returns `None` if the agent is not connected.
    pub async fn session(&self) -> Option<AgentSession> {
        let guard = self.session.read().await;
        guard.as_ref().map(|s| AgentSession {
            session: s.clone(),
        })
    }

    /// Returns a list of all endpoints created by this agent.
    pub async fn endpoints(&self) -> Vec<EndpointRef> {
        self.endpoints
            .read()
            .await
            .iter()
            .map(|e| EndpointRef {
                id: e.id.clone(),
                url: e.url.clone(),
            })
            .collect()
    }

    /// Listen for connections on a new endpoint.
    ///
    /// Creates an endpoint and returns a listener that can accept connections.
    /// The endpoint's protocol is determined by the URL scheme (defaulting to HTTPS).
    ///
    /// If the agent is not connected and `auto_connect` is enabled, it will
    /// automatically connect first.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use ngrok::agent::{Agent, with_url, with_metadata};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::builder()
    ///     .authtoken("your-authtoken")
    ///     .build()?;
    ///
    /// let listener = agent.listen(vec![
    ///     with_url("https://example.ngrok.app"),
    ///     with_metadata("my-service"),
    /// ]).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn listen(
        &self,
        opts: impl IntoIterator<Item = EndpointOption>,
    ) -> Result<AgentEndpointListener, AgentError> {
        self.ensure_connected().await?;

        let endpoint_opts = EndpointOpts::apply_options(opts);
        let scheme = endpoint_opts.url_scheme().to_owned();

        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::NotConnected)?;

        let listener = match scheme.as_str() {
            "http" | "https" => {
                let mut builder = HttpTunnelBuilder::from(session);
                apply_common_opts_http(&mut builder, &endpoint_opts);
                let tunnel = builder.listen().await?;
                let id = tunnel.id().to_string();
                let url = tunnel.url().to_string();
                self.register_endpoint(&id, Some(&url)).await;
                AgentEndpointListener::Http(tunnel)
            }
            "tcp" => {
                let mut builder = TcpTunnelBuilder::from(session);
                apply_common_opts_tcp(&mut builder, &endpoint_opts);
                let tunnel = builder.listen().await?;
                let id = tunnel.id().to_string();
                let url = tunnel.url().to_string();
                self.register_endpoint(&id, Some(&url)).await;
                AgentEndpointListener::Tcp(tunnel)
            }
            "tls" => {
                let mut builder = TlsTunnelBuilder::from(session);
                apply_common_opts_tls(&mut builder, &endpoint_opts);
                let tunnel = builder.listen().await?;
                let id = tunnel.id().to_string();
                let url = tunnel.url().to_string();
                self.register_endpoint(&id, Some(&url)).await;
                AgentEndpointListener::Tls(tunnel)
            }
            other => return Err(AgentError::UnsupportedScheme(other.to_string())),
        };

        Ok(listener)
    }

    /// Forward connections to an upstream service.
    ///
    /// Creates an endpoint that automatically forwards all incoming connections
    /// to the specified upstream address.
    ///
    /// If the agent is not connected and `auto_connect` is enabled, it will
    /// automatically connect first.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use ngrok::agent::{Agent, Upstream, with_url};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let agent = Agent::builder()
    ///     .authtoken("your-authtoken")
    ///     .build()?;
    ///
    /// let upstream = Upstream::new("http://localhost:8080");
    /// let forwarder = agent.forward(
    ///     upstream,
    ///     vec![with_url("https://example.ngrok.app")],
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn forward(
        &self,
        upstream: Upstream,
        opts: impl IntoIterator<Item = EndpointOption>,
    ) -> Result<AgentEndpointForwarder, AgentError> {
        self.ensure_connected().await?;

        let endpoint_opts = EndpointOpts::apply_options(opts);
        let scheme = endpoint_opts.url_scheme().to_owned();

        let to_url = upstream
            .to_url()
            .map_err(|e| AgentError::InvalidUrl(e.to_string()))?;

        let session = self
            .session
            .read()
            .await
            .clone()
            .ok_or(AgentError::NotConnected)?;

        let forwarder = match scheme.as_str() {
            "http" | "https" => {
                let mut builder = HttpTunnelBuilder::from(session);
                apply_common_opts_http(&mut builder, &endpoint_opts);
                if let Some(proto) = &upstream.protocol {
                    builder.app_protocol(proto);
                }
                let fwd = builder.listen_and_forward(to_url).await?;
                let id = fwd.id().to_string();
                let url = fwd.url().to_string();
                self.register_endpoint(&id, Some(&url)).await;
                AgentEndpointForwarder::Http(fwd)
            }
            "tcp" => {
                let mut builder = TcpTunnelBuilder::from(session);
                apply_common_opts_tcp(&mut builder, &endpoint_opts);
                let fwd = builder.listen_and_forward(to_url).await?;
                let id = fwd.id().to_string();
                let url = fwd.url().to_string();
                self.register_endpoint(&id, Some(&url)).await;
                AgentEndpointForwarder::Tcp(fwd)
            }
            "tls" => {
                let mut builder = TlsTunnelBuilder::from(session);
                apply_common_opts_tls(&mut builder, &endpoint_opts);
                let fwd = builder.listen_and_forward(to_url).await?;
                let id = fwd.id().to_string();
                let url = fwd.url().to_string();
                self.register_endpoint(&id, Some(&url)).await;
                AgentEndpointForwarder::Tls(fwd)
            }
            other => return Err(AgentError::UnsupportedScheme(other.to_string())),
        };

        Ok(forwarder)
    }

    async fn ensure_connected(&self) -> Result<(), AgentError> {
        {
            let guard = self.session.read().await;
            if guard.is_some() {
                return Ok(());
            }
        }

        if self.auto_connect {
            self.connect().await?;
        } else {
            return Err(AgentError::NotConnected);
        }
        Ok(())
    }

    async fn register_endpoint(&self, id: &str, url: Option<&str>) {
        self.endpoints.write().await.push(EndpointRecord {
            id: id.to_string(),
            url: url.map(String::from),
        });
    }

    fn emit_event(&self, event: Event) {
        for handler in &self.event_handlers {
            handler(event.clone());
        }
    }
}

/// An endpoint listener created by the agent.
///
/// This enum wraps the protocol-specific tunnel types, providing a unified
/// interface for accepting connections.
pub enum AgentEndpointListener {
    /// An HTTP endpoint listener.
    Http(HttpTunnel),
    /// A TCP endpoint listener.
    Tcp(TcpTunnel),
    /// A TLS endpoint listener.
    Tls(TlsTunnel),
}

impl AgentEndpointListener {
    /// Returns the endpoint's unique ID.
    pub fn id(&self) -> &str {
        match self {
            AgentEndpointListener::Http(t) => t.id(),
            AgentEndpointListener::Tcp(t) => t.id(),
            AgentEndpointListener::Tls(t) => t.id(),
        }
    }

    /// Returns the endpoint's URL.
    pub fn url(&self) -> &str {
        match self {
            AgentEndpointListener::Http(t) => t.url(),
            AgentEndpointListener::Tcp(t) => t.url(),
            AgentEndpointListener::Tls(t) => t.url(),
        }
    }

    /// Returns the endpoint's protocol.
    pub fn protocol(&self) -> &str {
        match self {
            AgentEndpointListener::Http(t) => t.proto(),
            AgentEndpointListener::Tcp(t) => t.proto(),
            AgentEndpointListener::Tls(t) => t.proto(),
        }
    }

    /// Returns the endpoint's metadata.
    pub fn metadata(&self) -> &str {
        match self {
            AgentEndpointListener::Http(t) => t.metadata(),
            AgentEndpointListener::Tcp(t) => t.metadata(),
            AgentEndpointListener::Tls(t) => t.metadata(),
        }
    }

    /// Close the endpoint.
    pub async fn close(&mut self) -> Result<(), RpcError> {
        match self {
            AgentEndpointListener::Http(t) => t.close().await,
            AgentEndpointListener::Tcp(t) => t.close().await,
            AgentEndpointListener::Tls(t) => t.close().await,
        }
    }
}

/// An endpoint forwarder created by the agent.
///
/// This enum wraps the protocol-specific forwarder types.
pub enum AgentEndpointForwarder {
    /// An HTTP endpoint forwarder.
    Http(Forwarder<HttpTunnel>),
    /// A TCP endpoint forwarder.
    Tcp(Forwarder<TcpTunnel>),
    /// A TLS endpoint forwarder.
    Tls(Forwarder<TlsTunnel>),
}

impl AgentEndpointForwarder {
    /// Returns the endpoint's unique ID.
    pub fn id(&self) -> &str {
        match self {
            AgentEndpointForwarder::Http(f) => f.id(),
            AgentEndpointForwarder::Tcp(f) => f.id(),
            AgentEndpointForwarder::Tls(f) => f.id(),
        }
    }

    /// Returns the endpoint's URL.
    pub fn url(&self) -> &str {
        match self {
            AgentEndpointForwarder::Http(f) => f.url(),
            AgentEndpointForwarder::Tcp(f) => f.url(),
            AgentEndpointForwarder::Tls(f) => f.url(),
        }
    }

    /// Returns the endpoint's metadata.
    pub fn metadata(&self) -> &str {
        match self {
            AgentEndpointForwarder::Http(f) => f.metadata(),
            AgentEndpointForwarder::Tcp(f) => f.metadata(),
            AgentEndpointForwarder::Tls(f) => f.metadata(),
        }
    }

    /// Close the endpoint.
    pub async fn close(&mut self) -> Result<(), RpcError> {
        match self {
            AgentEndpointForwarder::Http(f) => f.close().await,
            AgentEndpointForwarder::Tcp(f) => f.close().await,
            AgentEndpointForwarder::Tls(f) => f.close().await,
        }
    }
}

fn apply_common_opts_http(builder: &mut HttpTunnelBuilder, opts: &EndpointOpts) {
    if let Some(ref url) = opts.url {
        // Parse the URL to extract domain and set scheme
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                let domain_with_port = if let Some(port) = parsed.port() {
                    format!("{}:{}", host, port)
                } else {
                    host.to_string()
                };
                builder.domain(domain_with_port);
            }
            if parsed.scheme() == "http" {
                builder.scheme(Scheme::HTTP);
            } else {
                builder.scheme(Scheme::HTTPS);
            }
        }
    }
    if let Some(ref meta) = opts.metadata {
        builder.metadata(meta);
    }
    if let Some(ref policy) = opts.traffic_policy {
        builder.traffic_policy(policy);
    }
    if let Some(pooling) = opts.pooling_enabled {
        builder.pooling_enabled(pooling);
    }
    if !opts.bindings.is_empty() {
        // The underlying builder's binding() method supports only one binding at a time.
        // Use the first binding from the list.
        if let Some(binding) = opts.bindings.first() {
            builder.binding(binding.clone());
        }
    }
}

fn apply_common_opts_tcp(builder: &mut TcpTunnelBuilder, opts: &EndpointOpts) {
    if let Some(ref url) = opts.url {
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                let addr = if let Some(port) = parsed.port() {
                    format!("{}:{}", host, port)
                } else {
                    host.to_string()
                };
                builder.remote_addr(addr);
            }
        }
    }
    if let Some(ref meta) = opts.metadata {
        builder.metadata(meta);
    }
    if let Some(ref policy) = opts.traffic_policy {
        builder.traffic_policy(policy);
    }
    if let Some(pooling) = opts.pooling_enabled {
        builder.pooling_enabled(pooling);
    }
    if !opts.bindings.is_empty() {
        if let Some(binding) = opts.bindings.first() {
            builder.binding(binding.clone());
        }
    }
}

fn apply_common_opts_tls(builder: &mut TlsTunnelBuilder, opts: &EndpointOpts) {
    if let Some(ref url) = opts.url {
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                let domain_with_port = if let Some(port) = parsed.port() {
                    format!("{}:{}", host, port)
                } else {
                    host.to_string()
                };
                builder.domain(domain_with_port);
            }
        }
    }
    if let Some(ref meta) = opts.metadata {
        builder.metadata(meta);
    }
    if let Some(ref policy) = opts.traffic_policy {
        builder.traffic_policy(policy);
    }
    if let Some(pooling) = opts.pooling_enabled {
        builder.pooling_enabled(pooling);
    }
    if !opts.bindings.is_empty() {
        if let Some(binding) = opts.bindings.first() {
            builder.binding(binding.clone());
        }
    }
}
