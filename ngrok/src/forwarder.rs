//! Endpoint forwarder: auto-proxy connections to an upstream.

use std::future::Future;

use url::Url;

use crate::{
    error::Error,
    listener::EndpointListener,
    upstream::ProxyProtoVersion,
};

/// An auto-proxy handle that forwards connections from an ngrok endpoint to an upstream.
///
/// The SDK internally accepts connections and forwards them to the configured upstream;
/// no application code is required for I/O.
pub struct EndpointForwarder {
    /// The public URL.
    pub(crate) url: Url,
    /// The endpoint ID.
    pub(crate) id: String,
    /// The upstream URL.
    pub(crate) upstream_url: Url,
    /// The upstream protocol.
    pub(crate) upstream_protocol: Option<String>,
    /// The PROXY protocol version.
    pub(crate) proxy_proto: ProxyProtoVersion,
    /// Done signaling.
    pub(crate) done_rx: tokio::sync::watch::Receiver<bool>,
    /// Done sender.
    pub(crate) done_tx: tokio::sync::watch::Sender<bool>,
    /// The forwarding task handle.
    pub(crate) _task: tokio::task::JoinHandle<()>,
}

impl EndpointForwarder {
    /// The public URL assigned by ngrok cloud.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The endpoint ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The upstream URL.
    pub fn upstream_url(&self) -> &Url {
        &self.upstream_url
    }

    /// The upstream protocol, if set.
    pub fn upstream_protocol(&self) -> Option<&str> {
        self.upstream_protocol.as_deref()
    }

    /// The PROXY protocol version.
    pub fn proxy_protocol(&self) -> ProxyProtoVersion {
        self.proxy_proto
    }

    /// A future that resolves when this forwarder has been closed.
    pub fn done(&self) -> impl Future<Output = ()> + '_ {
        let mut rx = self.done_rx.clone();
        async move {
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    }

    /// Close the forwarder and stop forwarding connections.
    pub async fn close(self) -> Result<(), Error> {
        let _ = self.done_tx.send(true);
        Ok(())
    }
}

/// Spawn the forwarding loop task.
pub(crate) fn spawn_forward_loop(
    mut listener: EndpointListener,
    upstream_addr: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (stream, _addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            let addr = upstream_addr.clone();
            tokio::spawn(async move {
                if let Err(e) = forward_one(stream, &addr).await {
                    tracing::debug!(error = %e, "forward connection error");
                }
            });
        }
    })
}

/// Forward a single connection to the upstream.
async fn forward_one(
    mut ngrok_stream: crate::listener::NgrokStream,
    addr: &str,
) -> Result<(), Error> {
    let mut upstream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(Error::Io)?;

    tokio::io::copy_bidirectional(&mut ngrok_stream, &mut upstream)
        .await
        .map_err(Error::Io)?;

    Ok(())
}
