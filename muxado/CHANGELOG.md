## 0.4.0 (unreleased)

### Breaking Changes

* **Complete rewrite** of the muxado implementation. The crate now uses a
  clean-room architecture with separate reader/writer tokio tasks, per-stream
  flow control via `FlowWindow`, and zero-copy `bytes::Bytes` payloads.
* Renamed `SessionConfig` to `Config`.
* Renamed `Stream` to `MuxadoStream`.
* Renamed `Session::open_stream()` / `accept_stream()` to `open()` / `accept()`.
* Renamed `TypedStreamSession::open_typed_stream()` / `accept_typed_stream()`
  to `open_typed()` / `accept_typed()`.
* `TypedStream` fields (`stream_type`, `stream`) are now public instead of
  accessed via methods.
* `MuxadoStream::close_write()` and `reset()` are now synchronous (no `.await`).
* `Heartbeat::new()` now takes `(Arc<TypedStreamSession>, HeartbeatCallback, HeartbeatConfig)`.
  The callback is provided at construction time instead of at `start()`.
* `Heartbeat::start()` returns a `HeartbeatHandle` (no arguments).
* `TypedStreamSession` is no longer `Clone`. Use `Arc<TypedStreamSession>` for
  shared ownership.
* Removed `StreamId` wrapper type; stream IDs are plain `u32`.
* Removed `bitflags` dependency; frame flags use a custom `DataFlags` type.
* Removed `stream_manager`, `stream_output`, `codec`, `constrained`, and
  `cancellation_test` modules.
* Removed example programs (`client`, `server`, `heartbeat`, `subscriber`).

### Added

* `buffer` module with `InboundBuffer` for per-stream receive buffering.
* `config` module with `Config` struct (replaces `SessionConfig`).
* `window` module with `FlowWindow` for per-stream transmit flow control.
* `MuxadoStream::next_frame_payload()` for zero-copy chunk retrieval.
* `TypedStream` implements `AsyncRead` and `AsyncWrite` (delegating to inner
  `MuxadoStream`).
* `HeartbeatHandle` type returned by `Heartbeat::start()` for stopping the
  heartbeat loop.
* `HeartbeatResult` enum (`Ok(Duration)` / `Timeout`) for heartbeat callbacks.
* `Heartbeat::beat()` for on-demand out-of-band heartbeat checks.
* `Heartbeat::set_interval()` / `set_tolerance()` for runtime adjustment.

## 0.3.0

* Rework the heartbeat callback into a handler trait object.

## 0.2.0

* `Heartbeat` no longer `(Ref)?UnwindSafe`. Technically a breaking change,
therefore bumping its minor version.


## 0.1.0

* Initial Public Release