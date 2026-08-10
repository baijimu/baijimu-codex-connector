use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) const CODEX_TURN_COMPLETED: &str = "codexTurnCompleted";
pub(crate) const CODEX_THREAD_CLOSED: &str = "codexThreadClosed";
pub(crate) const CODEX_THREAD_ARCHIVED: &str = "codexThreadArchived";
pub(crate) const CODEX_THREAD_DELETED: &str = "codexThreadDeleted";

const SCHEMA_VERSION: u64 = 1;
const SOURCE: &str = "codex-app-server";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DomainEvent {
    pub(crate) name: &'static str,
    pub(crate) event_id: String,
    pub(crate) occurred_at: String,
    pub(crate) payload: Value,
}

pub(crate) fn normalize_codex_notification(
    method: &str,
    params: &Value,
    occurred_at: &str,
    stream_id: &str,
    sequence: u64,
) -> Result<Option<DomainEvent>, String> {
    match method {
        "turn/completed" => normalize_turn_completed(params, occurred_at),
        "thread/closed" => normalize_thread_lifecycle(
            CODEX_THREAD_CLOSED,
            method,
            params,
            occurred_at,
            stream_id,
            sequence,
        ),
        "thread/archived" => normalize_thread_lifecycle(
            CODEX_THREAD_ARCHIVED,
            method,
            params,
            occurred_at,
            stream_id,
            sequence,
        ),
        "thread/deleted" => normalize_thread_lifecycle(
            CODEX_THREAD_DELETED,
            method,
            params,
            occurred_at,
            stream_id,
            sequence,
        ),
        _ => Ok(None),
    }
}

fn normalize_turn_completed(
    params: &Value,
    occurred_at: &str,
) -> Result<Option<DomainEvent>, String> {
    let thread_id = required_string(params, "threadId", "turn/completed")?;
    let turn = params
        .get("turn")
        .and_then(Value::as_object)
        .ok_or_else(|| "turn/completed is missing object field turn".to_string())?;
    let turn_id = turn
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "turn/completed is missing turn.id".to_string())?;
    let status = turn
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| matches!(*status, "completed" | "interrupted" | "failed"))
        .ok_or_else(|| "turn/completed has no final turn.status".to_string())?;
    let event_id = stable_event_id(CODEX_TURN_COMPLETED, &[thread_id, turn_id]);
    let payload = json!({
        "schemaVersion": SCHEMA_VERSION,
        "threadId": thread_id,
        "turnId": turn_id,
        "status": status,
        "completedAt": turn.get("completedAt").cloned().unwrap_or(Value::Null),
        "durationMs": turn.get("durationMs").cloned().unwrap_or(Value::Null),
        "error": turn.get("error").cloned().unwrap_or(Value::Null),
        "occurredAt": occurred_at,
        "source": SOURCE,
        "sourceMethod": "turn/completed",
        "connectorVersion": env!("CARGO_PKG_VERSION"),
    });
    Ok(Some(DomainEvent {
        name: CODEX_TURN_COMPLETED,
        event_id,
        occurred_at: occurred_at.to_string(),
        payload,
    }))
}

fn normalize_thread_lifecycle(
    event_name: &'static str,
    method: &str,
    params: &Value,
    occurred_at: &str,
    stream_id: &str,
    sequence: u64,
) -> Result<Option<DomainEvent>, String> {
    let thread_id = required_string(params, "threadId", method)?;
    let event_id = stable_event_id(event_name, &[stream_id, &sequence.to_string(), thread_id]);
    let payload = json!({
        "schemaVersion": SCHEMA_VERSION,
        "threadId": thread_id,
        "occurredAt": occurred_at,
        "source": SOURCE,
        "sourceMethod": method,
        "connectorVersion": env!("CARGO_PKG_VERSION"),
    });
    Ok(Some(DomainEvent {
        name: event_name,
        event_id,
        occurred_at: occurred_at.to_string(),
        payload,
    }))
}

fn required_string<'a>(value: &'a Value, field: &str, method: &str) -> Result<&'a str, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{method} is missing string field {field}"))
}

fn stable_event_id(event_name: &str, identity: &[&str]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"baijimu-codex-domain-event-v1\0");
    digest.update(event_name.as_bytes());
    for part in identity {
        digest.update(b"\0");
        digest.update(part.as_bytes());
    }
    format!("codex_domain_v1_{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_completion_is_normalized_without_exposing_turn_items() {
        let params = json!({
            "threadId": "thr_123",
            "turn": {
                "id": "turn_456",
                "status": "failed",
                "completedAt": 100,
                "durationMs": 2500,
                "error": {"message": "provider failed"},
                "items": [{"type": "agentMessage", "text": "sensitive output"}]
            }
        });

        let event = normalize_codex_notification("turn/completed", &params, "101", "stream-a", 7)
            .unwrap()
            .unwrap();

        assert_eq!(event.name, CODEX_TURN_COMPLETED);
        assert_eq!(event.payload["schemaVersion"], 1);
        assert_eq!(event.payload["threadId"], "thr_123");
        assert_eq!(event.payload["turnId"], "turn_456");
        assert_eq!(event.payload["status"], "failed");
        assert_eq!(event.payload["error"]["message"], "provider failed");
        assert!(event.payload.get("items").is_none());
    }

    #[test]
    fn repeated_turn_completion_has_the_same_id_across_streams() {
        let params = json!({
            "threadId": "thr_123",
            "turn": {"id": "turn_456", "status": "completed"}
        });

        let first = normalize_codex_notification("turn/completed", &params, "101", "stream-a", 7)
            .unwrap()
            .unwrap();
        let replay = normalize_codex_notification("turn/completed", &params, "102", "stream-b", 1)
            .unwrap()
            .unwrap();

        assert_eq!(first.event_id, replay.event_id);
    }

    #[test]
    fn repeated_thread_closes_remain_distinct_lifecycle_occurrences() {
        let params = json!({"threadId": "thr_123"});
        let first = normalize_codex_notification("thread/closed", &params, "101", "stream-a", 7)
            .unwrap()
            .unwrap();
        let second = normalize_codex_notification("thread/closed", &params, "101", "stream-a", 8)
            .unwrap()
            .unwrap();

        assert_eq!(first.name, CODEX_THREAD_CLOSED);
        assert_ne!(first.event_id, second.event_id);
    }

    #[test]
    fn malformed_final_events_fail_closed() {
        let error = normalize_codex_notification(
            "turn/completed",
            &json!({
                "threadId": "thr_123",
                "turn": {"id": "turn_456", "status": "inProgress"}
            }),
            "101",
            "stream-a",
            7,
        )
        .unwrap_err();

        assert_eq!(error, "turn/completed has no final turn.status");
    }
}
