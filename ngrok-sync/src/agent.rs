//! Blocking agent wrapper.

use std::{
    sync::Arc,
    time::Duration,
};

use tokio::runtime::Runtime;

use crate::{
    forwarder::EndpointForwarder,
    listener::EndpointListener,
};

/// Builder for a blocking `Agent`.
pub struct AgentBuilder {
    inner: ngrok::AgentBuilder,
    rt: Option<Arc<Runtime>>,
}

impl AgentBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            inner: ngrok::Agent::builder(),
            rt: None,
        }
    }

    /// Inject a pre-built runtime (advanced -- for FFI adapters that share a runtime).
    pub fn runtime(mut self, rt: Arc<Runtime>) -> Self {
        self.rt = Some(rt);
        self
    }

    /// Set the authtoken.
    pub fn authtoken(mut self, token: impl Into<String>) -> Self {
        self.inner = self.inner.authtoken(token);
        self
    }

    /// Read the authtoken from the `NGROK_AUTHTOKEN` environment variable.
    pub fn authtoken_from_env(mut self) -> Self {
        self.inner = self.inner.authtoken_from_env();
        self
    }

    /// Set the connect URL.
    pub fn connect_url(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.connect_url(url);
        self
    }

    /// Set a custom CA certificate.
    pub fn connect_ca_cert(mut self, pem: &[u8]) -> Self {
        self.inner = self.inner.connect_ca_cert(pem);
        self
    }

    /// Set a custom TLS config.
    pub fn tls_config(mut self, cfg: rustls::ClientConfig) -> Self {
        self.inner = self.inner.tls_config(cfg);
        self
    }

    /// Set an HTTP/SOCKS proxy URL.
    pub fn proxy_url(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.proxy_url(url);
        self
    }

    /// Whether to auto-connect on build.
    pub fn auto_connect(mut self, auto: bool) -> Self {
        self.inner = self.inner.auto_connect(auto);
        self
    }

    /// Set the heartbeat interval.
    pub fn heartbeat_interval(mut self, d: Duration) -> Self {
        self.inner = self.inner.heartbeat_interval(d);
        self
    }

    /// Set the heartbeat tolerance.
    pub fn heartbeat_tolerance(mut self, d: Duration) -> Self {
        self.inner = self.inner.heartbeat_tolerance(d);
        self
    }

    /// Set client info.
    pub fn client_info(
        mut self,
        client_type: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        self.inner = self.inner.client_info(client_type, version);
        self
    }

    /// Set the agent description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.inner = self.inner.description(desc);
        self
    }

    /// Set the agent metadata.
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.inner = self.inner.metadata(meta);
        self
    }

    /// Register an event handler.
    pub fn on_event(mut self, handler: impl Fn(ngrok::Event) + Send + Sync + 'static) -> Self {
        self.inner = self.inner.on_event(handler);
        self
    }

    /// Register an RPC handler.
    pub fn on_rpc(
        mut self,
        handler: impl Fn(ngrok::RpcRequest) -> ngrok::RpcResponse + Send + Sync + 'static,
    ) -> Self {
        self.inner = self.inner.on_rpc(handler);
        self
    }

    /// Enable tracing.
    pub fn with_tracing(mut self) -> Self {
        self.inner = self.inner.with_tracing();
        self
    }

    /// Build the blocking `Agent`.
    ///
    /// Creates and owns a `tokio::Runtime` unless one was provided via `runtime()`.
    pub fn build(self) -> Result<Agent, ngrok::Error> {
        let rt = self
            .rt
            .unwrap_or_else(|| Arc::new(Runtime::new().expect("failed to create Tokio runtime")));
        let inner = rt.block_on(self.inner.build())?;
        Ok(Agent {
            inner: Arc::new(inner),
            rt,
        })
    }
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Blocking ngrok agent handle.
///
/// Wraps the async `ngrok::Agent` with a `tokio::Runtime` for blocking operations.
pub struct Agent {
    inner: Arc<ngrok::Agent>,
    rt: Arc<Runtime>,
}

impl Agent {
    /// Create a new builder.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    /// Expose the shared runtime so FFI adapters can spawn data-plane tasks on it.
    pub fn runtime(&self) -> Arc<Runtime> {
        Arc::clone(&self.rt)
    }

    /// Connect the agent to ngrok (blocking).
    pub fn connect(&self) -> Result<(), ngrok::Error> {
        self.rt.block_on(self.inner.connect())
    }

    /// Gracefully disconnect (blocking).
    pub fn disconnect(&self) -> Result<(), ngrok::Error> {
        self.rt.block_on(self.inner.disconnect())
    }

    /// Return the current session info, if connected.
    pub fn session(&self) -> Option<ngrok::AgentSession> {
        self.inner.session()
    }

    /// List currently active endpoints.
    pub fn endpoints(&self) -> Vec<ngrok::EndpointInfo> {
        self.inner.endpoints()
    }

    /// Open a new listener endpoint (blocking).
    pub fn listen(&self, opts: ngrok::EndpointOptions) -> Result<EndpointListener, ngrok::Error> {
        let listener = self.rt.block_on(self.inner.listen(opts))?;
        Ok(EndpointListener {
            inner: listener,
            rt: Arc::clone(&self.rt),
        })
    }

    /// Start forwarding traffic to an upstream service (blocking).
    pub fn forward(
        &self,
        upstream: ngrok::Upstream,
        opts: ngrok::EndpointOptions,
    ) -> Result<EndpointForwarder, ngrok::Error> {
        let fwd = self.rt.block_on(self.inner.forward(upstream, opts))?;
        Ok(EndpointForwarder {
            inner: fwd,
            rt: Arc::clone(&self.rt),
        })
    }
}
