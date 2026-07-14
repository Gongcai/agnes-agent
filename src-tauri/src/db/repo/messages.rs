//! messages repo - 消息及消息片段的 CRUD 操作。
use rusqlite::{params, Connection};

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
    pub created_at: String,
    pub updated_at: String,
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
         created_at, updated_at \
         FROM messages \
         WHERE session_id = ?1 \
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
            created_at: r.get(8)?,
            updated_at: r.get(9)?,
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
         metadata, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            m.id,
            m.session_id,
            m.role,
            m.seq,
            m.status,
            m.model,
            m.token_count,
            m.metadata,
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
         SET status = ?1, updated_at = ?2 \
         WHERE id = ?3",
        params![status, now_str, id],
    )?;
    Ok(())
}
