//! Endpoint metadata types.

use url::Url;

/// A non-owning snapshot of endpoint metadata.
///
/// Returned by `Agent::endpoints()`.
#[derive(Clone, Debug)]
pub struct EndpointInfo {
    /// The endpoint ID.
    pub id: String,
    /// The endpoint name.
    pub name: String,
    /// The public URL.
    pub url: Url,
    /// The protocol (e.g., "https", "tcp").
    pub protocol: String,
    /// Whether this is a listener or forwarder endpoint.
    pub kind: EndpointKind,
}

/// The kind of an endpoint.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum EndpointKind {
    /// A listener endpoint that delivers connections to application code.
    Listener,
    /// A forwarder endpoint that auto-proxies connections to an upstream.
    Forwarder {
        /// The upstream URL.
        upstream_url: Url,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_info_listener() {
        let info = EndpointInfo {
            id: "ep_123".into(),
            name: "my-tunnel".into(),
            url: Url::parse("https://abc.ngrok.app").unwrap(),
            protocol: "https".into(),
            kind: EndpointKind::Listener,
        };
        assert_eq!(info.id, "ep_123");
        assert_eq!(info.url.as_str(), "https://abc.ngrok.app/");
        matches!(info.kind, EndpointKind::Listener);
    }

    #[test]
    fn endpoint_info_forwarder() {
        let info = EndpointInfo {
            id: "ep_456".into(),
            name: "fwd".into(),
            url: Url::parse("tcp://0.tcp.ngrok.io:12345").unwrap(),
            protocol: "tcp".into(),
            kind: EndpointKind::Forwarder {
                upstream_url: Url::parse("tcp://localhost:8080").unwrap(),
            },
        };
        if let EndpointKind::Forwarder { upstream_url } = &info.kind {
            assert_eq!(upstream_url.as_str(), "tcp://localhost:8080");
        } else {
            panic!("expected Forwarder kind");
        }
    }

    #[test]
    fn endpoint_info_clone() {
        let info = EndpointInfo {
            id: "ep_1".into(),
            name: "t".into(),
            url: Url::parse("https://a.ngrok.app").unwrap(),
            protocol: "https".into(),
            kind: EndpointKind::Listener,
        };
        let cloned = info.clone();
        assert_eq!(info.id, cloned.id);
    }
}
