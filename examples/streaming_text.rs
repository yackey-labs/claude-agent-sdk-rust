//! Streaming text — print text as it arrives, character by character.

use claude_agent_sdk::Claude;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // One-shot streaming
    let reply = Claude::builder()
        .ask_streaming("Write a haiku about Rust programming", |text| {
            print!("{text}");
        })
        .await?;

    println!("\n\n--- done in {}ms ---", reply.duration_ms);

    // Multi-turn streaming
    let mut chat = Claude::chat().await?;
    print!("\nClaude: ");
    let r = chat.ask_streaming("Say hi in exactly 5 words", |text| {
        print!("{text}");
    }).await?;
    println!(" ({}ms)", r.duration_ms);

    chat.disconnect().await?;
    Ok(())
}
