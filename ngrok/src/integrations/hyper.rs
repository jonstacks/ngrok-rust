//! hyper 1.x integration.
//!
//! There is no trait to implement. Users bridge `NgrokStream` to hyper's IO
//! layer by wrapping it with `TokioIo` from hyper-util:
//!
//! ```rust,ignore
//! use hyper_util::rt::TokioIo;
//! let io = TokioIo::new(stream);  // stream: NgrokStream
//! ```
//!
//! Then pass `io` to hyper's `Builder::serve_connection(io, service)`.
//! `NgrokStream` already implements `tokio::io::AsyncRead + AsyncWrite`,
//! which is all `TokioIo` requires.
