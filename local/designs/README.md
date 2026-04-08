# ngrok-rust

> Idiomatic Rust SDK for embedding ngrok networking directly into Rust applications —
> the ngrok agent packaged as a library, with first-class async support and seamless
> integration with hyper, axum, and aws-lc-rs.

**Language:** Rust

## Quick Start

```rust
use ngrok::{Agent, EndpointOptions};

#[tokio::main]
async fn main() -> Result<(), ngrok::Error> {
    // Build an agent from the NGROK_AUTHTOKEN environment variable
    let agent = Agent::builder()
        .authtoken_from_env()
        .build()
        .await?;

    // Open a publicly-addressable HTTPS endpoint
    let listener = agent
        .listen(EndpointOptions::builder().build())
        .await?;

    println!("Listening on: {}", listener.url());

    // Hand the listener to any axum Router (feature = "axum")
    let app = axum::Router::new()
        .route("/", axum::routing::get(|| async { "Hello from ngrok!" }));

    axum::serve(listener, app).await?;
    Ok(())
}
```

## Crate Structure

```
ngrok-rust/                  ← Cargo workspace
├── muxado/                  ← Stream-multiplexing protocol crate (crates.io: muxado)
├── ngrok/                   ← Async SDK crate (crates.io: ngrok)
├── ngrok-sync/              ← Blocking wrapper crate (crates.io: ngrok-sync)
└── ngrok-testing/           ← Offline test helpers (in-process mock server)
```

FFI adapter crates (`ngrok-uniffi`, `ngrok-rustler`, etc.) live in their own
repositories and depend on the published `ngrok-sync` crate. See [decisions.md](decisions.md).

## Feature Flags (`ngrok` crate)

| Flag | Default | Effect |
|------|---------|--------|
| `rustls-native-roots` | ✓ | Load system CA roots via `rustls-native-roots` |
| `hyper` | — | Enables `hyper-util` as a dependency; use `TokioIo::new(stream)` to bridge `NgrokStream` into hyper 1.x (no trait impl added to `EndpointListener`) |
| `axum` | — | `axum::serve` compatibility for `EndpointListener` |
| `aws-lc-rs` | — | Use `aws-lc-rs` as the `rustls` crypto backend (FIPS-capable) |
| `online-tests` | — | Gate online integration tests (require `NGROK_AUTHTOKEN`) |
| `testing` | — | Re-export `ngrok_testing` helpers |

## Design Documents

| File | Description |
|------|-------------|
| [api.md](api.md) | Public API reference (`ngrok` and `ngrok-sync` crates) |
| [muxado-api.md](muxado-api.md) | Public API reference (`muxado` crate) |
| [architecture.md](architecture.md) | Internal architecture |
| [testing.md](testing.md) | Testing strategy |
| [examples.md](examples.md) | Usage examples |
| [decisions.md](decisions.md) | Design decisions |
| [implementation-guide.md](implementation-guide.md) | AI agent implementation guide |
