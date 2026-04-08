//! Connectivity diagnostics.

use std::time::Duration;

use crate::error::DiagnoseError;

/// The result of a successful connectivity probe.
#[derive(Debug, Clone)]
pub struct DiagnoseResult {
    /// The address that was probed.
    pub addr: String,
    /// The ngrok region.
    pub region: String,
    /// The measured round-trip latency.
    pub latency: Duration,
}

/// Run a connectivity probe to the given address.
///
/// Returns `Ok(DiagnoseResult)` if TCP, TLS, and muxado all succeed.
/// Returns `Err` identifying which layer failed.
pub async fn diagnose(
    addr: &str,
    tls_config: rustls::ClientConfig,
) -> Result<DiagnoseResult, DiagnoseError> {
    use std::sync::Arc;

    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;

    let t0 = tokio::time::Instant::now();

    // TCP connect.
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|source| DiagnoseError::Tcp {
            addr: addr.to_string(),
            source,
        })?;

    // TLS handshake.
    let connector = TlsConnector::from(Arc::new(tls_config));
    let server_name = addr
        .split(':')
        .next()
        .unwrap_or(addr)
        .to_string()
        .try_into()
        .unwrap_or_else(|_| rustls::pki_types::ServerName::try_from("localhost").unwrap());

    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| DiagnoseError::Tls {
            addr: addr.to_string(),
            source: rustls::Error::General(e.to_string()),
        })?;

    // muxado session + SrvInfo.
    let session = muxado::Session::client(tls, muxado::SessionConfig::default());
    let typed = muxado::TypedStreamSession::new(session);

    let region = crate::tunnel::raw_session::srv_info(&typed)
        .await
        .map_err(|e| DiagnoseError::Muxado(e.to_string()))?;

    let latency = t0.elapsed();

    Ok(DiagnoseResult {
        addr: addr.to_string(),
        region,
        latency,
    })
}
