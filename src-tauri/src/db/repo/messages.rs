//! messages repo - 消息及消息片段的 CRUD 操作。
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
         ORDER BY seq ASC",
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
    conn.execute(
        "INSERT INTO messages (id, session_id, role, seq, status, model, token_count, \
         metadata, parent_id, selected_child_id, created_at, updated_at, version) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)",
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
pub fn update_status(conn: &Connection, id: &str, status: &str) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE messages \
         SET status = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![status, now_str, id],
    )?;
    Ok(())
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
pub fn set_selected_child(conn: &Connection, parent_id: &str, child_id: Option<&str>) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE messages SET selected_child_id = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![child_id, now_str, parent_id],
    )?;
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
    conn.execute(
        "UPDATE messages SET updated_at = ?1, version = version + 1 \
         WHERE id = ?2 AND deleted_at IS NULL",
        params![now(), message_id],
    )?;
    Ok(())
}

/// Soft-delete a leaf message and clear any active parent pointer to it.
pub fn delete_message(conn: &mut Connection, id: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE messages SET selected_child_id = NULL, updated_at = ?1, version = version + 1 \
         WHERE selected_child_id = ?2 AND deleted_at IS NULL",
        params![now_str, id],
    )?;
    tx.execute(
        "UPDATE messages SET deleted_at = ?1, updated_at = ?1, version = version + 1 \
         WHERE id = ?2 AND deleted_at IS NULL",
        params![now_str, id],
    )?;
    tx.commit()?;
    Ok(())
}

/// 计算会话当前活动路径（root → selected_child_id → … → 叶子）。
///
/// 一次查询全部消息，内存里按 parent_id 分组同级，沿 selected_child_id 走链。
/// 每条附 version_index/version_count（同级数，供版本切换器）与 is_leaf。
pub fn list_active_with_parts(
    conn: &Connection,
    session_id: &str,
) -> AppResult<ActivePathResult> {
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
    let mut cur_opt = all.iter().filter(|m| m.parent_id.is_none()).min_by_key(|m| m.seq);
    let mut out = Vec::new();
    while let Some(cur) = cur_opt {
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

    #[test]
    fn deleting_a_message_keeps_a_versioned_tombstone() {
        let mut conn = Connection::open_in_memory().unwrap();
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
    }
}
