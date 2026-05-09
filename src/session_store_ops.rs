//! [`SessionStore`] adapter integration: in-memory reference impl, file-path
//! → key derivation, options validation, the transcript-mirror batcher, and
//! the incremental summary fold.
//!
//! Mirrors the upstream Python modules:
//!   - `_internal/session_store.py` (InMemorySessionStore + file_path_to_session_key)
//!   - `_internal/session_store_validation.py`
//!   - `_internal/transcript_mirror_batcher.py`
//!   - `_internal/session_summary.py`
//!   - `_internal/session_import.py`
//!   - `_internal/session_resume.py` (helper subset)

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

use crate::errors::{ClaudeSdkError, Result};
use crate::types::{
    ClaudeAgentOptions, SdkSessionInfo, SessionKey, SessionListSubkeysKey, SessionStore,
    SessionStoreEntry, SessionStoreFlushMode, SessionStoreListEntry, SessionSummaryEntry,
};

// ---------------------------------------------------------------------------
// file_path_to_session_key
// ---------------------------------------------------------------------------

/// Derive a [`SessionKey`] from an absolute transcript file path.
///
/// Main transcripts: `<projects_dir>/<project_key>/<session_id>.jsonl`
/// Subagent transcripts: `<projects_dir>/<project_key>/<session_id>/subagents/agent-<id>.jsonl`
///
/// Returns `None` if `file_path` is not under `projects_dir` or has an
/// unrecognized shape.
pub fn file_path_to_session_key(file_path: &str, projects_dir: &str) -> Option<SessionKey> {
    let abs = Path::new(file_path);
    let base = Path::new(projects_dir);
    let rel = abs.strip_prefix(base).ok()?;
    let parts: Vec<&str> = rel
        .components()
        .map(|c| match c {
            Component::Normal(s) => s.to_str().unwrap_or(""),
            Component::ParentDir => "..",
            _ => "",
        })
        .collect();
    if parts.is_empty() || parts[0] == ".." {
        return None;
    }
    if parts.len() < 2 {
        return None;
    }
    let project_key = parts[0].to_string();
    let second = parts[1];

    // Main transcript: <project_key>/<session_id>.jsonl
    if parts.len() == 2 && second.ends_with(".jsonl") {
        return Some(SessionKey {
            project_key,
            session_id: second[..second.len() - ".jsonl".len()].to_string(),
            subpath: String::new(),
        });
    }

    // Subagent transcript: <project_key>/<session_id>/subagents/.../agent-<id>.jsonl
    if parts.len() >= 4 {
        let mut subpath_parts: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
        if let Some(last) = subpath_parts.last_mut() {
            if let Some(stripped) = last.strip_suffix(".jsonl") {
                *last = stripped.to_string();
            }
        }
        return Some(SessionKey {
            project_key,
            session_id: second.to_string(),
            // Subpaths are always /-joined so keys are portable across platforms.
            subpath: subpath_parts.join("/"),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Options validation
// ---------------------------------------------------------------------------

/// Validate `session_store`-related option combinations. Called before
/// subprocess spawn so misconfiguration fails fast.
pub fn validate_session_store_options(options: &ClaudeAgentOptions) -> Result<()> {
    if options.session_store.is_none() {
        return Ok(());
    }
    if options.enable_file_checkpointing {
        return Err(ClaudeSdkError::InvalidArgument(
            "session_store cannot be combined with enable_file_checkpointing \
             (checkpoints are local-disk only and would diverge from the \
             mirrored transcript)"
                .into(),
        ));
    }
    // We can't probe whether `list_sessions` is overridden vs. the trait
    // default at runtime in Rust without runtime reflection. The upstream
    // check (continue_conversation + no resume → must implement
    // list_sessions) is enforced lazily — if list_sessions returns
    // NotImplemented during continue_conversation resolution, the caller
    // surfaces the error there.
    Ok(())
}

// ---------------------------------------------------------------------------
// Incremental summary fold (mirrors session_summary.py)
// ---------------------------------------------------------------------------

const LAST_WINS_FIELDS: &[(&str, &str)] = &[
    ("customTitle", "custom_title"),
    ("aiTitle", "ai_title"),
    ("lastPrompt", "last_prompt"),
    ("summary", "summary_hint"),
    ("gitBranch", "git_branch"),
];

fn iso_to_epoch_ms(ts: &str) -> Option<i64> {
    let normalized = match ts.strip_suffix('Z') {
        Some(stripped) => format!("{stripped}+00:00"),
        None => ts.to_string(),
    };
    DateTime::parse_from_rfc3339(&normalized)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn skip_first_prompt_re() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"(?s)^(?:<local-command-stdout>|<session-start-hook>|<tick>|<goal>|\[Request interrupted by user[^\]]*\]|\s*<ide_opened_file>.*</ide_opened_file>\s*$|\s*<ide_selection>.*</ide_selection>\s*$)",
        )
        .unwrap()
    })
}

fn command_name_re() -> &'static regex::Regex {
    use std::sync::OnceLock;
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"<command-name>(.*?)</command-name>").unwrap())
}

fn entry_text_blocks(entry: &Value) -> Vec<String> {
    let message = match entry.get("message") {
        Some(Value::Object(_)) => entry.get("message").unwrap(),
        _ => return Vec::new(),
    };
    let content = message.get("content");
    let mut texts = Vec::new();
    match content {
        Some(Value::String(s)) => texts.push(s.clone()),
        Some(Value::Array(arr)) => {
            for b in arr {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        texts.push(t.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    texts
}

fn fold_first_prompt(data: &mut serde_json::Map<String, Value>, entry: &Value) {
    if data.get("first_prompt_locked").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }
    if entry.get("type").and_then(Value::as_str) != Some("user") {
        return;
    }
    if entry.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
        || entry.get("isCompactSummary").and_then(Value::as_bool).unwrap_or(false)
    {
        return;
    }
    // Skip tool_result-carrying user messages.
    if let Some(Value::Object(msg)) = entry.get("message") {
        if let Some(Value::Array(arr)) = msg.get("content") {
            if arr.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
                return;
            }
        }
    }
    for raw in entry_text_blocks(entry) {
        let result = raw.replace('\n', " ").trim().to_string();
        if result.is_empty() {
            continue;
        }
        if let Some(cap) = command_name_re().captures(&result) {
            if !data.contains_key("command_fallback") {
                data.insert(
                    "command_fallback".into(),
                    Value::String(cap.get(1).unwrap().as_str().to_string()),
                );
            }
            continue;
        }
        if skip_first_prompt_re().is_match(&result) {
            continue;
        }
        let final_str = if result.chars().count() > 200 {
            let mut s: String = result.chars().take(200).collect();
            s = s.trim_end().to_string();
            s.push('\u{2026}');
            s
        } else {
            result
        };
        data.insert("first_prompt".into(), Value::String(final_str));
        data.insert("first_prompt_locked".into(), Value::Bool(true));
        return;
    }
}

/// Fold a batch of appended entries into the running summary for `key`.
///
/// Stores call this from inside `append()` to keep a
/// [`SessionSummaryEntry`] sidecar up to date without re-reading the
/// transcript. `prev` is the previous summary for the same key (or `None`
/// for the first append).
///
/// Do not call this for keys with a non-empty `subpath` — subagent
/// transcripts must not contribute to the main session's summary.
pub fn fold_session_summary(
    prev: Option<&SessionSummaryEntry>,
    key: &SessionKey,
    entries: &[SessionStoreEntry],
) -> SessionSummaryEntry {
    let mut summary = match prev {
        Some(p) => SessionSummaryEntry {
            session_id: p.session_id.clone(),
            mtime: p.mtime,
            data: p.data.clone(),
        },
        None => SessionSummaryEntry {
            session_id: key.session_id.clone(),
            mtime: 0,
            data: Value::Object(serde_json::Map::new()),
        },
    };
    let data_obj = match summary.data.as_object_mut() {
        Some(o) => o,
        None => {
            summary.data = Value::Object(serde_json::Map::new());
            summary.data.as_object_mut().unwrap()
        }
    };

    for entry in entries {
        let ms = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(iso_to_epoch_ms);

        if !data_obj.contains_key("is_sidechain") {
            let s = entry.get("isSidechain").and_then(Value::as_bool).unwrap_or(false);
            data_obj.insert("is_sidechain".into(), Value::Bool(s));
        }
        if !data_obj.contains_key("created_at") {
            if let Some(ms) = ms {
                data_obj.insert("created_at".into(), Value::from(ms));
            }
        }
        if !data_obj.contains_key("cwd") {
            if let Some(cwd) = entry.get("cwd").and_then(Value::as_str) {
                if !cwd.is_empty() {
                    data_obj.insert("cwd".into(), Value::String(cwd.into()));
                }
            }
        }
        fold_first_prompt(data_obj, entry);

        for (src, dst) in LAST_WINS_FIELDS {
            if let Some(v) = entry.get(*src).and_then(Value::as_str) {
                data_obj.insert((*dst).into(), Value::String(v.into()));
            }
        }
        if entry.get("type").and_then(Value::as_str) == Some("tag") {
            match entry.get("tag").and_then(Value::as_str) {
                Some(s) if !s.is_empty() => {
                    data_obj.insert("tag".into(), Value::String(s.into()));
                }
                _ => {
                    data_obj.remove("tag");
                }
            }
        }
    }

    summary
}

/// Convert a [`SessionSummaryEntry`] to [`SdkSessionInfo`]. Returns `None`
/// for sidechain sessions or sessions with no extractable summary.
pub fn summary_entry_to_sdk_info(
    entry: &SessionSummaryEntry,
    project_path: Option<&str>,
) -> Option<SdkSessionInfo> {
    let data = entry.data.as_object()?;
    if data.get("is_sidechain").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let locked = data.get("first_prompt_locked").and_then(Value::as_bool).unwrap_or(false);
    let first_prompt: Option<String> = if locked {
        data.get("first_prompt").and_then(Value::as_str).map(String::from)
    } else {
        data.get("command_fallback").and_then(Value::as_str).map(String::from)
    };
    let custom_title = data
        .get("custom_title")
        .and_then(Value::as_str)
        .map(String::from)
        .or_else(|| data.get("ai_title").and_then(Value::as_str).map(String::from));
    let summary = custom_title
        .clone()
        .or_else(|| data.get("last_prompt").and_then(Value::as_str).map(String::from))
        .or_else(|| data.get("summary_hint").and_then(Value::as_str).map(String::from))
        .or_else(|| first_prompt.clone());
    let summary = summary?;
    Some(SdkSessionInfo {
        session_id: entry.session_id.clone(),
        summary,
        last_modified: entry.mtime,
        file_size: None,
        custom_title,
        first_prompt,
        git_branch: data.get("git_branch").and_then(Value::as_str).map(String::from),
        cwd: data
            .get("cwd")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| project_path.map(String::from)),
        tag: data.get("tag").and_then(Value::as_str).map(String::from),
        created_at: data.get("created_at").and_then(Value::as_i64),
    })
}

// ---------------------------------------------------------------------------
// Transcript mirror batcher
// ---------------------------------------------------------------------------

/// Eager-flush threshold (entries).
pub const MAX_PENDING_ENTRIES: usize = 500;
/// Eager-flush threshold (bytes, 1 MiB).
pub const MAX_PENDING_BYTES: usize = 1 << 20;
/// Max time to wait on a single `store.append()` call.
pub const SEND_TIMEOUT_SECONDS: u64 = 60;
/// Bounded retry for transient adapter failures.
pub const MIRROR_APPEND_MAX_ATTEMPTS: usize = 3;
/// Backoff between retry attempts (seconds, length must be MAX_ATTEMPTS-1).
pub const MIRROR_APPEND_BACKOFF_S: &[f64] = &[0.2, 0.8];

struct MirrorEntry {
    file_path: String,
    entries: Vec<SessionStoreEntry>,
    bytes: usize,
}

/// Callback invoked when a flush fails after all retry attempts.
pub type OnMirrorError = Arc<
    dyn Fn(Option<SessionKey>, String) -> futures::future::BoxFuture<'static, ()>
        + Send
        + Sync,
>;

struct BatcherState {
    pending: Vec<MirrorEntry>,
    pending_entries: usize,
    pending_bytes: usize,
}

/// Accumulates `transcript_mirror` frames and flushes them to a
/// [`SessionStore`].
///
/// `enqueue` is fire-and-forget; `flush` is async. In `Batched` flush mode
/// the pending queue is bounded — when it exceeds [`MAX_PENDING_ENTRIES`] or
/// [`MAX_PENDING_BYTES`] an eager flush fires in the background so memory
/// stays flat during long turns where no `result` (and thus no explicit
/// `flush()`) arrives. In `Eager` flush mode every `enqueue` triggers a
/// background drain immediately so adapters see entries in near real time;
/// drains are still serialized via the flush lock so append ordering holds.
pub struct TranscriptMirrorBatcher {
    store: Arc<dyn SessionStore>,
    projects_dir: String,
    on_error: OnMirrorError,
    send_timeout: Duration,
    max_pending_entries: usize,
    max_pending_bytes: usize,
    flush_mode: SessionStoreFlushMode,
    state: Arc<Mutex<BatcherState>>,
    flush_lock: Arc<Mutex<()>>,
}

impl TranscriptMirrorBatcher {
    pub fn new(
        store: Arc<dyn SessionStore>,
        projects_dir: String,
        on_error: OnMirrorError,
    ) -> Self {
        Self {
            store,
            projects_dir,
            on_error,
            send_timeout: Duration::from_secs(SEND_TIMEOUT_SECONDS),
            max_pending_entries: MAX_PENDING_ENTRIES,
            max_pending_bytes: MAX_PENDING_BYTES,
            flush_mode: SessionStoreFlushMode::Batched,
            state: Arc::new(Mutex::new(BatcherState {
                pending: Vec::new(),
                pending_entries: 0,
                pending_bytes: 0,
            })),
            flush_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn with_send_timeout(mut self, t: Duration) -> Self {
        self.send_timeout = t;
        self
    }

    /// Configure the flush mode. [`SessionStoreFlushMode::Eager`] triggers a
    /// background drain after every `enqueue`; [`SessionStoreFlushMode::Batched`]
    /// (default) only drains on threshold or explicit `flush`.
    pub fn with_flush_mode(mut self, mode: SessionStoreFlushMode) -> Self {
        self.flush_mode = mode;
        self
    }

    /// Buffer a frame; in `Eager` mode trigger a background flush
    /// immediately, otherwise only when thresholds are exceeded.
    pub async fn enqueue(self: &Arc<Self>, file_path: String, entries: Vec<SessionStoreEntry>) {
        // Approximate wire size — one stringify per frame keeps it cheap.
        let bytes = serde_json::to_string(&entries).map(|s| s.len()).unwrap_or(0);
        let mut s = self.state.lock().await;
        s.pending_entries += entries.len();
        s.pending_bytes += bytes;
        s.pending.push(MirrorEntry { file_path, entries, bytes });
        let over_threshold = s.pending_entries > self.max_pending_entries
            || s.pending_bytes > self.max_pending_bytes;
        drop(s);
        let trigger = match self.flush_mode {
            SessionStoreFlushMode::Eager => true,
            SessionStoreFlushMode::Batched => over_threshold,
        };
        if trigger {
            let me = self.clone();
            tokio::spawn(async move {
                me.drain().await;
            });
        }
    }

    /// Flush all pending entries.
    pub async fn flush(self: &Arc<Self>) {
        self.drain().await;
    }

    /// Final flush before teardown.
    pub async fn close(self: &Arc<Self>) {
        self.flush().await;
    }

    async fn drain(self: &Arc<Self>) {
        // Detach pending buffer so enqueue() can keep accumulating.
        let items: Vec<MirrorEntry> = {
            let mut s = self.state.lock().await;
            let items = std::mem::take(&mut s.pending);
            s.pending_entries = 0;
            s.pending_bytes = 0;
            items
        };
        if items.is_empty() {
            return;
        }
        let _flush = self.flush_lock.lock().await;
        let mut errors: Vec<(SessionKey, String)> = Vec::new();
        self.do_flush(items, &mut errors).await;
        drop(_flush);
        // Report errors after releasing the lock.
        for (key, msg) in errors {
            (self.on_error)(Some(key), msg).await;
        }
    }

    async fn do_flush(
        &self,
        items: Vec<MirrorEntry>,
        errors: &mut Vec<(SessionKey, String)>,
    ) {
        // Coalesce by file_path, preserving first-seen order.
        let mut order: Vec<String> = Vec::new();
        let mut by_path: HashMap<String, Vec<SessionStoreEntry>> = HashMap::new();
        for item in items {
            if !by_path.contains_key(&item.file_path) {
                order.push(item.file_path.clone());
                by_path.insert(item.file_path.clone(), item.entries);
            } else {
                by_path.get_mut(&item.file_path).unwrap().extend(item.entries);
            }
            let _ = item.bytes; // currently unused after coalescing
        }
        for file_path in order {
            let entries = by_path.remove(&file_path).unwrap_or_default();
            if entries.is_empty() {
                continue;
            }
            let key = match file_path_to_session_key(&file_path, &self.projects_dir) {
                Some(k) => k,
                None => {
                    warn!(
                        "[SessionStore] dropping mirror frame: filePath {file_path} is not under {} \
                         -- subprocess CLAUDE_CONFIG_DIR likely differs from parent",
                        self.projects_dir
                    );
                    continue;
                }
            };
            let mut last_err: Option<String> = None;
            let mut succeeded = false;
            for attempt in 0..MIRROR_APPEND_MAX_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_secs_f64(
                        MIRROR_APPEND_BACKOFF_S[attempt - 1],
                    ))
                    .await;
                }
                let send = self.store.append(&key, &entries);
                match tokio::time::timeout(self.send_timeout, send).await {
                    Ok(Ok(())) => {
                        succeeded = true;
                        break;
                    }
                    Ok(Err(e)) => {
                        last_err = Some(e.to_string());
                        debug!(
                            "[TranscriptMirrorBatcher] append attempt {}/{} failed for {}: {}",
                            attempt + 1,
                            MIRROR_APPEND_MAX_ATTEMPTS,
                            file_path,
                            e
                        );
                    }
                    Err(_) => {
                        // Don't retry on timeout.
                        last_err = Some(format!(
                            "append timed out after {:.1}s",
                            self.send_timeout.as_secs_f64()
                        ));
                        debug!(
                            "[TranscriptMirrorBatcher] append timed out after {:.1}s for {} — not retrying",
                            self.send_timeout.as_secs_f64(),
                            file_path
                        );
                        break;
                    }
                }
            }
            if !succeeded {
                let msg = last_err.unwrap_or_else(|| "unknown error".into());
                error!("[TranscriptMirrorBatcher] flush failed for {file_path}: {msg}");
                errors.push((key, msg));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory reference store
// ---------------------------------------------------------------------------

/// In-memory [`SessionStore`] for testing and development. Not suitable for
/// production — data is lost when the process exits.
pub struct InMemorySessionStore {
    state: Mutex<InMemoryState>,
}

struct InMemoryState {
    store: HashMap<String, Vec<SessionStoreEntry>>,
    mtimes: HashMap<String, i64>,
    summaries: HashMap<(String, String), SessionSummaryEntry>,
    last_mtime: i64,
}

fn key_to_string(key: &SessionKey) -> String {
    if key.subpath.is_empty() {
        format!("{}/{}", key.project_key, key.session_id)
    } else {
        format!("{}/{}/{}", key.project_key, key.session_id, key.subpath)
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self { Self::new() }
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(InMemoryState {
                store: HashMap::new(),
                mtimes: HashMap::new(),
                summaries: HashMap::new(),
                last_mtime: 0,
            }),
        }
    }

    /// Test helper — get all entries for a key (empty if absent).
    pub async fn get_entries(&self, key: &SessionKey) -> Vec<SessionStoreEntry> {
        let s = self.state.lock().await;
        s.store.get(&key_to_string(key)).cloned().unwrap_or_default()
    }

    /// Test helper — number of stored sessions (main transcripts only).
    pub async fn size(&self) -> usize {
        let s = self.state.lock().await;
        s.store
            .keys()
            .filter(|k| {
                let mut iter = k.split('/');
                iter.next();
                iter.next();
                iter.next().is_none()
            })
            .count()
    }

    /// Test helper — clear all stored data.
    pub async fn clear(&self) {
        let mut s = self.state.lock().await;
        s.store.clear();
        s.mtimes.clear();
        s.summaries.clear();
        s.last_mtime = 0;
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn append(
        &self,
        key: &SessionKey,
        entries: &[SessionStoreEntry],
    ) -> Result<()> {
        let mut s = self.state.lock().await;
        let k = key_to_string(key);
        s.store.entry(k.clone()).or_default().extend(entries.iter().cloned());
        // Strictly monotonic mtime.
        let mut now_ms = chrono::Utc::now().timestamp_millis();
        if now_ms <= s.last_mtime {
            now_ms = s.last_mtime + 1;
        }
        s.last_mtime = now_ms;
        if key.subpath.is_empty() {
            let sk = (key.project_key.clone(), key.session_id.clone());
            let prev = s.summaries.get(&sk).cloned();
            let mut folded = fold_session_summary(prev.as_ref(), key, entries);
            folded.mtime = now_ms;
            s.summaries.insert(sk, folded);
        }
        s.mtimes.insert(k, now_ms);
        Ok(())
    }

    async fn load(
        &self,
        key: &SessionKey,
    ) -> Result<Option<Vec<SessionStoreEntry>>> {
        let s = self.state.lock().await;
        Ok(s.store.get(&key_to_string(key)).cloned())
    }

    async fn list_sessions(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionStoreListEntry>> {
        let s = self.state.lock().await;
        let prefix = format!("{project_key}/");
        let mut out = Vec::new();
        for k in s.store.keys() {
            if let Some(rest) = k.strip_prefix(&prefix) {
                if !rest.contains('/') {
                    out.push(SessionStoreListEntry {
                        session_id: rest.to_string(),
                        mtime: s.mtimes.get(k).copied().unwrap_or(0),
                    });
                }
            }
        }
        Ok(out)
    }

    async fn list_session_summaries(
        &self,
        project_key: &str,
    ) -> Result<Vec<SessionSummaryEntry>> {
        let s = self.state.lock().await;
        Ok(s
            .summaries
            .iter()
            .filter(|((pk, _), _)| pk == project_key)
            .map(|(_, v)| v.clone())
            .collect())
    }

    async fn delete(&self, key: &SessionKey) -> Result<()> {
        let mut s = self.state.lock().await;
        let k = key_to_string(key);
        s.store.remove(&k);
        s.mtimes.remove(&k);
        if key.subpath.is_empty() {
            s.summaries.remove(&(key.project_key.clone(), key.session_id.clone()));
            let prefix = format!("{}/{}/", key.project_key, key.session_id);
            let to_remove: Vec<String> =
                s.store.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            for k in to_remove {
                s.store.remove(&k);
                s.mtimes.remove(&k);
            }
        }
        Ok(())
    }

    async fn list_subkeys(
        &self,
        key: &SessionListSubkeysKey,
    ) -> Result<Vec<String>> {
        let s = self.state.lock().await;
        let prefix = format!("{}/{}/", key.project_key, key.session_id);
        Ok(s.store
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix).map(String::from))
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Local-disk → SessionStore import
// ---------------------------------------------------------------------------

/// Replay a local on-disk session transcript into a [`SessionStore`].
///
/// Reads `<projects_dir>/<project_key>/<session_id>.jsonl` and any subagent
/// transcripts under `<projects_dir>/<project_key>/<session_id>/subagents/`,
/// then calls `store.append()` once per file. Returns the number of files
/// successfully imported.
pub async fn import_session_to_store(
    store: &dyn SessionStore,
    projects_dir: &str,
    project_key: &str,
    session_id: &str,
) -> Result<usize> {
    let pdir = PathBuf::from(projects_dir);
    let main = pdir.join(project_key).join(format!("{session_id}.jsonl"));
    let mut imported = 0usize;
    if let Ok(content) = std::fs::read_to_string(&main) {
        let entries = parse_jsonl_entries(&content);
        if !entries.is_empty() {
            let key = SessionKey {
                project_key: project_key.into(),
                session_id: session_id.into(),
                subpath: String::new(),
            };
            store.append(&key, &entries).await?;
            imported += 1;
        }
    }
    let subagents_root = pdir.join(project_key).join(session_id).join("subagents");
    if subagents_root.is_dir() {
        for (sub, content) in walk_jsonl(&subagents_root) {
            let entries = parse_jsonl_entries(&content);
            if entries.is_empty() {
                continue;
            }
            let rel = sub
                .strip_prefix(&subagents_root)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            let stripped = rel.strip_suffix(".jsonl").unwrap_or(rel);
            let key = SessionKey {
                project_key: project_key.into(),
                session_id: session_id.into(),
                subpath: format!("subagents/{stripped}"),
            };
            store.append(&key, &entries).await?;
            imported += 1;
        }
    }
    Ok(imported)
}

fn parse_jsonl_entries(content: &str) -> Vec<Value> {
    content
        .lines()
        .filter_map(|l| {
            let s = l.trim();
            if s.is_empty() {
                None
            } else {
                serde_json::from_str(s).ok()
            }
        })
        .collect()
}

fn walk_jsonl(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let read = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return,
        };
        for entry in read.flatten() {
            let p = entry.path();
            let ftype = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ftype.is_dir() {
                walk(&p, out);
            } else if ftype.is_file() && p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(c) = std::fs::read_to_string(&p) {
                    out.push((p, c));
                }
            }
        }
    }
    walk(root, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Resume materialization
// ---------------------------------------------------------------------------

/// Result of [`materialize_resume_session`]. Drop calls `cleanup` on the
/// temporary config dir; alternatively call [`MaterializedResume::cleanup`]
/// explicitly.
pub struct MaterializedResume {
    /// Temporary directory laid out like `~/.claude/`. Point the subprocess
    /// at it via `CLAUDE_CONFIG_DIR`.
    pub config_dir: PathBuf,
    /// Session ID to pass as `--resume`. When the input was
    /// `continue_conversation`, this is the most-recent session resolved
    /// via [`SessionStore::list_sessions`].
    pub resume_session_id: String,
    /// Whether the temp dir is still owned by this struct. Set to `false`
    /// after `cleanup` runs.
    owned: bool,
}

impl MaterializedResume {
    /// Best-effort recursive removal of the temp config dir. Idempotent.
    pub fn cleanup(&mut self) {
        if self.owned {
            let _ = std::fs::remove_dir_all(&self.config_dir);
            self.owned = false;
        }
    }
}

impl Drop for MaterializedResume {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Load a session from `options.session_store` and write it to a temp dir.
///
/// Returns `None` when no materialization is needed (no store, no
/// resume/continue, store has no entries, or the resolved session ID is not
/// a valid UUID). For `continue_conversation` this means a fresh session;
/// for an explicit `resume` value the CLI receives it unchanged.
///
/// Note: unlike the Python SDK, this Rust port does **not** copy auth
/// credentials into the temp dir. Callers that rely on file-based auth
/// (`~/.claude/.credentials.json`) under `CLAUDE_CONFIG_DIR` should set
/// `CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_API_KEY` in `options.env`
/// instead. API-key auth Just Works.
pub async fn materialize_resume_session(
    options: &ClaudeAgentOptions,
) -> Result<Option<MaterializedResume>> {
    let store = match options.session_store.as_ref() {
        Some(s) => s.clone(),
        None => return Ok(None),
    };
    if options.resume.is_none() && !options.continue_conversation {
        return Ok(None);
    }
    let timeout = std::time::Duration::from_millis(options.load_timeout_ms.unwrap_or(60_000));
    let project_key = crate::sessions::project_key_for_directory(
        options.cwd.as_ref().and_then(|p| p.to_str()),
    );

    let resolved = if let Some(resume_id) = options.resume.as_deref() {
        // session_id is used as a path component; reject non-UUIDs to
        // prevent traversal.
        if !is_uuid(resume_id) {
            return Ok(None);
        }
        load_with_timeout(&*store, &project_key, resume_id, timeout)
            .await?
            .map(|entries| (resume_id.to_string(), entries))
    } else {
        resolve_continue_candidate(&*store, &project_key, timeout).await?
    };
    let (session_id, entries) = match resolved {
        Some(r) => r,
        None => return Ok(None),
    };

    let tmp_base = tempfile::Builder::new()
        .prefix("claude-resume-")
        .tempdir()
        .map_err(ClaudeSdkError::Io)?
        .keep();
    let tmp_base_clone = tmp_base.clone();
    let result: Result<()> = (|| {
        let project_dir = tmp_base_clone.join("projects").join(&project_key);
        std::fs::create_dir_all(&project_dir)?;
        write_jsonl(&project_dir.join(format!("{session_id}.jsonl")), &entries)?;
        Ok::<(), ClaudeSdkError>(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp_base);
        return Err(e);
    }

    // Materialize subagent transcripts if the store enumerates them.
    let project_dir = tmp_base.join("projects").join(&project_key);
    let subkeys_res = tokio::time::timeout(
        timeout,
        store.list_subkeys(&SessionListSubkeysKey {
            project_key: project_key.clone(),
            session_id: session_id.clone(),
        }),
    )
    .await;
    if let Ok(Ok(subkeys)) = subkeys_res {
        for sub in subkeys {
            let key = SessionKey {
                project_key: project_key.clone(),
                session_id: session_id.clone(),
                subpath: sub.clone(),
            };
            let entries = match tokio::time::timeout(timeout, store.load(&key)).await {
                Ok(Ok(Some(e))) => e,
                _ => continue,
            };
            // Path: <project_dir>/<session_id>/<subpath>.jsonl
            let target = project_dir.join(&session_id).join(format!("{sub}.jsonl"));
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = write_jsonl(&target, &entries);
        }
    }

    Ok(Some(MaterializedResume {
        config_dir: tmp_base,
        resume_session_id: session_id,
        owned: true,
    }))
}

fn is_uuid(s: &str) -> bool {
    use std::sync::OnceLock;
    static R: OnceLock<regex::Regex> = OnceLock::new();
    let re = R.get_or_init(|| {
        regex::Regex::new(
            r"^(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
        )
        .unwrap()
    });
    re.is_match(s)
}

async fn load_with_timeout(
    store: &dyn SessionStore,
    project_key: &str,
    session_id: &str,
    timeout: Duration,
) -> Result<Option<Vec<SessionStoreEntry>>> {
    let key = SessionKey {
        project_key: project_key.into(),
        session_id: session_id.into(),
        subpath: String::new(),
    };
    match tokio::time::timeout(timeout, store.load(&key)).await {
        Ok(Ok(opt)) => Ok(opt),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(ClaudeSdkError::SessionStoreTimeout(format!(
            "load({session_id}) exceeded {}ms",
            timeout.as_millis()
        ))),
    }
}

async fn resolve_continue_candidate(
    store: &dyn SessionStore,
    project_key: &str,
    timeout: Duration,
) -> Result<Option<(String, Vec<SessionStoreEntry>)>> {
    let listing = match tokio::time::timeout(timeout, store.list_sessions(project_key)).await {
        Ok(Ok(l)) => l,
        Ok(Err(ClaudeSdkError::NotImplemented(_))) => return Ok(None),
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(ClaudeSdkError::SessionStoreTimeout(format!(
                "list_sessions({project_key}) exceeded {}ms",
                timeout.as_millis()
            )))
        }
    };
    if listing.is_empty() {
        return Ok(None);
    }
    let mut best: Option<&SessionStoreListEntry> = None;
    for e in &listing {
        match best {
            None => best = Some(e),
            Some(cur) if e.mtime > cur.mtime => best = Some(e),
            _ => {}
        }
    }
    let candidate = best.unwrap();
    match load_with_timeout(store, project_key, &candidate.session_id, timeout).await? {
        Some(entries) => Ok(Some((candidate.session_id.clone(), entries))),
        None => Ok(None),
    }
}

fn write_jsonl(path: &Path, entries: &[SessionStoreEntry]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    for entry in entries {
        // Hoist `type` to the front so the disk-layout byte shape matches
        // what the disk path produces (the lite-parse scans for
        // `{"type":"tag"` as a line prefix).
        let serialized = if let Value::Object(map) = entry {
            if map.contains_key("type") {
                let mut reordered = serde_json::Map::with_capacity(map.len());
                if let Some(t) = map.get("type") {
                    reordered.insert("type".into(), t.clone());
                }
                for (k, v) in map.iter() {
                    if k != "type" {
                        reordered.insert(k.clone(), v.clone());
                    }
                }
                serde_json::to_string(&Value::Object(reordered))?
            } else {
                serde_json::to_string(entry)?
            }
        } else {
            serde_json::to_string(entry)?
        };
        f.write_all(serialized.as_bytes())?;
        f.write_all(b"\n")?;
    }
    Ok(())
}

/// Apply a [`MaterializedResume`] to options: rewrite `env` with
/// `CLAUDE_CONFIG_DIR`, set `resume`, clear `continue_conversation`.
///
/// The caller is responsible for keeping the [`MaterializedResume`] alive
/// for the duration of the subprocess (its `Drop` impl removes the temp
/// dir). Typically this means storing it on the client/options struct
/// alongside the spawned process.
pub fn apply_materialized_options(
    options: &mut ClaudeAgentOptions,
    materialized: &MaterializedResume,
) {
    options.env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        materialized.config_dir.to_string_lossy().into_owned(),
    );
    options.resume = Some(materialized.resume_session_id.clone());
    options.continue_conversation = false;
}
