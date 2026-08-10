use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STATE_VERSION: u32 = 1;
const STATE_FILE: &str = "thread-read-state.json";
const DESKTOP_UNREAD_KEY: &str = "unread-thread-ids-by-host-v1";
const LOCAL_HOST_ID: &str = "local";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadReadEntry {
    has_unread_turn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_updated_at: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_runtime_status_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_latest_turn_status: Option<String>,
    #[serde(default)]
    observed_desktop_unread: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadReadStore {
    version: u32,
    #[serde(default)]
    threads: HashMap<String, ThreadReadEntry>,
}

impl Default for ThreadReadStore {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            threads: HashMap::new(),
        }
    }
}

pub fn enrich_thread_list(
    connector_data_dir: &Path,
    codex_home: &Path,
    items: &mut [Value],
) -> Result<(), String> {
    let mut store = load_store(connector_data_dir)?;
    let desktop_unread = desktop_unread_thread_ids(codex_home)?;
    let original = store.clone();

    for item in items {
        let Some(map) = item.as_object_mut() else {
            continue;
        };
        let Some(thread_id) = thread_id(map) else {
            continue;
        };
        let updated_at = map.get("updatedAt").cloned();
        let is_desktop_unread = desktop_unread.contains(&thread_id);
        let runtime_status = normalize_runtime_status(map.get("status"));
        let runtime_status_type = runtime_status
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("notLoaded")
            .to_string();
        let latest_turn_status = latest_turn_status(map, &runtime_status);
        let entry = store
            .threads
            .entry(thread_id)
            .or_insert_with(|| ThreadReadEntry {
                has_unread_turn: is_desktop_unread,
                observed_updated_at: updated_at.clone(),
                observed_runtime_status_type: Some(runtime_status_type.clone()),
                observed_latest_turn_status: latest_turn_status.clone(),
                observed_desktop_unread: is_desktop_unread,
            });

        let revision_advanced = entry.observed_updated_at.is_some()
            && updated_at.is_some()
            && entry.observed_updated_at != updated_at;
        let activity_finished = entry.observed_runtime_status_type.as_deref() == Some("active")
            && runtime_status_type != "active"
            || entry.observed_latest_turn_status.as_deref() == Some("inProgress")
                && latest_turn_status.as_deref() != Some("inProgress");
        if revision_advanced && activity_finished {
            entry.has_unread_turn = true;
        }
        if !entry.observed_desktop_unread && is_desktop_unread {
            entry.has_unread_turn = true;
        }
        if updated_at.is_some() {
            entry.observed_updated_at = updated_at;
        }
        entry.observed_runtime_status_type = Some(runtime_status_type);
        entry.observed_latest_turn_status = latest_turn_status.clone();
        entry.observed_desktop_unread = is_desktop_unread;

        let is_in_progress = runtime_status
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value == "active")
            || latest_turn_status.as_deref() == Some("inProgress");
        let active_flags = runtime_status
            .get("activeFlags")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        map.insert("threadRuntimeStatus".to_string(), runtime_status);
        map.insert("activeFlags".to_string(), Value::Array(active_flags));
        map.insert("isInProgress".to_string(), Value::Bool(is_in_progress));
        map.insert(
            "latestTurnStatus".to_string(),
            latest_turn_status.map(Value::String).unwrap_or(Value::Null),
        );
        map.insert(
            "hasUnreadTurn".to_string(),
            Value::Bool(entry.has_unread_turn),
        );
    }

    if store.threads != original.threads {
        save_store(connector_data_dir, &store)?;
    }
    Ok(())
}

pub fn set_thread_read_state(
    connector_data_dir: &Path,
    codex_home: &Path,
    thread_id: &str,
    has_unread_turn: bool,
    observed_updated_at: Option<Value>,
) -> Result<Value, String> {
    let mut store = load_store(connector_data_dir)?;
    let desktop_unread = desktop_unread_thread_ids(codex_home)?.contains(thread_id);
    let entry = store.threads.entry(thread_id.to_string()).or_default();
    entry.has_unread_turn = has_unread_turn;
    if observed_updated_at.is_some() {
        entry.observed_updated_at = observed_updated_at;
    }
    entry.observed_desktop_unread = desktop_unread;
    let persisted_observed_updated_at = entry.observed_updated_at.clone();
    save_store(connector_data_dir, &store)?;
    Ok(json!({
        "threadId": thread_id,
        "hasUnreadTurn": has_unread_turn,
        "observedUpdatedAt": persisted_observed_updated_at,
    }))
}

pub fn mark_thread_unread(connector_data_dir: &Path, thread_id: &str) -> Result<(), String> {
    let mut store = load_store(connector_data_dir)?;
    store
        .threads
        .entry(thread_id.to_string())
        .or_default()
        .has_unread_turn = true;
    save_store(connector_data_dir, &store)
}

fn thread_id(map: &Map<String, Value>) -> Option<String> {
    ["threadId", "sessionId", "id"]
        .into_iter()
        .find_map(|key| map.get(key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_runtime_status(status: Option<&Value>) -> Value {
    match status {
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(Value::String(value)) if !value.is_empty() => json!({"type": value}),
        _ => json!({"type": "notLoaded"}),
    }
}

fn latest_turn_status(map: &Map<String, Value>, runtime_status: &Value) -> Option<String> {
    let from_turns = map
        .get("turns")
        .and_then(Value::as_array)
        .and_then(|turns| turns.last())
        .and_then(|value| value.get("turn").unwrap_or(value).get("status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    from_turns.or_else(|| {
        (runtime_status.get("type").and_then(Value::as_str) == Some("active"))
            .then(|| "inProgress".to_string())
    })
}

fn desktop_unread_thread_ids(codex_home: &Path) -> Result<HashSet<String>, String> {
    let path = codex_home.join(".codex-global-state.json");
    let Some(document) = read_optional_json(&path)? else {
        return Ok(HashSet::new());
    };
    let Some(value) = document.get(DESKTOP_UNREAD_KEY) else {
        return Ok(HashSet::new());
    };
    let hosts = value.as_object().ok_or_else(|| {
        format!(
            "Codex desktop unread state must be an object: {}",
            path.display()
        )
    })?;
    let Some(local) = hosts.get(LOCAL_HOST_ID) else {
        return Ok(HashSet::new());
    };
    let items = local.as_array().ok_or_else(|| {
        format!(
            "Codex desktop local unread state must be an array: {}",
            path.display()
        )
    })?;
    Ok(items
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect())
}

fn load_store(connector_data_dir: &Path) -> Result<ThreadReadStore, String> {
    let path = state_path(connector_data_dir);
    let Some(document) = read_optional_json(&path)? else {
        return Ok(ThreadReadStore::default());
    };
    let store: ThreadReadStore = serde_json::from_value(document)
        .map_err(|error| format!("invalid thread read state {}: {error}", path.display()))?;
    if store.version != STATE_VERSION {
        return Err(format!(
            "unsupported thread read state version {} in {}",
            store.version,
            path.display()
        ));
    }
    Ok(store)
}

fn save_store(connector_data_dir: &Path, store: &ThreadReadStore) -> Result<(), String> {
    fs::create_dir_all(connector_data_dir).map_err(|error| {
        format!(
            "failed to prepare connector data directory {}: {error}",
            connector_data_dir.display()
        )
    })?;
    let path = state_path(connector_data_dir);
    let temporary = temporary_path(&path);
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|error| format!("failed to encode thread read state: {error}"))?;
    fs::write(&temporary, bytes).map_err(|error| {
        format!(
            "failed to write thread read state {}: {error}",
            temporary.display()
        )
    })?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(|error| {
        format!(
            "failed to protect thread read state {}: {error}",
            temporary.display()
        )
    })?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path).map_err(|error| {
            format!(
                "failed to replace thread read state {}: {error}",
                path.display()
            )
        })?;
    }
    fs::rename(&temporary, &path).map_err(|error| {
        format!(
            "failed to publish thread read state {}: {error}",
            path.display()
        )
    })?;
    Ok(())
}

fn read_optional_json(path: &Path) -> Result<Option<Value>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|error| format!("invalid JSON in {}: {error}", path.display()))
}

fn state_path(connector_data_dir: &Path) -> PathBuf {
    connector_data_dir.join(STATE_FILE)
}

fn temporary_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("tmp-{}-{nanos}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "baijimu-codex-thread-state-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn enriches_runtime_and_desktop_unread_state_then_marks_read() {
        let root = temp_dir("enrich");
        let codex_home = root.join("codex");
        let connector_home = root.join("connector");
        fs::create_dir_all(&codex_home).unwrap();
        fs::write(
            codex_home.join(".codex-global-state.json"),
            r#"{"unread-thread-ids-by-host-v1":{"local":["thread-1"]}}"#,
        )
        .unwrap();
        let mut threads = vec![json!({
            "id": "thread-1",
            "updatedAt": 10,
            "status": {"type": "active", "activeFlags": ["waitingOnApproval"]},
            "turns": []
        })];

        enrich_thread_list(&connector_home, &codex_home, &mut threads).unwrap();
        assert_eq!(threads[0]["hasUnreadTurn"], true);
        assert_eq!(threads[0]["isInProgress"], true);
        assert_eq!(threads[0]["latestTurnStatus"], "inProgress");
        assert_eq!(threads[0]["activeFlags"][0], "waitingOnApproval");

        set_thread_read_state(
            &connector_home,
            &codex_home,
            "thread-1",
            false,
            Some(json!(10)),
        )
        .unwrap();
        enrich_thread_list(&connector_home, &codex_home, &mut threads).unwrap();
        assert_eq!(threads[0]["hasUnreadTurn"], false);

        threads[0]["updatedAt"] = json!(11);
        threads[0]["status"] = json!({"type": "idle"});
        threads[0]["turns"] = json!([{"status": "completed"}]);
        enrich_thread_list(&connector_home, &codex_home, &mut threads).unwrap();
        assert_eq!(threads[0]["hasUnreadTurn"], true);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completion_marks_a_thread_unread_without_touching_desktop_state() {
        let root = temp_dir("completion");
        mark_thread_unread(&root, "thread-2").unwrap();
        let store = load_store(&root).unwrap();
        assert!(store.threads["thread-2"].has_unread_turn);
        fs::remove_dir_all(root).unwrap();
    }
}
