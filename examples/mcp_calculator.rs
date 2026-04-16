//! In-process MCP server providing `add` and `multiply` tools.

use claude_agent_sdk::{create_sdk_mcp_server, schema, tool, Claude};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let add = tool!(
        "add",
        "Add two numbers",
        schema! { "a" => number, "b" => number },
        |args: serde_json::Value| async move {
            let sum = args["a"].as_f64().unwrap() + args["b"].as_f64().unwrap();
            json!({"content": [{"type": "text", "text": format!("Sum: {sum}")}]})
        }
    );
    let multiply = tool!(
        "multiply",
        "Multiply two numbers",
        schema! { "a" => number, "b" => number },
        |args: serde_json::Value| async move {
            let product = args["a"].as_f64().unwrap() * args["b"].as_f64().unwrap();
            json!({"content": [{"type": "text", "text": format!("Product: {product}")}]})
        }
    );

    let server = create_sdk_mcp_server("calculator", "1.0.0", vec![add, multiply]);

    let reply = Claude::builder()
        .add_sdk_mcp_server("calc", server)
        .allowed_tools(["mcp__calc__add", "mcp__calc__multiply"])
        .ask("Use the calculator to compute (3 + 4) * 5")
        .await?;

    println!("{}", reply.text);
    println!(
        "Tools used: {:?}",
        reply.tool_uses.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    Ok(())
}
