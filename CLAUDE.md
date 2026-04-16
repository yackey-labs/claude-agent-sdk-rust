# CLAUDE.md — contributor guide

This is the Rust port of [`claude-agent-sdk-python`](https://github.com/anthropics/claude-agent-sdk-python). When working in this repo, follow the conventions below.

## Architecture

```
src/
├── lib.rs              # Crate root: re-exports + public query() function
├── convenience.rs      # High-level API: Claude, Chat, Reply, ClaudeBuilder
├── types.rs            # All type definitions (messages, options, hooks, MCP, etc.)
├── errors.rs           # ClaudeSdkError enum + Result alias
├── mcp.rs              # In-process MCP SDK server (create_sdk_mcp_server, tool! macro)
├── message_parser.rs   # Parses raw CLI JSON → typed Message variants
├── query.rs            # Control protocol orchestrator (Query struct)
├── client.rs           # ClaudeSdkClient (low-level bidirectional client)
├── sessions.rs         # Session listing: list_sessions, get_session_info, get_session_messages
├── session_mutations.rs # rename_session, tag_session, delete_session, fork_session
└── transport/
    ├── mod.rs           # Transport trait (abstract I/O)
    └── subprocess.rs    # SubprocessTransport (shells out to `claude` CLI)
```

### Layers

1. **Transport** (`transport/`) — raw stdin/stdout I/O with the CLI process.
2. **Query** (`query.rs`) — control protocol routing (control_request/response, hooks, MCP, permissions).
3. **Client** (`client.rs`) — `ClaudeSdkClient` wrapping Query with connect/disconnect lifecycle.
4. **Convenience** (`convenience.rs`) — `Claude`, `Chat`, `Reply` wrapping ClaudeSdkClient for ergonomics.

Changes flow top-down: new CLI protocol features start in transport/query, then surface through client, then get convenience wrappers.

## Commands

```bash
cargo build                        # Build
cargo build --all-features         # Build with MCP support (default)
cargo test --all-features          # Run all tests
cargo clippy --all-features        # Lint
cargo build --examples             # Build all examples
cargo run --example quickstart     # Run an example (needs claude CLI)
```

No special env vars needed for building. Running examples requires `claude` on `$PATH` and valid auth.

## Upstream sync

This crate tracks a single upstream Python SDK commit in `UPSTREAM.md`. The `/sync-upstream` skill automates pulling new changes. The file-level mapping:

| Python module | Rust module |
|---|---|
| `_errors.py` | `errors.rs` |
| `types.py` | `types.rs` |
| `_internal/transport/__init__.py` | `transport/mod.rs` |
| `_internal/transport/subprocess_cli.py` | `transport/subprocess.rs` |
| `_internal/message_parser.py` | `message_parser.rs` |
| `_internal/query.py` | `query.rs` |
| `_internal/client.py` | `lib.rs` (the `query()` function) |
| `client.py` | `client.rs` |
| `query.py` | `lib.rs` (the `query()` function) |
| `_internal/sessions.py` | `sessions.rs` |
| `_internal/session_mutations.py` | `session_mutations.rs` |
| `__init__.py` (MCP helpers) | `mcp.rs` |

**`convenience.rs` has no upstream equivalent** — it's Rust-only ergonomics.

## Conventions

### Naming
- Rust snake_case for fields, matching the Python SDK's snake_case.
- Wire-format camelCase preserved via `#[serde(rename = "camelCase")]`.
- Python `async_` / `continue_` (keyword collision) → Rust `async_` / `continue_` with `#[serde(rename)]`.

### Async
- `anyio.Lock` → `tokio::sync::Mutex`
- `anyio.Event` → `tokio::sync::Notify` (one-shot broadcast) or `oneshot::channel`
- `anyio.create_memory_object_stream` → `tokio::sync::mpsc::unbounded_channel`
- `asyncio.create_task` → `tokio::spawn`
- `anyio.fail_after(N)` → `tokio::time::timeout`
- `AsyncIterator[X]` → `futures::stream::Stream<Item = X>` (or `BoxStream`)

### Types
- Python `TypedDict` (open shape, forward-compat) → use `serde_json::Value` passthrough.
- Python `TypedDict` (SDK constructs the value) → Rust struct with `Serialize/Deserialize`.
- Python `Literal["a", "b"]` → Rust enum with `#[serde(rename_all)]`.
- Python `dataclass` → Rust struct. Use builder methods for `ClaudeAgentOptions`, not `Default + field assignment`.
- Callbacks (`Callable[..., Awaitable[T]]`) → `Arc<dyn Fn(...) -> BoxFuture<T> + Send + Sync>`.

### Error handling
- All fallible functions return `Result<T, ClaudeSdkError>`.
- Use `ClaudeSdkError` variants, not `.unwrap()` or `panic!`.
- Forward-compat: `parse_message` returns `Ok(None)` for unknown message types, not `Err`.

### Tests
- Unit tests live in `tests/` (integration-style, using public API).
- Doc tests on public types and the convenience module.
- No live CLI tests in CI — examples serve as manual integration tests.

## Adding a new feature from upstream

1. Read the upstream diff (use `/sync-upstream` or manually `git diff`).
2. Identify which Rust module maps to the changed Python file (table above).
3. Port the *behavior*, not the syntax. Python idioms → Rust idioms.
4. If it's a new message type or field: add to `types.rs`, update `message_parser.rs`.
5. If it's a new control protocol subtype: add to `query.rs`.
6. If it's user-facing: add a convenience wrapper in `convenience.rs` and a builder method on `ClaudeBuilder`.
7. Update `UPSTREAM.md` with the new commit hash.
8. Run `cargo test --all-features && cargo clippy --all-features`.

## Release process

Not yet published to crates.io. When ready:

1. Bump version in `Cargo.toml`.
2. Update `UPSTREAM.md` if syncing.
3. `cargo publish --dry-run` to check.
4. Tag: `git tag v0.1.x && git push --tags`.
5. `cargo publish`.
