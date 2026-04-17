# claude-agent-sdk-rust

[![Crates.io](https://img.shields.io/crates/v/claude-agent-sdk.svg)](https://crates.io/crates/claude-agent-sdk)
[![docs.rs](https://docs.rs/claude-agent-sdk/badge.svg)](https://docs.rs/claude-agent-sdk)

Rust SDK for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) — a 1:1 port of the official
[`claude-agent-sdk` Python library](https://github.com/anthropics/claude-agent-sdk-python)
with an ergonomic high-level API on top.

## Quick start

```rust,no_run
use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reply = Claude::ask("What is 2 + 2?").await?;
    println!("{}", reply.text);            // "4"
    println!("cost: ${:.4}", reply.cost_usd);
    Ok(())
}
```

## Multi-turn conversations

```rust,no_run
use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut chat = Claude::chat().await?;
    let r1 = chat.ask("Explain ownership in Rust").await?;
    println!("{}", r1.text);
    let r2 = chat.ask("Give a code example").await?;
    println!("{}", r2.text);

    // History is tracked automatically
    for turn in &chat.history {
        println!("{}: {:.80}", turn.role, turn.text);
    }
    Ok(())
}
```

## Builder for custom options

```rust,no_run
use claude_agent_sdk::{Claude, PermissionMode};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // One-shot with options
    let reply = Claude::builder()
        .model("sonnet")
        .system_prompt("You are a Rust expert. Be concise.")
        .permission_mode(PermissionMode::Auto)
        .max_turns(5)
        .cwd("/home/user/project")
        .ask("What does this project do?")
        .await?;

    // Multi-turn with builder
    let mut chat = Claude::builder()
        .model("opus")
        .system_prompt("Be concise")
        .chat()
        .await?;
    let r = chat.ask("hi").await?;
    Ok(())
}
```

## Streaming text

```rust,no_run
use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Print text as it arrives
    Claude::builder()
        .ask_streaming("Write a haiku about Rust", |text| {
            print!("{text}");
        })
        .await?;

    // Also works on Chat
    let mut chat = Claude::chat().await?;
    chat.ask_streaming("Count to 5", |text| print!("{text}")).await?;
    Ok(())
}
```

## In-process MCP tools

```rust,no_run
use claude_agent_sdk::{Claude, create_sdk_mcp_server, tool};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let add = tool!("add", "Add two numbers",
        json!({"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]}),
        |args: serde_json::Value| async move {
            let sum = args["a"].as_f64().unwrap() + args["b"].as_f64().unwrap();
            json!({"content": [{"type": "text", "text": format!("{sum}")}]})
        }
    );
    let server = create_sdk_mcp_server("calc", "1.0.0", vec![add]);

    let reply = Claude::builder()
        .add_sdk_mcp_server("calc", server)
        .allowed_tools(["mcp__calc__add"])
        .ask("What is 3 + 4?")
        .await?;

    println!("{}", reply.text);
    println!("tools used: {:?}", reply.tool_uses.iter().map(|t| &t.name).collect::<Vec<_>>());
    Ok(())
}
```

## Resume a session

```rust,no_run
use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut chat = Claude::resume("previous-session-uuid").await?;
    let r = chat.ask("continue where we left off").await?;
    Ok(())
}
```

## Structured output

```rust,no_run
use claude_agent_sdk::Claude;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Answer { result: f64, explanation: String }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reply = Claude::builder()
        .output_format(json!({
            "type": "object",
            "properties": {
                "result": {"type": "number"},
                "explanation": {"type": "string"}
            },
            "required": ["result", "explanation"]
        }))
        .ask("What is 15% of 230?")
        .await?;

    let answer: Answer = reply.parse_structured()?;
    println!("{}: {}", answer.result, answer.explanation);
    Ok(())
}
```

## API layers

The SDK has two layers — use whichever fits:

| Layer | Entry point | Use when |
|---|---|---|
| **High-level** | `Claude::ask()`, `Claude::chat()`, `ClaudeBuilder` | Most use cases — one-shot, multi-turn, MCP tools, hooks |
| **Low-level** | `query()`, `ClaudeSdkClient`, `Transport` | Custom transports, fine-grained message stream control |

The high-level API (`Claude` / `Chat` / `Reply`) is built on top of the low-level API,
so you always have an escape hatch via `Claude::builder().build()` to get a raw
`ClaudeAgentOptions` for the low-level functions.

## Prerequisites

Claude Code CLI on your `$PATH`:

```bash
npm install -g @anthropic-ai/claude-code
```

Minimum CLI version: **2.0.0**.

## Cargo features

| Feature | Default | Description |
|---|---|---|
| `mcp`  | yes | In-process MCP SDK server support (`tool!`, `create_sdk_mcp_server`). |

## Tracking upstream

This is a port of [`claude-agent-sdk-python`](https://github.com/anthropics/claude-agent-sdk-python).
The upstream commit is tracked in [`UPSTREAM.md`](./UPSTREAM.md). Run `/sync-upstream` to pull changes.

## License

MIT — see [LICENSE](./LICENSE). Derived from `claude-agent-sdk-python` (also MIT, Anthropic).
