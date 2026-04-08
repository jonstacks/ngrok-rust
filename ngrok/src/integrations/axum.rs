//! axum integration: `axum::serve::Listener` implementation for `EndpointListener`.

use crate::listener::{
    EndpointListener,
    NgrokAddr,
    NgrokStream,
};

impl axum::serve::Listener for EndpointListener {
    type Io = NgrokStream;
    type Addr = NgrokAddr;

    /// Accept the next inbound connection.
    ///
    /// Transient accept errors (e.g. a single connection reset mid-handshake) are
    /// logged at `WARN` level and retried; fatal errors (endpoint closed) break the
    /// accept loop, causing `axum::serve` to return.
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match EndpointListener::accept(self).await {
                Ok((stream, addr)) => return (stream, addr),
                Err(e) => {
                    tracing::warn!(error = %e, "transient accept error; retrying");
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(NgrokAddr(self.url().to_string()))
    }
}
