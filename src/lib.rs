//! # claude-agent-sdk
//!
//! Rust SDK for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) — a 1:1
//! port of the official Python [`claude-agent-sdk`](https://github.com/anthropics/claude-agent-sdk-python)
//! library.
//!
//! ## Quick example
//!
//! ```no_run
//! use claude_agent_sdk::{query, ClaudeAgentOptions, Message, ContentBlock};
//! use futures::StreamExt;
//!
//! # async fn run() -> anyhow::Result<()> {
//! let mut stream = query("What is 2 + 2?", ClaudeAgentOptions::default()).await?;
//! while let Some(item) = stream.next().await {
//!     if let Message::Assistant(a) = item? {
//!         for block in &a.content {
//!             if let ContentBlock::Text(t) = block {
//!                 println!("{}", t.text);
//!             }
//!         }
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! ## Modules
//!
//! - [`types`] — message / option types.
//! - [`mcp`] — in-process MCP SDK server support.
//! - [`transport`] — `Transport` trait + subprocess CLI implementation.
//! - [`sessions`] / [`session_mutations`] — session listing and mutations.

#![allow(clippy::too_many_arguments)]

pub mod client;
pub mod convenience;
pub mod errors;
pub mod mcp;
pub mod message_parser;
pub mod query;
pub mod sessions;
pub mod session_mutations;
pub mod transport;
pub mod types;

use std::sync::Arc;

use futures::stream::BoxStream;
use serde_json::Value;

pub use client::ClaudeSdkClient;
pub use errors::{ClaudeSdkError, Result};
pub use mcp::{create_sdk_mcp_server, McpToolAnnotations, SdkMcpServer, SdkMcpTool};
pub use sessions::{get_session_info, get_session_messages, list_sessions};
pub use session_mutations::{delete_session, fork_session, rename_session, tag_session, ForkSessionResult};
pub use transport::Transport;
pub use types::*;
pub use convenience::{Chat, Claude, ClaudeBuilder, Reply, Turn};

/// One-shot query — fire a prompt at Claude Code and stream messages back.
///
/// For interactive sessions, use [`ClaudeSdkClient`] instead.
pub async fn query(
    prompt: impl Into<Prompt>,
    options: ClaudeAgentOptions,
) -> Result<BoxStream<'static, Result<Message>>> {
    query_with_transport(prompt, options, None).await
}

/// One-shot query with an optional custom [`Transport`].
pub async fn query_with_transport(
    prompt: impl Into<Prompt>,
    mut options: ClaudeAgentOptions,
    transport: Option<Box<dyn Transport>>,
) -> Result<BoxStream<'static, Result<Message>>> {
    let prompt: Prompt = prompt.into();

    if options.can_use_tool.is_some() {
        if matches!(prompt, Prompt::Text(_)) {
            return Err(ClaudeSdkError::InvalidArgument(
                "can_use_tool callback requires streaming prompt".into(),
            ));
        }
        if options.permission_prompt_tool_name.is_some() {
            return Err(ClaudeSdkError::InvalidArgument(
                "can_use_tool and permission_prompt_tool_name are mutually exclusive".into(),
            ));
        }
        options.permission_prompt_tool_name = Some("stdio".into());
    }

    let can_use_tool = options.can_use_tool.clone();
    let hooks = options.hooks.take();
    let agents = options.agents.clone();
    let sdk_mcp_servers = client::extract_sdk_mcp_servers(&options.mcp_servers);
    let exclude_dynamic_sections = client::preset_exclude_dynamic(&options.system_prompt);

    let transport: Box<dyn Transport> = match transport {
        Some(t) => t,
        None => Box::new(transport::subprocess::SubprocessTransport::new(options)),
    };

    let initialize_timeout = client::compute_initialize_timeout();
    let mut transport = transport;
    transport.connect().await?;

    let q = Arc::new(query::Query::new(
        transport,
        true,
        can_use_tool,
        hooks,
        sdk_mcp_servers,
        initialize_timeout,
        agents,
        exclude_dynamic_sections,
    ));
    q.start().await?;
    q.initialize().await?;

    match prompt {
        Prompt::Text(s) => {
            let msg = serde_json::json!({
                "type": "user",
                "session_id": "",
                "message": {"role": "user", "content": s},
                "parent_tool_use_id": Value::Null,
            });
            let line = format!("{}\n", serde_json::to_string(&msg)?);
            q.send_raw(&line).await?;
            let qc = q.clone();
            q.spawn_task(async move { qc.wait_for_result_and_end_input().await }).await;
        }
        Prompt::Stream(stream) => {
            let qc = q.clone();
            q.spawn_task(async move { qc.stream_input(stream).await }).await;
        }
    }

    let rx = q
        .take_receiver()
        .await
        .ok_or_else(|| ClaudeSdkError::cli_connection("Receiver already taken"))?;
    let q_for_drop = q.clone();

    let stream = async_stream::try_stream! {
        let mut rx = rx;
        while let Some(item) = rx.recv().await {
            let v = item?;
            if v.get("type").and_then(Value::as_str) == Some("end") { break; }
            if v.get("type").and_then(Value::as_str) == Some("error") {
                let msg = v.get("error").and_then(Value::as_str).unwrap_or("Unknown error").to_string();
                Err(ClaudeSdkError::cli_connection(msg))?;
                unreachable!();
            }
            if let Some(msg) = message_parser::parse_message(&v)? {
                yield msg;
            }
        }
        let _ = q_for_drop.close().await;
    };

    Ok(Box::pin(stream))
}
