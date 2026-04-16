//! Interactive ClaudeSdkClient — send multiple prompts on one connection.

use claude_agent_sdk::{ClaudeAgentOptions, ClaudeSdkClient, ContentBlock, Message, Prompt};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let mut client = ClaudeSdkClient::new(ClaudeAgentOptions::default());
    client.connect(None).await?;

    client.query(Prompt::Text("Say hi briefly".into()), "default").await?;
    {
        let mut s = client.receive_response().await?;
        while let Some(m) = s.next().await {
            print_message(&m?);
        }
    }

    client.query(Prompt::Text("Now count from 1 to 3".into()), "default").await?;
    {
        let mut s = client.receive_response().await?;
        while let Some(m) = s.next().await {
            print_message(&m?);
        }
    }

    client.disconnect().await?;
    Ok(())
}

fn print_message(m: &Message) {
    if let Message::Assistant(a) = m {
        for block in &a.content {
            if let ContentBlock::Text(t) = block {
                print!("{}", t.text);
            }
        }
    }
    if matches!(m, Message::Result(_)) {
        println!();
    }
}
