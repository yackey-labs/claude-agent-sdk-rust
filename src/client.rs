//! High-level client for bidirectional, interactive Claude Code sessions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::BoxStream;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::errors::{ClaudeSdkError, Result};
use crate::message_parser::parse_message;
use crate::query::Query;
use crate::transport::{subprocess::SubprocessTransport, Transport};
use crate::types::*;

/// Client for bidirectional, interactive conversations with Claude Code.
///
/// For one-shot queries, use [`crate::query()`] instead.
pub struct ClaudeSdkClient {
    options: Arc<Mutex<Option<ClaudeAgentOptions>>>,
    custom_transport: Option<Box<dyn Transport>>,
    query: Mutex<Option<Arc<Query>>>,
}

impl ClaudeSdkClient {
    pub fn new(options: ClaudeAgentOptions) -> Self {
        Self {
            options: Arc::new(Mutex::new(Some(options))),
            custom_transport: None,
            query: Mutex::new(None),
        }
    }

    pub fn with_transport(options: ClaudeAgentOptions, transport: Box<dyn Transport>) -> Self {
        Self {
            options: Arc::new(Mutex::new(Some(options))),
            custom_transport: Some(transport),
            query: Mutex::new(None),
        }
    }

    /// Connect, optionally seeding the conversation with an initial prompt.
    pub async fn connect(&mut self, prompt: Option<Prompt>) -> Result<()> {
        let mut options = self.options.lock().await.take().ok_or_else(|| {
            ClaudeSdkError::cli_connection("Client already connected (options taken)")
        })?;

        if options.can_use_tool.is_some() {
            if matches!(prompt, Some(Prompt::Text(_))) {
                return Err(ClaudeSdkError::InvalidArgument(
                    "can_use_tool callback requires streaming prompt".into(),
                ));
            }
            if options.permission_prompt_tool_name.is_some() {
                return Err(ClaudeSdkError::InvalidArgument(
                    "can_use_tool and permission_prompt_tool_name are mutually exclusive".into(),
                ));
            }
            options.permission_prompt_tool_name = Some("stdio".into());
        }

        // Clone the bits Query needs before moving options into transport.
        let can_use_tool = options.can_use_tool.clone();
        let hooks = options.hooks.take();
        let agents = options.agents.clone();
        let sdk_mcp_servers = extract_sdk_mcp_servers(&options.mcp_servers);
        let exclude_dynamic_sections = preset_exclude_dynamic(&options.system_prompt);

        let transport: Box<dyn Transport> = match self.custom_transport.take() {
            Some(t) => t,
            None => Box::new(SubprocessTransport::new(options)),
        };

        let initialize_timeout = compute_initialize_timeout();
        let mut transport = transport;
        transport.connect().await?;

        let q = Arc::new(Query::new(
            transport,
            true,
            can_use_tool,
            hooks,
            sdk_mcp_servers,
            initialize_timeout,
            agents,
            exclude_dynamic_sections,
        ));
        q.start().await?;
        q.initialize().await?;

        // Send the initial prompt, if any.
        match prompt {
            Some(Prompt::Text(s)) => {
                let msg = json!({
                    "type": "user",
                    "message": {"role": "user", "content": s},
                    "parent_tool_use_id": Value::Null,
                    "session_id": "default",
                });
                let line = format!("{}\n", serde_json::to_string(&msg)?);
                q.send_raw(&line).await?;
            }
            Some(Prompt::Stream(stream)) => {
                let qc = q.clone();
                q.spawn_task(async move { qc.stream_input(stream).await }).await;
            }
            None => {}
        }

        *self.query.lock().await = Some(q);
        Ok(())
    }

    /// Receive all messages.
    pub async fn receive_messages(&self) -> Result<BoxStream<'static, Result<Message>>> {
        let q = self.require_query().await?;
        let rx = q
            .take_receiver()
            .await
            .ok_or_else(|| ClaudeSdkError::cli_connection("Receiver already taken"))?;
        let stream = async_stream::try_stream! {
            let mut rx = rx;
            while let Some(item) = rx.recv().await {
                let v = item?;
                if v.get("type").and_then(Value::as_str) == Some("end") { break; }
                if v.get("type").and_then(Value::as_str) == Some("error") {
                    let msg = v.get("error").and_then(Value::as_str).unwrap_or("Unknown error").to_string();
                    Err(ClaudeSdkError::cli_connection(msg))?;
                    unreachable!();
                }
                if let Some(msg) = parse_message(&v)? {
                    yield msg;
                }
            }
        };
        Ok(Box::pin(stream))
    }

    /// Receive messages until and including the first [`Message::Result`].
    pub async fn receive_response(&self) -> Result<BoxStream<'static, Result<Message>>> {
        let inner = self.receive_messages().await?;
        let stream = async_stream::try_stream! {
            futures::pin_mut!(inner);
            while let Some(item) = inner.next().await {
                let m = item?;
                let is_result = matches!(m, Message::Result(_));
                yield m;
                if is_result { break; }
            }
        };
        Ok(Box::pin(stream))
    }

    /// Send another prompt on an open connection.
    pub async fn query(&self, prompt: Prompt, session_id: &str) -> Result<()> {
        let q = self.require_query().await?;
        match prompt {
            Prompt::Text(s) => {
                let msg = json!({
                    "type": "user",
                    "message": {"role": "user", "content": s},
                    "parent_tool_use_id": Value::Null,
                    "session_id": session_id,
                });
                let line = format!("{}\n", serde_json::to_string(&msg)?);
                q.send_raw(&line).await?;
            }
            Prompt::Stream(mut stream) => {
                while let Some(mut msg) = stream.next().await {
                    if msg.get("session_id").is_none() {
                        if let Some(obj) = msg.as_object_mut() {
                            obj.insert("session_id".into(), Value::String(session_id.into()));
                        }
                    }
                    let line = format!("{}\n", serde_json::to_string(&msg)?);
                    q.send_raw(&line).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn interrupt(&self) -> Result<()> { self.require_query().await?.interrupt().await }
    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<()> {
        self.require_query().await?.set_permission_mode(mode).await
    }
    pub async fn set_model(&self, model: Option<&str>) -> Result<()> {
        self.require_query().await?.set_model(model).await
    }
    pub async fn rewind_files(&self, user_message_id: &str) -> Result<()> {
        self.require_query().await?.rewind_files(user_message_id).await
    }
    pub async fn reconnect_mcp_server(&self, name: &str) -> Result<()> {
        self.require_query().await?.reconnect_mcp_server(name).await
    }
    pub async fn toggle_mcp_server(&self, name: &str, enabled: bool) -> Result<()> {
        self.require_query().await?.toggle_mcp_server(name, enabled).await
    }
    pub async fn stop_task(&self, task_id: &str) -> Result<()> {
        self.require_query().await?.stop_task(task_id).await
    }
    pub async fn get_mcp_status(&self) -> Result<McpStatusResponse> {
        self.require_query().await?.get_mcp_status().await
    }
    pub async fn get_context_usage(&self) -> Result<ContextUsageResponse> {
        self.require_query().await?.get_context_usage().await
    }
    pub async fn get_server_info(&self) -> Result<Option<Value>> {
        Ok(self.require_query().await?.initialization_result().await)
    }

    /// Disconnect cleanly.
    pub async fn disconnect(&self) -> Result<()> {
        let mut guard = self.query.lock().await;
        if let Some(q) = guard.take() {
            q.close().await?;
        }
        Ok(())
    }

    async fn require_query(&self) -> Result<Arc<Query>> {
        self.query
            .lock()
            .await
            .clone()
            .ok_or_else(|| ClaudeSdkError::cli_connection("Not connected. Call connect() first."))
    }
}

// Internal helpers --------------------------------------------------------

pub(crate) fn compute_initialize_timeout() -> Duration {
    let ms: u64 = std::env::var("CLAUDE_CODE_STREAM_CLOSE_TIMEOUT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60_000);
    Duration::from_millis(ms.max(60_000))
}

pub(crate) fn extract_sdk_mcp_servers(
    servers: &McpServers,
) -> HashMap<String, Arc<crate::mcp::SdkMcpServer>> {
    let mut out = HashMap::new();
    if let McpServers::Map(map) = servers {
        for (name, cfg) in map {
            if let McpServerConfig::Sdk { server, .. } = cfg {
                out.insert(name.clone(), server.clone());
            }
        }
    }
    out
}

pub(crate) fn preset_exclude_dynamic(sp: &Option<SystemPrompt>) -> Option<bool> {
    if let Some(SystemPrompt::Preset { exclude_dynamic_sections, .. }) = sp {
        *exclude_dynamic_sections
    } else {
        None
    }
}

