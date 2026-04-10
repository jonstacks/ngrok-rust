//! Raw session: Auth, Bind, and proxy stream dispatch over muxado.

use std::{
    collections::HashMap,
    sync::Arc,
};

use muxado::{
    Heartbeat,
    TypedStream,
    TypedStreamSession,
};
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    sync::mpsc,
};

use crate::{
    error::Error,
    proto::msg::{
        Auth,
        AuthExtra,
        AuthResp,
        BindReq,
        BindResp,
        ProxyHeader,
        SrvInfoResp,
        StopTunnel,
    },
    rpc::{
        RpcMethod,
        RpcRequest,
        RpcResponse,
    },
};

/// Stream type codes used in the ngrok protocol.
const AUTH_REQ: u32 = 0;
const BIND_REQ: u32 = 1;
const PROXY_REQ: u32 = 3;
const RESTART_REQ: u32 = 4;
const STOP_REQ: u32 = 5;
const UPDATE_REQ: u32 = 6;
const SRV_INFO_REQ: u32 = 8;
const STOP_TUNNEL_REQ: u32 = 9;

/// A proxy stream with its header metadata.
pub(crate) struct ProxyStream {
    /// The proxy header for this connection.
    pub header: ProxyHeader,
    /// The underlying typed muxado stream.
    pub stream: TypedStream,
}

/// Raw session over a muxado typed stream session.
pub(crate) struct RawSession {
    typed: Arc<TypedStreamSession>,
}

impl RawSession {
    /// Create a new raw session from a typed stream session.
    pub fn new(typed: Arc<TypedStreamSession>) -> Self {
        Self { typed }
    }

    /// Authenticate with ngrok cloud.
    pub async fn auth(&self, extra: AuthExtra) -> Result<AuthResp, Error> {
        let auth = Auth {
            version: vec!["3".into(), "2".into()],
            client_id: String::new(),
            extra,
        };

        let mut stream = self.typed.open_typed(AUTH_REQ).await?;
        let json = serde_json::to_vec(&auth)?;
        stream.write_all(&json).await.map_err(Error::Io)?;
        stream.close_write();

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.map_err(Error::Io)?;
        let resp: AuthResp = serde_json::from_slice(&buf)?;

        if !resp.error.is_empty() {
            return Err(Error::Cloud {
                code: String::new(),
                message: resp.error,
            });
        }

        Ok(resp)
    }

    /// Bind a tunnel endpoint.
    pub async fn bind(&self, req: BindReq) -> Result<BindResp, Error> {
        let mut stream = self.typed.open_typed(BIND_REQ).await?;
        let json = serde_json::to_vec(&req)?;
        stream.write_all(&json).await.map_err(Error::Io)?;
        stream.close_write();

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.map_err(Error::Io)?;
        let resp: BindResp = serde_json::from_slice(&buf)?;

        if !resp.error.is_empty() {
            return Err(Error::Cloud {
                code: String::new(),
                message: resp.error,
            });
        }

        Ok(resp)
    }

    /// Start the accept loop that dispatches incoming typed streams.
    ///
    /// Uses the `Heartbeat` wrapper to accept streams, which transparently
    /// handles heartbeat echo responses inline. This avoids a separate
    /// responder task that would race for the accept channel.
    ///
    /// Returns a join handle for the accept loop task.
    pub fn start_accept_loop(
        &self,
        heartbeat: Heartbeat,
        tunnels: Arc<tokio::sync::RwLock<HashMap<String, mpsc::Sender<ProxyStream>>>>,
        rpc_handler: Option<Arc<dyn Fn(RpcRequest) -> RpcResponse + Send + Sync>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Start the periodic heartbeat requester. The handle keeps it alive
            // for the duration of the accept loop.
            let _hb_handle = heartbeat.start();
            loop {
                let stream = match heartbeat.accept_typed_stream().await {
                    Ok(s) => s,
                    Err(_) => break,
                };

                match stream.stream_type {
                    PROXY_REQ => {
                        let tunnels = tunnels.clone();
                        tokio::spawn(async move {
                            handle_proxy(stream, tunnels).await;
                        });
                    }
                    STOP_REQ => {
                        if let Some(ref handler) = rpc_handler {
                            let req = RpcRequest {
                                method: RpcMethod::StopAgent,
                            };
                            let _resp = handler(req);
                        }
                    }
                    RESTART_REQ => {
                        if let Some(ref handler) = rpc_handler {
                            let req = RpcRequest {
                                method: RpcMethod::RestartAgent,
                            };
                            let _resp = handler(req);
                        }
                    }
                    UPDATE_REQ => {
                        if let Some(ref handler) = rpc_handler {
                            let req = RpcRequest {
                                method: RpcMethod::UpdateAgent,
                            };
                            let _resp = handler(req);
                        }
                    }
                    STOP_TUNNEL_REQ => {
                        tokio::spawn(async move {
                            handle_stop_tunnel(stream).await;
                        });
                    }
                    other => {
                        tracing::warn!(stream_type = other, "unknown typed stream received");
                    }
                }
            }
        })
    }
}

/// Handle an incoming proxy request stream.
async fn handle_proxy(
    mut stream: TypedStream,
    tunnels: Arc<tokio::sync::RwLock<HashMap<String, mpsc::Sender<ProxyStream>>>>,
) {
    // Read the proxy header: 8-byte little-endian length prefix, then JSON.
    let size = match stream.read_i64_le().await {
        Ok(s) => s as usize,
        Err(_) => return,
    };
    let mut buf = vec![0u8; size];
    if stream.read_exact(&mut buf).await.is_err() {
        return;
    }

    let header: ProxyHeader = match serde_json::from_slice(&buf) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(error = %e, "failed to parse proxy header");
            return;
        }
    };

    let endpoint_id = header.id.clone();
    let tunnels = tunnels.read().await;
    if let Some(tx) = tunnels.get(&endpoint_id) {
        let _ = tx.send(ProxyStream { header, stream }).await;
    } else {
        tracing::warn!(endpoint_id, "no tunnel found for proxy request");
    }
}

/// Handle a stop tunnel request.
async fn handle_stop_tunnel(mut stream: TypedStream) {
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).await.is_ok()
        && let Ok(msg) = serde_json::from_slice::<StopTunnel>(&buf)
    {
        tracing::info!(
            endpoint_id = msg.id,
            message = msg.message,
            "stop tunnel request"
        );
    }
}

/// Perform a SrvInfo exchange over a typed stream session.
pub(crate) async fn srv_info(typed: &TypedStreamSession) -> Result<String, Error> {
    let mut stream = typed.open_typed(SRV_INFO_REQ).await?;
    stream.write_all(b"{}").await.map_err(Error::Io)?;
    stream.close_write();

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.map_err(Error::Io)?;
    let resp: SrvInfoResp = serde_json::from_slice(&buf)?;

    Ok(resp.region)
}
