//! Endpoint configuration options.

/// Options for creating a new ngrok endpoint.
///
/// Build using `EndpointOptions::builder()` or use `Default::default()` for
/// an ephemeral endpoint with all defaults.
#[derive(Debug, Default, Clone)]
pub struct EndpointOptions {
    /// The full URL for the endpoint (scheme determines protocol).
    pub(crate) url: Option<String>,
    /// The endpoint name.
    pub(crate) name: Option<String>,
    /// A human-readable description.
    pub(crate) description: Option<String>,
    /// User-defined metadata.
    pub(crate) metadata: Option<String>,
    /// YAML or JSON traffic policy string.
    pub(crate) traffic_policy: Option<String>,
    /// Agent-side TLS termination config.
    pub(crate) agent_tls_termination: Option<rustls::ServerConfig>,
    /// Whether connection pooling is enabled.
    pub(crate) pooling_enabled: bool,
    /// Bindings.
    pub(crate) bindings: Vec<String>,
}

impl EndpointOptions {
    /// Create a new builder.
    pub fn builder() -> EndpointOptionsBuilder {
        EndpointOptionsBuilder::default()
    }
}

/// Builder for `EndpointOptions`.
#[derive(Debug, Default)]
pub struct EndpointOptionsBuilder {
    inner: EndpointOptions,
}

impl EndpointOptionsBuilder {
    /// Set the full URL (scheme determines protocol: https://, http://, tcp://, tls://).
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.inner.url = Some(url.into());
        self
    }

    /// Set the endpoint name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.inner.name = Some(name.into());
        self
    }

    /// Set the description.
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.inner.description = Some(desc.into());
        self
    }

    /// Set the metadata.
    pub fn metadata(mut self, meta: impl Into<String>) -> Self {
        self.inner.metadata = Some(meta.into());
        self
    }

    /// Set a YAML or JSON traffic policy string evaluated at the ngrok edge.
    pub fn traffic_policy(mut self, policy: impl Into<String>) -> Self {
        self.inner.traffic_policy = Some(policy.into());
        self
    }

    /// Configure agent-side TLS termination.
    pub fn agent_tls_termination(mut self, cfg: rustls::ServerConfig) -> Self {
        self.inner.agent_tls_termination = Some(cfg);
        self
    }

    /// Enable or disable connection pooling.
    pub fn pooling_enabled(mut self, pool: bool) -> Self {
        self.inner.pooling_enabled = pool;
        self
    }

    /// Set bindings.
    pub fn bindings(mut self, bindings: Vec<String>) -> Self {
        self.inner.bindings = bindings;
        self
    }

    /// Build the `EndpointOptions`.
    pub fn build(self) -> EndpointOptions {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_have_no_url() {
        let opts = EndpointOptions::default();
        assert!(opts.url.is_none());
        assert!(opts.name.is_none());
        assert!(opts.traffic_policy.is_none());
        assert!(!opts.pooling_enabled);
        assert!(opts.bindings.is_empty());
    }

    #[test]
    fn builder_sets_url() {
        let opts = EndpointOptions::builder()
            .url("https://custom.ngrok.app")
            .build();
        assert_eq!(opts.url.as_deref(), Some("https://custom.ngrok.app"));
    }

    #[test]
    fn builder_sets_all_fields() {
        let opts = EndpointOptions::builder()
            .url("tcp://0.tcp.ngrok.io:12345")
            .name("my-endpoint")
            .description("test endpoint")
            .metadata(r#"{"env": "test"}"#)
            .traffic_policy("on-http-request: []")
            .pooling_enabled(true)
            .bindings(vec!["public".into(), "internal".into()])
            .build();

        assert_eq!(opts.url.as_deref(), Some("tcp://0.tcp.ngrok.io:12345"));
        assert_eq!(opts.name.as_deref(), Some("my-endpoint"));
        assert_eq!(opts.description.as_deref(), Some("test endpoint"));
        assert_eq!(opts.metadata.as_deref(), Some(r#"{"env": "test"}"#));
        assert_eq!(opts.traffic_policy.as_deref(), Some("on-http-request: []"));
        assert!(opts.pooling_enabled);
        assert_eq!(opts.bindings, vec!["public", "internal"]);
    }

    #[test]
    fn builder_default_matches_options_default() {
        let from_builder = EndpointOptions::builder().build();
        let from_default = EndpointOptions::default();
        assert_eq!(from_builder.url, from_default.url);
        assert_eq!(from_builder.pooling_enabled, from_default.pooling_enabled);
        assert_eq!(from_builder.bindings, from_default.bindings);
    }
}
