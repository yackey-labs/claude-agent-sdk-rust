//! In-process MCP server providing `add` and `multiply` tools.

use claude_agent_sdk::{create_sdk_mcp_server, tool, Claude};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let reply = Claude::builder()
        .add_sdk_mcp_server("calc", server)
        .allowed_tools(["mcp__calc__add", "mcp__calc__multiply"])
        .ask("Use the calculator to compute (3 + 4) * 5")
        .await?;

    println!("{}", reply.text);
    println!("Tools used: {:?}", reply.tool_uses.iter().map(|t| &t.name).collect::<Vec<_>>());
    Ok(())
}
