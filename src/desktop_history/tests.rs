use super::*;

#[test]
fn native_store_is_read_only_and_missing_stores_are_not_created() {
    let dir = std::env::temp_dir().join(format!("history-{}", crate::random_event_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("history.sqlite");
    assert!(open_history(&path).is_err());
    assert!(!path.exists());
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('unchanged')",
        )
        .unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    {
        let db = open_history(&path).unwrap();
        assert_eq!(
            db.query_row("SELECT value FROM sentinel", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "unchanged"
        );
        assert!(db
            .execute("UPDATE sentinel SET value='changed'", [])
            .is_err());
    }
    assert_eq!(before, std::fs::read(&path).unwrap());
    std::fs::remove_dir_all(dir).unwrap();
}

fn fixture() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE thread_turns(thread_id TEXT,turn_id TEXT,rollout_ordinal INTEGER,status TEXT,error_json TEXT,started_at INTEGER,completed_at INTEGER,duration_ms INTEGER,first_user_item_id TEXT,final_agent_item_id TEXT,PRIMARY KEY(thread_id,turn_id)); CREATE UNIQUE INDEX turns_page ON thread_turns(thread_id,rollout_ordinal); CREATE TABLE thread_items(thread_id TEXT,turn_id TEXT,item_id TEXT,rollout_ordinal INTEGER,updated_at_ordinal INTEGER,item_type TEXT,item_json TEXT,PRIMARY KEY(thread_id,turn_id,item_id)); CREATE UNIQUE INDEX items_page ON thread_items(thread_id,rollout_ordinal); CREATE INDEX items_turn_page ON thread_items(thread_id,turn_id,rollout_ordinal);").unwrap();
    for n in 1..=4 {
        turn(&db, n);
    }
    db
}
fn turn(db: &Connection, n: i64) {
    let id = format!("t{n}");
    db.execute(
        "INSERT INTO thread_turns VALUES('thread',?1,?2,'completed',NULL,1,2,1000,'u','a')",
        params![id, n * 100],
    )
    .unwrap();
    item(
        db,
        &id,
        "u",
        n * 100 + 1,
        "userMessage",
        json!({"type":"userMessage","id":"u","content":[{"type":"text","text":format!("question {n}")}]}),
    );
    item(
        db,
        &id,
        "a",
        n * 100 + 3,
        "agentMessage",
        json!({"type":"agentMessage","id":"a","text":format!("answer {n}")}),
    );
}
fn item(db: &Connection, turn: &str, id: &str, ordinal: i64, ty: &str, v: Value) {
    db.execute(
        "INSERT INTO thread_items VALUES('thread',?1,?2,?3,?3,?4,?5)",
        params![turn, id, ordinal, ty, v.to_string()],
    )
    .unwrap();
}
fn next(body: &Value, result: &Value) -> Value {
    let mut b = body.clone();
    b["cursor"] = result["nextCursor"].clone();
    b
}
#[test]
fn latest_one_is_bounded_and_does_not_include_tool_payloads() {
    let db = fixture();
    item(
        &db,
        "t4",
        "huge",
        402,
        "mcpToolCall",
        json!({"type":"mcpToolCall","id":"huge","tool":"test","result":{"output":"x".repeat(4*1024*1024)}}),
    );
    let page = list_turns(&db, &json!({"threadId":"thread"})).unwrap();
    assert_eq!(page["data"].as_array().unwrap().len(), 1);
    assert_eq!(page["data"][0]["turnId"], "t4");
    assert_eq!(page["data"][0]["itemCount"], 3);
    assert_eq!(page["data"][0]["items"][0]["preview"], "question 4");
    assert_eq!(page["data"][0]["items"][1]["preview"], "answer 4");
    assert!(page.to_string().len() < 3000);
    let items = list_items(&db, &json!({"threadId":"thread","turnId":"t4"})).unwrap();
    assert_eq!(items["data"][1]["id"], "huge");
    assert!(items["data"][1]["contentBytes"].as_u64().unwrap() > 4 * 1024 * 1024);
    assert!(items.to_string().len() < 3000);
}
#[test]
fn stable_turn_cursors_do_not_shift_when_new_turns_arrive() {
    let db = fixture();
    let body = json!({"threadId":"thread","limit":2});
    let first = list_turns(&db, &body).unwrap();
    turn(&db, 5);
    let second = list_turns(&db, &next(&body, &first)).unwrap();
    assert_eq!(second["data"][0]["turnId"], "t2");
    assert_eq!(second["data"][1]["turnId"], "t1");
    assert!(second["nextCursor"].is_null());
    let asc = json!({"threadId":"thread","limit":2,"sortDirection":"asc"});
    let first = list_turns(&db, &asc).unwrap();
    turn(&db, 6);
    let second = list_turns(&db, &next(&asc, &first)).unwrap();
    let third = list_turns(&db, &next(&asc, &second)).unwrap();
    assert_eq!(third["data"][0]["turnId"], "t5");
    assert!(third["nextCursor"].is_null());
}
#[test]
fn cursors_are_scoped_and_deleted_anchors_are_rejected() {
    let db = fixture();
    let body = json!({"threadId":"thread"});
    let first = list_turns(&db, &body).unwrap();
    let mut b = next(&body, &first);
    b["threadId"] = json!("different");
    assert!(list_turns(&db, &b).is_err());
    b = next(&body, &first);
    b["sortDirection"] = json!("asc");
    assert!(list_turns(&db, &b).is_err());
    db.execute("DELETE FROM thread_turns WHERE turn_id='t4'", [])
        .unwrap();
    assert_eq!(
        list_turns(&db, &next(&body, &first)).unwrap_err().code,
        Some(json!("HISTORY_CURSOR_STALE"))
    );
    for cursor in ["1", "zz", "汉字", "ff"] {
        assert!(list_turns(&db, &json!({"threadId":"thread","cursor":cursor})).is_err());
    }
}
#[test]
fn item_pagination_is_scoped_and_stable() {
    let db = fixture();
    let body = json!({"threadId":"thread","turnId":"t4","limit":1});
    let first = list_items(&db, &body).unwrap();
    item(
        &db,
        "t4",
        "new",
        404,
        "agentMessage",
        json!({"type":"agentMessage","text":"new"}),
    );
    let second = list_items(&db, &next(&body, &first)).unwrap();
    assert_eq!(second["data"][0]["id"], "a");
    assert!(second["nextCursor"].is_null());
    let mut b = next(&body, &first);
    b["turnId"] = json!("t3");
    assert!(list_items(&db, &b).is_err());
    assert!(list_items(&db, &json!({"threadId":"thread","turnId":"missing"})).is_err());
}
#[test]
fn content_is_losslessly_chunked_and_mutation_invalidates_continuation() {
    let db = fixture();
    let value = json!({"type":"mcpToolCall","id":"big","result":"中文😀\n\"\\".repeat(9000)});
    item(&db, "t4", "big", 402, "mcpToolCall", value.clone());
    let body = json!({"threadId":"thread","turnId":"t4","itemId":"big"});
    let mut request = body.clone();
    let mut combined = String::new();
    let mut chunks = 0;
    loop {
        let result = read_item(&db, &request).unwrap();
        chunks += 1;
        assert!(result.to_string().len() < 64 * 1024);
        combined.push_str(result["content"].as_str().unwrap());
        if result["nextCursor"].is_null() {
            break;
        }
        request = next(&body, &result);
    }
    assert!(chunks > 1);
    assert_eq!(serde_json::from_str::<Value>(&combined).unwrap(), value);
    let first = read_item(&db, &body).unwrap();
    db.execute(
        "UPDATE thread_items SET updated_at_ordinal=999 WHERE item_id='big'",
        [],
    )
    .unwrap();
    assert_eq!(
        read_item(&db, &next(&body, &first)).unwrap_err().code,
        Some(json!("HISTORY_CURSOR_STALE"))
    );
}
#[test]
fn page_byte_budget_is_enforced_without_losing_next_row() {
    let db = fixture();
    for n in 5..=30 {
        turn(&db, n);
    }
    let v = json!({"type":"agentMessage","text":"\u{1}".repeat(3000)}).to_string();
    db.execute(
        "UPDATE thread_items SET item_json=?1 WHERE item_type='agentMessage'",
        [v],
    )
    .unwrap();
    let body = json!({"threadId":"thread","limit":100});
    let mut request = body.clone();
    let mut ids = vec![];
    loop {
        let result = list_turns(&db, &request).unwrap();
        assert!(result.to_string().len() < PAGE_BYTES + 8192);
        ids.extend(
            result["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["turnId"].as_str().unwrap().to_string()),
        );
        if result["nextCursor"].is_null() {
            break;
        }
        request = next(&body, &result);
    }
    assert_eq!(ids.len(), 30);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 30);
}
#[test]
fn not_loaded_and_empty_history_and_invalid_inputs_are_explicit() {
    let db = fixture();
    let result = list_turns(&db, &json!({"threadId":"thread","itemsView":"notLoaded"})).unwrap();
    assert_eq!(result["data"][0]["items"], json!([]));
    assert_eq!(
        list_turns(&db, &json!({"threadId":"none"})).unwrap()["data"],
        json!([])
    );
    for b in [
        json!({"threadId":"thread","limit":0}),
        json!({"threadId":"thread","limit":101}),
        json!({"threadId":"thread","itemsView":"full"}),
    ] {
        assert!(list_turns(&db, &b).is_err());
    }
}

#[test]
#[ignore = "explicit read-only native history verification; set CODEX_HISTORY_TEST_THREAD"]
fn native_history_readonly_probe() {
    let thread = std::env::var("CODEX_HISTORY_TEST_THREAD").expect("test thread required");
    let started = std::time::Instant::now();
    let result = invoke("listThreadTurns", &json!({"threadId":thread})).unwrap();
    assert!(result["data"].as_array().unwrap().len() <= 1);
    println!(
        "latest turn: {} bytes, {} ms; persistence={}",
        result.to_string().len(),
        started.elapsed().as_millis(),
        result["persistence"]
    );
    if let Some(turn) = result["data"].as_array().unwrap().first() {
        let items = invoke(
            "listThreadItems",
            &json!({"threadId":thread,"turnId":turn["turnId"]}),
        )
        .unwrap();
        println!(
            "item page: {} items, {} bytes",
            items["data"].as_array().unwrap().len(),
            items.to_string().len()
        );
        assert!(items.to_string().len() < PAGE_BYTES + 8192);
    }
}
