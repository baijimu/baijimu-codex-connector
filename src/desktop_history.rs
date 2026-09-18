//! Bounded, read-only access to Codex's native persisted history projection.
//! Control and pending approvals remain owned by Desktop IPC. No second engine,
//! transcript cache, migration, or source-store writes are performed here.
use crate::{desktop_catalog, process_runtime, HttpError};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

const PAGE_BYTES: usize = 256 * 1024;
const PREVIEW_CHARS: i64 = 2000;
const CHUNK_CHARS: i64 = 8192;

fn error(code: &str, message: impl Into<String>) -> HttpError {
    HttpError::coded(409, message, code, json!({}))
}
fn database(e: rusqlite::Error) -> HttpError {
    error(
        "DESKTOP_HISTORY_UNAVAILABLE",
        format!("桌面历史索引不可读或协议不兼容：{e}"),
    )
}
fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str, HttpError> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| HttpError::new(400, format!("{key} is required")))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    kind: String,
    thread: String,
    turn: String,
    order: String,
    head: i64,
    ordinal: i64,
    id: String,
    offset: i64,
    item_version: i64,
    length: i64,
}
impl Cursor {
    fn new(kind: &str, thread: &str, turn: &str, order: &str, head: i64) -> Self {
        Self {
            version: 1,
            kind: kind.into(),
            thread: thread.into(),
            turn: turn.into(),
            order: order.into(),
            head,
            ordinal: -1,
            id: String::new(),
            offset: 0,
            item_version: 0,
            length: 0,
        }
    }
    fn encode(&self) -> String {
        serde_json::to_vec(self)
            .unwrap()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    fn decode(
        body: &Value,
        kind: &str,
        thread: &str,
        turn: &str,
        order: &str,
    ) -> Result<Option<Self>, HttpError> {
        let Some(raw) = body.get("cursor").filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let bad = || error("INVALID_HISTORY_CURSOR", "分页游标无效或不属于当前查询");
        let raw = raw
            .as_str()
            .filter(|s| s.len() <= 8192 && s.len() % 2 == 0 && s.is_ascii())
            .ok_or_else(bad)?;
        let bytes = (0..raw.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).map_err(|_| bad()))
            .collect::<Result<Vec<_>, _>>()?;
        let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| bad())?;
        if cursor.version != 1
            || cursor.kind != kind
            || cursor.thread != thread
            || cursor.turn != turn
            || cursor.order != order
            || cursor.offset < 0
            || cursor.head < -1
            || cursor.ordinal < -1
        {
            return Err(bad());
        }
        Ok(Some(cursor))
    }
}

pub(crate) fn invoke(method: &str, body: &Value) -> Result<Value, HttpError> {
    let thread = required(body, "threadId")?;
    let source = desktop_catalog::history_source(thread)?;
    if source.0 != "paginated" {
        return Err(error("DESKTOP_HISTORY_MODE_UNSUPPORTED", "此任务尚未使用 Codex 原生分页历史；请在支持分页的桌面版本中打开该任务。Connector 不迁移或改写历史。"));
    }
    let path = std::env::var_os("CODEX_DESKTOP_HISTORY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| process_runtime::system_codex_home().join("thread_history_1.sqlite"));
    let db = open_history(&path)?;
    let projection: Option<i64> = db.query_row("SELECT next_rollout_byte_offset FROM thread_history_projection_state WHERE thread_id=?1", [thread], |r| r.get(0)).optional().map_err(database)?;
    let projected = projection.ok_or_else(|| {
        error(
            "DESKTOP_HISTORY_NOT_READY",
            "Codex 尚未建立此任务的历史投影；请在桌面打开任务后重试",
        )
    })?;
    let observed = std::fs::metadata(&source.1).ok().map(|m| m.len());
    let mut result = query(&db, method, body)?;
    result["source"] = json!("desktop-readonly-history");
    // Byte-offset equality is evidence about this observed file only, not a
    // claim that the native writer has flushed every live IPC event.
    result["persistence"] = json!({"projectedBytes":projected,"observedRolloutBytes":observed,"matchesObservedRollout":observed.map(|n| projected >= 0 && n == projected as u64),"liveStateSource":"desktop-ipc"});
    Ok(result)
}

fn open_history(path: &std::path::Path) -> Result<Connection, HttpError> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(database)?;
    db.busy_timeout(Duration::from_secs(2)).map_err(database)?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED")
        .map_err(database)?;
    Ok(db)
}

fn query(db: &Connection, method: &str, body: &Value) -> Result<Value, HttpError> {
    match method {
        "listThreadTurns" => list_turns(db, body),
        "listThreadItems" => list_items(db, body),
        "readThreadItem" => read_item(db, body),
        _ => Err(HttpError::new(404, "unknown history method")),
    }
}
fn order(body: &Value, default: &str) -> Result<&'static str, HttpError> {
    match body["sortDirection"].as_str().unwrap_or(default) {
        "asc" => Ok("asc"),
        "desc" => Ok("desc"),
        _ => Err(HttpError::new(400, "invalid sortDirection")),
    }
}
fn limit(body: &Value, default: i64) -> Result<i64, HttpError> {
    match body.get("limit") {
        None => Ok(default),
        Some(v) => v
            .as_i64()
            .filter(|n| (1..=100).contains(n))
            .ok_or_else(|| HttpError::new(400, "limit must be 1..100")),
    }
}
fn stale() -> HttpError {
    error(
        "HISTORY_CURSOR_STALE",
        "历史已变化，丢弃旧游标并重新读取最新页",
    )
}

fn list_turns(db: &Connection, body: &Value) -> Result<Value, HttpError> {
    let thread = required(body, "threadId")?;
    let sort = order(body, "desc")?;
    let view = body["itemsView"].as_str().unwrap_or("summary");
    if !matches!(view, "summary" | "notLoaded") {
        return Err(error("UNSUPPORTED_ITEMS_VIEW", "轮次列表支持 summary 或 notLoaded；详细条目使用 listThreadItems 和 readThreadItem 分页读取"));
    }
    let limit = limit(body, 1)?;
    let head: i64 = db
        .query_row(
            "SELECT coalesce(max(rollout_ordinal),-1) FROM thread_turns WHERE thread_id=?1",
            [thread],
            |r| r.get(0),
        )
        .map_err(database)?;
    let prior = Cursor::decode(body, "turns", thread, "", sort)?;
    if let Some(c) = &prior {
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND turn_id=?2 AND rollout_ordinal=?3)", params![thread,c.id,c.ordinal], |r| r.get(0)).map_err(database)?;
        if !exists || head < c.head {
            return Err(stale());
        }
    }
    let mut cursor = prior.unwrap_or_else(|| Cursor::new("turns", thread, "", sort, head));
    let op = if sort == "desc" { "<" } else { ">" };
    let sql = format!("SELECT turn_id,rollout_ordinal,status,substr(error_json,1,2000),started_at,completed_at,duration_ms,first_user_item_id,final_agent_item_id FROM thread_turns WHERE thread_id=?1 AND rollout_ordinal<=?2 AND (?3='' OR rollout_ordinal {op} ?4) ORDER BY rollout_ordinal {sort} LIMIT ?5");
    let mut stmt = db.prepare(&sql).map_err(database)?;
    let rows = stmt.query_map(params![thread,cursor.head,cursor.id,cursor.ordinal,limit+1], |r| {
        Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,json!({"turnId":r.get::<_,String>(0)?,"status":r.get::<_,String>(2)?,"errorPreview":r.get::<_,Option<String>>(3)?,"startedAt":r.get::<_,Option<i64>>(4)?,"completedAt":r.get::<_,Option<i64>>(5)?,"durationMs":r.get::<_,Option<i64>>(6)?}),r.get::<_,Option<String>>(7)?,r.get::<_,Option<String>>(8)?))
    }).map_err(database)?;
    let mut data = vec![];
    let mut bytes = 0;
    let mut more = false;
    for row in rows {
        let (id, ordinal, mut value, user, agent) = row.map_err(database)?;
        if data.len() == limit as usize {
            more = true;
            break;
        }
        value["itemsView"] = json!(view);
        value["itemCount"] = json!(db
            .query_row(
                "SELECT count(*) FROM thread_items WHERE thread_id=?1 AND turn_id=?2",
                params![thread, id],
                |r| r.get::<_, i64>(0)
            )
            .map_err(database)?);
        let mut items = vec![];
        if view == "summary" {
            let latest_agent = match agent {
                Some(id) => Some(id),
                None => db.query_row("SELECT item_id FROM thread_items WHERE thread_id=?1 AND turn_id=?2 AND item_type='agentMessage' ORDER BY rollout_ordinal DESC LIMIT 1", params![thread,id], |r|r.get(0)).optional().map_err(database)?,
            };
            for item in [user, latest_agent].into_iter().flatten() {
                if let Some(preview) = item_preview(db, thread, &id, &item)? {
                    items.push(preview);
                }
            }
        }
        value["items"] = json!(items);
        let size = serde_json::to_vec(&value).unwrap().len();
        if bytes + size > PAGE_BYTES && !data.is_empty() {
            more = true;
            break;
        }
        bytes += size;
        data.push(value);
        cursor.ordinal = ordinal;
        cursor.id = id;
    }
    Ok(
        json!({"data":data,"nextCursor":more.then(||cursor.encode()),"sortDirection":sort,"itemsView":view}),
    )
}

// Extract a short display string inside SQLite. Never deserialize or ship the
// whole tool output just to build a history list.
const PREVIEW_SQL: &str = "CASE item_type WHEN 'userMessage' THEN (SELECT json_extract(value,'$.text') FROM json_each(item_json,'$.content') WHERE json_extract(value,'$.type')='text' LIMIT 1) WHEN 'agentMessage' THEN json_extract(item_json,'$.text') WHEN 'commandExecution' THEN json_extract(item_json,'$.command') WHEN 'mcpToolCall' THEN json_extract(item_json,'$.tool') WHEN 'reasoning' THEN json_extract(item_json,'$.summary[0]') ELSE NULL END";
fn item_preview(
    db: &Connection,
    thread: &str,
    turn: &str,
    id: &str,
) -> Result<Option<Value>, HttpError> {
    let sql = format!("SELECT item_id,item_type,updated_at_ordinal,length(CAST(item_json AS BLOB)),substr(({PREVIEW_SQL}),1,?4),substr(json_extract(item_json,'$.status'),1,100) FROM thread_items WHERE thread_id=?1 AND turn_id=?2 AND item_id=?3");
    db.query_row(&sql, params![thread,turn,id,PREVIEW_CHARS], |r| {
        Ok(json!({"id":r.get::<_,String>(0)?,"type":r.get::<_,String>(1)?,"version":r.get::<_,i64>(2)?,"contentBytes":r.get::<_,i64>(3)?,"preview":r.get::<_,Option<String>>(4)?,"status":r.get::<_,Option<String>>(5)?,"detailAvailable":true,"previewOnly":true}))
    }).optional().map_err(database)
}
fn list_items(db: &Connection, body: &Value) -> Result<Value, HttpError> {
    let thread = required(body, "threadId")?;
    let turn = required(body, "turnId")?;
    let sort = order(body, "asc")?;
    let limit = limit(body, 20)?;
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND turn_id=?2)",
            params![thread, turn],
            |r| r.get(0),
        )
        .map_err(database)?;
    if !exists {
        return Err(error("TURN_NOT_FOUND", "轮次不存在于该任务"));
    }
    let head: i64 = db.query_row("SELECT coalesce(max(rollout_ordinal),-1) FROM thread_items WHERE thread_id=?1 AND turn_id=?2", params![thread,turn], |r| r.get(0)).map_err(database)?;
    let prior = Cursor::decode(body, "items", thread, turn, sort)?;
    if let Some(c) = &prior {
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM thread_items WHERE thread_id=?1 AND turn_id=?2 AND item_id=?3 AND rollout_ordinal=?4)",params![thread,turn,c.id,c.ordinal],|r|r.get(0)).map_err(database)?;
        if !exists || head < c.head {
            return Err(stale());
        }
    }
    let mut cursor = prior.unwrap_or_else(|| Cursor::new("items", thread, turn, sort, head));
    let op = if sort == "desc" { "<" } else { ">" };
    let sql = format!("SELECT item_id,rollout_ordinal FROM thread_items WHERE thread_id=?1 AND turn_id=?2 AND rollout_ordinal<=?3 AND (?4='' OR rollout_ordinal {op} ?5) ORDER BY rollout_ordinal {sort} LIMIT ?6");
    let mut stmt = db.prepare(&sql).map_err(database)?;
    let rows = stmt
        .query_map(
            params![
                thread,
                turn,
                cursor.head,
                cursor.id,
                cursor.ordinal,
                limit + 1
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .map_err(database)?;
    let mut data = vec![];
    let mut bytes = 0;
    let mut more = false;
    for row in rows {
        let (id, ordinal) = row.map_err(database)?;
        if data.len() == limit as usize {
            more = true;
            break;
        }
        let value = item_preview(db, thread, turn, &id)?.ok_or_else(stale)?;
        let size = serde_json::to_vec(&value).unwrap().len();
        if bytes + size > PAGE_BYTES && !data.is_empty() {
            more = true;
            break;
        }
        bytes += size;
        data.push(value);
        cursor.id = id;
        cursor.ordinal = ordinal;
    }
    Ok(json!({"data":data,"nextCursor":more.then(||cursor.encode()),"sortDirection":sort}))
}
fn read_item(db: &Connection, body: &Value) -> Result<Value, HttpError> {
    let thread = required(body, "threadId")?;
    let turn = required(body, "turnId")?;
    let id = required(body, "itemId")?;
    let prior = Cursor::decode(body, "content", thread, turn, "")?;
    let row: Option<(i64,i64,i64)> = db.query_row("SELECT rollout_ordinal,updated_at_ordinal,length(item_json) FROM thread_items WHERE thread_id=?1 AND turn_id=?2 AND item_id=?3",params![thread,turn,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(database)?;
    let (ordinal, version, length) =
        row.ok_or_else(|| error("ITEM_NOT_FOUND", "条目不存在于该轮次"))?;
    if let Some(c) = &prior {
        if c.id != id
            || c.ordinal != ordinal
            || c.item_version != version
            || c.length != length
            || c.offset > length
        {
            return Err(stale());
        }
    }
    let mut cursor = prior.unwrap_or_else(|| {
        let mut c = Cursor::new("content", thread, turn, "", ordinal);
        c.id = id.into();
        c.ordinal = ordinal;
        c.item_version = version;
        c.length = length;
        c
    });
    let offset = cursor.offset;
    let content: String = db.query_row("SELECT substr(item_json,?4,?5) FROM thread_items WHERE thread_id=?1 AND turn_id=?2 AND item_id=?3",params![thread,turn,id,offset+1,CHUNK_CHARS],|r|r.get(0)).map_err(database)?;
    cursor.offset += content.chars().count() as i64;
    Ok(
        json!({"threadId":thread,"turnId":turn,"itemId":id,"version":version,"encoding":"json-text","offset":offset,"totalCharacters":length,"content":content,"nextCursor":(cursor.offset<length).then(||cursor.encode())}),
    )
}

#[cfg(test)]
mod tests;
