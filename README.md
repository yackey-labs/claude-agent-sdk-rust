# claude-agent-sdk

[![Crates.io](https://img.shields.io/crates/v/claude-agent-sdk.svg)](https://crates.io/crates/claude-agent-sdk)
[![docs.rs](https://docs.rs/claude-agent-sdk/badge.svg)](https://docs.rs/claude-agent-sdk)

Rust SDK for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) — a 1:1 port of the official
[`claude-agent-sdk` Python library](https://github.com/anthropics/claude-agent-sdk-python).

The SDK shells out to the Claude Code CLI (`claude`) over stdin/stdout streaming JSON, and exposes:

- `query()` — fire-and-forget one-shot queries.
- `ClaudeSdkClient` — bidirectional, stateful, interactive sessions.
- Hook callbacks (`PreToolUse`, `PostToolUse`, `UserPromptSubmit`, …).
- Tool permission callbacks (`can_use_tool`).
- In-process MCP servers via `create_sdk_mcp_server` / the `tool!` macro.
- Session listing / mutations (`list_sessions`, `rename_session`, `tag_session`,
  `delete_session`, `fork_session`).

## Prerequisites

You need the Claude Code CLI on your `$PATH` (or pass `cli_path` in `ClaudeAgentOptions`):

```bash
npm install -g @anthropic-ai/claude-code
```

Minimum Claude Code CLI version: **2.0.0**.

## Quick start

```rust,no_run
use claude_agent_sdk::{query, ClaudeAgentOptions, Message, ContentBlock};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut stream = query("What is 2 + 2?", ClaudeAgentOptions::default()).await?;
    while let Some(msg) = stream.next().await {
        if let Message::Assistant(a) = msg? {
            for block in &a.content {
                if let ContentBlock::Text(t) = block {
                    println!("{}", t.text);
                }
            }
        }
    }
    Ok(())
}
```

See [`examples/`](./examples) for streaming clients, MCP servers, hooks, and permission callbacks.

## Cargo features

| Feature | Default | Description |
|---|---|---|
| `mcp`  | yes | In-process MCP SDK server support (`tool!`, `create_sdk_mcp_server`). |

## Mapping to the Python SDK

This crate aims to be an idiomatic but faithful port. Naming conventions:

| Python | Rust |
|---|---|
| `query()` | `query()` |
| `ClaudeSDKClient` | `ClaudeSdkClient` |
| `ClaudeAgentOptions` (snake_case dataclass) | `ClaudeAgentOptions` (snake_case struct, builder via fluent setters) |
| `@tool(...)` decorator | `tool!(name, desc, schema, handler)` macro |
| `create_sdk_mcp_server` | `create_sdk_mcp_server` |
| `async for msg in ...` | `Stream<Item = Result<Message>>` |

The wire protocol (control_request / control_response) is **identical** — this SDK can be
used against any `claude` CLI build the Python SDK supports.

## Tracking upstream

The Python SDK commit this port targets is recorded in [`UPSTREAM.md`](./UPSTREAM.md). To pull
in upstream changes, run the [`/sync-upstream`](./.claude/skills/sync-upstream/SKILL.md) skill,
which diffs the Python source against the recorded commit and ports the changes here.

## License

MIT — see [LICENSE](./LICENSE). Derived from `claude-agent-sdk-python` (also MIT, © Anthropic).
