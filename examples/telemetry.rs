//! Telemetry example — export traces/metrics/logs to an OTel collector.

use claude_agent_sdk::{Claude, Telemetry};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Option 1: Honeycomb (one-liner)
    let _honeycomb = Telemetry::honeycomb("your-api-key", "my-agent");

    // Option 2: Local collector (for dev)
    let _local = Telemetry::local("my-agent");

    // Option 3: Custom configuration
    let custom = Telemetry::new("https://otel-collector.example.com:4318")
        .service_name("my-agent")
        .header("Authorization", "Bearer token")
        .resource_attr("service.version", "1.0.0")
        .resource_attr("deployment.environment", "production")
        .traces()       // spans for agent loop, model calls, tool invocations
        .metrics()      // token counters, cost, session counts
        .logs()         // structured events for prompts and tool results
        .log_tool_details()   // include tool input args (file paths, commands)
        .export_interval_ms(1000);  // flush every 1s (default: 5s for traces/logs)

    let reply = Claude::builder()
        .telemetry(custom)
        .ask("What files are in this directory?")
        .await?;

    println!("{}", reply.text);
    println!("Session: {} (check your OTel backend for traces)", reply.session_id);
    Ok(())
}
