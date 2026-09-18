//! Desktop coordination IPC only. This module never starts a Codex process.
use crate::{random_event_id, HttpError, ServerOptions};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc as async_mpsc,
};
const MAX_FRAME: usize = 32 * 1024 * 1024;
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
type Pending = mpsc::SyncSender<Result<Value, HttpError>>;
pub(crate) struct DesktopClient {
    pub(crate) options: ServerOptions,
    runtime: tokio::runtime::Runtime,
    session: Mutex<Option<Arc<Session>>>,
    endpoint: PathBuf,
}
struct Session {
    id: String,
    alive: AtomicBool,
    outgoing: async_mpsc::Sender<Value>,
    pending: Mutex<HashMap<String, Pending>>,
    snapshots: Mutex<HashMap<String, (u64, Value)>>,
    changed: Condvar,
    owners: Mutex<HashMap<String, String>>,
    events: Mutex<(u64, std::collections::VecDeque<Value>)>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    publisher: Option<crate::desktop_events::Publisher>,
}
fn failure(code: &str, message: impl Into<String>) -> HttpError {
    HttpError::coded(503, message, code, json!({"transport":"desktop-ipc"}))
}
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Value, String> {
    let size = reader.read_u32_le().await.map_err(|e| e.to_string())? as usize;
    if size == 0 || size > MAX_FRAME {
        return Err("invalid IPC frame length".into());
    }
    let mut data = vec![0; size];
    reader
        .read_exact(&mut data)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}
async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FRAME {
        return Err("IPC frame too large".into());
    }
    writer
        .write_u32_le(bytes.len() as u32)
        .await
        .map_err(|e| e.to_string())?;
    writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}
impl DesktopClient {
    pub(crate) fn new(options: ServerOptions) -> Self {
        #[cfg(unix)]
        let default = crate::process_runtime::system_codex_home().join("ipc/ipc.sock");
        #[cfg(windows)]
        let default = PathBuf::from(r"\\.\pipe\codex-ipc");
        let endpoint = std::env::var_os("CODEX_DESKTOP_IPC_PATH")
            .map(PathBuf::from)
            .unwrap_or(default);
        Self {
            options,
            runtime: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("IPC runtime"),
            session: Mutex::new(None),
            endpoint,
        }
    }
    fn connect(&self) -> Result<Arc<Session>, HttpError> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| failure("IPC_INTERNAL_ERROR", "session lock"))?;
        if let Some(s) = guard.as_ref().filter(|s| s.alive.load(Ordering::Acquire)) {
            return Ok(s.clone());
        }
        if let Some(s) = guard.take() {
            s.close("IPC connection replaced");
        }
        let (stream,id)=self.runtime.block_on(async {
            let connect=async {
                #[cfg(unix)]
                let stream:Box<dyn Stream>={
                    use std::os::unix::fs::{FileTypeExt,MetadataExt};
                    let meta=std::fs::symlink_metadata(&self.endpoint).map_err(|e|e.to_string())?;
                    if !meta.file_type().is_socket() || meta.uid()!=unsafe{libc::geteuid()} {return Err("IPC endpoint must be a socket owned by the current user".to_string());}
                    Box::new(tokio::net::UnixStream::connect(&self.endpoint).await.map_err(|e|e.to_string())?)
                };
                #[cfg(windows)]
                let stream:Box<dyn Stream>=Box::new(tokio::net::windows::named_pipe::ClientOptions::new().open(&self.endpoint).map_err(|e|e.to_string())?);
                let mut stream=stream;
                let id=random_event_id();
                write_frame(&mut stream,&json!({"type":"request","requestId":id,"sourceClientId":"initializing-client","method":"initialize","version":0,"params":{"clientType":"baijimu-desktop-connector"},"timeoutMs":10000})).await?;
                loop {
                    let v=read_frame(&mut stream).await?;
                    if v["type"]=="response" && v["requestId"]==id {
                        let cid=v.pointer("/result/clientId").and_then(Value::as_str).filter(|_|v["resultType"]=="success").ok_or("desktop IPC initialization rejected")?.to_string();
                        return Ok::<_,String>((stream,cid));
                    }
                    if v["type"]=="client-discovery" {write_frame(&mut stream,&json!({"type":"client-discovery-response","requestId":v["requestId"],"response":{"canHandle":false}})).await?;}
                }
            };
            tokio::time::timeout(Duration::from_secs(10),connect).await.map_err(|_|"desktop IPC connection timed out".to_string())?
        }).map_err(|e|failure("DESKTOP_IPC_UNAVAILABLE",e))?;
        let (tx, mut rx) = async_mpsc::channel(64);
        let session = Arc::new(Session {
            id,
            alive: AtomicBool::new(true),
            outgoing: tx,
            pending: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
            owners: Mutex::new(HashMap::new()),
            events: Mutex::new((0, std::collections::VecDeque::new())),
            tasks: Mutex::new(vec![]),
            publisher: crate::desktop_events::Publisher::from_env(),
        });
        let (mut reader, mut writer) = tokio::io::split(stream);
        let s = session.clone();
        let task = self.runtime.spawn(async move {
            loop {
                match read_frame(&mut reader).await {
                    Ok(v) => s.receive(v),
                    Err(e) => {
                        s.close(&e);
                        break;
                    }
                }
            }
        });
        session.tasks.lock().unwrap().push(task.abort_handle());
        let s = session.clone();
        let task = self.runtime.spawn(async move {
            while let Some(v) = rx.recv().await {
                match tokio::time::timeout(Duration::from_secs(10), write_frame(&mut writer, &v))
                    .await
                {
                    Ok(Ok(())) => {}
                    _ => {
                        s.close("IPC write failed; delivery outcome may be unknown");
                        break;
                    }
                }
            }
        });
        session.tasks.lock().unwrap().push(task.abort_handle());
        *guard = Some(session.clone());
        Ok(session)
    }
    pub(crate) fn status(&self) -> Value {
        match self.connect() {
            Ok(s) => {
                json!({"connector":{"name":crate::CONNECTOR_NAME,"version":crate::VERSION},"transport":"desktop-ipc","connected":true,"clientId":s.id,"endpoint":self.endpoint,"capabilities":{"createThread":false,"rawAppServerRpc":false,"liveTaskControl":true}})
            }
            Err(e) => {
                json!({"transport":"desktop-ipc","connected":false,"endpoint":self.endpoint,"error":{"code":e.code,"message":e.message}})
            }
        }
    }
    pub(crate) fn ensure_connected(&self) -> Result<(), HttpError> {
        self.connect().map(|_| ())
    }
    pub(crate) fn read(&self, thread: &str) -> Result<Value, HttpError> {
        let s = self.connect()?;
        let owner = s.owner(thread, self.options.request_timeout_ms)?;
        s.send(json!({"type":"broadcast","method":"thread-stream-following-changed","version":1,"sourceClientId":s.id,"targetClientIds":[owner],"params":{"conversationId":thread,"hostId":"local","following":true}}))?;
        let response = s.rpc(
            "thread-follower-load-complete-history",
            1,
            json!({"conversationId":thread}),
            Some(&owner),
            self.options.request_timeout_ms,
            false,
        )?;
        let revision = response
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| failure("IPC_PROTOCOL_MISMATCH", "missing history revision"))?;
        let deadline = Instant::now() + Duration::from_millis(self.options.request_timeout_ms);
        let mut snapshots = s.snapshots.lock().unwrap();
        loop {
            if let Some((r, state)) = snapshots.get(thread).filter(|(r, _)| *r >= revision) {
                return Ok(
                    json!({"thread":state,"revision":r,"ownerClientId":owner,"transport":"desktop-ipc"}),
                );
            }
            if !s.alive.load(Ordering::Acquire) {
                return Err(failure(
                    "DESKTOP_IPC_DISCONNECTED",
                    "desktop disconnected during history read",
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(failure(
                    "IPC_SNAPSHOT_TIMEOUT",
                    "desktop did not deliver requested snapshot",
                ));
            }
            snapshots = s.changed.wait_timeout(snapshots, remaining).unwrap().0;
        }
    }
    pub(crate) fn follower(
        &self,
        thread: &str,
        method: &str,
        version: u64,
        params: Value,
    ) -> Result<Value, HttpError> {
        self.follower_checked(thread, method, version, params, None)
    }
    pub(crate) fn follower_for_owner(
        &self,
        thread: &str,
        method: &str,
        version: u64,
        params: Value,
        owner: &str,
    ) -> Result<Value, HttpError> {
        self.follower_checked(thread, method, version, params, Some(owner))
    }
    fn follower_checked(
        &self,
        thread: &str,
        method: &str,
        version: u64,
        mut params: Value,
        expected_owner: Option<&str>,
    ) -> Result<Value, HttpError> {
        let s = self.connect()?;
        let owner = s.owner(thread, self.options.request_timeout_ms)?;
        if expected_owner.is_some_and(|expected| expected != owner) {
            return Err(failure(
                "IPC_OWNER_CHANGED",
                "桌面任务所有者已变化，请重新读取待处理请求",
            ));
        }
        params["conversationId"] = json!(thread);
        s.rpc(
            method,
            version,
            params,
            Some(&owner),
            self.options.request_timeout_ms,
            true,
        )
    }
    pub(crate) fn recent_events(&self, body: &Value) -> Value {
        let guard = self.session.lock().unwrap();
        let Some(s) = guard.as_ref() else {
            return json!({"events":[],"connected":false});
        };
        let events = s.events.lock().unwrap();
        let after = body["afterSequence"].as_u64().unwrap_or(0);
        let limit = body["limit"].as_u64().unwrap_or(100).clamp(1, 500) as usize;
        let rows: Vec<_> = events
            .1
            .iter()
            .filter(|v| v["sequence"].as_u64().unwrap_or(0) > after)
            .take(limit)
            .collect();
        json!({"events":rows,"latestSequence":events.0,"streamId":s.id,"connected":s.alive.load(Ordering::Acquire)})
    }
}
impl Drop for DesktopClient {
    fn drop(&mut self) {
        if let Ok(g) = self.session.lock() {
            if let Some(s) = g.as_ref() {
                s.close("Connector stopped");
            }
        }
    }
}
impl Session {
    fn send(&self, v: Value) -> Result<(), HttpError> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(failure(
                "DESKTOP_IPC_DISCONNECTED",
                "desktop IPC disconnected",
            ));
        }
        self.outgoing
            .try_send(v)
            .map_err(|_| failure("IPC_QUEUE_UNAVAILABLE", "request was not queued"))
    }
    fn rpc(
        &self,
        method: &str,
        version: u64,
        params: Value,
        target: Option<&str>,
        timeout: u64,
        mutation: bool,
    ) -> Result<Value, HttpError> {
        let id = random_event_id();
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let mut v = json!({"type":"request","requestId":id,"sourceClientId":self.id,"method":method,"version":version,"params":params,"timeoutMs":timeout});
        if let Some(target) = target {
            v["targetClientId"] = json!(target);
        }
        if let Err(e) = self.send(v) {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let response = rx.recv_timeout(Duration::from_millis(timeout));
        self.pending.lock().unwrap().remove(&id);
        match response {
            Ok(Ok(v)) if v["resultType"] == "success" => Ok(v["result"].clone()),
            Ok(Ok(v)) => Err(HttpError::coded(
                409,
                v["error"].as_str().unwrap_or("desktop rejected request"),
                if mutation {
                    "IPC_OPERATION_OUTCOME_UNKNOWN"
                } else {
                    "DESKTOP_REQUEST_REJECTED"
                },
                json!({"method":method,"requestId":id,"response":v,"retrySafe":!mutation}),
            )),
            other => {
                let message = match other {
                    Ok(Err(e)) => e.message,
                    _ => "IPC response timed out".into(),
                };
                Err(HttpError::coded(
                    503,
                    message,
                    if mutation {
                        "IPC_OPERATION_OUTCOME_UNKNOWN"
                    } else {
                        "DESKTOP_IPC_DISCONNECTED"
                    },
                    json!({"requestId":id,"method":method,"retrySafe":!mutation}),
                ))
            }
        }
    }
    fn owner(&self, thread: &str, timeout: u64) -> Result<String, HttpError> {
        let id = random_event_id();
        let (tx, rx) = mpsc::sync_channel(1);
        self.pending.lock().unwrap().insert(id.clone(), tx);
        let sent=self.send(json!({"type":"request","requestId":id,"sourceClientId":self.id,"version":1,"method":"thread-owner-discovery","params":{"hostId":"local","conversationId":thread},"timeoutMs":timeout}));
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let result = rx.recv_timeout(Duration::from_millis(timeout));
        self.pending.lock().unwrap().remove(&id);
        let v = result
            .map_err(|_| failure("DESKTOP_OWNER_UNAVAILABLE", "owner discovery timed out"))??;
        let owner = v["handledByClientId"]
            .as_str()
            .filter(|_| v["resultType"] == "success")
            .ok_or_else(|| {
                failure(
                    "DESKTOP_OWNER_UNAVAILABLE",
                    "请在桌面端打开该任务；未找到任务所有者",
                )
            })?
            .to_string();
        if v.pointer("/result/supportsUntrustedAppInput") != Some(&Value::Bool(true)) {
            return Err(failure(
                "IPC_PROTOCOL_MISMATCH",
                "desktop owner does not advertise untrusted app input support",
            ));
        }
        let previous = self
            .owners
            .lock()
            .unwrap()
            .insert(thread.to_string(), owner.clone());
        if previous.as_ref() != Some(&owner) {
            self.snapshots.lock().unwrap().remove(thread);
        }
        Ok(owner)
    }
    fn receive(&self, v: Value) {
        match v["type"].as_str().unwrap_or("") {
            "response" => {
                if let Some(id) = v["requestId"].as_str() {
                    if let Some(tx) = self.pending.lock().unwrap().remove(id) {
                        let _ = tx.send(Ok(v));
                    }
                }
            }
            "client-discovery" => {
                let _=self.send(json!({"type":"client-discovery-response","requestId":v["requestId"],"response":{"canHandle":false}}));
            }
            "request" => {
                let _=self.send(json!({"type":"response","requestId":v["requestId"],"resultType":"error","error":"Connector is a follower only"}));
            }
            "broadcast" => {
                if v["method"] == "ipc-connection-reset" {
                    self.close("desktop IPC reset");
                    return;
                }
                if v["method"] == "thread-stream-state-changed" {
                    if v["version"] != 11 {
                        self.close("unsupported desktop stream protocol version");
                        return;
                    }
                    let params = &v["params"];
                    if params["hostId"] != "local" {
                        return;
                    }
                    if let Some(id) = params["conversationId"].as_str() {
                        let owner = self.owners.lock().unwrap().get(id).cloned();
                        if owner.as_deref() != v["sourceClientId"].as_str() {
                            return;
                        }
                        let change = &params["change"];
                        if change["type"] == "snapshot" {
                            if let Some(revision) = change["revision"].as_u64() {
                                self.snapshots.lock().unwrap().insert(
                                    id.into(),
                                    (revision, change["conversationState"].clone()),
                                );
                                self.changed.notify_all();
                            }
                        }
                    }
                }
                let mut events = self.events.lock().unwrap();
                events.0 += 1;
                let seq = events.0;
                events
                    .1
                    .push_back(json!({"streamId":self.id,"sequence":seq,"receivedAt":crate::timestamp(),"message":v}));
                if let (Some(publisher), Some(event)) = (&self.publisher, events.1.back()) {
                    publisher.publish(event.clone());
                }
                // Bound retained payload bytes as well as count; snapshots can be large.
                while events.1.len() > 100
                    || events.1.iter().map(|e| e.to_string().len()).sum::<usize>() > 8 * 1024 * 1024
                {
                    events.1.pop_front();
                }
            }
            _ => {}
        }
    }
    fn close(&self, message: &str) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(failure("DESKTOP_IPC_DISCONNECTED", message)));
        }
        self.snapshots.lock().unwrap().clear();
        self.changed.notify_all();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_oversized_frames_before_allocating() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut bytes = &(MAX_FRAME as u32 + 1).to_le_bytes()[..];
            assert!(read_frame(&mut bytes).await.unwrap_err().contains("length"));
        });
    }
    #[test]
    fn framing_round_trip() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let v = json!({"type":"response","text":"中文"});
            let mut b = vec![];
            write_frame(&mut b, &v).await.unwrap();
            assert_eq!(read_frame(&mut &b[..]).await.unwrap(), v);
        });
    }
}

#[cfg(all(test, unix))]
mod protocol_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
    };
    fn read(s: &mut std::os::unix::net::UnixStream) -> Option<Value> {
        let mut n = [0; 4];
        s.read_exact(&mut n).ok()?;
        let mut b = vec![0; u32::from_le_bytes(n) as usize];
        s.read_exact(&mut b).ok()?;
        serde_json::from_slice(&b).ok()
    }
    fn send(s: &mut std::os::unix::net::UnixStream, v: Value) {
        let b = serde_json::to_vec(&v).unwrap();
        let mut frame = (b.len() as u32).to_le_bytes().to_vec();
        frame.extend(b); // fragmentation must be accepted
        for chunk in frame.chunks(7) {
            if s.write_all(chunk).is_err() {
                return;
            }
        }
    }
    fn fixture(drop_mutation: bool) -> (DesktopClient, std::thread::JoinHandle<usize>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("ipc-{}", &random_event_id()[10..22]));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("ipc.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
            let mut writes = 0;
            while let Some(v) = read(&mut socket) {
                let method = v["method"].as_str().unwrap_or("");
                let mut result = json!({});
                let mut owner = "owner-a";
                match method {
                    "initialize" => {
                        assert_eq!(v["version"], 0);
                        owner = "follower-a";
                        result = json!({"clientId":"follower-a"});
                    }
                    "thread-owner-discovery" => {
                        assert_eq!(v["params"]["hostId"], "local");
                        result = json!({"supportsUntrustedAppInput":true});
                    }
                    "thread-stream-following-changed" => continue,
                    "thread-follower-load-complete-history" => {
                        assert_eq!(v["targetClientId"], "owner-a");
                        assert_eq!(v["version"], 1);
                        send(
                            &mut socket,
                            json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner-a","params":{"hostId":"local","conversationId":"task-a","change":{"type":"snapshot","revision":7,"conversationState":{"id":"task-a","turns":[{"turnId":"turn-a"}]}}}}),
                        );
                        result = json!({"revision":7});
                    }
                    "thread-follower-start-turn" => {
                        writes += 1;
                        assert_eq!(v["version"], 2);
                        assert_eq!(v["targetClientId"], "owner-a");
                        if drop_mutation {
                            break;
                        }
                        assert_eq!(v["params"]["turnStart"]["request"]["cwd"], "/project-a");
                        result = json!({"result":{"turn":{"id":"new-turn"}}});
                    }
                    _ => {}
                }
                send(
                    &mut socket,
                    json!({"type":"response","method":method,"requestId":v["requestId"],"resultType":"success","handledByClientId":owner,"result":result}),
                );
            }
            writes
        });
        let mut client = DesktopClient::new(ServerOptions {
            host: "127.0.0.1".into(),
            port: 0,
            listen: "stdio://".into(),
            extra_args: vec![],
            request_timeout_ms: 1000,
            daemon: false,
        });
        client.endpoint = path;
        (client, handle, dir)
    }
    #[test]
    fn start_turn_preserves_cwd_and_approval_cannot_change_owner() {
        let (c, h, dir) = fixture(false);
        let state = crate::AppState {
            client: c,
            management_operation: std::sync::Mutex::new(()),
            management_token: String::new(),
        };
        let body =
            serde_json::to_vec(&json!({"threadId":"task-a","input":"hello","cwd":"/project-a"}))
                .unwrap();
        let value =
            crate::desktop_invoke::invoke_http("/invoke/startTurn", &body, 1, &state).unwrap();
        assert_eq!(value["result"]["turn"]["id"], "new-turn");
        let error = state
            .client
            .follower_for_owner(
                "task-a",
                "thread-follower-command-approval-decision",
                1,
                json!({"requestId":1,"decision":"accept"}),
                "different-owner",
            )
            .unwrap_err();
        assert_eq!(error.code, Some(json!("IPC_OWNER_CHANGED")));
        drop(state);
        assert_eq!(h.join().unwrap(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn reads_owner_snapshot_through_fragmented_ipc() {
        let (c, h, dir) = fixture(false);
        let state = c.read("task-a").unwrap();
        assert_eq!(state["revision"], 7);
        assert_eq!(state["thread"]["turns"][0]["turnId"], "turn-a");
        drop(c);
        assert_eq!(h.join().unwrap(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn disconnected_write_is_unknown_and_is_never_replayed() {
        let (c, h, dir) = fixture(true);
        let e = c
            .follower(
                "task-a",
                "thread-follower-start-turn",
                2,
                json!({"turnStart":{}}),
            )
            .unwrap_err();
        assert_eq!(e.code, Some(json!("IPC_OPERATION_OUTCOME_UNKNOWN")));
        assert_eq!(e.data.unwrap()["retrySafe"], false);
        drop(c);
        assert_eq!(h.join().unwrap(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unavailable_desktop_cannot_start_a_backend() {
        let (mut c, h, dir) = fixture(false);
        c.endpoint = dir.join("missing.sock");
        let err = c.read("task-a").unwrap_err();
        assert_eq!(err.code, Some(json!("DESKTOP_IPC_UNAVAILABLE"))); // release fixture accept without any RPC
        let _ = std::os::unix::net::UnixStream::connect(dir.join("ipc.sock"));
        drop(c);
        assert_eq!(h.join().unwrap(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
