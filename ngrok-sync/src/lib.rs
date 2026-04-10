//! # ngrok-sync
//!
//! Blocking (synchronous) wrapper around the async `ngrok` crate.
//!
//! Provides control-plane APIs as blocking functions. Data plane I/O remains
//! async and should be driven via the runtime exposed by `Agent::runtime()`.

pub mod agent;
pub mod forwarder;
pub mod listener;

pub use agent::{
    Agent,
    AgentBuilder,
};
pub use forwarder::EndpointForwarder;
pub use listener::EndpointListener;
// Re-export types from ngrok that FFI adapters need.
pub use ngrok::{
    AgentSession,
    DiagnoseError,
    EndpointInfo,
    EndpointOptions,
    Error,
    Event,
    NgrokStream,
    ProxyProtoVersion,
    RpcRequest,
    RpcResponse,
    Upstream,
};
