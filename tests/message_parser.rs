use claude_agent_sdk::message_parser::parse_message;
use claude_agent_sdk::{ContentBlock, Message};
use serde_json::json;

#[test]
fn parses_assistant_text_message() {
    let v = json!({
        "type": "assistant",
        "session_id": "s1",
        "uuid": "u1",
        "message": {
            "id": "m1",
            "model": "claude-sonnet-4-5",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hi"}]
        }
    });
    let msg = parse_message(&v).unwrap().unwrap();
    let Message::Assistant(a) = msg else { panic!("expected assistant") };
    assert_eq!(a.model, "claude-sonnet-4-5");
    assert_eq!(a.message_id.as_deref(), Some("m1"));
    let ContentBlock::Text(t) = &a.content[0] else { panic!("expected text") };
    assert_eq!(t.text, "hi");
}

#[test]
fn parses_user_message_with_blocks() {
    let v = json!({
        "type": "user",
        "uuid": "u2",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
        }
    });
    let Message::User(u) = parse_message(&v).unwrap().unwrap() else { panic!() };
    assert_eq!(u.uuid.as_deref(), Some("u2"));
}

#[test]
fn unknown_message_type_is_skipped() {
    let v = json!({"type": "future_thing", "x": 1});
    assert!(parse_message(&v).unwrap().is_none());
}

#[test]
fn parses_result_message() {
    let v = json!({
        "type": "result",
        "subtype": "success",
        "duration_ms": 100,
        "duration_api_ms": 50,
        "is_error": false,
        "num_turns": 1,
        "session_id": "s1",
        "total_cost_usd": 0.001,
    });
    let Message::Result(r) = parse_message(&v).unwrap().unwrap() else { panic!() };
    assert_eq!(r.duration_ms, 100);
    assert!(!r.is_error);
}
