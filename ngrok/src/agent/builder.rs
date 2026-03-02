use std::time::Duration;

use bytes::Bytes;
use futures_rustls::rustls;
use tokio::sync::RwLock;

use crate::session::{
    CommandHandler,
    Connector,
    HeartbeatHandler,
    InvalidHeartbeatInterval,
    InvalidHeartbeatTolerance,
    InvalidServerAddr,
    ProxyUnsupportedError,
    Restart,
    SessionBuilder,
    Stop,
    Update,
};

use super::{
    agent::Agent,
    events::EventHandler,
};

/// A builder for configuring and creating an [`Agent`].
///
/// # Example
///
/// ```rust,no_run
/// use ngrok::agent::Agent;
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let agent = Agent::builder()
///     .authtoken("your-authtoken")
///     .metadata("my-agent")
///     .auto_connect(true)
///     .build()?;
/// # Ok(())
/// # }
/// ```
pub struct AgentBuilder {
    session_builder: SessionBuilder,
    auto_connect: bool,
    event_handlers: Vec<EventHandler>,
}

/// Errors that can occur when building an agent.
#[derive(Debug, thiserror::Error)]
pub enum AgentBuildError {
    /// An invalid heartbeat interval was provided.
    #[error(transparent)]
    InvalidHeartbeatInterval(#[from] InvalidHeartbeatInterval),
    /// An invalid heartbeat tolerance was provided.
    #[error(transparent)]
    InvalidHeartbeatTolerance(#[from] InvalidHeartbeatTolerance),
    /// An invalid server address was provided.
    #[error(transparent)]
    InvalidServerAddr(#[from] InvalidServerAddr),
    /// An unsupported proxy URL was provided.
    #[error(transparent)]
    ProxyUnsupported(#[from] ProxyUnsupportedError),
}

impl Default for AgentBuilder {
    fn default() -> Self {
        AgentBuilder {
            session_builder: SessionBuilder::default(),
            auto_connect: true,
            event_handlers: Vec::new(),
        }
    }
}

impl AgentBuilder {
    /// Sets the authtoken for authentication with the ngrok service.
    ///
    /// See <https://ngrok.com/docs/ngrok-agent/config#authtoken>
    pub fn authtoken(mut self, token: impl Into<String>) -> Self {
        self.session_builder.authtoken(token);
        self
    }

    /// Sets the authtoken from the `NGROK_AUTHTOKEN` environment variable.
    pub fn authtoken_from_env(mut self) -> Self {
        self.session_builder.authtoken_from_env();
        self
    }

    /// Sets opaque, machine-readable metadata for the agent session.
    ///
    /// See <https://ngrok.com/docs/ngrok-agent/config#metadata>
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.session_builder.metadata(meta);
        self
    }

    /// Controls whether the agent will automatically connect when an endpoint
    /// is created via [`Agent::listen`] or [`Agent::forward`].
    ///
    /// Defaults to `true`.
    pub fn auto_connect(mut self, auto: bool) -> Self {
        self.auto_connect = auto;
        self
    }

    /// Sets how often the agent sends heartbeat messages to the ngrok service.
    ///
    /// See <https://ngrok.com/docs/ngrok-agent/config#heartbeat_interval>
    pub fn heartbeat_interval(
        mut self,
        interval: Duration,
    ) -> Result<Self, InvalidHeartbeatInterval> {
        self.session_builder.heartbeat_interval(interval)?;
        Ok(self)
    }

    /// Sets how long to wait for a heartbeat response before assuming
    /// the connection is dead.
    ///
    /// See <https://ngrok.com/docs/ngrok-agent/config#heartbeat_tolerance>
    pub fn heartbeat_tolerance(
        mut self,
        tolerance: Duration,
    ) -> Result<Self, InvalidHeartbeatTolerance> {
        self.session_builder.heartbeat_tolerance(tolerance)?;
        Ok(self)
    }

    /// Sets the network address to connect to the ngrok service.
    ///
    /// See <https://ngrok.com/docs/ngrok-agent/config#server_addr>
    pub fn server_addr(mut self, addr: impl Into<String>) -> Result<Self, InvalidServerAddr> {
        self.session_builder.server_addr(addr)?;
        Ok(self)
    }

    /// Sets the CA certificate for validating ngrok session TLS connections.
    pub fn ca_cert(mut self, ca_cert: Bytes) -> Self {
        self.session_builder.ca_cert(ca_cert);
        self
    }

    /// Sets a custom TLS configuration for connecting to the ngrok service.
    pub fn tls_config(mut self, config: rustls::ClientConfig) -> Self {
        self.session_builder.tls_config(config);
        self
    }

    /// Sets a custom connector for establishing connections to the ngrok service.
    pub fn connector(mut self, connect: impl Connector) -> Self {
        self.session_builder.connector(connect);
        self
    }

    /// Sets a proxy URL for connecting to the ngrok service.
    ///
    /// See <https://ngrok.com/docs/ngrok-agent/config#proxy_url>
    pub fn proxy_url(mut self, url: url::Url) -> Result<Self, ProxyUnsupportedError> {
        self.session_builder.proxy_url(url)?;
        Ok(self)
    }

    /// Registers an event handler to receive events from the agent.
    ///
    /// Multiple handlers can be registered by calling this method multiple times.
    /// Handlers must not block.
    pub fn event_handler(mut self, handler: EventHandler) -> Self {
        self.event_handlers.push(handler);
        self
    }

    /// Registers a handler for the ngrok stop command.
    pub fn handle_stop_command(mut self, handler: impl CommandHandler<Stop>) -> Self {
        self.session_builder.handle_stop_command(handler);
        self
    }

    /// Registers a handler for the ngrok restart command.
    pub fn handle_restart_command(mut self, handler: impl CommandHandler<Restart>) -> Self {
        self.session_builder.handle_restart_command(handler);
        self
    }

    /// Registers a handler for the ngrok update command.
    pub fn handle_update_command(mut self, handler: impl CommandHandler<Update>) -> Self {
        self.session_builder.handle_update_command(handler);
        self
    }

    /// Registers a heartbeat handler.
    pub fn handle_heartbeat(mut self, callback: impl HeartbeatHandler) -> Self {
        self.session_builder.handle_heartbeat(callback);
        self
    }

    /// Adds client type and version information.
    pub fn client_info(
        mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
        comments: Option<impl Into<String>>,
    ) -> Self {
        self.session_builder
            .client_info(client_type, version, comments);
        self
    }

    /// Builds the [`Agent`].
    ///
    /// This does not connect to the ngrok service. Call [`Agent::connect`]
    /// to establish a session, or set [`AgentBuilder::auto_connect`] to `true`
    /// (the default) to connect automatically when creating an endpoint.
    pub fn build(self) -> Result<Agent, AgentBuildError> {
        Ok(Agent {
            session_builder: self.session_builder,
            session: RwLock::new(None),
            auto_connect: self.auto_connect,
            event_handlers: self.event_handlers,
            endpoints: RwLock::new(Vec::new()),
        })
    }
}
