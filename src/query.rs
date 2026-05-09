//! Bidirectional control-protocol orchestrator on top of [`Transport`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rand::RngCore;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, error};

use crate::errors::{ClaudeSdkError, Result};
use crate::mcp::SdkMcpServer;
use crate::transport::Transport;
use crate::types::*;

/// Internal hooks-by-event mapping passed from the client layer.
pub(crate) type InternalHooks = HashMap<HookEvent, Vec<HookMatcher>>;

/// Routes control protocol traffic and yields SDK messages.
pub struct Query {
    transport: Arc<Mutex<Box<dyn Transport>>>,
    is_streaming_mode: bool,
    can_use_tool: Option<CanUseTool>,
    hooks: InternalHooks,
    sdk_mcp_servers: HashMap<String, Arc<SdkMcpServer>>,
    initialize_timeout: Duration,
    agents: Option<HashMap<String, AgentDefinition>>,
    exclude_dynamic_sections: Option<bool>,
    /// Optional skills allowlist forwarded via `initialize`. `None` means
    /// "don't send the field"; `Some(SkillsConfig::All)` is also a no-op at
    /// the wire level (omitted) since `"all"` and absent are equivalent —
    /// only `Some(SkillsConfig::Only(_))` actually serializes.
    skills: Option<SkillsConfig>,

    pending: Arc<Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>>>,
    hook_callbacks: Arc<Mutex<HashMap<String, HookCallback>>>,
    next_callback_id: Arc<std::sync::atomic::AtomicUsize>,
    request_counter: Arc<std::sync::atomic::AtomicU64>,

    msg_tx: mpsc::UnboundedSender<Result<Value>>,
    msg_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<Result<Value>>>>>,
    first_result: Arc<Notify>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    read_handle: Mutex<Option<JoinHandle<()>>>,
    child_handles: Mutex<Vec<JoinHandle<()>>>,
    /// Inflight control request handlers, keyed by request_id for cancellation.
    inflight_requests: Arc<Mutex<HashMap<String, JoinHandle<()>>>>,
    initialization_result: Arc<Mutex<Option<Value>>>,
    /// Transcript-mirror batcher attached via [`Query::set_transcript_mirror_batcher`].
    transcript_mirror_batcher: Mutex<Option<Arc<crate::session_store_ops::TranscriptMirrorBatcher>>>,
    /// Set to the result's error text when the most recent message is a
    /// result with `is_error=true`. Used to replace the generic
    /// "exit code 1" ProcessError with the structured error the CLI
    /// already reported.
    last_error_result_text: Arc<Mutex<Option<String>>>,
}

impl Query {
    pub fn new(
        transport: Box<dyn Transport>,
        is_streaming_mode: bool,
        can_use_tool: Option<CanUseTool>,
        hooks: Option<InternalHooks>,
        sdk_mcp_servers: HashMap<String, Arc<SdkMcpServer>>,
        initialize_timeout: Duration,
        agents: Option<HashMap<String, AgentDefinition>>,
        exclude_dynamic_sections: Option<bool>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            transport: Arc::new(Mutex::new(transport)),
            is_streaming_mode,
            can_use_tool,
            hooks: hooks.unwrap_or_default(),
            sdk_mcp_servers,
            initialize_timeout,
            agents,
            exclude_dynamic_sections,
            skills: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            hook_callbacks: Arc::new(Mutex::new(HashMap::new())),
            next_callback_id: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            request_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            msg_tx: tx,
            msg_rx: Arc::new(Mutex::new(Some(rx))),
            first_result: Arc::new(Notify::new()),
            closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            read_handle: Mutex::new(None),
            child_handles: Mutex::new(Vec::new()),
            inflight_requests: Arc::new(Mutex::new(HashMap::new())),
            initialization_result: Arc::new(Mutex::new(None)),
            transcript_mirror_batcher: Mutex::new(None),
            last_error_result_text: Arc::new(Mutex::new(None)),
        }
    }

    /// Forward a `skills` allowlist via the initialize control request so the
    /// CLI filters which skills load into the system prompt. `Some(All)` is
    /// equivalent to `None` at the wire level — only `Only(_)` is forwarded.
    pub fn set_skills(&mut self, skills: Option<SkillsConfig>) {
        self.skills = skills;
    }

    /// Attach a transcript-mirror batcher. When set, the read loop peels
    /// `transcript_mirror` frames off stdout (they are not yielded to
    /// consumers), enqueues them on the batcher, and flushes before yielding
    /// each `result` message.
    pub async fn set_transcript_mirror_batcher(
        &self,
        batcher: Arc<crate::session_store_ops::TranscriptMirrorBatcher>,
    ) {
        *self.transcript_mirror_batcher.lock().await = Some(batcher);
    }

    /// Start the background reader. Must be called once.
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let stream = {
            let mut t = self.transport.lock().await;
            t.read_messages()
        };
        let me = self.clone();
        let handle = tokio::spawn(async move {
            me.run_reader(stream).await;
        });
        *self.read_handle.lock().await = Some(handle);
        Ok(())
    }

    async fn run_reader(self: Arc<Self>, mut stream: futures::stream::BoxStream<'static, Result<Value>>) {
        while let Some(item) = stream.next().await {
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            match item {
                Ok(msg) => {
                    let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                    match msg_type.as_str() {
                        "control_response" => {
                            let response = msg.get("response").cloned().unwrap_or(Value::Null);
                            let request_id = response.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
                            let mut pending = self.pending.lock().await;
                            if let Some(tx) = pending.remove(&request_id) {
                                if response.get("subtype").and_then(Value::as_str) == Some("error") {
                                    let _ = tx.send(Err(response.get("error").and_then(Value::as_str).unwrap_or("Unknown error").to_string()));
                                } else {
                                    let _ = tx.send(Ok(response));
                                }
                            }
                        }
                        "control_request" => {
                            let req_id = msg.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
                            let me = self.clone();
                            let req = msg.clone();
                            let inflight = self.inflight_requests.clone();
                            let rid = req_id.clone();
                            let h = tokio::spawn(async move {
                                me.handle_control_request(req).await;
                                inflight.lock().await.remove(&rid);
                            });
                            self.inflight_requests.lock().await.insert(req_id, h);
                        }
                        "control_cancel_request" => {
                            let cancel_id = msg.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
                            if let Some(handle) = self.inflight_requests.lock().await.remove(&cancel_id) {
                                handle.abort();
                                debug!("Cancelled inflight request: {cancel_id}");
                            }
                        }
                        "transcript_mirror" => {
                            // SessionStore write path: peel mirror frames off
                            // stdout and hand to the batcher; do NOT yield to
                            // consumers.
                            let batcher = self.transcript_mirror_batcher.lock().await.clone();
                            if let Some(b) = batcher {
                                let file_path = msg
                                    .get("filePath")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let entries = msg
                                    .get("entries")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                b.enqueue(file_path, entries).await;
                            }
                        }
                        _ => {
                            if msg_type == "result" {
                                // Flush pending transcript_mirror entries
                                // before yielding so consumers observing the
                                // result see an up-to-date SessionStore.
                                let batcher =
                                    self.transcript_mirror_batcher.lock().await.clone();
                                if let Some(b) = batcher {
                                    b.flush().await;
                                }
                                self.first_result.notify_waiters();
                                if msg.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
                                    let errors_str = msg
                                        .get("errors")
                                        .and_then(Value::as_array)
                                        .map(|a| {
                                            a.iter()
                                                .filter_map(|v| v.as_str())
                                                .collect::<Vec<_>>()
                                                .join("; ")
                                        });
                                    let text = errors_str.unwrap_or_else(|| {
                                        msg.get("subtype")
                                            .and_then(Value::as_str)
                                            .unwrap_or("unknown error")
                                            .to_string()
                                    });
                                    *self.last_error_result_text.lock().await = Some(text);
                                } else {
                                    *self.last_error_result_text.lock().await = None;
                                }
                            } else if !(msg_type == "system"
                                && msg.get("subtype").and_then(Value::as_str)
                                    == Some("session_state_changed"))
                            {
                                // Anything other than the post-turn marker
                                // means the conversation moved on; clear the
                                // suppression text so a fresh crash reports
                                // properly.
                                *self.last_error_result_text.lock().await = None;
                            }
                            let _ = self.msg_tx.send(Ok(msg));
                        }
                    }
                }
                Err(e) => {
                    // Replace ProcessError with the CLI's structured error
                    // when one was just reported via a result message.
                    let last_err = self.last_error_result_text.lock().await.clone();
                    let err_text = match (&e, &last_err) {
                        (ClaudeSdkError::Process { .. }, Some(text)) => {
                            debug!("Replacing ProcessError with result error text");
                            ClaudeSdkError::Process {
                                message: format!(
                                    "Claude Code returned an error result: {text}"
                                ),
                                exit_code: None,
                                stderr: None,
                            }
                        }
                        _ => {
                            error!("Fatal reader error: {e}");
                            e
                        }
                    };
                    let mut pending = self.pending.lock().await;
                    let err_msg = err_text.to_string();
                    for (_, tx) in pending.drain() {
                        let _ = tx.send(Err(err_msg.clone()));
                    }
                    let _ = self.msg_tx.send(Err(err_text));
                    break;
                }
            }
        }
        // Final flush + close of any attached batcher.
        let batcher = self.transcript_mirror_batcher.lock().await.clone();
        if let Some(b) = batcher {
            b.close().await;
        }
        // Always notify and signal end.
        self.first_result.notify_waiters();
        let _ = self.msg_tx.send(Ok(json!({"type": "end"})));
    }

    async fn handle_control_request(self: Arc<Self>, request: Value) {
        let request_id = request.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
        let inner = request.get("request").cloned().unwrap_or(Value::Null);
        let subtype = inner.get("subtype").and_then(Value::as_str).unwrap_or("").to_string();

        let result: std::result::Result<Value, String> = match subtype.as_str() {
            "can_use_tool" => self.handle_can_use_tool(&inner).await,
            "hook_callback" => self.handle_hook_callback(&inner).await,
            "mcp_message" => self.handle_mcp_message(&inner).await,
            other => Err(format!("Unsupported control request subtype: {other}")),
        };

        let response = match result {
            Ok(data) => json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": data,
                },
            }),
            Err(e) => json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": request_id,
                    "error": e,
                },
            }),
        };
        let _ = self.write_value(&response).await;
    }

    async fn handle_can_use_tool(&self, req: &Value) -> std::result::Result<Value, String> {
        let cb = self.can_use_tool.as_ref().ok_or_else(|| "canUseTool callback is not provided".to_string())?;
        let tool_name = req.get("tool_name").and_then(Value::as_str).unwrap_or("").to_string();
        let original_input = req.get("input").cloned().unwrap_or(Value::Null);
        let suggestions: Vec<PermissionUpdate> = req
            .get("permission_suggestions")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(PermissionUpdate::from_value).collect())
            .unwrap_or_default();
        let context = ToolPermissionContext {
            suggestions,
            tool_use_id: req.get("tool_use_id").and_then(Value::as_str).map(String::from),
            agent_id: req.get("agent_id").and_then(Value::as_str).map(String::from),
            blocked_path: req.get("blocked_path").and_then(Value::as_str).map(String::from),
            decision_reason: req.get("decision_reason").and_then(Value::as_str).map(String::from),
            title: req.get("title").and_then(Value::as_str).map(String::from),
            display_name: req.get("display_name").and_then(Value::as_str).map(String::from),
            description: req.get("description").and_then(Value::as_str).map(String::from),
        };
        let result = cb(tool_name, original_input.clone(), context).await;
        match result {
            PermissionResult::Allow { updated_input, updated_permissions } => {
                let mut obj = serde_json::Map::new();
                obj.insert("behavior".into(), Value::String("allow".into()));
                obj.insert("updatedInput".into(), updated_input.unwrap_or(original_input));
                if let Some(perms) = updated_permissions {
                    obj.insert("updatedPermissions".into(), Value::Array(perms.iter().map(|p| p.to_value()).collect()));
                }
                Ok(Value::Object(obj))
            }
            PermissionResult::Deny { message, interrupt } => {
                let mut obj = serde_json::Map::new();
                obj.insert("behavior".into(), Value::String("deny".into()));
                obj.insert("message".into(), Value::String(message));
                if interrupt {
                    obj.insert("interrupt".into(), Value::Bool(true));
                }
                Ok(Value::Object(obj))
            }
        }
    }

    async fn handle_hook_callback(&self, req: &Value) -> std::result::Result<Value, String> {
        let cb_id = req.get("callback_id").and_then(Value::as_str).unwrap_or("").to_string();
        let cb = {
            let map = self.hook_callbacks.lock().await;
            map.get(&cb_id).cloned()
        };
        let cb = cb.ok_or_else(|| format!("No hook callback found for ID: {cb_id}"))?;
        let raw = req.get("input").cloned().unwrap_or(Value::Null);
        let event_name = raw.get("hook_event_name").and_then(Value::as_str).unwrap_or("");
        let event = match event_name {
            "PreToolUse" => HookEvent::PreToolUse,
            "PostToolUse" => HookEvent::PostToolUse,
            "PostToolUseFailure" => HookEvent::PostToolUseFailure,
            "UserPromptSubmit" => HookEvent::UserPromptSubmit,
            "Stop" => HookEvent::Stop,
            "SubagentStop" => HookEvent::SubagentStop,
            "PreCompact" => HookEvent::PreCompact,
            "Notification" => HookEvent::Notification,
            "SubagentStart" => HookEvent::SubagentStart,
            "PermissionRequest" => HookEvent::PermissionRequest,
            other => return Err(format!("Unknown hook event: {other}")),
        };
        let input = HookInput::new(event, raw);
        let tool_use_id = req.get("tool_use_id").and_then(Value::as_str).map(String::from);
        let out = cb(input, tool_use_id, HookContext::default()).await;
        serde_json::to_value(&out).map_err(|e| e.to_string())
    }

    async fn handle_mcp_message(&self, req: &Value) -> std::result::Result<Value, String> {
        let server_name = req.get("server_name").and_then(Value::as_str).ok_or("Missing server_name")?;
        let message = req.get("message").ok_or("Missing message")?;
        let server = self
            .sdk_mcp_servers
            .get(server_name)
            .ok_or_else(|| format!("SDK MCP server '{server_name}' not found"))?;
        let mcp_response = server.handle_jsonrpc(message).await;
        Ok(json!({"mcp_response": mcp_response}))
    }

    async fn write_value(&self, v: &Value) -> Result<()> {
        let s = format!("{}\n", serde_json::to_string(v)?);
        let t = self.transport.lock().await;
        t.write(&s).await
    }

    /// Send the `initialize` control request to the CLI.
    pub async fn initialize(&self) -> Result<Option<Value>> {
        if !self.is_streaming_mode {
            return Ok(None);
        }
        // Build hooks config; register callbacks.
        let mut hooks_config = serde_json::Map::new();
        for (event, matchers) in &self.hooks {
            let mut event_arr = Vec::new();
            for matcher in matchers {
                let mut callback_ids = Vec::new();
                for cb in &matcher.hooks {
                    let id = self.next_callback_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let cb_id = format!("hook_{id}");
                    self.hook_callbacks.lock().await.insert(cb_id.clone(), cb.clone());
                    callback_ids.push(Value::String(cb_id));
                }
                let mut m = serde_json::Map::new();
                m.insert("matcher".into(), matcher.matcher.clone().map(Value::String).unwrap_or(Value::Null));
                m.insert("hookCallbackIds".into(), Value::Array(callback_ids));
                if let Some(t) = matcher.timeout {
                    m.insert("timeout".into(), serde_json::to_value(t).unwrap());
                }
                event_arr.push(Value::Object(m));
            }
            if !event_arr.is_empty() {
                hooks_config.insert(event.as_str().into(), Value::Array(event_arr));
            }
        }
        let mut request = serde_json::Map::new();
        request.insert("subtype".into(), Value::String("initialize".into()));
        request.insert(
            "hooks".into(),
            if hooks_config.is_empty() { Value::Null } else { Value::Object(hooks_config) },
        );
        if let Some(agents) = &self.agents {
            let agents_val: HashMap<String, Value> = agents
                .iter()
                .map(|(k, v)| (k.clone(), strip_nulls(serde_json::to_value(v).unwrap())))
                .collect();
            request.insert("agents".into(), serde_json::to_value(agents_val).unwrap());
        }
        if let Some(eds) = self.exclude_dynamic_sections {
            request.insert("excludeDynamicSections".into(), Value::Bool(eds));
        }
        // Forward `skills` only when it's an explicit list — `"all"` and
        // omitted are equivalent at the wire level (no filter).
        if let Some(SkillsConfig::Only(list)) = &self.skills {
            request.insert(
                "skills".into(),
                Value::Array(list.iter().cloned().map(Value::String).collect()),
            );
        }
        let response = self
            .send_control_request(Value::Object(request), self.initialize_timeout)
            .await?;
        *self.initialization_result.lock().await = Some(response.clone());
        Ok(Some(response))
    }

    /// Send a control request and wait for its response.
    pub async fn send_control_request(&self, request: Value, timeout: Duration) -> Result<Value> {
        if !self.is_streaming_mode {
            return Err(ClaudeSdkError::ControlRequest("Control requests require streaming mode".into()));
        }
        let counter = self.request_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut hex = [0u8; 4];
        rand::rng().fill_bytes(&mut hex);
        let request_id = format!("req_{counter}_{}", hex_encode(&hex));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let outer = json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        });
        self.write_value(&outer).await?;

        let subtype = request
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let res = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(ClaudeSdkError::ControlRequest(e)),
            Ok(Err(_)) => Err(ClaudeSdkError::ControlRequest("response sender dropped".into())),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(ClaudeSdkError::ControlTimeout(subtype))
            }
        }?;
        let response_data = res.get("response").cloned().unwrap_or(Value::Object(Default::default()));
        Ok(if response_data.is_object() { response_data } else { Value::Object(Default::default()) })
    }

    pub async fn interrupt(&self) -> Result<()> {
        self.send_control_request(json!({"subtype": "interrupt"}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<()> {
        self.send_control_request(json!({"subtype": "set_permission_mode", "mode": mode.as_str()}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn set_model(&self, model: Option<&str>) -> Result<()> {
        self.send_control_request(json!({"subtype": "set_model", "model": model}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn rewind_files(&self, user_message_id: &str) -> Result<()> {
        self.send_control_request(json!({"subtype": "rewind_files", "user_message_id": user_message_id}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn reconnect_mcp_server(&self, server_name: &str) -> Result<()> {
        self.send_control_request(json!({"subtype": "mcp_reconnect", "serverName": server_name}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn toggle_mcp_server(&self, server_name: &str, enabled: bool) -> Result<()> {
        self.send_control_request(json!({"subtype": "mcp_toggle", "serverName": server_name, "enabled": enabled}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn stop_task(&self, task_id: &str) -> Result<()> {
        self.send_control_request(json!({"subtype": "stop_task", "task_id": task_id}), Duration::from_secs(60)).await?;
        Ok(())
    }
    pub async fn get_mcp_status(&self) -> Result<Value> {
        self.send_control_request(json!({"subtype": "mcp_status"}), Duration::from_secs(60)).await
    }
    pub async fn get_context_usage(&self) -> Result<Value> {
        self.send_control_request(json!({"subtype": "get_context_usage"}), Duration::from_secs(60)).await
    }

    pub async fn initialization_result(&self) -> Option<Value> {
        self.initialization_result.lock().await.clone()
    }

    /// Wait for the first result message (if SDK MCP servers/hooks are present), then close stdin.
    pub async fn wait_for_result_and_end_input(&self) {
        if !self.sdk_mcp_servers.is_empty() || !self.hooks.is_empty() {
            self.first_result.notified().await;
        }
        let t = self.transport.lock().await;
        let _ = t.end_input().await;
    }

    /// Stream input messages (already in CLI wire format) to the transport.
    pub async fn stream_input(&self, mut stream: PromptStream) {
        while let Some(msg) = stream.next().await {
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if let Ok(s) = serde_json::to_string(&msg) {
                let _ = self.write_value_str(&s).await;
            }
        }
        self.wait_for_result_and_end_input().await;
    }

    async fn write_value_str(&self, s: &str) -> Result<()> {
        let t = self.transport.lock().await;
        t.write(&format!("{s}\n")).await
    }

    /// Write a pre-serialized line (must already include trailing newline).
    pub async fn send_raw(&self, line: &str) -> Result<()> {
        let t = self.transport.lock().await;
        t.write(line).await
    }

    /// Take the SDK message receiver. Can only be called once.
    pub async fn take_receiver(&self) -> Option<mpsc::UnboundedReceiver<Result<Value>>> {
        self.msg_rx.lock().await.take()
    }

    /// Inject a synthetic message into the SDK message stream. Used by the
    /// transcript-mirror batcher to surface `mirror_error` events as system
    /// messages without going through the CLI subprocess.
    pub async fn inject_message(&self, msg: Value) {
        let _ = self.msg_tx.send(Ok(msg));
    }

    /// Spawn a tracked child task.
    pub async fn spawn_task<F>(&self, fut: F)
    where F: std::future::Future<Output = ()> + Send + 'static,
    {
        let h = tokio::spawn(fut);
        self.child_handles.lock().await.push(h);
    }

    /// Close everything.
    pub async fn close(&self) -> Result<()> {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.read_handle.lock().await.take() {
            handle.abort();
        }
        for h in self.child_handles.lock().await.drain(..) {
            h.abort();
        }
        for (_, h) in self.inflight_requests.lock().await.drain() {
            h.abort();
        }
        let mut t = self.transport.lock().await;
        t.close().await
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Recursively strip `null` values from a JSON value (matches Python's
/// `{k: v for k, v in asdict(d).items() if v is not None}`).
fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, val)| !val.is_null())
                .map(|(k, val)| (k, strip_nulls(val)))
                .collect(),
        ),
        other => other,
    }
}
