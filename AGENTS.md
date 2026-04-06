# AGENTS.md

Guidelines for AI agents working on this repository.

## Repository Overview

**ngrok-rust** is a native Rust SDK for [ngrok](https://ngrok.com) — an ingress-as-a-service platform. It lets Rust applications listen on a public internet address without managing IPs, NAT, certificates, or load balancers.

### Workspace Crates

| Crate | Path | Purpose |
|-------|------|---------|
| `ngrok` | `ngrok/` | Main SDK crate — tunnels, sessions, connections, config builders |
| `muxado` | `muxado/` | Stream multiplexing protocol implementation |
| `cargo-doc-ngrok` | `cargo-doc-ngrok/` | Cargo subcommand to serve Rust docs via ngrok |

---

## Build, Lint & Test

All commands use **Cargo**. There is no Makefile.

### Build

```bash
cargo build                                  # Build all crates
cargo build -p ngrok --all-features          # Build main crate with all features
```

### Format

```bash
cargo fmt --all                              # Auto-format code
cargo fmt --all -- --check                   # Check formatting (used in CI)
```

### Lint

```bash
cargo clippy --all-targets --all-features -- -D warnings   # Lint (warnings are errors)
cargo udeps --workspace --all-targets --all-features       # Check for unused dependencies
```

### Test

```bash
cargo test --workspace --all-targets         # Unit and integration tests
```

Online integration tests in `ngrok/src/online_tests.rs` require a valid `NGROK_AUTHTOKEN` environment variable. These are skipped automatically if the token is not set. Tests guarded by `#[cfg(feature = "paid-tests")]` require a paid ngrok account.

### Code Coverage

```bash
cargo llvm-cov --workspace --all-targets --features hyper,axum,aws-lc-rs           # Show coverage in terminal
cargo llvm-cov --workspace --all-targets --features hyper,axum,aws-lc-rs --open    # Open HTML coverage report in browser
```

### Semver Compliance

```bash
cargo semver-checks check-release -p ngrok
cargo semver-checks check-release -p muxado
```

---

## Code Style

Formatting is configured in `rustfmt.toml`:

```toml
imports_layout = "Vertical"
imports_granularity = "Crate"
group_imports = "StdExternalCrate"
edition = "2021"
```

- **Clippy** is enforced with `-D warnings`. All warnings must be resolved.
- Run `cargo fmt --all` before committing.
- Unused dependencies are caught by `cargo udeps`.

---

## CI/CD Pipelines

Workflows live in `.github/workflows/`:

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | push/PR to `main` | Format, clippy, udeps, tests, semver checks |
| `publish.yml` | manual dispatch | Publish crates to crates.io |
| `release.yml` | called by publish | Tag + release individual crates |
| `docs.yml` | automatic | Build and publish documentation |

CI runs on both **Linux** and **Windows**. Always ensure code is cross-platform.

---

## Key Conventions

- **Dual licensed:** MIT and Apache-2.0. All contributions are automatically dual-licensed.
- **Async:** The codebase is async-first using **Tokio**. Avoid blocking operations.
- **Error handling:** Uses `thiserror` for library error types. Do not use `anyhow` in library code (only in tests/examples).
- **Tracing:** Use `tracing` for structured logging, not `println!` or `eprintln!`.
- **Features:** The `ngrok` crate uses Cargo feature flags for optional integrations (e.g., `hyper`, `axum`, `aws-lc-rs`). Keep feature gates consistent.
- **Public API changes:** Any breaking changes to `ngrok` or `muxado` must be reflected in a semver version bump and validated with `cargo semver-checks`.

---

## Development Environment

A [Nix flake](https://nixos.org/manual/nix/stable/command-ref/new-cli/nix3-flake.html) (`flake.nix`) is provided for a reproducible dev environment with the correct Rust toolchain, `sccache`, and pre-commit hooks. Using Nix is optional but recommended.

Nix shortcuts (when inside `nix develop`):

```bash
fix-n-fmt       # Auto-fix clippy warnings and reformat code
setup-hooks     # Install pre-commit hooks
coverage        # Show code coverage in the terminal
coverage-html   # Open HTML code coverage report in the browser
```

---

## Making Changes

1. **Bug fixes and small improvements** — open a PR directly.
2. **Larger features** — open an issue for discussion first (see `CONTRIBUTING.md`).
3. Always run `cargo fmt --all` and `cargo clippy --all-targets --all-features -- -D warnings` before pushing.
4. Add or update tests for any changed behavior.
5. If modifying a public API, check semver compliance with `cargo semver-checks`.
