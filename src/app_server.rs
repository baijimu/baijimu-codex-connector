mod event_store;

use crate::{child_process, codex_binary, timestamp, HttpError, ServerOptions, VERSION};
#[cfg(test)]
pub(crate) use event_store::retryable_event_status;
use event_store::EventStore;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
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
    pending: Mutex<HashMap<u64, SyncSender<Result<Value, RpcFailure>>>>,
    next_id: AtomicU64,
    initialized: AtomicBool,
    alive: AtomicBool,
    pid: u32,
    started_at: String,
    exit: Mutex<Option<Value>>,
}

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
                "codexBinary": codex_binary::COMMAND,
                "codexBinaryResolution": binary_status,
                "listen": self.options.listen,
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

    pub(crate) fn usable_cli_for_setup(&self) -> Result<bool, HttpError> {
        match codex_binary::inspect() {
            Ok(inspection) => {
                let supported =
                    !self.options.extra_args.is_empty() || inspection.app_server_supported;
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
        writeln!(stdin, "{message}").map_err(|error| HttpError::internal(error.to_string()))?;
        stdin
            .flush()
            .map_err(|error| HttpError::internal(error.to_string()))
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
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let sender = session
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
        });
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
