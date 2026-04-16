---
name: sync-upstream
description: |
  Sync this Rust port with the latest changes in the upstream Python
  claude-agent-sdk-python repo. Use when the user asks to "pull upstream
  changes", "sync the SDK", "update from python sdk", "what changed
  upstream", or any time the goal is to bring this crate up to date with
  https://github.com/anthropics/claude-agent-sdk-python.
---

# sync-upstream

This skill ports new upstream commits from
[`claude-agent-sdk-python`](https://github.com/anthropics/claude-agent-sdk-python)
into this Rust crate.

The crate tracks a single upstream commit hash in `UPSTREAM.md`. The skill
fetches new commits since that hash, presents the diff in `src/claude_agent_sdk/`,
and ports the changes here.

## Steps

### 1. Read the currently tracked upstream commit

```bash
grep upstream_commit /home/steve/fj/claude-agent-sdk-rust/UPSTREAM.md | head -1
```

### 2. Clone or refresh the upstream repo

```bash
if [ -d /tmp/claude-agent-sdk-python ]; then
  cd /tmp/claude-agent-sdk-python && git fetch origin && git checkout main && git pull --ff-only
else
  git clone https://github.com/anthropics/claude-agent-sdk-python /tmp/claude-agent-sdk-python
fi
```

### 3. Show what changed since the tracked commit

```bash
cd /tmp/claude-agent-sdk-python
LAST=$(grep upstream_commit /home/steve/fj/claude-agent-sdk-rust/UPSTREAM.md | head -1 | awk '{print $2}')
echo "Tracked upstream commit: $LAST"
git log --oneline "$LAST"..HEAD -- src/
git diff --stat "$LAST"..HEAD -- src/
```

If the diff is empty, stop — nothing to do; report "Already up to date".

### 4. For each changed file, port it to Rust

Map upstream Python files to Rust modules:

| Upstream file | Rust module |
|---|---|
| `src/claude_agent_sdk/_errors.py` | `src/errors.rs` |
| `src/claude_agent_sdk/types.py` | `src/types.rs` |
| `src/claude_agent_sdk/_internal/transport/subprocess_cli.py` | `src/transport/subprocess.rs` |
| `src/claude_agent_sdk/_internal/transport/__init__.py` | `src/transport/mod.rs` (`Transport` trait) |
| `src/claude_agent_sdk/_internal/message_parser.py` | `src/message_parser.rs` |
| `src/claude_agent_sdk/_internal/query.py` | `src/query.rs` |
| `src/claude_agent_sdk/_internal/client.py` | `src/lib.rs::query()` (one-shot path) |
| `src/claude_agent_sdk/client.py` | `src/client.rs` (`ClaudeSdkClient`) |
| `src/claude_agent_sdk/query.py` | `src/lib.rs::query()` |
| `src/claude_agent_sdk/_internal/sessions.py` | `src/sessions.rs` |
| `src/claude_agent_sdk/_internal/session_mutations.py` | `src/session_mutations.rs` |
| `src/claude_agent_sdk/__init__.py` (MCP server helpers, `tool` decorator) | `src/mcp.rs` (`create_sdk_mcp_server`, `tool!` macro) |
| `src/claude_agent_sdk/_version.py` | `Cargo.toml` `version` |
| `src/claude_agent_sdk/_cli_version.py` | `src/transport/subprocess.rs` `MINIMUM_CLAUDE_CODE_VERSION` |

For each changed Python file:

1. Read both old and new versions:
   ```bash
   cd /tmp/claude-agent-sdk-python
   git show "$LAST":<file> > /tmp/old.py
   cat <file> > /tmp/new.py
   diff -u /tmp/old.py /tmp/new.py
   ```
2. Read the corresponding Rust file via the table above.
3. Apply the equivalent change in Rust. **Do not** add features that don't
   exist upstream.
4. Preserve naming conventions:
   - Python `snake_case` field → Rust `snake_case` field.
   - Python `camelCase` JSON wire field → keep with `#[serde(rename = "...")]`.
   - Python `async_` / `continue_` (keyword-collision) → Rust uses `async_` /
     `continue_` field names with `#[serde(rename = "async"/"continue")]`.
   - Python `TypedDict` → Rust struct with `#[derive(Serialize, Deserialize)]`,
     OR `serde_json::Value` if the type is a passthrough.
5. Async equivalences:
   - `anyio.Lock` / `asyncio.Lock` → `tokio::sync::Mutex`.
   - `anyio.Event` → `tokio::sync::Notify` (one-shot) or `oneshot::channel` (single response).
   - `anyio.create_memory_object_stream` → `tokio::sync::mpsc::unbounded_channel`.
   - `asyncio.create_task` → `tokio::spawn`.
   - `anyio.fail_after(N)` → `tokio::time::timeout`.
   - `AsyncIterator[X]` → `futures::stream::Stream<Item = X>` (or `BoxStream`).

### 5. Bump versions and tracked commit

After porting:

```bash
cd /tmp/claude-agent-sdk-python
NEW_HASH=$(git rev-parse HEAD)
NEW_SHORT=$(git rev-parse --short HEAD)
NEW_SUBJECT=$(git log -1 --pretty=format:'%s')
NEW_DATE=$(git log -1 --pretty=format:'%cI')
TODAY=$(date -u +%Y-%m-%d)
```

Update `/home/steve/fj/claude-agent-sdk-rust/UPSTREAM.md` with the new
`upstream_commit`, `upstream_short`, `upstream_subject`, `upstream_committed`,
and `synced_at` values.

If the upstream Python `_version.py` changed, decide:
- Cosmetic / no behavior change → keep this crate's version.
- New features / bug fixes → bump patch (`x.y.Z+1`).
- Breaking API change → bump minor (`x.Y+1.0`).

Update `Cargo.toml` `version` accordingly.

### 6. Verify and commit

```bash
cd /home/steve/fj/claude-agent-sdk-rust
cargo build --all-features
cargo test --all-features
cargo clippy --all-features -- -D warnings
```

Then commit:

```bash
git add -A
git commit -m "$(cat <<EOF
chore: sync with upstream $NEW_SHORT

$NEW_SUBJECT

Upstream: https://github.com/anthropics/claude-agent-sdk-python/commit/$NEW_HASH
EOF
)"
```

## Notes

- **Don't blindly mirror Python idioms** — port the *behavior*, not the syntax.
  Python uses dataclasses for everything; Rust uses what fits (struct, enum,
  newtype, builder).
- **Forward-compat fields**: Python uses open `TypedDict`; Rust message variants
  carry `raw: Value` so unknown fields survive a roundtrip.
- **MCP**: the Python SDK delegates to `mcp` PyPI package. This crate
  reimplements just the JSON-RPC subset the CLI talks. If upstream adds a new
  MCP method (resources, prompts, etc.), add it to `SdkMcpServer::handle_jsonrpc`.
