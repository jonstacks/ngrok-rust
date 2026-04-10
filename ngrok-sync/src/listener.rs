//! Blocking endpoint listener wrapper.

use std::{
    sync::Arc,
    time::Duration,
};

use tokio::runtime::Runtime;
use url::Url;

/// Blocking wrapper around `ngrok::EndpointListener`.
///
/// The `accept()` method blocks until the next inbound connection arrives,
/// returning the async `ngrok::NgrokStream` type. This is a one-time `block_on`
/// (acceptable: one call per connection, not one per read). The caller drives
/// I/O by spawning a task on `agent.runtime()`.
pub struct EndpointListener {
    pub(crate) inner: ngrok::EndpointListener,
    pub(crate) rt: Arc<Runtime>,
}

impl EndpointListener {
    /// Block until the next inbound connection arrives.
    ///
    /// Returns `ngrok::NgrokStream` -- the async stream type. The caller is
    /// responsible for driving I/O on the returned stream; use `agent.runtime()`
    /// to spawn a data-plane task.
    pub fn accept(&mut self) -> Result<(ngrok::NgrokStream, ngrok::NgrokAddr), ngrok::Error> {
        self.rt.block_on(self.inner.accept())
    }

    /// Block until the next inbound connection arrives, with a timeout.
    ///
    /// Returns `ngrok::Error::Timeout` if the deadline elapses before a
    /// connection is available.
    pub fn accept_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<(ngrok::NgrokStream, ngrok::NgrokAddr), ngrok::Error> {
        self.rt.block_on(async {
            tokio::time::timeout(timeout, self.inner.accept())
                .await
                .map_err(|_| ngrok::Error::Timeout)?
        })
    }

    /// The public URL for this endpoint.
    pub fn url(&self) -> &Url {
        self.inner.url()
    }

    /// The endpoint ID.
    pub fn id(&self) -> &str {
        self.inner.id()
    }

    /// Close the listener and release the endpoint (blocking).
    pub fn close(self) -> Result<(), ngrok::Error> {
        self.rt.block_on(self.inner.close())
    }
}
