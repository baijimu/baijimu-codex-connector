mod app_server;
mod baijimu_cli;
mod child_process;
mod cli;
mod codex_binary;
mod events;
mod invoke;
mod json_compat;
mod process_runtime;
mod project_checkout;
mod setup;
mod thread_state;

#[cfg(test)]
use app_server::retryable_event_status;
use app_server::CodexClient;
use cli::run;
use invoke::handle_invoke;
#[cfg(test)]
use invoke::{read_json_file, resolve_state_project_references};
use process_runtime::*;
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Map, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 18111;
const DEFAULT_LISTEN: &str = "stdio://";
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 120_000;
const MAX_EVENTS: usize = 1000;
const DEFAULT_PROJECT_LIMIT: usize = 100;
const DEFAULT_PROJECT_THREAD_PAGE_LIMIT: usize = 100;
const DEFAULT_PROJECT_THREAD_MAX_PAGES: usize = 100;
const DEFAULT_THREAD_SORT_KEY: &str = "updated_at";
const DEFAULT_THREAD_SORT_DIRECTION: &str = "desc";
const MANAGEMENT_TOKEN_FILE: &str = "management-token";
const CONNECTOR_HEALTH_IO_TIMEOUT: Duration = Duration::from_secs(1);
const CONNECTOR_HEALTH_MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const DOMAIN_EVENT_PUBLISH_ATTEMPTS: usize = 5;
const DOMAIN_EVENT_RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
struct ServerOptions {
    host: String,
    port: u16,
    listen: String,
    extra_args: Vec<String>,
    request_timeout_ms: u64,
    daemon: bool,
}

struct AppState {
    client: CodexClient,
    management_operation: Mutex<()>,
    runtime_operation: RwLock<()>,
    setup: setup::SetupManager,
    management_token: String,
    startup: StartupReadiness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum StartupPhase {
    Initializing,
    Ready,
    Failed,
}

#[derive(Clone, Debug)]
struct StartupSnapshot {
    phase: StartupPhase,
    message: String,
    error: Option<String>,
    started_at: String,
    completed_at: Option<String>,
}

#[derive(Clone)]
struct StartupReadiness {
    inner: Arc<Mutex<StartupSnapshot>>,
}

impl StartupReadiness {
    fn initializing() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StartupSnapshot {
                phase: StartupPhase::Initializing,
                message: "正在初始化 Codex 远程连接器本机环境".to_string(),
                error: None,
                started_at: timestamp(),
                completed_at: None,
            })),
        }
    }

    fn snapshot(&self) -> StartupSnapshot {
        self.inner
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_else(|_| StartupSnapshot {
                phase: StartupPhase::Failed,
                message: "Codex 远程连接器初始化状态不可用".to_string(),
                error: Some("startup readiness lock poisoned".to_string()),
                started_at: timestamp(),
                completed_at: Some(timestamp()),
            })
    }

    fn ready(&self) {
        if let Ok(mut snapshot) = self.inner.lock() {
            snapshot.phase = StartupPhase::Ready;
            snapshot.message = "Codex 远程连接器已就绪".to_string();
            snapshot.error = None;
            snapshot.completed_at = Some(timestamp());
        }
    }

    fn fail(&self, error: String) {
        if let Ok(mut snapshot) = self.inner.lock() {
            snapshot.phase = StartupPhase::Failed;
            snapshot.message = "Codex 远程连接器初始化失败".to_string();
            snapshot.error = Some(error);
            snapshot.completed_at = Some(timestamp());
        }
    }
}

impl StartupSnapshot {
    fn is_ready(&self) -> bool {
        self.phase == StartupPhase::Ready
    }

    fn status_name(&self) -> &'static str {
        match self.phase {
            StartupPhase::Initializing => "initializing",
            StartupPhase::Ready => "ready",
            StartupPhase::Failed => "failed",
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "status": self.status_name(),
            "message": self.message,
            "error": self.error,
            "startedAt": self.started_at,
            "completedAt": self.completed_at,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupReadinessDecision {
    Ready,
    Start(u64),
    Initializing,
    Failed,
    NeedsWorkspace,
}

fn decide_setup_readiness(
    setup: &setup::SetupStatus,
    current_workspace_id: Option<u64>,
    current_workspace_credential_available: bool,
    cli_ready: bool,
) -> SetupReadinessDecision {
    if cli_ready {
        return SetupReadinessDecision::Ready;
    }
    if setup.status == "running" {
        return SetupReadinessDecision::Initializing;
    }
    let Some(workspace_id) =
        current_workspace_id.filter(|_| current_workspace_credential_available)
    else {
        return SetupReadinessDecision::NeedsWorkspace;
    };
    if setup.status == "failed" && setup.workspace_id == Some(workspace_id) {
        return SetupReadinessDecision::Failed;
    }
    if setup.status == "needs_retry"
        || (setup.status == "interrupted" && setup.automatic_retry_count <= 1)
    {
        return SetupReadinessDecision::Start(workspace_id);
    }
    if setup.status == "interrupted" {
        return SetupReadinessDecision::Failed;
    }
    SetupReadinessDecision::Start(workspace_id)
}

#[derive(Debug)]
struct HttpError {
    status: u16,
    message: String,
    code: Option<Value>,
    data: Option<Value>,
}

impl HttpError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }

    fn coded(
        status: u16,
        message: impl Into<String>,
        code: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            status,
            message: message.into(),
            code: Some(Value::String(code.into())),
            data: Some(data),
        }
    }
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = run(args);
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn start_server(options: ServerOptions) -> Result<(), String> {
    let listener = TcpListener::bind((options.host.as_str(), options.port))
        .map_err(|error| error.to_string())?;
    let management_token = load_or_create_management_token()
        .map_err(|error| format!("failed to initialize management token: {error}"))?;
    fs::write(pid_path(), format!("{}\n", std::process::id()))
        .map_err(|error| format!("failed to record connector process id: {error}"))?;
    println!(
        "{}",
        json!({"ok": true, "url": format!("http://{}:{}", options.host, options.port), "pid": std::process::id()})
    );
    let setup = setup::SetupManager::load();
    let state = Arc::new(AppState {
        client: CodexClient::new(options.clone()),
        management_operation: Mutex::new(()),
        runtime_operation: RwLock::new(()),
        setup,
        management_token,
        startup: StartupReadiness::initializing(),
    });
    let initializing_state = Arc::clone(&state);
    thread::spawn(move || match initialize_connector(&initializing_state) {
        Ok(()) => initializing_state.startup.ready(),
        Err(error) => {
            eprintln!("Codex 远程连接器初始化失败：{error}");
            initializing_state.startup.fail(error);
        }
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    let _ = handle_connection(stream, state);
                });
            }
            Err(error) => eprintln!("accept failed: {error}"),
        }
    }
    Ok(())
}

fn initialize_connector(state: &AppState) -> Result<(), String> {
    initialize_server()?;
    ensure_codex_ready(state)
        .map(|_| ())
        .map_err(|error| error.message)
}

fn initialize_server() -> Result<(), String> {
    if test_control_enabled() {
        if let Ok(delay_ms) = env::var("CODEX_CONNECTOR_TEST_STARTUP_DELAY_MS") {
            let delay_ms = delay_ms
                .parse::<u64>()
                .map_err(|error| format!("invalid test startup delay: {error}"))?;
            thread::sleep(Duration::from_millis(delay_ms.min(30_000)));
        }
        if let Ok(error) = env::var("CODEX_CONNECTOR_TEST_STARTUP_FAILURE") {
            if !error.trim().is_empty() {
                return Err(error);
            }
        }
    }
    Ok(())
}

fn test_control_enabled() -> bool {
    env::var("CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN")
        .ok()
        .as_deref()
        == Some("1")
}

fn connector_identity() -> Value {
    json!({
        "name": "@baijimu/connector-codex",
        "version": VERSION,
        "pid": std::process::id(),
    })
}

fn startup_response(snapshot: &StartupSnapshot) -> Value {
    json!({
        "ok": snapshot.is_ready(),
        "status": {
            "connector": connector_identity(),
            "startup": snapshot.to_value(),
        },
        "error": (!snapshot.is_ready()).then(|| json!({
            "code": if snapshot.phase == StartupPhase::Failed {
                "connector_initialization_failed"
            } else {
                "connector_initializing"
            },
            "message": snapshot.error.as_deref().unwrap_or(&snapshot.message),
        })),
    })
}

fn handle_connection(mut stream: TcpStream, state: Arc<AppState>) -> Result<(), String> {
    let request = read_http_request(&mut stream)?;
    let path = request
        .path
        .split('?')
        .next()
        .unwrap_or(request.path.as_str())
        .to_string();
    let response = match (request.method.as_str(), path.as_str()) {
        ("GET", "/healthz") => {
            let snapshot = state.startup.snapshot();
            let mut response = startup_response(&snapshot);
            response["ok"] = Value::Bool(true);
            (200, response)
        }
        ("GET", "/readyz") => {
            let snapshot = state.startup.snapshot();
            let status = if snapshot.is_ready() { 200 } else { 503 };
            (status, startup_response(&snapshot))
        }
        ("POST", "/__shutdown")
            if env::var("CODEX_CONNECTOR_ENABLE_TEST_SHUTDOWN")
                .ok()
                .as_deref()
                == Some("1") =>
        {
            state.client.shutdown();
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(20));
                std::process::exit(0);
            });
            (200, json!({"ok": true}))
        }
        ("POST", path) if path.starts_with("/invoke/") => {
            if let Some(response) = startup_not_ready_response(&state.startup) {
                return write_json(&mut stream, 503, &response);
            }
            let body = if request.body.is_empty() {
                json!({})
            } else {
                serde_json::from_slice(&request.body).map_err(|error| error.to_string())?
            };
            let Some(workspace_id) = request.workspace_id else {
                return write_json(
                    &mut stream,
                    400,
                    &json!({
                        "ok": false,
                        "error": {
                            "code": "WORKSPACE_CONTEXT_REQUIRED",
                            "message": "本地应用调用缺少可信工作区上下文"
                        }
                    }),
                );
            };
            let _runtime_guard = match ensure_cli_available(state.as_ref(), workspace_id) {
                Ok(guard) => guard,
                Err(error) => {
                    return write_json(
                        &mut stream,
                        error.status,
                        &json!({
                            "ok": false,
                            "error": {
                                "message": error.message,
                                "code": error.code,
                                "data": error.data,
                            }
                        }),
                    );
                }
            };
            match handle_invoke(path, &body, &state.client) {
                Ok(data) => (200, json!({"ok": true, "data": data})),
                Err(error) => (
                    error.status,
                    json!({
                        "ok": false,
                        "error": {
                            "message": error.message,
                            "code": error.code,
                            "data": error.data,
                        }
                    }),
                ),
            }
        }
        (method, path) if path.starts_with("/management/") => {
            if let Some(response) = startup_not_ready_response(&state.startup) {
                return write_json(&mut stream, 503, &response);
            }
            if !management_authorized(request.authorization.as_deref(), &state.management_token) {
                (
                    401,
                    json!({"ok": false, "error": {"message": "management authorization required"}}),
                )
            } else {
                let body = if request.body.is_empty() {
                    json!({})
                } else {
                    serde_json::from_slice(&request.body).map_err(|error| error.to_string())?
                };
                match handle_management(method, path, &body, &state) {
                    Ok(data) => (200, json!({"ok": true, "data": data})),
                    Err(error) => (
                        error.status,
                        json!({"ok": false, "error": {"message": error.message, "code": error.code, "data": error.data}}),
                    ),
                }
            }
        }
        _ => (404, json!({"ok": false, "error": {"message": "not found"}})),
    };
    write_json(&mut stream, response.0, &response.1)
}

fn startup_not_ready_response(startup: &StartupReadiness) -> Option<Value> {
    let snapshot = startup.snapshot();
    (!snapshot.is_ready()).then(|| startup_response(&snapshot))
}

fn handle_management(
    method: &str,
    path: &str,
    body: &Value,
    state: &AppState,
) -> Result<Value, HttpError> {
    match (method, path) {
        ("GET", "/management/v1/setup/state") => serde_json::to_value(state.setup.state())
            .map_err(|error| HttpError::internal(error.to_string())),
        ("POST", "/management/v1/setup/ensure-ready") => ensure_codex_ready(state),
        ("POST", "/management/v1/setup/retry") => {
            let _operation_guard = state
                .management_operation
                .lock()
                .map_err(|_| HttpError::internal("凭证管理状态锁异常"))?;
            let workspace_id = body
                .get("workspaceId")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0)
                .ok_or_else(|| HttpError::new(400, "必须提供 workspaceId"))?;
            let _runtime_guard = state
                .runtime_operation
                .write()
                .map_err(|_| HttpError::internal("Codex 运行时状态锁异常"))?;
            state.client.shutdown();
            let requirement = state.setup.cli_requirement();
            let codex_cli = state.client.usable_cli_for_setup(requirement.target())?;
            let verify_app_server_capability = state.client.options.extra_args.is_empty();
            serde_json::to_value(
                state
                    .setup
                    .start(
                        workspace_id,
                        codex_cli,
                        true,
                        verify_app_server_capability,
                        requirement.target().clone(),
                    )
                    .map_err(|error| HttpError::new(409, error.to_string()))?,
            )
            .map_err(|error| HttpError::internal(error.to_string()))
        }
        ("POST", "/management/v1/projects/checkout") => {
            let _project_guard = state
                .management_operation
                .lock()
                .map_err(|_| HttpError::internal("项目检出状态锁异常"))?;
            let request: project_checkout::CheckoutRequest =
                serde_json::from_value(body.clone())
                    .map_err(|error| HttpError::new(400, format!("项目检出请求无效：{error}")))?;
            let result = project_checkout::prepare(request)
                .map_err(|error| HttpError::internal(error.to_string()))?;
            serde_json::to_value(result).map_err(|error| HttpError::internal(error.to_string()))
        }
        _ => Err(HttpError::new(404, format!("未知的管理接口路径：{path}"))),
    }
}

fn setup_readiness_value(
    readiness: &str,
    message: impl Into<String>,
    setup: setup::SetupStatus,
) -> Value {
    json!({
        "readiness": readiness,
        "message": message.into(),
        "setup": setup,
    })
}

fn ensure_cli_available(
    state: &AppState,
    workspace_id: u64,
) -> Result<RwLockReadGuard<'_, ()>, HttpError> {
    loop {
        let runtime_guard = state
            .runtime_operation
            .read()
            .map_err(|_| HttpError::internal("Codex 运行时状态锁异常"))?;
        let requirement = state.setup.cli_requirement();
        if state.client.usable_cli_for_setup(requirement.target())? {
            return Ok(runtime_guard);
        }
        drop(runtime_guard);

        let update_guard = state
            .runtime_operation
            .write()
            .map_err(|_| HttpError::internal("Codex 运行时状态锁异常"))?;
        let requirement = state.setup.cli_requirement();
        if state.client.usable_cli_for_setup(requirement.target())? {
            drop(update_guard);
            continue;
        }
        let status = state.setup.state();
        if status.status == "failed" && status.workspace_id == Some(workspace_id) {
            return Err(HttpError::coded(
                503,
                status
                    .error
                    .clone()
                    .unwrap_or_else(|| "Codex CLI 初始化失败".to_string()),
                "CODEX_CLI_SETUP_FAILED",
                json!({
                    "workspaceId": workspace_id,
                    "cliRequirement": requirement.status_value(),
                    "setup": status
                }),
            ));
        }
        state.client.shutdown();
        state
            .setup
            .start(
                workspace_id,
                false,
                false,
                state.client.options.extra_args.is_empty(),
                requirement.target().clone(),
            )
            .map_err(|error| HttpError::new(409, error.to_string()))?;
        return Err(HttpError::coded(
            503,
            "Codex CLI 正在同步到兼容版本并验证，请稍后重试",
            "CODEX_CLI_INITIALIZING",
            json!({
                "workspaceId": workspace_id,
                "cliRequirement": requirement.status_value(),
                "setup": state.setup.state()
            }),
        ));
    }
}

fn ensure_codex_ready(state: &AppState) -> Result<Value, HttpError> {
    let _operation_guard = state
        .management_operation
        .lock()
        .map_err(|_| HttpError::internal("凭证管理状态锁异常"))?;
    let _runtime_guard = state
        .runtime_operation
        .write()
        .map_err(|_| HttpError::internal("Codex 运行时状态锁异常"))?;
    let setup_status = state.setup.state();
    let client = &state.client;
    let requirement = state.setup.cli_requirement();
    let codex_cli_available = client.usable_cli_for_setup(requirement.target())?;
    let verify_app_server_capability = client.options.extra_args.is_empty();
    if codex_cli_available {
        return Ok(setup_readiness_value(
            "ready",
            "系统默认 Codex CLI 与 app-server 能力已就绪",
            setup_status,
        ));
    }
    let auth = baijimu_cli::auth_status()
        .map_err(|error| HttpError::new(409, format!("读取安装授权上下文失败：{error}")))?;
    let current_workspace_id = auth.current_workspace_id;
    let current_workspace_credential_available = current_workspace_id
        .is_some_and(|workspace_id| auth.credential_workspace_ids.contains(&workspace_id));
    match decide_setup_readiness(
        &setup_status,
        current_workspace_id,
        current_workspace_credential_available,
        codex_cli_available,
    ) {
        SetupReadinessDecision::Ready => Ok(setup_readiness_value(
            "ready",
            "Codex CLI 与 app-server 能力已就绪",
            setup_status,
        )),
        SetupReadinessDecision::Start(workspace_id) => {
            client.shutdown();
            let setup_status = state
                .setup
                .start(
                    workspace_id,
                    codex_cli_available,
                    false,
                    verify_app_server_capability,
                    requirement.target().clone(),
                )
                .map_err(|error| HttpError::new(409, error.to_string()))?;
            Ok(setup_readiness_value(
                "initializing",
                "正在自动安装并验证 Codex CLI",
                setup_status,
            ))
        }
        SetupReadinessDecision::Initializing => Ok(setup_readiness_value(
            "initializing",
            "正在自动安装并验证 Codex CLI",
            setup_status,
        )),
        SetupReadinessDecision::Failed => Ok(setup_readiness_value(
            "failed",
            setup_status
                .error
                .clone()
                .unwrap_or_else(|| "Codex 初始化失败，请检查失败步骤后重试".to_string()),
            setup_status,
        )),
        SetupReadinessDecision::NeedsWorkspace => Ok(setup_readiness_value(
            "needs_workspace",
            "当前百积木账号没有明确且已授权的工作区，请先完成工作区授权",
            setup_status,
        )),
    }
}

fn management_authorized(header: Option<&str>, expected: &str) -> bool {
    let provided = header
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .as_bytes();
    let expected = expected.as_bytes();
    if provided.len() != expected.len() {
        return false;
    }
    provided
        .iter()
        .zip(expected)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    let headers_end;
    loop {
        let n = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if n == 0 {
            return Err("connection closed".to_string());
        }
        buffer.extend_from_slice(&temp[..n]);
        if let Some(end) = find_headers_end(&buffer) {
            headers_end = end;
            break;
        }
    }
    let content_length = parse_content_length(&buffer[..headers_end]).unwrap_or(0);
    let body_start = headers_end + 4;
    while buffer.len() < body_start + content_length {
        let n = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..n]);
    }
    let header_text = String::from_utf8_lossy(&buffer[..headers_end]);
    let request_line = header_text.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let header = |expected: &str| {
        header_text.lines().skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(expected)
                .then(|| value.trim().to_string())
        })
    };
    let authorization = header("authorization");
    let workspace_id = header("x-baijimu-workspace-id")
        .map(|value| value.parse::<u64>())
        .transpose()
        .map_err(|_| "x-baijimu-workspace-id must be a positive integer".to_string())?
        .filter(|value| *value > 0);
    Ok(HttpRequest {
        method: parts.next().unwrap_or_default().to_string(),
        path: parts.next().unwrap_or_default().to_string(),
        authorization,
        workspace_id,
        body: buffer[body_start..].to_vec(),
    })
}

struct HttpRequest {
    method: String,
    path: String,
    authorization: Option<String>,
    workspace_id: Option<u64>,
    body: Vec<u8>,
}

fn write_json(stream: &mut TcpStream, status: u16, payload: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(payload).map_err(|error| error.to_string())?;
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        410 => "Gone",
        503 => "Service Unavailable",
        500 => "Internal Server Error",
        _ => "OK",
    };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|error| error.to_string())
}

fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(headers);
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                return value.trim().parse().ok();
            }
        }
    }
    None
}

fn print_help() {
    println!(
        "baijimu-connector-codex {VERSION}\n\nUsage:\n  baijimu-connector-codex start [--host 127.0.0.1] [--port 18111] [--listen stdio://] [--daemon]\n  baijimu-connector-codex status\n  baijimu-connector-codex stop\n  baijimu-connector-codex checkout-project --workspace-id <id> --project-id <id> [--branch <name>]\n  baijimu-connector-codex --version"
    );
}

#[cfg(test)]
mod project_state_tests {
    use super::*;

    #[test]
    fn event_delivery_retries_only_temporary_failures() {
        for status in [408, 429, 500, 503] {
            assert!(retryable_event_status(status), "status {status}");
        }
        for status in [400, 401, 403, 404, 409, 422] {
            assert!(!retryable_event_status(status), "status {status}");
        }
    }

    #[test]
    fn startup_readiness_separates_liveness_from_initialization() {
        let startup = StartupReadiness::initializing();
        let initializing = startup.snapshot();
        assert_eq!(initializing.phase, StartupPhase::Initializing);
        assert!(!initializing.is_ready());
        assert_eq!(
            startup_response(&initializing)["error"]["code"],
            "connector_initializing"
        );

        startup.ready();
        let ready = startup.snapshot();
        assert_eq!(ready.phase, StartupPhase::Ready);
        assert!(ready.is_ready());
        assert_eq!(startup_response(&ready)["ok"], true);
    }

    #[test]
    fn startup_readiness_preserves_the_initialization_root_cause() {
        let startup = StartupReadiness::initializing();
        startup.fail("Connector 元数据初始化超时".to_string());

        let failed = startup.snapshot();
        let response = startup_response(&failed);
        assert_eq!(failed.phase, StartupPhase::Failed);
        assert_eq!(response["error"]["code"], "connector_initialization_failed");
        assert_eq!(response["error"]["message"], "Connector 元数据初始化超时");
    }

    #[test]
    fn reads_global_state_json_with_utf8_bom() {
        let path = env::temp_dir().join(format!(
            "baijimu-codex-global-state-bom-{}",
            std::process::id()
        ));
        fs::write(&path, "\u{feff}{\"projects\":[\"one\"]}").unwrap();

        assert_eq!(read_json_file(&path), json!({"projects": ["one"]}));
        fs::remove_file(path).unwrap();
    }

    fn setup_status(status: &str, workspace_id: Option<u64>) -> setup::SetupStatus {
        setup::SetupStatus {
            status: status.to_string(),
            workspace_id,
            message: status.to_string(),
            error: (status == "failed").then(|| "installer failed".to_string()),
            retryable: matches!(status, "failed" | "interrupted" | "needs_retry"),
            ..setup::SetupStatus::default()
        }
    }

    #[test]
    fn automatic_setup_readiness_covers_install_repair_and_manual_retry_states() {
        assert_eq!(
            decide_setup_readiness(&setup_status("pending", None), Some(642), true, false),
            SetupReadinessDecision::Start(642)
        );
        assert_eq!(
            decide_setup_readiness(&setup_status("succeeded", Some(642)), Some(642), true, true,),
            SetupReadinessDecision::Ready
        );
        assert_eq!(
            decide_setup_readiness(
                &setup_status("succeeded", Some(642)),
                Some(642),
                true,
                false,
            ),
            SetupReadinessDecision::Start(642)
        );
        assert_eq!(
            decide_setup_readiness(&setup_status("running", Some(642)), Some(642), true, false,),
            SetupReadinessDecision::Initializing
        );
        assert_eq!(
            decide_setup_readiness(&setup_status("failed", Some(642)), Some(642), true, false,),
            SetupReadinessDecision::Failed
        );
        assert_eq!(
            decide_setup_readiness(&setup_status("failed", Some(100)), Some(642), true, false,),
            SetupReadinessDecision::Start(642)
        );
        assert_eq!(
            decide_setup_readiness(
                &setup_status("interrupted", Some(642)),
                Some(642),
                true,
                false,
            ),
            SetupReadinessDecision::Start(642)
        );
        assert_eq!(
            decide_setup_readiness(
                &setup_status("needs_retry", Some(642)),
                Some(642),
                true,
                false,
            ),
            SetupReadinessDecision::Start(642)
        );
        let mut repeated_interruption = setup_status("interrupted", Some(642));
        repeated_interruption.automatic_retry_count = 2;
        assert_eq!(
            decide_setup_readiness(&repeated_interruption, Some(642), true, false),
            SetupReadinessDecision::Failed
        );
        assert_eq!(
            decide_setup_readiness(&setup_status("pending", None), Some(642), false, false),
            SetupReadinessDecision::NeedsWorkspace
        );
    }

    fn health_options(port: u16) -> ServerOptions {
        ServerOptions {
            host: DEFAULT_HOST.to_string(),
            port,
            listen: DEFAULT_LISTEN.to_string(),
            extra_args: Vec::new(),
            request_timeout_ms: DEFAULT_REQUEST_TIMEOUT_MS,
            daemon: false,
        }
    }

    #[test]
    fn connector_health_accepts_a_bounded_healthy_response() {
        let listener = TcpListener::bind((DEFAULT_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            assert!(stream.read(&mut request).unwrap() > 0);
            let body = r#"{"ok":true,"status":{"connector":{"pid":7}}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let health = connector_health(&health_options(port)).unwrap();

        server.join().unwrap();
        assert_eq!(health["ok"], true);
        assert_eq!(health.pointer("/status/connector/pid"), Some(&json!(7)));
    }

    #[test]
    fn connector_health_read_has_a_hard_timeout() {
        let listener = TcpListener::bind((DEFAULT_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(3));
        });
        let started_at = Instant::now();

        let error = connector_health(&health_options(port)).unwrap_err();

        assert!(!error.is_empty());
        assert!(
            started_at.elapsed() < Duration::from_secs(2),
            "health probe waited beyond its configured I/O timeout"
        );
    }

    #[test]
    fn connector_health_rejects_an_oversized_response() {
        let listener = TcpListener::bind((DEFAULT_HOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            assert!(stream.read(&mut request).unwrap() > 0);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n")
                .unwrap();
            stream
                .write_all(&vec![
                    b'x';
                    CONNECTOR_HEALTH_MAX_RESPONSE_BYTES as usize + 1
                ])
                .unwrap();
        });

        let error = connector_health(&health_options(port)).unwrap_err();

        server.join().unwrap();
        assert!(error.contains("response exceeds"), "{error}");
    }

    #[test]
    fn connector_stop_pid_requires_the_codex_health_identity() {
        let valid = json!({
            "status": {
                "connector": {
                    "name": "@baijimu/connector-codex",
                    "pid": 42
                }
            }
        });
        assert_eq!(verified_connector_pid(&valid).unwrap(), 42);

        let unrelated = json!({
            "status": {
                "connector": {
                    "name": "another-service",
                    "pid": 42
                }
            }
        });
        assert!(verified_connector_pid(&unrelated)
            .unwrap_err()
            .contains("does not belong"));
    }

    #[test]
    fn resolves_current_project_ids_and_keeps_legacy_paths() {
        let local_root = env::temp_dir().join("codex-current-project");
        let assigned_root = env::temp_dir().join("codex-assigned-project");
        let legacy_root = env::temp_dir().join("codex-legacy-project");
        let state = json!({
            "local-projects": {
                "local-current": {
                    "id": "local-current",
                    "name": "Current Project",
                    "rootPaths": [local_root]
                }
            },
            "thread-project-assignments": {
                "thread-1": {
                    "projectId": "remote-project-id",
                    "cwd": assigned_root
                }
            }
        });

        let resolved = resolve_state_project_references(
            &state,
            vec![
                "local-current".to_string(),
                "remote-project-id".to_string(),
                "local-unresolved".to_string(),
                legacy_root.display().to_string(),
            ],
        );

        assert_eq!(resolved.len(), 3);
        assert_eq!(resolved[0].path, local_root.display().to_string());
        assert_eq!(resolved[0].project_id.as_deref(), Some("local-current"));
        assert_eq!(resolved[0].project_name.as_deref(), Some("Current Project"));
        assert_eq!(resolved[1].path, assigned_root.display().to_string());
        assert_eq!(resolved[1].project_id.as_deref(), Some("remote-project-id"));
        assert_eq!(resolved[2].path, legacy_root.display().to_string());
        assert_eq!(resolved[2].project_id, None);
    }
}
