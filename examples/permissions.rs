//! Permission callback — approve/deny tools programmatically.

use claude_agent_sdk::{permission_fn, Claude, PermissionResult};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cb = permission_fn(|tool_name, input, _ctx| async move {
        println!("[permission] {tool_name} requested");
        if tool_name == "Bash" {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.contains("rm ") {
                return PermissionResult::deny("Destructive commands are blocked");
            }
        }
        PermissionResult::allow()
    });

    let reply = Claude::builder()
        .can_use_tool(cb)
        .ask("Create a file called /tmp/test.txt with 'hello' in it, then delete it")
        .await?;

    println!("{}", reply.text);
    Ok(())
}
