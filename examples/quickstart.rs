//! Minimal one-shot query — prints assistant text blocks as they arrive.

use claude_agent_sdk::{query, ClaudeAgentOptions, ContentBlock, Message};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut stream = query("What is 2 + 2?", ClaudeAgentOptions::default()).await?;
    while let Some(item) = stream.next().await {
        match item? {
            Message::Assistant(a) => {
                for block in &a.content {
                    if let ContentBlock::Text(t) = block {
                        println!("{}", t.text);
                    }
                }
            }
            Message::Result(r) => {
                println!("\n--- done in {}ms (cost ${:.4}) ---",
                    r.duration_ms,
                    r.total_cost_usd.unwrap_or(0.0));
            }
            _ => {}
        }
    }
    Ok(())
}
