# Vendored tiny_http

This is tiny_http **0.12.0** as published on crates.io (git `212b1c4`,
MIT OR Apache-2.0: see `LICENSE-MIT`, `LICENSE-APACHE`), patched for
anofox-visualization. The root `Cargo.toml` points every crate in the
workspace at it through `[patch.crates-io]`, so the core `serve` binary, the
DuckDB extension and the FFI build all use it.

The first commit adding this folder is the unmodified upstream copy; `git log
-p vendor/tiny_http` shows each change below on its own.

Why 0.12.0 and not the fork `tiny_http_dh` 0.12.2: the fork moves to Rust
2024 and a new pool API (`PoolConfig`, `*_with_pool`) and is a ~1,500-line
diff. Its security fixes are small and are backported here instead. Its
bounded pool has the same lost wake-up as 0.12.0 (#1 below).

Drop this folder once upstream ships these fixes.

## Changes

1. **TaskPool lost wake-up** (`src/util/task_pool.rs`, anofox-visualization
   #86). `spawn` checked `waiting_tasks > 0` and then only queued the task and
   called `notify_one`. A woken thread stays counted as waiting until it
   re-takes the lock, so a second connection accepted before that lost its
   wake-up and waited until some pool thread finished its current task. A
   keep-alive connection is one long task, so requests hung for minutes.
   `spawn` now queues first and starts a thread whenever the queue holds more
   tasks than there are idle threads. Test:
   `task_pool::tests::spawns_burst_without_losing_a_wake_up` (fails on the old
   `spawn`: 4 of 6 tasks start).
2. **CVE-2026-66752, request smuggling** (`src/request.rs`, `src/client.rs`),
   from tiny_http_dh 0.12.1. A `Transfer-Encoding` other than a bare
   `chunked`, or `Transfer-Encoding` together with `Content-Length`, is
   answered with `400` and the connection is closed.
   Ours on top: a repeated `Transfer-Encoding` or `Content-Length` header is
   rejected the same way (only the first one of each was looked at).
3. **CVE-2026-66753, header injection** (`src/common.rs`), from tiny_http_dh
   0.12.1. Control characters in a header name or value make
   `Header::from_bytes`, `Header::from_str` and `HeaderField::from_bytes` fail
   (so CR/LF can't reach a response header, and a request header carrying one
   gets a `400`). `HeaderField::from_bytes` now returns
   `FromAsciiError<Vec<u8>>`. Ours: HTAB stays allowed inside a value
   (RFC 9110 §5.5), which the fork rejects.
4. **Body drain allocation** (`src/util/equal_reader.rs`), from tiny_http_dh
   0.12.1. Dropping a request whose body was not read no longer allocates the
   client-declared `Content-Length` in one piece (a few bytes of request could
   ask for a terabyte).
5. **peer_addr panic** (`src/client.rs`), from tiny_http_dh 0.12.1. A failed
   `peer_addr()` on a fresh connection closes it instead of panicking the pool
   thread.
6. **TCP_NODELAY** (`src/connection.rs`, anofox-visualization #86). Accepted
   TCP sockets get `set_nodelay(true)`. A response is written in pieces (head,
   then body), so with Nagle's algorithm the last piece waited for the
   client's delayed ACK: ~40 ms on every response larger than a segment, even
   a cached one (up to 200 ms with Windows clients). A 4 KB cached panel over
   keep-alive: p50 41.0 ms → 0.4 ms. Test:
   `connection::tests::accepted_tcp_connections_have_nodelay`.
7. **Manifest** (`Cargo.toml`). The TLS features (`ssl`, `ssl-openssl`,
   `ssl-rustls`) and their optional dependencies are removed: we serve plain
   HTTP, and keeping them would lock rustls 0.20 / ring 0.16 into
   `Cargo.lock`. The dev-dependencies, `tests/`, `examples/` and `benches/`
   are left out (`doctest = false`: one doc example needs `rustc-serialize`).
   Upstream's code is exempt from the workspace's clippy and dead-code lints.
