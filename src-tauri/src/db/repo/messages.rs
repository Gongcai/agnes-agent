//! messages repo（V0.1 仅占位；V0.2 落地 messages/message_parts 读写与 seq/status）。

use rusqlite::Connection;

use crate::error::AppResult;

pub fn count(_conn: &Connection, _session_id: &str) -> AppResult<u64> {
    Ok(0)
}
