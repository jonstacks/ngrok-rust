//! RPC request and response types for server-initiated commands.

/// A server-initiated RPC command received from ngrok cloud.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RpcRequest {
    /// The RPC method to invoke.
    pub method: RpcMethod,
}

/// The type of RPC command received from ngrok cloud.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RpcMethod {
    /// Stop the ngrok agent.
    StopAgent,
    /// Restart the ngrok agent.
    RestartAgent,
    /// Update the ngrok agent.
    UpdateAgent,
}

/// The response to an RPC command.
#[derive(Debug, Default)]
pub struct RpcResponse {
    /// An optional error message. `None` indicates success.
    pub error: Option<String>,
}

/// RPC method name constants.
pub mod constants {
    /// Stop the ngrok agent.
    pub const STOP_AGENT: &str = "StopAgent";
    /// Restart the ngrok agent.
    pub const RESTART_AGENT: &str = "RestartAgent";
    /// Update the ngrok agent.
    pub const UPDATE_AGENT: &str = "UpdateAgent";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_response_default_is_success() {
        let resp = RpcResponse::default();
        assert!(resp.error.is_none());
    }

    #[test]
    fn rpc_response_with_error() {
        let resp = RpcResponse {
            error: Some("not supported".into()),
        };
        assert_eq!(resp.error.as_deref(), Some("not supported"));
    }

    #[test]
    fn rpc_request_debug_format() {
        let req = RpcRequest {
            method: RpcMethod::StopAgent,
        };
        let debug = format!("{:?}", req);
        assert!(debug.contains("StopAgent"));
    }

    #[test]
    fn rpc_method_variants() {
        // Ensure all variants are constructable (non-exhaustive guard).
        let _ = RpcMethod::StopAgent;
        let _ = RpcMethod::RestartAgent;
        let _ = RpcMethod::UpdateAgent;
    }

    #[test]
    fn rpc_constants_match_expected_values() {
        assert_eq!(constants::STOP_AGENT, "StopAgent");
        assert_eq!(constants::RESTART_AGENT, "RestartAgent");
        assert_eq!(constants::UPDATE_AGENT, "UpdateAgent");
    }
}
