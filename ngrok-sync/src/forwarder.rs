//! Blocking endpoint forwarder wrapper.

use std::sync::Arc;

use tokio::runtime::Runtime;
use url::Url;

/// Blocking wrapper around `ngrok::EndpointForwarder`.
pub struct EndpointForwarder {
    pub(crate) inner: ngrok::EndpointForwarder,
    pub(crate) rt: Arc<Runtime>,
}

impl EndpointForwarder {
    /// The public URL for this forwarded endpoint.
    pub fn url(&self) -> &Url {
        self.inner.url()
    }

    /// The endpoint ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }

    /// Stop forwarding and release the endpoint (blocking).
    pub fn close(self) -> Result<(), ngrok::Error> {
        self.rt.block_on(self.inner.close())
    }
}
