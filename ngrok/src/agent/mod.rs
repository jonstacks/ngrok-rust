//! A high-level agent API for the ngrok service.
//!
//! The [`Agent`] provides a clean, high-level interface for interacting with
//! ngrok, inspired by the ngrok-go v2 API. It handles session management,
//! endpoint creation, and event handling.
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use ngrok::agent::{Agent, Upstream, with_url, with_metadata};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create and connect an agent
//! let agent = Agent::builder()
//!     .authtoken_from_env()
//!     .build()?;
//!
//! // Listen for connections (auto-connects if needed)
//! let listener = agent.listen(vec![
//!     with_url("https://example.ngrok.app"),
//!     with_metadata("my-service"),
//! ]).await?;
//!
//! println!("Listening on: {}", listener.url());
//!
//! // Or forward to an upstream
//! let upstream = Upstream::new("http://localhost:8080");
//! let forwarder = agent.forward(upstream, vec![
//!     with_url("https://example2.ngrok.app"),
//! ]).await?;
//!
//! println!("Forwarding to upstream on: {}", forwarder.url());
//! # Ok(())
//! # }
//! ```
//!
//! # Events
//!
//! You can register event handlers to receive notifications about agent lifecycle events:
//!
//! ```rust
//! use ngrok::agent::{Agent, Event, EventHandler};
//! use std::sync::Arc;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let handler: EventHandler = Arc::new(|event: Event| {
//!     match &event {
//!         Event::AgentConnectSucceeded { .. } => println!("Connected!"),
//!         Event::AgentDisconnected { error, .. } => println!("Disconnected: {:?}", error),
//!         Event::AgentHeartbeatReceived { latency, .. } => println!("Heartbeat: {:?}", latency),
//!     }
//! });
//!
//! let agent = Agent::builder()
//!     .authtoken("your-token")
//!     .event_handler(handler)
//!     .build()?;
//! # Ok(())
//! # }
//! ```

mod agent;
mod builder;
mod endpoint_options;
mod events;
mod upstream;

#[cfg(test)]
mod tests;

pub use agent::{
    Agent,
    AgentEndpointForwarder,
    AgentEndpointListener,
    AgentError,
    AgentSession,
    EndpointRef,
};
pub use builder::{AgentBuildError, AgentBuilder};
pub use endpoint_options::{
    with_bindings, with_description, with_metadata, with_pooling_enabled, with_traffic_policy,
    with_url, EndpointOption,
};
pub use events::{Event, EventHandler, EventType};
pub use upstream::Upstream;
