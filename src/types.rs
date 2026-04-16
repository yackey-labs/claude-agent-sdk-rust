//! Type definitions for the Claude Agent SDK.
//!
//! Mirrors `claude_agent_sdk.types` from the Python SDK. Where Python uses
//! `TypedDict` (open dict-shaped types) we use `serde_json::Value` for
//! flexibility, falling back to typed structs only where the SDK actively
//! constructs the value.

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Permission modes / beta / setting source
// ---------------------------------------------------------------------------

/// Permission mode for tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionMode {
    #[serde(rename = "default")]
    Default,
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    #[serde(rename = "plan")]
    Plan,
    #[serde(rename = "bypassPermissions")]
    BypassPermissions,
    #[serde(rename = "dontAsk")]
    DontAsk,
    #[serde(rename = "auto")]
    Auto,
}

impl PermissionMode {
    /// Wire-format value (matches the CLI flag).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Auto => "auto",
        }
    }
}

/// Beta-feature header values (`SdkBeta` in the Python SDK).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SdkBeta(pub String);

impl SdkBeta {
    pub const CONTEXT_1M_2025_08_07: &'static str = "context-1m-2025-08-07";
}

/// Source of agent / settings configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    User,
    Project,
    Local,
}

// ---------------------------------------------------------------------------
// System prompt configuration
// ---------------------------------------------------------------------------

/// System prompt configuration. Either a literal string, a preset, or a file.
#[derive(Debug, Clone)]
pub enum SystemPrompt {
    /// A literal system prompt string.
    Text(String),
    /// Use the built-in `claude_code` preset, optionally appending content.
    Preset {
        /// Additional text appended after the preset.
        append: Option<String>,
        /// Strip per-user dynamic sections so the prompt stays cacheable.
        exclude_dynamic_sections: Option<bool>,
    },
    /// Load the system prompt from a file.
    File(PathBuf),
}

// ---------------------------------------------------------------------------
// Tools preset
// ---------------------------------------------------------------------------

/// Tools configuration. Either an explicit allowlist or the `claude_code`
/// preset (which the CLI maps to `default`).
#[derive(Debug, Clone)]
pub enum ToolsConfig {
    /// Explicit tool list (an empty vec disables all built-in tools).
    Explicit(Vec<String>),
    /// The `claude_code` preset.
    PresetClaudeCode,
}

// ---------------------------------------------------------------------------
// Task budget
// ---------------------------------------------------------------------------

/// API-side task budget in tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TaskBudget {
    pub total: u64,
}

// ---------------------------------------------------------------------------
// Agent definition
// ---------------------------------------------------------------------------

/// Definition of a sub-agent that can be invoked from a session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDefinition {
    pub description: String,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "disallowedTools")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// One of `"user"`, `"project"`, `"local"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    /// Each entry is either a server name or an inline `{name: config}` object.
    #[serde(skip_serializing_if = "Option::is_none", rename = "mcpServers")]
    pub mcp_servers: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "initialPrompt")]
    pub initial_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxTurns")]
    pub max_turns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    /// `"low" | "medium" | "high" | "max"` or an integer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "permissionMode")]
    pub permission_mode: Option<PermissionMode>,
}

// ---------------------------------------------------------------------------
// Permission updates
// ---------------------------------------------------------------------------

/// Destination scope for a permission update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionUpdateDestination {
    #[serde(rename = "userSettings")]
    UserSettings,
    #[serde(rename = "projectSettings")]
    ProjectSettings,
    #[serde(rename = "localSettings")]
    LocalSettings,
    #[serde(rename = "session")]
    Session,
}

/// Permission rule behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

/// One rule inside a permission update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRuleValue {
    #[serde(rename = "toolName")]
    pub tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "ruleContent")]
    pub rule_content: Option<String>,
}

/// A permission update issued from a `can_use_tool` callback.
#[derive(Debug, Clone)]
pub struct PermissionUpdate {
    pub kind: PermissionUpdateKind,
    pub destination: Option<PermissionUpdateDestination>,
}

/// Variants of a permission update.
#[derive(Debug, Clone)]
pub enum PermissionUpdateKind {
    AddRules { rules: Vec<PermissionRuleValue>, behavior: Option<PermissionBehavior> },
    ReplaceRules { rules: Vec<PermissionRuleValue>, behavior: Option<PermissionBehavior> },
    RemoveRules { rules: Vec<PermissionRuleValue>, behavior: Option<PermissionBehavior> },
    SetMode { mode: PermissionMode },
    AddDirectories { directories: Vec<String> },
    RemoveDirectories { directories: Vec<String> },
}

impl PermissionUpdate {
    /// Convert to wire-format dictionary expected by the control protocol.
    pub fn to_value(&self) -> Value {
        let mut obj = serde_json::Map::new();
        let (type_name, body) = match &self.kind {
            PermissionUpdateKind::AddRules { rules, behavior } => {
                ("addRules", Some((rules.clone(), *behavior)))
            }
            PermissionUpdateKind::ReplaceRules { rules, behavior } => {
                ("replaceRules", Some((rules.clone(), *behavior)))
            }
            PermissionUpdateKind::RemoveRules { rules, behavior } => {
                ("removeRules", Some((rules.clone(), *behavior)))
            }
            PermissionUpdateKind::SetMode { mode } => {
                obj.insert("type".into(), Value::String("setMode".into()));
                if let Some(d) = &self.destination {
                    obj.insert("destination".into(), serde_json::to_value(d).unwrap());
                }
                obj.insert("mode".into(), Value::String(mode.as_str().to_string()));
                return Value::Object(obj);
            }
            PermissionUpdateKind::AddDirectories { directories } => {
                obj.insert("type".into(), Value::String("addDirectories".into()));
                if let Some(d) = &self.destination {
                    obj.insert("destination".into(), serde_json::to_value(d).unwrap());
                }
                obj.insert(
                    "directories".into(),
                    Value::Array(directories.iter().cloned().map(Value::String).collect()),
                );
                return Value::Object(obj);
            }
            PermissionUpdateKind::RemoveDirectories { directories } => {
                obj.insert("type".into(), Value::String("removeDirectories".into()));
                if let Some(d) = &self.destination {
                    obj.insert("destination".into(), serde_json::to_value(d).unwrap());
                }
                obj.insert(
                    "directories".into(),
                    Value::Array(directories.iter().cloned().map(Value::String).collect()),
                );
                return Value::Object(obj);
            }
        };
        obj.insert("type".into(), Value::String(type_name.into()));
        if let Some(d) = &self.destination {
            obj.insert("destination".into(), serde_json::to_value(d).unwrap());
        }
        if let Some((rules, behavior)) = body {
            obj.insert("rules".into(), serde_json::to_value(rules).unwrap());
            if let Some(b) = behavior {
                obj.insert("behavior".into(), serde_json::to_value(b).unwrap());
            }
        }
        Value::Object(obj)
    }
}

// ---------------------------------------------------------------------------
// Tool permission callbacks
// ---------------------------------------------------------------------------

/// Context information for tool permission callbacks.
#[derive(Debug, Clone, Default)]
pub struct ToolPermissionContext {
    /// Permission suggestions from CLI (raw JSON values).
    pub suggestions: Vec<Value>,
    /// Unique identifier for this specific tool call.
    pub tool_use_id: Option<String>,
    /// If running within the context of a sub-agent, the sub-agent's ID.
    pub agent_id: Option<String>,
}

/// Result of a `can_use_tool` callback.
#[derive(Debug, Clone)]
pub enum PermissionResult {
    Allow {
        /// Optional override of the input passed to the tool.
        updated_input: Option<Value>,
        /// Optional permission rule updates to apply.
        updated_permissions: Option<Vec<PermissionUpdate>>,
    },
    Deny {
        message: String,
        interrupt: bool,
    },
}

/// Type of `can_use_tool` callback. Returns a future resolving to a [`PermissionResult`].
pub type CanUseTool = Arc<
    dyn Fn(String, Value, ToolPermissionContext) -> BoxFuture<'static, PermissionResult>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// Hook events / inputs / outputs
// ---------------------------------------------------------------------------

/// Hook event types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    UserPromptSubmit,
    Stop,
    SubagentStop,
    PreCompact,
    Notification,
    SubagentStart,
    PermissionRequest,
}

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::Notification => "Notification",
            Self::SubagentStart => "SubagentStart",
            Self::PermissionRequest => "PermissionRequest",
        }
    }
}

/// Strongly-typed hook input. The variant depends on the `hook_event_name` field
/// in the wire payload. The full raw JSON is always preserved on the
/// [`HookInput::raw`] field for forward-compatibility.
#[derive(Debug, Clone)]
pub struct HookInput {
    pub event: HookEvent,
    /// The original wire payload — read fields off this if you need anything
    /// not surfaced as a typed accessor.
    pub raw: Value,
}

impl HookInput {
    pub fn new(event: HookEvent, raw: Value) -> Self { Self { event, raw } }

    pub fn session_id(&self) -> Option<&str> {
        self.raw.get("session_id").and_then(Value::as_str)
    }
    pub fn cwd(&self) -> Option<&str> {
        self.raw.get("cwd").and_then(Value::as_str)
    }
    pub fn tool_name(&self) -> Option<&str> {
        self.raw.get("tool_name").and_then(Value::as_str)
    }
    pub fn tool_input(&self) -> Option<&Value> { self.raw.get("tool_input") }
    pub fn tool_use_id(&self) -> Option<&str> {
        self.raw.get("tool_use_id").and_then(Value::as_str)
    }
    pub fn prompt(&self) -> Option<&str> { self.raw.get("prompt").and_then(Value::as_str) }
}

/// Hook context (currently a placeholder for future abort-signal support).
#[derive(Debug, Clone, Default)]
pub struct HookContext {}

/// A hook callback's output. Mirrors `HookJSONOutput` in the Python SDK.
///
/// Field names use Python-safe `async_` / `continue_` for parity with the
/// upstream API; they are translated to `async`/`continue` on the wire.
#[derive(Debug, Clone, Default, Serialize)]
pub struct HookOutput {
    /// Set to `Some(true)` to defer hook execution.
    #[serde(skip_serializing_if = "Option::is_none", rename = "async")]
    pub async_: Option<bool>,
    /// Optional async timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none", rename = "asyncTimeout")]
    pub async_timeout: Option<u64>,
    /// Whether Claude should proceed after hook execution (default `true`).
    #[serde(skip_serializing_if = "Option::is_none", rename = "continue")]
    pub continue_: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "suppressOutput")]
    pub suppress_output: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "stopReason")]
    pub stop_reason: Option<String>,
    /// Decision field — currently only `"block"` is meaningful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemMessage")]
    pub system_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Hook-event-specific output. See the hook docs for shape per event.
    #[serde(skip_serializing_if = "Option::is_none", rename = "hookSpecificOutput")]
    pub hook_specific_output: Option<Value>,
}

/// Type of a hook callback. Returns a future resolving to a [`HookOutput`].
pub type HookCallback = Arc<
    dyn Fn(HookInput, Option<String>, HookContext) -> BoxFuture<'static, HookOutput>
        + Send
        + Sync,
>;

/// Hook matcher — pairs a tool-name regex with one or more callbacks.
#[derive(Clone)]
pub struct HookMatcher {
    /// Regex-style matcher (e.g. `"Bash"` or `"Write|MultiEdit|Edit"`).
    pub matcher: Option<String>,
    pub hooks: Vec<HookCallback>,
    /// Timeout in seconds for all hooks in this matcher (default 60).
    pub timeout: Option<f64>,
}

impl std::fmt::Debug for HookMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookMatcher")
            .field("matcher", &self.matcher)
            .field("hooks", &format!("<{} callbacks>", self.hooks.len()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl HookMatcher {
    pub fn new() -> Self { Self { matcher: None, hooks: Vec::new(), timeout: None } }
    pub fn with_matcher(mut self, matcher: impl Into<String>) -> Self {
        self.matcher = Some(matcher.into());
        self
    }
    pub fn with_callback(mut self, cb: HookCallback) -> Self {
        self.hooks.push(cb);
        self
    }
    pub fn with_timeout(mut self, timeout_secs: f64) -> Self {
        self.timeout = Some(timeout_secs);
        self
    }
}

impl Default for HookMatcher {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// MCP server configuration
// ---------------------------------------------------------------------------

/// MCP server configuration.
#[derive(Debug, Clone)]
pub enum McpServerConfig {
    /// stdio-launched server.
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// SSE server.
    Sse { url: String, headers: HashMap<String, String> },
    /// HTTP server.
    Http { url: String, headers: HashMap<String, String> },
    /// SDK in-process server. The instance is stored as an [`SdkMcpServer`].
    Sdk { name: String, server: Arc<crate::mcp::SdkMcpServer> },
}

impl McpServerConfig {
    /// Wire representation passed to the CLI via `--mcp-config`. SDK servers
    /// have their `instance` stripped (the in-process server is wired up over
    /// the control protocol).
    pub fn to_cli_value(&self) -> Value {
        match self {
            Self::Stdio { command, args, env } => serde_json::json!({
                "type": "stdio",
                "command": command,
                "args": args,
                "env": env,
            }),
            Self::Sse { url, headers } => serde_json::json!({
                "type": "sse",
                "url": url,
                "headers": headers,
            }),
            Self::Http { url, headers } => serde_json::json!({
                "type": "http",
                "url": url,
                "headers": headers,
            }),
            Self::Sdk { name, .. } => serde_json::json!({
                "type": "sdk",
                "name": name,
            }),
        }
    }
}

/// Where to source MCP server config from. Either an inline map or a file/JSON path.
#[derive(Debug, Clone)]
pub enum McpServers {
    Map(HashMap<String, McpServerConfig>),
    /// File path or JSON string passed to `--mcp-config` as-is.
    Inline(String),
}

impl Default for McpServers {
    fn default() -> Self { Self::Map(HashMap::new()) }
}

// ---------------------------------------------------------------------------
// SDK Plugin / Sandbox config (passthrough JSON for now)
// ---------------------------------------------------------------------------

/// SDK plugin configuration. Currently only `local` plugins are supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SdkPluginConfig {
    #[serde(rename = "local")]
    Local { path: String },
}

/// Sandbox settings — passed through as a JSON object.
pub type SandboxSettings = Value;

// ---------------------------------------------------------------------------
// Thinking config
// ---------------------------------------------------------------------------

/// Controls extended-thinking behavior.
#[derive(Debug, Clone, Copy)]
pub enum ThinkingConfig {
    Adaptive,
    Enabled { budget_tokens: u32 },
    Disabled,
}

/// Effort level for thinking depth.
#[derive(Debug, Clone, Copy)]
pub enum Effort {
    Low,
    Medium,
    High,
    Max,
}

impl Effort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

// ---------------------------------------------------------------------------
// Content blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextBlock { pub text: String }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingBlock {
    pub thinking: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// A content block in a user or assistant message.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text(TextBlock),
    Thinking(ThinkingBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone)]
pub struct UserMessage {
    pub content: UserContent,
    pub uuid: Option<String>,
    pub parent_tool_use_id: Option<String>,
    pub tool_use_result: Option<Value>,
}

/// Top-level error type carried on `AssistantMessage::error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantMessageError {
    AuthenticationFailed,
    BillingError,
    RateLimit,
    InvalidRequest,
    ServerError,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub parent_tool_use_id: Option<String>,
    pub error: Option<AssistantMessageError>,
    pub usage: Option<Value>,
    pub message_id: Option<String>,
    pub stop_reason: Option<String>,
    pub session_id: Option<String>,
    pub uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SystemMessage {
    pub subtype: String,
    pub data: Value,
    /// Populated for `task_started` / `task_progress` / `task_notification` subtypes.
    pub task: Option<TaskMessage>,
}

/// Task-related payload carried inside [`SystemMessage`] for the
/// `task_started` / `task_progress` / `task_notification` subtypes.
#[derive(Debug, Clone)]
pub enum TaskMessage {
    Started {
        task_id: String,
        description: String,
        uuid: String,
        session_id: String,
        tool_use_id: Option<String>,
        task_type: Option<String>,
    },
    Progress {
        task_id: String,
        description: String,
        usage: TaskUsage,
        uuid: String,
        session_id: String,
        tool_use_id: Option<String>,
        last_tool_name: Option<String>,
    },
    Notification {
        task_id: String,
        status: TaskNotificationStatus,
        output_file: String,
        summary: String,
        uuid: String,
        session_id: String,
        tool_use_id: Option<String>,
        usage: Option<TaskUsage>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TaskUsage {
    pub total_tokens: u64,
    pub tool_uses: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskNotificationStatus { Completed, Failed, Stopped }

#[derive(Debug, Clone)]
pub struct ResultMessage {
    pub subtype: String,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub is_error: bool,
    pub num_turns: u64,
    pub session_id: String,
    pub stop_reason: Option<String>,
    pub total_cost_usd: Option<f64>,
    pub usage: Option<Value>,
    pub result: Option<String>,
    pub structured_output: Option<Value>,
    pub model_usage: Option<Value>,
    pub permission_denials: Option<Vec<Value>>,
    pub errors: Option<Vec<String>>,
    pub uuid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub uuid: String,
    pub session_id: String,
    pub event: Value,
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitStatus { Allowed, AllowedWarning, Rejected }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitType { FiveHour, SevenDay, SevenDayOpus, SevenDaySonnet, Overage }

#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    pub status: RateLimitStatus,
    pub resets_at: Option<i64>,
    pub rate_limit_type: Option<RateLimitType>,
    pub utilization: Option<f64>,
    pub overage_status: Option<RateLimitStatus>,
    pub overage_resets_at: Option<i64>,
    pub overage_disabled_reason: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct RateLimitEvent {
    pub rate_limit_info: RateLimitInfo,
    pub uuid: String,
    pub session_id: String,
}

/// Top-level message yielded by the SDK message stream.
#[derive(Debug, Clone)]
pub enum Message {
    User(UserMessage),
    Assistant(AssistantMessage),
    System(SystemMessage),
    Result(ResultMessage),
    StreamEvent(StreamEvent),
    RateLimitEvent(RateLimitEvent),
}

// ---------------------------------------------------------------------------
// Session listing types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SdkSessionInfo {
    pub session_id: String,
    pub summary: String,
    pub last_modified: i64,
    pub file_size: Option<u64>,
    pub custom_title: Option<String>,
    pub first_prompt: Option<String>,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub tag: Option<String>,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMessageType { User, Assistant }

#[derive(Debug, Clone)]
pub struct SessionMessage {
    pub r#type: SessionMessageType,
    pub uuid: String,
    pub session_id: String,
    pub message: Option<Value>,
    pub parent_tool_use_id: Option<String>,
}

// ---------------------------------------------------------------------------
// MCP status / context usage — passthrough Value for forward-compat
// ---------------------------------------------------------------------------

pub type McpStatusResponse = Value;
pub type ContextUsageResponse = Value;

// ---------------------------------------------------------------------------
// ClaudeAgentOptions
// ---------------------------------------------------------------------------

/// Options for `query()` and [`crate::ClaudeSdkClient`].
///
/// Use `ClaudeAgentOptions::default()` and the `with_*` builder helpers, or
/// construct via field assignment.
#[derive(Default)]
pub struct ClaudeAgentOptions {
    pub tools: Option<ToolsConfig>,
    pub allowed_tools: Vec<String>,
    pub system_prompt: Option<SystemPrompt>,
    pub mcp_servers: McpServers,
    pub permission_mode: Option<PermissionMode>,
    pub continue_conversation: bool,
    pub resume: Option<String>,
    pub session_id: Option<String>,
    pub max_turns: Option<u64>,
    pub max_budget_usd: Option<f64>,
    pub disallowed_tools: Vec<String>,
    pub model: Option<String>,
    pub fallback_model: Option<String>,
    pub betas: Vec<String>,
    pub permission_prompt_tool_name: Option<String>,
    pub cwd: Option<PathBuf>,
    pub cli_path: Option<PathBuf>,
    /// Either a literal JSON object as a string (`"{...}"`) or a file path.
    pub settings: Option<String>,
    pub add_dirs: Vec<PathBuf>,
    pub env: HashMap<String, String>,
    /// Arbitrary extra CLI flags. `None` value = boolean flag, `Some(val)` = `--flag val`.
    pub extra_args: HashMap<String, Option<String>>,
    /// Maximum bytes to buffer when reassembling truncated stdout JSON lines.
    pub max_buffer_size: Option<usize>,
    /// Callback invoked with each stderr line.
    pub stderr: Option<StderrCallback>,
    pub can_use_tool: Option<CanUseTool>,
    pub hooks: Option<HashMap<HookEvent, Vec<HookMatcher>>>,
    pub user: Option<String>,
    pub include_partial_messages: bool,
    pub fork_session: bool,
    pub agents: Option<HashMap<String, AgentDefinition>>,
    pub setting_sources: Option<Vec<SettingSource>>,
    pub sandbox: Option<SandboxSettings>,
    pub plugins: Vec<SdkPluginConfig>,
    /// Deprecated — use `thinking` instead.
    pub max_thinking_tokens: Option<u32>,
    pub thinking: Option<ThinkingConfig>,
    pub effort: Option<Effort>,
    /// `{"type": "json_schema", "schema": {...}}` shape.
    pub output_format: Option<Value>,
    pub enable_file_checkpointing: bool,
    pub task_budget: Option<TaskBudget>,
}

impl std::fmt::Debug for ClaudeAgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeAgentOptions")
            .field("model", &self.model)
            .field("permission_mode", &self.permission_mode)
            .field("cwd", &self.cwd)
            .field("max_turns", &self.max_turns)
            .field("has_can_use_tool", &self.can_use_tool.is_some())
            .field("has_hooks", &self.hooks.is_some())
            .finish_non_exhaustive()
    }
}

/// Stderr line callback type.
pub type StderrCallback = Arc<dyn Fn(&str) + Send + Sync>;

impl ClaudeAgentOptions {
    pub fn new() -> Self { Self::default() }

    pub fn with_system_prompt(mut self, sp: impl Into<String>) -> Self {
        self.system_prompt = Some(SystemPrompt::Text(sp.into()));
        self
    }
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }
    pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
        self.permission_mode = Some(mode);
        self
    }
    pub fn with_max_turns(mut self, n: u64) -> Self {
        self.max_turns = Some(n);
        self
    }
    pub fn with_allowed_tools<I, S>(mut self, tools: I) -> Self
    where I: IntoIterator<Item = S>, S: Into<String>,
    {
        self.allowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }
    pub fn with_disallowed_tools<I, S>(mut self, tools: I) -> Self
    where I: IntoIterator<Item = S>, S: Into<String>,
    {
        self.disallowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }
    pub fn with_can_use_tool(mut self, cb: CanUseTool) -> Self {
        self.can_use_tool = Some(cb);
        self
    }
    pub fn with_mcp_servers(mut self, servers: HashMap<String, McpServerConfig>) -> Self {
        self.mcp_servers = McpServers::Map(servers);
        self
    }
    pub fn with_hook(mut self, event: HookEvent, matcher: HookMatcher) -> Self {
        self.hooks
            .get_or_insert_with(HashMap::new)
            .entry(event)
            .or_default()
            .push(matcher);
        self
    }
    pub fn with_agent(mut self, name: impl Into<String>, def: AgentDefinition) -> Self {
        self.agents
            .get_or_insert_with(HashMap::new)
            .insert(name.into(), def);
        self
    }
}

// ---------------------------------------------------------------------------
// Prompt input — accepted by `query()` and `client.query()`.
// ---------------------------------------------------------------------------

/// A streaming prompt is a stream of JSON message dicts (already in CLI wire format).
pub type PromptStream = Pin<Box<dyn futures::Stream<Item = Value> + Send>>;

/// Input prompt: either a single string or a stream of pre-formed user messages.
pub enum Prompt {
    Text(String),
    Stream(PromptStream),
}

impl From<String> for Prompt { fn from(s: String) -> Self { Prompt::Text(s) } }
impl From<&str> for Prompt { fn from(s: &str) -> Self { Prompt::Text(s.to_string()) } }
