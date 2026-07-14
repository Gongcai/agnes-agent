//! tool_calls repo（V0.1 仅占位；V0.5 落地工具执行审计落库）。

use rusqlite::Connection;

use crate::error::AppResult;

pub fn record(_conn: &Connection, _session_id: &str, _tool: &str) -> AppResult<()> {
    Ok(())
}
