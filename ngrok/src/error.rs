use thiserror::Error;

use crate::{
    internals::{
        proto::Error as ProtoError,
        raw_session::RpcError,
    },
    session::ConnectError,
    tunnel::AcceptError,
};

/// The unified error type for all ngrok v2 operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NgrokError {
    /// An error from the ngrok API or RPC layer.
    #[error("{message}")]
    Api {
        /// The ngrok error code, if any (e.g. "ERR_NGROK_108").
        code: Option<String>,
        /// The error message.
        message: String,
    },

    /// The agent is not connected.
    #[error("agent not connected")]
    NotConnected,

    /// An unsupported URL scheme was provided.
    #[error("unsupported URL scheme '{scheme}'")]
    UnsupportedScheme {
        /// The unsupported scheme.
        scheme: String,
    },

    /// An invalid URL was provided.
    #[error("invalid URL '{url}': {reason}")]
    InvalidUrl {
        /// The invalid URL.
        url: String,
        /// The reason it's invalid.
        reason: String,
    },

    /// An I/O error occurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// An error occurred during session connection.
    #[error("connection error: {0}")]
    Connect(#[from] ConnectError),

    /// An error occurred when accepting a connection.
    #[error("accept error: {0}")]
    Accept(#[from] AcceptError),
}

impl NgrokError {
    /// Returns the ngrok error code, if any.
    pub fn code(&self) -> Option<&str> {
        match self {
            NgrokError::Api { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}

impl From<RpcError> for NgrokError {
    fn from(err: RpcError) -> Self {
        let code = ProtoError::error_code(&err).map(String::from);
        let message = ProtoError::msg(&err);
        NgrokError::Api { code, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ngrok_error_display() {
        let err = NgrokError::Api {
            code: Some("ERR_NGROK_108".into()),
            message: "not found".into(),
        };
        assert_eq!(err.to_string(), "not found");

        let err = NgrokError::NotConnected;
        assert_eq!(err.to_string(), "agent not connected");

        let err = NgrokError::UnsupportedScheme {
            scheme: "ftp".into(),
        };
        assert_eq!(err.to_string(), "unsupported URL scheme 'ftp'");

        let err = NgrokError::InvalidUrl {
            url: "bad".into(),
            reason: "no scheme".into(),
        };
        assert_eq!(err.to_string(), "invalid URL 'bad': no scheme");
    }

    #[test]
    fn test_ngrok_error_code() {
        let err = NgrokError::Api {
            code: Some("ERR_123".into()),
            message: "msg".into(),
        };
        assert_eq!(err.code(), Some("ERR_123"));

        let err = NgrokError::Api {
            code: None,
            message: "msg".into(),
        };
        assert_eq!(err.code(), None);

        let err = NgrokError::NotConnected;
        assert_eq!(err.code(), None);
    }
}
