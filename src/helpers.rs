//! Ergonomic helpers that reduce boilerplate for common SDK patterns.
//!
//! These don't add new functionality — they just make the existing API
//! less verbose for the 90% case.

use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::Value;

use crate::types::*;

// ===========================================================================
// Hook helpers
// ===========================================================================

/// Create a hook callback from a simple async closure.
///
/// Eliminates the `Arc::new(|..| Box::pin(async move { ... }))` boilerplate.
///
/// ```no_run
/// use claude_agent_sdk::{hook_fn, HookOutput};
///
/// let my_hook = hook_fn(|input, _tool_use_id, _ctx| async move {
///     println!("Tool: {:?}", input.tool_name());
///     HookOutput::default()
/// });
/// ```
pub fn hook_fn<F, Fut>(f: F) -> HookCallback
where
    F: Fn(HookInput, Option<String>, HookContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookOutput> + Send + 'static,
{
    Arc::new(move |input, tool_use_id, ctx| {
        Box::pin(f(input, tool_use_id, ctx)) as BoxFuture<'static, HookOutput>
    })
}

/// Create a [`HookMatcher`] that matches all tools and runs a single callback.
///
/// ```no_run
/// use claude_agent_sdk::{hook_all, HookOutput};
///
/// let matcher = hook_all(|input, _, _| async move {
///     println!("Any tool called: {:?}", input.tool_name());
///     HookOutput::default()
/// });
/// ```
pub fn hook_all<F, Fut>(f: F) -> HookMatcher
where
    F: Fn(HookInput, Option<String>, HookContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookOutput> + Send + 'static,
{
    HookMatcher::new().with_callback(hook_fn(f))
}

/// Create a [`HookMatcher`] that matches specific tools and runs a single callback.
///
/// ```no_run
/// use claude_agent_sdk::{hook_tools, HookOutput};
///
/// let matcher = hook_tools("Bash|Write", |input, _, _| async move {
///     println!("Bash or Write called");
///     HookOutput::default()
/// });
/// ```
pub fn hook_tools<F, Fut>(matcher: &str, f: F) -> HookMatcher
where
    F: Fn(HookInput, Option<String>, HookContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookOutput> + Send + 'static,
{
    HookMatcher::new()
        .with_matcher(matcher)
        .with_callback(hook_fn(f))
}

// ===========================================================================
// Permission callback helpers
// ===========================================================================

/// Create a `can_use_tool` callback from a simple async closure.
///
/// Eliminates the `Arc::new(|..| Box::pin(async move { ... }))` boilerplate.
///
/// ```no_run
/// use claude_agent_sdk::{permission_fn, PermissionResult};
///
/// let cb = permission_fn(|tool_name, input, ctx| async move {
///     if tool_name == "Bash" {
///         PermissionResult::Deny { message: "No bash".into(), interrupt: false }
///     } else {
///         PermissionResult::Allow { updated_input: None, updated_permissions: None }
///     }
/// });
/// ```
pub fn permission_fn<F, Fut>(f: F) -> CanUseTool
where
    F: Fn(String, Value, ToolPermissionContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = PermissionResult> + Send + 'static,
{
    Arc::new(move |name, input, ctx| {
        Box::pin(f(name, input, ctx)) as BoxFuture<'static, PermissionResult>
    })
}

/// Convenience: allow all tools automatically.
pub fn allow_all() -> CanUseTool {
    permission_fn(|_, _, _| async {
        PermissionResult::Allow { updated_input: None, updated_permissions: None }
    })
}

/// Convenience: deny a specific tool by name.
///
/// ```no_run
/// use claude_agent_sdk::deny_tool;
/// let cb = deny_tool("Bash", "Bash is not allowed in this context");
/// ```
pub fn deny_tool(tool: &str, message: &str) -> CanUseTool {
    let tool = tool.to_string();
    let msg = message.to_string();
    permission_fn(move |name, _, _| {
        let tool = tool.clone();
        let msg = msg.clone();
        async move {
            if name == tool {
                PermissionResult::Deny { message: msg, interrupt: false }
            } else {
                PermissionResult::Allow { updated_input: None, updated_permissions: None }
            }
        }
    })
}

// ===========================================================================
// HookOutput convenience constructors
// ===========================================================================

impl HookOutput {
    /// Block the tool from executing.
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            decision: Some("block".into()),
            reason: Some(reason.into()),
            ..Default::default()
        }
    }

    /// Allow the tool and inject additional context for the model.
    pub fn with_context(context: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(serde_json::json!({
                "additionalContext": context.into()
            })),
            ..Default::default()
        }
    }

    /// Stop the session with a reason.
    pub fn stop(reason: impl Into<String>) -> Self {
        Self {
            continue_: Some(false),
            stop_reason: Some(reason.into()),
            ..Default::default()
        }
    }
}

// ===========================================================================
// PermissionResult convenience constructors
// ===========================================================================

impl PermissionResult {
    /// Allow with no modifications.
    pub fn allow() -> Self {
        Self::Allow { updated_input: None, updated_permissions: None }
    }

    /// Allow but modify the tool input.
    pub fn allow_with_input(input: Value) -> Self {
        Self::Allow { updated_input: Some(input), updated_permissions: None }
    }

    /// Deny with a message.
    pub fn deny(message: impl Into<String>) -> Self {
        Self::Deny { message: message.into(), interrupt: false }
    }

    /// Deny and interrupt the session.
    pub fn deny_and_interrupt(message: impl Into<String>) -> Self {
        Self::Deny { message: message.into(), interrupt: true }
    }
}

// ===========================================================================
// Tool schema helpers
// ===========================================================================

/// Build a simple JSON Schema object from field definitions.
///
/// ```no_run
/// use claude_agent_sdk::schema;
///
/// let s = schema! {
///     "name" => string,
///     "age" => number,
///     "active" => boolean,
/// };
/// // Produces: {"type":"object","properties":{"name":{"type":"string"},...},"required":["name","age","active"]}
/// ```
#[macro_export]
macro_rules! schema {
    ($($field:literal => $ty:ident),+ $(,)?) => {{
        let mut props = serde_json::Map::new();
        let mut required = Vec::new();
        $(
            props.insert(
                $field.to_string(),
                serde_json::json!({"type": stringify!($ty)}),
            );
            required.push(serde_json::Value::String($field.to_string()));
        )+
        serde_json::json!({
            "type": "object",
            "properties": serde_json::Value::Object(props),
            "required": serde_json::Value::Array(required),
        })
    }};
}
