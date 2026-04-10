# ngrok-rust

[![Crates.io][crates-badge]][crates-url]
[![docs.rs][docs-badge]][docs-url]
[![MIT licensed][mit-badge]][mit-url]
[![Apache-2.0 licensed][apache-badge]][apache-url]
[![Continuous integration][ci-badge]][ci-url]

[crates-badge]: https://img.shields.io/crates/v/ngrok.svg
[crates-url]: https://crates.io/crates/ngrok
[docs-badge]: https://img.shields.io/docsrs/ngrok.svg
[docs-url]: https://docs.rs/ngrok
[ci-badge]: https://github.com/ngrok/ngrok-rust/actions/workflows/ci.yml/badge.svg
[ci-url]: https://github.com/ngrok/ngrok-rust/actions/workflows/ci.yml
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/ngrok/ngrok-rust/blob/main/LICENSE-MIT
[apache-badge]: https://img.shields.io/badge/license-Apache_2.0-blue.svg
[apache-url]: https://github.com/ngrok/ngrok-rust/blob/main/LICENSE-APACHE

[API Docs (main)](https://ngrok.github.io/ngrok-rust/ngrok)

[ngrok](https://ngrok.com) is a simplified API-first ingress-as-a-service that adds connectivity,
security, and observability to your apps.

ngrok-rust is the native Rust SDK for ngrok. It lets Rust applications listen on
ngrok’s global ingress network for TCP and HTTP traffic without managing IPs, NAT,
certificates, or load balancers. Connections implement
[tokio’s AsyncRead and AsyncWrite traits](https://docs.rs/tokio/latest/tokio/io/index.html),
making it easy to integrate with frameworks like [axum](https://docs.rs/axum/latest/axum/)
and [hyper](https://docs.rs/hyper/latest/hyper/).

For working with the [ngrok API](https://ngrok.com/docs/api/), check out the
[ngrok Rust API Client Library](https://github.com/ngrok/ngrok-api-rs).

## Installation

Add `ngrok` to the `[dependencies]` section of your `Cargo.toml`:

```bash
cargo add ngrok
```

## Quickstart

Create a simple HTTP server using `ngrok` and `axum`:

```rust
use ngrok::{AgentBuilder, EndpointOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Build and connect an ngrok agent.
    let agent = AgentBuilder::new()
        .authtoken_from_env()
        .build()
        .await?;
    agent.connect().await?;

    // Forward traffic to a local service.
    let _forwarder = agent
        .forward(
            EndpointOptions::builder()
                .url("https://your-domain.ngrok.app")
                .build(),
            "http://localhost:3000",
        )
        .await?;

    // Wait indefinitely.
    tokio::signal::ctrl_c().await?;
    Ok(())
}
```

See [`/ngrok/tests/integration.rs`][integration-tests] for more usage examples.

[integration-tests]: https://github.com/ngrok/ngrok-rust/blob/main/ngrok/tests/integration.rs

# Changelog

Changes to `ngrok-rust` are tracked under [CHANGELOG.md](https://github.com/ngrok/ngrok-rust/blob/main/ngrok/CHANGELOG.md).

# Join the ngrok Community

- Check out [our official docs](https://docs.ngrok.com)
- Read about updates on [our blog](https://ngrok.com/blog)
- Open an [issue](https://github.com/ngrok/ngrok-rust/issues) or [pull request](https://github.com/ngrok/ngrok-rust/pulls)
- Join our [Slack community](https://ngrok.com/slack)
- Follow us on [X / Twitter (@ngrokHQ)](https://twitter.com/ngrokhq)
- Subscribe to our [Youtube channel (@ngrokHQ)](https://www.youtube.com/@ngrokhq)

# License

This project is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE][apache-url] or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT][mit-url] or
  <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in ngrok by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
