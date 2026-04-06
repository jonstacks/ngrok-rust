use std::sync::{
    Mutex,
    OnceLock,
};

use crate::{
    agent::Agent,
    endpoint::EndpointForwarder,
    endpoint_builder::{
        EndpointForwardBuilder,
        EndpointListenBuilder,
    },
    error::NgrokError,
    upstream::Upstream,
};

/// Options for configuring an endpoint, used with [`forward_to`].
#[derive(Default)]
pub struct EndpointOptions {
    /// The URL for the endpoint.
    pub url: Option<String>,
    /// The traffic policy.
    pub traffic_policy: Option<String>,
    /// Opaque metadata.
    pub metadata: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Bindings.
    pub bindings: Vec<String>,
    /// Whether pooling is enabled.
    pub pooling_enabled: Option<bool>,
}

static DEFAULT_AGENT: OnceLock<Mutex<Agent>> = OnceLock::new();

fn get_or_init_default_agent() -> &'static Mutex<Agent> {
    DEFAULT_AGENT.get_or_init(|| {
        let agent = Agent::builder()
            .authtoken_from_env()
            .build()
            .expect("failed to build default agent");
        Mutex::new(agent)
    })
}

/// Returns the process-global default agent.
///
/// The default agent reads the authtoken from the `NGROK_AUTHTOKEN`
/// environment variable.
///
/// This function is safe to call from inside a tokio runtime.
pub fn default_agent() -> Agent {
    let guard = get_or_init_default_agent()
        .lock()
        .expect("default agent lock poisoned");
    guard.clone()
}

/// Set the process-global default agent.
pub fn set_default_agent(agent: Agent) {
    let _ = DEFAULT_AGENT.set(Mutex::new(agent));
}

/// Reset the process-global default agent.
///
/// The next call to [`default_agent()`] will create a new default agent.
pub fn reset_default_agent() {
    // OnceLock doesn't support reset, so we replace the inner value
    if let Some(m) = DEFAULT_AGENT.get() {
        let new_agent = Agent::builder()
            .authtoken_from_env()
            .build()
            .expect("failed to build default agent");
        let mut guard = m.lock().expect("default agent lock poisoned");
        *guard = new_agent;
    }
}

/// Start building an endpoint listener using the default agent.
///
/// This is a convenience function equivalent to `default_agent().listen()`.
///
/// Note: Each call leaks a small `Agent` (Arc-based, ~pointer-sized) to
/// obtain a `'static` lifetime. This is acceptable for module-level
/// convenience functions that are called infrequently.
pub fn listen() -> EndpointListenBuilder<'static> {
    let agent = default_agent();
    let agent_ref: &'static Agent = Box::leak(Box::new(agent));
    EndpointListenBuilder::new(agent_ref)
}

/// Start building an endpoint forwarder using the default agent.
///
/// This is a convenience function equivalent to
/// `default_agent().forward(upstream)`.
pub fn forward(upstream: Upstream) -> EndpointForwardBuilder<'static> {
    let agent = default_agent();
    let agent_ref: &'static Agent = Box::leak(Box::new(agent));
    EndpointForwardBuilder::new(agent_ref, upstream)
}

/// Forward to an address using the default agent with the given options.
///
/// This is a convenience function for the common FFI pattern of forwarding
/// with a single function call.
pub async fn forward_to(
    addr: &str,
    opts: EndpointOptions,
) -> Result<EndpointForwarder, NgrokError> {
    let agent = default_agent();
    let agent_ref: &'static Agent = Box::leak(Box::new(agent));
    let upstream = Upstream::new(addr);

    let mut fwd_builder = EndpointForwardBuilder::new(agent_ref, upstream);
    if let Some(ref url) = opts.url {
        fwd_builder = fwd_builder.url(url);
    }
    if let Some(ref tp) = opts.traffic_policy {
        fwd_builder = fwd_builder.traffic_policy(tp);
    }
    if let Some(ref meta) = opts.metadata {
        fwd_builder = fwd_builder.metadata(meta);
    }
    if let Some(ref desc) = opts.description {
        fwd_builder = fwd_builder.description(desc);
    }
    if !opts.bindings.is_empty() {
        fwd_builder = fwd_builder.bindings(opts.bindings.iter().map(|s| s.as_str()));
    }
    if let Some(pe) = opts.pooling_enabled {
        fwd_builder = fwd_builder.pooling_enabled(pe);
    }
    fwd_builder.start().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_options_default() {
        let opts = EndpointOptions::default();
        assert!(opts.url.is_none());
        assert!(opts.traffic_policy.is_none());
        assert!(opts.metadata.is_none());
        assert!(opts.description.is_none());
        assert!(opts.bindings.is_empty());
        assert!(opts.pooling_enabled.is_none());
    }

    #[tokio::test]
    async fn test_default_agent_in_async() {
        // This must not panic — verifies OnceLock+Mutex is safe in async
        let _agent = default_agent();
    }
}
