//! Session mutations — `rename_session`, `tag_session`, `delete_session`, `fork_session`.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use regex::Regex;
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::errors::{ClaudeSdkError, Result};
use crate::sessions::{
    canonicalize_path, extract_first_prompt_from_head, extract_last_json_string_field,
    find_project_dir, projects_dir, validate_uuid, worktree_paths, LITE_READ_BUF_SIZE,
};

/// Result returned from [`fork_session`].
#[derive(Debug, Clone)]
pub struct ForkSessionResult {
    pub session_id: String,
}

/// Append a `custom-title` entry to a session's JSONL file.
pub fn rename_session(session_id: &str, title: &str, directory: Option<&str>) -> Result<()> {
    if validate_uuid(session_id).is_none() {
        return Err(ClaudeSdkError::InvalidArgument(format!("Invalid session_id: {session_id}")));
    }
    let stripped = title.trim();
    if stripped.is_empty() {
        return Err(ClaudeSdkError::InvalidArgument("title must be non-empty".into()));
    }
    let data = json!({
        "type": "custom-title",
        "customTitle": stripped,
        "sessionId": session_id,
    }).to_string() + "\n";
    append_to_session(session_id, &data, directory)
}

/// Append a `tag` entry. Pass `None` to clear the tag.
pub fn tag_session(session_id: &str, tag: Option<&str>, directory: Option<&str>) -> Result<()> {
    if validate_uuid(session_id).is_none() {
        return Err(ClaudeSdkError::InvalidArgument(format!("Invalid session_id: {session_id}")));
    }
    let final_tag = match tag {
        None => String::new(),
        Some(t) => {
            let s = sanitize_unicode(t).trim().to_string();
            if s.is_empty() {
                return Err(ClaudeSdkError::InvalidArgument("tag must be non-empty (use None to clear)".into()));
            }
            s
        }
    };
    let data = json!({
        "type": "tag",
        "tag": final_tag,
        "sessionId": session_id,
    }).to_string() + "\n";
    append_to_session(session_id, &data, directory)
}

/// Hard-delete a session by removing its JSONL file and subagent transcript directory.
pub fn delete_session(session_id: &str, directory: Option<&str>) -> Result<()> {
    if validate_uuid(session_id).is_none() {
        return Err(ClaudeSdkError::InvalidArgument(format!("Invalid session_id: {session_id}")));
    }
    let path = find_session_file(session_id, directory).ok_or_else(|| {
        ClaudeSdkError::FileNotFound(format!("Session {session_id} not found"))
    })?;
    std::fs::remove_file(&path)?;
    let subagent = path.parent().unwrap().join(session_id);
    let _ = std::fs::remove_dir_all(subagent);
    Ok(())
}

/// Fork a session into a new branch with fresh UUIDs.
pub fn fork_session(
    session_id: &str,
    directory: Option<&str>,
    up_to_message_id: Option<&str>,
    title: Option<&str>,
) -> Result<ForkSessionResult> {
    if validate_uuid(session_id).is_none() {
        return Err(ClaudeSdkError::InvalidArgument(format!("Invalid session_id: {session_id}")));
    }
    if let Some(m) = up_to_message_id {
        if validate_uuid(m).is_none() {
            return Err(ClaudeSdkError::InvalidArgument(format!("Invalid up_to_message_id: {m}")));
        }
    }
    let (file_path, project_dir) = find_session_file_with_dir(session_id, directory)
        .ok_or_else(|| ClaudeSdkError::FileNotFound(format!("Session {session_id} not found")))?;

    let content = std::fs::read(&file_path)?;
    if content.is_empty() {
        return Err(ClaudeSdkError::InvalidArgument(format!(
            "Session {session_id} has no messages to fork"
        )));
    }
    let (transcript, content_replacements) = parse_fork_transcript(&content, session_id);
    let mut transcript: Vec<Value> = transcript
        .into_iter()
        .filter(|e| !e.get("isSidechain").and_then(Value::as_bool).unwrap_or(false))
        .collect();
    if transcript.is_empty() {
        return Err(ClaudeSdkError::InvalidArgument(format!(
            "Session {session_id} has no messages to fork"
        )));
    }
    if let Some(cutoff_id) = up_to_message_id {
        let cutoff = transcript.iter().position(|e| e.get("uuid").and_then(Value::as_str) == Some(cutoff_id));
        let cutoff = cutoff.ok_or_else(|| {
            ClaudeSdkError::InvalidArgument(format!("Message {cutoff_id} not found in session {session_id}"))
        })?;
        transcript.truncate(cutoff + 1);
    }

    let mut uuid_mapping: HashMap<String, String> = HashMap::new();
    for entry in &transcript {
        let old = entry.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
        uuid_mapping.insert(old, Uuid::new_v4().to_string());
    }
    let writable: Vec<&Value> = transcript
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) != Some("progress"))
        .collect();
    if writable.is_empty() {
        return Err(ClaudeSdkError::InvalidArgument(format!(
            "Session {session_id} has no messages to fork"
        )));
    }
    let by_uuid: HashMap<&str, &Value> = transcript
        .iter()
        .map(|e| (e.get("uuid").and_then(Value::as_str).unwrap_or(""), e))
        .collect();
    let forked_session_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut lines: Vec<String> = Vec::new();
    let writable_len = writable.len();
    for (i, original) in writable.iter().enumerate() {
        let new_uuid = uuid_mapping.get(original.get("uuid").and_then(Value::as_str).unwrap_or("")).cloned().unwrap();
        let mut new_parent: Option<String> = None;
        let mut parent_id = original.get("parentUuid").and_then(Value::as_str).map(String::from);
        while let Some(pid) = parent_id.clone() {
            let parent = match by_uuid.get(pid.as_str()) { Some(p) => *p, None => break };
            if parent.get("type").and_then(Value::as_str) != Some("progress") {
                new_parent = uuid_mapping.get(&pid).cloned();
                break;
            }
            parent_id = parent.get("parentUuid").and_then(Value::as_str).map(String::from);
        }
        let timestamp = if i == writable_len - 1 {
            now.clone()
        } else {
            original.get("timestamp").and_then(Value::as_str).unwrap_or(&now).to_string()
        };
        let logical_parent = original.get("logicalParentUuid").and_then(Value::as_str).map(String::from);
        let new_logical_parent = logical_parent.as_ref().map(|lp| uuid_mapping.get(lp).cloned().unwrap_or_else(|| lp.clone()));
        let mut forked = original.as_object().cloned().unwrap_or_default();
        forked.insert("uuid".into(), Value::String(new_uuid));
        forked.insert("parentUuid".into(), new_parent.map(Value::String).unwrap_or(Value::Null));
        forked.insert("logicalParentUuid".into(), new_logical_parent.map(Value::String).unwrap_or(Value::Null));
        forked.insert("sessionId".into(), Value::String(forked_session_id.clone()));
        forked.insert("timestamp".into(), Value::String(timestamp));
        forked.insert("isSidechain".into(), Value::Bool(false));
        forked.insert(
            "forkedFrom".into(),
            json!({
                "sessionId": session_id,
                "messageUuid": original.get("uuid").and_then(Value::as_str).unwrap_or(""),
            }),
        );
        for k in ["teamName", "agentName", "slug", "sourceToolAssistantUUID"] {
            forked.remove(k);
        }
        lines.push(serde_json::to_string(&Value::Object(forked))?);
    }
    if !content_replacements.is_empty() {
        lines.push(json!({
            "type": "content-replacement",
            "sessionId": forked_session_id,
            "replacements": content_replacements,
        }).to_string());
    }
    let fork_title = match title.map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            let buf_len = content.len();
            let head_end = buf_len.min(LITE_READ_BUF_SIZE);
            let head = String::from_utf8_lossy(&content[..head_end]).into_owned();
            let tail_start = if buf_len > LITE_READ_BUF_SIZE { buf_len - LITE_READ_BUF_SIZE } else { 0 };
            let tail = String::from_utf8_lossy(&content[tail_start..]).into_owned();
            let base = extract_last_json_string_field(&tail, "customTitle")
                .or_else(|| extract_last_json_string_field(&head, "customTitle"))
                .or_else(|| extract_last_json_string_field(&tail, "aiTitle"))
                .or_else(|| extract_last_json_string_field(&head, "aiTitle"))
                .unwrap_or_else(|| {
                    let fp = extract_first_prompt_from_head(&head);
                    if fp.is_empty() { "Forked session".into() } else { fp }
                });
            format!("{base} (fork)")
        }
    };
    lines.push(json!({
        "type": "custom-title",
        "sessionId": forked_session_id,
        "customTitle": fork_title,
    }).to_string());
    let fork_path = project_dir.join(format!("{forked_session_id}.jsonl"));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&fork_path)?;
    f.write_all((lines.join("\n") + "\n").as_bytes())?;
    Ok(ForkSessionResult { session_id: forked_session_id })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_session_file(session_id: &str, directory: Option<&str>) -> Option<PathBuf> {
    find_session_file_with_dir(session_id, directory).map(|(p, _)| p)
}

fn find_session_file_with_dir(session_id: &str, directory: Option<&str>) -> Option<(PathBuf, PathBuf)> {
    let file_name = format!("{session_id}.jsonl");
    let try_dir = |pd: &Path| -> Option<(PathBuf, PathBuf)> {
        let p = pd.join(&file_name);
        match std::fs::metadata(&p) {
            Ok(m) if m.len() > 0 => Some((p, pd.to_path_buf())),
            _ => None,
        }
    };
    if let Some(dir) = directory {
        let canonical = canonicalize_path(dir);
        if let Some(pd) = find_project_dir(&canonical) {
            if let Some(r) = try_dir(&pd) { return Some(r); }
        }
        for wt in worktree_paths(&canonical) {
            if wt == canonical { continue; }
            if let Some(wpd) = find_project_dir(&wt) {
                if let Some(r) = try_dir(&wpd) { return Some(r); }
            }
        }
        return None;
    }
    let pdir = projects_dir();
    let read = std::fs::read_dir(&pdir).ok()?;
    for entry in read.flatten() {
        if let Some(r) = try_dir(&entry.path()) { return Some(r); }
    }
    None
}

const TRANSCRIPT_TYPES: &[&str] = &["user", "assistant", "attachment", "system", "progress"];

fn parse_fork_transcript(content: &[u8], session_id: &str) -> (Vec<Value>, Vec<Value>) {
    let s = String::from_utf8_lossy(content).into_owned();
    let mut transcript = Vec::new();
    let mut replacements = Vec::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let entry: Value = match serde_json::from_str(line) { Ok(e) => e, Err(_) => continue };
        if !entry.is_object() { continue; }
        let t = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if TRANSCRIPT_TYPES.contains(&t) && entry.get("uuid").and_then(Value::as_str).is_some() {
            transcript.push(entry);
        } else if t == "content-replacement"
            && entry.get("sessionId").and_then(Value::as_str) == Some(session_id)
        {
            if let Some(r) = entry.get("replacements").and_then(Value::as_array) {
                replacements.extend(r.iter().cloned());
            }
        }
    }
    (transcript, replacements)
}

fn append_to_session(session_id: &str, data: &str, directory: Option<&str>) -> Result<()> {
    let file_name = format!("{session_id}.jsonl");
    let try_append = |path: &Path| -> Result<bool> {
        match OpenOptions::new().write(true).append(true).open(path) {
            Ok(mut f) => {
                let m = f.metadata()?;
                if m.len() == 0 { return Ok(false); }
                f.write_all(data.as_bytes())?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    };
    if let Some(dir) = directory {
        let canonical = canonicalize_path(dir);
        if let Some(pd) = find_project_dir(&canonical) {
            if try_append(&pd.join(&file_name))? { return Ok(()); }
        }
        for wt in worktree_paths(&canonical) {
            if wt == canonical { continue; }
            if let Some(wpd) = find_project_dir(&wt) {
                if try_append(&wpd.join(&file_name))? { return Ok(()); }
            }
        }
        return Err(ClaudeSdkError::FileNotFound(format!(
            "Session {session_id} not found in project directory for {dir}"
        )));
    }
    let pdir = projects_dir();
    let read = std::fs::read_dir(&pdir)?;
    for entry in read.flatten() {
        if try_append(&entry.path().join(&file_name))? { return Ok(()); }
    }
    Err(ClaudeSdkError::FileNotFound(format!(
        "Session {session_id} not found in any project directory"
    )))
}

// ---------------------------------------------------------------------------
// Unicode sanitization
// ---------------------------------------------------------------------------

fn unicode_strip_re() -> &'static Regex {
    use std::sync::OnceLock;
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            "[\u{200B}-\u{200F}\u{202A}-\u{202E}\u{2066}-\u{2069}\u{FEFF}\u{E000}-\u{F8FF}]",
        )
        .unwrap()
    })
}

fn sanitize_unicode(value: &str) -> String {
    let mut current = value.to_string();
    for _ in 0..10 {
        let previous = current.clone();
        // NFKC normalize
        current = current.nfkc().collect::<String>();
        // Strip Cf (format) / Co (private use) / Cn (unassigned) categories.
        current = current
            .chars()
            .filter(|c| {
                let cat = unicode_category(*c);
                cat != UnicodeCategory::Cf && cat != UnicodeCategory::Co && cat != UnicodeCategory::Cn
            })
            .collect();
        // Strip explicit dangerous ranges.
        current = unicode_strip_re().replace_all(&current, "").into_owned();
        if current == previous { break; }
    }
    current
}

#[derive(PartialEq, Eq)]
enum UnicodeCategory { Cf, Co, Cn, Other }

fn unicode_category(c: char) -> UnicodeCategory {
    let cp = c as u32;
    // Private use areas (Co)
    if (0xE000..=0xF8FF).contains(&cp) || (0xF0000..=0xFFFFD).contains(&cp) || (0x100000..=0x10FFFD).contains(&cp) {
        return UnicodeCategory::Co;
    }
    // Common Cf characters we care about
    matches!(cp,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x180E |
        0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 |
        0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB
    ).then_some(UnicodeCategory::Cf).unwrap_or(UnicodeCategory::Other)
}
