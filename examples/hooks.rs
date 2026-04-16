//! Hook example — log all tool calls and block Bash.

use claude_agent_sdk::{hook_all, hook_tools, Claude, HookEvent, HookOutput, PermissionMode};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Log every tool call
    let logger = hook_all(|input, _, _| async move {
        println!("[hook] {} called", input.tool_name().unwrap_or("?"));
        HookOutput::default()
    });

    // Block Bash specifically
    let bash_blocker = hook_tools("Bash", |_, _, _| async move {
        HookOutput::block("Bash is not allowed in this session")
    });

    let reply = Claude::builder()
        .permission_mode(PermissionMode::Auto)
        .hook(HookEvent::PreToolUse, logger)
        .hook(HookEvent::PreToolUse, bash_blocker)
        .ask("List files in the current directory, then tell me how many there are")
        .await?;

    println!("\n{}", reply.text);
    Ok(())
}
