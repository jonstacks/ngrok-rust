//! FFI-safe types for embedding ngrok in language wrappers.
//!
//! This module is gated behind the `ffi` feature flag.

use tokio::sync::broadcast;

use crate::{
    agent::Agent,
    endpoint::{
        Endpoint,
        EndpointForwarder,
        EndpointListener,
    },
    error::NgrokError,
    event::Event,
    upstream::Upstream,
};

/// FFI-safe error type that serializes cleanly across any FFI boundary.
#[derive(Debug, Clone)]
pub struct FfiError {
    /// The ngrok error code, if any.
    pub code: Option<String>,
    /// The error message.
    pub message: String,
}

impl std::fmt::Display for FfiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref code) = self.code {
            write!(f, "{}: {}", code, self.message)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for FfiError {}

impl From<NgrokError> for FfiError {
    fn from(err: NgrokError) -> Self {
        FfiError {
            code: err.code().map(String::from),
            message: err.to_string(),
        }
    }
}

/// Concrete, FFI-safe endpoint options. No generics, no `Into<>`, all `String`
/// fields.
#[derive(Debug, Clone, Default)]
pub struct FfiEndpointOptions {
    /// The URL for the endpoint.
    pub url: Option<String>,
    /// The traffic policy (YAML or JSON).
    pub traffic_policy: Option<String>,
    /// Opaque metadata.
    pub metadata: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Bindings.
    pub bindings: Vec<String>,
    /// Whether pooling is enabled.
    pub pooling_enabled: Option<bool>,
}

/// Concrete, FFI-safe upstream configuration.
#[derive(Debug, Clone)]
pub struct FfiUpstream {
    /// The upstream address (e.g. "localhost:8080").
    pub addr: String,
    /// The upstream protocol.
    pub protocol: Option<String>,
    /// Whether to verify upstream TLS certificates.
    pub verify_upstream_tls: bool,
}

impl Default for FfiUpstream {
    fn default() -> Self {
        Self {
            addr: String::new(),
            protocol: None,
            verify_upstream_tls: true,
        }
    }
}

/// FFI-safe Agent wrapper. Clone is cheap (Arc-based).
#[derive(Clone)]
pub struct FfiAgent {
    inner: Agent,
    event_tx: broadcast::Sender<Event>,
}

impl FfiAgent {
    /// Create a new FFI agent from an existing Agent.
    pub fn new(agent: Agent) -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            inner: agent,
            event_tx,
        }
    }

    /// Create a new FFI agent with the given authtoken.
    pub fn with_authtoken(authtoken: &str) -> Result<Self, FfiError> {
        let agent = Agent::builder()
            .authtoken(authtoken)
            .build()
            .map_err(FfiError::from)?;
        Ok(Self::new(agent))
    }

    /// Create a new FFI agent that reads the authtoken from the environment.
    pub fn from_env() -> Result<Self, FfiError> {
        let agent = Agent::builder()
            .authtoken_from_env()
            .build()
            .map_err(FfiError::from)?;
        Ok(Self::new(agent))
    }

    /// Connect to the ngrok service.
    pub async fn connect(&self) -> Result<(), FfiError> {
        self.inner.connect().await.map_err(FfiError::from)
    }

    /// Listen for incoming connections with the given options.
    pub async fn listen(&self, opts: FfiEndpointOptions) -> Result<FfiEndpointListener, FfiError> {
        let mut builder = self.inner.listen();
        if let Some(ref url) = opts.url {
            builder = builder.url(url);
        }
        if let Some(ref tp) = opts.traffic_policy {
            builder = builder.traffic_policy(tp);
        }
        if let Some(ref meta) = opts.metadata {
            builder = builder.metadata(meta);
        }
        if let Some(ref desc) = opts.description {
            builder = builder.description(desc);
        }
        if !opts.bindings.is_empty() {
            builder = builder.bindings(opts.bindings.iter().map(|s| s.as_str()));
        }
        if let Some(pe) = opts.pooling_enabled {
            builder = builder.pooling_enabled(pe);
        }
        let listener = builder.start().await.map_err(FfiError::from)?;
        Ok(FfiEndpointListener::new(listener))
    }

    /// Forward connections to the given upstream with the given options.
    pub async fn forward(
        &self,
        upstream: FfiUpstream,
        opts: FfiEndpointOptions,
    ) -> Result<FfiEndpointForwarder, FfiError> {
        let mut up = Upstream::new(&upstream.addr);
        if let Some(ref proto) = upstream.protocol {
            up = up.protocol(proto);
        }
        up = up.verify_upstream_tls(upstream.verify_upstream_tls);

        let mut builder = self.inner.forward(up);
        if let Some(ref url) = opts.url {
            builder = builder.url(url);
        }
        if let Some(ref tp) = opts.traffic_policy {
            builder = builder.traffic_policy(tp);
        }
        if let Some(ref meta) = opts.metadata {
            builder = builder.metadata(meta);
        }
        if let Some(ref desc) = opts.description {
            builder = builder.description(desc);
        }
        if !opts.bindings.is_empty() {
            builder = builder.bindings(opts.bindings.iter().map(|s| s.as_str()));
        }
        if let Some(pe) = opts.pooling_enabled {
            builder = builder.pooling_enabled(pe);
        }
        let forwarder = builder.start().await.map_err(FfiError::from)?;
        Ok(FfiEndpointForwarder::new(forwarder))
    }

    /// Forward connections to the given address with the given options.
    pub async fn forward_to(
        &self,
        addr: &str,
        opts: FfiEndpointOptions,
    ) -> Result<FfiEndpointForwarder, FfiError> {
        let upstream = FfiUpstream {
            addr: addr.to_string(),
            ..Default::default()
        };
        self.forward(upstream, opts).await
    }

    /// Get an event receiver for channel-based event delivery.
    pub fn event_receiver(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }
}

/// FFI-safe endpoint listener wrapper.
pub struct FfiEndpointListener {
    inner: EndpointListener,
}

impl FfiEndpointListener {
    pub(crate) fn new(inner: EndpointListener) -> Self {
        Self { inner }
    }

    /// Returns the endpoint's unique ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }

    /// Returns the endpoint's public URL.
    pub fn url(&self) -> &str {
        self.inner.url()
    }

    /// Returns the endpoint's protocol.
    pub fn protocol(&self) -> &str {
        self.inner.protocol()
    }

    /// Returns the endpoint's metadata.
    pub fn metadata(&self) -> &str {
        self.inner.metadata()
    }

    /// Close this endpoint listener.
    pub async fn close(&mut self) -> Result<(), FfiError> {
        self.inner.close().await.map_err(FfiError::from)
    }

    /// Returns a one-shot receiver that resolves when this endpoint is closed.
    pub fn closed_receiver(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut closed_rx = self.inner.closed_rx.clone();
        tokio::spawn(async move {
            while !*closed_rx.borrow_and_update() {
                if closed_rx.changed().await.is_err() {
                    break;
                }
            }
            let _ = tx.send(());
        });
        rx
    }
}

/// FFI-safe endpoint forwarder wrapper.
pub struct FfiEndpointForwarder {
    inner: EndpointForwarder,
}

impl FfiEndpointForwarder {
    pub(crate) fn new(inner: EndpointForwarder) -> Self {
        Self { inner }
    }

    /// Returns the endpoint's unique ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }

    /// Returns the endpoint's public URL.
    pub fn url(&self) -> &str {
        self.inner.url()
    }

    /// Returns the upstream URL.
    pub fn upstream_url(&self) -> &str {
        self.inner.upstream_url()
    }

    /// Close this endpoint forwarder.
    pub async fn close(&mut self) -> Result<(), FfiError> {
        self.inner.close().await.map_err(FfiError::from)
    }

    /// Returns a one-shot receiver that resolves when this endpoint is closed.
    pub fn closed_receiver(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut closed_rx = self.inner.closed_rx.clone();
        tokio::spawn(async move {
            while !*closed_rx.borrow_and_update() {
                if closed_rx.changed().await.is_err() {
                    break;
                }
            }
            let _ = tx.send(());
        });
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffi_error_from_ngrok_error() {
        let err = NgrokError::Api {
            code: Some("ERR_123".into()),
            message: "test".into(),
        };
        let ffi_err = FfiError::from(err);
        assert_eq!(ffi_err.code.as_deref(), Some("ERR_123"));
        assert_eq!(ffi_err.message, "test");
    }

    #[test]
    fn test_ffi_error_display() {
        let err = FfiError {
            code: Some("ERR_1".into()),
            message: "msg".into(),
        };
        assert_eq!(err.to_string(), "ERR_1: msg");

        let err = FfiError {
            code: None,
            message: "msg".into(),
        };
        assert_eq!(err.to_string(), "msg");
    }

    #[test]
    fn test_ffi_endpoint_options_default() {
        let opts = FfiEndpointOptions::default();
        assert!(opts.url.is_none());
        assert!(opts.traffic_policy.is_none());
        assert!(opts.bindings.is_empty());
    }

    #[test]
    fn test_ffi_upstream_default() {
        let up = FfiUpstream::default();
        assert!(up.addr.is_empty());
        assert!(up.verify_upstream_tls);
    }

    #[test]
    fn test_ffi_agent_from_env() {
        let agent = FfiAgent::from_env();
        assert!(agent.is_ok());
    }
}
