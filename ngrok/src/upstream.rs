use crate::config::ProxyProto;

/// Configuration for the upstream target that connections are forwarded to.
pub struct Upstream {
    pub(crate) addr: String,
    pub(crate) protocol: Option<String>,
    pub(crate) proxy_proto: Option<ProxyProto>,
    pub(crate) verify_upstream_tls: bool,
}

impl Upstream {
    /// Create a new upstream target with the given address.
    ///
    /// The address should be in the form `host:port` (e.g. `"localhost:8080"`).
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            protocol: None,
            proxy_proto: None,
            verify_upstream_tls: true,
        }
    }

    /// Set the application protocol for the upstream (e.g. `"http2"`).
    pub fn protocol(mut self, proto: impl Into<String>) -> Self {
        self.protocol = Some(proto.into());
        self
    }

    /// Set the proxy protocol version for the upstream.
    pub fn proxy_proto(mut self, version: ProxyProto) -> Self {
        self.proxy_proto = Some(version);
        self
    }

    /// Set whether to verify upstream TLS certificates. Defaults to `true`.
    pub fn verify_upstream_tls(mut self, verify: bool) -> Self {
        self.verify_upstream_tls = verify;
        self
    }
}
