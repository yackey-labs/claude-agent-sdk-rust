//! High-level convenience API — `Claude`, `Chat`, `Reply`.
//!
//! # One-shot
//! ```no_run
//! # async fn run() -> claude_agent_sdk::Result<()> {
//! let reply = claude_agent_sdk::Claude::ask("What is 2 + 2?").await?;
//! println!("{}", reply.text);
//! # Ok(()) }
//! ```
//!
//! # Multi-turn
//! ```no_run
//! # async fn run() -> claude_agent_sdk::Result<()> {
//! let mut chat = claude_agent_sdk::Claude::chat().await?;
//! let r1 = chat.ask("Explain ownership in Rust").await?;
//! println!("{}", r1.text);
//! let r2 = chat.ask("Give a code example").await?;
//! println!("{}", r2.text);
//! # Ok(()) }
//! ```
//!
//! # With options (builder)
//! ```no_run
//! # async fn run() -> claude_agent_sdk::Result<()> {
//! use claude_agent_sdk::PermissionMode;
//! let reply = claude_agent_sdk::Claude::builder()
//!     .model("sonnet")
//!     .system_prompt("Be concise")
//!     .permission_mode(PermissionMode::Auto)
//!     .ask("Summarize this project")
//!     .await?;
//! # Ok(()) }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;

use crate::client::ClaudeSdkClient;
use crate::errors::{ClaudeSdkError, Result};
use crate::mcp::SdkMcpServer;
use crate::types::*;

// ===========================================================================
// Reply — the response from a single turn
// ===========================================================================

/// The collected response from a single Claude turn.
///
/// Contains the concatenated text, all raw messages, the result metadata,
/// tool use details, and structured output if requested.
#[derive(Debug, Clone)]
pub struct Reply {
    /// Concatenated text from all assistant text blocks.
    pub text: String,
    /// Session ID from the result (for resuming later).
    pub session_id: String,
    /// Total cost in USD for this turn.
    pub cost_usd: f64,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// API round-trip duration in milliseconds.
    pub duration_api_ms: u64,
    /// Number of turns the model took.
    pub num_turns: u64,
    /// Model used for the response.
    pub model: Option<String>,
    /// Stop reason (e.g. "end_turn", "max_turns").
    pub stop_reason: Option<String>,
    /// Whether the result indicates an error.
    pub is_error: bool,
    /// Raw usage object from the API.
    pub usage: Option<Value>,
    /// Per-model usage breakdown.
    pub model_usage: Option<Value>,
    /// All raw messages received in this turn.
    pub messages: Vec<Message>,
    /// All assistant messages (convenience filter of `messages`).
    pub assistant_messages: Vec<AssistantMessage>,
    /// All tool use blocks across all assistant messages.
    pub tool_uses: Vec<ToolUseBlock>,
    /// Structured output if `output_format` was set.
    pub structured_output: Option<Value>,
    /// The raw result text (may differ from `text` for structured outputs).
    pub result: Option<String>,
    /// Errors reported by the CLI.
    pub errors: Vec<String>,
}

impl Reply {
    /// Build a `Reply` from collected messages.
    pub(crate) fn from_messages(messages: Vec<Message>) -> Self {
        let mut text = String::new();
        let mut assistant_messages = Vec::new();
        let mut tool_uses = Vec::new();
        let mut model: Option<String> = None;

        for msg in &messages {
            if let Message::Assistant(a) = msg {
                text.push_str(&a.text());
                if model.is_none() {
                    model = Some(a.model.clone());
                }
                for tu in a.tool_uses() {
                    tool_uses.push(tu.clone());
                }
                assistant_messages.push(a.clone());
            }
        }

        // Extract result metadata
        let result_msg = messages.iter().rev().find_map(|m| m.as_result());
        let (session_id, cost_usd, duration_ms, duration_api_ms, num_turns, stop_reason, is_error, usage, model_usage, structured_output, result_text, errors) =
            match result_msg {
                Some(r) => (
                    r.session_id.clone(),
                    r.total_cost_usd.unwrap_or(0.0),
                    r.duration_ms,
                    r.duration_api_ms,
                    r.num_turns,
                    r.stop_reason.clone(),
                    r.is_error,
                    r.usage.clone(),
                    r.model_usage.clone(),
                    r.structured_output.clone(),
                    r.result.clone(),
                    r.errors.clone().unwrap_or_default(),
                ),
                None => (String::new(), 0.0, 0, 0, 0, None, false, None, None, None, None, Vec::new()),
            };

        Reply {
            text,
            session_id,
            cost_usd,
            duration_ms,
            duration_api_ms,
            num_turns,
            model,
            stop_reason,
            is_error,
            usage,
            model_usage,
            messages,
            assistant_messages,
            tool_uses,
            structured_output,
            result: result_text,
            errors,
        }
    }

    /// Whether tools were used in this response.
    pub fn used_tools(&self) -> bool {
        !self.tool_uses.is_empty()
    }

    /// Get the structured output, parsed into a Rust type.
    pub fn parse_structured<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        let v = self.structured_output.as_ref().ok_or_else(|| {
            ClaudeSdkError::Other("No structured output in response".into())
        })?;
        serde_json::from_value(v.clone()).map_err(Into::into)
    }
}

// ===========================================================================
// Turn — a record in the conversation history
// ===========================================================================

/// A single turn in the conversation history.
#[derive(Debug, Clone)]
pub struct Turn {
    /// `"user"` or `"assistant"`.
    pub role: String,
    /// The text content of the turn.
    pub text: String,
    /// The full reply (only for assistant turns).
    pub reply: Option<Reply>,
}

// ===========================================================================
// Chat — multi-turn conversation handle
// ===========================================================================

/// A multi-turn conversation with Claude. Auto-connects on creation,
/// auto-disconnects on drop.
///
/// ```no_run
/// # async fn run() -> claude_agent_sdk::Result<()> {
/// let mut chat = claude_agent_sdk::Claude::chat().await?;
/// let r = chat.ask("hi").await?;
/// println!("{}", r.text);
/// chat.disconnect().await?; // or just drop it
/// # Ok(()) }
/// ```
pub struct Chat {
    client: ClaudeSdkClient,
    /// Conversation history — user prompts + assistant replies.
    pub history: Vec<Turn>,
    /// Session ID (available after the first reply).
    pub session_id: Option<String>,
    connected: bool,
}

impl Chat {
    /// Create and connect a new chat session.
    pub(crate) async fn new(options: ClaudeAgentOptions) -> Result<Self> {
        let mut client = ClaudeSdkClient::new(options);
        client.connect(None).await?;
        Ok(Self { client, history: Vec::new(), session_id: None, connected: true })
    }

    /// Send a prompt and collect the full response.
    pub async fn ask(&mut self, prompt: &str) -> Result<Reply> {
        self.history.push(Turn { role: "user".into(), text: prompt.to_string(), reply: None });
        self.client.query(Prompt::Text(prompt.to_string()), "default").await?;
        let mut messages = Vec::new();
        let mut stream = self.client.receive_response().await?;
        while let Some(item) = stream.next().await {
            messages.push(item?);
        }
        let reply = Reply::from_messages(messages);
        if self.session_id.is_none() && !reply.session_id.is_empty() {
            self.session_id = Some(reply.session_id.clone());
        }
        self.history.push(Turn {
            role: "assistant".into(),
            text: reply.text.clone(),
            reply: Some(reply.clone()),
        });
        Ok(reply)
    }

    /// Send a prompt and invoke a callback for each assistant text chunk
    /// as it arrives. Returns the full reply when done.
    pub async fn ask_streaming<F>(&mut self, prompt: &str, mut on_text: F) -> Result<Reply>
    where F: FnMut(&str),
    {
        self.history.push(Turn { role: "user".into(), text: prompt.to_string(), reply: None });
        self.client.query(Prompt::Text(prompt.to_string()), "default").await?;
        let mut messages = Vec::new();
        let mut stream = self.client.receive_response().await?;
        while let Some(item) = stream.next().await {
            let msg = item?;
            if let Some(t) = msg.text() {
                on_text(&t);
            }
            messages.push(msg);
        }
        let reply = Reply::from_messages(messages);
        if self.session_id.is_none() && !reply.session_id.is_empty() {
            self.session_id = Some(reply.session_id.clone());
        }
        self.history.push(Turn {
            role: "assistant".into(),
            text: reply.text.clone(),
            reply: Some(reply.clone()),
        });
        Ok(reply)
    }

    /// Send an interrupt signal to the Claude process.
    pub async fn interrupt(&self) -> Result<()> { self.client.interrupt().await }

    /// Change the model mid-conversation.
    pub async fn set_model(&self, model: &str) -> Result<()> {
        self.client.set_model(Some(model)).await
    }

    /// Change the permission mode mid-conversation.
    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<()> {
        self.client.set_permission_mode(mode).await
    }

    /// Get MCP server status.
    pub async fn mcp_status(&self) -> Result<Value> { self.client.get_mcp_status().await }

    /// Get context window usage.
    pub async fn context_usage(&self) -> Result<Value> { self.client.get_context_usage().await }

    /// Stop a running background task.
    pub async fn stop_task(&self, task_id: &str) -> Result<()> {
        self.client.stop_task(task_id).await
    }

    /// Get server initialization info (available commands, output styles).
    pub async fn server_info(&self) -> Result<Option<Value>> {
        self.client.get_server_info().await
    }

    /// Reconnect a failed MCP server.
    pub async fn reconnect_mcp_server(&self, name: &str) -> Result<()> {
        self.client.reconnect_mcp_server(name).await
    }

    /// Enable or disable an MCP server.
    pub async fn toggle_mcp_server(&self, name: &str, enabled: bool) -> Result<()> {
        self.client.toggle_mcp_server(name, enabled).await
    }

    /// Rewind tracked files to a specific user message checkpoint.
    pub async fn rewind_files(&self, user_message_id: &str) -> Result<()> {
        self.client.rewind_files(user_message_id).await
    }

    /// Disconnect the session.
    pub async fn disconnect(&mut self) -> Result<()> {
        if self.connected {
            self.connected = false;
            self.client.disconnect().await?;
        }
        Ok(())
    }
}

impl Drop for Chat {
    fn drop(&mut self) {
        // We can't reliably do async cleanup in a sync destructor. In practice,
        // the OS will clean up the subprocess when our process exits. Users
        // should call `chat.disconnect().await` for graceful shutdown.
    }
}

// ===========================================================================
// Claude — top-level entrypoint
// ===========================================================================

/// Top-level entrypoint for the Claude Agent SDK.
///
/// Provides static methods for one-shot queries and multi-turn conversations,
/// plus a builder for configuring options.
pub struct Claude;

impl Claude {
    /// One-shot: ask a question, get a reply. Simplest possible API.
    ///
    /// ```no_run
    /// # async fn run() -> claude_agent_sdk::Result<()> {
    /// let reply = claude_agent_sdk::Claude::ask("What is 2 + 2?").await?;
    /// println!("{}", reply.text); // "4"
    /// println!("cost: ${:.4}", reply.cost_usd);
    /// # Ok(()) }
    /// ```
    pub async fn ask(prompt: &str) -> Result<Reply> {
        Self::builder().ask(prompt).await
    }

    /// Start a multi-turn conversation with default options.
    ///
    /// ```no_run
    /// # async fn run() -> claude_agent_sdk::Result<()> {
    /// let mut chat = claude_agent_sdk::Claude::chat().await?;
    /// let r = chat.ask("hi").await?;
    /// let r = chat.ask("now count to 3").await?;
    /// # Ok(()) }
    /// ```
    pub async fn chat() -> Result<Chat> {
        Self::builder().chat().await
    }

    /// Resume a previous session by session ID.
    ///
    /// ```no_run
    /// # async fn run() -> claude_agent_sdk::Result<()> {
    /// let mut chat = claude_agent_sdk::Claude::resume("session-uuid").await?;
    /// let r = chat.ask("continue where we left off").await?;
    /// # Ok(()) }
    /// ```
    pub async fn resume(session_id: &str) -> Result<Chat> {
        Self::builder().resume(session_id).chat().await
    }

    /// Create a builder for configuring options before starting a query or chat.
    pub fn builder() -> ClaudeBuilder {
        ClaudeBuilder { options: ClaudeAgentOptions::default() }
    }
}

// ===========================================================================
// ClaudeBuilder — fluent configuration
// ===========================================================================

/// Fluent builder for configuring Claude queries and chat sessions.
///
/// ```no_run
/// # async fn run() -> claude_agent_sdk::Result<()> {
/// use claude_agent_sdk::{Claude, PermissionMode};
///
/// // One-shot with options
/// let reply = Claude::builder()
///     .model("opus")
///     .system_prompt("You are a Rust expert")
///     .max_turns(5)
///     .permission_mode(PermissionMode::Auto)
///     .ask("Review this function")
///     .await?;
///
/// // Multi-turn with builder
/// let mut chat = Claude::builder()
///     .model("sonnet")
///     .cwd("/home/user/project")
///     .allowed_tools(["mcp__calc__add", "mcp__calc__multiply"])
///     .chat()
///     .await?;
/// # Ok(()) }
/// ```
pub struct ClaudeBuilder {
    options: ClaudeAgentOptions,
}

impl ClaudeBuilder {
    // ---- Model / prompt config ----

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.options.model = Some(model.into());
        self
    }
    pub fn fallback_model(mut self, model: impl Into<String>) -> Self {
        self.options.fallback_model = Some(model.into());
        self
    }
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.options.system_prompt = Some(SystemPrompt::Text(prompt.into()));
        self
    }
    pub fn system_prompt_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.options.system_prompt = Some(SystemPrompt::File(path.into()));
        self
    }

    // ---- Behavior ----

    pub fn permission_mode(mut self, mode: PermissionMode) -> Self {
        self.options.permission_mode = Some(mode);
        self
    }
    pub fn max_turns(mut self, n: u64) -> Self {
        self.options.max_turns = Some(n);
        self
    }
    pub fn max_budget_usd(mut self, usd: f64) -> Self {
        self.options.max_budget_usd = Some(usd);
        self
    }
    pub fn thinking(mut self, config: ThinkingConfig) -> Self {
        self.options.thinking = Some(config);
        self
    }
    pub fn effort(mut self, effort: Effort) -> Self {
        self.options.effort = Some(effort);
        self
    }

    // ---- Tools / MCP ----

    pub fn allowed_tools<I, S>(mut self, tools: I) -> Self
    where I: IntoIterator<Item = S>, S: Into<String>,
    {
        self.options.allowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }
    pub fn disallowed_tools<I, S>(mut self, tools: I) -> Self
    where I: IntoIterator<Item = S>, S: Into<String>,
    {
        self.options.disallowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }
    pub fn mcp_servers(mut self, servers: HashMap<String, McpServerConfig>) -> Self {
        self.options.mcp_servers = McpServers::Map(servers);
        self
    }
    /// Add a single MCP server by name.
    pub fn add_mcp_server(mut self, name: impl Into<String>, config: McpServerConfig) -> Self {
        match &mut self.options.mcp_servers {
            McpServers::Map(map) => { map.insert(name.into(), config); }
            _ => {
                let mut map = HashMap::new();
                map.insert(name.into(), config);
                self.options.mcp_servers = McpServers::Map(map);
            }
        }
        self
    }
    /// Add an in-process SDK MCP server.
    pub fn add_sdk_mcp_server(self, name: impl Into<String>, server: Arc<SdkMcpServer>) -> Self {
        let n: String = name.into();
        self.add_mcp_server(n.clone(), McpServerConfig::Sdk { name: n, server })
    }
    pub fn can_use_tool(mut self, cb: CanUseTool) -> Self {
        self.options.can_use_tool = Some(cb);
        self
    }

    // ---- Hooks ----

    pub fn hook(mut self, event: HookEvent, matcher: HookMatcher) -> Self {
        self.options.hooks
            .get_or_insert_with(HashMap::new)
            .entry(event)
            .or_default()
            .push(matcher);
        self
    }

    // ---- Agents ----

    pub fn agent(mut self, name: impl Into<String>, def: AgentDefinition) -> Self {
        self.options.agents
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), def);
        self
    }

    // ---- Session / directory ----

    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.options.cwd = Some(path.into());
        self
    }
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.options.session_id = Some(id.into());
        self
    }
    /// Resume a previous session.
    pub fn resume(mut self, session_id: &str) -> Self {
        self.options.resume = Some(session_id.to_string());
        self
    }
    /// Continue the most recent session in the working directory.
    pub fn continue_conversation(mut self) -> Self {
        self.options.continue_conversation = true;
        self
    }
    pub fn cli_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.options.cli_path = Some(path.into());
        self
    }
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.env.insert(key.into(), value.into());
        self
    }

    // ---- Output ----

    /// Request structured JSON output. Pass the JSON Schema for the output.
    pub fn output_format(mut self, schema: Value) -> Self {
        self.options.output_format = Some(serde_json::json!({
            "type": "json_schema",
            "schema": schema,
        }));
        self
    }
    /// Enable file checkpointing (for `rewind_files`).
    pub fn enable_file_checkpointing(mut self) -> Self {
        self.options.enable_file_checkpointing = true;
        self
    }

    // ---- Session features ----

    /// Fork resumed sessions to a new session ID instead of continuing the original.
    pub fn fork_session(mut self) -> Self {
        self.options.fork_session = true;
        self
    }
    /// Enable partial (streaming) message events.
    pub fn include_partial_messages(mut self) -> Self {
        self.options.include_partial_messages = true;
        self
    }
    /// Set the API-side task budget in tokens.
    pub fn task_budget(mut self, total_tokens: u64) -> Self {
        self.options.task_budget = Some(TaskBudget { total: total_tokens });
        self
    }

    // ---- Settings / plugins / betas ----

    /// Set which setting sources to load (`user`, `project`, `local`).
    pub fn setting_sources(mut self, sources: Vec<SettingSource>) -> Self {
        self.options.setting_sources = Some(sources);
        self
    }
    /// Add a beta feature flag.
    pub fn beta(mut self, beta: impl Into<String>) -> Self {
        self.options.betas.push(beta.into());
        self
    }
    /// Add a local plugin directory.
    pub fn plugin_dir(mut self, path: impl Into<String>) -> Self {
        self.options.plugins.push(SdkPluginConfig::Local { path: path.into() });
        self
    }
    /// Set the `user` field (passed to the CLI for attribution).
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.options.user = Some(user.into());
        self
    }
    /// Add additional working directories.
    pub fn add_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.options.add_dirs.push(path.into());
        self
    }
    /// Settings file path or inline JSON string.
    pub fn settings(mut self, settings: impl Into<String>) -> Self {
        self.options.settings = Some(settings.into());
        self
    }

    // ---- Telemetry ----

    /// Configure OpenTelemetry export for this agent. Sets the necessary
    /// environment variables on the CLI process.
    ///
    /// ```no_run
    /// # async fn run() -> claude_agent_sdk::Result<()> {
    /// use claude_agent_sdk::{Claude, Telemetry};
    ///
    /// let reply = Claude::builder()
    ///     .telemetry(Telemetry::honeycomb("your-api-key", "my-agent"))
    ///     .ask("What files are here?")
    ///     .await?;
    /// # Ok(()) }
    /// ```
    pub fn telemetry(mut self, config: crate::telemetry::Telemetry) -> Self {
        for (k, v) in config.to_env() {
            self.options.env.insert(k, v);
        }
        self
    }

    // ---- Sandbox ----

    pub fn sandbox(mut self, settings: Value) -> Self {
        self.options.sandbox = Some(settings);
        self
    }

    // ---- Stderr ----

    pub fn stderr(mut self, cb: StderrCallback) -> Self {
        self.options.stderr = Some(cb);
        self
    }

    // ---- Extra CLI flags ----

    /// Pass an arbitrary CLI flag. `value` of `None` = boolean flag.
    pub fn extra_arg(mut self, flag: impl Into<String>, value: Option<String>) -> Self {
        self.options.extra_args.insert(flag.into(), value);
        self
    }

    // ---- Build raw options (escape hatch) ----

    /// Take the configured options (for use with the low-level `query()` or
    /// `ClaudeSdkClient` APIs directly).
    pub fn build(self) -> ClaudeAgentOptions {
        self.options
    }

    // ---- Terminal methods ----

    /// One-shot: send a prompt and return the reply.
    pub async fn ask(self, prompt: &str) -> Result<Reply> {
        let mut stream = crate::query(prompt, self.options).await?;
        let mut messages = Vec::new();
        while let Some(item) = stream.next().await {
            messages.push(item?);
        }
        Ok(Reply::from_messages(messages))
    }

    /// One-shot with streaming callback: invokes `on_text` for each text
    /// chunk as it arrives, then returns the full reply.
    pub async fn ask_streaming<F>(self, prompt: &str, mut on_text: F) -> Result<Reply>
    where F: FnMut(&str),
    {
        let mut stream = crate::query(prompt, self.options).await?;
        let mut messages = Vec::new();
        while let Some(item) = stream.next().await {
            let msg = item?;
            if let Some(t) = msg.text() {
                on_text(&t);
            }
            messages.push(msg);
        }
        Ok(Reply::from_messages(messages))
    }

    /// Start a multi-turn chat session.
    pub async fn chat(self) -> Result<Chat> {
        Chat::new(self.options).await
    }
}
