use std::{
    pin::Pin,
    task::{
        Context,
        Poll,
    },
};

use futures::Stream;
use tokio::sync::watch;

use crate::{
    agent::Agent,
    conn::EndpointConn,
    error::NgrokError,
    tunnel::{
        AcceptError,
        EndpointInfo as TunnelEndpointInfo,
        HttpTunnel,
        TcpTunnel,
        TlsTunnel,
        TunnelCloser,
        TunnelInfo,
    },
};

/// Common interface for all ngrok endpoints.
pub trait Endpoint: Send + Sync {
    /// Returns the endpoint's unique ID.
    fn id(&self) -> &str;
    /// Returns the endpoint's public URL.
    fn url(&self) -> &str;
    /// Returns the endpoint's protocol (e.g. "https", "tcp", "tls").
    fn protocol(&self) -> &str;
    /// Returns the endpoint's metadata.
    fn metadata(&self) -> &str;
    /// Returns the endpoint's description.
    fn description(&self) -> &str;
    /// Returns the endpoint's traffic policy.
    fn traffic_policy(&self) -> &str;
    /// Returns the endpoint's bindings.
    fn bindings(&self) -> &[String];
    /// Returns whether connection pooling is enabled.
    fn pooling_enabled(&self) -> bool;
}

/// The inner tunnel type, erasing the protocol-specific type.
pub(crate) enum TunnelInner {
    Http(HttpTunnel),
    Tcp(TcpTunnel),
    Tls(TlsTunnel),
}

/// An endpoint listener that accepts incoming connections.
///
/// Implements `Stream<Item = Result<EndpointConn, NgrokError>>` to accept
/// connections.
pub struct EndpointListener {
    pub(crate) inner: TunnelInner,
    pub(crate) agent: Agent,
    pub(crate) traffic_policy: String,
    pub(crate) description: String,
    pub(crate) bindings: Vec<String>,
    pub(crate) pooling_enabled: bool,
    pub(crate) closed_tx: watch::Sender<bool>,
    pub(crate) closed_rx: watch::Receiver<bool>,
}

impl Endpoint for EndpointListener {
    fn id(&self) -> &str {
        match &self.inner {
            TunnelInner::Http(t) => TunnelInfo::id(t),
            TunnelInner::Tcp(t) => TunnelInfo::id(t),
            TunnelInner::Tls(t) => TunnelInfo::id(t),
        }
    }

    fn url(&self) -> &str {
        match &self.inner {
            TunnelInner::Http(t) => TunnelEndpointInfo::url(t),
            TunnelInner::Tcp(t) => TunnelEndpointInfo::url(t),
            TunnelInner::Tls(t) => TunnelEndpointInfo::url(t),
        }
    }

    fn protocol(&self) -> &str {
        match &self.inner {
            TunnelInner::Http(t) => TunnelEndpointInfo::proto(t),
            TunnelInner::Tcp(t) => TunnelEndpointInfo::proto(t),
            TunnelInner::Tls(t) => TunnelEndpointInfo::proto(t),
        }
    }

    fn metadata(&self) -> &str {
        match &self.inner {
            TunnelInner::Http(t) => TunnelInfo::metadata(t),
            TunnelInner::Tcp(t) => TunnelInfo::metadata(t),
            TunnelInner::Tls(t) => TunnelInfo::metadata(t),
        }
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn traffic_policy(&self) -> &str {
        &self.traffic_policy
    }

    fn bindings(&self) -> &[String] {
        &self.bindings
    }

    fn pooling_enabled(&self) -> bool {
        self.pooling_enabled
    }
}

impl EndpointListener {
    /// Returns a reference to the agent that created this endpoint.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Close this endpoint listener.
    pub async fn close(&mut self) -> Result<(), NgrokError> {
        match &mut self.inner {
            TunnelInner::Http(t) => t.close().await.map_err(NgrokError::from)?,
            TunnelInner::Tcp(t) => t.close().await.map_err(NgrokError::from)?,
            TunnelInner::Tls(t) => t.close().await.map_err(NgrokError::from)?,
        }
        let _ = self.closed_tx.send(true);
        Ok(())
    }

    /// Returns a future that resolves when this endpoint is closed.
    pub async fn closed(&self) {
        let mut rx = self.closed_rx.clone();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Stream for EndpointListener {
    type Item = Result<EndpointConn, NgrokError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let poll: Poll<Option<Result<EndpointConn, AcceptError>>> = match &mut self.inner {
            TunnelInner::Http(t) => Pin::new(t).poll_next(cx),
            TunnelInner::Tcp(t) => Pin::new(t).poll_next(cx),
            TunnelInner::Tls(t) => Pin::new(t).poll_next(cx),
        };
        poll.map(|opt| opt.map(|res| res.map_err(NgrokError::from)))
    }
}

/// An endpoint forwarder that accepts connections and forwards them to an
/// upstream URL.
pub struct EndpointForwarder {
    pub(crate) id: String,
    pub(crate) url: String,
    pub(crate) protocol: String,
    pub(crate) metadata_val: String,
    pub(crate) upstream_url: String,
    pub(crate) upstream_protocol: Option<String>,
    pub(crate) agent: Agent,
    pub(crate) traffic_policy: String,
    pub(crate) description: String,
    pub(crate) bindings: Vec<String>,
    pub(crate) pooling_enabled: bool,
    pub(crate) join: tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
    pub(crate) closed_tx: watch::Sender<bool>,
    pub(crate) closed_rx: watch::Receiver<bool>,
}

impl Endpoint for EndpointForwarder {
    fn id(&self) -> &str {
        &self.id
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn protocol(&self) -> &str {
        &self.protocol
    }

    fn metadata(&self) -> &str {
        &self.metadata_val
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn traffic_policy(&self) -> &str {
        &self.traffic_policy
    }

    fn bindings(&self) -> &[String] {
        &self.bindings
    }

    fn pooling_enabled(&self) -> bool {
        self.pooling_enabled
    }
}

impl EndpointForwarder {
    /// Returns a reference to the agent that created this endpoint.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Returns the upstream URL this forwarder is forwarding to.
    pub fn upstream_url(&self) -> &str {
        &self.upstream_url
    }

    /// Returns the upstream protocol, if any.
    pub fn upstream_protocol(&self) -> Option<&str> {
        self.upstream_protocol.as_deref()
    }

    /// Close this endpoint forwarder.
    pub async fn close(&mut self) -> Result<(), NgrokError> {
        self.join.abort();
        let _ = self.closed_tx.send(true);
        Ok(())
    }

    /// Returns a future that resolves when this endpoint is closed.
    pub async fn closed(&self) {
        let mut rx = self.closed_rx.clone();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Wait for the forwarding task to exit.
    pub fn join(
        &mut self,
    ) -> &mut tokio::task::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        &mut self.join
    }
}
