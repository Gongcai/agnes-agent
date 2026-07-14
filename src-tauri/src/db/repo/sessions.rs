//! sessions repo（V0.1 仅占位；V0.2 落地会话预算/压缩与 summary 写入）。

use rusqlite::Connection;

use crate::error::AppResult;

pub fn list(_conn: &Connection) -> AppResult<Vec<(String, String)>> {
    // TODO(V0.2): SELECT id, title FROM sessions
    Ok(vec![])
}
