mod event_store;

use crate::{child_process, codex_binary, timestamp, HttpError, ServerOptions, VERSION};
#[cfg(test)]
pub(crate) use event_store::retryable_event_status;
use event_store::EventStore;
use serde_json::{json, Value};
use std::collections::HashMap;
#[cfg(unix)]
use std::fs;
#[cfg(all(unix, not(target_os = "macos")))]
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
struct ClientRuntime {
    active_codex_home: PathBuf,
    session: Option<Arc<ProcessSession>>,
    last_exit: Option<Value>,
    codex_binary_error: Option<codex_binary::CommandError>,
    codex_cli_inspection: Option<codex_binary::CliInspection>,
}

pub(crate) struct CodexClient {
    pub(crate) options: ServerOptions,
    state_dir: PathBuf,
    runtime: Mutex<ClientRuntime>,
    lifecycle: Mutex<()>,
    events: Arc<EventStore>,
}

struct ProcessSession {
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    transport: RpcTransportKind,
    pending: Mutex<HashMap<u64, SyncSender<Result<Value, RpcFailure>>>>,
    next_id: AtomicU64,
    initialized: AtomicBool,
    alive: AtomicBool,
    pid: u32,
    started_at: String,
    exit: Mutex<Option<Value>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RpcTransportKind {
    JsonLines,
    #[cfg(unix)]
    WebSocketProxy,
}

impl RpcTransportKind {
    fn status_name(self) -> &'static str {
        match self {
            Self::JsonLines => "private_stdio",
            #[cfg(unix)]
            Self::WebSocketProxy => "shared_control_socket",
        }
    }

    fn listen_name(self, configured: &str) -> &str {
        match self {
            Self::JsonLines => configured,
            #[cfg(unix)]
            Self::WebSocketProxy => "unix://",
        }
    }

    fn is_shared(self) -> bool {
        #[cfg(unix)]
        {
            self == Self::WebSocketProxy
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

#[cfg(unix)]
const CONTROL_DIR_NAME: &str = "app-server-control";
#[cfg(unix)]
const CONTROL_SOCKET_NAME: &str = "app-server-control.sock";
#[cfg(all(unix, not(target_os = "macos")))]
const SHARED_START_LOCK_NAME: &str = "baijimu-connector-start.lock";
#[cfg(all(unix, not(target_os = "macos")))]
const SHARED_LOG_NAME: &str = "baijimu-connector-app-server.log";
#[cfg(all(unix, not(target_os = "macos")))]
const SHARED_START_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(unix)]
const PROXY_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
struct RpcFailure {
    message: String,
    code: Option<Value>,
    data: Option<Value>,
}

impl RpcFailure {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            data: None,
        }
    }

    fn into_http_error(self) -> HttpError {
        HttpError {
            status: 500,
            message: self.message,
            code: self.code,
            data: self.data,
        }
    }
}

impl CodexClient {
    pub(crate) fn new(options: ServerOptions) -> Self {
        let connector_home = crate::process_runtime::connector_home();
        Self::new_with_home(
            options,
            crate::process_runtime::system_codex_home(),
            connector_home,
        )
    }

    pub(crate) fn new_with_home(
        options: ServerOptions,
        codex_home: PathBuf,
        state_dir: PathBuf,
    ) -> Self {
        let mut runtime = ClientRuntime {
            active_codex_home: codex_home,
            session: None,
            last_exit: None,
            codex_binary_error: None,
            codex_cli_inspection: None,
        };
        refresh_codex_command_status(&mut runtime);
        Self {
            options,
            state_dir,
            runtime: Mutex::new(runtime),
            lifecycle: Mutex::new(()),
            events: Arc::new(EventStore::new()),
        }
    }

    pub(crate) fn status(&self) -> Value {
        let mut runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(session) = runtime.session.as_ref() {
            session.refresh_process_state();
            if !session.is_alive() {
                runtime.last_exit = session.exit();
            }
        }
        let session = runtime
            .session
            .as_ref()
            .filter(|session| session.is_alive());
        let binary_status = if let Some(inspection) = &runtime.codex_cli_inspection {
            inspection.status_value()
        } else if let Some(error) = &runtime.codex_binary_error {
            error.status_value()
        } else {
            json!({
                "mode": "path",
                "resolved": codex_binary::COMMAND,
                "source": "process_path",
                "checkedPaths": [],
                "version": null,
                "appServerSupported": null,
                "inspectionError": null,
                "error": null,
            })
        };
        let (latest_sequence, retained) = self.events.summary();
        json!({
            "connector": {
                "name": "@baijimu/connector-codex",
                "version": VERSION,
                "pid": std::process::id(),
            },
            "appServer": {
                "running": session.is_some(),
                "initialized": session.is_some_and(|session| session.initialized.load(Ordering::Acquire)),
                "pid": session.map(|session| session.pid),
                "transport": session.map(|session| session.transport.status_name()),
                "shared": session.is_some_and(|session| session.transport.is_shared()),
                "codexBinary": codex_binary::COMMAND,
                "codexBinaryResolution": binary_status,
                "listen": session
                    .map(|session| session.transport.listen_name(&self.options.listen))
                    .unwrap_or(&self.options.listen),
                "startedAt": session.map(|session| session.started_at.clone()),
                "lastExit": runtime.last_exit,
                "codexHome": runtime.active_codex_home,
            },
            "events": {
                "latestSequence": latest_sequence,
                "retained": retained,
            }
        })
    }

    pub(crate) fn request(
        &self,
        method: &str,
        params: Value,
        timeout_ms: Option<u64>,
    ) -> Result<Value, HttpError> {
        let session = self.ensure_session()?;
        session.request(
            method,
            params,
            timeout_ms.unwrap_or(self.options.request_timeout_ms),
        )
    }

    fn ensure_session(&self) -> Result<Arc<ProcessSession>, HttpError> {
        if let Some(session) = self.current_ready_session() {
            return Ok(session);
        }
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| HttpError::internal("Codex app-server 生命周期锁异常"))?;
        self.refresh_active_home_locked();
        if let Some(session) = self.current_ready_session() {
            return Ok(session);
        }
        let session = match self.current_live_session() {
            Some(session) => session,
            None => self.start_process_locked()?,
        };
        if !session.initialized.load(Ordering::Acquire) {
            session.request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "baijimu_connector_codex",
                        "title": "百积木 Codex 远程连接器",
                        "version": VERSION,
                    },
                    "capabilities": { "experimentalApi": true }
                }),
                30_000,
            )?;
            session.send_notification("initialized", json!({}))?;
            session.initialized.store(true, Ordering::Release);
        }
        Ok(session)
    }

    fn current_live_session(&self) -> Option<Arc<ProcessSession>> {
        let runtime = self.runtime.lock().ok()?;
        let session = runtime.session.as_ref()?.clone();
        session.refresh_process_state();
        session.is_alive().then_some(session)
    }

    fn current_ready_session(&self) -> Option<Arc<ProcessSession>> {
        self.current_live_session()
            .filter(|session| session.initialized.load(Ordering::Acquire))
    }

    fn refresh_active_home_locked(&self) {
        // The remote connector has one process-wide client bound to the device's
        // system Codex state. Platform workspace identity is authorization
        // context and never selects a different Codex home.
    }

    pub(crate) fn refresh_active_home(&self) {
        if let Ok(_lifecycle) = self.lifecycle.lock() {
            self.refresh_active_home_locked();
        }
    }

    pub(crate) fn active_codex_home(&self) -> PathBuf {
        self.runtime
            .lock()
            .map(|runtime| runtime.active_codex_home.clone())
            .unwrap_or_default()
    }

    pub(crate) fn state_dir(&self) -> &PathBuf {
        &self.state_dir
    }

    fn start_process_locked(&self) -> Result<Arc<ProcessSession>, HttpError> {
        let inspection = match codex_binary::inspect() {
            Ok(inspection) => inspection,
            Err(error) => {
                let message = error.to_string();
                let data = error.data_value();
                if let Ok(mut runtime) = self.runtime.lock() {
                    runtime.codex_binary_error = Some(error);
                    runtime.codex_cli_inspection = None;
                }
                return Err(HttpError::coded(
                    500,
                    message,
                    "CODEX_BINARY_NOT_FOUND",
                    data,
                ));
            }
        };
        if self.options.extra_args.is_empty() && !inspection.app_server_supported {
            let message = inspection.error.clone().unwrap_or_else(|| {
                "the selected Codex CLI does not support app-server".to_string()
            });
            if let Ok(mut runtime) = self.runtime.lock() {
                runtime.codex_cli_inspection = Some(inspection);
                runtime.codex_binary_error = None;
            }
            return Err(HttpError::coded(
                500,
                message.clone(),
                "CODEX_APP_SERVER_UNSUPPORTED",
                json!({"command": codex_binary::COMMAND, "error": message}),
            ));
        }
        #[cfg(unix)]
        if self.options.extra_args.is_empty() {
            return self.start_shared_process_locked(inspection);
        }
        self.start_private_process_locked(inspection)
    }

    fn start_private_process_locked(
        &self,
        inspection: codex_binary::CliInspection,
    ) -> Result<Arc<ProcessSession>, HttpError> {
        let active_home = self.active_codex_home();
        let args = if self.options.extra_args.is_empty() {
            vec![
                "app-server".to_string(),
                "--listen".to_string(),
                self.options.listen.clone(),
            ]
        } else {
            self.options.extra_args.clone()
        };
        let mut command = Command::new(codex_binary::COMMAND);
        child_process::isolate_from_connector_environment(&mut command);
        let mut child = command
            .args(args)
            .env("CODEX_HOME", active_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if let Ok(mut runtime) = self.runtime.lock() {
                    runtime.codex_binary_error = Some(codex_binary::CommandError {
                        reason: error.to_string(),
                    });
                    runtime.codex_cli_inspection = None;
                }
                HttpError::coded(
                    500,
                    format!(
                        "failed to start Codex command '{}': {error}",
                        codex_binary::COMMAND
                    ),
                    "CODEX_PROCESS_START_FAILED",
                    json!({"command": codex_binary::COMMAND, "error": error.to_string()}),
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            HttpError::internal("codex app-server stdin is unavailable after process start")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            HttpError::internal("codex app-server stdout is unavailable after process start")
        })?;
        let stderr = child.stderr.take();
        let session = Arc::new(ProcessSession {
            pid: child.id(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            transport: RpcTransportKind::JsonLines,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            initialized: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            started_at: timestamp(),
            exit: Mutex::new(None),
        });
        ProcessSession::spawn_stdout_reader(Arc::clone(&session), stdout, Arc::clone(&self.events));
        if let Some(stderr) = stderr {
            ProcessSession::spawn_stderr_reader(stderr);
        }
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.session = Some(Arc::clone(&session));
            runtime.last_exit = None;
            runtime.codex_cli_inspection = Some(inspection);
            runtime.codex_binary_error = None;
        }
        Ok(session)
    }

    #[cfg(unix)]
    fn start_shared_process_locked(
        &self,
        inspection: codex_binary::CliInspection,
    ) -> Result<Arc<ProcessSession>, HttpError> {
        let active_home = self.active_codex_home();
        #[cfg(target_os = "macos")]
        {
            start_managed_daemon(&active_home, &inspection)?;
            self.connect_shared_proxy(&active_home, &inspection)
        }
        #[cfg(not(target_os = "macos"))]
        {
            if let Ok(session) = self.connect_shared_proxy(&active_home, &inspection) {
                return Ok(session);
            }
            self.start_legacy_shared_process(&active_home, &inspection)
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn start_legacy_shared_process(
        &self,
        active_home: &std::path::Path,
        inspection: &codex_binary::CliInspection,
    ) -> Result<Arc<ProcessSession>, HttpError> {
        let control_dir = active_home.join(CONTROL_DIR_NAME);
        fs::create_dir_all(&control_dir).map_err(|error| {
            HttpError::coded(
                500,
                format!("创建 Codex control socket 目录失败：{error}"),
                "CODEX_CONTROL_DIR_FAILED",
                json!({"path": control_dir, "error": error.to_string()}),
            )
        })?;
        fs::set_permissions(&control_dir, fs::Permissions::from_mode(0o700)).map_err(|error| {
            HttpError::internal(format!("设置 Codex control socket 目录权限失败：{error}"))
        })?;
        let lock_path = control_dir.join(SHARED_START_LOCK_NAME);
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|error| {
                HttpError::internal(format!("打开共享 app-server 启动锁失败：{error}"))
            })?;
        lock_exclusive(&lock).map_err(|error| {
            HttpError::internal(format!("获取共享 app-server 启动锁失败：{error}"))
        })?;
        fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            HttpError::internal(format!("设置共享 app-server 启动锁权限失败：{error}"))
        })?;

        if let Ok(session) = self.connect_shared_proxy(active_home, inspection) {
            return Ok(session);
        }

        let socket_path = control_dir.join(CONTROL_SOCKET_NAME);
        remove_stale_control_socket(&socket_path)?;
        let log_path = control_dir.join(SHARED_LOG_NAME);
        let log = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(|error| {
                HttpError::internal(format!("打开共享 app-server 日志失败：{error}"))
            })?;
        fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            HttpError::internal(format!("设置共享 app-server 日志权限失败：{error}"))
        })?;
        let mut command = Command::new(codex_binary::COMMAND);
        child_process::isolate_from_connector_environment(&mut command);
        command
            .args(["app-server", "--listen", "unix://"])
            .env("CODEX_HOME", active_home)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|error| {
                HttpError::internal(format!("复制共享 app-server 日志句柄失败：{error}"))
            })?))
            .stderr(Stdio::from(log));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut backend = command.spawn().map_err(|error| {
            HttpError::coded(
                500,
                format!("启动共享 Codex app-server 失败：{error}"),
                "CODEX_SHARED_APP_SERVER_START_FAILED",
                json!({"command": codex_binary::COMMAND, "error": error.to_string(), "logPath": log_path}),
            )
        })?;
        let backend_pid = backend.id();
        wait_for_control_socket(&mut backend, &socket_path, &log_path)?;
        drop(backend);
        self.connect_shared_proxy(active_home, inspection).map_err(|error| {
            HttpError::coded(
                500,
                format!("共享 Codex app-server 已启动，但 proxy 连接失败：{}", error.message),
                "CODEX_SHARED_PROXY_CONNECT_FAILED",
                json!({"backendPid": backend_pid, "socketPath": socket_path, "logPath": log_path}),
            )
        })
    }

    #[cfg(unix)]
    fn connect_shared_proxy(
        &self,
        active_home: &std::path::Path,
        inspection: &codex_binary::CliInspection,
    ) -> Result<Arc<ProcessSession>, HttpError> {
        let mut command = Command::new(codex_binary::COMMAND);
        child_process::isolate_from_connector_environment(&mut command);
        let mut child = command
            .args(["app-server", "proxy"])
            .env("CODEX_HOME", active_home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                HttpError::coded(
                    500,
                    format!("启动 Codex app-server proxy 失败：{error}"),
                    "CODEX_PROXY_START_FAILED",
                    json!({"command": codex_binary::COMMAND, "error": error.to_string()}),
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HttpError::internal("codex app-server proxy stdin 启动后不可用"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HttpError::internal("codex app-server proxy stdout 启动后不可用"))?;
        let stderr = child.stderr.take();
        let (handshake_sender, handshake_receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut stdin = stdin;
            let result = crate::websocket_transport::client_handshake(&mut stdin, stdout)
                .map(|reader| (stdin, reader));
            let _ = handshake_sender.send(result);
        });
        let (stdin, reader) = match handshake_receiver.recv_timeout(PROXY_HANDSHAKE_TIMEOUT) {
            Ok(Ok(transport)) => transport,
            Ok(Err(error)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HttpError::coded(
                    500,
                    format!("连接 Codex control socket 失败：{error}"),
                    "CODEX_CONTROL_SOCKET_UNAVAILABLE",
                    json!({"codexHome": active_home, "error": error.to_string()}),
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HttpError::coded(
                    500,
                    "连接 Codex control socket 的 WebSocket 握手超时",
                    "CODEX_CONTROL_SOCKET_TIMEOUT",
                    json!({"codexHome": active_home, "timeoutSeconds": PROXY_HANDSHAKE_TIMEOUT.as_secs()}),
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HttpError::coded(
                    500,
                    "Codex control socket WebSocket 握手线程异常退出",
                    "CODEX_CONTROL_SOCKET_HANDSHAKE_FAILED",
                    json!({"codexHome": active_home}),
                ));
            }
        };
        let session = Arc::new(ProcessSession {
            pid: child.id(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            transport: RpcTransportKind::WebSocketProxy,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            initialized: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            started_at: timestamp(),
            exit: Mutex::new(None),
        });
        ProcessSession::spawn_websocket_reader(
            Arc::clone(&session),
            reader,
            Arc::clone(&self.events),
        );
        if let Some(stderr) = stderr {
            ProcessSession::spawn_stderr_reader(stderr);
        }
        if let Ok(mut runtime) = self.runtime.lock() {
            runtime.session = Some(Arc::clone(&session));
            runtime.last_exit = None;
            runtime.codex_cli_inspection = Some(inspection.clone());
            runtime.codex_binary_error = None;
        }
        Ok(session)
    }

    pub(crate) fn recent_events(&self, body: &Value) -> Value {
        self.events.recent(body)
    }

    pub(crate) fn record_event(&self, method: &str, params: Value) {
        self.events.push(method, params);
    }

    pub(crate) fn shutdown(&self) {
        if let Ok(_lifecycle) = self.lifecycle.lock() {
            self.shutdown_locked();
        }
    }

    pub(crate) fn shutdown_for_upgrade(&self) -> Result<(), HttpError> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| HttpError::internal("Codex app-server 生命周期锁异常"))?;
        self.shutdown_locked();
        #[cfg(target_os = "macos")]
        if self.options.extra_args.is_empty() {
            stop_shared_backend_for_upgrade(&self.active_codex_home())?;
        }
        Ok(())
    }

    fn shutdown_locked(&self) {
        let session = self
            .runtime
            .lock()
            .ok()
            .and_then(|mut runtime| runtime.session.take());
        if let Some(session) = session {
            session.shutdown();
            if let Ok(mut runtime) = self.runtime.lock() {
                runtime.last_exit = session.exit();
            }
        }
    }

    pub(crate) fn usable_cli_for_setup(
        &self,
        required_version: &semver::Version,
    ) -> Result<bool, HttpError> {
        match codex_binary::inspect() {
            Ok(inspection) => {
                let supported = if !self.options.extra_args.is_empty() {
                    true
                } else if !inspection.satisfies(required_version) {
                    false
                } else {
                    #[cfg(target_os = "macos")]
                    {
                        codex_binary::managed_install_ready(
                            &self.active_codex_home(),
                            required_version,
                        )
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        true
                    }
                };
                let mut runtime = self
                    .runtime
                    .lock()
                    .map_err(|_| HttpError::internal("Codex 客户端状态锁异常"))?;
                runtime.codex_binary_error = None;
                runtime.codex_cli_inspection = Some(inspection);
                Ok(supported)
            }
            Err(error) => {
                let mut runtime = self
                    .runtime
                    .lock()
                    .map_err(|_| HttpError::internal("Codex 客户端状态锁异常"))?;
                runtime.codex_binary_error = Some(error);
                runtime.codex_cli_inspection = None;
                Ok(false)
            }
        }
    }
}

impl ProcessSession {
    fn request(&self, method: &str, params: Value, timeout_ms: u64) -> Result<Value, HttpError> {
        if !self.is_alive() {
            return Err(HttpError::internal("codex app-server exited"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(1);
        self.pending
            .lock()
            .map_err(|_| HttpError::internal("Codex pending request table lock poisoned"))?
            .insert(id, sender);
        if let Err(error) =
            self.write_message(json!({"method": method, "id": id, "params": params}))
        {
            self.remove_pending(id);
            return Err(error);
        }
        match receiver.recv_timeout(Duration::from_millis(timeout_ms)) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(error.into_http_error()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.remove_pending(id);
                Err(HttpError::internal(format!(
                    "codex app-server request timed out: {method}"
                )))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(HttpError::internal(
                "codex app-server response dispatcher stopped",
            )),
        }
    }

    fn send_notification(&self, method: &str, params: Value) -> Result<(), HttpError> {
        self.write_message(json!({"method": method, "params": params}))
    }

    fn write_message(&self, message: Value) -> Result<(), HttpError> {
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| HttpError::internal("Codex app-server stdin lock poisoned"))?;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| HttpError::internal("codex app-server is not writable"))?;
        match self.transport {
            RpcTransportKind::JsonLines => writeln!(stdin, "{message}")
                .and_then(|_| stdin.flush())
                .map_err(|error| HttpError::internal(error.to_string())),
            #[cfg(unix)]
            RpcTransportKind::WebSocketProxy => {
                crate::websocket_transport::write_text(stdin, &message.to_string())
                    .map_err(|error| HttpError::internal(error.to_string()))
            }
        }
    }

    fn remove_pending(&self, id: u64) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&id);
        }
    }

    fn spawn_stdout_reader(
        session: Arc<Self>,
        stdout: std::process::ChildStdout,
        events: Arc<EventStore>,
    ) {
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        session.mark_exited(RpcFailure::transport("codex app-server exited"));
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        session.mark_exited(RpcFailure::transport(format!(
                            "failed to read codex app-server stdout: {error}"
                        )));
                        return;
                    }
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(trimmed) {
                    Ok(value) => value,
                    Err(error) => {
                        events.push(
                            "connector/parseError",
                            json!({"line": trimmed, "error": error.to_string()}),
                        );
                        continue;
                    }
                };
                session.dispatch_value(value, &events);
            }
        });
    }

    #[cfg(unix)]
    fn spawn_websocket_reader(
        session: Arc<Self>,
        mut reader: BufReader<std::process::ChildStdout>,
        events: Arc<EventStore>,
    ) {
        thread::spawn(move || loop {
            let incoming =
                crate::websocket_transport::read_message(&mut reader, |opcode, payload| {
                    let mut stdin = session.stdin.lock().map_err(|_| {
                        std::io::Error::other("Codex app-server stdin lock poisoned")
                    })?;
                    let stdin = stdin.as_mut().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "codex app-server proxy is not writable",
                        )
                    })?;
                    crate::websocket_transport::write_control(stdin, opcode, payload)
                });
            match incoming {
                Ok(crate::websocket_transport::IncomingMessage::Text(text)) => {
                    let value = match serde_json::from_str::<Value>(&text) {
                        Ok(value) => value,
                        Err(error) => {
                            events.push(
                                "connector/parseError",
                                json!({"line": text, "error": error.to_string()}),
                            );
                            continue;
                        }
                    };
                    session.dispatch_value(value, &events);
                }
                Ok(crate::websocket_transport::IncomingMessage::Closed) => {
                    session.mark_exited(RpcFailure::transport(
                        "codex app-server proxy WebSocket closed",
                    ));
                    return;
                }
                Err(error) => {
                    session.mark_exited(RpcFailure::transport(format!(
                        "failed to read codex app-server proxy WebSocket: {error}"
                    )));
                    return;
                }
            }
        });
    }

    fn dispatch_value(&self, value: Value, events: &EventStore) {
        if let Some(id) = value.get("id").and_then(Value::as_u64) {
            let sender = self
                .pending
                .lock()
                .ok()
                .and_then(|mut pending| pending.remove(&id));
            if let Some(sender) = sender {
                let result = if let Some(error) = value.get("error") {
                    Err(RpcFailure {
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("codex app-server error")
                            .to_string(),
                        code: error.get("code").cloned(),
                        data: error.get("data").cloned(),
                    })
                } else {
                    Ok(value.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = sender.send(result);
            } else {
                events.push("connector/unmatchedResponse", value);
            }
        } else {
            let method = value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("codex/notification")
                .to_string();
            let params = value.get("params").cloned().unwrap_or(value);
            events.push(&method, params);
        }
    }

    fn spawn_stderr_reader(stderr: std::process::ChildStderr) {
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                line.clear();
            }
        });
    }

    fn refresh_process_state(&self) {
        if !self.is_alive() {
            return;
        }
        let status = self.child.lock().ok().and_then(|mut child| {
            child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
        });
        if let Some(status) = status {
            self.set_exit(json!({
                "code": status.code(),
                "signal": null,
                "at": timestamp(),
            }));
            self.fail_pending(RpcFailure::transport("codex app-server exited"));
        }
    }

    fn shutdown(&self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            #[cfg(unix)]
            if self.transport == RpcTransportKind::WebSocketProxy {
                if let Some(writer) = stdin.as_mut() {
                    let _ = crate::websocket_transport::write_control(writer, 0x8, &[]);
                }
            }
            stdin.take();
        }
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let status = child.wait().ok();
                self.set_exit(json!({
                    "code": status.as_ref().and_then(|status| status.code()),
                    "signal": null,
                    "at": timestamp(),
                }));
            }
        }
        self.fail_pending(RpcFailure::transport("codex app-server was stopped"));
    }

    fn mark_exited(&self, failure: RpcFailure) {
        self.set_exit(json!({"error": failure.message, "at": timestamp()}));
        self.fail_pending(failure);
    }

    fn set_exit(&self, exit: Value) {
        if self.alive.swap(false, Ordering::AcqRel) {
            self.initialized.store(false, Ordering::Release);
            if let Ok(mut current) = self.exit.lock() {
                *current = Some(exit);
            }
        }
    }

    fn fail_pending(&self, failure: RpcFailure) {
        let pending = self
            .pending
            .lock()
            .map(|mut pending| {
                pending
                    .drain()
                    .map(|(_, sender)| sender)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for sender in pending {
            let _ = sender.send(Err(failure.clone()));
        }
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn exit(&self) -> Option<Value> {
        self.exit.lock().ok().and_then(|exit| exit.clone())
    }
}

fn refresh_codex_command_status(runtime: &mut ClientRuntime) {
    match codex_binary::inspect() {
        Ok(inspection) => {
            runtime.codex_cli_inspection = Some(inspection);
            runtime.codex_binary_error = None;
        }
        Err(error) => {
            runtime.codex_cli_inspection = None;
            runtime.codex_binary_error = Some(error);
        }
    }
}

#[cfg(target_os = "macos")]
fn run_daemon_command(codex_home: &std::path::Path, action: &str) -> Result<Value, HttpError> {
    let mut command = Command::new(codex_binary::COMMAND);
    child_process::isolate_from_connector_environment(&mut command);
    let output = command
        .args(["app-server", "daemon", action])
        .env("CODEX_HOME", codex_home)
        .output()
        .map_err(|error| {
            HttpError::coded(
                500,
                format!("执行 Codex app-server daemon {action} 失败：{error}"),
                "CODEX_DAEMON_COMMAND_FAILED",
                json!({"action": action, "codexHome": codex_home, "error": error.to_string()}),
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(HttpError::coded(
            500,
            format!(
                "Codex app-server daemon {action} 失败：{}",
                if stderr.is_empty() { &stdout } else { &stderr }
            ),
            "CODEX_DAEMON_COMMAND_FAILED",
            json!({
                "action": action,
                "codexHome": codex_home,
                "status": output.status.to_string(),
                "stdout": stdout,
                "stderr": stderr,
            }),
        ));
    }
    serde_json::from_str(&stdout).map_err(|error| {
        HttpError::coded(
            500,
            format!("Codex app-server daemon {action} 返回了无效 JSON：{error}"),
            "CODEX_DAEMON_RESPONSE_INVALID",
            json!({"action": action, "codexHome": codex_home, "stdout": stdout}),
        )
    })
}

#[cfg(target_os = "macos")]
fn start_managed_daemon(
    codex_home: &std::path::Path,
    inspection: &codex_binary::CliInspection,
) -> Result<(), HttpError> {
    let expected = inspection
        .semantic_version()
        .map(|value| value.to_string())
        .ok_or_else(|| HttpError::internal("Codex CLI 版本无法用于校验 app-server 守护进程"))?;
    let initial_start = run_daemon_command(codex_home, "start");
    let initial_version = initial_start
        .as_ref()
        .ok()
        .and_then(|_| run_daemon_command(codex_home, "version").ok());
    let version = if initial_version
        .as_ref()
        .is_some_and(|value| daemon_version_matches(value, &expected))
    {
        initial_version.expect("checked as present")
    } else {
        stop_shared_backend_for_upgrade(codex_home)?;
        run_daemon_command(codex_home, "start")?;
        run_daemon_command(codex_home, "version")?
    };
    if !daemon_version_matches(&version, &expected) {
        return Err(HttpError::coded(
            503,
            "Codex CLI 已更新，但共享 app-server 尚未切换到同一版本",
            "CODEX_DAEMON_VERSION_MISMATCH",
            json!({"expectedVersion": expected, "daemon": version}),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn daemon_version_matches(version: &Value, expected: &str) -> bool {
    let cli_version = version.get("cliVersion").and_then(Value::as_str);
    let app_server_version = version.get("appServerVersion").and_then(Value::as_str);
    let managed_path = version.get("managedCodexPath").and_then(Value::as_str);
    let running = version.get("status").and_then(Value::as_str) == Some("running");
    running
        && managed_path.is_some()
        && cli_version == Some(expected)
        && app_server_version == Some(expected)
}

#[cfg(target_os = "macos")]
fn stop_shared_backend_for_upgrade(codex_home: &std::path::Path) -> Result<(), HttpError> {
    let socket_path = codex_home.join(CONTROL_DIR_NAME).join(CONTROL_SOCKET_NAME);
    if run_daemon_command(codex_home, "stop").is_ok() {
        wait_for_socket_shutdown(&socket_path)?;
        return Ok(());
    }
    let stream = match UnixStream::connect(&socket_path) {
        Ok(stream) => stream,
        Err(_) => {
            remove_stale_control_socket(&socket_path)?;
            return Ok(());
        }
    };
    let pid = unix_peer_pid(&stream).map_err(|error| {
        HttpError::coded(
            500,
            format!("无法确认遗留 Codex app-server 的 socket 对端：{error}"),
            "CODEX_LEGACY_DAEMON_IDENTITY_FAILED",
            json!({"socketPath": socket_path, "error": error.to_string()}),
        )
    })?;
    validate_legacy_backend_process(pid)?;
    terminate_verified_process(pid)?;
    wait_for_socket_shutdown(&socket_path)
}

#[cfg(target_os = "macos")]
fn wait_for_socket_shutdown(socket_path: &std::path::Path) -> Result<(), HttpError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if UnixStream::connect(socket_path).is_err() {
            return remove_stale_control_socket(socket_path);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(HttpError::coded(
        500,
        "Codex app-server 在升级前未按时停止",
        "CODEX_DAEMON_STOP_TIMEOUT",
        json!({"socketPath": socket_path}),
    ))
}

#[cfg(target_os = "macos")]
fn unix_peer_pid(stream: &UnixStream) -> std::io::Result<u32> {
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERPID: libc::c_int = 0x002;
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if status == -1 {
        return Err(std::io::Error::last_os_error());
    }
    u32::try_from(pid).map_err(|_| std::io::Error::other("socket 对端 PID 无效"))
}

#[cfg(target_os = "macos")]
fn validate_legacy_backend_process(pid: u32) -> Result<(), HttpError> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "uid=", "-o", "command="])
        .output()
        .map_err(|error| HttpError::internal(format!("检查遗留 app-server 进程失败：{error}")))?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let expected_uid = unsafe { libc::geteuid() };
    if !output.status.success() || !legacy_backend_identity_matches(&line, expected_uid) {
        return Err(HttpError::coded(
            500,
            "control socket 对端不是当前用户的 Codex app-server，拒绝终止",
            "CODEX_LEGACY_DAEMON_IDENTITY_FAILED",
            json!({"pid": pid, "expectedUid": expected_uid, "command": line}),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn legacy_backend_identity_matches(line: &str, expected_uid: u32) -> bool {
    let mut fields = line.split_whitespace();
    let uid = fields.next().and_then(|value| value.parse::<u32>().ok());
    let command = fields.collect::<Vec<_>>();
    let executable_is_codex = command
        .first()
        .and_then(|value| std::path::Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "codex" || value.starts_with("codex-"));
    let is_app_server = command.iter().any(|value| *value == "app-server");
    let listens_on_control_socket = command
        .windows(2)
        .any(|pair| pair == ["--listen", "unix://"]);
    uid == Some(expected_uid) && executable_is_codex && is_app_server && listens_on_control_socket
}

#[cfg(target_os = "macos")]
fn terminate_verified_process(pid: u32) -> Result<(), HttpError> {
    let pid = i32::try_from(pid).map_err(|_| HttpError::internal("遗留 app-server PID 无效"))?;
    if unsafe { libc::kill(pid, libc::SIGTERM) } == -1 {
        return Err(HttpError::internal(format!(
            "停止遗留 app-server 失败：{}",
            std::io::Error::last_os_error()
        )));
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    if unsafe { libc::kill(pid, libc::SIGKILL) } == -1 {
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(HttpError::internal(format!(
            "强制停止遗留 app-server 失败：{}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn lock_exclusive(file: &File) -> std::io::Result<()> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn remove_stale_control_socket(socket_path: &std::path::Path) -> Result<(), HttpError> {
    use std::os::unix::fs::FileTypeExt;

    if !socket_path.exists() {
        return Ok(());
    }
    for _ in 0..20 {
        if UnixStream::connect(socket_path).is_ok() {
            return Err(HttpError::coded(
                500,
                "Codex control socket 正在监听，但 proxy 无法完成 WebSocket 协商",
                "CODEX_CONTROL_SOCKET_PROTOCOL_FAILED",
                json!({"socketPath": socket_path}),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
    let metadata = fs::symlink_metadata(socket_path).map_err(|error| {
        HttpError::internal(format!("读取 Codex control socket 状态失败：{error}"))
    })?;
    if !metadata.file_type().is_socket() {
        return Err(HttpError::coded(
            500,
            "Codex control socket 路径被非 socket 文件占用",
            "CODEX_CONTROL_SOCKET_PATH_OCCUPIED",
            json!({"socketPath": socket_path}),
        ));
    }
    fs::remove_file(socket_path).map_err(|error| {
        HttpError::internal(format!("清理失效的 Codex control socket 失败：{error}"))
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn wait_for_control_socket(
    backend: &mut Child,
    socket_path: &std::path::Path,
    log_path: &std::path::Path,
) -> Result<(), HttpError> {
    let deadline = std::time::Instant::now() + SHARED_START_TIMEOUT;
    loop {
        if UnixStream::connect(socket_path).is_ok() {
            return Ok(());
        }
        if let Some(status) = backend.try_wait().map_err(|error| {
            HttpError::internal(format!("检查共享 app-server 状态失败：{error}"))
        })? {
            return Err(HttpError::coded(
                500,
                format!("共享 Codex app-server 在创建 control socket 前退出：{status}"),
                "CODEX_SHARED_APP_SERVER_EXITED",
                json!({
                    "status": status.to_string(),
                    "socketPath": socket_path,
                    "logPath": log_path,
                    "logTail": read_log_tail(log_path),
                }),
            ));
        }
        if std::time::Instant::now() >= deadline {
            let _ = backend.kill();
            let _ = backend.wait();
            return Err(HttpError::coded(
                500,
                "共享 Codex app-server 未在 10 秒内创建 control socket",
                "CODEX_SHARED_APP_SERVER_TIMEOUT",
                json!({
                    "socketPath": socket_path,
                    "logPath": log_path,
                    "logTail": read_log_tail(log_path),
                }),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_log_tail(path: &std::path::Path) -> String {
    const LIMIT: usize = 8 * 1024;
    fs::read(path)
        .map(|bytes| {
            let start = bytes.len().saturating_sub(LIMIT);
            String::from_utf8_lossy(&bytes[start..]).to_string()
        })
        .unwrap_or_default()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{daemon_version_matches, legacy_backend_identity_matches};
    use serde_json::json;

    #[test]
    fn managed_daemon_requires_cli_and_app_server_to_match() {
        let matching = json!({
            "status": "running",
            "managedCodexPath": "/tmp/current/codex",
            "cliVersion": "0.152.0",
            "appServerVersion": "0.152.0",
        });
        assert!(daemon_version_matches(&matching, "0.152.0"));

        let stale = json!({
            "status": "running",
            "managedCodexPath": "/tmp/current/codex",
            "cliVersion": "0.152.0",
            "appServerVersion": "0.151.0",
        });
        assert!(!daemon_version_matches(&stale, "0.152.0"));
    }

    #[test]
    fn legacy_backend_identity_requires_exact_user_command_and_control_listener() {
        assert!(legacy_backend_identity_matches(
            "501 /Users/c/.local/bin/codex app-server --listen unix://",
            501,
        ));
        assert!(legacy_backend_identity_matches(
            "501 codex app-server --listen unix://",
            501,
        ));
        assert!(!legacy_backend_identity_matches(
            "502 codex app-server --listen unix://",
            501,
        ));
        assert!(!legacy_backend_identity_matches(
            "501 codex app-server --listen stdio://",
            501,
        ));
        assert!(!legacy_backend_identity_matches(
            "501 unrelated app-server --listen unix://",
            501,
        ));
    }
}
