# Pure Rust implementation of the muxado protocol

[![Crates.io][crates-badge]][crates-url]
[![docs.rs][docs-badge]][docs-url]
[![MIT licensed][mit-badge]][mit-url]
[![Apache-2.0 licensed][apache-badge]][apache-url]
[![Continuous integration][ci-badge]][ci-url]

[crates-badge]: https://img.shields.io/crates/v/muxado.svg
[crates-url]: https://crates.io/crates/muxado
[docs-badge]: https://img.shields.io/docsrs/muxado.svg
[docs-url]: https://docs.rs/muxado
[ci-badge]: https://github.com/ngrok/ngrok-rust/actions/workflows/ci.yml/badge.svg
[ci-url]: https://github.com/ngrok/ngrok-rust/actions/workflows/ci.yml
[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: https://github.com/ngrok/ngrok-rust/blob/main/LICENSE-MIT
[apache-badge]: https://img.shields.io/badge/license-Apache_2.0-blue.svg
[apache-url]: https://github.com/ngrok/ngrok-rust/blob/main/LICENSE-APACHE

An async Rust implementation of the muxado stream multiplexing protocol,
built on [tokio](https://tokio.rs). Muxado multiplexes many bidirectional
byte streams over a single transport connection (e.g. TCP or TLS).

This crate powers the tunnel transport layer in the
[ngrok Rust SDK](https://crates.io/crates/ngrok).

## Features

- **Fully async** — built on tokio with separate reader/writer tasks
- **Per-stream flow control** — configurable receive windows with automatic WNDINC
- **Zero-copy I/O** — uses `bytes::Bytes` for frame payloads
- **Typed streams** — 4-byte type prefix for application-level stream routing
- **Heartbeat** — periodic keep-alive with nonce verification and latency measurement
- **Half-close** — independent FIN for each direction (HTTP-style request/response)
- **Graceful shutdown** — GOAWAY with last-stream-ID tracking

See [PROTOCOL.md](PROTOCOL.md) for the wire format specification.

# License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE][apache-url] or
   <http://www.apache.org/licenses/LICENSE-2.0>)
 * MIT license ([LICENSE-MIT][mit-url] or
   <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in muxado by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
