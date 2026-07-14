//! memory_store repo（V0.1 仅占位；V0.2 实现 memory_search：
//! sqlite-vec 向量 + FTS5 字符串混合检索，RRF 融合，限定 agent_id）。

use rusqlite::Connection;

use crate::error::AppResult;

pub fn search(_conn: &Connection, _query: &str, _agent_id: &str) -> AppResult<Vec<String>> {
    Ok(vec![])
}
