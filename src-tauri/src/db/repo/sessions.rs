//! sessions repo - 会话管理的 CRUD 操作。
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

#[derive(Debug, Clone)]
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
         summary, summary_updated_at, \
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
            summary: r.get(11)?,
            summary_updated_at: r.get(12)?,
            created_at: r.get(13)?,
            updated_at: r.get(14)?,
            version: r.get(15)?,
            deleted_at: r.get(16)?,
            origin_device_id: r.get(17)?,
            pinned: r.get(18)?,
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
         summary, summary_updated_at, \
         created_at, updated_at, version, deleted_at, origin_device_id, pinned \
         FROM sessions \
         WHERE id = ?1",
    )?;

    let res = stmt.query_row([id], |r| {
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
            summary: r.get(11)?,
            summary_updated_at: r.get(12)?,
            created_at: r.get(13)?,
            updated_at: r.get(14)?,
            version: r.get(15)?,
            deleted_at: r.get(16)?,
            origin_device_id: r.get(17)?,
            pinned: r.get(18)?,
        })
    }).optional()?;

    Ok(res)
}

/// 插入一个新会话。
pub fn insert(conn: &Connection, s: &NewSession) -> AppResult<String> {
    let now_str = now();
    let compress_threshold = s.compress_threshold.unwrap_or(0.85);
    let recency_window = s.recency_window.unwrap_or(20);

    conn.execute(
        "INSERT INTO sessions (id, agent_id, title, context_limit, compress_threshold, \
         recency_window, reserved_output_tokens, summarizer_model, model, thinking_mode, thinking_budget, \
         created_at, updated_at, version, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1, ?14)",
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
            now_str,
            now_str,
            s.origin_device_id,
        ],
    )?;

    Ok(s.id.clone())
}

/// 更新会话级模型与思考配置（输入框切换模型/思考强度时调用）。
pub fn update_llm(
    conn: &Connection,
    id: &str,
    model: &str,
    thinking_mode: &str,
    thinking_budget: i64,
) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE sessions SET model = ?1, thinking_mode = ?2, thinking_budget = ?3, \
         updated_at = ?4, version = version + 1 WHERE id = ?5",
        params![model, thinking_mode, thinking_budget, now_str, id],
    )?;
    Ok(())
}

/// 更新会话标题。
pub fn update_title(conn: &Connection, id: &str, title: &str) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE sessions \
         SET title = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3",
        params![title, now_str, id],
    )?;
    Ok(())
}

/// 更新滚动摘要。
pub fn update_summary(conn: &Connection, id: &str, summary: &str) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE sessions \
         SET summary = ?1, summary_updated_at = ?2, updated_at = ?3, version = version + 1 \
         WHERE id = ?4",
        params![summary, now_str, now_str, id],
    )?;
    Ok(())
}

/// 软删除一个会话（标记 deleted_at）。
pub fn delete(conn: &Connection, id: &str) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE sessions \
         SET deleted_at = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3",
        params![now_str, now_str, id],
    )?;
    Ok(())
}

/// 设置或取消会话置顶。
pub fn set_pin(conn: &Connection, id: &str, pinned: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE sessions SET pinned = ?1, updated_at = ?2 WHERE id = ?3",
        params![if pinned { 1 } else { 0 }, now(), id],
    )?;
    Ok(())
}
