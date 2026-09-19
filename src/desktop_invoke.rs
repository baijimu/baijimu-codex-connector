use crate::{desktop_catalog, desktop_history, AppState, HttpError};
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
        Some(Value::Array(a)) if !a.is_empty() => {
            let mut items = a.clone();
            for (index, item) in items.iter_mut().enumerate() {
                let item = item.as_object_mut().ok_or_else(|| {
                    HttpError::new(400, format!("input[{index}] must be an object"))
                })?;
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    if !item.get("text").is_some_and(Value::is_string) {
                        return Err(HttpError::new(
                            400,
                            format!("input[{index}].text must be a string"),
                        ));
                    }
                    // Desktop renders the original IPC input before Core defaults it.
                    // Preserve supplied annotations; only an absent field means empty.
                    let elements = item.entry("text_elements").or_insert_with(|| json!([]));
                    if !elements.is_array() {
                        return Err(HttpError::new(
                            400,
                            format!("input[{index}].text_elements must be an array"),
                        ));
                    }
                }
            }
            Ok(Value::Array(items))
        }
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
    validate_request(path, &body)?;
    let c = &state.client;
    let value = match path {
        "/invoke/status" => return Ok(c.status()),
        "/invoke/listThreads" | "/invoke/listSessions" | "/invoke/searchThreads" => {
            c.ensure_connected()?;
            desktop_catalog::list(&body)?
        }
        "/invoke/listProjects" => {
            c.ensure_connected()?;
            desktop_catalog::projects(&body)?
        }
        "/invoke/readThread" | "/invoke/resumeThread" => {
            let thread = required(&body, "threadId")?;
            if body["excludeTurns"] == true || body["includeTurns"] == false {
                c.read_metadata(thread)?
            } else {
                c.read(thread)?
            }
        }
        "/invoke/listThreadTurns" | "/invoke/listThreadItems" | "/invoke/readThreadItem" => {
            c.ensure_connected()?;
            desktop_history::invoke(path.trim_start_matches("/invoke/"), &body)?
        }
        "/invoke/startTurn" => {
            let id = required(&body, "threadId")?;
            let mut request = json!({"threadId":id,"input":input(&body)?});
            for k in ["model", "cwd"] {
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
            let read = c.read_metadata(required(&body, "threadId")?)?;
            json!({"requests":read["thread"]["requests"],"revision":read["revision"]})
        }
        "/invoke/respondToRequest" => {
            let id = required(&body, "threadId")?;
            let request_id = body
                .get("requestId")
                .filter(|v| v.is_string() || v.is_number())
                .ok_or_else(|| HttpError::new(400, "requestId required"))?;
            let read = c.read_metadata(id)?;
            let requests = read["thread"]["requests"]
                .as_array()
                .ok_or_else(|| HttpError::new(409, "no pending request"))?;
            let request = requests
                .iter()
                .find(|r| &r["id"] == request_id)
                .ok_or_else(|| HttpError::new(409, "request no longer pending"))?;
            require_request_turn(request, required(&body, "turnId")?)?;
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
            c.follower_for_owner(id, method, 1, params, required(&read, "ownerClientId")?)?
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
fn validate_request(path: &str, body: &Value) -> Result<(), HttpError> {
    let manifest: Value =
        serde_json::from_str(include_str!("../connector.json")).expect("valid manifest");
    let Some(method) = manifest["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == path)
    else {
        return Ok(());
    };
    let obj = body
        .as_object()
        .ok_or_else(|| HttpError::new(400, "request must be an object"))?;
    let schema = &method["input_schema"];
    // prepareProject owns its tagged-union validation in its typed deserializer.
    if schema.get("oneOf").is_some() {
        return Ok(());
    }
    if let Some(required) = schema["required"].as_array() {
        for key in required {
            let key = key.as_str().unwrap();
            if !obj.contains_key(key) {
                return Err(HttpError::new(400, format!("{key} is required")));
            }
        }
    }
    for (key, value) in obj {
        if schema["properties"].get(key).is_none() {
            return Err(HttpError::coded(
                400,
                format!("Connector 不支持参数 {key}"),
                "UNSUPPORTED_PARAMETER",
                json!({"parameter":key}),
            ));
        }
        let field = &schema["properties"][key];
        let matches_type = |kind: &str| match kind {
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "null" => value.is_null(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            _ => false,
        };
        let valid_type = match &field["type"] {
            Value::String(kind) => matches_type(kind),
            Value::Array(kinds) => kinds.iter().filter_map(Value::as_str).any(matches_type),
            Value::Null => true,
            _ => false,
        };
        let valid_enum = field["enum"]
            .as_array()
            .is_none_or(|items| items.contains(value));
        let in_range = value.as_f64().is_none_or(|n| {
            field["minimum"].as_f64().is_none_or(|min| n >= min)
                && field["maximum"].as_f64().is_none_or(|max| n <= max)
        });
        if !valid_type || !valid_enum || !in_range {
            return Err(HttpError::new(400, format!("invalid {key}")));
        }
    }
    Ok(())
}
fn require_request_turn(request: &Value, turn: &str) -> Result<(), HttpError> {
    if request.pointer("/params/turnId").and_then(Value::as_str) != Some(turn) {
        return Err(HttpError::coded(
            409,
            "待处理请求不属于指定轮次",
            "PENDING_REQUEST_TURN_MISMATCH",
            json!({}),
        ));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn text_input_is_complete_for_desktop_rendering() {
        let expected = json!([{"type":"text","text":"确认","text_elements":[]}]);
        assert_eq!(super::input(&json!({"input":"确认"})).unwrap(), expected);
        assert_eq!(
            super::input(&json!({"input":[{"type":"text","text":"确认"}]})).unwrap(),
            expected
        );
    }

    #[test]
    fn input_preserves_annotations_and_other_content() {
        let items = json!([
            {"type":"text","text":"文件","text_elements":[{"byteRange":{"start":0,"end":6},"placeholder":"文件"}]},
            {"type":"image","url":"https://example.test/image.png"},
            {"type":"text","text":"后续","text_elements":[]}
        ]);
        assert_eq!(super::input(&json!({"input":items})).unwrap(), items);
    }

    #[test]
    fn invalid_text_input_is_rejected_before_ipc() {
        for items in [
            json!([null]),
            json!([{"type":"text"}]),
            json!([{"type":"text","text":42}]),
            json!([{"type":"text","text":"确认","text_elements":null}]),
            json!([{"type":"text","text":"确认","text_elements":{}}]),
            json!([{"type":"text","text":"确认","text_elements":"[]"}]),
        ] {
            assert!(super::input(&json!({"input":items})).is_err());
        }
    }

    #[test]
    fn approval_requires_matching_request_turn() {
        let r = serde_json::json!({"params":{"turnId":"turn-a"}});
        assert!(super::require_request_turn(&r, "turn-a").is_ok());
        assert_eq!(
            super::require_request_turn(&r, "turn-b").unwrap_err().code,
            Some(serde_json::json!("PENDING_REQUEST_TURN_MISMATCH"))
        );
        assert!(super::require_request_turn(&serde_json::json!({}), "turn-a").is_err());
    }
    #[test]
    fn unsupported_legacy_options_are_rejected() {
        for (path, body) in [
            (
                "/invoke/listProjects",
                serde_json::json!({"includeSaved":true}),
            ),
            (
                "/invoke/listThreadTurns",
                serde_json::json!({"threadId":"a","unknownHistoryOption":true}),
            ),
            (
                "/invoke/startTurn",
                serde_json::json!({"threadId":"a","input":"test","effort":"high"}),
            ),
        ] {
            assert_eq!(
                super::validate_request(path, &body).unwrap_err().code,
                Some(serde_json::json!("UNSUPPORTED_PARAMETER"))
            );
        }
        assert!(super::validate_request(
            "/invoke/listProjects",
            &serde_json::json!({"archived":"false"})
        )
        .is_err());
        assert!(
            super::validate_request("/invoke/listProjects", &serde_json::json!({"limit":0}))
                .is_err()
        );
        assert!(super::validate_request(
            "/invoke/interruptTurn",
            &serde_json::json!({"threadId":"a"})
        )
        .is_err());
    }
}
