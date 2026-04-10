//! Error types for the ngrok crate.

/// The primary error type for ngrok operations.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error returned by ngrok cloud.
    #[error("ngrok cloud error [{code}]: {message}")]
    Cloud {
        /// The ngrok error code (e.g., "ERR_NGROK_108").
        code: String,
        /// A human-readable message.
        message: String,
    },

    /// The agent is not connected.
    #[error("agent not connected")]
    NotConnected,

    /// The agent is already connected.
    #[error("agent already connected")]
    AlreadyConnected,

    /// An unsupported URL scheme was provided.
    #[error("unsupported URL scheme: {0}")]
    UnsupportedScheme(String),

    /// A TLS error occurred.
    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A muxado protocol error occurred.
    #[error("muxado protocol error: {0}")]
    Muxado(String),

    /// The authtoken is missing.
    #[error("authtoken missing — set NGROK_AUTHTOKEN or call .authtoken()")]
    MissingAuthtoken,

    /// An invalid upstream address was provided.
    #[error("invalid upstream address: {0}")]
    InvalidUpstream(String),

    /// A JSON serialization/deserialization error occurred.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An operation timed out.
    #[error("operation timed out")]
    Timeout,

    /// An invalid URL was provided.
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
}

impl Error {
    /// Returns the ngrok error code if this is a `Cloud` variant.
    pub fn code(&self) -> Option<&str> {
        if let Self::Cloud { code, .. } = self {
            Some(code.as_str())
        } else {
            None
        }
    }
}

impl From<muxado::MuxadoError> for Error {
    fn from(e: muxado::MuxadoError) -> Self {
        Error::Muxado(e.to_string())
    }
}

/// Errors returned by `Agent::diagnose`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DiagnoseError {
    /// TCP connection failed.
    #[error("TCP connection to {addr} failed: {source}")]
    Tcp {
        /// The address that failed.
        addr: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// TLS handshake failed.
    #[error("TLS handshake with {addr} failed: {source}")]
    Tls {
        /// The address that failed.
        addr: String,
        /// The underlying TLS error.
        #[source]
        source: rustls::Error,
    },

    /// muxado SrvInfo exchange failed.
    #[error("muxado SrvInfo exchange failed: {0}")]
    Muxado(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_error_exposes_code() {
        let err = Error::Cloud {
            code: "ERR_NGROK_108".into(),
            message: "invalid authtoken".into(),
        };
        assert_eq!(err.code(), Some("ERR_NGROK_108"));
        assert!(err.to_string().contains("ERR_NGROK_108"));
    }

    #[test]
    fn non_cloud_error_has_no_code() {
        let err = Error::NotConnected;
        assert_eq!(err.code(), None);
    }
}
