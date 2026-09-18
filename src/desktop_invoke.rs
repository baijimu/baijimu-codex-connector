use crate::{desktop_catalog, AppState, HttpError};
use serde_json::{json, Value};
fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str, HttpError> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| HttpError::new(400, format!("{key} is required")))
}
fn input(v: &Value) -> Result<Value, HttpError> {
    match v.get("input") {
        Some(Value::String(s)) if !s.is_empty() => {
            Ok(json!([{"type":"text","text":s,"text_elements":[]}]))
        }
        Some(Value::Array(a)) if !a.is_empty() => Ok(json!(a)),
        _ => Err(HttpError::new(400, "input is required")),
    }
}
pub(crate) fn invoke_http(
    path: &str,
    bytes: &[u8],
    workspace: u64,
    state: &AppState,
) -> Result<Value, HttpError> {
    let body: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(bytes).map_err(|e| HttpError::new(400, e.to_string()))?
    };
    let c = &state.client;
    let value = match path {
        "/invoke/status" => return Ok(c.status()),
        "/invoke/listThreads" | "/invoke/listSessions" | "/invoke/searchThreads" => {
            c.ensure_connected()?;
            desktop_catalog::list(&body)?
        }
        "/invoke/listProjects" => {
            c.ensure_connected()?;
            desktop_catalog::projects()?
        }
        "/invoke/readThread" | "/invoke/resumeThread" => {
            let mut read = c.read(required(&body, "threadId")?)?;
            if body["excludeTurns"] == true || body["includeTurns"] == false {
                if let Some(o) = read["thread"].as_object_mut() {
                    o.remove("turns");
                    o.remove("turnHistory");
                }
            }
            read
        }
        "/invoke/listThreadTurns" => {
            let read = c.read(required(&body, "threadId")?)?;
            let turns = turns(&read["thread"])?;
            let offset = body["cursor"]
                .as_str()
                .unwrap_or("0")
                .parse::<usize>()
                .map_err(|_| HttpError::new(400, "invalid cursor"))?;
            let limit = body["limit"].as_u64().unwrap_or(50).clamp(1, 100) as usize;
            let next = offset.saturating_add(limit);
            let more = next < turns.len();
            json!({"data":turns.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"nextCursor":more.then(||next.to_string()),"revision":read["revision"]})
        }
        "/invoke/startTurn" => {
            let id = required(&body, "threadId")?;
            let mut request = json!({"threadId":id,"input":input(&body)?});
            for k in ["model", "effort", "serviceTier", "summary"] {
                if let Some(v) = body.get(k) {
                    request[k] = v.clone();
                }
            }
            let result = c.follower(
                id,
                "thread-follower-start-turn",
                2,
                json!({"turnStart":{"request":request,"context":{"inheritThreadSettings":true}}}),
            )?;
            result.get("result").cloned().ok_or_else(|| {
                HttpError::coded(
                    502,
                    "desktop turn result missing",
                    "IPC_PROTOCOL_MISMATCH",
                    json!({}),
                )
            })?
        }
        "/invoke/steerTurn" => {
            // The desktop steering contract does not provide an atomic expected-turn guard.
            // Expose its actual contract; never pretend app-server's expectedTurnId is enforced.
            if body.get("turnId").is_some() {
                return Err(HttpError::new(
                    400,
                    "desktop steerTurn 不支持 turnId 条件；使用桌面当前轮次语义",
                ));
            }
            let id = required(&body, "threadId")?;
            c.follower(id,"thread-follower-steer-turn",1,json!({"input":input(&body)?,"restoreMessage":null,"attachments":[],"clientUserMessageId":crate::random_event_id()}))?
        }
        "/invoke/interruptTurn" => {
            let id = required(&body, "threadId")?;
            c.follower(
                id,
                "thread-follower-interrupt-turn",
                4,
                json!({"expectedTurnId":required(&body,"turnId")?,"mode":"user-stop"}),
            )?
        }
        "/invoke/pendingRequests" => {
            let read = c.read(required(&body, "threadId")?)?;
            json!({"requests":read["thread"]["requests"],"revision":read["revision"]})
        }
        "/invoke/respondToRequest" => {
            let id = required(&body, "threadId")?;
            let request_id = body
                .get("requestId")
                .filter(|v| v.is_string() || v.is_number())
                .ok_or_else(|| HttpError::new(400, "requestId required"))?;
            let read = c.read(id)?;
            let requests = read["thread"]["requests"]
                .as_array()
                .ok_or_else(|| HttpError::new(409, "no pending request"))?;
            let request = requests
                .iter()
                .find(|r| &r["id"] == request_id)
                .ok_or_else(|| HttpError::new(409, "request no longer pending"))?;
            let result = body
                .get("result")
                .ok_or_else(|| HttpError::new(400, "result required"))?;
            let (method, key) = match request["method"].as_str().unwrap_or("") {
                "item/commandExecution/requestApproval" => {
                    ("thread-follower-command-approval-decision", "decision")
                }
                "item/fileChange/requestApproval" => {
                    ("thread-follower-file-approval-decision", "decision")
                }
                "item/tool/requestUserInput" => ("thread-follower-submit-user-input", "response"),
                "item/permissions/requestApproval" => (
                    "thread-follower-permissions-request-approval-response",
                    "response",
                ),
                "mcpServer/elicitation/request" => (
                    "thread-follower-submit-mcp-server-elicitation-response",
                    "response",
                ),
                _ => {
                    return Err(HttpError::new(
                        409,
                        "unsupported desktop pending request type",
                    ))
                }
            };
            let mut params = json!({"requestId":request_id});
            params[key] = if key == "decision" {
                result
                    .get("decision")
                    .ok_or_else(|| HttpError::new(400, "decision required"))?
                    .clone()
            } else {
                result.clone()
            };
            c.follower(id, method, 1, params)?
        }
        "/invoke/recentEvents" => return Ok(c.recent_events(&body)),
        "/invoke/prepareProject" => {
            let _guard = state
                .management_operation
                .lock()
                .map_err(|_| HttpError::internal("project lock"))?;
            return serde_json::to_value(crate::project_preparation::prepare(
                workspace,
                serde_json::from_value(body).map_err(|e| HttpError::new(400, e.to_string()))?,
            )?)
            .map_err(|e| HttpError::internal(e.to_string()));
        }
        _ => {
            return Err(HttpError::coded(
                404,
                "接口不受桌面 IPC Connector 支持",
                "METHOD_NOT_SUPPORTED",
                json!({"path":path}),
            ))
        }
    };
    Ok(json!({"result":value}))
}
fn turns(state: &Value) -> Result<Vec<Value>, HttpError> {
    if state.pointer("/turnHistory/kind") == Some(&json!("canonical")) {
        let history = &state["turnHistory"]["history"];
        let islands = history["islands"]
            .as_array()
            .ok_or_else(|| HttpError::new(502, "invalid canonical turn history"))?;
        let mut out = vec![];
        for island in islands {
            for entry in island["entries"]
                .as_array()
                .ok_or_else(|| HttpError::new(502, "invalid turn island"))?
            {
                let key = entry["value"]
                    .as_str()
                    .ok_or_else(|| HttpError::new(502, "invalid turn key"))?;
                out.push(
                    history["entitiesByKey"]
                        .get(key)
                        .ok_or_else(|| HttpError::new(502, "missing canonical turn"))?
                        .clone(),
                );
            }
        }
        Ok(out)
    } else {
        state["turns"]
            .as_array()
            .cloned()
            .ok_or_else(|| HttpError::new(502, "missing desktop turns"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_history_is_not_mistaken_for_empty_legacy_turns() {
        let s = json!({"turns":[],"turnHistory":{"kind":"canonical","history":{"islands":[{"entries":[{"value":"x"}]}],"entitiesByKey":{"x":{"turnId":"t"}}}}});
        assert_eq!(turns(&s).unwrap()[0]["turnId"], "t");
    }
}
