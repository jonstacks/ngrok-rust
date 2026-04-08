//! Upstream forwarding configuration.

/// The PROXY protocol version to use when forwarding connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ProxyProtoVersion {
    /// No PROXY protocol header (default).
    #[default]
    None,
    /// PROXY protocol version 1 (text).
    V1,
    /// PROXY protocol version 2 (binary).
    V2,
}

/// Upstream forwarding configuration.
///
/// Specifies where to forward traffic for a `forward()` endpoint.
pub struct Upstream {
    /// The upstream address (e.g., `"localhost:8080"`, `"http://localhost:8080"`).
    pub(crate) addr: String,
    /// Optional protocol override.
    pub(crate) protocol: Option<String>,
    /// Optional custom TLS config for upstream TLS connections.
    pub(crate) tls_config: Option<rustls::ClientConfig>,
    /// PROXY protocol version.
    pub(crate) proxy_proto: ProxyProtoVersion,
}

impl Upstream {
    /// Create an upstream from an address string.
    ///
    /// `addr` accepts: `"8080"`, `"host:port"`, `"http://host:port"`, `"https://host:port"`.
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            protocol: None,
            tls_config: None,
            proxy_proto: ProxyProtoVersion::None,
        }
    }

    /// Set the upstream protocol.
    pub fn protocol(mut self, proto: impl Into<String>) -> Self {
        self.protocol = Some(proto.into());
        self
    }

    /// Set a custom TLS configuration for upstream connections.
    pub fn tls_config(mut self, cfg: rustls::ClientConfig) -> Self {
        self.tls_config = Some(cfg);
        self
    }

    /// Set the PROXY protocol version.
    pub fn proxy_proto(mut self, version: ProxyProtoVersion) -> Self {
        self.proxy_proto = version;
        self
    }

    /// The upstream address string.
    pub fn addr(&self) -> &str {
        &self.addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_stores_addr() {
        let u = Upstream::new("localhost:8080");
        assert_eq!(u.addr(), "localhost:8080");
        assert!(u.protocol.is_none());
        assert!(u.tls_config.is_none());
        assert_eq!(u.proxy_proto, ProxyProtoVersion::None);
    }

    #[test]
    fn protocol_sets_protocol() {
        let u = Upstream::new("8080").protocol("http2");
        assert_eq!(u.protocol.as_deref(), Some("http2"));
    }

    #[test]
    fn proxy_proto_sets_version() {
        let u = Upstream::new("8080").proxy_proto(ProxyProtoVersion::V1);
        assert_eq!(u.proxy_proto, ProxyProtoVersion::V1);

        let u = Upstream::new("8080").proxy_proto(ProxyProtoVersion::V2);
        assert_eq!(u.proxy_proto, ProxyProtoVersion::V2);
    }

    #[test]
    fn default_proxy_proto_is_none() {
        assert_eq!(ProxyProtoVersion::default(), ProxyProtoVersion::None);
    }

    #[test]
    fn builder_chain() {
        let u = Upstream::new("http://backend:3000")
            .protocol("http2")
            .proxy_proto(ProxyProtoVersion::V2);
        assert_eq!(u.addr(), "http://backend:3000");
        assert_eq!(u.protocol.as_deref(), Some("http2"));
        assert_eq!(u.proxy_proto, ProxyProtoVersion::V2);
    }
}
