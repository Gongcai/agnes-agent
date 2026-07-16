//! sessions repo - 会话管理的 CRUD 操作。
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::sync::payload::SyncEntityType;

#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub context_limit: Option<i64>,
    pub compress_threshold: f64,
    pub recency_window: i64,
    pub reserved_output_tokens: Option<i64>,
    pub summarizer_model: Option<String>,
    pub model: Option<String>,
    pub thinking_mode: Option<String>,
    pub thinking_budget: Option<i64>,
    pub permission_mode: String,
    pub workspace_id: Option<String>,
    pub summary: Option<String>,
    pub summary_updated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
    pub pinned: i64,
}

pub struct NewSession {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub context_limit: Option<i64>,
    pub compress_threshold: Option<f64>,
    pub recency_window: Option<i64>,
    pub reserved_output_tokens: Option<i64>,
    pub summarizer_model: Option<String>,
    pub model: Option<String>,
    pub thinking_mode: Option<String>,
    pub thinking_budget: Option<i64>,
    pub permission_mode: String,
    pub workspace_id: Option<String>,
    pub origin_device_id: Option<String>,
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// 列出某个 Agent 的所有非软删除会话，置顶优先，其余按更新时间倒序。
pub fn list(conn: &Connection, agent_id: &str) -> AppResult<Vec<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, title, context_limit, compress_threshold, recency_window, \
         reserved_output_tokens, summarizer_model, model, thinking_mode, thinking_budget, \
         permission_mode, workspace_id, summary, summary_updated_at, \
         created_at, updated_at, version, deleted_at, origin_device_id, pinned \
         FROM sessions \
         WHERE agent_id = ?1 AND deleted_at IS NULL \
         ORDER BY pinned DESC, updated_at DESC",
    )?;

    let rows = stmt.query_map([agent_id], |r| {
        Ok(SessionRow {
            id: r.get(0)?,
            agent_id: r.get(1)?,
            title: r.get(2)?,
            context_limit: r.get(3)?,
            compress_threshold: r.get(4)?,
            recency_window: r.get(5)?,
            reserved_output_tokens: r.get(6)?,
            summarizer_model: r.get(7)?,
            model: r.get(8)?,
            thinking_mode: r.get(9)?,
            thinking_budget: r.get(10)?,
            permission_mode: r.get(11)?,
            workspace_id: r.get(12)?,
            summary: r.get(13)?,
            summary_updated_at: r.get(14)?,
            created_at: r.get(15)?,
            updated_at: r.get(16)?,
            version: r.get(17)?,
            deleted_at: r.get(18)?,
            origin_device_id: r.get(19)?,
            pinned: r.get(20)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// 获取单个会话的详细信息。
pub fn get(conn: &Connection, id: &str) -> AppResult<Option<SessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, title, context_limit, compress_threshold, recency_window, \
         reserved_output_tokens, summarizer_model, model, thinking_mode, thinking_budget, \
         permission_mode, workspace_id, summary, summary_updated_at, \
         created_at, updated_at, version, deleted_at, origin_device_id, pinned \
         FROM sessions \
         WHERE id = ?1",
    )?;

    let res = stmt
        .query_row([id], |r| {
            Ok(SessionRow {
                id: r.get(0)?,
                agent_id: r.get(1)?,
                title: r.get(2)?,
                context_limit: r.get(3)?,
                compress_threshold: r.get(4)?,
                recency_window: r.get(5)?,
                reserved_output_tokens: r.get(6)?,
                summarizer_model: r.get(7)?,
                model: r.get(8)?,
                thinking_mode: r.get(9)?,
                thinking_budget: r.get(10)?,
                permission_mode: r.get(11)?,
                workspace_id: r.get(12)?,
                summary: r.get(13)?,
                summary_updated_at: r.get(14)?,
                created_at: r.get(15)?,
                updated_at: r.get(16)?,
                version: r.get(17)?,
                deleted_at: r.get(18)?,
                origin_device_id: r.get(19)?,
                pinned: r.get(20)?,
            })
        })
        .optional()?;

    Ok(res)
}

/// 插入一个新会话。
pub fn insert(conn: &mut Connection, s: &NewSession) -> AppResult<String> {
    let now_str = now();
    let compress_threshold = s.compress_threshold.unwrap_or(0.85);
    let recency_window = s.recency_window.unwrap_or(20);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let local_device_id = super::sync::device_id(&tx)?;
    let is_remote = s.origin_device_id.is_some();
    let origin_device_id = s.origin_device_id.as_deref().unwrap_or(&local_device_id);

    tx.execute(
        "INSERT INTO sessions (id, agent_id, title, context_limit, compress_threshold, \
         recency_window, reserved_output_tokens, summarizer_model, model, thinking_mode, thinking_budget, \
         permission_mode, workspace_id, created_at, updated_at, version, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 1, ?16)",
        params![
            s.id,
            s.agent_id,
            s.title,
            s.context_limit,
            compress_threshold,
            recency_window,
            s.reserved_output_tokens,
            s.summarizer_model,
            s.model,
            s.thinking_mode,
            s.thinking_budget,
            s.permission_mode,
            s.workspace_id,
            now_str,
            now_str,
            origin_device_id,
        ],
    )?;
    if !is_remote {
        enqueue_current(&tx, &s.id)?;
    }
    tx.commit()?;
    Ok(s.id.clone())
}

/// 更新会话级模型与思考配置（输入框切换模型/思考强度时调用）。
pub fn update_llm(
    conn: &mut Connection,
    id: &str,
    model: &str,
    thinking_mode: &str,
    thinking_budget: i64,
) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE sessions SET model = ?1, thinking_mode = ?2, thinking_budget = ?3, \
         updated_at = ?4, version = version + 1, origin_device_id = ?5 \
         WHERE id = ?6 AND deleted_at IS NULL",
        params![
            model,
            thinking_mode,
            thinking_budget,
            now_str,
            device_id,
            id
        ],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// Update the session-level tool permission mode.
pub fn update_permission_mode(conn: &Connection, id: &str, permission_mode: &str) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE sessions SET permission_mode = ?1, updated_at = ?2, \
         version = version + 1 WHERE id = ?3 AND deleted_at IS NULL",
        params![permission_mode, now_str, id],
    )?;
    Ok(())
}

/// 更新会话标题。
pub fn update_title(conn: &mut Connection, id: &str, title: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE sessions \
         SET title = ?1, updated_at = ?2, version = version + 1, origin_device_id = ?3 \
         WHERE id = ?4 AND deleted_at IS NULL",
        params![title, now_str, device_id, id],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// 更新滚动摘要。
pub fn update_summary(conn: &mut Connection, id: &str, summary: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE sessions \
         SET summary = ?1, summary_updated_at = ?2, updated_at = ?3, \
             version = version + 1, origin_device_id = ?4 \
         WHERE id = ?5 AND deleted_at IS NULL",
        params![summary, now_str, now_str, device_id, id],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// 软删除一个会话（标记 deleted_at）。
pub fn delete(conn: &mut Connection, id: &str) -> AppResult<()> {
    let now_str = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE sessions \
         SET deleted_at = ?1, updated_at = ?2, version = version + 1, origin_device_id = ?3 \
         WHERE id = ?4 AND deleted_at IS NULL",
        params![now_str, now_str, device_id, id],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

/// 设置或取消会话置顶。
pub fn set_pin(conn: &mut Connection, id: &str, pinned: bool) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE sessions SET pinned = ?1, updated_at = ?2, version = version + 1, \
         origin_device_id = ?3 WHERE id = ?4 AND deleted_at IS NULL",
        params![if pinned { 1 } else { 0 }, now(), device_id, id],
    )?;
    if changed > 0 {
        enqueue_current(&tx, id)?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn enqueue_current(conn: &Connection, id: &str) -> AppResult<()> {
    let row = get(conn, id)?.ok_or_else(|| {
        AppError::Other(format!("session `{id}` disappeared during sync enqueue"))
    })?;
    let source = serde_json::to_value(&row)?;
    super::sync::enqueue_projection(
        conn,
        SyncEntityType::Session,
        &row.id,
        row.version,
        row.deleted_at.is_some(),
        &source,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_session(origin_device_id: Option<String>) -> NewSession {
        NewSession {
            id: "session-1".into(),
            agent_id: "agent-1".into(),
            title: "Session".into(),
            context_limit: None,
            compress_threshold: None,
            recency_window: None,
            reserved_output_tokens: None,
            summarizer_model: None,
            model: None,
            thinking_mode: None,
            thinking_budget: None,
            permission_mode: "auto".into(),
            workspace_id: Some("local-workspace".into()),
            origin_device_id,
        }
    }

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) VALUES (1, '12345678-device')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent-1', 'Agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workspaces (id, agent_id, name) VALUES ('local-workspace', 'agent-1', 'Local')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn session_sync_ignores_device_local_fields_and_chains_revisions() {
        let mut conn = setup();
        insert(&mut conn, &new_session(None)).unwrap();
        update_title(&mut conn, "session-1", "Renamed").unwrap();
        set_pin(&mut conn, "session-1", true).unwrap();
        update_permission_mode(&conn, "session-1", "accept_edits").unwrap();
        delete(&mut conn, "session-1").unwrap();

        let rows: Vec<(Option<i64>, i64, String, Option<String>)> = conn
            .prepare(
                "SELECT base_revision, local_version, operation, payload FROM sync_outbox \
                 WHERE entity_id = 'session-1' ORDER BY created_at, rowid",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].0, None);
        assert_eq!(rows[1].0, Some(1));
        assert_eq!(rows[2].0, Some(2));
        assert_eq!(rows[3].0, Some(3));
        assert_eq!(rows[3].1, 5);
        assert_eq!(rows[3].2, "delete");
        let initial_payload = rows[0].3.as_deref().unwrap();
        assert!(!initial_payload.contains("permission_mode"));
        assert!(initial_payload.contains("workspace_id"));
        assert!(initial_payload.contains("local-workspace"));
    }

    #[test]
    fn remote_origin_session_does_not_echo_into_outbox() {
        let mut conn = setup();
        insert(&mut conn, &new_session(Some("remote-device".into()))).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
