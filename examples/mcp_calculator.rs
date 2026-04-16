//! In-process MCP server providing `add` and `multiply` tools.

use std::collections::HashMap;

use claude_agent_sdk::{
    create_sdk_mcp_server, query, tool, ClaudeAgentOptions, ContentBlock, McpServerConfig,
    McpServers, Message,
};
use futures::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let add = tool!(
        "add",
        "Add two numbers",
        json!({
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"]
        }),
        |args: serde_json::Value| async move {
            let a = args["a"].as_f64().unwrap_or(0.0);
            let b = args["b"].as_f64().unwrap_or(0.0);
            json!({"content": [{"type": "text", "text": format!("Sum: {}", a + b)}]})
        }
    );
    let multiply = tool!(
        "multiply",
        "Multiply two numbers",
        json!({
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"]
        }),
        |args: serde_json::Value| async move {
            let a = args["a"].as_f64().unwrap_or(0.0);
            let b = args["b"].as_f64().unwrap_or(0.0);
            json!({"content": [{"type": "text", "text": format!("Product: {}", a * b)}]})
        }
    );

    let server = create_sdk_mcp_server("calculator", "1.0.0", vec![add, multiply]);

    let mut servers: HashMap<String, McpServerConfig> = HashMap::new();
    servers.insert(
        "calc".into(),
        McpServerConfig::Sdk { name: "calculator".into(), server },
    );

    let options = ClaudeAgentOptions {
        mcp_servers: McpServers::Map(servers),
        allowed_tools: vec!["mcp__calc__add".into(), "mcp__calc__multiply".into()],
        ..Default::default()
    };

    let mut stream = query("Use the calculator to compute (3 + 4) * 5", options).await?;
    while let Some(item) = stream.next().await {
        if let Message::Assistant(a) = item? {
            for b in &a.content {
                if let ContentBlock::Text(t) = b {
                    println!("{}", t.text);
                }
            }
        }
    }
    Ok(())
}
