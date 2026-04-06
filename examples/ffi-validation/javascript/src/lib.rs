//! NAPI-RS FFI validation — proves ngrok::ffi types compile and are usable
//! from a cdylib targeting language wrappers.
//!
//! This crate does NOT depend on napi/napi-derive (which require Node.js).
//! Instead it validates that every FFI type can be constructed, that async
//! methods have the right signatures, and that Send + Sync bounds hold.

use ngrok::ffi::*;

// Prove Send + Sync
fn _assert_send<T: Send>() {}
fn _assert_sync<T: Sync>() {}

fn _prove_bounds() {
    _assert_send::<FfiAgent>();
    _assert_sync::<FfiAgent>();
    _assert_send::<FfiError>();
    _assert_sync::<FfiError>();
}

/// Wrapper showing how a language binding would create an agent.
pub fn create_agent_from_env() -> Result<FfiAgent, FfiError> {
    FfiAgent::from_env()
}

/// Wrapper showing how a language binding would create an agent with token.
pub fn create_agent(token: &str) -> Result<FfiAgent, FfiError> {
    FfiAgent::with_authtoken(token)
}

/// Wrapper showing how a language binding would listen.
pub async fn agent_listen(
    agent: &FfiAgent,
    url: Option<String>,
) -> Result<FfiEndpointListener, FfiError> {
    let opts = FfiEndpointOptions {
        url,
        ..Default::default()
    };
    agent.listen(opts).await
}

/// Wrapper showing how a language binding would forward.
pub async fn agent_forward(
    agent: &FfiAgent,
    upstream_addr: &str,
    url: Option<String>,
) -> Result<FfiEndpointForwarder, FfiError> {
    let opts = FfiEndpointOptions {
        url,
        ..Default::default()
    };
    let upstream = FfiUpstream {
        addr: upstream_addr.to_string(),
        ..Default::default()
    };
    agent.forward(upstream, opts).await
}

/// Show how to read endpoint info from a listener.
pub fn listener_info(l: &FfiEndpointListener) -> (&str, &str, &str) {
    (l.id(), l.url(), l.protocol())
}

/// Show how to read endpoint info from a forwarder.
pub fn forwarder_info(f: &FfiEndpointForwarder) -> (&str, &str, &str) {
    (f.id(), f.url(), f.upstream_url())
}

/// Show FfiError conversion.
pub fn error_to_string(e: &FfiError) -> String {
    e.to_string()
}
