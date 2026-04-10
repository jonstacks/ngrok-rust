//! Serde-serializable ngrok wire protocol messages.
//!
//! JSON field names use Go's encoding/json conventions (PascalCase).

use serde::{
    Deserialize,
    Serialize,
};

/// Authentication request sent to ngrok cloud.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct Auth {
    /// Supported protocol versions.
    #[serde(rename = "Version")]
    pub version: Vec<String>,

    /// The client ID (empty for new sessions).
    #[serde(rename = "ClientId")]
    pub client_id: String,

    /// Additional authentication metadata.
    #[serde(rename = "Extra")]
    pub extra: AuthExtra,
}

/// Extra fields in the Auth message.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct AuthExtra {
    /// The operating system.
    #[serde(rename = "Os")]
    pub os: String,

    /// The CPU architecture.
    #[serde(rename = "Arch")]
    pub arch: String,

    /// The ngrok authtoken.
    #[serde(rename = "Authtoken")]
    pub authtoken: String,

    /// The agent version.
    #[serde(rename = "Version")]
    pub version: String,

    /// The hostname of the agent machine.
    #[serde(rename = "Hostname")]
    pub hostname: String,

    /// The user agent string.
    #[serde(rename = "UserAgent")]
    pub user_agent: String,

    /// User-defined metadata.
    #[serde(rename = "Metadata")]
    pub metadata: String,

    /// A session cookie for reconnection.
    #[serde(rename = "Cookie")]
    pub cookie: String,

    /// Heartbeat interval in milliseconds.
    #[serde(rename = "HeartbeatInterval")]
    pub heartbeat_interval_ms: i64,

    /// Heartbeat tolerance in milliseconds.
    #[serde(rename = "HeartbeatTolerance")]
    pub heartbeat_tolerance_ms: i64,

    /// Error if StopAgent is unsupported.
    #[serde(
        rename = "StopUnsupportedError",
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_unsupported_error: Option<String>,

    /// Error if RestartAgent is unsupported.
    #[serde(
        rename = "RestartUnsupportedError",
        skip_serializing_if = "Option::is_none"
    )]
    pub restart_unsupported_error: Option<String>,

    /// Error if UpdateAgent is unsupported.
    #[serde(
        rename = "UpdateUnsupportedError",
        skip_serializing_if = "Option::is_none"
    )]
    pub update_unsupported_error: Option<String>,
}

/// Authentication response from ngrok cloud.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct AuthResp {
    /// The protocol version negotiated.
    #[serde(rename = "Version", default)]
    pub version: String,

    /// The server-assigned client ID.
    #[serde(rename = "ClientId", default)]
    pub client_id: String,

    /// Extra metadata from the server.
    #[serde(rename = "Extra", default)]
    pub extra: AuthRespExtra,

    /// Error message, if any. Empty string indicates success.
    #[serde(rename = "Error", default)]
    pub error: String,
}

/// Extra fields in the AuthResp message.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct AuthRespExtra {
    /// The ngrok region.
    #[serde(rename = "Region", default)]
    pub region: String,

    /// The server-assigned agent session ID.
    #[serde(rename = "AgentSessionID", default)]
    pub agent_session_id: String,

    /// A cookie for session resumption.
    #[serde(rename = "Cookie", default)]
    pub cookie: String,

    /// Alternative connect addresses.
    #[serde(rename = "ConnectAddresses", default)]
    pub connect_addresses: Vec<ConnectAddress>,
}

/// A connect address entry returned by ngrok cloud.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct ConnectAddress {
    /// The region identifier.
    #[serde(rename = "Region", default)]
    pub region: String,

    /// The server address (host:port).
    #[serde(rename = "ServerAddr", default)]
    pub server_addr: String,
}

/// Tunnel bind request.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BindReq {
    /// The client ID.
    #[serde(rename = "Id")]
    pub client_id: String,

    /// The protocol (e.g., "https", "tcp").
    pub proto: String,

    /// Protocol-specific options as a JSON object.
    pub opts: serde_json::Value,

    /// The upstream forwarding address.
    #[serde(rename = "ForwardsTo")]
    pub forwards: String,
}

/// Tunnel bind response.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BindResp {
    /// The endpoint ID.
    #[serde(rename = "Id", default)]
    pub client_id: String,

    /// The public URL assigned to this endpoint.
    #[serde(rename = "URL", default)]
    pub url: String,

    /// The protocol.
    #[serde(default)]
    pub proto: String,

    /// Error message, if any. Empty string indicates success.
    #[serde(default)]
    pub error: String,
}

/// Proxy connection header sent before each proxied connection.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct ProxyHeader {
    /// The endpoint ID.
    #[serde(rename = "Id", default)]
    pub id: String,

    /// The client's IP address as reported by ngrok cloud.
    #[serde(rename = "ClientAddr", default)]
    pub client_addr: String,

    /// The protocol.
    #[serde(rename = "Proto", default)]
    pub proto: String,

    /// The edge type.
    #[serde(rename = "EdgeType", default)]
    pub edge_type: String,

    /// Whether this connection uses TLS passthrough.
    #[serde(rename = "PassthroughTLS", default)]
    pub passthrough_tls: bool,
}

/// Server-initiated stop tunnel message.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct StopTunnel {
    /// The endpoint ID to stop.
    #[serde(rename = "Id")]
    pub id: String,

    /// A human-readable message.
    #[serde(rename = "Message")]
    pub message: String,

    /// The error code, if any.
    #[serde(rename = "ErrorCode")]
    pub error_code: String,
}

/// Server information response.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
pub struct SrvInfoResp {
    /// The ngrok region.
    #[serde(rename = "Region")]
    pub region: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_serializes_correctly() {
        let auth = Auth {
            version: vec!["3".into(), "2".into()],
            client_id: String::new(),
            extra: AuthExtra {
                authtoken: "test-token".into(),
                os: "linux".into(),
                arch: "amd64".into(),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("\"3\""));
        assert!(json.contains("test-token"));
        assert!(json.contains("\"Os\""));
        assert!(json.contains("linux"));
    }

    #[test]
    fn auth_resp_round_trips() {
        let resp = AuthResp {
            version: "3".into(),
            client_id: "client123".into(),
            extra: AuthRespExtra {
                region: "us".into(),
                agent_session_id: "sess_abc123".into(),
                cookie: "cookie-value".into(),
                connect_addresses: vec![ConnectAddress {
                    region: "us".into(),
                    server_addr: "addr1".into(),
                }],
            },
            error: String::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: AuthResp = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.extra.region, "us");
        assert_eq!(decoded.extra.agent_session_id, "sess_abc123");
    }

    #[test]
    fn proxy_header_round_trips() {
        let header = ProxyHeader {
            id: "ep_123".into(),
            client_addr: "1.2.3.4:5678".into(),
            proto: "https".into(),
            edge_type: "cloud".into(),
            passthrough_tls: false,
        };
        let json = serde_json::to_string(&header).unwrap();
        let decoded: ProxyHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "ep_123");
        assert_eq!(decoded.client_addr, "1.2.3.4:5678");
    }
}
