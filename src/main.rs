mod desktop_catalog;
mod desktop_events;
mod desktop_history;
mod desktop_invoke;
mod desktop_ipc;
use desktop_invoke::invoke_http;
use desktop_ipc::DesktopClient as CodexClient;
mod baijimu_cli;
mod cli;
mod json_compat;
mod process_runtime;
mod project_checkout;
mod project_preparation;
use process_runtime::*;
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Map, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 60_000;
const MANAGEMENT_TOKEN_FILE: &str = "management-token";
const CONNECTOR_HEALTH_IO_TIMEOUT: Duration = Duration::from_secs(1);
const CONNECTOR_HEALTH_MAX_RESPONSE_BYTES: u64 = 64 * 1024;
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
    management_token: String,
}
#[derive(Clone, Debug)]
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
    if let Err(e) = cli::run(env::args().skip(1).collect()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn start_server(options: ServerOptions) -> Result<(), String> {
    if !options
        .host
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
    {
        return Err("Connector HTTP 必须绑定本机 loopback 地址".into());
    }
    let listener =
        TcpListener::bind((options.host.as_str(), options.port)).map_err(|e| e.to_string())?;
    let management_token = load_or_create_management_token()?;
    fs::write(pid_path(), format!("{}\n", std::process::id())).map_err(|e| e.to_string())?;
    let state = Arc::new(AppState {
        client: CodexClient::new(options),
        management_operation: Mutex::new(()),
        management_token,
    });
    for stream in listener.incoming() {
        let state = state.clone();
        if let Ok(stream) = stream {
            thread::spawn(move || {
                let _ = handle_connection(stream, state);
            });
        }
    }
    Ok(())
}
fn handle_connection(mut stream: TcpStream, state: Arc<AppState>) -> Result<(), String> {
    let request = match read_http_request(&mut stream) {
        Ok(r) => r,
        Err(e) => {
            return write_json(
                &mut stream,
                400,
                &json!({"ok":false,"error":{"code":"INVALID_HTTP_REQUEST","message":e}}),
            )
        }
    };
    let path = request.path.split('?').next().unwrap_or("");
    let public = request.method == "GET" && matches!(path, "/healthz" | "/readyz");
    if !public && !management_authorized(request.authorization.as_deref(), &state.management_token)
    {
        return write_json(
            &mut stream,
            401,
            &json!({"ok":false,"error":{"code":"UNAUTHORIZED","message":"local app authorization required"}}),
        );
    }
    if public {
        return write_json(
            &mut stream,
            200,
            &json!({"ok":true,"status":{"connector":{"name":CONNECTOR_NAME,"version":VERSION,"pid":std::process::id()}}}),
        );
    }
    let response: Result<Value, HttpError> = (|| {
        if request.method == "POST" && path.starts_with("/invoke/") {
            let workspace = request.workspace_id.ok_or_else(|| {
                HttpError::coded(
                    400,
                    "本地应用调用缺少可信工作区上下文",
                    "WORKSPACE_CONTEXT_REQUIRED",
                    json!({}),
                )
            })?;
            return invoke_http(path, &request.body, workspace, &state);
        }
        let body = if request.body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&request.body).map_err(|e| HttpError::new(400, e.to_string()))?
        };
        match (request.method.as_str(), path) {
            ("GET", "/management/v1/setup/state")
            | ("POST", "/management/v1/setup/ensure-ready") => Ok(state.client.status()),
            ("POST", "/management/v1/projects/checkout") => {
                let _lock = state
                    .management_operation
                    .lock()
                    .map_err(|_| HttpError::internal("project lock poisoned"))?;
                let request = serde_json::from_value(body)
                    .map_err(|e| HttpError::new(400, format!("项目检出请求无效：{e}")))?;
                serde_json::to_value(
                    project_checkout::prepare(request)
                        .map_err(|e| HttpError::internal(e.to_string()))?,
                )
                .map_err(|e| HttpError::internal(e.to_string()))
            }
            _ => Err(HttpError::coded(
                404,
                "接口不受此 Connector 支持",
                "METHOD_NOT_SUPPORTED",
                json!({"path":path}),
            )),
        }
    })();
    match response {
        Ok(data) => write_json(&mut stream, 200, &json!({"ok":true,"data":data})),
        Err(e) => write_json(
            &mut stream,
            e.status,
            &json!({"ok":false,"error":{"code":e.code,"message":e.message,"data":e.data}}),
        ),
    }
}
const CONNECTOR_NAME: &str = "@baijimu/connector-codex";
const DEFAULT_LISTEN: &str = "stdio://";
fn configured_port() -> u16 {
    serde_json::from_str::<Value>(include_str!("../connector.json")).expect("valid manifest")
        ["configSchema"]["properties"]["port"]["default"]
        .as_u64()
        .expect("manifest port") as u16
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
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    let headers_end;
    loop {
        let n = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if n == 0 {
            return Err("connection closed".to_string());
        }
        buffer.extend_from_slice(&temp[..n]);
        if buffer.len() > 65536 {
            return Err("HTTP headers too large".into());
        }
        if let Some(end) = find_headers_end(&buffer) {
            headers_end = end;
            break;
        }
    }
    let content_length = parse_content_length(&buffer[..headers_end]).unwrap_or(0);
    if content_length > 8 * 1024 * 1024 {
        return Err("HTTP body too large".into());
    }
    let body_start = headers_end + 4;
    while buffer.len() < body_start + content_length {
        let n = stream.read(&mut temp).map_err(|error| error.to_string())?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..n]);
    }
    if buffer.len() != body_start + content_length {
        return Err("HTTP body length mismatch".into());
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
    println!("{} {}\nCommands: start [--host <loopback>] [--port <port>] [--daemon], status, stop, checkout-project, --version", CONNECTOR_NAME, VERSION);
}
