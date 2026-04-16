//! Minimal one-shot query — the simplest possible usage.

use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reply = Claude::ask("What is 2 + 2? Reply with just the number.").await?;
    println!("{}", reply.text);
    println!("--- done in {}ms (cost ${:.4}) ---", reply.duration_ms, reply.cost_usd);
    Ok(())
}
