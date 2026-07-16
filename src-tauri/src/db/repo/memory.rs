//! Structured long-term memory persistence and retrieval.
use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

pub const MAX_EMBEDDING_DIMS: usize = 8_192;

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
    #[serde(skip_serializing)]
    pub status: String,
    #[serde(skip_serializing)]
    pub version: i64,
    #[serde(skip_serializing)]
    pub deleted_at: Option<String>,
    #[serde(skip_serializing)]
    pub origin_device_id: Option<String>,
    #[serde(skip_serializing)]
    pub embedding_id: Option<String>,
    #[serde(skip_serializing)]
    pub embedding_model: Option<String>,
    #[serde(skip_serializing)]
    pub embedding_content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QueryEmbedding {
    pub model: String,
    pub vector: Vec<f32>,
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
        status: row.get(8)?,
        version: row.get(9)?,
        deleted_at: row.get(10)?,
        origin_device_id: row.get(11)?,
        embedding_id: row.get(12)?,
        embedding_model: row.get(13)?,
        embedding_content_hash: row.get(14)?,
    })
}

const MEMORY_SELECT: &str = "SELECT m.id, m.agent_id, m.name, m.keywords, m.content, m.creator, \
     m.created_at, m.updated_at, m.status, m.version, m.deleted_at, m.origin_device_id, \
     m.embedding_id, e.model, e.content_hash \
     FROM memory_store m LEFT JOIN embedding_items e ON e.id = m.embedding_id";

pub fn list(conn: &Connection, agent_id: &str) -> AppResult<Vec<MemoryRow>> {
    let sql = format!(
        "{MEMORY_SELECT} WHERE m.agent_id = ?1 AND m.status = 'active' \
         AND m.deleted_at IS NULL ORDER BY CAST(m.created_at AS INTEGER) DESC, m.id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([agent_id], row_from_sql)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Search visible fields within one agent.
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
    let sql = format!(
        "{MEMORY_SELECT} WHERE m.agent_id = ?1 AND m.status = 'active' AND m.deleted_at IS NULL \
           AND (lower(m.name) LIKE ?2 OR lower(COALESCE(m.keywords, '')) LIKE ?2 OR lower(m.content) LIKE ?2) \
         ORDER BY CASE \
           WHEN lower(m.name) = lower(?3) THEN 0 \
           WHEN lower(m.name) LIKE ?2 THEN 1 \
           WHEN lower(COALESCE(m.keywords, '')) LIKE ?2 THEN 2 \
           ELSE 3 END, \
           CAST(m.created_at AS INTEGER) DESC, m.id DESC LIMIT ?4"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![agent_id, pattern, query, limit as i64],
        row_from_sql,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn search_hybrid(
    conn: &Connection,
    query_text: &str,
    agent_id: &str,
    limit: usize,
    query_embedding: Option<&QueryEmbedding>,
) -> AppResult<Vec<MemoryRow>> {
    let candidate_limit = limit.saturating_mul(3).clamp(limit, 150);
    let string_rows = search(conn, query_text, agent_id, candidate_limit)?;
    let vector_rows = match query_embedding.filter(|embedding| valid_vector(&embedding.vector)) {
        Some(embedding) => search_vector(conn, agent_id, embedding, candidate_limit)
            .unwrap_or_else(|error| {
                eprintln!("[memory] Vector search failed; using string results: {error}");
                Vec::new()
            }),
        None => Vec::new(),
    };

    let mut fused: HashMap<String, (f64, MemoryRow)> = HashMap::new();
    for (rank, memory) in string_rows.into_iter().enumerate() {
        let score = 1.0 / (60.0 + rank as f64 + 1.0);
        fused
            .entry(memory.id.clone())
            .and_modify(|entry| entry.0 += score)
            .or_insert((score, memory));
    }
    for (rank, memory) in vector_rows.into_iter().enumerate() {
        let score = 1.0 / (60.0 + rank as f64 + 1.0);
        fused
            .entry(memory.id.clone())
            .and_modify(|entry| entry.0 += score)
            .or_insert((score, memory));
    }

    let mut ranked = fused.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| right.1.created_at.cmp(&left.1.created_at))
            .then_with(|| right.1.id.cmp(&left.1.id))
    });
    Ok(ranked
        .into_iter()
        .map(|(_, memory)| memory)
        .take(limit)
        .collect())
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
    let previous: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT content, embedding_id FROM memory_store \
             WHERE id = ?1 AND agent_id = ?2 AND status = 'active' AND deleted_at IS NULL",
            params![id, agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let affected = conn.execute(
        "UPDATE memory_store \
         SET name = ?1, keywords = ?2, content = ?3, updated_at = ?4, version = version + 1, \
             embedding_id = CASE WHEN content = ?3 THEN embedding_id ELSE NULL END \
         WHERE id = ?5 AND agent_id = ?6 AND status = 'active' AND deleted_at IS NULL",
        params![name, keywords, content, now(), id, agent_id],
    )?;
    if affected == 0 {
        return Err(AppError::Other(
            "Memory was not found for this agent".into(),
        ));
    }
    if let Some((previous_content, Some(embedding_id))) = previous {
        if previous_content != content {
            delete_embedding_by_id(conn, &embedding_id)?;
        }
    }
    Ok(())
}

pub fn delete(conn: &Connection, id: &str, agent_id: &str) -> AppResult<()> {
    let embedding_id = conn
        .query_row(
            "SELECT embedding_id FROM memory_store WHERE id = ?1 AND agent_id = ?2",
            params![id, agent_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
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
    if let Some(embedding_id) = embedding_id {
        delete_embedding_by_id(conn, &embedding_id)?;
    }
    Ok(())
}

pub fn get(conn: &Connection, id: &str, agent_id: &str) -> AppResult<Option<MemoryRow>> {
    let sql = format!(
        "{MEMORY_SELECT} WHERE m.id = ?1 AND m.agent_id = ?2 \
         AND m.status = 'active' AND m.deleted_at IS NULL"
    );
    conn.query_row(&sql, params![id, agent_id], row_from_sql)
        .optional()
        .map_err(Into::into)
}

pub fn upsert_memory_embedding(
    conn: &mut Connection,
    embedding_id: &str,
    memory_id: &str,
    model: &str,
    content: &str,
    vector: &[f32],
) -> AppResult<bool> {
    if model.trim().is_empty() {
        return Err(AppError::Other("Embedding model cannot be empty".into()));
    }
    if !valid_vector(vector) {
        return Err(AppError::Other(format!(
            "Embedding vector must contain 1 to {MAX_EMBEDDING_DIMS} finite values"
        )));
    }
    let dims = vector.len();
    let tx = conn.transaction()?;
    let current: Option<(String, Option<String>, String)> = tx
        .query_row(
            "SELECT content, embedding_id, agent_id FROM memory_store \
             WHERE id = ?1 AND status = 'active' AND deleted_at IS NULL",
            [memory_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((current_content, old_embedding_id, agent_id)) = current else {
        return Ok(false);
    };
    if current_content != content {
        return Ok(false);
    }
    if let Some(old_embedding_id) = old_embedding_id {
        delete_embedding_by_id_tx(&tx, &old_embedding_id)?;
    }

    ensure_vector_table(&tx, dims)?;
    tx.execute(
        "INSERT INTO embedding_items (id, ref_type, ref_id, model, dims, content_hash, created_at) \
         VALUES (?1, 'memory', ?2, ?3, ?4, ?5, ?6)",
        params![
            embedding_id,
            memory_id,
            model.trim(),
            dims as i64,
            memory_content_hash(content),
            now(),
        ],
    )?;
    let table = vector_table_name(dims)?;
    tx.execute(
        &format!(
            "INSERT INTO {table} (embedding_id, agent_id, model, vector) \
             VALUES (?1, ?2, ?3, ?4)"
        ),
        params![embedding_id, agent_id, model.trim(), f32_to_bytes(vector)],
    )?;
    tx.execute(
        "UPDATE memory_store SET embedding_id = ?1 WHERE id = ?2 AND content = ?3",
        params![embedding_id, memory_id, content],
    )?;
    tx.commit()?;
    Ok(true)
}

pub fn memory_content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn search_vector(
    conn: &Connection,
    agent_id: &str,
    query: &QueryEmbedding,
    limit: usize,
) -> AppResult<Vec<MemoryRow>> {
    let dims = query.vector.len();
    let table = vector_table_name(dims)?;
    if !table_exists(conn, &table)? {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT m.id, m.agent_id, m.name, m.keywords, m.content, m.creator, \
                m.created_at, m.updated_at, m.status, m.version, m.deleted_at, \
                m.origin_device_id, m.embedding_id, e.model, e.content_hash \
         FROM (SELECT embedding_id, distance FROM {table} \
               WHERE vector MATCH ?1 AND k = ?2 AND agent_id = ?3 AND model = ?4 \
               ORDER BY distance) nearest \
         JOIN embedding_items e ON e.id = nearest.embedding_id \
         JOIN memory_store m ON m.id = e.ref_id \
         WHERE e.ref_type = 'memory' AND e.model = ?4 AND m.agent_id = ?3 \
           AND m.status = 'active' AND m.deleted_at IS NULL \
         ORDER BY nearest.distance LIMIT ?5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            f32_to_bytes(&query.vector),
            limit as i64,
            agent_id,
            query.model,
            limit as i64,
        ],
        row_from_sql,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn valid_vector(vector: &[f32]) -> bool {
    !vector.is_empty()
        && vector.len() <= MAX_EMBEDDING_DIMS
        && vector.iter().all(|value| value.is_finite())
}

fn vector_table_name(dims: usize) -> AppResult<String> {
    if dims == 0 || dims > MAX_EMBEDDING_DIMS {
        return Err(AppError::Other(format!(
            "Unsupported embedding dimension: {dims}; expected 1 to {MAX_EMBEDDING_DIMS}"
        )));
    }
    Ok(format!("vec_embeddings_{dims}"))
}

fn ensure_vector_table(conn: &Connection, dims: usize) -> AppResult<()> {
    let table = vector_table_name(dims)?;
    if table_exists(conn, &table)? && !vector_table_has_partitions(conn, &table)? {
        conn.execute(
            "UPDATE memory_store SET embedding_id = NULL WHERE embedding_id IN (\
             SELECT id FROM embedding_items WHERE dims = ?1)",
            [dims as i64],
        )?;
        conn.execute("DELETE FROM embedding_items WHERE dims = ?1", [dims as i64])?;
        conn.execute_batch(&format!("DROP TABLE {table}"))?;
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING vec0(\
         embedding_id TEXT PRIMARY KEY, agent_id TEXT PARTITION KEY, \
         model TEXT PARTITION KEY, vector float[{dims}] distance_metric=cosine)"
    ))?;
    Ok(())
}

fn vector_table_has_partitions(conn: &Connection, table: &str) -> AppResult<bool> {
    let partition_columns: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM pragma_table_info('{table}') \
             WHERE name IN ('agent_id', 'model')"
        ),
        [],
        |row| row.get(0),
    )?;
    Ok(partition_columns == 2)
}

fn table_exists(conn: &Connection, table: &str) -> AppResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn delete_embedding_by_id(conn: &Connection, embedding_id: &str) -> AppResult<()> {
    let dims = conn
        .query_row(
            "SELECT dims FROM embedding_items WHERE id = ?1",
            [embedding_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(dims) = dims.filter(|dims| *dims > 0) {
        let table = vector_table_name(dims as usize)?;
        if table_exists(conn, &table)? {
            conn.execute(
                &format!("DELETE FROM {table} WHERE embedding_id = ?1"),
                [embedding_id],
            )?;
        }
        if dims == 1536 && table_exists(conn, "vec_embeddings")? {
            conn.execute(
                "DELETE FROM vec_embeddings WHERE embedding_id = ?1",
                [embedding_id],
            )?;
        }
    }
    conn.execute("DELETE FROM embedding_items WHERE id = ?1", [embedding_id])?;
    Ok(())
}

fn delete_embedding_by_id_tx(conn: &Transaction<'_>, embedding_id: &str) -> AppResult<()> {
    delete_embedding_by_id(conn, embedding_id)
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
        let before = get(&conn, "m1", "agent-a").unwrap().unwrap();
        let before_version: i64 = conn
            .query_row(
                "SELECT version FROM memory_store WHERE id = 'm1'",
                [],
                |row| row.get(0),
            )
            .unwrap();

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
        let after = get(&conn, "m1", "agent-a").unwrap().unwrap();
        assert_eq!(after.creator, "user");
        assert_eq!(after.created_at, before.created_at);
        let after_version: i64 = conn
            .query_row(
                "SELECT version FROM memory_store WHERE id = 'm1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after_version, before_version + 1);

        delete(&conn, "m1", "agent-a").unwrap();
        assert!(list(&conn, "agent-a").unwrap().is_empty());
    }

    #[test]
    fn indexes_and_searches_multiple_embedding_dimensions() {
        let mut conn = connection();
        let first = memory("m1", "agent-a", "user");
        let mut second = memory("m2", "agent-a", "ai");
        second.name = "Database choice".into();
        second.content = "The application stores local state in SQLite.".into();
        let mut foreign = memory("m3", "agent-b", "ai");
        foreign.name = "Foreign preference".into();
        foreign.content = "A different agent prefers another runtime.".into();
        let mut other_model = memory("m4", "agent-a", "ai");
        other_model.name = "Other model".into();
        other_model.content = "This vector belongs to a different embedding model.".into();
        assert!(insert(&conn, &first).unwrap());
        assert!(insert(&conn, &second).unwrap());
        assert!(insert(&conn, &foreign).unwrap());
        assert!(insert(&conn, &other_model).unwrap());

        assert!(upsert_memory_embedding(
            &mut conn,
            "e1",
            "m1",
            "provider/embed-3",
            &first.content,
            &[1.0, 0.0, 0.0],
        )
        .unwrap());
        assert!(upsert_memory_embedding(
            &mut conn,
            "e2",
            "m2",
            "provider/embed-4",
            &second.content,
            &[0.0, 1.0, 0.0, 0.0],
        )
        .unwrap());
        assert!(upsert_memory_embedding(
            &mut conn,
            "e3",
            "m3",
            "provider/embed-3",
            &foreign.content,
            &[1.0, 0.0, 0.0],
        )
        .unwrap());
        assert!(upsert_memory_embedding(
            &mut conn,
            "e4",
            "m4",
            "provider/other-3",
            &other_model.content,
            &[1.0, 0.0, 0.0],
        )
        .unwrap());

        let found = search_hybrid(
            &conn,
            "words absent from every memory",
            "agent-a",
            10,
            Some(&QueryEmbedding {
                model: "provider/embed-3".into(),
                vector: vec![0.99, 0.01, 0.0],
            }),
        )
        .unwrap();
        assert_eq!(
            found
                .iter()
                .map(|memory| memory.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1"]
        );

        for table in ["vec_embeddings_3", "vec_embeddings_4"] {
            assert!(table_exists(&conn, table).unwrap());
        }
    }

    #[test]
    fn content_changes_invalidate_the_previous_embedding() {
        let mut conn = connection();
        let first = memory("m1", "agent-a", "user");
        assert!(insert(&conn, &first).unwrap());
        assert!(upsert_memory_embedding(
            &mut conn,
            "e1",
            "m1",
            "provider/embed-3",
            &first.content,
            &[1.0, 0.0, 0.0],
        )
        .unwrap());

        update(
            &conn,
            "m1",
            "agent-a",
            &MemoryUpdate {
                name: first.name,
                keywords: first.keywords,
                content: "Updated content requires a new embedding.".into(),
            },
        )
        .unwrap();

        assert!(get(&conn, "m1", "agent-a")
            .unwrap()
            .unwrap()
            .embedding_id
            .is_none());
        let metadata_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embedding_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(metadata_count, 0);
    }

    #[test]
    fn rebuilds_pre_partition_vector_tables_and_invalidates_affected_metadata() {
        let mut conn = connection();
        let first = memory("m1", "agent-a", "user");
        let mut second = memory("m2", "agent-a", "ai");
        second.name = "Second memory".into();
        second.content = "Another indexed memory.".into();
        assert!(insert(&conn, &first).unwrap());
        assert!(insert(&conn, &second).unwrap());

        conn.execute_batch(
            "CREATE VIRTUAL TABLE vec_embeddings_3 USING vec0(\
             embedding_id TEXT PRIMARY KEY, vector float[3] distance_metric=cosine)",
        )
        .unwrap();
        for (embedding_id, memory) in [("old-e1", &first), ("old-e2", &second)] {
            conn.execute(
                "INSERT INTO embedding_items \
                 (id, ref_type, ref_id, model, dims, content_hash, created_at) \
                 VALUES (?1, 'memory', ?2, 'provider/embed-3', 3, ?3, '1')",
                params![
                    embedding_id,
                    memory.id,
                    memory_content_hash(&memory.content)
                ],
            )
            .unwrap();
            conn.execute(
                "UPDATE memory_store SET embedding_id = ?1 WHERE id = ?2",
                params![embedding_id, memory.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO vec_embeddings_3 (embedding_id, vector) VALUES (?1, ?2)",
                params![embedding_id, f32_to_bytes(&[1.0, 0.0, 0.0])],
            )
            .unwrap();
        }

        assert!(upsert_memory_embedding(
            &mut conn,
            "new-e1",
            "m1",
            "provider/embed-3",
            &first.content,
            &[1.0, 0.0, 0.0],
        )
        .unwrap());

        assert!(vector_table_has_partitions(&conn, "vec_embeddings_3").unwrap());
        assert!(get(&conn, "m2", "agent-a")
            .unwrap()
            .unwrap()
            .embedding_id
            .is_none());
        let old_metadata: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM embedding_items WHERE id = 'old-e2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_metadata, 0);
    }

    #[test]
    fn rejects_vectors_above_sqlite_vec_dimension_limit() {
        let mut conn = connection();
        let first = memory("m1", "agent-a", "user");
        assert!(insert(&conn, &first).unwrap());

        let error = upsert_memory_embedding(
            &mut conn,
            "e1",
            "m1",
            "provider/oversized",
            &first.content,
            &vec![0.0; MAX_EMBEDDING_DIMS + 1],
        )
        .unwrap_err();

        assert!(error.to_string().contains("1 to 8192"));
    }
}
