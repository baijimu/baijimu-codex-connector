//! Read-only desktop metadata index. Never opens rollouts for writing or starts a runtime.
use crate::{process_runtime, HttpError};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::PathBuf;
fn open() -> Result<Connection, HttpError> {
    let path = std::env::var_os("CODEX_DESKTOP_STATE_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| process_runtime::system_codex_home().join("state_5.sqlite"));
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| {
        HttpError::coded(
            503,
            format!("桌面元数据索引不可读：{e}"),
            "DESKTOP_INDEX_UNAVAILABLE",
            json!({}),
        )
    })
}
pub(crate) fn list(body: &Value) -> Result<Value, HttpError> {
    query(&open()?, body)
}
fn query(db: &Connection, body: &Value) -> Result<Value, HttpError> {
    let limit = body["limit"].as_u64().unwrap_or(50).clamp(1, 100) as i64;
    let offset = match body.get("cursor") {
        None | Some(Value::Null) => 0,
        Some(v) => v
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|v| *v >= 0)
            .ok_or_else(|| HttpError::new(400, "invalid cursor"))?,
    };
    let asc = body["sortDirection"] == "asc";
    let key = if body["sortKey"] == "created_at" {
        "created_at"
    } else {
        "updated_at"
    };
    let order = if asc { "ASC" } else { "DESC" };
    let sql=format!("SELECT id,cwd,title,created_at,updated_at,source,model_provider,archived FROM threads WHERE archived=?1 AND (?2='' OR instr(lower(title),lower(?2))>0) AND (?3='' OR cwd=?3) ORDER BY {key} {order},id {order} LIMIT ?4 OFFSET ?5");
    let mut stmt = db.prepare(&sql).map_err(schema)?;
    let rows=stmt.query_map(params![body["archived"].as_bool().unwrap_or(false),body["searchTerm"].as_str().unwrap_or(""),body["cwd"].as_str().unwrap_or(""),limit+1,offset],|r|Ok(json!({"id":r.get::<_,String>(0)?,"cwd":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?,"createdAt":r.get::<_,i64>(3)?,"updatedAt":r.get::<_,i64>(4)?,"source":r.get::<_,String>(5)?,"modelProvider":r.get::<_,String>(6)?,"archived":r.get::<_,bool>(7)?,"metadataOnly":true}))).map_err(schema)?;
    let mut data = rows.collect::<Result<Vec<_>, _>>().map_err(schema)?;
    let more = data.len() > limit as usize;
    data.truncate(limit as usize);
    Ok(
        json!({"data":data,"nextCursor":more.then(||(offset+limit).to_string()),"source":"desktop-readonly-index"}),
    )
}
fn schema(e: rusqlite::Error) -> HttpError {
    HttpError::coded(
        503,
        format!("桌面索引协议不兼容：{e}"),
        "DESKTOP_INDEX_INCOMPATIBLE",
        json!({}),
    )
}
pub(crate) fn projects() -> Result<Value, HttpError> {
    let db = open()?;
    let mut stmt = db
        .prepare("SELECT DISTINCT cwd FROM threads WHERE archived=0 ORDER BY cwd")
        .map_err(schema)?;
    let data = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(schema)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(schema)?;
    Ok(
        json!({"data":data.into_iter().map(|cwd|json!({"path":cwd,"source":"desktop-readonly-index"})).collect::<Vec<_>>()}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_search_is_parameterized_and_paginated() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE threads(id TEXT,cwd TEXT,title TEXT,created_at INTEGER,updated_at INTEGER,source TEXT,model_provider TEXT,archived INTEGER);INSERT INTO threads VALUES('a','/p','one',1,2,'vscode','openai',0),('b','/p','two',2,3,'vscode','openai',0);").unwrap();
        let r = query(&c, &json!({"limit":1})).unwrap();
        assert_eq!(r["data"][0]["id"], "b");
        assert_eq!(r["nextCursor"], "1");
        assert_eq!(
            query(&c, &json!({"searchTerm":"' OR 1=1 --"})).unwrap()["data"],
            json!([])
        );
    }
}
