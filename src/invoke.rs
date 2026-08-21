use crate::*;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StateProjectRoot {
    pub(crate) path: String,
    pub(crate) project_id: Option<String>,
    pub(crate) project_name: Option<String>,
    pub(crate) root_paths: Vec<String>,
}

pub(crate) fn handle_invoke(
    path: &str,
    body: &Value,
    client: &CodexClient,
) -> Result<Value, HttpError> {
    match path {
        "/invoke/status" => Ok(client.status()),
        "/invoke/listThreads" | "/invoke/listSessions" => list_threads(body, client),
        "/invoke/listProjects" => list_projects(body, client),
        "/invoke/searchThreads" => {
            if string_field(body, "searchTerm").is_none()
                && body
                    .pointer("/params/searchTerm")
                    .and_then(Value::as_str)
                    .is_none()
            {
                return Err(HttpError::new(400, "searchTerm is required"));
            }
            let params = merge_params(
                body,
                &[
                    "cursor",
                    "limit",
                    "sortKey",
                    "sortDirection",
                    "sourceKinds",
                    "archived",
                    "searchTerm",
                ],
            );
            Ok(json!({"result": client.request("thread/search", params, timeout_ms(body))?}))
        }
        "/invoke/readThread" => {
            let Some(thread_id) = string_field(body, "threadId").or_else(|| {
                body.pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }) else {
                return Err(HttpError::new(400, "threadId is required"));
            };
            let mut params = merge_params(body, &["threadId", "includeTurns"]);
            params["threadId"] = Value::String(thread_id);
            Ok(json!({"result": client.request("thread/read", params, timeout_ms(body))?}))
        }
        "/invoke/setThreadReadState" => {
            let Some(thread_id) = string_param(body, "threadId") else {
                return Err(HttpError::new(400, "threadId is required"));
            };
            let Some(has_unread_turn) = body
                .get("hasUnreadTurn")
                .or_else(|| body.pointer("/params/hasUnreadTurn"))
                .and_then(Value::as_bool)
            else {
                return Err(HttpError::new(400, "hasUnreadTurn is required"));
            };
            let mut observed_updated_at = body
                .get("observedUpdatedAt")
                .or_else(|| body.pointer("/params/observedUpdatedAt"))
                .cloned();
            if observed_updated_at.is_none() {
                let result = client.request(
                    "thread/read",
                    json!({"threadId": thread_id, "includeTurns": false}),
                    timeout_ms(body),
                )?;
                observed_updated_at = result.pointer("/thread/updatedAt").cloned();
            }
            client.refresh_active_home();
            let result = thread_state::set_thread_read_state(
                client.state_dir(),
                &client.active_codex_home(),
                &thread_id,
                has_unread_turn,
                observed_updated_at,
            )
            .map_err(HttpError::internal)?;
            Ok(json!({"result": result}))
        }
        "/invoke/listThreadTurns" => list_thread_turns(body, client),
        "/invoke/listApps" => {
            let params = merge_params(body, &["cursor", "limit", "threadId", "forceRefetch"]);
            Ok(json!({"result": client.request("app/list", params, timeout_ms(body))?}))
        }
        "/invoke/startThread" => {
            let mut params = merge_params(body, &[]);
            if let Some(model) = body.get("model").cloned() {
                params["model"] = model;
            }
            if let Some(cwd) = body.get("cwd").cloned() {
                params["cwd"] = cwd;
            }
            Ok(json!({"result": client.request("thread/start", params, timeout_ms(body))?}))
        }
        "/invoke/resumeThread" => {
            let Some(thread_id) = string_field(body, "threadId") else {
                return Err(HttpError::new(400, "threadId is required"));
            };
            let mut params = merge_params(body, &[]);
            params["threadId"] = Value::String(thread_id);
            for key in ["excludeTurns", "initialTurnsPage"] {
                if let Some(value) = body.get(key).cloned() {
                    params[key] = value;
                }
            }
            Ok(json!({"result": client.request("thread/resume", params, timeout_ms(body))?}))
        }
        "/invoke/startTurn" => {
            let Some(thread_id) = string_field(body, "threadId") else {
                return Err(HttpError::new(400, "threadId is required"));
            };
            let Some(input) = body.get("input").cloned() else {
                return Err(HttpError::new(400, "input is required"));
            };
            let mut params = merge_params(body, &[]);
            params["threadId"] = Value::String(thread_id);
            params["input"] = normalize_input(input);
            if let Some(model) = body.get("model").cloned() {
                params["model"] = model;
            }
            if let Some(cwd) = body.get("cwd").cloned() {
                params["cwd"] = cwd;
            }
            Ok(json!({
                "result": client.request("turn/start", params, timeout_ms(body))?,
                "recentEvents": client.recent_events(&json!({"limit": 50})),
            }))
        }
        "/invoke/steerTurn" => {
            let Some(input) = body.get("input").cloned() else {
                return Err(HttpError::new(400, "input is required"));
            };
            let mut params = merge_params(body, &[]);
            for key in ["threadId", "turnId"] {
                if let Some(value) = body.get(key).cloned() {
                    params[key] = value;
                }
            }
            params["input"] = normalize_input(input);
            Ok(json!({
                "result": client.request("turn/steer", params, timeout_ms(body))?,
                "recentEvents": client.recent_events(&json!({"limit": 50})),
            }))
        }
        "/invoke/interruptTurn" => {
            let mut params = merge_params(body, &[]);
            for key in ["threadId", "turnId"] {
                if let Some(value) = body.get(key).cloned() {
                    params[key] = value;
                }
            }
            Ok(json!({
                "result": client.request("turn/interrupt", params, timeout_ms(body))?,
                "recentEvents": client.recent_events(&json!({"limit": 50})),
            }))
        }
        "/invoke/recentEvents" => Ok(client.recent_events(body)),
        "/invoke/request" => {
            let Some(method) = string_field(body, "method") else {
                return Err(HttpError::new(400, "method is required"));
            };
            let params = body.get("params").cloned().unwrap_or_else(|| json!({}));
            Ok(json!({
                "result": client.request(&method, params, timeout_ms(body))?,
                "recentEvents": client.recent_events(&json!({"limit": 50})),
            }))
        }
        _ => Err(HttpError::new(404, format!("unknown invoke path: {path}"))),
    }
}

fn list_threads(body: &Value, client: &CodexClient) -> Result<Value, HttpError> {
    let mut params = merge_params(
        body,
        &[
            "cursor",
            "limit",
            "sortKey",
            "sortDirection",
            "modelProviders",
            "sourceKinds",
            "archived",
            "cwd",
            "useStateDbOnly",
            "searchTerm",
        ],
    );
    if params.get("sortKey").is_none() {
        params["sortKey"] = Value::String(DEFAULT_THREAD_SORT_KEY.to_string());
    }
    if params.get("sortDirection").is_none() {
        params["sortDirection"] = Value::String(DEFAULT_THREAD_SORT_DIRECTION.to_string());
    }
    let mut result = client.request("thread/list", params, timeout_ms(body))?;
    if let Some(data) = result.get_mut("data").and_then(Value::as_array_mut) {
        for item in data.iter_mut() {
            *item = normalize_thread_list_item(item.clone());
        }
        client.refresh_active_home();
        thread_state::enrich_thread_list(client.state_dir(), &client.active_codex_home(), data)
            .map_err(HttpError::internal)?;
    }
    Ok(json!({"result": result}))
}

fn list_thread_turns(body: &Value, client: &CodexClient) -> Result<Value, HttpError> {
    let Some(thread_id) = string_field(body, "threadId").or_else(|| {
        body.pointer("/params/threadId")
            .and_then(Value::as_str)
            .map(str::to_string)
    }) else {
        return Ok(json!({"result": {"data": [], "nextCursor": null, "backwardsCursor": null}}));
    };
    let mut params = merge_params(
        body,
        &["threadId", "cursor", "limit", "sortDirection", "itemsView"],
    );
    params["threadId"] = Value::String(thread_id.clone());
    match client.request("thread/turns/list", params, timeout_ms(body)) {
        Ok(result) => Ok(json!({"result": result})),
        Err(error) => {
            client.record_event(
                "connector/threadTurnsListFallback",
                json!({"threadId": thread_id, "error": error.message, "code": error.code}),
            );
            let result = client.request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": true}),
                timeout_ms(body),
            )?;
            let turns = result
                .pointer("/thread/turns")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Ok(
                json!({"result": {"data": turns, "nextCursor": null, "backwardsCursor": null, "fallback": "thread/read"}}),
            )
        }
    }
}

fn list_projects(body: &Value, client: &CodexClient) -> Result<Value, HttpError> {
    client.refresh_active_home();
    let home = client.active_codex_home();
    let global_state = read_json_file(&home.join(".codex-global-state.json"));
    let configured = parse_codex_project_config(&home.join("config.toml"));
    let mut projects = Map::new();
    let mut saved_roots = resolve_state_project_references(
        &global_state,
        array_strings(global_state.get("project-order")),
    );
    saved_roots.extend(resolve_state_project_references(
        &global_state,
        array_strings(global_state.get("electron-saved-workspace-roots")),
    ));
    let saved_roots = unique_state_project_roots(saved_roots);
    let saved_order = saved_roots
        .iter()
        .map(|project| project.path.clone())
        .collect::<Vec<_>>();
    if bool_param(body, "includeSaved", true) {
        for project in &saved_roots {
            upsert_project(
                &mut projects,
                &project.path,
                "saved",
                state_project_fields(project),
            );
        }
    }
    for project in resolve_state_project_references(
        &global_state,
        array_strings(global_state.get("active-workspace-roots")),
    ) {
        let mut fields = state_project_fields(&project);
        fields.insert("active".to_string(), Value::Bool(true));
        upsert_project(&mut projects, &project.path, "active", fields);
    }
    for project in resolve_state_project_references(
        &global_state,
        array_strings(global_state.get("pinned-project-ids")),
    ) {
        let mut fields = state_project_fields(&project);
        fields.insert("pinned".to_string(), Value::Bool(true));
        upsert_project(&mut projects, &project.path, "pinned", fields);
    }
    if bool_param(body, "includeTrusted", true) {
        for (path, trust_level) in configured {
            let mut fields = Map::new();
            fields.insert("trustLevel".to_string(), Value::String(trust_level));
            upsert_project(&mut projects, &path, "trusted", fields);
        }
    }
    read_thread_projects(body, client, &mut projects)?;
    let search_term = string_param(body, "searchTerm")
        .unwrap_or_default()
        .to_lowercase();
    let exists_only = bool_param(body, "existsOnly", false);
    let mut items = projects
        .into_values()
        .filter(|project| {
            if exists_only && project.get("exists").and_then(Value::as_bool) != Some(true) {
                return false;
            }
            if search_term.is_empty() {
                return true;
            }
            project
                .get("title")
                .and_then(Value::as_str)
                .is_some_and(|value| value.to_lowercase().contains(&search_term))
                || project
                    .get("path")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.to_lowercase().contains(&search_term))
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        let lp = left.get("pinned").and_then(Value::as_bool).unwrap_or(false);
        let rp = right
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if lp != rp {
            return rp.cmp(&lp);
        }
        let lpath = left.get("path").and_then(Value::as_str).unwrap_or("");
        let rpath = right.get("path").and_then(Value::as_str).unwrap_or("");
        let li = saved_order
            .iter()
            .position(|path| path == lpath)
            .unwrap_or(usize::MAX);
        let ri = saved_order
            .iter()
            .position(|path| path == rpath)
            .unwrap_or(usize::MAX);
        li.cmp(&ri).then_with(|| {
            left.get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(right.get("title").and_then(Value::as_str).unwrap_or(""))
        })
    });
    let total = items.len();
    let cursor = usize_param(body, "cursor", 0);
    let limit = usize_param(body, "limit", DEFAULT_PROJECT_LIMIT).clamp(1, 500);
    let page = items
        .into_iter()
        .skip(cursor)
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if cursor + limit < total {
        Value::String((cursor + limit).to_string())
    } else {
        Value::Null
    };
    Ok(
        json!({"result": {"projects": page, "items": page, "total": total, "nextCursor": next_cursor, "codexHome": home}}),
    )
}

pub(crate) fn resolve_state_project_references(
    global_state: &Value,
    references: Vec<String>,
) -> Vec<StateProjectRoot> {
    let known_projects = state_project_roots(global_state);
    let mut resolved = Vec::new();
    for reference in references {
        let matches = known_projects
            .iter()
            .filter(|project| project.project_id.as_deref() == Some(reference.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            resolved.extend(matches);
            continue;
        }
        let Some(path) = normalize_state_project_path(&reference) else {
            continue;
        };
        if let Some(project) = known_projects
            .iter()
            .find(|project| project.path == path)
            .cloned()
        {
            resolved.push(project);
        } else {
            resolved.push(StateProjectRoot {
                path: path.clone(),
                project_id: None,
                project_name: None,
                root_paths: vec![path],
            });
        }
    }
    unique_state_project_roots(resolved)
}

fn state_project_roots(global_state: &Value) -> Vec<StateProjectRoot> {
    let mut projects = Vec::new();
    if let Some(local_projects) = global_state
        .get("local-projects")
        .and_then(Value::as_object)
    {
        for (fallback_id, value) in local_projects {
            let project_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(fallback_id)
                .to_string();
            let project_name = value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string);
            let root_paths = unique_strings(
                array_strings(value.get("rootPaths"))
                    .into_iter()
                    .filter_map(|path| normalize_state_project_path(&path))
                    .collect(),
            );
            for path in &root_paths {
                projects.push(StateProjectRoot {
                    path: path.clone(),
                    project_id: Some(project_id.clone()),
                    project_name: project_name.clone(),
                    root_paths: root_paths.clone(),
                });
            }
        }
    }

    let mut assignment_roots: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(assignments) = global_state
        .get("thread-project-assignments")
        .and_then(Value::as_object)
    {
        for assignment in assignments.values() {
            let Some(project_id) = assignment.get("projectId").and_then(Value::as_str) else {
                continue;
            };
            let Some(path) = assignment
                .get("cwd")
                .and_then(Value::as_str)
                .and_then(normalize_state_project_path)
            else {
                continue;
            };
            let roots = assignment_roots.entry(project_id.to_string()).or_default();
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    for (project_id, root_paths) in assignment_roots {
        if projects
            .iter()
            .any(|project| project.project_id.as_deref() == Some(project_id.as_str()))
        {
            continue;
        }
        for path in &root_paths {
            projects.push(StateProjectRoot {
                path: path.clone(),
                project_id: Some(project_id.clone()),
                project_name: None,
                root_paths: root_paths.clone(),
            });
        }
    }
    unique_state_project_roots(projects)
}

fn unique_state_project_roots(projects: Vec<StateProjectRoot>) -> Vec<StateProjectRoot> {
    let mut out = Vec::new();
    for project in projects {
        if !out
            .iter()
            .any(|candidate: &StateProjectRoot| candidate.path == project.path)
        {
            out.push(project);
        }
    }
    out
}

fn state_project_fields(project: &StateProjectRoot) -> Map<String, Value> {
    let mut fields = Map::new();
    if let Some(project_id) = &project.project_id {
        fields.insert("projectId".to_string(), Value::String(project_id.clone()));
    }
    if let Some(project_name) = &project.project_name {
        fields.insert(
            "projectName".to_string(),
            Value::String(project_name.clone()),
        );
        fields.insert("title".to_string(), Value::String(project_name.clone()));
    }
    fields.insert(
        "rootPaths".to_string(),
        Value::Array(
            project
                .root_paths
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    fields
}

fn normalize_state_project_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = if trimmed == "~" {
        home_dir()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if !path.is_absolute() {
        return None;
    }
    Some(path.display().to_string())
}

fn read_thread_projects(
    body: &Value,
    client: &CodexClient,
    projects: &mut Map<String, Value>,
) -> Result<(), HttpError> {
    if !bool_param(body, "includeThreadStats", true) {
        return Ok(());
    }
    let mut cursor = body
        .get("threadCursor")
        .or_else(|| body.pointer("/params/threadCursor"))
        .cloned()
        .unwrap_or(Value::Null);
    let max_pages =
        usize_param(body, "maxThreadPages", DEFAULT_PROJECT_THREAD_MAX_PAGES).clamp(1, 500);
    let limit =
        usize_param(body, "threadPageLimit", DEFAULT_PROJECT_THREAD_PAGE_LIMIT).clamp(1, 100);
    for _ in 0..max_pages {
        let mut params = json!({
            "cursor": cursor,
            "limit": limit,
            "sortKey": string_param(body, "sortKey").unwrap_or_else(|| DEFAULT_THREAD_SORT_KEY.to_string()),
            "sortDirection": string_param(body, "sortDirection").unwrap_or_else(|| DEFAULT_THREAD_SORT_DIRECTION.to_string()),
        });
        for key in ["archived", "useStateDbOnly"] {
            if let Some(value) = body
                .get(key)
                .or_else(|| body.pointer(&format!("/params/{key}")))
                .cloned()
            {
                params[key] = value;
            }
        }
        let result = client.request("thread/list", params, timeout_ms(body))?;
        let threads = result
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(normalize_thread_list_item)
            .collect::<Vec<_>>();
        for thread in &threads {
            if let Some(cwd) = thread.get("cwd").and_then(Value::as_str) {
                if let Some(path) = normalize_project_path(cwd) {
                    let project = upsert_project(projects, &path, "threads", Map::new());
                    let count = project
                        .get("sessionCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        + 1;
                    project["sessionCount"] = Value::from(count);
                    if let Some(timestamp) = thread_timestamp(thread) {
                        project["lastActiveAt"] = timestamp;
                    }
                }
            }
        }
        cursor = result.get("nextCursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() || threads.is_empty() {
            return Ok(());
        }
    }
    Ok(())
}

fn merge_params(body: &Value, keys: &[&str]) -> Value {
    let mut map = body
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in keys {
        if let Some(value) = body.get(*key).cloned() {
            map.insert((*key).to_string(), value);
        }
    }
    Value::Object(map)
}

fn normalize_input(input: Value) -> Value {
    match input {
        Value::String(text) => json!([{"type": "text", "text": text}]),
        other => other,
    }
}

fn normalize_thread_list_item(item: Value) -> Value {
    let Some(thread) = item.get("thread").and_then(Value::as_object) else {
        return item;
    };
    let mut map = item.as_object().cloned().unwrap_or_default();
    for (key, value) in thread {
        map.insert(key.clone(), value.clone());
    }
    Value::Object(map)
}

fn string_field(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn timeout_ms(body: &Value) -> Option<u64> {
    body.get("timeoutMs").and_then(Value::as_u64)
}

fn string_param(body: &Value, key: &str) -> Option<String> {
    body.get(key)
        .or_else(|| body.pointer(&format!("/params/{key}")))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn bool_param(body: &Value, key: &str, default: bool) -> bool {
    body.get(key)
        .or_else(|| body.pointer(&format!("/params/{key}")))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn usize_param(body: &Value, key: &str, default: usize) -> usize {
    body.get(key)
        .or_else(|| body.pointer(&format!("/params/{key}")))
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
}

pub(crate) fn read_json_file(path: &Path) -> Value {
    fs::read(path)
        .ok()
        .and_then(|content| json_compat::from_slice(&content).ok())
        .unwrap_or_else(|| json!({}))
}

fn parse_codex_project_config(path: &Path) -> Vec<(String, String)> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut current: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(raw) = trimmed
            .strip_prefix("[projects.\"")
            .and_then(|value| value.strip_suffix("\"]"))
        {
            current = normalize_project_path(&raw.replace("\\\"", "\""));
            continue;
        }
        if let Some(project) = current.as_ref() {
            if let Some(value) = trimmed
                .strip_prefix("trust_level")
                .and_then(|value| value.split_once('='))
                .map(|(_, value)| value.trim().trim_matches('"').to_string())
            {
                result.push((project.clone(), value));
            }
        }
    }
    result
}

fn upsert_project<'a>(
    projects: &'a mut Map<String, Value>,
    path: &str,
    source: &str,
    fields: Map<String, Value>,
) -> &'a mut Value {
    let title = Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_string();
    let project = projects.entry(path.to_string()).or_insert_with(|| {
        json!({
            "id": path,
            "path": path,
            "cwd": path,
            "title": title,
            "exists": Path::new(path).exists(),
            "pinned": false,
            "active": false,
            "trustLevel": null,
            "projectId": null,
            "projectName": null,
            "rootPaths": [path],
            "sessionCount": 0,
            "lastActiveAt": null,
            "gitBranch": null,
            "gitOriginUrl": null,
            "sources": [],
        })
    });
    if let Some(sources) = project.get_mut("sources").and_then(Value::as_array_mut) {
        if !sources.iter().any(|value| value.as_str() == Some(source)) {
            sources.push(Value::String(source.to_string()));
        }
    }
    if let Some(map) = project.as_object_mut() {
        for (key, value) in fields {
            map.insert(key, value);
        }
    }
    project
}

fn thread_timestamp(thread: &Value) -> Option<Value> {
    for key in [
        "recencyAt",
        "recency_at",
        "updatedAt",
        "updated_at",
        "createdAt",
        "created_at",
    ] {
        if let Some(value) = thread.get(key).cloned() {
            return Some(value);
        }
    }
    None
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        if !value.trim().is_empty() && !out.iter().any(|candidate| candidate == &value) {
            out.push(value);
        }
    }
    out
}

fn array_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_project_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expanded = if trimmed == "~" {
        home_dir()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    };
    Some(absolute.display().to_string())
}
