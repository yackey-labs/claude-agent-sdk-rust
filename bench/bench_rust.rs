//! Benchmark: Rust claude-agent-sdk — one-shot query.

use claude_agent_sdk::{query, ClaudeAgentOptions, ContentBlock, Message};
use futures::StreamExt;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let options = ClaudeAgentOptions {
        max_turns: Some(1),
        ..Default::default()
    };

    let start = Instant::now();
    let mut result_text = String::new();
    let mut stream = query("What is 2 + 2? Reply with just the number.", options)
        .await
        .expect("query failed");
    while let Some(item) = stream.next().await {
        match item.expect("message error") {
            Message::Assistant(a) => {
                for block in &a.content {
                    if let ContentBlock::Text(t) = block {
                        result_text.push_str(&t.text);
                    }
                }
            }
            _ => {}
        }
    }
    let elapsed = start.elapsed();

    // Read /proc/self/status for RSS
    let rss_kb = read_rss_kb().unwrap_or(0);
    let rusage = unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut u);
        u
    };
    let child_rusage = unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_CHILDREN, &mut u);
        u
    };

    println!("answer: {}", result_text.trim());
    println!("wall_ms: {}", elapsed.as_millis());
    println!(
        "self_user_ms: {}",
        rusage.ru_utime.tv_sec * 1000 + rusage.ru_utime.tv_usec as i64 / 1000
    );
    println!(
        "self_sys_ms: {}",
        rusage.ru_stime.tv_sec * 1000 + rusage.ru_stime.tv_usec as i64 / 1000
    );
    println!(
        "child_user_ms: {}",
        child_rusage.ru_utime.tv_sec * 1000 + child_rusage.ru_utime.tv_usec as i64 / 1000
    );
    println!(
        "child_sys_ms: {}",
        child_rusage.ru_stime.tv_sec * 1000 + child_rusage.ru_stime.tv_usec as i64 / 1000
    );
    println!("max_rss_kb: {}", rss_kb);
}

fn read_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            return parts.get(1)?.parse().ok();
        }
    }
    None
}
