//! Lines written to a CLI's stdin (user messages and control responses).

use serde_json::{json, Value};

pub(crate) fn user_message_line(prompt: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": prompt }] }
    })
    .to_string()
}

pub(crate) fn steer_user_message_line(prompt: &str, uuid: &str, priority: &str) -> String {
    json!({
        "type": "user",
        "uuid": uuid,
        "priority": priority,
        "message": { "role": "user", "content": [{ "type": "text", "text": prompt }] }
    })
    .to_string()
}

pub(crate) fn control_response(request_id: &str, allow: bool, input: &Value) -> String {
    let body = if allow {
        json!({ "behavior": "allow", "updatedInput": input })
    } else {
        json!({ "behavior": "deny", "message": "The user denied this action in Vireo." })
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": body }
    })
    .to_string()
}

pub(crate) fn control_error(request_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": { "subtype": "error", "request_id": request_id, "error": "Unsupported control request" }
    })
    .to_string()
}

/// A request from us to Claude Code (`initialize`, `interrupt`, `set_model`, …).
pub(crate) fn control_request(request_id: &str, subtype: &str, extra: Value) -> String {
    let mut request = json!({ "subtype": subtype });
    if let (Some(dst), Some(src)) = (request.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    json!({ "type": "control_request", "request_id": request_id, "request": request }).to_string()
}
