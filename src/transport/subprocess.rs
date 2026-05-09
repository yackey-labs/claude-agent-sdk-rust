//! Subprocess transport over the Claude Code CLI (`claude --output-format stream-json`).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::errors::{ClaudeSdkError, Result};
use crate::transport::Transport;
use crate::types::{
    ClaudeAgentOptions, McpServers, SkillsConfig, SystemPrompt, ThinkingConfig, ToolsConfig,
};

const DEFAULT_MAX_BUFFER_SIZE: usize = 1024 * 1024;
/// Minimum bundled Claude Code CLI version. Synced with the Python SDK's
/// `_cli_version.py` value (currently `2.1.137`).
pub const MINIMUM_CLAUDE_CODE_VERSION: &str = "2.0.0";
pub const BUNDLED_CLI_VERSION: &str = "2.1.137";
const SDK_VERSION: &str = env!("CARGO_PKG_VERSION");

// Track live CLI subprocess PIDs so we can SIGTERM them on parent process
// exit. Mirrors the upstream Python `_ACTIVE_CHILDREN` + `atexit` cleanup —
// prevents orphaned `claude` processes from leaking when callers crash or
// exit before awaiting close().
fn active_children() -> &'static std::sync::Mutex<Vec<u32>> {
    use std::sync::OnceLock;
    static ACTIVE: OnceLock<std::sync::Mutex<Vec<u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| {
        // Register the at-exit hook on first use (cheap + lazy, so libraries
        // that never spawn don't pay for it).
        register_atexit_kill();
        std::sync::Mutex::new(Vec::new())
    })
}

fn register_atexit_kill() {
    extern "C" fn handler() {
        if let Ok(mut g) = active_children().lock() {
            for pid in g.drain(..) {
                #[cfg(unix)]
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
                #[cfg(not(unix))]
                {
                    let _ = pid;
                }
            }
        }
    }
    unsafe {
        libc::atexit(handler);
    }
}

/// Subprocess CLI transport.
pub struct SubprocessTransport {
    options: ClaudeAgentOptions,
    cli_path: Option<PathBuf>,
    child: Option<Child>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: Option<tokio::process::ChildStdout>,
    stderr_handle: Option<tokio::task::JoinHandle<()>>,
    ready: bool,
    max_buffer_size: usize,
}

impl SubprocessTransport {
    pub fn new(options: ClaudeAgentOptions) -> Self {
        let max_buffer_size = options.max_buffer_size.unwrap_or(DEFAULT_MAX_BUFFER_SIZE);
        Self {
            options,
            cli_path: None,
            child: None,
            stdin: Arc::new(Mutex::new(None)),
            stdout: None,
            stderr_handle: None,
            ready: false,
            max_buffer_size,
        }
    }

    fn find_cli(&self) -> Result<PathBuf> {
        if let Some(p) = &self.options.cli_path {
            return Ok(p.clone());
        }
        if let Ok(p) = which::which("claude") {
            return Ok(p);
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let candidates = [
            format!("{home}/.npm-global/bin/claude"),
            "/usr/local/bin/claude".to_string(),
            format!("{home}/.local/bin/claude"),
            format!("{home}/node_modules/.bin/claude"),
            format!("{home}/.yarn/bin/claude"),
            format!("{home}/.claude/local/claude"),
        ];
        for c in &candidates {
            let p = PathBuf::from(c);
            if p.is_file() {
                return Ok(p);
            }
        }
        Err(ClaudeSdkError::cli_not_found(
            "Claude Code not found. Install with:\n  \
             npm install -g @anthropic-ai/claude-code\n\
             Or set ClaudeAgentOptions::cli_path.",
            None,
        ))
    }

    fn build_settings_value(&self) -> Result<Option<String>> {
        let has_settings = self.options.settings.is_some();
        let has_sandbox = self.options.sandbox.is_some();
        if !has_settings && !has_sandbox {
            return Ok(None);
        }
        if has_settings && !has_sandbox {
            return Ok(self.options.settings.clone());
        }
        // Merge: parse settings (JSON or file) into object, then add sandbox.
        let mut obj = serde_json::Map::new();
        if let Some(s) = &self.options.settings {
            let trimmed = s.trim();
            let parsed: Option<Value> = if trimmed.starts_with('{') && trimmed.ends_with('}') {
                serde_json::from_str(trimmed).ok()
            } else {
                std::fs::read_to_string(trimmed).ok().and_then(|c| serde_json::from_str(&c).ok())
            };
            if let Some(Value::Object(map)) = parsed {
                obj = map;
            }
        }
        if let Some(sandbox) = &self.options.sandbox {
            obj.insert("sandbox".into(), sandbox.clone());
        }
        Ok(Some(serde_json::to_string(&Value::Object(obj))?))
    }

    /// Compute effective allowed_tools and setting_sources, applying
    /// `skills` defaults. When `options.skills == SkillsConfig::All`, the
    /// bare `Skill` tool is appended to allowed_tools; for `SkillsConfig::Only`,
    /// `Skill(name)` is appended for each entry. In either case
    /// `setting_sources` defaults to `[user, project]` when unset so the CLI
    /// discovers installed skills without the caller wiring up both options
    /// manually. `None` is a no-op.
    fn apply_skills_defaults(&self) -> (Vec<String>, Option<Vec<crate::types::SettingSource>>) {
        let mut allowed_tools: Vec<String> = self.options.allowed_tools.clone();
        let mut setting_sources: Option<Vec<crate::types::SettingSource>> =
            self.options.setting_sources.clone();
        match &self.options.skills {
            None => return (allowed_tools, setting_sources),
            Some(SkillsConfig::All) => {
                if !allowed_tools.iter().any(|t| t == "Skill") {
                    allowed_tools.push("Skill".to_string());
                }
            }
            Some(SkillsConfig::Only(names)) => {
                for name in names {
                    let pat = format!("Skill({name})");
                    if !allowed_tools.contains(&pat) {
                        allowed_tools.push(pat);
                    }
                }
            }
        }
        if setting_sources.is_none() {
            setting_sources = Some(vec![
                crate::types::SettingSource::User,
                crate::types::SettingSource::Project,
            ]);
        }
        (allowed_tools, setting_sources)
    }

    fn build_command(&self) -> Result<Vec<String>> {
        let cli = self.cli_path.as_ref().ok_or_else(|| {
            ClaudeSdkError::cli_not_found("CLI path not resolved. Call connect() first.", None)
        })?;
        let mut cmd: Vec<String> = vec![
            cli.to_string_lossy().into_owned(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
        ];

        match &self.options.system_prompt {
            None => {
                cmd.push("--system-prompt".into());
                cmd.push(String::new());
            }
            Some(SystemPrompt::Text(s)) => {
                cmd.push("--system-prompt".into());
                cmd.push(s.clone());
            }
            Some(SystemPrompt::File(path)) => {
                cmd.push("--system-prompt-file".into());
                cmd.push(path.to_string_lossy().into_owned());
            }
            Some(SystemPrompt::Preset { append, .. }) => {
                if let Some(a) = append {
                    cmd.push("--append-system-prompt".into());
                    cmd.push(a.clone());
                }
            }
        }

        if let Some(tools) = &self.options.tools {
            match tools {
                ToolsConfig::Explicit(list) => {
                    cmd.push("--tools".into());
                    cmd.push(if list.is_empty() { String::new() } else { list.join(",") });
                }
                ToolsConfig::PresetClaudeCode => {
                    cmd.push("--tools".into());
                    cmd.push("default".into());
                }
            }
        }

        let (effective_allowed_tools, effective_setting_sources) = self.apply_skills_defaults();

        if !effective_allowed_tools.is_empty() {
            cmd.push("--allowedTools".into());
            cmd.push(effective_allowed_tools.join(","));
        }
        if let Some(n) = self.options.max_turns {
            cmd.push("--max-turns".into());
            cmd.push(n.to_string());
        }
        if let Some(b) = self.options.max_budget_usd {
            cmd.push("--max-budget-usd".into());
            cmd.push(b.to_string());
        }
        if !self.options.disallowed_tools.is_empty() {
            cmd.push("--disallowedTools".into());
            cmd.push(self.options.disallowed_tools.join(","));
        }
        if let Some(tb) = self.options.task_budget {
            cmd.push("--task-budget".into());
            cmd.push(tb.total.to_string());
        }
        if let Some(m) = &self.options.model {
            cmd.push("--model".into());
            cmd.push(m.clone());
        }
        if let Some(m) = &self.options.fallback_model {
            cmd.push("--fallback-model".into());
            cmd.push(m.clone());
        }
        if !self.options.betas.is_empty() {
            cmd.push("--betas".into());
            cmd.push(self.options.betas.join(","));
        }
        if let Some(t) = &self.options.permission_prompt_tool_name {
            cmd.push("--permission-prompt-tool".into());
            cmd.push(t.clone());
        }
        if let Some(m) = self.options.permission_mode {
            cmd.push("--permission-mode".into());
            cmd.push(m.as_str().into());
        }
        if self.options.continue_conversation {
            cmd.push("--continue".into());
        }
        if let Some(r) = &self.options.resume {
            cmd.push("--resume".into());
            cmd.push(r.clone());
        }
        if let Some(s) = &self.options.session_id {
            cmd.push("--session-id".into());
            cmd.push(s.clone());
        }
        if let Some(settings_value) = self.build_settings_value()? {
            cmd.push("--settings".into());
            cmd.push(settings_value);
        }
        for d in &self.options.add_dirs {
            cmd.push("--add-dir".into());
            cmd.push(d.to_string_lossy().into_owned());
        }
        match &self.options.mcp_servers {
            McpServers::Map(map) if !map.is_empty() => {
                let mut servers_obj = serde_json::Map::new();
                for (name, cfg) in map {
                    servers_obj.insert(name.clone(), cfg.to_cli_value());
                }
                let payload = serde_json::json!({"mcpServers": Value::Object(servers_obj)});
                cmd.push("--mcp-config".into());
                cmd.push(serde_json::to_string(&payload)?);
            }
            McpServers::Map(_) => {}
            McpServers::Inline(s) => {
                cmd.push("--mcp-config".into());
                cmd.push(s.clone());
            }
        }
        if self.options.include_partial_messages {
            cmd.push("--include-partial-messages".into());
        }
        if self.options.include_hook_events {
            cmd.push("--include-hook-events".into());
        }
        if self.options.strict_mcp_config {
            cmd.push("--strict-mcp-config".into());
        }
        if self.options.fork_session {
            cmd.push("--fork-session".into());
        }
        if self.options.session_store.is_some() {
            cmd.push("--session-mirror".into());
        }
        if let Some(ss) = &effective_setting_sources {
            let parts: Vec<&str> = ss.iter().map(|s| match s {
                crate::types::SettingSource::User => "user",
                crate::types::SettingSource::Project => "project",
                crate::types::SettingSource::Local => "local",
            }).collect();
            // Use `--setting-sources=value` so an empty list still passes
            // through (`--setting-sources=` disables filesystem settings).
            // Without `=`, an empty positional value would be dropped.
            cmd.push(format!("--setting-sources={}", parts.join(",")));
        }
        for plugin in &self.options.plugins {
            match plugin {
                crate::types::SdkPluginConfig::Local { path } => {
                    cmd.push("--plugin-dir".into());
                    cmd.push(path.clone());
                }
            }
        }
        for (flag, value) in &self.options.extra_args {
            match value {
                None => cmd.push(format!("--{flag}")),
                Some(v) => {
                    cmd.push(format!("--{flag}"));
                    cmd.push(v.clone());
                }
            }
        }
        if let Some(t) = &self.options.thinking {
            let display = match t {
                ThinkingConfig::Adaptive { display } => {
                    cmd.push("--thinking".into());
                    cmd.push("adaptive".into());
                    *display
                }
                ThinkingConfig::Enabled { budget_tokens, display } => {
                    cmd.push("--max-thinking-tokens".into());
                    cmd.push(budget_tokens.to_string());
                    *display
                }
                ThinkingConfig::Disabled => {
                    cmd.push("--thinking".into());
                    cmd.push("disabled".into());
                    None
                }
            };
            if let Some(d) = display {
                cmd.push("--thinking-display".into());
                cmd.push(d.as_str().into());
            }
        } else if let Some(n) = self.options.max_thinking_tokens {
            cmd.push("--max-thinking-tokens".into());
            cmd.push(n.to_string());
        }
        if let Some(e) = &self.options.effort {
            cmd.push("--effort".into());
            cmd.push(e.as_str().into());
        }
        if let Some(of) = &self.options.output_format {
            if of.get("type").and_then(Value::as_str) == Some("json_schema") {
                if let Some(schema) = of.get("schema") {
                    cmd.push("--json-schema".into());
                    cmd.push(serde_json::to_string(schema)?);
                }
            }
        }
        cmd.push("--input-format".into());
        cmd.push("stream-json".into());
        Ok(cmd)
    }

    async fn check_version(&self) -> Result<()> {
        let cli = self.cli_path.as_ref().ok_or_else(|| {
            ClaudeSdkError::cli_not_found("CLI path not resolved", None)
        })?;
        let fut = async {
            let out = Command::new(cli).arg("-v").output().await.ok()?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let re = regex::Regex::new(r"([0-9]+\.[0-9]+\.[0-9]+)").ok()?;
            let cap = re.captures(&stdout)?;
            let version = cap.get(1)?.as_str().to_string();
            let parts: Vec<u32> = version.split('.').filter_map(|p| p.parse().ok()).collect();
            let min: Vec<u32> = MINIMUM_CLAUDE_CODE_VERSION.split('.').filter_map(|p| p.parse().ok()).collect();
            if parts < min {
                warn!(
                    "Claude Code version {version} at {cli:?} is below minimum {MINIMUM_CLAUDE_CODE_VERSION}"
                );
            }
            Some(())
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), fut).await;
        Ok(())
    }
}

#[async_trait]
impl Transport for SubprocessTransport {
    async fn connect(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        if self.cli_path.is_none() {
            self.cli_path = Some(self.find_cli()?);
        }
        if std::env::var("CLAUDE_AGENT_SDK_SKIP_VERSION_CHECK").is_err() {
            self.check_version().await?;
        }

        let cmd_parts = self.build_command()?;
        let (program, args) = cmd_parts.split_first().ok_or_else(|| {
            ClaudeSdkError::cli_connection("Empty CLI command")
        })?;

        let mut command = Command::new(program);
        command.args(args);

        // Build env
        let inherited: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k != "CLAUDECODE")
            .collect();
        command.env_clear();
        for (k, v) in inherited {
            command.env(k, v);
        }
        command.env("CLAUDE_CODE_ENTRYPOINT", "sdk-rust");
        for (k, v) in &self.options.env {
            command.env(k, v);
        }
        command.env("CLAUDE_AGENT_SDK_VERSION", SDK_VERSION);
        if self.options.enable_file_checkpointing {
            command.env("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING", "true");
        }
        if let Some(cwd) = &self.options.cwd {
            if !cwd.exists() {
                return Err(ClaudeSdkError::cli_connection(format!(
                    "Working directory does not exist: {}",
                    cwd.display()
                )));
            }
            command.current_dir(cwd);
            command.env("PWD", cwd);
        }

        // Pipe stderr only when the caller registered a callback. The CLI's
        // `--debug-to-stderr` flag was removed upstream so we no longer
        // detect it here.
        //
        // OTEL trace context (TRACEPARENT/TRACESTATE) is propagated
        // implicitly via inherited env above — the Rust port doesn't depend
        // on opentelemetry, so we don't synthesize a fresh carrier the way
        // the Python SDK does.
        let pipe_stderr = self.options.stderr.is_some();

        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(if pipe_stderr { Stdio::piped() } else { Stdio::null() });

        let mut child = command.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ClaudeSdkError::cli_not_found(
                    format!("Claude Code not found at: {}", self.cli_path.as_ref().unwrap().display()),
                    self.cli_path.as_ref().map(|p| p.display().to_string()),
                )
            } else {
                ClaudeSdkError::cli_connection(format!("Failed to start Claude Code: {e}"))
            }
        })?;

        if let Some(pid) = child.id() {
            if let Ok(mut g) = active_children().lock() {
                g.push(pid);
            }
        }

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(err) = stderr {
            let cb = self.options.stderr.clone();
            let handle = tokio::spawn(async move {
                let mut reader = BufReader::new(err);
                let mut buf = String::new();
                loop {
                    buf.clear();
                    match reader.read_line(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let line = buf.trim_end_matches(['\r', '\n']).to_string();
                            if line.is_empty() { continue; }
                            if let Some(cb) = &cb {
                                cb(&line);
                            }
                        }
                    }
                }
            });
            self.stderr_handle = Some(handle);
        }

        *self.stdin.lock().await = stdin;
        self.stdout = stdout;
        self.child = Some(child);
        self.ready = true;
        Ok(())
    }

    async fn write(&self, data: &str) -> Result<()> {
        let mut guard = self.stdin.lock().await;
        let stdin = guard.as_mut().ok_or_else(|| {
            ClaudeSdkError::cli_connection("Transport is not ready for writing")
        })?;
        stdin.write_all(data.as_bytes()).await.map_err(|e| {
            ClaudeSdkError::cli_connection(format!("Failed to write to process stdin: {e}"))
        })?;
        stdin.flush().await.map_err(|e| {
            ClaudeSdkError::cli_connection(format!("Failed to flush stdin: {e}"))
        })?;
        Ok(())
    }

    fn read_messages(&mut self) -> BoxStream<'static, Result<Value>> {
        let stdout = match self.stdout.take() {
            Some(s) => s,
            None => {
                return stream::once(async {
                    Err(ClaudeSdkError::cli_connection("Not connected"))
                })
                .boxed();
            }
        };
        let max_buffer = self.max_buffer_size;
        // Take child to allow waiting for exit at end-of-stream.
        let child = self.child.take();

        let s = async_stream::try_stream! {
            let mut reader = BufReader::new(stdout);
            let mut json_buffer = String::new();
            let mut line = String::new();
            loop {
                line.clear();
                let n = match reader.read_line(&mut line).await {
                    Ok(n) => n,
                    Err(e) => {
                        Err(ClaudeSdkError::Io(e))?;
                        unreachable!()
                    }
                };
                if n == 0 { break; }
                let line_str = line.trim();
                if line_str.is_empty() { continue; }
                for part in line_str.split('\n') {
                    let part = part.trim();
                    if part.is_empty() { continue; }
                    if json_buffer.is_empty() && !part.starts_with('{') {
                        debug!("Skipping non-JSON line from CLI stdout: {part:.200}");
                        continue;
                    }
                    json_buffer.push_str(part);
                    if json_buffer.len() > max_buffer {
                        let len = json_buffer.len();
                        json_buffer.clear();
                        Err(ClaudeSdkError::JsonDecode {
                            snippet: format!("JSON message exceeded max buffer size of {max_buffer} bytes"),
                            source_message: format!("buffer length {len}"),
                        })?;
                        unreachable!();
                    }
                    match serde_json::from_str::<Value>(&json_buffer) {
                        Ok(v) => {
                            json_buffer.clear();
                            yield v;
                        }
                        Err(_) => continue,
                    }
                }
            }
            // Reap process
            if let Some(mut ch) = child {
                let pid = ch.id();
                let status_res = ch.wait().await;
                if let Some(pid) = pid {
                    if let Ok(mut g) = active_children().lock() {
                        g.retain(|p| *p != pid);
                    }
                }
                if let Ok(status) = status_res {
                    if !status.success() {
                        let code = status.code();
                        Err(ClaudeSdkError::process(
                            format!("Command failed with exit code {}", code.unwrap_or(-1)),
                            code,
                            Some("Check stderr output for details".into()),
                        ))?;
                    }
                }
            }
        };
        Box::pin(s)
    }

    async fn close(&mut self) -> Result<()> {
        // Close stdin
        {
            let mut guard = self.stdin.lock().await;
            if let Some(mut s) = guard.take() {
                let _ = s.shutdown().await;
            }
        }
        self.ready = false;
        if let Some(handle) = self.stderr_handle.take() {
            handle.abort();
        }
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            let timeout = std::time::Duration::from_secs(5);
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = tokio::time::timeout(timeout, child.wait()).await;
                }
            }
            if let Some(pid) = pid {
                if let Ok(mut g) = active_children().lock() {
                    g.retain(|p| *p != pid);
                }
            }
        }
        Ok(())
    }

    fn is_ready(&self) -> bool { self.ready }

    async fn end_input(&self) -> Result<()> {
        let mut guard = self.stdin.lock().await;
        if let Some(mut s) = guard.take() {
            let _ = s.shutdown().await;
        }
        Ok(())
    }
}
