//! Reconnecting session: dial loop with exponential backoff and automatic re-bind.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{
        Duration,
        SystemTime,
    },
};

use muxado::{
    Heartbeat,
    HeartbeatConfig,
    Session,
    SessionConfig,
    TypedStreamSession,
    heartbeat::HeartbeatResult,
};
use tokio::sync::{
    RwLock,
    mpsc,
    watch,
};

use crate::{
    error::Error,
    events::{
        AgentConnectSucceededEvent,
        AgentDisconnectedEvent,
        AgentHeartbeatReceivedEvent,
        Event,
    },
    proto::msg::{
        AuthExtra,
        AuthResp,
        BindReq,
        BindResp,
    },
    rpc::{
        RpcRequest,
        RpcResponse,
    },
    session::AgentSession,
    tunnel::raw_session::{
        ProxyStream,
        RawSession,
    },
};

/// Configuration for a bound tunnel, used for re-binding after reconnect.
#[derive(Clone, Debug)]
pub(crate) struct TunnelConfig {
    /// The bind request to replay.
    pub bind_req: BindReq,
    /// Channel to send accepted proxy streams to.
    pub accept_tx: mpsc::Sender<ProxyStream>,
}

/// Dial configuration for connecting to ngrok cloud.
#[derive(Clone)]
pub(crate) struct DialConfig {
    /// The connect address (host:port).
    pub connect_url: String,
    /// The TLS client config.
    pub tls_config: Arc<rustls::ClientConfig>,
    /// Heartbeat interval.
    pub heartbeat_interval: Duration,
    /// Heartbeat tolerance.
    pub heartbeat_tolerance: Duration,
}

/// Type alias for event handler list.
type EventHandlers = Arc<Vec<Box<dyn Fn(Event) + Send + Sync>>>;

/// Inner state shared between the reconnect task and the Agent.
pub(crate) struct ReconnectInner {
    /// Auth extra to send on each connection.
    pub auth_extra: RwLock<AuthExtra>,
    /// Bound tunnel configs to re-bind on reconnect.
    pub tunnels: RwLock<Vec<TunnelConfig>>,
    /// Tunnel accept senders keyed by endpoint ID.
    pub tunnel_senders: Arc<RwLock<HashMap<String, mpsc::Sender<ProxyStream>>>>,
    /// RPC handler.
    pub rpc_handler: Option<Arc<dyn Fn(RpcRequest) -> RpcResponse + Send + Sync>>,
    /// Event handlers.
    pub event_handlers: EventHandlers,
    /// Session cookie for reconnection.
    pub cookie: RwLock<String>,
    /// The current session info.
    pub session: RwLock<Option<AgentSession>>,
    /// The current raw session for sending bind requests.
    pub raw_session: RwLock<Option<RawSession>>,
    /// Whether the agent has been closed.
    pub closed: std::sync::atomic::AtomicBool,
    /// Dial configuration.
    pub dial_config: DialConfig,
    /// Notify when session is established.
    pub connected_tx: watch::Sender<bool>,
}

/// A reconnecting session that automatically re-dials and re-binds on failure.
pub(crate) struct ReconnectingSession {
    inner: Arc<ReconnectInner>,
    /// Handle for the reconnect loop task.
    _task: tokio::task::JoinHandle<()>,
}

impl ReconnectingSession {
    /// Start a new reconnecting session.
    pub fn start(inner: Arc<ReconnectInner>) -> Self {
        let task_inner = inner.clone();
        let task = tokio::spawn(async move {
            reconnect_loop(task_inner).await;
        });
        Self { inner, _task: task }
    }

    /// Bind a new tunnel and register it for re-binding.
    pub async fn bind(&self, mut config: TunnelConfig) -> Result<BindResp, Error> {
        // Wait for connection
        let mut rx = self.inner.connected_tx.subscribe();
        while !*rx.borrow() {
            rx.changed().await.map_err(|_| Error::NotConnected)?;
        }

        // Perform bind
        let resp = do_bind(&self.inner, &config).await?;

        // Save the assigned hostname so re-binds after reconnect get the same URL.
        if let Ok(url) = url::Url::parse(&resp.url)
            && let Some(host) = url.host_str()
            && let Some(obj) = config.bind_req.opts.as_object_mut()
        {
            obj.insert(
                "Hostname".into(),
                serde_json::Value::String(host.to_string()),
            );
        }

        // Register for re-bind
        {
            let mut senders = self.inner.tunnel_senders.write().await;
            senders.insert(resp.client_id.clone(), config.accept_tx.clone());
        }
        {
            let mut tunnels = self.inner.tunnels.write().await;
            tunnels.push(config);
        }

        Ok(resp)
    }
}

/// Perform a single bind operation using the current raw session.
async fn do_bind(inner: &ReconnectInner, config: &TunnelConfig) -> Result<BindResp, Error> {
    let raw = inner.raw_session.read().await;
    let raw = raw.as_ref().ok_or(Error::NotConnected)?;
    raw.bind(config.bind_req.clone()).await
}

/// The main reconnect loop.
async fn reconnect_loop(inner: Arc<ReconnectInner>) {
    let mut backoff = Duration::from_millis(500);
    let max_backoff = Duration::from_secs(30);

    loop {
        if inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        match dial_and_auth(&inner).await {
            Ok((raw_session, typed, auth_resp)) => {
                backoff = Duration::from_millis(500);

                // Store cookie for reconnection.
                {
                    let mut cookie = inner.cookie.write().await;
                    *cookie = auth_resp.extra.cookie.clone();
                }

                // Update auth extra with cookie.
                {
                    let mut extra = inner.auth_extra.write().await;
                    extra.cookie = auth_resp.extra.cookie.clone();
                }

                // Create session snapshot.
                let session = AgentSession {
                    id: auth_resp.extra.agent_session_id.clone(),
                    warnings: Vec::new(),
                    started_at: SystemTime::now(),
                };

                {
                    let mut s = inner.session.write().await;
                    *s = Some(session.clone());
                }

                // Store the raw session for user-initiated binds.
                {
                    let mut rs = inner.raw_session.write().await;
                    *rs = Some(raw_session);
                }

                // Re-bind all tunnels using the stored raw session.
                let tunnel_configs = {
                    let tunnels = inner.tunnels.read().await;
                    tunnels.clone()
                };

                let mut bind_failed = false;
                for config in &tunnel_configs {
                    match do_bind(&inner, config).await {
                        Ok(resp) => {
                            let mut senders = inner.tunnel_senders.write().await;
                            senders.insert(resp.client_id.clone(), config.accept_tx.clone());
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "failed to re-bind tunnel");
                            bind_failed = true;
                            break;
                        }
                    }
                }

                if bind_failed {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(max_backoff);
                    continue;
                }

                // Start heartbeat requester (no responder — the accept loop
                // handles heartbeat echo via Heartbeat::accept_typed_stream).
                let hb_handlers = inner.event_handlers.clone();
                let hb_session = session.clone();
                let config = &inner.dial_config;
                let heartbeat = Heartbeat::new(
                    typed,
                    Box::new(move |result| {
                        if let HeartbeatResult::Ok(latency) = result {
                            emit_event(
                                &hb_handlers,
                                Event::AgentHeartbeatReceived(AgentHeartbeatReceivedEvent {
                                    occurred_at: SystemTime::now(),
                                    session: hb_session.clone(),
                                    latency,
                                }),
                            );
                        }
                    }),
                    HeartbeatConfig {
                        interval: config.heartbeat_interval,
                        tolerance: config.heartbeat_tolerance,
                        ..Default::default()
                    },
                );
                // Start accept loop (uses Heartbeat::accept_typed_stream to
                // transparently echo heartbeats and yield only app streams).
                // The heartbeat requester is started inside the accept loop
                // since it takes ownership of the Heartbeat.
                let accept_handle = {
                    let rs = inner.raw_session.read().await;
                    let raw = rs.as_ref().expect("raw_session must be set after auth");
                    raw.start_accept_loop(
                        heartbeat,
                        inner.tunnel_senders.clone(),
                        inner.rpc_handler.clone(),
                    )
                };

                // Signal connected.
                let _ = inner.connected_tx.send(true);

                // Emit connect event.
                emit_event(
                    &inner.event_handlers,
                    Event::AgentConnectSucceeded(AgentConnectSucceededEvent {
                        occurred_at: SystemTime::now(),
                        session: session.clone(),
                    }),
                );

                // Wait for the accept loop to end (session dropped).
                accept_handle.await.ok();

                // Signal disconnected and clear raw session.
                let _ = inner.connected_tx.send(false);
                {
                    let mut rs = inner.raw_session.write().await;
                    *rs = None;
                }

                if !inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
                    emit_event(
                        &inner.event_handlers,
                        Event::AgentDisconnected(AgentDisconnectedEvent {
                            occurred_at: SystemTime::now(),
                            session: session.clone(),
                            error: Some("session dropped".into()),
                        }),
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, backoff = ?backoff, "dial/auth failed, retrying");
            }
        }

        if inner.closed.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Dial and authenticate to ngrok cloud.
async fn dial_and_auth(
    inner: &ReconnectInner,
) -> Result<(RawSession, Arc<TypedStreamSession>, AuthResp), Error> {
    let config = &inner.dial_config;

    // TCP connect.
    let tcp = tokio::net::TcpStream::connect(&config.connect_url)
        .await
        .map_err(Error::Io)?;

    // TLS handshake.
    let connector = tokio_rustls::TlsConnector::from(config.tls_config.clone());
    let server_name: rustls::pki_types::ServerName<'_> = config
        .connect_url
        .split(':')
        .next()
        .unwrap_or(&config.connect_url)
        .to_string()
        .try_into()
        .map_err(|_| Error::Io(std::io::Error::other("invalid server name")))?;

    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;

    // muxado session.
    let session = Session::client(tls, SessionConfig::default());
    let typed = Arc::new(TypedStreamSession::new(session));

    let raw = RawSession::new(typed.clone());

    // Authenticate.
    let extra = {
        let e = inner.auth_extra.read().await;
        e.clone()
    };
    let resp = raw.auth(extra).await?;

    Ok((raw, typed, resp))
}

/// Emit an event to all handlers.
fn emit_event(handlers: &[Box<dyn Fn(Event) + Send + Sync>], event: Event) {
    for handler in handlers {
        handler(event.clone());
    }
}
