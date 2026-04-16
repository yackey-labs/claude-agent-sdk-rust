---
name: claude-agent-sdk-rust
description: |
  Guide for building AI agents and automations with the claude-agent-sdk
  Rust crate. Covers one-shot queries, multi-turn conversations, MCP tool
  servers, hooks, permission callbacks, streaming, structured output, and
  session management. Trigger when code imports claude_agent_sdk, or the
  user asks about building agents/automations in Rust with Claude Code.
---

# claude-agent-sdk Rust SDK

This skill teaches you how to use the `claude-agent-sdk` Rust crate to build
AI agents powered by Claude Code.

The SDK shells out to the `claude` CLI binary over stdin/stdout streaming JSON.
It does **not** call the Anthropic API directly — it orchestrates Claude Code,
which has tool use, file editing, bash execution, MCP servers, and all built-in
Claude Code capabilities.

## Crate setup

```toml
# Cargo.toml
[dependencies]
claude-agent-sdk = "0.1"
tokio = { version = "1", features = ["full"] }
serde_json = "1"
futures = "0.3"
```

The `claude` CLI must be on `$PATH` (or set `.cli_path()` on the builder).
Install: `npm install -g @anthropic-ai/claude-code`.

## API layers

There are two layers. **Always prefer the high-level API** unless the user
explicitly needs low-level stream control.

| Layer | Entry points | When to use |
|---|---|---|
| **High-level** | `Claude::ask()`, `Claude::chat()`, `ClaudeBuilder` | Almost always |
| **Low-level** | `query()`, `ClaudeSdkClient`, `Transport` trait | Custom transports, raw message streams |

---

## High-level API

### One-shot query

```rust
use claude_agent_sdk::Claude;

let reply = Claude::ask("What is 2 + 2?").await?;
println!("{}", reply.text);     // "4"
println!("${:.4}", reply.cost_usd);
```

### One-shot with options (builder)

```rust
use claude_agent_sdk::{Claude, PermissionMode};

let reply = Claude::builder()
    .model("sonnet")
    .system_prompt("You are a Rust expert. Be concise.")
    .permission_mode(PermissionMode::Auto)
    .max_turns(5)
    .cwd("/path/to/project")
    .ask("What does this project do?")
    .await?;
```

### Multi-turn conversation

```rust
use claude_agent_sdk::Claude;

let mut chat = Claude::chat().await?;
let r1 = chat.ask("Explain ownership in Rust").await?;
println!("{}", r1.text);
let r2 = chat.ask("Give a code example").await?;
println!("{}", r2.text);

// History is tracked automatically
for turn in &chat.history {
    println!("{}: {:.80}", turn.role, turn.text);
}

// Session ID available for resuming later
println!("session: {}", chat.session_id.as_deref().unwrap_or(""));

chat.disconnect().await?; // or just drop
```

### Multi-turn with builder

```rust
let mut chat = Claude::builder()
    .model("opus")
    .system_prompt("Be concise")
    .permission_mode(PermissionMode::Auto)
    .cwd("/path/to/project")
    .chat()
    .await?;
```

### Resume a previous session

```rust
let mut chat = Claude::resume("previous-session-uuid").await?;
let r = chat.ask("continue where we left off").await?;
```

### Streaming text (print as it arrives)

```rust
// One-shot streaming
Claude::builder()
    .ask_streaming("Write a haiku", |text| print!("{text}"))
    .await?;

// Multi-turn streaming
let mut chat = Claude::chat().await?;
chat.ask_streaming("Count to 5", |text| print!("{text}")).await?;
```

---

## Reply type

Every `.ask()` call returns a `Reply` with these fields:

| Field | Type | Description |
|---|---|---|
| `text` | `String` | Concatenated text from all assistant blocks |
| `session_id` | `String` | Session ID (for resuming) |
| `cost_usd` | `f64` | Total cost in USD |
| `duration_ms` | `u64` | Wall-clock time |
| `duration_api_ms` | `u64` | API round-trip time |
| `num_turns` | `u64` | Number of model turns |
| `model` | `Option<String>` | Model used |
| `stop_reason` | `Option<String>` | Why the model stopped |
| `is_error` | `bool` | Whether the result is an error |
| `tool_uses` | `Vec<ToolUseBlock>` | All tool calls made |
| `assistant_messages` | `Vec<AssistantMessage>` | All assistant messages |
| `messages` | `Vec<Message>` | All raw messages |
| `structured_output` | `Option<Value>` | Parsed structured output |
| `errors` | `Vec<String>` | Any errors from the CLI |

### Reply helpers

```rust
reply.used_tools()             // bool — were any tools called?
reply.parse_structured::<T>()  // deserialize structured_output into T
```

---

## In-process MCP tools

Define tools that run inside your Rust process. Claude calls them via the
control protocol — no subprocess or network hop.

```rust
use claude_agent_sdk::{Claude, create_sdk_mcp_server, tool};
use serde_json::json;

// Define tools with the tool! macro
let greet = tool!(
    "greet",                    // tool name
    "Greet a user by name",     // description
    json!({                     // JSON Schema for input
        "type": "object",
        "properties": {"name": {"type": "string"}},
        "required": ["name"]
    }),
    |args: serde_json::Value| async move {
        let name = args["name"].as_str().unwrap_or("world");
        json!({"content": [{"type": "text", "text": format!("Hello, {name}!")}]})
    }
);

// Bundle tools into an MCP server
let server = create_sdk_mcp_server("my-tools", "1.0.0", vec![greet]);

// Use with the builder
let reply = Claude::builder()
    .add_sdk_mcp_server("my-tools", server)
    .allowed_tools(["mcp__my-tools__greet"])
    .ask("Greet Alice")
    .await?;
```

**Tool naming convention**: Tools are exposed to Claude as `mcp__<server-name>__<tool-name>`.
Add them to `allowed_tools` with this full name.

**Tool handler return format**: Return a JSON object with a `content` array:
```rust
json!({
    "content": [{"type": "text", "text": "result here"}],
    "is_error": false  // optional, defaults to false
})
```

### External MCP servers (stdio/SSE/HTTP)

```rust
use claude_agent_sdk::{Claude, McpServerConfig};

let reply = Claude::builder()
    .add_mcp_server("my-server", McpServerConfig::Stdio {
        command: "node".into(),
        args: vec!["server.js".into()],
        env: Default::default(),
    })
    .allowed_tools(["mcp__my-server__some_tool"])
    .ask("Use the tool")
    .await?;
```

---

## Hooks

Hooks intercept tool execution, notifications, and other events. They run
in-process as async callbacks.

```rust
use claude_agent_sdk::*;
use std::sync::Arc;

let hook: HookCallback = Arc::new(|input, tool_use_id, ctx| {
    Box::pin(async move {
        println!("Tool: {:?}, Input: {:?}",
            input.tool_name(),
            input.tool_input());
        HookOutput::default() // allow by default
    })
});

let reply = Claude::builder()
    .hook(HookEvent::PreToolUse, HookMatcher::new()
        .with_matcher("Bash")  // only match Bash tool
        .with_callback(hook))
    .ask("List files in the current directory")
    .await?;
```

### Hook events

| Event | When | Typical use |
|---|---|---|
| `PreToolUse` | Before a tool runs | Block/modify/log tool calls |
| `PostToolUse` | After a tool succeeds | Log results, inject context |
| `PostToolUseFailure` | After a tool fails | Custom error handling |
| `UserPromptSubmit` | User prompt received | Prompt filtering |
| `Stop` | Session stopping | Cleanup |
| `SubagentStart` | Sub-agent spawning | Track agents |
| `SubagentStop` | Sub-agent done | Collect results |
| `PreCompact` | Before context compaction | Add compaction instructions |
| `Notification` | System notification | Alerts |
| `PermissionRequest` | Tool needs permission | Custom auth flows |

### Blocking a tool

```rust
let blocker: HookCallback = Arc::new(|input, _, _| {
    Box::pin(async move {
        HookOutput {
            decision: Some("block".into()),
            reason: Some("Bash is not allowed".into()),
            ..Default::default()
        }
    })
});
```

---

## Permission callbacks (can_use_tool)

Fine-grained per-tool-call permission decisions:

```rust
use claude_agent_sdk::*;
use std::sync::Arc;

let permission_cb: CanUseTool = Arc::new(|tool_name, input, context| {
    Box::pin(async move {
        if tool_name == "Bash" {
            PermissionResult::Deny {
                message: "Bash not allowed".into(),
                interrupt: false,
            }
        } else {
            PermissionResult::Allow {
                updated_input: None,
                updated_permissions: None,
            }
        }
    })
});

let reply = Claude::builder()
    .can_use_tool(permission_cb)
    .ask("Run ls -la")
    .await?;
```

**Important**: `can_use_tool` only works with streaming prompts in the low-level
API. The high-level `Claude::builder()` handles this automatically.

---

## Structured output

Request JSON output matching a schema:

```rust
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Analysis {
    summary: String,
    issues: Vec<String>,
    score: f64,
}

let reply = Claude::builder()
    .output_format(json!({
        "type": "object",
        "properties": {
            "summary": {"type": "string"},
            "issues": {"type": "array", "items": {"type": "string"}},
            "score": {"type": "number"}
        },
        "required": ["summary", "issues", "score"]
    }))
    .ask("Analyze this Rust function for correctness")
    .await?;

let analysis: Analysis = reply.parse_structured()?;
```

---

## Agent definitions (sub-agents)

Define custom sub-agents that Claude can spawn:

```rust
use claude_agent_sdk::*;

let reply = Claude::builder()
    .agent("reviewer", AgentDefinition {
        description: "Reviews code for bugs".into(),
        prompt: "You are a code reviewer. Find bugs.".into(),
        tools: Some(vec!["Read".into(), "Grep".into(), "Glob".into()]),
        model: Some("sonnet".into()),
        max_turns: Some(10),
        ..Default::default()
    })
    .ask("Use the reviewer agent to check src/lib.rs")
    .await?;
```

---

## Session management

### List sessions

```rust
use claude_agent_sdk::list_sessions;

let sessions = list_sessions(Some("/path/to/project"), Some(10), 0, true);
for s in &sessions {
    println!("{}: {} ({})", s.session_id, s.summary, s.last_modified);
}
```

### Rename / tag / delete / fork

```rust
use claude_agent_sdk::{rename_session, tag_session, delete_session, fork_session};

rename_session("uuid", "My session title", Some("/path"))?;
tag_session("uuid", Some("experiment-1"), Some("/path"))?;
delete_session("uuid", Some("/path"))?;
let fork = fork_session("uuid", Some("/path"), None, Some("Forked session"))?;
println!("new session: {}", fork.session_id);
```

### Read session messages

```rust
use claude_agent_sdk::get_session_messages;

let messages = get_session_messages("uuid", Some("/path"), Some(50), 0);
for m in &messages {
    println!("{:?}: {:?}", m.r#type, m.message);
}
```

---

## Mid-conversation controls (Chat)

```rust
let mut chat = Claude::chat().await?;
chat.ask("Start working on this").await?;

// Change model mid-conversation
chat.set_model("opus").await?;

// Change permissions
chat.set_permission_mode(PermissionMode::AcceptEdits).await?;

// Interrupt the current response
chat.interrupt().await?;

// Check MCP server status
let status = chat.mcp_status().await?;

// Check context window usage
let usage = chat.context_usage().await?;

// Stop a background task
chat.stop_task("task-id").await?;
```

---

## Message type helpers

When working with raw messages (low-level API or `reply.messages`):

```rust
// On Message
msg.text()              // Option<String> — assistant text if any
msg.as_assistant()      // Option<&AssistantMessage>
msg.as_result()         // Option<&ResultMessage>
msg.as_system()         // Option<&SystemMessage>
msg.as_user()           // Option<&UserMessage>

// On AssistantMessage
a.text()                // String — concatenated text blocks
a.tool_uses()           // Vec<&ToolUseBlock>
a.thinking()            // String — concatenated thinking blocks

// On ContentBlock
block.as_text()         // Option<&str>
block.as_thinking()     // Option<&str>
block.as_tool_use()     // Option<&ToolUseBlock>
block.as_tool_result()  // Option<&ToolResultBlock>
```

---

## ClaudeBuilder reference

All builder methods (chainable):

| Method | Description |
|---|---|
| `.model("sonnet")` | Set the model |
| `.fallback_model("haiku")` | Fallback model |
| `.system_prompt("...")` | Set system prompt text |
| `.system_prompt_file(path)` | Load system prompt from file |
| `.permission_mode(mode)` | Set permission mode |
| `.max_turns(n)` | Limit model turns |
| `.max_budget_usd(n)` | Cost limit |
| `.thinking(ThinkingConfig::Adaptive)` | Extended thinking |
| `.effort(Effort::High)` | Thinking effort level |
| `.allowed_tools([...])` | Allowlist tools |
| `.disallowed_tools([...])` | Blocklist tools |
| `.mcp_servers(map)` | Set all MCP servers |
| `.add_mcp_server(name, config)` | Add one MCP server |
| `.add_sdk_mcp_server(name, server)` | Add in-process MCP server |
| `.can_use_tool(callback)` | Permission callback |
| `.hook(event, matcher)` | Add a hook |
| `.agent(name, definition)` | Add a sub-agent |
| `.cwd(path)` | Working directory |
| `.session_id(id)` | Set session ID |
| `.resume(id)` | Resume previous session |
| `.continue_conversation()` | Continue most recent session |
| `.cli_path(path)` | Path to `claude` binary |
| `.env(key, value)` | Set environment variable |
| `.output_format(schema)` | Request structured JSON output |
| `.enable_file_checkpointing()` | Enable file rewind |
| `.sandbox(settings)` | Sandbox config |
| `.stderr(callback)` | Stderr line callback |
| `.extra_arg(flag, value)` | Pass arbitrary CLI flags |
| `.build()` | Get raw `ClaudeAgentOptions` (escape hatch) |
| `.ask(prompt)` | Terminal: one-shot query |
| `.ask_streaming(prompt, callback)` | Terminal: streaming one-shot |
| `.chat()` | Terminal: start multi-turn session |

---

## Common patterns

### Batch processing

```rust
let prompts = vec!["Summarize file A", "Summarize file B", "Summarize file C"];
let mut results = Vec::new();
for prompt in prompts {
    let reply = Claude::builder()
        .cwd("/project")
        .max_turns(3)
        .ask(prompt)
        .await?;
    results.push(reply);
}
```

### Agent pipeline (multi-step)

```rust
let mut chat = Claude::builder()
    .permission_mode(PermissionMode::Auto)
    .cwd("/project")
    .chat()
    .await?;

let analysis = chat.ask("Analyze the codebase for security issues").await?;
let plan = chat.ask("Create a plan to fix the top 3 issues").await?;
let fix = chat.ask("Implement the fixes").await?;

println!("Cost: ${:.2}", analysis.cost_usd + plan.cost_usd + fix.cost_usd);
```

### Error handling

```rust
let reply = Claude::ask("do something").await?;
if reply.is_error {
    eprintln!("Claude error: {:?}", reply.errors);
}
if !reply.errors.is_empty() {
    for err in &reply.errors {
        eprintln!("  {err}");
    }
}
```

---

## Gotchas

1. **`claude` CLI must be installed** — the SDK doesn't bundle it. Install via
   `npm install -g @anthropic-ai/claude-code`.

2. **Async runtime**: Requires tokio. The crate uses `tokio::process`,
   `tokio::sync`, and `tokio::spawn` internally.

3. **Tool name format for MCP**: Tools are `mcp__<server>__<tool>`. The double
   underscore is required.

4. **`can_use_tool` requires streaming mode** — the high-level API handles
   this automatically. If using the low-level `query()`, you must pass a
   streaming prompt (not a string) when using `can_use_tool`.

5. **Chat disconnect**: Call `chat.disconnect().await` for graceful shutdown.
   Dropping the `Chat` without disconnecting will still clean up, but the CLI
   process may not flush its session file.

6. **Cost**: Each `.ask()` call creates a full CLI → API round-trip. The CLI
   process itself uses ~333MB RSS (it's a compiled Bun/Node.js binary). The
   Rust SDK adds ~6MB on top.

7. **Structured output**: Use `.output_format(schema)` on the builder, then
   `reply.parse_structured::<T>()` to deserialize. The schema is a JSON Schema
   object, not a Rust type.
