//! In-process MCP SDK server support.
//!
//! Mirrors `create_sdk_mcp_server` and the `@tool` decorator from the Python
//! SDK. Tools are async functions that accept a JSON `Value` argument and
//! return a JSON `Value` result of the shape `{"content": [...], "is_error": bool?}`.

use std::collections::HashMap;
use std::sync::Arc;

use futures::future::BoxFuture;
use serde_json::{json, Value};
use tracing::warn;

/// Tool annotations (matches the MCP spec's `ToolAnnotations`).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct McpToolAnnotations {
    #[serde(skip_serializing_if = "Option::is_none", rename = "readOnlyHint")]
    pub read_only_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "destructiveHint")]
    pub destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "idempotentHint")]
    pub idempotent_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "openWorldHint")]
    pub open_world_hint: Option<bool>,
    /// Anthropic-specific: max characters of tool result before the CLI spills
    /// to disk. Surfaced to the CLI via `_meta["anthropic/maxResultSizeChars"]`.
    #[serde(skip)]
    pub max_result_size_chars: Option<u32>,
}

/// Handler for an SDK MCP tool.
pub type ToolHandler =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Value> + Send + Sync>;

/// Definition of a single in-process MCP tool.
#[derive(Clone)]
pub struct SdkMcpTool {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's input.
    pub input_schema: Value,
    pub handler: ToolHandler,
    pub annotations: Option<McpToolAnnotations>,
}

impl std::fmt::Debug for SdkMcpTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkMcpTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("input_schema", &self.input_schema)
            .finish_non_exhaustive()
    }
}

impl SdkMcpTool {
    /// Construct a new tool definition. `input_schema` should be a JSON Schema
    /// object (e.g. `json!({"type": "object", "properties": {...}})`).
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: ToolHandler,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            handler,
            annotations: None,
        }
    }

    pub fn with_annotations(mut self, ann: McpToolAnnotations) -> Self {
        self.annotations = Some(ann);
        self
    }
}

/// In-process MCP server.
pub struct SdkMcpServer {
    pub name: String,
    pub version: String,
    pub tools: HashMap<String, SdkMcpTool>,
}

impl std::fmt::Debug for SdkMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkMcpServer")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Create an in-process MCP server (parity with Python `create_sdk_mcp_server`).
pub fn create_sdk_mcp_server(
    name: impl Into<String>,
    version: impl Into<String>,
    tools: Vec<SdkMcpTool>,
) -> Arc<SdkMcpServer> {
    let mut tool_map = HashMap::new();
    for t in tools {
        tool_map.insert(t.name.clone(), t);
    }
    Arc::new(SdkMcpServer { name: name.into(), version: version.into(), tools: tool_map })
}

impl SdkMcpServer {
    /// Handle a JSON-RPC request from the CLI. Returns the JSON-RPC response object.
    pub async fn handle_jsonrpc(&self, message: &Value) -> Value {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

        match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": self.name, "version": self.version},
                },
            }),
            "notifications/initialized" => json!({"jsonrpc": "2.0", "result": {}}),
            "tools/list" => {
                let tools: Vec<Value> = self.tools.values().map(|t| {
                    let mut obj = serde_json::Map::new();
                    obj.insert("name".into(), Value::String(t.name.clone()));
                    obj.insert("description".into(), Value::String(t.description.clone()));
                    obj.insert("inputSchema".into(), t.input_schema.clone());
                    if let Some(ann) = &t.annotations {
                        obj.insert("annotations".into(), serde_json::to_value(ann).unwrap_or(Value::Null));
                        if let Some(max) = ann.max_result_size_chars {
                            obj.insert("_meta".into(), json!({"anthropic/maxResultSizeChars": max}));
                        }
                    }
                    Value::Object(obj)
                }).collect();
                json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools}})
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let Some(tool) = self.tools.get(name) else {
                    return json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": format!("Tool '{name}' not found")},
                    });
                };
                let handler = tool.handler.clone();
                let result = handler(args).await;
                let content = result
                    .get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| {
                        warn!("SDK MCP tool '{name}' returned no `content` field");
                        Vec::new()
                    });
                let mut response = serde_json::Map::new();
                response.insert("content".into(), Value::Array(content));
                if matches!(result.get("is_error"), Some(Value::Bool(true))) {
                    response.insert("isError".into(), Value::Bool(true));
                }
                json!({"jsonrpc": "2.0", "id": id, "result": Value::Object(response)})
            }
            _ => json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("Method '{method}' not found")},
            }),
        }
    }
}

/// Helper macro for defining an SDK MCP tool with an async handler closure.
///
/// # Example
/// ```ignore
/// use serde_json::json;
/// use claude_agent_sdk::{tool, mcp::SdkMcpTool};
///
/// let greet = tool!(
///     "greet",
///     "Greet a user",
///     json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
///     |args| async move {
///         let name = args["name"].as_str().unwrap_or("world");
///         json!({"content": [{"type": "text", "text": format!("Hello, {name}!")}]})
///     }
/// );
/// ```
#[macro_export]
macro_rules! tool {
    ($name:expr, $description:expr, $schema:expr, $handler:expr $(,)?) => {{
        use std::sync::Arc;
        let h = $handler;
        $crate::mcp::SdkMcpTool::new(
            $name,
            $description,
            $schema,
            Arc::new(move |args: serde_json::Value| {
                Box::pin(h(args)) as ::futures::future::BoxFuture<'static, serde_json::Value>
            }),
        )
    }};
}
