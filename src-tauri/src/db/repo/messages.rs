//! messages repo - 消息及消息片段的 CRUD 操作。
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use crate::error::{AppError, AppResult};
use crate::sync::payload::SyncEntityType;

#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub seq: i32,
    pub status: String,
    pub model: Option<String>,
    pub token_count: Option<i64>,
    pub metadata: Option<String>,
    pub parent_id: Option<String>,
    pub selected_child_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessagePartRow {
    pub id: String,
    pub message_id: String,
    pub kind: String,
    pub ordinal: i32,
    pub mime_type: Option<String>,
    pub tool_call_id: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
}

pub struct NewMessage {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub seq: i32,
    pub status: String,
    pub model: Option<String>,
    pub token_count: Option<i64>,
    pub metadata: Option<String>,
    pub parent_id: Option<String>,
    pub selected_child_id: Option<String>,
}

pub struct NewMessagePart {
    pub id: String,
    pub message_id: String,
    pub kind: String,
    pub ordinal: i32,
    pub mime_type: Option<String>,
    pub tool_call_id: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// 计算下条消息的序列号（seq）。
pub fn get_next_seq(conn: &Connection, session_id: &str) -> AppResult<i32> {
    let val: Option<i32> = conn.query_row(
        "SELECT MAX(seq) FROM messages WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )?;
    Ok(val.map(|x| x + 1).unwrap_or(0))
}

/// 统计会话中的消息数。
pub fn count(conn: &Connection, session_id: &str) -> AppResult<u64> {
    let cnt: u64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )?;
    Ok(cnt)
}

/// 获取某个会话中所有的消息（不含片段），按 seq 升序。
pub fn list(conn: &Connection, session_id: &str) -> AppResult<Vec<MessageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, role, seq, status, model, token_count, metadata, \
         parent_id, selected_child_id, created_at, updated_at, version, deleted_at, origin_device_id \
         FROM messages \
         WHERE session_id = ?1 AND deleted_at IS NULL \
         ORDER BY seq ASC, id ASC",
    )?;

    let rows = stmt.query_map([session_id], |r| {
        Ok(MessageRow {
            id: r.get(0)?,
            session_id: r.get(1)?,
            role: r.get(2)?,
            seq: r.get(3)?,
            status: r.get(4)?,
            model: r.get(5)?,
            token_count: r.get(6)?,
            metadata: r.get(7)?,
            parent_id: r.get(8)?,
            selected_child_id: r.get(9)?,
            created_at: r.get(10)?,
            updated_at: r.get(11)?,
            version: r.get(12)?,
            deleted_at: r.get(13)?,
            origin_device_id: r.get(14)?,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 获取某条消息关联的所有消息片段（message_parts），按 ordinal 升序。
pub fn list_parts(conn: &Connection, message_id: &str) -> AppResult<Vec<MessagePartRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, message_id, kind, ordinal, mime_type, tool_call_id, content, metadata \
         FROM message_parts \
         WHERE message_id = ?1 \
         ORDER BY ordinal ASC",
    )?;

    let rows = stmt.query_map([message_id], |r| {
        Ok(MessagePartRow {
            id: r.get(0)?,
            message_id: r.get(1)?,
            kind: r.get(2)?,
            ordinal: r.get(3)?,
            mime_type: r.get(4)?,
            tool_call_id: r.get(5)?,
            content: r.get(6)?,
            metadata: r.get(7)?,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 获取某个会话中所有的消息及消息片段，按 seq 和 ordinal 排序。
pub fn list_with_parts(
    conn: &Connection,
    session_id: &str,
) -> AppResult<Vec<(MessageRow, Vec<MessagePartRow>)>> {
    let msgs = list(conn, session_id)?;
    let mut out = Vec::with_capacity(msgs.len());
    for msg in msgs {
        let parts = list_parts(conn, &msg.id)?;
        out.push((msg, parts));
    }
    Ok(out)
}

/// 插入一条新消息。
pub fn insert(conn: &Connection, m: &NewMessage) -> AppResult<()> {
    let now_str = now();
    let device_id = super::sync::device_id(conn)?;
    conn.execute(
        "INSERT INTO messages (id, session_id, role, seq, status, model, token_count, \
         metadata, parent_id, selected_child_id, created_at, updated_at, version, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, ?13)",
        params![
            m.id,
            m.session_id,
            m.role,
            m.seq,
            m.status,
            m.model,
            m.token_count,
            m.metadata,
            m.parent_id,
            m.selected_child_id,
            now_str,
            now_str,
            device_id,
        ],
    )?;
    Ok(())
}

/// 插入一个消息片段。
pub fn insert_part(conn: &Connection, p: &NewMessagePart) -> AppResult<()> {
    conn.execute(
        "INSERT INTO message_parts (id, message_id, kind, ordinal, mime_type, \
         tool_call_id, content, metadata) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            p.id,
            p.message_id,
            p.kind,
            p.ordinal,
            p.mime_type,
            p.tool_call_id,
            p.content,
            p.metadata,
        ],
    )?;
    Ok(())
}

/// 更新消息状态。
pub fn update_status(conn: &mut Connection, id: &str, status: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    tx.execute(
        "UPDATE messages \
         SET status = ?1, updated_at = ?2, version = version + 1, origin_device_id = ?3 \
         WHERE id = ?4 AND deleted_at IS NULL",
        params![status, now_str, device_id, id],
    )?;
    if status == "complete" {
        enqueue_if_complete(&tx, id)?;
        enqueue_complete_parents_selecting(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// Persist provider-reported usage in message metadata without changing message content.
pub fn update_usage(
    conn: &Connection,
    id: &str,
    input_tokens: i64,
    cached_tokens: i64,
    output_tokens: i64,
    context_tokens: i64,
) -> AppResult<()> {
    let existing = conn
        .query_row("SELECT metadata FROM messages WHERE id = ?1", [id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    let mut metadata = existing
        .as_deref()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    metadata["usage"] = serde_json::json!({
        "input_tokens": input_tokens.max(0),
        "cached_tokens": cached_tokens.max(0),
        "output_tokens": output_tokens.max(0),
        "context_tokens": context_tokens.max(0),
    });
    conn.execute(
        "UPDATE messages SET metadata = ?1 WHERE id = ?2 AND deleted_at IS NULL",
        params![metadata.to_string(), id],
    )?;
    Ok(())
}

pub fn token_usage_totals(conn: &Connection, agent_id: Option<&str>) -> AppResult<(i64, i64, i64)> {
    let mut statement = conn.prepare(
        "SELECT m.metadata FROM messages m \
         JOIN sessions s ON s.id = m.session_id \
         WHERE m.role = 'assistant' AND m.deleted_at IS NULL \
           AND (?1 IS NULL OR s.agent_id = ?1)",
    )?;
    let rows = statement.query_map([agent_id], |row| row.get::<_, Option<String>>(0))?;
    let mut totals = (0_i64, 0_i64, 0_i64);
    for row in rows {
        let Some(metadata) = row? else { continue };
        let Some(usage) = serde_json::from_str::<Value>(&metadata)
            .ok()
            .and_then(|value| value.get("usage").cloned())
        else {
            continue;
        };
        totals.0 += usage
            .get("input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        totals.1 += usage
            .get("cached_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        totals.2 += usage
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
    }
    Ok(totals)
}

/// Update the actual model selected for an in-flight assistant response.
pub fn update_model(conn: &Connection, id: &str, model: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE messages SET model = ?1, updated_at = ?2 \
         WHERE id = ?3 AND role = 'assistant' AND status IN ('pending', 'streaming') \
         AND deleted_at IS NULL",
        params![model, now(), id],
    )?;
    Ok(())
}

/// Finish an interrupted pending response with a visible error instead of an
/// empty assistant bubble. This state is local-only until a later successful
/// completion is explicitly synced.
pub fn fail_pending_assistant(conn: &mut Connection, id: &str, message: &str) -> AppResult<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let status: Option<(String, String)> = tx
        .query_row(
            "SELECT role, status FROM messages WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((role, status)) = status else {
        tx.commit()?;
        return Ok(false);
    };
    if role != "assistant" || !matches!(status.as_str(), "pending" | "streaming") {
        tx.commit()?;
        return Ok(false);
    }

    let ordinal: i32 = tx.query_row(
        "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM message_parts WHERE message_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO message_parts (id, message_id, kind, ordinal, content) VALUES (?1, ?2, 'text', ?3, ?4)",
        params![uuid::Uuid::new_v4().to_string(), id, ordinal, message],
    )?;
    let device_id = super::sync::device_id(&tx)?;
    tx.execute(
        "UPDATE messages SET status = 'failed', updated_at = ?1, version = version + 1, origin_device_id = ?2 WHERE id = ?3",
        params![now(), device_id, id],
    )?;
    tx.commit()?;
    Ok(true)
}

/// Recover stale assistant placeholders left behind by a previous process.
pub fn recover_interrupted_assistants(conn: &mut Connection) -> AppResult<usize> {
    let ids = {
        let mut statement = conn.prepare(
            "SELECT id FROM messages WHERE role = 'assistant' AND status IN ('pending', 'streaming') AND deleted_at IS NULL",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut recovered = 0;
    for id in ids {
        if fail_pending_assistant(conn, &id, "（回复在完成前中断，请重新发送）")? {
            recovered += 1;
        }
    }
    Ok(recovered)
}

// ===== 版本树相关 =====

/// 活动路径上的一条消息（含片段与版本信息）。
#[derive(Debug, Clone)]
pub struct ActivePathMessage {
    pub message: MessageRow,
    pub parts: Vec<MessagePartRow>,
    pub version_index: usize, // 在同级中的序号（0-based）
    pub version_count: usize, // 该分支点的同级总数
    pub is_leaf: bool,        // 无子节点（前端用于禁用删除）
}

#[derive(Debug, Clone)]
pub struct ActivePathResult {
    pub messages: Vec<ActivePathMessage>,
}

/// 按 id 取单条消息。
pub fn get(conn: &Connection, id: &str) -> AppResult<Option<MessageRow>> {
    let res = conn
        .query_row(
            "SELECT id, session_id, role, seq, status, model, token_count, metadata, \
             parent_id, selected_child_id, created_at, updated_at, version, deleted_at, origin_device_id \
             FROM messages WHERE id = ?1 AND deleted_at IS NULL",
            [id],
            |r| {
                Ok(MessageRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    role: r.get(2)?,
                    seq: r.get(3)?,
                    status: r.get(4)?,
                    model: r.get(5)?,
                    token_count: r.get(6)?,
                    metadata: r.get(7)?,
                    parent_id: r.get(8)?,
                    selected_child_id: r.get(9)?,
                    created_at: r.get(10)?,
                    updated_at: r.get(11)?,
                    version: r.get(12)?,
                    deleted_at: r.get(13)?,
                    origin_device_id: r.get(14)?,
                })
            },
        )
        .optional()?;
    Ok(res)
}

/// 统计某条消息的子节点数（用于判断是否叶子）。
pub fn count_children(conn: &Connection, id: &str) -> AppResult<u64> {
    let cnt: u64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE parent_id = ?1 AND deleted_at IS NULL",
        [id],
        |r| r.get(0),
    )?;
    Ok(cnt)
}

/// 设置某消息选中的子消息（切换活动路径）；child_id 为 None 表示该消息变为叶子。
pub fn set_selected_child_local(
    conn: &Connection,
    parent_id: &str,
    child_id: Option<&str>,
) -> AppResult<()> {
    let now_str = now();
    let device_id = super::sync::device_id(conn)?;
    conn.execute(
        "UPDATE messages SET selected_child_id = ?1, updated_at = ?2, version = version + 1, \
         origin_device_id = ?3 WHERE id = ?4 AND deleted_at IS NULL",
        params![child_id, now_str, device_id, parent_id],
    )?;
    Ok(())
}

pub fn set_selected_child(
    conn: &mut Connection,
    parent_id: &str,
    child_id: Option<&str>,
) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    set_selected_child_local(&tx, parent_id, child_id)?;
    enqueue_if_complete(&tx, parent_id)?;
    tx.commit()?;
    Ok(())
}

/// 删除某消息的全部片段。
pub fn delete_parts(conn: &Connection, message_id: &str) -> AppResult<()> {
    conn.execute(
        "DELETE FROM message_parts WHERE message_id = ?1",
        [message_id],
    )?;
    Ok(())
}

pub fn mark_content_updated(conn: &Connection, message_id: &str) -> AppResult<()> {
    let device_id = super::sync::device_id(conn)?;
    conn.execute(
        "UPDATE messages SET updated_at = ?1, version = version + 1, origin_device_id = ?2 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![now(), device_id, message_id],
    )?;
    Ok(())
}

/// Soft-delete a leaf message and clear any active parent pointer to it.
pub fn delete_message(conn: &mut Connection, id: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let parent_ids = {
        let mut statement = tx.prepare(
            "SELECT id FROM messages WHERE selected_child_id = ?1 AND deleted_at IS NULL",
        )?;
        let rows = statement
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    tx.execute(
        "UPDATE messages SET selected_child_id = NULL, updated_at = ?1, version = version + 1, \
         origin_device_id = ?2 WHERE selected_child_id = ?3 AND deleted_at IS NULL",
        params![now_str, device_id, id],
    )?;
    for parent_id in parent_ids {
        enqueue_if_complete(&tx, &parent_id)?;
    }
    let changed = tx.execute(
        "UPDATE messages SET deleted_at = ?1, updated_at = ?1, version = version + 1, \
         origin_device_id = ?2 WHERE id = ?3 AND deleted_at IS NULL",
        params![now_str, device_id, id],
    )?;
    if changed > 0 {
        enqueue_if_complete(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

fn get_any(conn: &Connection, id: &str) -> AppResult<Option<MessageRow>> {
    conn.query_row(
        "SELECT id, session_id, role, seq, status, model, token_count, metadata, \
         parent_id, selected_child_id, created_at, updated_at, version, deleted_at, origin_device_id \
         FROM messages WHERE id = ?1",
        [id],
        |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                seq: row.get(3)?,
                status: row.get(4)?,
                model: row.get(5)?,
                token_count: row.get(6)?,
                metadata: row.get(7)?,
                parent_id: row.get(8)?,
                selected_child_id: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
                version: row.get(12)?,
                deleted_at: row.get(13)?,
                origin_device_id: row.get(14)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub(crate) fn enqueue_if_complete(conn: &Connection, id: &str) -> AppResult<bool> {
    let Some(row) = get_any(conn, id)? else {
        return Err(AppError::Other(format!(
            "message `{id}` disappeared during sync enqueue"
        )));
    };
    if row.status != "complete" {
        return Ok(false);
    }

    let mut source = serde_json::to_value(&row)?;
    let object = source
        .as_object_mut()
        .ok_or_else(|| AppError::Other("message sync source is not an object".into()))?;
    if let Some(child_id) = row.selected_child_id.as_deref() {
        let child_complete = conn
            .query_row(
                "SELECT status = 'complete' AND deleted_at IS NULL FROM messages WHERE id = ?1",
                [child_id],
                |query_row| query_row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if !child_complete {
            object.insert("selected_child_id".into(), Value::Null);
        }
    }
    object.insert("parts".into(), serde_json::to_value(list_parts(conn, id)?)?);
    super::sync::enqueue_projection(
        conn,
        SyncEntityType::Message,
        &row.id,
        row.version,
        row.deleted_at.is_some(),
        &source,
    )?;
    Ok(true)
}

fn enqueue_complete_parents_selecting(conn: &Connection, child_id: &str) -> AppResult<()> {
    let parent_ids = {
        let mut statement = conn.prepare(
            "SELECT id FROM messages WHERE selected_child_id = ?1 AND deleted_at IS NULL",
        )?;
        let rows = statement
            .query_map([child_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for parent_id in parent_ids {
        enqueue_if_complete(conn, &parent_id)?;
    }
    Ok(())
}

/// 计算会话当前活动路径（root → selected_child_id → … → 叶子）。
///
/// 一次查询全部消息，内存里按 parent_id 分组同级，沿 selected_child_id 走链。
/// 每条附 version_index/version_count（同级数，供版本切换器）与 is_leaf。
pub fn list_active_with_parts(conn: &Connection, session_id: &str) -> AppResult<ActivePathResult> {
    use std::collections::{HashMap, HashSet};

    let all = list(conn, session_id)?;
    if all.is_empty() {
        return Ok(ActivePathResult { messages: vec![] });
    }

    // 按 parent_id 分组同级（NULL 用哨兵 "__root__"）
    let mut siblings: HashMap<String, Vec<&MessageRow>> = HashMap::new();
    for m in &all {
        let key = m
            .parent_id
            .clone()
            .unwrap_or_else(|| "__root__".to_string());
        siblings.entry(key).or_default().push(m);
    }
    let has_children: HashSet<String> = all.iter().filter_map(|m| m.parent_id.clone()).collect();

    // 根 = parent_id 为 NULL 且 seq 最小
    let mut cur_opt = all
        .iter()
        .filter(|m| m.parent_id.is_none())
        .min_by_key(|m| m.seq);
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    while let Some(cur) = cur_opt {
        if !visited.insert(cur.id.clone()) {
            break;
        }
        let key = cur
            .parent_id
            .clone()
            .unwrap_or_else(|| "__root__".to_string());
        let group = siblings.get(&key).cloned().unwrap_or_default();
        let vidx = group.iter().position(|m| m.id == cur.id).unwrap_or(0);
        let vcount = group.len();
        let parts = list_parts(conn, &cur.id)?;
        out.push(ActivePathMessage {
            message: cur.clone(),
            parts,
            version_index: vidx,
            version_count: vcount,
            is_leaf: !has_children.contains(&cur.id),
        });
        // 沿 selected_child_id 继续走链
        cur_opt = match &cur.selected_child_id {
            Some(cid) => all.iter().find(|m| m.id == *cid),
            None => None,
        };
    }

    Ok(ActivePathResult { messages: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent-1', 'Agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, title) VALUES ('session-1', 'agent-1', 'Session')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) \
             VALUES (1, '00000000-0000-4000-8000-000000000001')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn deleting_a_message_keeps_a_versioned_tombstone() {
        let mut conn = setup();
        insert(
            &conn,
            &NewMessage {
                id: "message-1".into(),
                session_id: "session-1".into(),
                role: "user".into(),
                seq: 0,
                status: "complete".into(),
                model: None,
                token_count: None,
                metadata: None,
                parent_id: None,
                selected_child_id: None,
            },
        )
        .unwrap();
        delete_message(&mut conn, "message-1").unwrap();

        assert!(get(&conn, "message-1").unwrap().is_none());
        let (version, deleted_at): (i64, Option<String>) = conn
            .query_row(
                "SELECT version, deleted_at FROM messages WHERE id = 'message-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(version, 2);
        assert!(deleted_at.is_some());
        let operation: String = conn
            .query_row(
                "SELECT operation FROM sync_outbox WHERE entity_id = 'message-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(operation, "delete");
    }

    #[test]
    fn pending_message_is_enqueued_only_after_completion_with_public_text_parts() {
        let mut conn = setup();
        insert(
            &conn,
            &NewMessage {
                id: "message-1".into(),
                session_id: "session-1".into(),
                role: "assistant".into(),
                seq: 4,
                status: "pending".into(),
                model: Some("private/model".into()),
                token_count: None,
                metadata: Some("private metadata".into()),
                parent_id: None,
                selected_child_id: None,
            },
        )
        .unwrap();
        for (id, kind, ordinal, content) in [
            ("part-text", "text", 1, "Visible answer"),
            ("part-reasoning", "reasoning", 0, "Private reasoning"),
        ] {
            insert_part(
                &conn,
                &NewMessagePart {
                    id: id.into(),
                    message_id: "message-1".into(),
                    kind: kind.into(),
                    ordinal,
                    mime_type: None,
                    tool_call_id: None,
                    content: content.into(),
                    metadata: None,
                },
            )
            .unwrap();
        }
        assert!(!enqueue_if_complete(&conn, "message-1").unwrap());
        update_status(&mut conn, "message-1", "complete").unwrap();

        let payload: String = conn
            .query_row(
                "SELECT payload FROM sync_outbox WHERE entity_id = 'message-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(payload.contains("Visible answer"));
        assert!(payload.contains("\"seq\":4"));
        for private in ["Private reasoning", "private/model", "private metadata"] {
            assert!(!payload.contains(private));
        }
    }

    #[test]
    fn fallback_updates_only_an_in_flight_assistant_model() {
        let mut conn = setup();
        insert(
            &conn,
            &NewMessage {
                id: "message-fallback".into(),
                session_id: "session-1".into(),
                role: "assistant".into(),
                seq: 1,
                status: "pending".into(),
                model: Some("primary/model".into()),
                token_count: None,
                metadata: None,
                parent_id: None,
                selected_child_id: None,
            },
        )
        .unwrap();
        update_model(&conn, "message-fallback", "backup/model").unwrap();
        assert_eq!(
            get(&conn, "message-fallback")
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("backup/model")
        );

        update_status(&mut conn, "message-fallback", "complete").unwrap();
        update_model(&conn, "message-fallback", "ignored/model").unwrap();
        assert_eq!(
            get(&conn, "message-fallback")
                .unwrap()
                .unwrap()
                .model
                .as_deref(),
            Some("backup/model")
        );
    }

    #[test]
    fn usage_metadata_round_trips_and_totals_completed_assistant_calls() {
        let conn = setup();
        insert(
            &conn,
            &NewMessage {
                id: "message-usage".into(),
                session_id: "session-1".into(),
                role: "assistant".into(),
                seq: 1,
                status: "complete".into(),
                model: Some("provider/model".into()),
                token_count: None,
                metadata: None,
                parent_id: None,
                selected_child_id: None,
            },
        )
        .unwrap();

        update_usage(&conn, "message-usage", 120, 80, 30, 150).unwrap();

        let metadata: Value = serde_json::from_str(
            get(&conn, "message-usage")
                .unwrap()
                .unwrap()
                .metadata
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(metadata["usage"]["context_tokens"], 150);
        assert_eq!(token_usage_totals(&conn, None).unwrap(), (120, 80, 30));
        assert_eq!(
            token_usage_totals(&conn, Some("agent-1")).unwrap(),
            (120, 80, 30)
        );
        assert_eq!(
            token_usage_totals(&conn, Some("missing-agent")).unwrap(),
            (0, 0, 0)
        );
    }

    #[test]
    fn parent_projects_pending_selected_child_as_null_until_child_completes() {
        let mut conn = setup();
        for (id, role, status, parent_id) in [
            ("message-parent", "user", "complete", None),
            (
                "message-child",
                "assistant",
                "pending",
                Some("message-parent"),
            ),
        ] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: "session-1".into(),
                    role: role.into(),
                    seq: if parent_id.is_none() { 1 } else { 2 },
                    status: status.into(),
                    model: None,
                    token_count: None,
                    metadata: None,
                    parent_id: parent_id.map(str::to_string),
                    selected_child_id: None,
                },
            )
            .unwrap();
        }

        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        set_selected_child_local(&tx, "message-parent", Some("message-child")).unwrap();
        enqueue_if_complete(&tx, "message-parent").unwrap();
        tx.commit().unwrap();

        let pending_payload: String = conn
            .query_row(
                "SELECT payload FROM sync_outbox WHERE entity_id = 'message-parent' \
                 ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(pending_payload.contains("\"selected_child_id\":null"));

        update_status(&mut conn, "message-child", "complete").unwrap();
        let completed_payload: String = conn
            .query_row(
                "SELECT payload FROM sync_outbox WHERE entity_id = 'message-parent' \
                 ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(completed_payload.contains("\"selected_child_id\":\"message-child\""));
    }

    #[test]
    fn failed_or_recovered_pending_assistants_gain_visible_text() {
        let mut conn = setup();
        for (id, status) in [
            ("message-pending", "pending"),
            ("message-streaming", "streaming"),
        ] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: "session-1".into(),
                    role: "assistant".into(),
                    seq: if status == "pending" { 1 } else { 2 },
                    status: status.into(),
                    model: None,
                    token_count: None,
                    metadata: None,
                    parent_id: None,
                    selected_child_id: None,
                },
            )
            .unwrap();
        }

        assert!(fail_pending_assistant(&mut conn, "message-pending", "Connection lost").unwrap());
        assert_eq!(
            get(&conn, "message-pending").unwrap().unwrap().status,
            "failed"
        );
        assert_eq!(
            list_parts(&conn, "message-pending").unwrap()[0].content,
            "Connection lost"
        );
        assert_eq!(recover_interrupted_assistants(&mut conn).unwrap(), 1);
        assert_eq!(
            get(&conn, "message-streaming").unwrap().unwrap().status,
            "failed"
        );
        assert_eq!(
            list_parts(&conn, "message-streaming").unwrap()[0].content,
            "（回复在完成前中断，请重新发送）"
        );
    }
}
