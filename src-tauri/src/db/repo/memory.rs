//! Structured long-term memory persistence and retrieval.
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
pub struct MemoryRow {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub content: String,
    pub creator: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewMemory {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub content: String,
    pub creator: String,
    pub memory_type: String,
    pub scope: String,
    pub source: String,
    pub confidence: f64,
    pub embedding_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MemoryUpdate {
    pub name: String,
    pub keywords: Vec<String>,
    pub content: String,
}

fn normalize_required(value: &str, field: &str) -> AppResult<String> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(AppError::Other(format!("{field} cannot be empty")));
    }
    Ok(normalized.to_string())
}

pub fn normalize_keywords(keywords: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for keyword in keywords {
        let value = keyword.trim();
        if value.is_empty() || normalized.iter().any(|existing| existing == value) {
            continue;
        }
        normalized.push(value.to_string());
    }
    normalized
}

fn encode_keywords(keywords: &[String]) -> AppResult<String> {
    Ok(serde_json::to_string(&normalize_keywords(keywords))?)
}

fn decode_keywords(value: Option<String>) -> Vec<String> {
    value
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .map(|values| normalize_keywords(&values))
        .unwrap_or_default()
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRow> {
    Ok(MemoryRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        name: row.get(2)?,
        keywords: decode_keywords(row.get(3)?),
        content: row.get(4)?,
        creator: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub fn list(conn: &Connection, agent_id: &str) -> AppResult<Vec<MemoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, name, keywords, content, creator, created_at, updated_at \
         FROM memory_store \
         WHERE agent_id = ?1 AND status = 'active' AND deleted_at IS NULL \
         ORDER BY CAST(created_at AS INTEGER) DESC, id DESC",
    )?;
    let rows = stmt.query_map([agent_id], row_from_sql)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Search visible fields within one agent. Vector fusion will be re-enabled after
/// embedding dimensions become configurable.
pub fn search(
    conn: &Connection,
    query_text: &str,
    agent_id: &str,
    limit: usize,
) -> AppResult<Vec<MemoryRow>> {
    let query = query_text.trim();
    if query.is_empty() {
        return Ok(list(conn, agent_id)?.into_iter().take(limit).collect());
    }

    let pattern = format!("%{}%", query.to_lowercase());
    let mut stmt = conn.prepare(
        "SELECT id, agent_id, name, keywords, content, creator, created_at, updated_at \
         FROM memory_store \
         WHERE agent_id = ?1 AND status = 'active' AND deleted_at IS NULL \
           AND (lower(name) LIKE ?2 OR lower(COALESCE(keywords, '')) LIKE ?2 OR lower(content) LIKE ?2) \
         ORDER BY CASE \
           WHEN lower(name) = lower(?3) THEN 0 \
           WHEN lower(name) LIKE ?2 THEN 1 \
           WHEN lower(COALESCE(keywords, '')) LIKE ?2 THEN 2 \
           ELSE 3 END, \
           CAST(created_at AS INTEGER) DESC, id DESC \
         LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![agent_id, pattern, query, limit as i64],
        row_from_sql,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Insert an AI or user memory, skipping an exact duplicate within the same agent.
pub fn insert(conn: &Connection, memory: &NewMemory) -> AppResult<bool> {
    let name = normalize_required(&memory.name, "Memory name")?;
    let content = normalize_required(&memory.content, "Memory content")?;
    if !matches!(memory.creator.as_str(), "ai" | "user") {
        return Err(AppError::Other("Memory creator must be ai or user".into()));
    }
    let keywords = encode_keywords(&memory.keywords)?;
    let now = now();
    let affected = conn.execute(
        "INSERT INTO memory_store \
         (id, agent_id, name, keywords, content, creator, type, scope, source, confidence, \
          created_at, updated_at, embedding_id, version) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, 1 \
         WHERE NOT EXISTS ( \
           SELECT 1 FROM memory_store \
           WHERE agent_id = ?2 AND status = 'active' AND deleted_at IS NULL \
             AND lower(trim(name)) = lower(trim(?3)) \
             AND lower(trim(content)) = lower(trim(?5)) \
         )",
        params![
            memory.id,
            memory.agent_id,
            name,
            keywords,
            content,
            memory.creator,
            memory.memory_type,
            memory.scope,
            memory.source,
            memory.confidence,
            now,
            memory.embedding_id,
        ],
    )?;
    Ok(affected > 0)
}

pub fn update(
    conn: &Connection,
    id: &str,
    agent_id: &str,
    changes: &MemoryUpdate,
) -> AppResult<()> {
    let name = normalize_required(&changes.name, "Memory name")?;
    let content = normalize_required(&changes.content, "Memory content")?;
    let keywords = encode_keywords(&changes.keywords)?;
    let affected = conn.execute(
        "UPDATE memory_store \
         SET name = ?1, keywords = ?2, content = ?3, updated_at = ?4, version = version + 1 \
         WHERE id = ?5 AND agent_id = ?6 AND status = 'active' AND deleted_at IS NULL",
        params![name, keywords, content, now(), id, agent_id],
    )?;
    if affected == 0 {
        return Err(AppError::Other(
            "Memory was not found for this agent".into(),
        ));
    }
    Ok(())
}

pub fn delete(conn: &Connection, id: &str, agent_id: &str) -> AppResult<()> {
    let timestamp = now();
    let affected = conn.execute(
        "UPDATE memory_store \
         SET status = 'deleted', deleted_at = ?1, updated_at = ?1, version = version + 1 \
         WHERE id = ?2 AND agent_id = ?3 AND status = 'active' AND deleted_at IS NULL",
        params![timestamp, id, agent_id],
    )?;
    if affected == 0 {
        return Err(AppError::Other(
            "Memory was not found for this agent".into(),
        ));
    }
    Ok(())
}

pub fn get(conn: &Connection, id: &str, agent_id: &str) -> AppResult<Option<MemoryRow>> {
    conn.query_row(
        "SELECT id, agent_id, name, keywords, content, creator, created_at, updated_at \
         FROM memory_store \
         WHERE id = ?1 AND agent_id = ?2 AND status = 'active' AND deleted_at IS NULL",
        params![id, agent_id],
        row_from_sql,
    )
    .optional()
    .map_err(Into::into)
}

pub fn insert_embedding(
    conn: &Connection,
    embedding_id: &str,
    ref_type: &str,
    ref_id: &str,
    model: &str,
    dims: i32,
    content_hash: &str,
    vector: &[f32],
) -> AppResult<()> {
    let now = now();
    conn.execute(
        "INSERT INTO embedding_items (id, ref_type, ref_id, model, dims, content_hash, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            embedding_id,
            ref_type,
            ref_id,
            model,
            dims,
            content_hash,
            &now,
        ),
    )?;

    let vector_bytes = f32_to_bytes(vector);
    conn.execute(
        "INSERT INTO vec_embeddings (embedding_id, vector) VALUES (?1, ?2)",
        (embedding_id, vector_bytes),
    )?;
    Ok(())
}

fn f32_to_bytes(values: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values))
    }
}

fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    seconds.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent-a', 'A'), ('agent-b', 'B')",
            [],
        )
        .unwrap();
        conn
    }

    fn memory(id: &str, agent_id: &str, creator: &str) -> NewMemory {
        NewMemory {
            id: id.into(),
            agent_id: agent_id.into(),
            name: "Rust workspace".into(),
            keywords: vec!["rust".into(), " workspace ".into(), "rust".into()],
            content: "The project uses a Rust execution core.".into(),
            creator: creator.into(),
            memory_type: "Fact".into(),
            scope: "agent".into(),
            source: "test".into(),
            confidence: 1.0,
            embedding_id: None,
        }
    }

    #[test]
    fn searches_name_keywords_and_content_with_agent_isolation() {
        let conn = connection();
        assert!(insert(&conn, &memory("m1", "agent-a", "user")).unwrap());
        assert!(insert(&conn, &memory("m2", "agent-b", "ai")).unwrap());

        for query in ["Rust workspace", "workspace", "execution core"] {
            let found = search(&conn, query, "agent-a", 10).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].id, "m1");
            assert_eq!(found[0].keywords, vec!["rust", "workspace"]);
            assert_eq!(found[0].creator, "user");
        }
        assert!(search(&conn, "Rust", "missing-agent", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn validates_fields_preserves_creator_and_skips_duplicates() {
        let conn = connection();
        assert!(insert(&conn, &memory("m1", "agent-a", "user")).unwrap());
        assert!(!insert(&conn, &memory("m2", "agent-a", "ai")).unwrap());
        assert_eq!(list(&conn, "agent-a").unwrap().len(), 1);

        update(
            &conn,
            "m1",
            "agent-a",
            &MemoryUpdate {
                name: "Updated".into(),
                keywords: vec![],
                content: "Updated content".into(),
            },
        )
        .unwrap();
        assert_eq!(
            get(&conn, "m1", "agent-a").unwrap().unwrap().creator,
            "user"
        );

        delete(&conn, "m1", "agent-a").unwrap();
        assert!(list(&conn, "agent-a").unwrap().is_empty());
    }
}
