pub mod buffer;
pub mod config;
pub mod error;
pub mod frame;
pub mod heartbeat;
pub mod session;
pub mod stream;
pub mod typed;
pub mod window;

// Re-export main types for convenience
pub use config::Config;
/// Alias for backwards compatibility with the old API.
pub type SessionConfig = Config;
pub use error::{
    ErrorCode,
    MuxadoError,
};
pub use heartbeat::{
    Heartbeat,
    HeartbeatConfig,
};
pub use session::Session;
pub use stream::MuxadoStream;
pub use typed::{
    TypedStream,
    TypedStreamSession,
};
