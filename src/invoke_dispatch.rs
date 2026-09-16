use crate::{AppState, HttpError};
use serde_json::{json, Value};

pub(crate) fn invoke_http(
    path: &str,
    bytes: &[u8],
    workspace: u64,
    state: &AppState,
) -> Result<Value, HttpError> {
    if path == "/invoke/prepareProject" {
        let request = serde_json::from_slice(bytes)
            .map_err(|error| HttpError::new(400, format!("项目准备请求无效: {error}")))?;
        let _guard = state
            .management_operation
            .lock()
            .map_err(|_| HttpError::internal("项目准备状态锁异常"))?;
        let prepared = crate::project_preparation::prepare(workspace, request)?;
        return serde_json::to_value(prepared)
            .map_err(|error| HttpError::internal(error.to_string()));
    }
    let body = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(bytes).map_err(|error| HttpError::new(400, error.to_string()))?
    };
    invoke_with_state(path, &body, state)
}

pub(crate) fn invoke_with_state(
    path: &str,
    body: &Value,
    state: &AppState,
) -> Result<Value, HttpError> {
    // Diagnostics remain available even while explicit installation holds the
    // runtime lock. They neither resolve the release catalog nor launch Codex.
    if path == "/invoke/status" {
        let mut status = state.client.status();
        status["setup"] = serde_json::to_value(state.setup.state())
            .map_err(|error| HttpError::internal(error.to_string()))?;
        return Ok(status);
    }
    let _runtime_guard = state
        .runtime_operation
        .read()
        .map_err(|_| HttpError::internal("Codex 运行时状态锁异常"))?;
    // The client reuses its ready session and inspects CLI compatibility only
    // when it actually needs to start an app-server. Installation is management-only.
    crate::invoke::handle_invoke(path, body, &state.client)
}
