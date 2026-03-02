use url::Url;

use crate::config::ProxyProto;

/// Configuration for forwarding to an upstream service.
///
/// Created using [`Upstream::new`].
///
/// # Example
///
/// ```rust
/// use ngrok::agent::Upstream;
///
/// // Simple address
/// let upstream = Upstream::new("http://localhost:8080");
///
/// // With protocol
/// let upstream = Upstream::new("http://localhost:8080")
///     .protocol("http2");
/// ```
#[derive(Debug, Clone)]
pub struct Upstream {
    pub(crate) addr: String,
    pub(crate) protocol: Option<String>,
    pub(crate) proxy_proto: Option<ProxyProto>,
}

impl Upstream {
    /// Creates a new upstream configuration with the given address.
    ///
    /// The address can be in various formats:
    /// - `"80"` - a port number for local services
    /// - `"localhost:8080"` - a host:port combination
    /// - `"http://localhost:8080"` - a full URL
    pub fn new(addr: impl Into<String>) -> Self {
        Upstream {
            addr: addr.into(),
            protocol: None,
            proxy_proto: None,
        }
    }

    /// Sets the protocol to use when forwarding to the upstream.
    ///
    /// This is typically used to specify `"http2"` when communicating with an
    /// upstream HTTP/2 server.
    pub fn protocol(mut self, proto: impl Into<String>) -> Self {
        self.protocol = Some(proto.into());
        self
    }

    /// Sets the PROXY protocol version to use when connecting to the upstream.
    pub fn proxy_proto(mut self, proxy_proto: ProxyProto) -> Self {
        self.proxy_proto = Some(proxy_proto);
        self
    }

    /// Returns the upstream address as a URL.
    ///
    /// If the address is just a port number, it will be prefixed with `http://localhost:`.
    /// If the address is a host:port, it will be prefixed with `http://`.
    pub(crate) fn to_url(&self) -> Result<Url, url::ParseError> {
        let addr = &self.addr;

        // Try as port only
        if let Ok(_port) = addr.parse::<u16>() {
            return Url::parse(&format!("http://localhost:{addr}"));
        }

        // Try parsing as a full URL first - only if it has a recognized scheme
        if let Ok(url) = Url::parse(addr) {
            if matches!(url.scheme(), "http" | "https" | "tcp" | "tls" | "unix") {
                return Ok(url);
            }
        }

        // Otherwise treat as host:port or host
        Url::parse(&format!("http://{addr}"))
    }
}
