//! Stream-multiplexing protocol (muxado) for Rust.

pub mod error;
pub mod frame;
pub mod session;

pub use error::{ErrorCode, MuxadoError};
pub use session::{Role, Session, Stream};
