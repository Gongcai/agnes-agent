//! memory_store repo - 长期记忆存储库的 CRUD 操作。
use rusqlite::Connection;

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct NewMemory {
    pub id: String,
    pub agent_id: String,
    pub content: String,
    pub memory_type: String, // "Preference" | "Fact" | "Context" | "Codebase"
    pub scope: String,       // "global" | "agent"
    pub source: String,
    pub confidence: f64,
}

pub fn search(_conn: &Connection, _query: &str, _agent_id: &str) -> AppResult<Vec<String>> {
    // V0.2 实现 sqlite-vec 向量混合检索
    Ok(vec![])
}

/// 插入一条新的长期记忆条目。
pub fn insert(conn: &Connection, m: &NewMemory) -> AppResult<()> {
    let now_str = chrono_like_now();
    conn.execute(
        "INSERT INTO memory_store (id, agent_id, content, type, scope, source, confidence, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            &m.id,
            &m.agent_id,
            &m.content,
            &m.memory_type,
            &m.scope,
            &m.source,
            m.confidence,
            &now_str,
            &now_str,
        ),
    )?;
    Ok(())
}

fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
