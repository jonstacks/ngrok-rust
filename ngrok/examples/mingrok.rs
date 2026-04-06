use anyhow::Error;
use ngrok::{
    Endpoint,
    Event,
    Upstream,
};
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let addr = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing forwarding address"))?;

    let agent = ngrok::Agent::builder()
        .authtoken_from_env()
        .event_handler(|event: Event| match &event {
            Event::AgentHeartbeatReceived { latency } => {
                info!(?latency, "heartbeat received");
            }
            Event::AgentConnectSucceeded => {
                info!("agent connected");
            }
            Event::AgentDisconnected { error } => {
                info!(?error, "agent disconnected");
            }
        })
        .build()?;

    let mut forwarder = agent.forward(Upstream::new(&addr)).start().await?;

    info!(
        url = forwarder.url(),
        upstream = forwarder.upstream_url(),
        "started forwarder"
    );

    forwarder.join().await??;

    Ok(())
}
