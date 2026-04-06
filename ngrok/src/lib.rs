#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub(crate) mod internals {
    #[macro_use]
    pub mod rpc;
    pub mod proto;
    pub mod raw_session;
}

/// Tunnel and endpoint configuration types.
///
/// These are the v1 configuration types. For v2, use [`Agent`], [`EndpointListener`],
/// and [`EndpointForwarder`] instead.
pub mod config {
    #[macro_use]
    mod common;
    pub use common::*;

    mod headers;
    mod http;
    pub use self::http::*;
    mod labeled;
    pub use labeled::*;
    mod oauth;
    pub use oauth::*;
    mod oidc;
    pub use policies::*;
    mod policies;
    pub use oidc::*;
    mod tcp;
    pub use tcp::*;
    mod tls;
    pub use tls::*;
    mod webhook_verification;
}

mod proxy_proto;

/// Types for working with the ngrok session (v1 API).
///
/// For v2, use [`Agent`] and [`AgentBuilder`] instead.
pub mod session;

/// Types for working with ngrok tunnels (v1 API).
///
/// For v2, use [`EndpointListener`] and [`EndpointForwarder`] instead.
pub mod tunnel;

/// Types for working with ngrok connections.
pub mod conn;

/// Types for working with connection forwarders (v1 API).
///
/// For v2, use [`EndpointForwarder`] instead.
pub mod forwarder;
/// Extension methods for tunnels (v1 API, deprecated).
pub mod tunnel_ext;

// --- v2 API modules ---

/// The unified error type for ngrok operations.
pub mod error;

/// The ngrok agent — manages connections to the ngrok service.
pub mod agent;

/// Endpoint types — listeners and forwarders.
pub mod endpoint;

/// Endpoint builders for configuring listeners and forwarders.
pub mod endpoint_builder;

/// Upstream configuration for forwarding targets.
pub mod upstream;

/// Events emitted by the ngrok agent.
pub mod event;

/// Default agent and top-level convenience functions.
pub mod default_agent;

/// FFI-safe types for embedding ngrok in language wrappers.
#[cfg(feature = "ffi")]
#[cfg_attr(docsrs, doc(cfg(feature = "ffi")))]
pub mod ffi;

// --- v2 public API re-exports ---

#[doc(inline)]
pub use agent::{
    Agent,
    AgentBuilder,
    AgentSession,
};
// v1 re-exports (kept for backward compatibility)
#[doc(inline)]
pub use conn::EdgeConn;
#[doc(inline)]
pub use conn::{
    Conn,
    EndpointConn,
};
#[doc(inline)]
pub use default_agent::{
    default_agent,
    forward,
    forward_to,
    listen,
    reset_default_agent,
    set_default_agent,
};
#[doc(inline)]
pub use endpoint::{
    Endpoint,
    EndpointForwarder,
    EndpointListener,
};
#[doc(inline)]
pub use endpoint_builder::{
    EndpointForwardBuilder,
    EndpointListenBuilder,
};
#[doc(inline)]
pub use error::NgrokError;
#[doc(inline)]
pub use event::Event;
#[doc(inline)]
pub use internals::proto::Error;
#[doc(inline)]
pub use session::Session;
#[doc(inline)]
pub use tunnel::Tunnel;
#[doc(inline)]
pub use upstream::Upstream;

/// A prelude of commonly used types and traits.
///
/// This prelude includes both v2 types (preferred) and v1 types for backward
/// compatibility.
pub mod prelude {
    #[allow(deprecated)]
    #[doc(inline)]
    pub use crate::{
        // v2 types
        agent::{
            Agent,
            AgentBuilder,
            AgentSession,
        },
        config::{
            Action,
            ForwarderBuilder,
            HttpTunnelBuilder,
            InvalidPolicy,
            LabeledTunnelBuilder,
            OauthOptions,
            OidcOptions,
            Policy,
            ProxyProto,
            Rule,
            Scheme,
            TcpTunnelBuilder,
            TlsTunnelBuilder,
            TunnelBuilder,
        },
        conn::{
            Conn,
            ConnInfo,
            EdgeConnInfo,
            EndpointConn,
            EndpointConnInfo,
        },
        default_agent::{
            default_agent,
            forward,
            forward_to,
            listen,
        },
        endpoint::{
            Endpoint,
            EndpointForwarder,
            EndpointListener,
        },
        endpoint_builder::{
            EndpointForwardBuilder,
            EndpointListenBuilder,
        },
        error::NgrokError,
        event::Event,
        internals::proto::EdgeType,
        internals::proto::Error,
        tunnel::{
            EdgeInfo,
            EndpointInfo,
            Tunnel,
            TunnelCloser,
            TunnelInfo,
        },
        tunnel_ext::TunnelExt,
        upstream::Upstream,
    };
}

#[cfg(all(test, feature = "online-tests"))]
mod online_tests;
