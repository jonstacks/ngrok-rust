/// A functional option for configuring endpoints.
///
/// Endpoint options are used with [`Agent::listen`] and [`Agent::forward`] to
/// configure the created endpoints.
///
/// # Example
///
/// ```rust
/// use ngrok::agent::{with_url, with_metadata, with_traffic_policy};
///
/// let opts = vec![
///     with_url("https://example.ngrok.app"),
///     with_metadata("my-endpoint"),
///     with_traffic_policy(r#"{"on_http_request": []}"#),
/// ];
/// ```
pub struct EndpointOption {
    pub(crate) apply: Box<dyn FnOnce(&mut EndpointOpts) + Send>,
}

#[derive(Default, Clone, Debug)]
pub(crate) struct EndpointOpts {
    pub(crate) url: Option<String>,
    pub(crate) metadata: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) traffic_policy: Option<String>,
    pub(crate) bindings: Vec<String>,
    pub(crate) pooling_enabled: Option<bool>,
}

/// Sets the URL for the endpoint.
///
/// The URL determines the protocol and address of the endpoint. Examples:
/// - `"https://example.ngrok.app"` - HTTPS with a custom domain
/// - `"tcp://1.tcp.ngrok.io:12345"` - TCP with a specific address
/// - `"tls://example.ngrok.app"` - TLS endpoint
pub fn with_url(url: impl Into<String> + Send + 'static) -> EndpointOption {
    EndpointOption {
        apply: Box::new(move |opts| {
            opts.url = Some(url.into());
        }),
    }
}

/// Sets opaque, machine-readable metadata for the endpoint.
pub fn with_metadata(meta: impl Into<String> + Send + 'static) -> EndpointOption {
    EndpointOption {
        apply: Box::new(move |opts| {
            opts.metadata = Some(meta.into());
        }),
    }
}

/// Sets a human-readable description for the endpoint.
pub fn with_description(desc: impl Into<String> + Send + 'static) -> EndpointOption {
    EndpointOption {
        apply: Box::new(move |opts| {
            opts.description = Some(desc.into());
        }),
    }
}

/// Sets the traffic policy for the endpoint.
///
/// See <https://ngrok.com/docs/traffic-policy/>
pub fn with_traffic_policy(policy: impl Into<String> + Send + 'static) -> EndpointOption {
    EndpointOption {
        apply: Box::new(move |opts| {
            opts.traffic_policy = Some(policy.into());
        }),
    }
}

/// Sets the endpoint's bindings.
///
/// See <https://ngrok.com/docs/universal-gateway/bindings/>
pub fn with_bindings(bindings: Vec<String>) -> EndpointOption {
    EndpointOption {
        apply: Box::new(move |opts| {
            opts.bindings = bindings;
        }),
    }
}

/// Controls whether the endpoint supports connection pooling.
///
/// See <https://ngrok.com/docs/universal-gateway/endpoint-pooling/>
pub fn with_pooling_enabled(pool: bool) -> EndpointOption {
    EndpointOption {
        apply: Box::new(move |opts| {
            opts.pooling_enabled = Some(pool);
        }),
    }
}

impl EndpointOpts {
    pub(crate) fn apply_options(opts: impl IntoIterator<Item = EndpointOption>) -> Self {
        let mut endpoint_opts = EndpointOpts::default();
        for opt in opts {
            (opt.apply)(&mut endpoint_opts);
        }
        endpoint_opts
    }

    /// Determine the URL scheme from the URL, defaulting to https.
    pub(crate) fn url_scheme(&self) -> &str {
        match &self.url {
            Some(url) => {
                if let Ok(parsed) = url::Url::parse(url) {
                    match parsed.scheme() {
                        "http" | "https" | "tcp" | "tls" => {
                            // We need to return a static str, so match again
                            if url.starts_with("http://") {
                                return "http";
                            } else if url.starts_with("tcp://") {
                                return "tcp";
                            } else if url.starts_with("tls://") {
                                return "tls";
                            }
                        }
                        _ => {}
                    }
                }
                "https"
            }
            None => "https",
        }
    }
}
