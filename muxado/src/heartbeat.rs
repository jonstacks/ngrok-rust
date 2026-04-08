use std::{
    sync::{
        Arc,
        atomic::{
            AtomicU64,
            Ordering,
        },
    },
    time::{
        Duration,
        Instant,
    },
};

use rand::Rng;
use tokio::{
    io::{
        AsyncReadExt,
        AsyncWriteExt,
    },
    sync::{
        Mutex,
        Notify,
    },
};
use tracing::{
    debug,
    trace,
    warn,
};

use crate::{
    error::MuxadoError,
    typed::{
        StreamType,
        TypedStream,
        TypedStreamSession,
    },
};

/// Default heartbeat stream type (matches muxado-go: 0xFFFFFFFF).
pub const DEFAULT_HEARTBEAT_TYPE: StreamType = 0xFFFF_FFFF;

/// Configuration for the heartbeat mechanism.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Time between heartbeat pings.
    pub interval: Duration,
    /// How long to wait for a pong before declaring timeout.
    pub tolerance: Duration,
    /// Stream type used for the heartbeat stream.
    pub stream_type: StreamType,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            tolerance: Duration::from_secs(15),
            stream_type: DEFAULT_HEARTBEAT_TYPE,
        }
    }
}

/// Result of a heartbeat check.
#[derive(Debug, Clone, Copy)]
pub enum HeartbeatResult {
    /// Successful pong with round-trip latency.
    Ok(Duration),
    /// Timed out waiting for pong.
    Timeout,
}

/// Callback type for heartbeat events.
pub type HeartbeatCallback = Box<dyn Fn(HeartbeatResult) + Send + Sync + 'static>;

/// A heartbeat manager that sends periodic pings over typed streams.
///
/// Supports both periodic heartbeats and on-demand `beat()` calls with
/// random nonce verification to detect stale connections.
pub struct Heartbeat {
    inner: Arc<HeartbeatInner>,
}

struct HeartbeatInner {
    session: Arc<TypedStreamSession>,
    callback: HeartbeatCallback,
    stop: Notify,
    /// Interval stored as atomic nanoseconds for runtime adjustment.
    interval_nanos: AtomicU64,
    /// Tolerance stored as atomic nanoseconds for runtime adjustment.
    tolerance_nanos: AtomicU64,
    stream_type: StreamType,
    /// Lock to serialize heartbeat requests (periodic + on-demand).
    beat_lock: Mutex<()>,
}

impl Heartbeat {
    /// Create a new heartbeat manager.
    pub fn new(
        session: Arc<TypedStreamSession>,
        callback: HeartbeatCallback,
        config: HeartbeatConfig,
    ) -> Self {
        Self {
            inner: Arc::new(HeartbeatInner {
                session,
                callback,
                stop: Notify::new(),
                interval_nanos: AtomicU64::new(config.interval.as_nanos() as u64),
                tolerance_nanos: AtomicU64::new(config.tolerance.as_nanos() as u64),
                stream_type: config.stream_type,
                beat_lock: Mutex::new(()),
            }),
        }
    }

    /// Start the heartbeat requester. Spawns a task that sends periodic
    /// pings. Returns a handle to stop the heartbeat.
    pub fn start(&self) -> HeartbeatHandle<'_> {
        let inner = self.inner.clone();

        let handle = tokio::spawn(async move {
            heartbeat_loop(inner).await;
        });

        HeartbeatHandle {
            _handle: handle,
            stop: &self.inner.stop,
        }
    }

    /// Start the heartbeat responder. This should be called on the side
    /// that accepts typed streams. It will echo back ping values.
    pub fn start_responder(&self) -> tokio::task::JoinHandle<()> {
        let inner = self.inner.clone();

        tokio::spawn(async move {
            heartbeat_responder(inner).await;
        })
    }

    /// Trigger an immediate out-of-band heartbeat.
    ///
    /// Returns `(latency, timed_out)`. Serialized with periodic heartbeats
    /// via an internal lock.
    pub async fn beat(&self) -> (Duration, bool) {
        let tolerance_nanos = self.inner.tolerance_nanos.load(Ordering::Relaxed);
        let tolerance = Duration::from_nanos(tolerance_nanos);

        let start = Instant::now();
        let result = tokio::time::timeout(tolerance, do_heartbeat(&self.inner)).await;

        match result {
            Ok(Ok(())) => {
                let latency = start.elapsed();
                debug!(latency_ms = latency.as_millis(), "on-demand heartbeat OK");
                (latency, false)
            }
            _ => {
                warn!("on-demand heartbeat timed out");
                (Duration::ZERO, true)
            }
        }
    }

    /// Accept a typed stream, filtering out heartbeat streams.
    ///
    /// Heartbeat streams are automatically echoed back. Non-heartbeat
    /// streams are returned to the caller.
    pub async fn accept_typed_stream(&self) -> Result<TypedStream, MuxadoError> {
        loop {
            let ts = self.inner.session.accept_typed().await?;
            if ts.stream_type == self.inner.stream_type {
                // Echo heartbeat stream
                tokio::spawn(echo_heartbeat(ts));
            } else {
                return Ok(ts);
            }
        }
    }

    /// Update the heartbeat interval at runtime.
    pub fn set_interval(&self, interval: Duration) {
        self.inner
            .interval_nanos
            .store(interval.as_nanos() as u64, Ordering::Relaxed);
    }

    /// Update the heartbeat tolerance at runtime.
    pub fn set_tolerance(&self, tolerance: Duration) {
        self.inner
            .tolerance_nanos
            .store(tolerance.as_nanos() as u64, Ordering::Relaxed);
    }
}

/// Handle returned by `Heartbeat::start()`. Dropping it stops the heartbeat.
pub struct HeartbeatHandle<'a> {
    _handle: tokio::task::JoinHandle<()>,
    stop: &'a Notify,
}

impl<'a> HeartbeatHandle<'a> {
    /// Stop the heartbeat.
    pub fn stop(&self) {
        self.stop.notify_waiters();
    }
}

impl<'a> Drop for HeartbeatHandle<'a> {
    fn drop(&mut self) {
        self.stop.notify_waiters();
    }
}

async fn heartbeat_loop(inner: Arc<HeartbeatInner>) {
    loop {
        let interval = Duration::from_nanos(inner.interval_nanos.load(Ordering::Relaxed));
        let tolerance = Duration::from_nanos(inner.tolerance_nanos.load(Ordering::Relaxed));

        tokio::select! {
            _ = inner.stop.notified() => return,
            _ = tokio::time::sleep(interval) => {}
        }

        let start = Instant::now();
        let result = tokio::time::timeout(tolerance, do_heartbeat(&inner)).await;

        match result {
            Ok(Ok(())) => {
                let latency = start.elapsed();
                trace!(latency_ms = latency.as_millis(), "heartbeat OK");
                (inner.callback)(HeartbeatResult::Ok(latency));
            }
            _ => {
                warn!("heartbeat timed out");
                (inner.callback)(HeartbeatResult::Timeout);
                return;
            }
        }
    }
}

/// Perform a single heartbeat: open typed stream, send random nonce,
/// verify echo matches.
async fn do_heartbeat(inner: &HeartbeatInner) -> Result<(), MuxadoError> {
    let _lock = inner.beat_lock.lock().await;

    let nonce: u32 = rand::rng().random();
    let mut stream = inner.session.open_typed(inner.stream_type).await?;

    stream
        .write_all(&nonce.to_be_bytes())
        .await
        .map_err(MuxadoError::Io)?;

    let mut pong_buf = [0u8; 4];
    stream
        .read_exact(&mut pong_buf)
        .await
        .map_err(MuxadoError::Io)?;

    let echo = u32::from_be_bytes(pong_buf);
    if echo != nonce {
        return Err(MuxadoError::Protocol {
            code: crate::error::ErrorCode::ProtocolError,
        });
    }

    trace!(nonce, "heartbeat nonce verified");
    Ok(())
}

async fn heartbeat_responder(inner: Arc<HeartbeatInner>) {
    loop {
        let ts = tokio::select! {
            _ = inner.stop.notified() => return,
            result = inner.session.accept_typed() => {
                match result {
                    Ok(ts) => ts,
                    Err(_) => return,
                }
            }
        };

        if ts.stream_type != inner.stream_type {
            continue;
        }

        // Echo pings back
        tokio::spawn(echo_heartbeat(ts));
    }
}

/// Echo a heartbeat stream: read 4-byte values and write them back.
async fn echo_heartbeat(ts: TypedStream) {
    let mut stream = ts.stream;
    let mut buf = [0u8; 4];
    loop {
        match stream.read_exact(&mut buf).await {
            Ok(_) => {
                if stream.write_all(&buf).await.is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::config::Config;

    fn make_heartbeat_pair() -> (Arc<TypedStreamSession>, Arc<TypedStreamSession>) {
        let (c, s) = tokio::io::duplex(64 * 1024);
        let client = Arc::new(TypedStreamSession::new(crate::session::Session::client(
            c,
            Config::default(),
        )));
        let server = Arc::new(TypedStreamSession::new(crate::session::Session::server(
            s,
            Config::default(),
        )));
        (client, server)
    }

    #[tokio::test]
    async fn heartbeat_ping_pong() {
        let (client, server) = make_heartbeat_pair();

        let got_pong = Arc::new(AtomicBool::new(false));
        let got_pong2 = got_pong.clone();

        let config = HeartbeatConfig {
            interval: Duration::from_millis(50),
            tolerance: Duration::from_millis(500),
            ..Default::default()
        };

        let hb = Heartbeat::new(
            client.clone(),
            Box::new(move |result| {
                if let HeartbeatResult::Ok(latency) = result {
                    assert!(latency < Duration::from_secs(1));
                    got_pong2.store(true, Ordering::Relaxed);
                }
            }),
            config.clone(),
        );

        // Start responder on server side
        let server_hb = Heartbeat::new(server.clone(), Box::new(|_| {}), config);
        let _responder = server_hb.start_responder();

        let handle = hb.start();

        // Wait a bit for at least one ping-pong
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert!(
            got_pong.load(Ordering::Relaxed),
            "should have received at least one pong"
        );
        handle.stop();
    }

    #[tokio::test]
    async fn heartbeat_timeout_on_no_responder() {
        let (client, _server) = make_heartbeat_pair();

        let got_timeout = Arc::new(AtomicBool::new(false));
        let got_timeout2 = got_timeout.clone();

        let config = HeartbeatConfig {
            interval: Duration::from_millis(10),
            tolerance: Duration::from_millis(50),
            ..Default::default()
        };

        let hb = Heartbeat::new(
            client.clone(),
            Box::new(move |result| {
                if let HeartbeatResult::Timeout = result {
                    got_timeout2.store(true, Ordering::Relaxed);
                }
            }),
            config,
        );

        let _handle = hb.start();

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(got_timeout.load(Ordering::Relaxed), "should have timed out");
    }

    #[tokio::test]
    async fn runtime_interval_adjustment() {
        let (client, server) = make_heartbeat_pair();

        let pong_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let pong_count2 = pong_count.clone();

        let config = HeartbeatConfig {
            interval: Duration::from_millis(500), // Start slow
            tolerance: Duration::from_millis(500),
            ..Default::default()
        };

        let hb = Heartbeat::new(
            client.clone(),
            Box::new(move |result| {
                if let HeartbeatResult::Ok(_) = result {
                    pong_count2.fetch_add(1, Ordering::Relaxed);
                }
            }),
            config.clone(),
        );

        let server_hb = Heartbeat::new(server.clone(), Box::new(|_| {}), config);
        let _responder = server_hb.start_responder();

        // Speed up the interval
        hb.set_interval(Duration::from_millis(20));
        let handle = hb.start();

        tokio::time::sleep(Duration::from_millis(200)).await;

        let count = pong_count.load(Ordering::Relaxed);
        assert!(
            count >= 2,
            "expected at least 2 pongs with fast interval, got {count}"
        );
        handle.stop();
    }

    #[tokio::test]
    async fn on_demand_beat() {
        let (client, server) = make_heartbeat_pair();

        let config = HeartbeatConfig {
            interval: Duration::from_secs(999), // Won't fire periodically
            tolerance: Duration::from_millis(500),
            ..Default::default()
        };

        let hb = Heartbeat::new(client.clone(), Box::new(|_| {}), config.clone());

        let server_hb = Heartbeat::new(server.clone(), Box::new(|_| {}), config);
        let _responder = server_hb.start_responder();

        let (latency, timed_out) = hb.beat().await;
        assert!(!timed_out, "on-demand beat should not time out");
        assert!(latency < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn on_demand_beat_timeout() {
        let (client, _server) = make_heartbeat_pair();

        let config = HeartbeatConfig {
            interval: Duration::from_secs(999),
            tolerance: Duration::from_millis(50),
            ..Default::default()
        };

        let hb = Heartbeat::new(client.clone(), Box::new(|_| {}), config);

        let (_latency, timed_out) = hb.beat().await;
        assert!(
            timed_out,
            "on-demand beat should time out without responder"
        );
    }

    #[tokio::test]
    async fn accept_typed_stream_filters_heartbeats() {
        let (client, server) = make_heartbeat_pair();

        let config = HeartbeatConfig::default();
        let hb = Heartbeat::new(server.clone(), Box::new(|_| {}), config);

        // Send a heartbeat-typed stream from client
        let mut hb_stream = client.open_typed(DEFAULT_HEARTBEAT_TYPE).await.unwrap();
        hb_stream.write_all(&42u32.to_be_bytes()).await.unwrap();

        // Send a normal typed stream
        let mut normal_stream = client.open_typed(0x42).await.unwrap();
        normal_stream.write_all(b"normal data").await.unwrap();

        // accept_typed_stream should skip the heartbeat and return the normal one
        let ts = hb.accept_typed_stream().await.unwrap();
        assert_eq!(ts.stream_type, 0x42);
    }
}
