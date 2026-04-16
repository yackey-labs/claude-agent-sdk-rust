//! Multi-turn chat — send multiple prompts on one connection.

use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut chat = Claude::chat().await?;

    let r1 = chat.ask("Say hi briefly").await?;
    println!("Claude: {}", r1.text);

    let r2 = chat.ask("Now count from 1 to 3").await?;
    println!("Claude: {}", r2.text);

    // History is tracked automatically
    println!("\n--- {} turns in history ---", chat.history.len());
    for turn in &chat.history {
        println!("  {}: {:.60}", turn.role, turn.text.replace('\n', " "));
    }

    chat.disconnect().await?;
    Ok(())
}
