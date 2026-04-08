//! Package-level convenience functions using `NGROK_AUTHTOKEN`.

use crate::{
    Agent,
    error::Error,
    forwarder::EndpointForwarder,
    listener::EndpointListener,
    options::EndpointOptions,
    upstream::Upstream,
};

/// Listen using `NGROK_AUTHTOKEN` from the environment.
///
/// Equivalent to `Agent::builder().authtoken_from_env().build().await?.listen(opts).await`.
pub async fn listen(opts: EndpointOptions) -> Result<EndpointListener, Error> {
    let agent = Agent::builder().authtoken_from_env().build().await?;
    agent.listen(opts).await
}

/// Forward using `NGROK_AUTHTOKEN` from the environment.
///
/// Equivalent to `Agent::builder().authtoken_from_env().build().await?.forward(upstream, opts).await`.
pub async fn forward(
    upstream: Upstream,
    opts: EndpointOptions,
) -> Result<EndpointForwarder, Error> {
    let agent = Agent::builder().authtoken_from_env().build().await?;
    agent.forward(upstream, opts).await
}
