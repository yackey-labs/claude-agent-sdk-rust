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

    /// Inverse of [`PermissionUpdate::to_value`]. Returns `None` if `v` does
    /// not match the expected wire shape (unknown `type`, missing fields).
    pub fn from_value(v: &Value) -> Option<Self> {
        let obj = v.as_object()?;
        let type_str = obj.get("type")?.as_str()?;
        let destination = obj
            .get("destination")
            .and_then(|d| serde_json::from_value::<PermissionUpdateDestination>(d.clone()).ok());
        let parse_rules = || -> Option<Vec<PermissionRuleValue>> {
            obj.get("rules")?
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| {
                            Some(PermissionRuleValue {
                                tool_name: r.get("toolName").and_then(Value::as_str)?.to_string(),
                                rule_content: r
                                    .get("ruleContent")
                                    .and_then(Value::as_str)
                                    .map(String::from),
                            })
                        })
                        .collect()
                })
        };
        let parse_behavior = || -> Option<PermissionBehavior> {
            obj.get("behavior")
                .and_then(|b| serde_json::from_value(b.clone()).ok())
        };
        let parse_dirs = || -> Option<Vec<String>> {
            obj.get("directories")?
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        };
        let kind = match type_str {
            "addRules" => PermissionUpdateKind::AddRules {
                rules: parse_rules()?,
                behavior: parse_behavior(),
            },
            "replaceRules" => PermissionUpdateKind::ReplaceRules {
                rules: parse_rules()?,
                behavior: parse_behavior(),
            },
            "removeRules" => PermissionUpdateKind::RemoveRules {
                rules: parse_rules()?,
                behavior: parse_behavior(),
            },
            "setMode" => {
                let mode_str = obj.get("mode")?.as_str()?;
                let mode = match mode_str {
                    "default" => PermissionMode::Default,
                    "acceptEdits" => PermissionMode::AcceptEdits,
                    "plan" => PermissionMode::Plan,
                    "bypassPermissions" => PermissionMode::BypassPermissions,
                    "dontAsk" => PermissionMode::DontAsk,
                    "auto" => PermissionMode::Auto,
                    _ => return None,
                };
                PermissionUpdateKind::SetMode { mode }
            }
            "addDirectories" => PermissionUpdateKind::AddDirectories {
                directories: parse_dirs()?,
            },
            "removeDirectories" => PermissionUpdateKind::RemoveDirectories {
                directories: parse_dirs()?,
            },
            _ => return None,
        };
        Some(Self { kind, destination })
    }
}

// ---------------------------------------------------------------------------
// Tool permission callbacks
// ---------------------------------------------------------------------------

/// Context information for tool permission callbacks.
///
/// `can_use_tool` only fires for tool calls the CLI evaluates to `"ask"`. It
/// is not invoked for tools auto-allowed via `allowed_tools`, `permission_mode`
/// (e.g. `"acceptEdits"` / `"bypassPermissions"`), or `permissions.allow`
/// rules in settings — those never reach a prompt. To gate every tool call
/// regardless, use a `PreToolUse` hook instead.
#[derive(Debug, Clone, Default)]
pub struct ToolPermissionContext {
    /// Permission suggestions from CLI, deserialized into [`PermissionUpdate`]
    /// instances. Unparseable entries are skipped silently.
    pub suggestions: Vec<PermissionUpdate>,
    /// Unique identifier for this specific tool call.
    pub tool_use_id: Option<String>,
    /// If running within the context of a sub-agent, the sub-agent's ID.
    pub agent_id: Option<String>,
    /// File path that triggered the permission request, when applicable
    /// (e.g. when a Bash command tries to access a path outside allowed dirs).
    pub blocked_path: Option<String>,
    /// Why this permission request was triggered. When a `PreToolUse` hook
    /// returns `permissionDecision: "ask"` with a `permissionDecisionReason`,
    /// that reason is forwarded here.
    pub decision_reason: Option<String>,
    /// Full permission prompt sentence (e.g. "Claude wants to read foo.txt").
    /// Use this as the primary prompt text when present.
    pub title: Option<String>,
    /// Short noun phrase for the tool action (e.g. "Read file"), suitable
    /// for button labels or compact UI.
    pub display_name: Option<String>,
    /// Human-readable subtitle for the permission UI.
    pub description: Option<String>,
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
    Adaptive { display: Option<ThinkingDisplay> },
    Enabled { budget_tokens: u32, display: Option<ThinkingDisplay> },
    Disabled,
}

impl ThinkingConfig {
    /// Adaptive thinking with no display preference.
    pub const fn adaptive() -> Self { Self::Adaptive { display: None } }
    /// Enabled thinking with a fixed budget and no display preference.
    pub const fn enabled(budget_tokens: u32) -> Self {
        Self::Enabled { budget_tokens, display: None }
    }
}

/// Effort level for thinking depth.
///
/// `Xhigh` is Opus 4.7 only; older models silently fall back to `High`.
#[derive(Debug, Clone, Copy)]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Controls whether thinking text is returned summarized or omitted.
///
/// Opus 4.7+ defaults to `Omitted` (signature-only); pass `Summarized` to
/// receive thinking text. Maps to the CLI's `--thinking-display` flag.
#[derive(Debug, Clone, Copy)]
pub enum ThinkingDisplay {
    Summarized,
    Omitted,
}

impl ThinkingDisplay {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Summarized => "summarized",
            Self::Omitted => "omitted",
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

/// Server-side tool name. The API executes these on the model's behalf, so
/// they appear in the message stream alongside regular `tool_use` blocks but
/// the caller never returns a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerToolName {
    Advisor,
    WebSearch,
    WebFetch,
    CodeExecution,
    BashCodeExecution,
    TextEditorCodeExecution,
    ToolSearchToolRegex,
    ToolSearchToolBm25,
    /// Forward-compat: an unrecognized server tool name. Branch on this if
    /// you need the raw string.
    #[serde(other)]
    Other,
}

/// Server-side tool use block (e.g. advisor, web_search, web_fetch).
///
/// `name` is a discriminator — branch on it to know which server tool was
/// invoked. The raw input shape is opaque to this layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerToolUseBlock {
    pub id: String,
    pub name: ServerToolName,
    /// Original `name` string from the wire (preserved verbatim so callers can
    /// distinguish server tools that decode to [`ServerToolName::Other`]).
    #[serde(skip)]
    pub name_raw: String,
    pub input: Value,
}

/// Result block returned for a server-side tool call. Mirrors
/// [`ToolResultBlock`]'s shape; `content` is the raw dict from the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerToolResultBlock {
    pub tool_use_id: String,
    pub content: Value,
}

/// A content block in a user or assistant message.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    Text(TextBlock),
    Thinking(ThinkingBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
    ServerToolUse(ServerToolUseBlock),
    ServerToolResult(ServerToolResultBlock),
}

impl ContentBlock {
    /// Extract the text if this is a [`ContentBlock::Text`].
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(t) => Some(&t.text),
            _ => None,
        }
    }
    /// Extract the thinking text if this is a [`ContentBlock::Thinking`].
    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            Self::Thinking(t) => Some(&t.thinking),
            _ => None,
        }
    }
    /// Extract the tool use if this is a [`ContentBlock::ToolUse`].
    pub fn as_tool_use(&self) -> Option<&ToolUseBlock> {
        match self {
            Self::ToolUse(t) => Some(t),
            _ => None,
        }
    }
    /// Extract the tool result if this is a [`ContentBlock::ToolResult`].
    pub fn as_tool_result(&self) -> Option<&ToolResultBlock> {
        match self {
            Self::ToolResult(t) => Some(t),
            _ => None,
        }
    }
    /// Extract the server tool use if this is a [`ContentBlock::ServerToolUse`].
    pub fn as_server_tool_use(&self) -> Option<&ServerToolUseBlock> {
        match self {
            Self::ServerToolUse(t) => Some(t),
            _ => None,
        }
    }
    /// Extract the server tool result if this is a [`ContentBlock::ServerToolResult`].
    pub fn as_server_tool_result(&self) -> Option<&ServerToolResultBlock> {
        match self {
            Self::ServerToolResult(t) => Some(t),
            _ => None,
        }
    }
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

impl AssistantMessage {
    /// Concatenate all text blocks into a single string.
    pub fn text(&self) -> String {
        self.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("")
    }
    /// All tool-use blocks in the response.
    pub fn tool_uses(&self) -> Vec<&ToolUseBlock> {
        self.content.iter().filter_map(|b| b.as_tool_use()).collect()
    }
    /// Concatenate all thinking blocks.
    pub fn thinking(&self) -> String {
        self.content.iter().filter_map(|b| b.as_thinking()).collect::<Vec<_>>().join("")
    }
}

impl Message {
    /// Extract the text from this message if it's an assistant message with text content.
    pub fn text(&self) -> Option<String> {
        match self {
            Self::Assistant(a) => {
                let t = a.text();
                if t.is_empty() { None } else { Some(t) }
            }
            _ => None,
        }
    }
    /// Downcast to [`AssistantMessage`].
    pub fn as_assistant(&self) -> Option<&AssistantMessage> {
        match self { Self::Assistant(a) => Some(a), _ => None }
    }
    /// Downcast to [`ResultMessage`].
    pub fn as_result(&self) -> Option<&ResultMessage> {
        match self { Self::Result(r) => Some(r), _ => None }
    }
    /// Downcast to [`SystemMessage`].
    pub fn as_system(&self) -> Option<&SystemMessage> {
        match self { Self::System(s) => Some(s), _ => None }
    }
    /// Downcast to [`UserMessage`].
    pub fn as_user(&self) -> Option<&UserMessage> {
        match self { Self::User(u) => Some(u), _ => None }
    }
}

#[derive(Debug, Clone)]
pub struct SystemMessage {
    pub subtype: String,
    pub data: Value,
    /// Populated for `task_started` / `task_progress` / `task_notification` subtypes.
    pub task: Option<TaskMessage>,
    /// Populated for `hook_started` / `hook_response` subtypes when
    /// `include_hook_events` is enabled.
    pub hook_event: Option<HookEventInfo>,
    /// Populated for the SDK-synthesized `mirror_error` subtype emitted when
    /// a [`SessionStore::append`] call fails.
    pub mirror_error: Option<MirrorErrorInfo>,
}

/// Hook lifecycle event info attached to a [`SystemMessage`] when
/// `include_hook_events` is enabled. The full raw payload is in
/// [`SystemMessage::data`].
#[derive(Debug, Clone)]
pub struct HookEventInfo {
    /// Name of the hook event (e.g. `"PreToolUse"`, `"PostToolUse"`, `"Stop"`).
    pub hook_event_name: String,
    pub session_id: Option<String>,
    pub uuid: Option<String>,
}

/// Non-fatal mirror failure info attached to a `mirror_error` system message.
#[derive(Debug, Clone)]
pub struct MirrorErrorInfo {
    pub key: Option<SessionKey>,
    pub error: String,
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

/// Tool use that was deferred by a `PreToolUse` hook returning `"defer"`.
///
/// When a `PreToolUse` hook returns `permissionDecision: "defer"`, the run
/// stops and the result message carries the deferred tool call here so the
/// caller can inspect it and decide whether to resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeferredToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

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
    /// Set when a `PreToolUse` hook returned `"defer"` — see [`DeferredToolUse`].
    pub deferred_tool_use: Option<DeferredToolUse>,
    pub errors: Option<Vec<String>>,
    /// HTTP status code (e.g. 429, 500, 529) of the failing API call when
    /// `is_error` is `true` and `subtype` is `"success"`. `None` otherwise.
    /// Emitted by the CLI since v2.1.110. Safe to log (no message content).
    pub api_error_status: Option<i64>,
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
// Skills config
// ---------------------------------------------------------------------------

/// Skills to enable for the main session.
///
/// This is the single place to turn skills on; you do not need to add
/// `"Skill"` to `allowed_tools` or set `setting_sources` yourself — the SDK
/// does both when this is set.
///
/// `None` (i.e. unset on `ClaudeAgentOptions`) is **not** "skills off" — the
/// CLI's own defaults still apply. To suppress every skill from the listing,
/// use [`SkillsConfig::Only`] with an empty vec.
///
/// Skills are a **context filter**, not a sandbox: unlisted skills are hidden
/// from the model's listing and rejected by the Skill tool, but their files
/// remain on disk and are reachable via Read/Bash. Don't store secrets in
/// skill files.
#[derive(Debug, Clone)]
pub enum SkillsConfig {
    /// Enable every discovered skill.
    All,
    /// Enable only the listed skills. Names match the SKILL.md `name` /
    /// directory name, or `plugin:skill` for plugin-qualified skills.
    Only(Vec<String>),
}

// ---------------------------------------------------------------------------
// Sandbox network config (passthrough JSON also works via `SandboxSettings`)
// ---------------------------------------------------------------------------

/// Network configuration for sandbox.
///
/// Mirrors the upstream `SandboxNetworkConfig` TypedDict. All fields are
/// optional; the CLI ignores absent ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxNetworkConfig {
    /// Domain names that sandboxed processes can access.
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowedDomains")]
    pub allowed_domains: Option<Vec<String>>,
    /// Domains that are always blocked, even if matched by `allowed_domains`.
    #[serde(skip_serializing_if = "Option::is_none", rename = "deniedDomains")]
    pub denied_domains: Option<Vec<String>>,
    /// When `true` in managed settings, only managed-settings `allowed_domains`
    /// are respected.
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowManagedDomainsOnly")]
    pub allow_managed_domains_only: Option<bool>,
    /// Unix socket paths accessible in sandbox (e.g., SSH agents).
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowUnixSockets")]
    pub allow_unix_sockets: Option<Vec<String>>,
    /// Allow all Unix sockets (less secure).
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowAllUnixSockets")]
    pub allow_all_unix_sockets: Option<bool>,
    /// Allow binding to localhost ports (macOS only).
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowLocalBinding")]
    pub allow_local_binding: Option<bool>,
    /// macOS only: XPC/Mach service names to allow (supports trailing wildcard).
    #[serde(skip_serializing_if = "Option::is_none", rename = "allowMachLookup")]
    pub allow_mach_lookup: Option<Vec<String>>,
    /// HTTP proxy port if bringing your own proxy.
    #[serde(skip_serializing_if = "Option::is_none", rename = "httpProxyPort")]
    pub http_proxy_port: Option<u16>,
    /// SOCKS5 proxy port if bringing your own proxy.
    #[serde(skip_serializing_if = "Option::is_none", rename = "socksProxyPort")]
    pub socks_proxy_port: Option<u16>,
}

// ---------------------------------------------------------------------------
// Session store types
// ---------------------------------------------------------------------------

/// Identifies a session transcript or subagent transcript in a [`SessionStore`].
///
/// Main transcripts have no `subpath`; subagent transcripts include a
/// `subpath` like `"subagents/agent-{id}"` that mirrors the on-disk
/// directory structure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    /// Caller-defined scope. Default: sanitized cwd. Multi-tenant deployments
    /// should set this to a tenant ID or project name.
    pub project_key: String,
    pub session_id: String,
    /// Omit (empty string) for the main transcript; set for subagent files.
    /// Opaque to the adapter — just use it as a storage key suffix.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub subpath: String,
}

/// One JSONL transcript line as observed by a [`SessionStore`] adapter.
///
/// Adapters should treat entries as pass-through blobs; round-tripping
/// `serde_json::to_string` / `from_str` is the only required invariant.
pub type SessionStoreEntry = Value;

/// Entry returned by [`SessionStore::list_sessions`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStoreListEntry {
    pub session_id: String,
    /// Last-modified time in Unix epoch milliseconds.
    pub mtime: i64,
}

/// Incrementally-maintained session summary persisted by stores via
/// `fold_session_summary`. The `data` field is opaque SDK-owned state —
/// stores MUST NOT interpret it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryEntry {
    pub session_id: String,
    /// Storage write time of the sidecar in Unix epoch milliseconds. Use the
    /// same clock source as [`SessionStoreListEntry::mtime`] for this session.
    pub mtime: i64,
    pub data: Value,
}

/// Key argument to [`SessionStore::list_subkeys`] (no `subpath`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListSubkeysKey {
    pub project_key: String,
    pub session_id: String,
}

/// Controls when transcript-mirror entries are flushed to a [`SessionStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionStoreFlushMode {
    /// Buffer entries and flush once per turn (on the `result` message) or
    /// when the pending buffer exceeds 500 entries / 1 MiB. Keeps adapter
    /// latency off the streaming hot path.
    #[default]
    Batched,
    /// Trigger a background flush after every `transcript_mirror` frame so
    /// `SessionStore::append` sees entries in near real time. Appends are
    /// still serialized in enqueue order.
    Eager,
}

impl SessionStoreFlushMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Batched => "batched",
            Self::Eager => "eager",
        }
    }
}

/// Adapter for mirroring session transcripts to external storage.
///
/// The subprocess still writes to local disk; the adapter receives a
/// secondary copy. Only [`append`](SessionStore::append) and
/// [`load`](SessionStore::load) are required — the rest have default
/// implementations that return [`ClaudeSdkError::NotImplemented`].
///
/// The SDK never deletes from your store unless you call
/// `delete_session_via_store()` with [`delete`](SessionStore::delete)
/// implemented. Retention is the adapter's responsibility — implement TTL,
/// object-storage lifecycle policies, or scheduled cleanup according to your
/// compliance requirements.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Mirror a batch of transcript entries.
    ///
    /// Called AFTER the subprocess's local write succeeds — durability is
    /// already guaranteed locally. Most entries carry a stable `uuid` that
    /// adapters should treat as an idempotency key (upsert / ignore-duplicate).
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> crate::errors::Result<()>;

    /// Load a full session for resume. Return `None` for a key that was never
    /// written.
    async fn load(
        &self,
        key: &SessionKey,
    ) -> crate::errors::Result<Option<Vec<SessionStoreEntry>>>;

    /// List sessions for a `project_key`. Returns IDs + modification times.
    /// Optional — default returns [`ClaudeSdkError::NotImplemented`].
    async fn list_sessions(
        &self,
        _project_key: &str,
    ) -> crate::errors::Result<Vec<SessionStoreListEntry>> {
        Err(crate::errors::ClaudeSdkError::NotImplemented("list_sessions"))
    }

    /// Return incrementally-maintained summaries for all sessions in one call.
    /// Optional — default returns [`ClaudeSdkError::NotImplemented`].
    async fn list_session_summaries(
        &self,
        _project_key: &str,
    ) -> crate::errors::Result<Vec<SessionSummaryEntry>> {
        Err(crate::errors::ClaudeSdkError::NotImplemented(
            "list_session_summaries",
        ))
    }

    /// Delete a session. Optional — default returns
    /// [`ClaudeSdkError::NotImplemented`]; appropriate for WORM/append-only
    /// backends.
    async fn delete(&self, _key: &SessionKey) -> crate::errors::Result<()> {
        Err(crate::errors::ClaudeSdkError::NotImplemented("delete"))
    }

    /// List all subpath keys under a session (e.g. subagent transcripts).
    /// Optional — default returns [`ClaudeSdkError::NotImplemented`].
    async fn list_subkeys(
        &self,
        _key: &SessionListSubkeysKey,
    ) -> crate::errors::Result<Vec<String>> {
        Err(crate::errors::ClaudeSdkError::NotImplemented("list_subkeys"))
    }
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
    /// Specify the base set of available built-in tools. Use the `tools` field
    /// to constrain availability; pair with `allowed_tools` to auto-approve.
    pub tools: Option<ToolsConfig>,
    /// Tool names auto-allowed without prompting for permission.
    ///
    /// Passing `"Skill"` here is deprecated. Use the [`Self::skills`] option
    /// instead, which configures everything needed (including allowing the
    /// `Skill` tool).
    pub allowed_tools: Vec<String>,
    /// System prompt configuration. `None` clears the default system prompt
    /// entirely (passes `--system-prompt ""`).
    pub system_prompt: Option<SystemPrompt>,
    pub mcp_servers: McpServers,
    /// When `true`, only use MCP servers passed via `mcp_servers`, ignoring
    /// all other MCP configurations the CLI would otherwise load (project
    /// `.mcp.json`, user/global settings, plugin-provided servers). Maps to
    /// the CLI's `--strict-mcp-config` flag.
    pub strict_mcp_config: bool,
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
    /// When `true`, the CLI emits hook lifecycle events (`PreToolUse`,
    /// `PostToolUse`, `Stop`, etc.) into the message stream as
    /// `SystemMessage`s with `subtype` of `"hook_started"` / `"hook_response"`
    /// and a populated `hook_event` field. Matches the TypeScript SDK's
    /// `includeHookEvents`.
    pub include_hook_events: bool,
    pub fork_session: bool,
    pub agents: Option<HashMap<String, AgentDefinition>>,
    /// Control which filesystem settings to load.
    ///
    /// `None` = load all sources (matches CLI defaults). `Some(vec![])`
    /// disables filesystem settings (SDK isolation mode). Must include
    /// [`SettingSource::Project`] to load CLAUDE.md files.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Skills to enable for the main session. See [`SkillsConfig`].
    pub skills: Option<SkillsConfig>,
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
    /// Mirror session transcripts to an external store. When set, every
    /// transcript line written locally is also passed to
    /// [`SessionStore::append`].
    pub session_store: Option<Arc<dyn SessionStore>>,
    /// When to flush mirrored transcript entries to `session_store`.
    /// Ignored when `session_store` is `None`. Defaults to
    /// [`SessionStoreFlushMode::Batched`].
    pub session_store_flush: SessionStoreFlushMode,
    /// Timeout for each `session_store.load()` / `list_subkeys()` call during
    /// resume materialization, in milliseconds. Default 60 000.
    pub load_timeout_ms: Option<u64>,
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
    pub fn with_skills(mut self, skills: SkillsConfig) -> Self {
        self.skills = Some(skills);
        self
    }
    pub fn with_strict_mcp_config(mut self, strict: bool) -> Self {
        self.strict_mcp_config = strict;
        self
    }
    pub fn with_include_hook_events(mut self, include: bool) -> Self {
        self.include_hook_events = include;
        self
    }
    pub fn with_session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }
    pub fn with_session_store_flush(mut self, mode: SessionStoreFlushMode) -> Self {
        self.session_store_flush = mode;
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
