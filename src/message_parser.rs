//! Parses raw JSON messages from the CLI into typed [`Message`] variants.

use serde_json::Value;
use tracing::debug;

use crate::errors::{ClaudeSdkError, Result};
use crate::types::*;

/// Parse a raw CLI message dict. Returns `Ok(None)` for unrecognized message
/// types (forward-compat) so newer CLI versions don't crash older SDKs.
pub fn parse_message(data: &Value) -> Result<Option<Message>> {
    let obj = data.as_object().ok_or_else(|| {
        ClaudeSdkError::message_parse(
            format!("Invalid message data type (expected object, got {})", short_type(data)),
            Some(data.clone()),
        )
    })?;

    let msg_type = obj.get("type").and_then(Value::as_str).ok_or_else(|| {
        ClaudeSdkError::message_parse("Message missing 'type' field", Some(data.clone()))
    })?;

    match msg_type {
        "user" => parse_user(data).map(Some),
        "assistant" => parse_assistant(data).map(Some),
        "system" => parse_system(data).map(Some),
        "result" => parse_result(data).map(Some),
        "stream_event" => parse_stream_event(data).map(Some),
        "rate_limit_event" => parse_rate_limit(data).map(Some),
        other => {
            debug!("Skipping unknown message type: {other}");
            Ok(None)
        }
    }
}

fn short_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn missing(field: &str, data: &Value) -> ClaudeSdkError {
    ClaudeSdkError::message_parse(
        format!("Missing required field: {field}"),
        Some(data.clone()),
    )
}

fn parse_blocks(blocks_val: &Value, data: &Value) -> Result<Vec<ContentBlock>> {
    let arr = blocks_val.as_array().ok_or_else(|| {
        ClaudeSdkError::message_parse("'content' is not a list", Some(data.clone()))
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for b in arr {
        let t = b.get("type").and_then(Value::as_str).unwrap_or("");
        let block = match t {
            "text" => ContentBlock::Text(TextBlock {
                text: b.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
            }),
            "thinking" => ContentBlock::Thinking(ThinkingBlock {
                thinking: b.get("thinking").and_then(Value::as_str).unwrap_or("").to_string(),
                signature: b.get("signature").and_then(Value::as_str).unwrap_or("").to_string(),
            }),
            "tool_use" => ContentBlock::ToolUse(ToolUseBlock {
                id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                input: b.get("input").cloned().unwrap_or(Value::Null),
            }),
            "tool_result" => ContentBlock::ToolResult(ToolResultBlock {
                tool_use_id: b
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                content: b.get("content").cloned(),
                is_error: b.get("is_error").and_then(Value::as_bool),
            }),
            _ => continue,
        };
        out.push(block);
    }
    Ok(out)
}

fn parse_user(data: &Value) -> Result<Message> {
    let message = data.get("message").ok_or_else(|| missing("message", data))?;
    let content_val = message.get("content").ok_or_else(|| missing("message.content", data))?;
    let content = if content_val.is_array() {
        UserContent::Blocks(parse_blocks(content_val, data)?)
    } else if let Some(s) = content_val.as_str() {
        UserContent::Text(s.to_string())
    } else {
        UserContent::Text(content_val.to_string())
    };
    Ok(Message::User(UserMessage {
        content,
        uuid: data.get("uuid").and_then(Value::as_str).map(String::from),
        parent_tool_use_id: data
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .map(String::from),
        tool_use_result: data.get("tool_use_result").cloned(),
    }))
}

fn parse_assistant(data: &Value) -> Result<Message> {
    let message = data.get("message").ok_or_else(|| missing("message", data))?;
    let content = parse_blocks(
        message.get("content").ok_or_else(|| missing("message.content", data))?,
        data,
    )?;
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| missing("message.model", data))?
        .to_string();
    let error = data.get("error").and_then(Value::as_str).and_then(|s| {
        serde_json::from_str::<AssistantMessageError>(&format!("\"{s}\"")).ok()
    });
    Ok(Message::Assistant(AssistantMessage {
        content,
        model,
        parent_tool_use_id: data
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .map(String::from),
        error,
        usage: message.get("usage").cloned(),
        message_id: message.get("id").and_then(Value::as_str).map(String::from),
        stop_reason: message.get("stop_reason").and_then(Value::as_str).map(String::from),
        session_id: data.get("session_id").and_then(Value::as_str).map(String::from),
        uuid: data.get("uuid").and_then(Value::as_str).map(String::from),
    }))
}

fn str_field(v: &Value, k: &str, data: &Value) -> Result<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| missing(k, data))
}

fn parse_system(data: &Value) -> Result<Message> {
    let subtype = str_field(data, "subtype", data)?;
    let task = match subtype.as_str() {
        "task_started" => Some(TaskMessage::Started {
            task_id: str_field(data, "task_id", data)?,
            description: str_field(data, "description", data)?,
            uuid: str_field(data, "uuid", data)?,
            session_id: str_field(data, "session_id", data)?,
            tool_use_id: data.get("tool_use_id").and_then(Value::as_str).map(String::from),
            task_type: data.get("task_type").and_then(Value::as_str).map(String::from),
        }),
        "task_progress" => {
            let usage: TaskUsage = serde_json::from_value(
                data.get("usage").cloned().ok_or_else(|| missing("usage", data))?,
            )?;
            Some(TaskMessage::Progress {
                task_id: str_field(data, "task_id", data)?,
                description: str_field(data, "description", data)?,
                usage,
                uuid: str_field(data, "uuid", data)?,
                session_id: str_field(data, "session_id", data)?,
                tool_use_id: data.get("tool_use_id").and_then(Value::as_str).map(String::from),
                last_tool_name: data
                    .get("last_tool_name")
                    .and_then(Value::as_str)
                    .map(String::from),
            })
        }
        "task_notification" => {
            let status: TaskNotificationStatus = serde_json::from_value(
                data.get("status").cloned().ok_or_else(|| missing("status", data))?,
            )?;
            let usage = data
                .get("usage")
                .cloned()
                .map(serde_json::from_value::<TaskUsage>)
                .transpose()?;
            Some(TaskMessage::Notification {
                task_id: str_field(data, "task_id", data)?,
                status,
                output_file: str_field(data, "output_file", data)?,
                summary: str_field(data, "summary", data)?,
                uuid: str_field(data, "uuid", data)?,
                session_id: str_field(data, "session_id", data)?,
                tool_use_id: data.get("tool_use_id").and_then(Value::as_str).map(String::from),
                usage,
            })
        }
        _ => None,
    };
    Ok(Message::System(SystemMessage { subtype, data: data.clone(), task }))
}

fn parse_result(data: &Value) -> Result<Message> {
    Ok(Message::Result(ResultMessage {
        subtype: str_field(data, "subtype", data)?,
        duration_ms: data.get("duration_ms").and_then(Value::as_u64).ok_or_else(|| missing("duration_ms", data))?,
        duration_api_ms: data.get("duration_api_ms").and_then(Value::as_u64).ok_or_else(|| missing("duration_api_ms", data))?,
        is_error: data.get("is_error").and_then(Value::as_bool).ok_or_else(|| missing("is_error", data))?,
        num_turns: data.get("num_turns").and_then(Value::as_u64).ok_or_else(|| missing("num_turns", data))?,
        session_id: str_field(data, "session_id", data)?,
        stop_reason: data.get("stop_reason").and_then(Value::as_str).map(String::from),
        total_cost_usd: data.get("total_cost_usd").and_then(Value::as_f64),
        usage: data.get("usage").cloned(),
        result: data.get("result").and_then(Value::as_str).map(String::from),
        structured_output: data.get("structured_output").cloned(),
        model_usage: data.get("modelUsage").cloned(),
        permission_denials: data
            .get("permission_denials")
            .and_then(Value::as_array)
            .cloned(),
        errors: data
            .get("errors")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()),
        uuid: data.get("uuid").and_then(Value::as_str).map(String::from),
    }))
}

fn parse_stream_event(data: &Value) -> Result<Message> {
    Ok(Message::StreamEvent(StreamEvent {
        uuid: str_field(data, "uuid", data)?,
        session_id: str_field(data, "session_id", data)?,
        event: data.get("event").cloned().ok_or_else(|| missing("event", data))?,
        parent_tool_use_id: data
            .get("parent_tool_use_id")
            .and_then(Value::as_str)
            .map(String::from),
    }))
}

fn parse_rate_limit(data: &Value) -> Result<Message> {
    let info = data
        .get("rate_limit_info")
        .ok_or_else(|| missing("rate_limit_info", data))?;
    let status: RateLimitStatus = serde_json::from_value(
        info.get("status").cloned().ok_or_else(|| missing("rate_limit_info.status", data))?,
    )?;
    let rate_limit_type = info
        .get("rateLimitType")
        .cloned()
        .map(serde_json::from_value::<RateLimitType>)
        .transpose()
        .ok()
        .flatten();
    let overage_status = info
        .get("overageStatus")
        .cloned()
        .map(serde_json::from_value::<RateLimitStatus>)
        .transpose()
        .ok()
        .flatten();
    Ok(Message::RateLimitEvent(RateLimitEvent {
        rate_limit_info: RateLimitInfo {
            status,
            resets_at: info.get("resetsAt").and_then(Value::as_i64),
            rate_limit_type,
            utilization: info.get("utilization").and_then(Value::as_f64),
            overage_status,
            overage_resets_at: info.get("overageResetsAt").and_then(Value::as_i64),
            overage_disabled_reason: info
                .get("overageDisabledReason")
                .and_then(Value::as_str)
                .map(String::from),
            raw: info.clone(),
        },
        uuid: str_field(data, "uuid", data)?,
        session_id: str_field(data, "session_id", data)?,
    }))
}
