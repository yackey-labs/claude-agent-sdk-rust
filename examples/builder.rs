//! Builder pattern — one-shot with custom options.

use claude_agent_sdk::{Claude, PermissionMode};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reply = Claude::builder()
        .model("sonnet")
        .system_prompt("You are a Rust expert. Be concise.")
        .permission_mode(PermissionMode::Plan)
        .max_turns(1)
        .ask("What's the difference between &str and String?")
        .await?;

    println!("{}", reply.text);
    println!("\nModel: {:?}", reply.model);
    println!("Cost: ${:.4}", reply.cost_usd);
    Ok(())
}
