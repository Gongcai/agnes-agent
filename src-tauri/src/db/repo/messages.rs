//! messages repo - 消息及消息片段的 CRUD 操作。
use std::collections::BTreeMap;

use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenUsageDay {
    pub date: String,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationSearchResult {
    pub session_id: String,
    pub session_title: String,
    pub message_id: String,
    pub role: String,
    pub seq: i32,
    pub created_at: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationMessageResult {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub seq: i32,
    pub created_at: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationReadResult {
    pub session_id: String,
    pub session_title: String,
    pub messages: Vec<ConversationMessageResult>,
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

#[derive(Debug, Clone)]
pub struct NewUserMessagePart {
    pub kind: String,
    pub mime_type: Option<String>,
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

/// Search completed visible text for sessions owned by one Agent.
pub fn search_conversation(
    conn: &Connection,
    agent_id: &str,
    session_id: Option<&str>,
    query: &str,
    role: Option<&str>,
    limit: usize,
) -> AppResult<Vec<ConversationSearchResult>> {
    let can_use_substring_fts = query.chars().count() >= 3
        && query
            .chars()
            .all(|character| !character.is_control() && character != '"')
        && query.chars().any(char::is_alphanumeric);
    let fts_query = format!("\"{}\"", query.replace('"', "\"\""));
    if can_use_substring_fts {
        return search_conversation_with_fts(
            conn,
            "message_parts_fts_substring",
            agent_id,
            session_id,
            role,
            &fts_query,
            query,
            limit,
        );
    }

    let can_use_fts = query
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character.is_ascii_whitespace())
        && query
            .chars()
            .any(|character| character.is_ascii_alphanumeric());
    if can_use_fts && !can_use_substring_fts {
        let results = search_conversation_with_fts(
            conn,
            "message_parts_fts",
            agent_id,
            session_id,
            role,
            &fts_query,
            query,
            limit,
        )?;
        if !results.is_empty() {
            return Ok(results);
        }
    }

    // Short terms and unusual token boundaries still need the substring fallback.
    search_conversation_by_substring(conn, agent_id, session_id, query, role, limit)
}

fn search_conversation_with_fts(
    conn: &Connection,
    fts_table: &str,
    agent_id: &str,
    session_id: Option<&str>,
    role: Option<&str>,
    fts_query: &str,
    query: &str,
    limit: usize,
) -> AppResult<Vec<ConversationSearchResult>> {
    // The table name is selected only by private call sites; message content
    // remains a bound parameter and never becomes SQL.
    let sql = format!(
        "SELECT m.id, m.session_id, COALESCE(s.title, ''), m.role, m.seq, m.created_at, \
         GROUP_CONCAT(p.content, char(10) || char(10)) \
         FROM {fts_table} f \
         JOIN message_parts p ON p.rowid = f.rowid AND p.kind = 'text' \
         JOIN messages m ON m.id = p.message_id \
         JOIN sessions s ON s.id = m.session_id \
         WHERE s.agent_id = ?1 \
           AND s.deleted_at IS NULL \
           AND m.deleted_at IS NULL \
           AND m.status = 'complete' \
           AND (?2 IS NULL OR m.session_id = ?2) \
           AND (?3 IS NULL OR m.role = ?3) \
           AND f.content MATCH ?4 \
         GROUP BY m.id, m.session_id, s.title, m.role, m.seq, m.created_at \
         ORDER BY m.updated_at DESC, m.seq DESC, m.id DESC \
         LIMIT ?5"
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map(
        params![agent_id, session_id, role, fts_query, limit as i64],
        |row| conversation_search_row(row, query),
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn search_conversation_by_substring(
    conn: &Connection,
    agent_id: &str,
    session_id: Option<&str>,
    query: &str,
    role: Option<&str>,
    limit: usize,
) -> AppResult<Vec<ConversationSearchResult>> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.session_id, COALESCE(s.title, ''), m.role, m.seq, m.created_at, \
         GROUP_CONCAT(p.content, char(10) || char(10)) \
         FROM sessions s \
         JOIN messages m ON m.session_id = s.id \
         JOIN message_parts p ON p.message_id = m.id AND p.kind = 'text' \
         WHERE s.agent_id = ?1 \
           AND s.deleted_at IS NULL \
           AND m.deleted_at IS NULL \
           AND m.status = 'complete' \
           AND (?2 IS NULL OR m.session_id = ?2) \
           AND (?3 IS NULL OR m.role = ?3) \
           AND instr(lower(COALESCE(p.content, '')), lower(?4)) > 0 \
         GROUP BY m.id, m.session_id, s.title, m.role, m.seq, m.created_at \
         ORDER BY m.updated_at DESC, m.seq DESC, m.id DESC \
         LIMIT ?5",
    )?;

    let rows = stmt.query_map(
        params![agent_id, session_id, role, query, limit as i64],
        |row| conversation_search_row(row, query),
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn conversation_search_row(
    row: &Row<'_>,
    query: &str,
) -> rusqlite::Result<ConversationSearchResult> {
    let content = row.get::<_, Option<String>>(6)?.unwrap_or_default();
    Ok(ConversationSearchResult {
        session_id: row.get(1)?,
        session_title: row.get(2)?,
        message_id: row.get(0)?,
        role: row.get(3)?,
        seq: row.get(4)?,
        created_at: row.get(5)?,
        snippet: conversation_snippet(&content, query),
    })
}

/// Read visible text around a search result, or the latest messages in a session.
pub fn read_conversation(
    conn: &Connection,
    agent_id: &str,
    session_id: &str,
    message_id: Option<&str>,
    before: usize,
    after: usize,
    max_messages: usize,
) -> AppResult<Option<ConversationReadResult>> {
    let session_title = conn
        .query_row(
            "SELECT COALESCE(s.title, '') FROM sessions s \
             WHERE s.id = ?1 AND s.agent_id = ?2 AND s.deleted_at IS NULL",
            params![session_id, agent_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(session_title) = session_title else {
        return Ok(None);
    };

    let (messages, anchor) = if let Some(message_id) = message_id {
        let anchor_seq = conn
            .query_row(
                "SELECT m.seq FROM messages m \
                 JOIN sessions s ON s.id = m.session_id \
                 WHERE m.id = ?1 AND s.id = ?2 AND s.agent_id = ?3 \
                   AND s.deleted_at IS NULL AND m.deleted_at IS NULL \
                   AND m.status = 'complete' \
                   AND EXISTS (\
                     SELECT 1 FROM message_parts p \
                     WHERE p.message_id = m.id AND p.kind = 'text' \
                       AND p.content IS NOT NULL AND trim(p.content) <> ''\
                   )",
                params![message_id, session_id, agent_id],
                |row| row.get::<_, i32>(0),
            )
            .optional()?;
        let Some(anchor_seq) = anchor_seq else {
            return Ok(None);
        };

        let mut before_stmt = conn.prepare(
            r#"
            WITH selected_messages AS (
              SELECT m.id, m.session_id, m.role, m.seq, m.created_at
              FROM messages m
              JOIN sessions s ON s.id = m.session_id
              WHERE s.id = ?1 AND s.agent_id = ?2
                AND s.deleted_at IS NULL AND m.deleted_at IS NULL
                AND m.status = 'complete'
                AND (m.seq < ?3 OR (m.seq = ?3 AND m.id < ?4))
                AND EXISTS (
                  SELECT 1 FROM message_parts p
                  WHERE p.message_id = m.id AND p.kind = 'text'
                    AND p.content IS NOT NULL AND trim(p.content) <> ''
                )
              ORDER BY m.seq DESC, m.id DESC
              LIMIT ?5
            )
            SELECT v.id, v.session_id, v.role, v.seq, v.created_at, p.content
            FROM selected_messages v
            JOIN message_parts p ON p.message_id = v.id
              AND p.kind = 'text' AND p.content IS NOT NULL AND trim(p.content) <> ''
            ORDER BY v.seq DESC, v.id DESC, p.ordinal ASC
            "#,
        )?;
        let before_rows = before_stmt.query_map(
            params![session_id, agent_id, anchor_seq, message_id, before as i64],
            conversation_message_row,
        )?;
        let mut before_messages = collect_conversation_messages(before_rows)?;
        before_messages.reverse();

        let mut after_stmt = conn.prepare(
            r#"
            WITH selected_messages AS (
              SELECT m.id, m.session_id, m.role, m.seq, m.created_at
              FROM messages m
              JOIN sessions s ON s.id = m.session_id
              WHERE s.id = ?1 AND s.agent_id = ?2
                AND s.deleted_at IS NULL AND m.deleted_at IS NULL
                AND m.status = 'complete'
                AND (m.seq > ?3 OR (m.seq = ?3 AND m.id >= ?4))
                AND EXISTS (
                  SELECT 1 FROM message_parts p
                  WHERE p.message_id = m.id AND p.kind = 'text'
                    AND p.content IS NOT NULL AND trim(p.content) <> ''
                )
              ORDER BY m.seq ASC, m.id ASC
              LIMIT ?5
            )
            SELECT v.id, v.session_id, v.role, v.seq, v.created_at, p.content
            FROM selected_messages v
            JOIN message_parts p ON p.message_id = v.id
              AND p.kind = 'text' AND p.content IS NOT NULL AND trim(p.content) <> ''
            ORDER BY v.seq ASC, v.id ASC, p.ordinal ASC
            "#,
        )?;
        let after_rows = after_stmt.query_map(
            params![
                session_id,
                agent_id,
                anchor_seq,
                message_id,
                after.saturating_add(1) as i64
            ],
            conversation_message_row,
        )?;
        let mut messages = before_messages;
        let anchor = Some(messages.len());
        messages.extend(collect_conversation_messages(after_rows)?);
        (messages, anchor)
    } else {
        let mut stmt = conn.prepare(
            r#"
            WITH selected_messages AS (
              SELECT m.id, m.session_id, m.role, m.seq, m.created_at
              FROM messages m
              JOIN sessions s ON s.id = m.session_id
              WHERE s.id = ?1 AND s.agent_id = ?2
                AND s.deleted_at IS NULL AND m.deleted_at IS NULL
                AND m.status = 'complete'
                AND EXISTS (
                  SELECT 1 FROM message_parts p
                  WHERE p.message_id = m.id AND p.kind = 'text'
                    AND p.content IS NOT NULL AND trim(p.content) <> ''
                )
              ORDER BY m.seq DESC, m.id DESC
              LIMIT ?3
            )
            SELECT v.id, v.session_id, v.role, v.seq, v.created_at, p.content
            FROM selected_messages v
            JOIN message_parts p ON p.message_id = v.id
              AND p.kind = 'text' AND p.content IS NOT NULL AND trim(p.content) <> ''
            ORDER BY v.seq DESC, v.id DESC, p.ordinal ASC
            "#,
        )?;
        let rows = stmt.query_map(
            params![session_id, agent_id, max_messages as i64],
            conversation_message_row,
        )?;
        let mut messages = collect_conversation_messages(rows)?;
        messages.reverse();
        let anchor = None;
        (messages, anchor)
    };

    let (start, end) = if let Some(anchor) = anchor {
        let mut start = anchor.saturating_sub(before);
        let mut end = (anchor + after + 1).min(messages.len());
        if end.saturating_sub(start) > max_messages {
            start = anchor.saturating_sub(max_messages / 2);
            end = (start + max_messages).min(messages.len());
            if end <= anchor {
                end = anchor + 1;
                start = end.saturating_sub(max_messages);
            }
        }
        (start, end)
    } else {
        (messages.len().saturating_sub(max_messages), messages.len())
    };

    Ok(Some(ConversationReadResult {
        session_id: session_id.to_string(),
        session_title,
        messages: messages[start..end].to_vec(),
    }))
}

fn conversation_message_row(
    row: &Row<'_>,
) -> rusqlite::Result<(String, String, String, i32, String, String)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn collect_conversation_messages<I>(rows: I) -> AppResult<Vec<ConversationMessageResult>>
where
    I: Iterator<Item = rusqlite::Result<(String, String, String, i32, String, String)>>,
{
    let mut messages = Vec::<ConversationMessageResult>::new();
    for row in rows {
        let (id, session_id, role, seq, created_at, content) = row?;
        if let Some(previous) = messages.last_mut().filter(|message| message.id == id) {
            previous.content.push_str("\n\n");
            previous.content.push_str(&content);
        } else {
            messages.push(ConversationMessageResult {
                id,
                session_id,
                role,
                seq,
                created_at,
                content,
            });
        }
    }
    Ok(messages)
}

fn conversation_snippet(content: &str, query: &str) -> String {
    const MAX_CHARS: usize = 360;
    let content = content.trim();
    if content.is_empty() {
        return String::new();
    }

    let total_chars = content.chars().count();
    let match_char = content
        .find(query)
        .map(|byte_index| content[..byte_index].chars().count())
        .unwrap_or(0);
    let mut start = match_char.saturating_sub(96);
    if total_chars > MAX_CHARS {
        start = start.min(total_chars - MAX_CHARS);
    } else {
        start = 0;
    }
    let snippet: String = content.chars().skip(start).take(MAX_CHARS).collect();
    let prefix = if start > 0 { "..." } else { "" };
    let end = start + snippet.chars().count();
    let suffix = if end < total_chars { "..." } else { "" };
    format!("{prefix}{snippet}{suffix}")
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
        let usage = usage_from_metadata(&metadata);
        totals.0 += usage.0;
        totals.1 += usage.1;
        totals.2 += usage.2;
    }
    Ok(totals)
}

pub fn token_usage_by_day(
    conn: &Connection,
    agent_id: Option<&str>,
    timezone_offset_minutes: i32,
) -> AppResult<Vec<TokenUsageDay>> {
    let mut statement = conn.prepare(
        "SELECT m.created_at, m.metadata FROM messages m \
         JOIN sessions s ON s.id = m.session_id \
         WHERE m.role = 'assistant' AND m.deleted_at IS NULL \
           AND (?1 IS NULL OR s.agent_id = ?1)",
    )?;
    let rows = statement.query_map([agent_id], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
        ))
    })?;
    let timezone_offset_minutes = timezone_offset_minutes.clamp(-1_440, 1_440);
    let mut days = BTreeMap::<String, (i64, i64, i64)>::new();

    for row in rows {
        let (Some(created_at), Some(metadata)) = row? else {
            continue;
        };
        let Some(timestamp) = parse_message_timestamp(&created_at) else {
            continue;
        };
        let Some(local_timestamp) =
            timestamp.checked_sub_signed(Duration::minutes(i64::from(timezone_offset_minutes)))
        else {
            continue;
        };
        let usage = usage_from_metadata(&metadata);
        if usage == (0, 0, 0) {
            continue;
        }
        let totals = days
            .entry(local_timestamp.date_naive().to_string())
            .or_default();
        totals.0 += usage.0;
        totals.1 += usage.1;
        totals.2 += usage.2;
    }

    Ok(days
        .into_iter()
        .map(
            |(date, (input_tokens, cached_tokens, output_tokens))| TokenUsageDay {
                date,
                input_tokens,
                cached_tokens,
                output_tokens,
            },
        )
        .collect())
}

fn usage_from_metadata(metadata: &str) -> (i64, i64, i64) {
    let Some(usage) = serde_json::from_str::<Value>(metadata)
        .ok()
        .and_then(|value| value.get("usage").cloned())
    else {
        return (0, 0, 0);
    };
    (
        usage
            .get("input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0),
        usage
            .get("cached_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0),
        usage
            .get("output_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0),
    )
}

fn parse_message_timestamp(value: &str) -> Option<DateTime<Utc>> {
    value
        .parse::<i64>()
        .ok()
        .and_then(|timestamp| Utc.timestamp_opt(timestamp, 0).single())
        .or_else(|| {
            DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|timestamp| timestamp.with_timezone(&Utc))
        })
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

pub(crate) fn enqueue_existing_sync_entities(conn: &Connection) -> AppResult<()> {
    let ids = {
        let mut statement = conn.prepare(
            "SELECT id FROM messages m WHERE m.status='complete' AND NOT EXISTS (
               SELECT 1 FROM sync_entity_state e WHERE e.entity_type='message' AND e.entity_id=m.id
             ) AND NOT EXISTS (
               SELECT 1 FROM sync_outbox o WHERE o.entity_type='message' AND o.entity_id=m.id
                 AND o.status IN ('pending','in_flight','conflict','dead_letter')
             ) ORDER BY session_id, seq, id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };
    for id in ids {
        enqueue_if_complete(conn, &id)?;
    }
    Ok(())
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

    // The session owns the active root pointer because root siblings have no
    // parent message on which selected_child_id could be stored.
    let selected_root_id = conn
        .query_row(
            "SELECT selected_root_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let mut cur_opt = selected_root_id
        .as_deref()
        .and_then(|id| all.iter().find(|message| message.id == id))
        .or_else(|| {
            all.iter()
                .filter(|message| message.parent_id.is_none())
                .max_by_key(|message| message.seq)
        });
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
    fn active_path_starts_from_the_session_selected_root() {
        let conn = setup();
        for (id, seq, selected_child_id) in [
            ("root-old", 0, Some("answer-old")),
            ("root-new", 2, Some("answer-new")),
        ] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: "session-1".into(),
                    role: "user".into(),
                    seq,
                    status: "complete".into(),
                    model: None,
                    token_count: None,
                    metadata: None,
                    parent_id: None,
                    selected_child_id: selected_child_id.map(str::to_string),
                },
            )
            .unwrap();
        }
        for (id, seq, parent_id) in [("answer-old", 1, "root-old"), ("answer-new", 3, "root-new")] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: "session-1".into(),
                    role: "assistant".into(),
                    seq,
                    status: "complete".into(),
                    model: None,
                    token_count: None,
                    metadata: None,
                    parent_id: Some(parent_id.into()),
                    selected_child_id: None,
                },
            )
            .unwrap();
        }

        crate::db::repo::sessions::set_selected_root_local(&conn, "session-1", Some("root-new"))
            .unwrap();
        let current = list_active_with_parts(&conn, "session-1").unwrap();
        assert_eq!(current.messages[0].message.id, "root-new");
        assert_eq!(current.messages[0].version_index, 1);
        assert_eq!(current.messages[0].version_count, 2);
        assert_eq!(current.messages[1].message.id, "answer-new");

        crate::db::repo::sessions::set_selected_root_local(&conn, "session-1", Some("root-old"))
            .unwrap();
        let previous = list_active_with_parts(&conn, "session-1").unwrap();
        assert_eq!(previous.messages[0].message.id, "root-old");
        assert_eq!(previous.messages[1].message.id, "answer-old");
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
    fn token_usage_by_day_respects_the_client_timezone() {
        let conn = setup();
        for (id, seq, created_at, input, cached, output) in [
            ("usage-before-midnight", 1, "1767281400", 100, 50, 20),
            (
                "usage-after-midnight",
                2,
                "2026-01-01T17:30:00Z",
                200,
                80,
                40,
            ),
        ] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: "session-1".into(),
                    role: "assistant".into(),
                    seq,
                    status: "complete".into(),
                    model: Some("provider/model".into()),
                    token_count: None,
                    metadata: None,
                    parent_id: None,
                    selected_child_id: None,
                },
            )
            .unwrap();
            update_usage(&conn, id, input, cached, output, input + output).unwrap();
            conn.execute(
                "UPDATE messages SET created_at = ?1 WHERE id = ?2",
                params![created_at, id],
            )
            .unwrap();
        }

        assert_eq!(
            token_usage_by_day(&conn, None, 0).unwrap(),
            vec![TokenUsageDay {
                date: "2026-01-01".into(),
                input_tokens: 300,
                cached_tokens: 130,
                output_tokens: 60,
            }]
        );
        assert_eq!(
            token_usage_by_day(&conn, Some("agent-1"), -480).unwrap(),
            vec![
                TokenUsageDay {
                    date: "2026-01-01".into(),
                    input_tokens: 100,
                    cached_tokens: 50,
                    output_tokens: 20,
                },
                TokenUsageDay {
                    date: "2026-01-02".into(),
                    input_tokens: 200,
                    cached_tokens: 80,
                    output_tokens: 40,
                },
            ]
        );
        assert!(token_usage_by_day(&conn, Some("missing-agent"), -480)
            .unwrap()
            .is_empty());
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

    #[test]
    fn conversation_search_is_agent_scoped_and_excludes_non_visible_parts() {
        let conn = setup();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent-2', 'Other Agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, title) VALUES ('session-2', 'agent-2', 'Other Session')",
            [],
        )
        .unwrap();

        for (id, session_id, role, seq, kind, content) in [
            (
                "history-user",
                "session-1",
                "user",
                1,
                "text",
                "Remember the release keyword",
            ),
            (
                "history-reasoning",
                "session-1",
                "assistant",
                2,
                "reasoning",
                "The release keyword is private reasoning",
            ),
            (
                "other-agent",
                "session-2",
                "user",
                1,
                "text",
                "Remember the release keyword",
            ),
            (
                "history-chinese",
                "session-1",
                "user",
                3,
                "text",
                "历史中的天气预报",
            ),
        ] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: session_id.into(),
                    role: role.into(),
                    seq,
                    status: "complete".into(),
                    model: None,
                    token_count: None,
                    metadata: None,
                    parent_id: None,
                    selected_child_id: None,
                },
            )
            .unwrap();
            insert_part(
                &conn,
                &NewMessagePart {
                    id: format!("{id}-part"),
                    message_id: id.into(),
                    kind: kind.into(),
                    ordinal: 0,
                    mime_type: None,
                    tool_call_id: None,
                    content: content.into(),
                    metadata: None,
                },
            )
            .unwrap();
        }

        let results =
            search_conversation(&conn, "agent-1", None, "release keyword", None, 20).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message_id, "history-user");
        assert_eq!(results[0].session_id, "session-1");

        let chinese_results =
            search_conversation(&conn, "agent-1", None, "天气", None, 20).unwrap();
        assert_eq!(chinese_results.len(), 1);
        assert_eq!(chinese_results[0].message_id, "history-chinese");
        let chinese_phrase_results =
            search_conversation(&conn, "agent-1", None, "天气预报", None, 20).unwrap();
        assert_eq!(chinese_phrase_results.len(), 1);
        assert_eq!(chinese_phrase_results[0].message_id, "history-chinese");

        for message_id in [
            "history-user",
            "history-reasoning",
            "other-agent",
            "history-chinese",
        ] {
            delete_parts(&conn, message_id).unwrap();
        }
        insert_part(
            &conn,
            &NewMessagePart {
                id: "replacement-part".into(),
                message_id: "history-user".into(),
                kind: "text".into(),
                ordinal: 0,
                mime_type: None,
                tool_call_id: None,
                content: "Fresh content".into(),
                metadata: None,
            },
        )
        .unwrap();
        assert!(
            search_conversation(&conn, "agent-1", None, "release keyword", None, 20)
                .unwrap()
                .is_empty()
        );
        let replacement =
            search_conversation(&conn, "agent-1", None, "Fresh content", None, 20).unwrap();
        assert_eq!(replacement.len(), 1);
        assert_eq!(replacement[0].message_id, "history-user");
    }

    #[test]
    fn conversation_read_returns_bounded_visible_context() {
        let conn = setup();
        for (id, role, seq, content) in [
            ("read-user-1", "user", 1, "First question"),
            ("read-assistant", "assistant", 2, "A useful answer"),
            ("read-user-2", "user", 3, "Follow-up question"),
        ] {
            insert(
                &conn,
                &NewMessage {
                    id: id.into(),
                    session_id: "session-1".into(),
                    role: role.into(),
                    seq,
                    status: "complete".into(),
                    model: None,
                    token_count: None,
                    metadata: None,
                    parent_id: None,
                    selected_child_id: None,
                },
            )
            .unwrap();
            insert_part(
                &conn,
                &NewMessagePart {
                    id: format!("{id}-part"),
                    message_id: id.into(),
                    kind: "text".into(),
                    ordinal: 0,
                    mime_type: None,
                    tool_call_id: None,
                    content: content.into(),
                    metadata: None,
                },
            )
            .unwrap();
        }

        insert_part(
            &conn,
            &NewMessagePart {
                id: "read-assistant-part-2".into(),
                message_id: "read-assistant".into(),
                kind: "text".into(),
                ordinal: 1,
                mime_type: None,
                tool_call_id: None,
                content: "With a second text part".into(),
                metadata: None,
            },
        )
        .unwrap();

        let result = read_conversation(
            &conn,
            "agent-1",
            "session-1",
            Some("read-assistant"),
            1,
            1,
            3,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.messages[1].id, "read-assistant");
        assert_eq!(
            result.messages[1].content,
            "A useful answer\n\nWith a second text part"
        );

        let latest = read_conversation(&conn, "agent-1", "session-1", None, 0, 0, 2)
            .unwrap()
            .unwrap();
        assert_eq!(
            latest
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            vec!["read-assistant", "read-user-2"]
        );
    }
}
