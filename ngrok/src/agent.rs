use std::{
    error::Error as StdError,
    sync::{
        Arc,
        Mutex,
    },
    time::{
        Duration,
        Instant,
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use futures_rustls::rustls;
use url::Url;

use crate::{
    endpoint_builder::{
        EndpointForwardBuilder,
        EndpointListenBuilder,
    },
    error::NgrokError,
    event::Event,
    session::{
        ConnectError,
        Connector,
        IoStream,
        Session,
        SessionBuilder,
    },
    tunnel::AcceptError,
    upstream::Upstream,
};

/// An ngrok agent. Manages the connection to the ngrok service and provides
/// methods to create endpoints.
///
/// `Agent` is cheap to clone (it wraps an `Arc` internally).
#[derive(Clone)]
pub struct Agent {
    inner: Arc<AgentInner>,
}

struct AgentInner {
    config: AgentConfig,
    session: Mutex<Option<Session>>,
}

struct AgentConfig {
    authtoken: Option<String>,
    authtoken_from_env: bool,
    connect_url: Option<String>,
    ca_cert: Option<Bytes>,
    tls_config: Option<rustls::ClientConfig>,
    connector: Option<Arc<dyn Connector>>,
    proxy_url: Option<String>,
    auto_connect: bool,
    metadata: Option<String>,
    #[allow(dead_code)]
    description: Option<String>,
    heartbeat_interval: Option<Duration>,
    heartbeat_tolerance: Option<Duration>,
    client_info: Vec<(String, String, Option<String>)>,
    event_handler: Option<Arc<dyn Fn(Event) + Send + Sync + 'static>>,
}

/// A builder for configuring and creating an [`Agent`].
pub struct AgentBuilder {
    config: AgentConfig,
}

impl Agent {
    /// Create a new [`AgentBuilder`] to configure an agent.
    pub fn builder() -> AgentBuilder {
        AgentBuilder {
            config: AgentConfig {
                authtoken: None,
                authtoken_from_env: false,
                connect_url: None,
                ca_cert: None,
                tls_config: None,
                connector: None,
                proxy_url: None,
                auto_connect: true,
                metadata: None,
                description: None,
                heartbeat_interval: None,
                heartbeat_tolerance: None,
                client_info: Vec::new(),
                event_handler: None,
            },
        }
    }

    /// Explicitly connect to the ngrok service.
    ///
    /// This is called automatically on the first `listen` or `forward` call
    /// if `auto_connect` is enabled (the default).
    pub async fn connect(&self) -> Result<(), NgrokError> {
        let session = self.build_session().await?;
        let mut guard = self.inner.session.lock().expect("session lock poisoned");
        *guard = Some(session);
        Ok(())
    }

    /// Disconnect from the ngrok service.
    pub async fn disconnect(&self) -> Result<(), NgrokError> {
        let session = {
            let mut guard = self.inner.session.lock().expect("session lock poisoned");
            guard.take()
        };
        if let Some(mut session) = session {
            session.close().await.map_err(NgrokError::from)?;
        }
        Ok(())
    }

    /// Returns the current session, if connected.
    pub fn session(&self) -> Option<AgentSession> {
        let guard = self.inner.session.lock().expect("session lock poisoned");
        guard.as_ref().map(|s| AgentSession {
            session: s.clone(),
            agent: self.clone(),
            started_at: Instant::now(),
        })
    }

    /// Start building an endpoint listener.
    pub fn listen(&self) -> EndpointListenBuilder<'_> {
        EndpointListenBuilder::new(self)
    }

    /// Start building an endpoint forwarder.
    pub fn forward(&self, upstream: Upstream) -> EndpointForwardBuilder<'_> {
        EndpointForwardBuilder::new(self, upstream)
    }

    /// Ensure the agent is connected, connecting if necessary when auto_connect
    /// is enabled.
    pub(crate) async fn ensure_connected(&self) -> Result<Session, NgrokError> {
        {
            let guard = self.inner.session.lock().expect("session lock poisoned");
            if let Some(session) = guard.as_ref() {
                return Ok(session.clone());
            }
        }

        if !self.inner.config.auto_connect {
            return Err(NgrokError::NotConnected);
        }

        self.connect().await?;

        let guard = self.inner.session.lock().expect("session lock poisoned");
        guard.as_ref().cloned().ok_or(NgrokError::NotConnected)
    }

    async fn build_session(&self) -> Result<Session, NgrokError> {
        let config = &self.inner.config;
        let mut builder = SessionBuilder::default();

        if config.authtoken_from_env {
            builder.authtoken_from_env();
        }
        if let Some(ref token) = config.authtoken {
            builder.authtoken(token.clone());
        }
        if let Some(ref url) = config.connect_url {
            builder
                .server_addr(url.as_str())
                .map_err(|e| NgrokError::InvalidUrl {
                    url: url.clone(),
                    reason: e.to_string(),
                })?;
        }
        if let Some(ref cert) = config.ca_cert {
            builder.ca_cert(cert.clone());
        }
        if let Some(ref tls) = config.tls_config {
            builder.tls_config(tls.clone());
        }
        if let Some(ref connector) = config.connector {
            let c = connector.clone();
            builder.connector(ConnectorWrapper(c));
        }
        if let Some(ref proxy) = config.proxy_url {
            let url: Url = proxy.parse().map_err(|_| NgrokError::InvalidUrl {
                url: proxy.clone(),
                reason: "invalid proxy URL".into(),
            })?;
            builder.proxy_url(url).map_err(|e| NgrokError::Api {
                code: None,
                message: e.to_string(),
            })?;
        }
        if let Some(ref meta) = config.metadata {
            builder.metadata(meta.clone());
        }
        if let Some(interval) = config.heartbeat_interval {
            builder
                .heartbeat_interval(interval)
                .map_err(|e| NgrokError::Api {
                    code: None,
                    message: e.to_string(),
                })?;
        }
        if let Some(tolerance) = config.heartbeat_tolerance {
            builder
                .heartbeat_tolerance(tolerance)
                .map_err(|e| NgrokError::Api {
                    code: None,
                    message: e.to_string(),
                })?;
        }
        for (client_type, version, comments) in &config.client_info {
            builder.client_info(client_type.clone(), version.clone(), comments.clone());
        }

        if let Some(ref handler) = config.event_handler {
            let handler = handler.clone();
            builder.handle_heartbeat(HeartbeatAdapter(handler));
        }

        let session = builder.connect().await?;
        Ok(session)
    }
}

/// Adapter that converts heartbeat callbacks into Event emissions.
struct HeartbeatAdapter(Arc<dyn Fn(Event) + Send + Sync + 'static>);

#[async_trait]
impl muxado::heartbeat::HeartbeatHandler for HeartbeatAdapter {
    async fn handle_heartbeat(&self, latency: Option<Duration>) -> Result<(), Box<dyn StdError>> {
        if let Some(latency) = latency {
            (self.0)(Event::AgentHeartbeatReceived { latency });
        }
        Ok(())
    }
}

/// Adapter that wraps an Arc<dyn Connector> so we can pass it to SessionBuilder.
struct ConnectorWrapper(Arc<dyn Connector>);

#[async_trait]
impl Connector for ConnectorWrapper {
    async fn connect(
        &self,
        host: String,
        port: u16,
        tls_config: Arc<rustls::ClientConfig>,
        err: Option<AcceptError>,
    ) -> Result<Box<dyn IoStream>, ConnectError> {
        self.0.connect(host, port, tls_config, err).await
    }
}

impl AgentBuilder {
    /// Set the authtoken for the agent.
    pub fn authtoken(mut self, token: impl Into<String>) -> Self {
        self.config.authtoken = Some(token.into());
        self
    }

    /// Load the authtoken from the `NGROK_AUTHTOKEN` environment variable.
    pub fn authtoken_from_env(mut self) -> Self {
        self.config.authtoken_from_env = true;
        self
    }

    /// Set the URL to connect to the ngrok service.
    pub fn connect_url(mut self, url: impl Into<String>) -> Self {
        self.config.connect_url = Some(url.into());
        self
    }

    /// Set the CA certificate (PEM) for the agent to use.
    pub fn connect_cas(mut self, pem: impl Into<Vec<u8>>) -> Self {
        self.config.ca_cert = Some(Bytes::from(pem.into()));
        self
    }

    /// Set the TLS configuration for the agent.
    pub fn tls_config(mut self, config: rustls::ClientConfig) -> Self {
        self.config.tls_config = Some(config);
        self
    }

    /// Set a custom connector for the agent.
    pub fn connector(mut self, connector: impl Connector) -> Self {
        self.config.connector = Some(Arc::new(connector));
        self
    }

    /// Set the proxy URL for the agent.
    pub fn proxy_url(mut self, url: impl Into<String>) -> Self {
        self.config.proxy_url = Some(url.into());
        self
    }

    /// Set whether the agent should automatically connect on the first
    /// `listen` or `forward` call. Defaults to `true`.
    pub fn auto_connect(mut self, auto: bool) -> Self {
        self.config.auto_connect = auto;
        self
    }

    /// Set opaque metadata for the agent session.
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.config.metadata = Some(meta.into());
        self
    }

    /// Set a human-readable description for the agent.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.config.description = Some(desc.into());
        self
    }

    /// Set the heartbeat interval.
    pub fn heartbeat_interval(mut self, interval: Duration) -> Self {
        self.config.heartbeat_interval = Some(interval);
        self
    }

    /// Set the heartbeat tolerance.
    pub fn heartbeat_tolerance(mut self, tolerance: Duration) -> Self {
        self.config.heartbeat_tolerance = Some(tolerance);
        self
    }

    /// Add client type and version information.
    pub fn client_info(
        mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
        comments: Option<String>,
    ) -> Self {
        self.config
            .client_info
            .push((client_type.into(), version.into(), comments));
        self
    }

    /// Set a unified event handler for the agent.
    ///
    /// The handler is called for connection, disconnection, and heartbeat
    /// events.
    pub fn event_handler(mut self, handler: impl Fn(Event) + Send + Sync + 'static) -> Self {
        self.config.event_handler = Some(Arc::new(handler));
        self
    }

    /// Build the agent.
    ///
    /// This does NOT connect to the ngrok service. Call [`Agent::connect()`]
    /// explicitly or rely on auto-connect (the default) when calling
    /// [`Agent::listen()`] or [`Agent::forward()`].
    pub fn build(self) -> Result<Agent, NgrokError> {
        Ok(Agent {
            inner: Arc::new(AgentInner {
                config: self.config,
                session: Mutex::new(None),
            }),
        })
    }
}

/// Represents an active connection to the ngrok service.
pub struct AgentSession {
    session: Session,
    agent: Agent,
    started_at: Instant,
}

impl AgentSession {
    /// Returns the agent that owns this session.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Returns when this session was started.
    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Returns the internal session ID.
    pub fn id(&self) -> String {
        self.session.id()
    }
}
