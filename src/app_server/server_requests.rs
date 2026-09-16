//! Connection-local JSON-RPC server requests. These are pending protocol frames,
//! not durable session state or a platform authorization grant.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_PENDING_REQUESTS: usize = 128;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub(crate) enum RequestId {
    Number(i64),
    Text(String),
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServerRequest {
    request_id: RequestId,
    method: String,
    pub(super) thread_id: String,
    pub(super) turn_id: String,
    params: Value,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Reply {
    pub thread_id: String,
    pub turn_id: String,
    pub request_id: RequestId,
    pub result: ReplyResult,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum ReplyResult {
    Decision(DecisionReply),
    Answers(AnswersReply),
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionReply {
    decision: Decision,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum Decision {
    Accept,
    Decline,
    Cancel,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnswersReply {
    answers: BTreeMap<String, Answer>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    answers: Vec<String>,
}

impl ProcessSession {
    pub(super) fn receive_server_request(&self, value: Value, events: &EventStore) {
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let request = (|| {
            let request_id: RequestId = serde_json::from_value(id.clone()).ok()?;
            if !matches!(
                method,
                "item/commandExecution/requestApproval"
                    | "item/fileChange/requestApproval"
                    | "item/tool/requestUserInput"
            ) {
                return None;
            }
            Some(ServerRequest {
                request_id,
                method: method.into(),
                thread_id: params.get("threadId")?.as_str()?.to_owned(),
                turn_id: params.get("turnId")?.as_str()?.to_owned(),
                params,
            })
        })();
        let Some(request) = request else {
            let _ = self.write_message(json!({"id":id,"error":{"code":-32601,"message":"Unsupported connector server request"}}));
            return;
        };
        let Ok(mut pending) = self.server_requests.lock() else {
            return;
        };
        if pending.len() >= MAX_PENDING_REQUESTS || pending.contains_key(&id.to_string()) {
            let _ = self.write_message(json!({"id":id,"error":{"code":-32000,"message":"Server request capacity or identity conflict"}}));
            return;
        }
        pending.insert(id.to_string(), request.clone());
        drop(pending);
        events.push(
            "connector/serverRequest",
            serde_json::to_value(request).expect("serializable request"),
        );
    }

    fn respond(&self, reply: Reply) -> Result<Value, HttpError> {
        let id = serde_json::to_value(&reply.request_id)
            .map_err(|error| HttpError::internal(error.to_string()))?;
        let mut pending = self
            .server_requests
            .lock()
            .map_err(|_| HttpError::internal("server request lock poisoned"))?;
        let request = pending
            .get(&id.to_string())
            .ok_or_else(|| HttpError::new(409, "request no longer pending"))?;
        validate_reply(request, &reply)?;
        // Remove before write; uncertain writes are never replayed automatically.
        pending.remove(&id.to_string());
        self.write_message(json!({"id":id,"result":reply.result}))?;
        Ok(json!({"accepted":true}))
    }
}

fn validate_reply(request: &ServerRequest, reply: &Reply) -> Result<(), HttpError> {
    if reply.thread_id != request.thread_id
        || reply.turn_id != request.turn_id
        || reply.request_id != request.request_id
    {
        return Err(HttpError::new(
            403,
            "server request belongs to another thread or turn",
        ));
    }
    match (&reply.result, request.method.as_str()) {
        (
            ReplyResult::Decision(_),
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval",
        ) => Ok(()),
        (ReplyResult::Answers(answers), "item/tool/requestUserInput") => {
            let questions = request
                .params
                .get("questions")
                .and_then(Value::as_array)
                .ok_or_else(|| HttpError::new(400, "invalid pending questions"))?;
            if answers.answers.len() != questions.len()
                || questions.iter().any(|q| {
                    q.get("id")
                        .and_then(Value::as_str)
                        .is_none_or(|id| !answers.answers.contains_key(id))
                })
            {
                return Err(HttpError::new(
                    400,
                    "answers do not match pending questions",
                ));
            }
            Ok(())
        }
        _ => Err(HttpError::new(
            400,
            "response does not match server request method",
        )),
    }
}

impl CodexClient {
    pub(crate) fn respond_to_server_request(&self, reply: Reply) -> Result<Value, HttpError> {
        self.current_ready_session()
            .ok_or_else(|| HttpError::new(409, "Codex connection no longer active"))?
            .respond(reply)
    }
    pub(crate) fn pending_server_requests(&self, thread: &str) -> Result<Value, HttpError> {
        let Some(session) = self.current_ready_session() else {
            return Ok(json!([]));
        };
        let pending = session
            .server_requests
            .lock()
            .map_err(|_| HttpError::internal("server request lock poisoned"))?;
        Ok(json!(pending
            .values()
            .filter(|request| request.thread_id == thread)
            .collect::<Vec<_>>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bidirectional_rpc_ids_do_not_consume_each_others_requests() {
        let session = ProcessSession {
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            transport: RpcTransportKind::JsonLines,
            pending: Mutex::new(HashMap::new()),
            server_requests: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            initialized: AtomicBool::new(true),
            alive: AtomicBool::new(true),
            pid: 0,
            started_at: "test".into(),
            exit: Mutex::new(None),
        };
        let events = EventStore::in_memory();
        let (sender, receiver) = mpsc::sync_channel(1);
        session.pending.lock().unwrap().insert(7, sender);
        session.dispatch_value(json!({"id":7,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread","turnId":"turn","command":"cargo test"}}), &events);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(session.pending.lock().unwrap().contains_key(&7));
        assert_eq!(session.server_requests.lock().unwrap().len(), 1);
        let delivered = events.recent(&json!({}));
        assert_eq!(delivered["events"][0]["method"], "connector/serverRequest");
        assert_eq!(delivered["events"][0]["params"]["requestId"], 7);
        session.dispatch_value(json!({"id":7,"result":{"turn":{"id":"turn"}}}), &events);
        assert_eq!(receiver.try_recv().unwrap().unwrap()["turn"]["id"], "turn");
        assert_eq!(session.server_requests.lock().unwrap().len(), 1);
        session.dispatch_value(
            json!({"method":"serverRequest/resolved","params":{"requestId":7,"threadId":"other"}}),
            &events,
        );
        assert_eq!(session.server_requests.lock().unwrap().len(), 1);
        session.dispatch_value(
            json!({"method":"serverRequest/resolved","params":{"requestId":7,"threadId":"thread"}}),
            &events,
        );
        assert!(session.server_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn replies_are_scoped_to_both_thread_and_turn_and_do_not_grant_future_approval() {
        let request = ServerRequest {
            request_id: RequestId::Number(1),
            method: "item/commandExecution/requestApproval".into(),
            thread_id: "thread".into(),
            turn_id: "turn".into(),
            params: json!({}),
        };
        let body = json!({"requestId":1,"threadId":"thread","turnId":"turn","result":{"decision":"accept"}});
        let mut reply: Reply = serde_json::from_value(body.clone()).unwrap();
        assert!(validate_reply(&request, &reply).is_ok());
        reply.thread_id = "other".into();
        assert!(validate_reply(&request, &reply).is_err());
        let mut changed = body;
        changed["result"]["decision"] = json!("acceptForSession");
        assert!(serde_json::from_value::<Reply>(changed).is_err());
    }
}
