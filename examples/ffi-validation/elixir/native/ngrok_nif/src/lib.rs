//! Rustler NIF validation — proves ngrok::ffi types can back an Elixir NIF.
//!
//! This crate does NOT depend on rustler (which requires Erlang/OTP).
//! Instead it validates that FFI types compile and have the right shape
//! for NIF wrapping: concrete structs, no generics, Send + Sync.

use std::sync::Arc;

use ngrok::ffi::*;

/// Simulates a NIF resource holding an agent.
pub struct AgentResource {
    agent: Arc<FfiAgent>,
}

impl AgentResource {
    pub fn new(authtoken: &str) -> Result<Self, FfiError> {
        let agent = FfiAgent::with_authtoken(authtoken)?;
        Ok(Self {
            agent: Arc::new(agent),
        })
    }

    pub fn from_env() -> Result<Self, FfiError> {
        let agent = FfiAgent::from_env()?;
        Ok(Self {
            agent: Arc::new(agent),
        })
    }
}

// Prove the resource is Send + Sync (required for NIF resources)
fn _assert_send_sync<T: Send + Sync>() {}
fn _static_assertions() {
    _assert_send_sync::<AgentResource>();
    _assert_send_sync::<FfiAgent>();
    _assert_send_sync::<FfiError>();
}

/// Simulates a NIF that creates a listener and returns info as a tuple.
pub async fn nif_listen(
    resource: &AgentResource,
    url: Option<String>,
) -> Result<(String, String, String), FfiError> {
    let opts = FfiEndpointOptions {
        url,
        ..Default::default()
    };
    let listener = resource.agent.listen(opts).await?;
    Ok((
        listener.id().to_string(),
        listener.url().to_string(),
        listener.protocol().to_string(),
    ))
}

/// Simulates a NIF that creates a forwarder and returns info.
pub async fn nif_forward(
    resource: &AgentResource,
    upstream: &str,
    url: Option<String>,
) -> Result<(String, String, String), FfiError> {
    let opts = FfiEndpointOptions {
        url,
        ..Default::default()
    };
    let up = FfiUpstream {
        addr: upstream.to_string(),
        ..Default::default()
    };
    let fwd = resource.agent.forward(up, opts).await?;
    Ok((
        fwd.id().to_string(),
        fwd.url().to_string(),
        fwd.upstream_url().to_string(),
    ))
}

/// Simulates error conversion for {:error, message} tuples.
pub fn error_to_tuple(e: FfiError) -> (String, String) {
    let code = e.code.unwrap_or_default();
    let msg = e.message;
    (code, msg)
}
