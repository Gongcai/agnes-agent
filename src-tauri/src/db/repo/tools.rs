//! tool_calls repo - 工具审计日志与调用状态的 CRUD 操作。
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct ToolCallRow {
    pub id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub tool: String,
    pub params: Option<String>,
    pub result: Option<String>,
    pub status: String,
    pub risk_level: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub approval_policy_snapshot: Option<String>,
    pub created_at: String,
}

pub struct NewToolCall {
    pub id: String,
    pub session_id: String,
    pub message_id: Option<String>,
    pub tool: String,
    pub params: Option<String>,
    pub status: String,
    pub risk_level: Option<String>,
    pub approval_policy_snapshot: Option<String>,
}

fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// 记录初始的工具调用请求（状态通常为 pending_approval 或 running）。
pub fn insert(conn: &Connection, tc: &NewToolCall) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "INSERT INTO tool_calls (id, session_id, message_id, tool, params, status, \
         risk_level, approval_policy_snapshot, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            tc.id,
            tc.session_id,
            tc.message_id,
            tc.tool,
            tc.params,
            tc.status,
            tc.risk_level,
            tc.approval_policy_snapshot,
            now_str,
        ],
    )?;
    Ok(())
}

/// 将工具调用状态更新为 running，并记录开始时间及工作目录（cwd）。
pub fn update_running(conn: &Connection, id: &str, cwd: &str) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE tool_calls \
         SET status = 'running', cwd = ?1, started_at = ?2 \
         WHERE id = ?3",
        params![cwd, now_str, id],
    )?;
    Ok(())
}

/// 更新工具执行完成/失败/拒绝的状态及输出。
pub fn update_complete(
    conn: &Connection,
    id: &str,
    status: &str,
    result: Option<&str>,
    exit_code: Option<i32>,
    stdout: Option<&str>,
    stderr: Option<&str>,
) -> AppResult<()> {
    let now_str = now();
    conn.execute(
        "UPDATE tool_calls \
         SET status = ?1, result = ?2, exit_code = ?3, stdout = ?4, stderr = ?5, \
             completed_at = ?6 \
         WHERE id = ?7",
        params![status, result, exit_code, stdout, stderr, now_str, id],
    )?;
    Ok(())
}

/// 列出会话内的所有工具调用记录，按创建时间排序。
pub fn list_for_session(conn: &Connection, session_id: &str) -> AppResult<Vec<ToolCallRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, message_id, tool, params, result, status, risk_level, \
         cwd, exit_code, stdout, stderr, started_at, completed_at, \
         approval_policy_snapshot, created_at \
         FROM tool_calls \
         WHERE session_id = ?1 \
         ORDER BY created_at ASC",
    )?;

    let rows = stmt.query_map([session_id], |r| {
        Ok(ToolCallRow {
            id: r.get(0)?,
            session_id: r.get(1)?,
            message_id: r.get(2)?,
            tool: r.get(3)?,
            params: r.get(4)?,
            result: r.get(5)?,
            status: r.get(6)?,
            risk_level: r.get(7)?,
            cwd: r.get(8)?,
            exit_code: r.get(9)?,
            stdout: r.get(10)?,
            stderr: r.get(11)?,
            started_at: r.get(12)?,
            completed_at: r.get(13)?,
            approval_policy_snapshot: r.get(14)?,
            created_at: r.get(15)?,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 获取单个工具调用详情。
pub fn get(conn: &Connection, id: &str) -> AppResult<Option<ToolCallRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, session_id, message_id, tool, params, result, status, risk_level, \
         cwd, exit_code, stdout, stderr, started_at, completed_at, \
         approval_policy_snapshot, created_at \
         FROM tool_calls \
         WHERE id = ?1",
    )?;

    let res = stmt.query_row([id], |r| {
        Ok(ToolCallRow {
            id: r.get(0)?,
            session_id: r.get(1)?,
            message_id: r.get(2)?,
            tool: r.get(3)?,
            params: r.get(4)?,
            result: r.get(5)?,
            status: r.get(6)?,
            risk_level: r.get(7)?,
            cwd: r.get(8)?,
            exit_code: r.get(9)?,
            stdout: r.get(10)?,
            stderr: r.get(11)?,
            started_at: r.get(12)?,
            completed_at: r.get(13)?,
            approval_policy_snapshot: r.get(14)?,
            created_at: r.get(15)?,
        })
    }).optional()?;

    Ok(res)
}
