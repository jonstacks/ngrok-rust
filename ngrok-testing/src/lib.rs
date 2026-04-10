//! # ngrok-testing
//!
//! Test helpers for ngrok-rust, including a mock ngrok server.
//!
//! This crate depends on `muxado` directly (not on `ngrok`) to avoid circular
//! dependencies. The `AgentTestExt` trait is defined in the `ngrok` crate
//! behind the `testing` feature flag.

pub mod endpoint;
pub mod fixtures;
pub mod server;

pub use endpoint::MockEndpoint;
pub use fixtures::{
    TestCertificate,
    danger_accept_any_cert_config,
};
pub use server::MockNgrokServer;
