//! Session listing — `list_sessions`, `get_session_info`, `get_session_messages`.
//!
//! Scans `~/.claude/projects/<sanitized-cwd>/` for `.jsonl` session files
//! and extracts metadata via stat + head/tail reads.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::DateTime;
use regex::Regex;
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::types::{SdkSessionInfo, SessionMessage, SessionMessageType};

pub(crate) const LITE_READ_BUF_SIZE: usize = 65536;
const MAX_SANITIZED_LENGTH: usize = 200;

fn uuid_regex() -> &'static Regex {
    use std::sync::OnceLock;
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
    })
}

fn sanitize_re() -> &'static Regex {
    use std::sync::OnceLock;
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^a-zA-Z0-9]").unwrap())
}

fn skip_first_prompt_re() -> &'static Regex {
    use std::sync::OnceLock;
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?s)^(?:<local-command-stdout>|<session-start-hook>|<tick>|<goal>|\[Request interrupted by user[^\]]*\]|\s*<ide_opened_file>.*</ide_opened_file>\s*$|\s*<ide_selection>.*</ide_selection>\s*$)",
        )
        .unwrap()
    })
}

fn command_name_re() -> &'static Regex {
    use std::sync::OnceLock;
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"<command-name>(.*?)</command-name>").unwrap())
}

pub(crate) fn validate_uuid(s: &str) -> Option<String> {
    if uuid_regex().is_match(s) {
        Some(s.to_string())
    } else {
        None
    }
}

fn simple_hash(s: &str) -> String {
    // Match the JS `simpleHash` impl byte-for-byte.
    let mut h: i64 = 0;
    for ch in s.chars() {
        let c = ch as i64;
        h = ((h << 5).wrapping_sub(h)).wrapping_add(c);
        // 32-bit signed coerce
        h &= 0xFFFFFFFF;
        if h >= 0x80000000 {
            h -= 0x100000000;
        }
    }
    let mut h = h.unsigned_abs();
    if h == 0 {
        return "0".into();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while h > 0 {
        out.push(digits[(h % 36) as usize]);
        h /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

fn sanitize_path(name: &str) -> String {
    let sanitized = sanitize_re().replace_all(name, "-").into_owned();
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        sanitized
    } else {
        let h = simple_hash(name);
        let prefix: String = sanitized.chars().take(MAX_SANITIZED_LENGTH).collect();
        format!("{prefix}-{h}")
    }
}

pub(crate) fn claude_config_home_dir() -> PathBuf {
    if let Ok(d) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(d.nfc().collect::<String>());
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(format!("{home}/.claude").nfc().collect::<String>())
}

pub(crate) fn projects_dir() -> PathBuf { claude_config_home_dir().join("projects") }

pub(crate) fn project_dir(project_path: &str) -> PathBuf {
    projects_dir().join(sanitize_path(project_path))
}

pub(crate) fn canonicalize_path(d: &str) -> String {
    let p = PathBuf::from(d);
    let resolved = std::fs::canonicalize(&p).unwrap_or(p);
    resolved.to_string_lossy().nfc().collect()
}

pub(crate) fn find_project_dir(project_path: &str) -> Option<PathBuf> {
    let exact = project_dir(project_path);
    if exact.is_dir() {
        return Some(exact);
    }
    let sanitized = sanitize_path(project_path);
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return None;
    }
    let prefix: String = sanitized.chars().take(MAX_SANITIZED_LENGTH).collect();
    let pdir = projects_dir();
    let read = std::fs::read_dir(&pdir).ok()?;
    for entry in read.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&format!("{prefix}-")) {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

pub(crate) struct LiteFile {
    pub mtime: i64,
    pub size: u64,
    pub head: String,
    pub tail: String,
}

pub(crate) fn read_session_lite(path: &Path) -> Option<LiteFile> {
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut f = File::open(path).ok()?;
    let mut head_bytes = vec![0u8; LITE_READ_BUF_SIZE];
    let n = f.read(&mut head_bytes).ok()?;
    if n == 0 {
        return None;
    }
    head_bytes.truncate(n);
    let head = String::from_utf8_lossy(&head_bytes).to_string();
    let tail = if size as usize <= LITE_READ_BUF_SIZE {
        head.clone()
    } else {
        let off = size - LITE_READ_BUF_SIZE as u64;
        f.seek(SeekFrom::Start(off)).ok()?;
        let mut tb = vec![0u8; LITE_READ_BUF_SIZE];
        let n2 = f.read(&mut tb).ok()?;
        tb.truncate(n2);
        String::from_utf8_lossy(&tb).to_string()
    };
    Some(LiteFile { mtime, size, head, tail })
}

pub(crate) fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    for pattern in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        if let Some(idx) = text.find(&pattern) {
            return extract_after(text, idx + pattern.len());
        }
    }
    None
}

pub(crate) fn extract_last_json_string_field(text: &str, key: &str) -> Option<String> {
    let mut last = None;
    for pattern in [format!("\"{key}\":\""), format!("\"{key}\": \"")] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(&pattern) {
            let idx = from + rel;
            let val_start = idx + pattern.len();
            if let Some(v) = extract_after(text, val_start) {
                last = Some(v);
            }
            from = val_start + 1;
        }
    }
    last
}

fn extract_after(text: &str, value_start: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = value_start;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            i += 2;
            continue;
        }
        if b == b'"' {
            let raw = &text[value_start..i];
            return Some(unescape_json_string(raw));
        }
        i += 1;
    }
    None
}

fn unescape_json_string(raw: &str) -> String {
    if !raw.contains('\\') {
        return raw.to_string();
    }
    serde_json::from_str::<String>(&format!("\"{raw}\"")).unwrap_or_else(|_| raw.to_string())
}

pub(crate) fn extract_first_prompt_from_head(head: &str) -> String {
    let mut command_fallback = String::new();
    for line in head.split('\n') {
        if !line.contains("\"type\":\"user\"") && !line.contains("\"type\": \"user\"") {
            continue;
        }
        if line.contains("\"tool_result\"")
            || line.contains("\"isMeta\":true")
            || line.contains("\"isMeta\": true")
            || line.contains("\"isCompactSummary\":true")
            || line.contains("\"isCompactSummary\": true")
        {
            continue;
        }
        let entry: Value = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let message = match entry.get("message").and_then(Value::as_object) {
            Some(m) => m,
            None => continue,
        };
        let content = message.get("content");
        let mut texts: Vec<String> = Vec::new();
        if let Some(Value::String(s)) = content {
            texts.push(s.clone());
        } else if let Some(Value::Array(arr)) = content {
            for b in arr {
                if b.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        texts.push(t.to_string());
                    }
                }
            }
        }
        for raw in texts {
            let result = raw.replace('\n', " ").trim().to_string();
            if result.is_empty() {
                continue;
            }
            if let Some(cap) = command_name_re().captures(&result) {
                if command_fallback.is_empty() {
                    command_fallback = cap.get(1).unwrap().as_str().to_string();
                }
                continue;
            }
            if skip_first_prompt_re().is_match(&result) {
                continue;
            }
            return if result.chars().count() > 200 {
                let mut s: String = result.chars().take(200).collect();
                s = s.trim_end().to_string();
                s.push('\u{2026}');
                s
            } else {
                result
            };
        }
    }
    command_fallback
}

fn parse_session_info_from_lite(
    session_id: &str,
    lite: &LiteFile,
    project_path: Option<&str>,
) -> Option<SdkSessionInfo> {
    let head = &lite.head;
    let tail = &lite.tail;
    let first_line = match head.find('\n') {
        Some(i) => &head[..i],
        None => head.as_str(),
    };
    if first_line.contains("\"isSidechain\":true") || first_line.contains("\"isSidechain\": true") {
        return None;
    }
    let custom_title = extract_last_json_string_field(tail, "customTitle")
        .or_else(|| extract_last_json_string_field(head, "customTitle"))
        .or_else(|| extract_last_json_string_field(tail, "aiTitle"))
        .or_else(|| extract_last_json_string_field(head, "aiTitle"));
    let first_prompt_str = extract_first_prompt_from_head(head);
    let first_prompt = if first_prompt_str.is_empty() { None } else { Some(first_prompt_str) };
    let summary = custom_title
        .clone()
        .or_else(|| extract_last_json_string_field(tail, "lastPrompt"))
        .or_else(|| extract_last_json_string_field(tail, "summary"))
        .or_else(|| first_prompt.clone());
    let summary = summary?;
    let git_branch = extract_last_json_string_field(tail, "gitBranch")
        .or_else(|| extract_json_string_field(head, "gitBranch"));
    let session_cwd = extract_json_string_field(head, "cwd").or_else(|| project_path.map(String::from));
    let tag_line = tail
        .split('\n')
        .rev()
        .find(|ln| ln.starts_with("{\"type\":\"tag\""));
    let tag = tag_line.and_then(|ln| extract_last_json_string_field(ln, "tag"));
    let mut created_at = None;
    if let Some(ts) = extract_json_string_field(first_line, "timestamp") {
        let normalized = if ts.ends_with('Z') {
            format!("{}+00:00", &ts[..ts.len() - 1])
        } else {
            ts
        };
        if let Ok(dt) = DateTime::parse_from_rfc3339(&normalized) {
            created_at = Some(dt.timestamp_millis());
        }
    }
    Some(SdkSessionInfo {
        session_id: session_id.to_string(),
        summary,
        last_modified: lite.mtime,
        file_size: Some(lite.size),
        custom_title,
        first_prompt,
        git_branch,
        cwd: session_cwd,
        tag,
        created_at,
    })
}

fn read_sessions_from_dir(dir: &Path, project_path: Option<&str>) -> Vec<SdkSessionInfo> {
    let mut out = Vec::new();
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for entry in read.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".jsonl") {
            continue;
        }
        let stem = &name_str[..name_str.len() - 6];
        let session_id = match validate_uuid(stem) {
            Some(s) => s,
            None => continue,
        };
        let lite = match read_session_lite(&entry.path()) {
            Some(l) => l,
            None => continue,
        };
        if let Some(info) = parse_session_info_from_lite(&session_id, &lite, project_path) {
            out.push(info);
        }
    }
    out
}

fn dedup_by_id(mut sessions: Vec<SdkSessionInfo>) -> Vec<SdkSessionInfo> {
    let mut by_id: HashMap<String, SdkSessionInfo> = HashMap::new();
    for s in sessions.drain(..) {
        match by_id.get(&s.session_id) {
            Some(existing) if existing.last_modified >= s.last_modified => {}
            _ => {
                by_id.insert(s.session_id.clone(), s);
            }
        }
    }
    by_id.into_values().collect()
}

fn apply_sort_limit(mut sessions: Vec<SdkSessionInfo>, limit: Option<usize>, offset: usize) -> Vec<SdkSessionInfo> {
    sessions.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    if offset > 0 && offset < sessions.len() {
        sessions = sessions.split_off(offset);
    } else if offset >= sessions.len() {
        sessions.clear();
    }
    if let Some(l) = limit {
        sessions.truncate(l);
    }
    sessions
}

pub(crate) fn worktree_paths(cwd: &str) -> Vec<String> {
    let out = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .split('\n')
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|s| s.nfc().collect::<String>())
        .collect()
}

fn list_for_project(directory: &str, limit: Option<usize>, offset: usize, include_worktrees: bool) -> Vec<SdkSessionInfo> {
    let canonical = canonicalize_path(directory);
    let wts = if include_worktrees { worktree_paths(&canonical) } else { Vec::new() };
    if wts.len() <= 1 {
        let pd = match find_project_dir(&canonical) { Some(p) => p, None => return Vec::new() };
        let sessions = read_sessions_from_dir(&pd, Some(&canonical));
        return apply_sort_limit(sessions, limit, offset);
    }
    // Worktree-aware
    let pdir = projects_dir();
    let mut all = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(canonical_pd) = find_project_dir(&canonical) {
        if let Some(name) = canonical_pd.file_name().and_then(|n| n.to_str()) {
            seen.insert(name.to_string());
            all.extend(read_sessions_from_dir(&canonical_pd, Some(&canonical)));
        }
    }
    let mut indexed: Vec<(String, String)> = wts.into_iter().map(|w| {
        let s = sanitize_path(&w);
        (w, s)
    }).collect();
    indexed.sort_by_key(|(_, p)| std::cmp::Reverse(p.len()));
    if let Ok(read) = std::fs::read_dir(&pdir) {
        for entry in read.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let name = match entry.file_name().to_str() { Some(s) => s.to_string(), None => continue };
            if seen.contains(&name) { continue; }
            for (wt, prefix) in &indexed {
                let exact = name == *prefix;
                let trunc = prefix.len() >= MAX_SANITIZED_LENGTH && name.starts_with(&format!("{prefix}-"));
                if exact || trunc {
                    seen.insert(name.clone());
                    all.extend(read_sessions_from_dir(&entry.path(), Some(wt)));
                    break;
                }
            }
        }
    }
    apply_sort_limit(dedup_by_id(all), limit, offset)
}

fn list_all(limit: Option<usize>, offset: usize) -> Vec<SdkSessionInfo> {
    let pdir = projects_dir();
    let mut all = Vec::new();
    if let Ok(read) = std::fs::read_dir(&pdir) {
        for entry in read.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                all.extend(read_sessions_from_dir(&entry.path(), None));
            }
        }
    }
    apply_sort_limit(dedup_by_id(all), limit, offset)
}

/// List sessions, with optional pagination and project directory scoping.
pub fn list_sessions(
    directory: Option<&str>,
    limit: Option<usize>,
    offset: usize,
    include_worktrees: bool,
) -> Vec<SdkSessionInfo> {
    match directory {
        Some(d) => list_for_project(d, limit, offset, include_worktrees),
        None => list_all(limit, offset),
    }
}

/// Look up metadata for a single session.
pub fn get_session_info(session_id: &str, directory: Option<&str>) -> Option<SdkSessionInfo> {
    let uuid = validate_uuid(session_id)?;
    let file_name = format!("{uuid}.jsonl");
    if let Some(dir) = directory {
        let canonical = canonicalize_path(dir);
        if let Some(pd) = find_project_dir(&canonical) {
            if let Some(lite) = read_session_lite(&pd.join(&file_name)) {
                return parse_session_info_from_lite(&uuid, &lite, Some(&canonical));
            }
        }
        for wt in worktree_paths(&canonical) {
            if wt == canonical { continue; }
            if let Some(wpd) = find_project_dir(&wt) {
                if let Some(lite) = read_session_lite(&wpd.join(&file_name)) {
                    return parse_session_info_from_lite(&uuid, &lite, Some(&wt));
                }
            }
        }
        return None;
    }
    let pdir = projects_dir();
    let read = std::fs::read_dir(&pdir).ok()?;
    for entry in read.flatten() {
        if let Some(lite) = read_session_lite(&entry.path().join(&file_name)) {
            return parse_session_info_from_lite(&uuid, &lite, None);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// get_session_messages
// ---------------------------------------------------------------------------

const TRANSCRIPT_TYPES: &[&str] = &["user", "assistant", "progress", "system", "attachment"];

fn try_read_session_file(project_dir: &Path, file_name: &str) -> Option<String> {
    std::fs::read_to_string(project_dir.join(file_name)).ok()
}

pub(crate) fn read_session_file(session_id: &str, directory: Option<&str>) -> Option<String> {
    let file_name = format!("{session_id}.jsonl");
    if let Some(dir) = directory {
        let canonical = canonicalize_path(dir);
        if let Some(pd) = find_project_dir(&canonical) {
            if let Some(c) = try_read_session_file(&pd, &file_name) { return Some(c); }
        }
        for wt in worktree_paths(&canonical) {
            if wt == canonical { continue; }
            if let Some(wpd) = find_project_dir(&wt) {
                if let Some(c) = try_read_session_file(&wpd, &file_name) { return Some(c); }
            }
        }
        return None;
    }
    let pdir = projects_dir();
    let read = std::fs::read_dir(&pdir).ok()?;
    for entry in read.flatten() {
        if let Some(c) = try_read_session_file(&entry.path(), &file_name) { return Some(c); }
    }
    None
}

fn parse_transcript_entries(content: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        let entry: Value = match serde_json::from_str(line) { Ok(e) => e, Err(_) => continue };
        if !entry.is_object() { continue; }
        let t = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if TRANSCRIPT_TYPES.contains(&t) && entry.get("uuid").and_then(Value::as_str).is_some() {
            out.push(entry);
        }
    }
    out
}

fn build_conversation_chain(entries: &[Value]) -> Vec<Value> {
    if entries.is_empty() { return Vec::new(); }
    let mut by_uuid: HashMap<&str, &Value> = HashMap::new();
    let mut entry_index: HashMap<&str, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let uid = e.get("uuid").and_then(Value::as_str).unwrap_or("");
        by_uuid.insert(uid, e);
        entry_index.insert(uid, i);
    }
    let mut parents: HashSet<&str> = HashSet::new();
    for e in entries {
        if let Some(p) = e.get("parentUuid").and_then(Value::as_str) {
            parents.insert(p);
        }
    }
    let terminals: Vec<&Value> = entries.iter()
        .filter(|e| !parents.contains(e.get("uuid").and_then(Value::as_str).unwrap_or("")))
        .collect();
    let mut leaves: Vec<&Value> = Vec::new();
    for terminal in terminals {
        let mut cur = Some(terminal);
        let mut seen = HashSet::new();
        while let Some(c) = cur {
            let uid = c.get("uuid").and_then(Value::as_str).unwrap_or("");
            if !seen.insert(uid) { break; }
            let t = c.get("type").and_then(Value::as_str).unwrap_or("");
            if t == "user" || t == "assistant" {
                leaves.push(c);
                break;
            }
            let parent = c.get("parentUuid").and_then(Value::as_str);
            cur = parent.and_then(|p| by_uuid.get(p).copied());
        }
    }
    if leaves.is_empty() { return Vec::new(); }
    let main_leaves: Vec<&&Value> = leaves.iter().filter(|leaf| {
        !leaf.get("isSidechain").and_then(Value::as_bool).unwrap_or(false)
            && leaf.get("teamName").is_none()
            && !leaf.get("isMeta").and_then(Value::as_bool).unwrap_or(false)
    }).collect();
    let pick_best = |cands: &[&Value]| -> Value {
        let mut best = cands[0];
        let mut best_idx = entry_index.get(best.get("uuid").and_then(Value::as_str).unwrap_or("")).copied().unwrap_or(0);
        for &c in cands.iter().skip(1) {
            let idx = entry_index.get(c.get("uuid").and_then(Value::as_str).unwrap_or("")).copied().unwrap_or(0);
            if idx > best_idx { best = c; best_idx = idx; }
        }
        best.clone()
    };
    let leaf = if !main_leaves.is_empty() {
        let v: Vec<&Value> = main_leaves.iter().map(|x| **x).collect();
        pick_best(&v)
    } else {
        pick_best(&leaves)
    };
    let mut chain: Vec<Value> = Vec::new();
    let mut chain_seen: HashSet<String> = HashSet::new();
    let mut cur_owned: Option<Value> = Some(leaf);
    while let Some(c) = cur_owned.take() {
        let uid = c.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
        if !chain_seen.insert(uid) { break; }
        let parent_id = c.get("parentUuid").and_then(Value::as_str).map(String::from);
        chain.push(c);
        if let Some(pid) = parent_id {
            if let Some(&pv) = by_uuid.get(pid.as_str()) {
                cur_owned = Some(pv.clone());
            }
        }
    }
    chain.reverse();
    chain
}

fn is_visible(entry: &Value) -> bool {
    let t = entry.get("type").and_then(Value::as_str).unwrap_or("");
    if t != "user" && t != "assistant" { return false; }
    if entry.get("isMeta").and_then(Value::as_bool).unwrap_or(false) { return false; }
    if entry.get("isSidechain").and_then(Value::as_bool).unwrap_or(false) { return false; }
    entry.get("teamName").is_none()
}

fn to_session_message(entry: &Value) -> SessionMessage {
    let t = entry.get("type").and_then(Value::as_str).unwrap_or("");
    let mtype = if t == "user" { SessionMessageType::User } else { SessionMessageType::Assistant };
    SessionMessage {
        r#type: mtype,
        uuid: entry.get("uuid").and_then(Value::as_str).unwrap_or("").to_string(),
        session_id: entry.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string(),
        message: entry.get("message").cloned(),
        parent_tool_use_id: None,
    }
}

/// Read a session's conversation messages in chronological order.
pub fn get_session_messages(
    session_id: &str,
    directory: Option<&str>,
    limit: Option<usize>,
    offset: usize,
) -> Vec<SessionMessage> {
    if validate_uuid(session_id).is_none() { return Vec::new(); }
    let content = match read_session_file(session_id, directory) { Some(c) => c, None => return Vec::new() };
    let entries = parse_transcript_entries(&content);
    let chain = build_conversation_chain(&entries);
    let visible: Vec<SessionMessage> = chain.iter().filter(|e| is_visible(e)).map(to_session_message).collect();
    let len = visible.len();
    if let Some(l) = limit {
        let end = (offset + l).min(len);
        if offset >= len { return Vec::new(); }
        return visible[offset..end].to_vec();
    }
    if offset > 0 {
        if offset >= len { return Vec::new(); }
        return visible[offset..].to_vec();
    }
    visible
}
