//! Local knowledge collection persistence and FTS retrieval.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

const DEFAULT_PARSER_PROFILE_ID: &str = "builtin-utf8-text-v1";
const DEFAULT_CHUNKER_PROFILE_ID: &str = "builtin-paragraph-1200-200-v1";
const MAX_EMBEDDING_DIMS: usize = 8_192;

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeCollectionRow {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub permission: String,
    pub document_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeDocumentRow {
    pub id: String,
    pub collection_id: String,
    pub title: String,
    pub media_type: String,
    pub status: String,
    pub current_version_id: Option<String>,
    pub chunk_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KnowledgeSearchResult {
    pub document_id: String,
    pub document_version_id: String,
    pub chunk_id: String,
    pub title: String,
    pub ordinal: i64,
    pub section_path: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct KnowledgeChunkForEmbedding {
    pub id: String,
    pub collection_id: String,
    pub content: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
pub struct KnowledgeQueryEmbedding {
    pub model: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct NewKnowledgeCollection {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub agent_id: String,
}

#[derive(Debug, Clone)]
pub struct NewDocumentChunk {
    pub content: String,
    pub section_path: Option<String>,
    pub token_count: i64,
}

#[derive(Debug, Clone)]
pub struct NewLocalDocument {
    pub collection_id: String,
    pub agent_id: String,
    pub title: String,
    pub media_type: String,
    pub local_path: String,
    pub plaintext_hash: String,
    pub size: i64,
    pub chunks: Vec<NewDocumentChunk>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportDocumentResult {
    pub document_id: String,
    pub document_version_id: String,
    pub indexed_chunks: usize,
    pub unchanged: bool,
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn hash_text(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}")
}

fn embedding_profile_id(model: &str, dims: usize) -> String {
    let material = format!("local-rag-v1:{model}:{dims}:normalized");
    format!("local-rag-{}", &hash_text(&material)[..24])
}

fn valid_vector(vector: &[f32]) -> bool {
    !vector.is_empty()
        && vector.len() <= MAX_EMBEDDING_DIMS
        && vector.iter().all(|value| value.is_finite())
}

fn rag_vector_table_name(dims: usize) -> AppResult<String> {
    if dims == 0 || dims > MAX_EMBEDDING_DIMS {
        return Err(AppError::Other(format!(
            "Unsupported embedding dimension: {dims}; expected 1 to {MAX_EMBEDDING_DIMS}"
        )));
    }
    Ok(format!("rag_vec_embeddings_{dims}"))
}

fn table_exists(conn: &Connection, table: &str) -> AppResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn ensure_rag_vector_table(conn: &Connection, dims: usize) -> AppResult<()> {
    let table = rag_vector_table_name(dims)?;
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING vec0(\
         embedding_id TEXT PRIMARY KEY, collection_id TEXT PARTITION KEY, \
         embedding_profile_id TEXT PARTITION KEY, vector float[{dims}] distance_metric=cosine)"
    ))?;
    Ok(())
}

fn f32_to_bytes(values: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values))
    }
}

fn normalize_name(value: &str, field: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Other(format!("{field} cannot be empty")));
    }
    Ok(value.to_string())
}

fn can_write_collection(conn: &Connection, collection_id: &str, agent_id: &str) -> AppResult<bool> {
    conn.query_row(
        r#"SELECT EXISTS(
             SELECT 1 FROM collection_agents
             WHERE collection_id = ?1 AND agent_id = ?2 AND permission IN ('write', 'manage')
           )"#,
        params![collection_id, agent_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn list_collections(
    conn: &Connection,
    agent_id: &str,
) -> AppResult<Vec<KnowledgeCollectionRow>> {
    let mut stmt = conn.prepare(
        r#"SELECT c.id, c.name, c.scope,
             COALESCE(a.permission, 'read'),
             COUNT(d.id), c.updated_at
           FROM knowledge_collections c
           LEFT JOIN collection_agents a ON a.collection_id = c.id AND a.agent_id = ?1
           LEFT JOIN documents d ON d.collection_id = c.id AND d.status = 'active' AND d.deleted_at IS NULL
           WHERE c.deleted_at IS NULL AND (c.scope = 'user_global' OR a.agent_id IS NOT NULL)
           GROUP BY c.id, c.name, c.scope, a.permission, c.updated_at
           ORDER BY CAST(c.updated_at AS INTEGER) DESC, c.id DESC"#,
    )?;
    let rows = stmt.query_map([agent_id], |row| {
        Ok(KnowledgeCollectionRow {
            id: row.get(0)?,
            name: row.get(1)?,
            scope: row.get(2)?,
            permission: row.get(3)?,
            document_count: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn create_collection(conn: &mut Connection, row: &NewKnowledgeCollection) -> AppResult<()> {
    let name = normalize_name(&row.name, "Collection name")?;
    if !matches!(
        row.scope.as_str(),
        "user_global" | "workspace" | "agent_private" | "custom"
    ) {
        return Err(AppError::Other("Unsupported collection scope".into()));
    }
    let timestamp = now();
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO knowledge_collections (id, name, scope, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![row.id, name, row.scope, timestamp],
    )?;
    tx.execute(
        "INSERT INTO collection_agents (collection_id, agent_id, permission, created_at, updated_at) VALUES (?1, ?2, 'manage', ?3, ?3)",
        params![row.id, row.agent_id, timestamp],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn list_documents(
    conn: &Connection,
    collection_id: &str,
    agent_id: &str,
) -> AppResult<Vec<KnowledgeDocumentRow>> {
    let visible: bool = conn.query_row(
        r#"SELECT EXISTS(
             SELECT 1 FROM knowledge_collections c
             LEFT JOIN collection_agents a ON a.collection_id = c.id AND a.agent_id = ?2
             WHERE c.id = ?1 AND c.deleted_at IS NULL AND (c.scope = 'user_global' OR a.agent_id IS NOT NULL)
           )"#,
        params![collection_id, agent_id],
        |row| row.get(0),
    )?;
    if !visible {
        return Err(AppError::Other(
            "Knowledge collection is unavailable".into(),
        ));
    }

    let mut stmt = conn.prepare(
        r#"SELECT d.id, d.collection_id, d.title, d.media_type, d.status, d.current_version_id,
             COUNT(ch.id), d.updated_at
           FROM documents d
           LEFT JOIN document_chunks ch ON ch.document_version_id = d.current_version_id
           WHERE d.collection_id = ?1 AND d.deleted_at IS NULL
           GROUP BY d.id, d.collection_id, d.title, d.media_type, d.status, d.current_version_id, d.updated_at
           ORDER BY CAST(d.updated_at AS INTEGER) DESC, d.id DESC"#,
    )?;
    let rows = stmt.query_map([collection_id], |row| {
        Ok(KnowledgeDocumentRow {
            id: row.get(0)?,
            collection_id: row.get(1)?,
            title: row.get(2)?,
            media_type: row.get(3)?,
            status: row.get(4)?,
            current_version_id: row.get(5)?,
            chunk_count: row.get(6)?,
            updated_at: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn ensure_builtin_profiles(tx: &Transaction<'_>, timestamp: &str) -> AppResult<()> {
    tx.execute(
        "INSERT OR IGNORE INTO parser_profiles (id, parser_name, parser_version, options_hash, created_at) VALUES (?1, 'utf8_text', '1', 'builtin', ?2)",
        params![DEFAULT_PARSER_PROFILE_ID, timestamp],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO chunker_profiles (id, chunker_name, chunker_version, chunk_size, overlap, options_hash, created_at) VALUES (?1, 'paragraph', '1', 1200, 200, 'builtin', ?2)",
        params![DEFAULT_CHUNKER_PROFILE_ID, timestamp],
    )?;
    Ok(())
}

pub fn import_local_document(
    conn: &mut Connection,
    input: &NewLocalDocument,
) -> AppResult<ImportDocumentResult> {
    if input.chunks.is_empty() {
        return Err(AppError::Other(
            "Document contains no indexable text".into(),
        ));
    }
    if !can_write_collection(conn, &input.collection_id, &input.agent_id)? {
        return Err(AppError::Other(
            "No write permission for this knowledge collection".into(),
        ));
    }

    let timestamp = now();
    let existing: Option<(String, Option<String>, i64, Option<String>)> = conn
        .query_row(
            r#"SELECT d.id, d.current_version_id,
                 COALESCE((SELECT MAX(v.logical_version) FROM document_versions v WHERE v.document_id = d.id), 0),
                 (SELECT v.plaintext_hash FROM document_versions v WHERE v.id = d.current_version_id)
               FROM documents d
               JOIN document_sources s ON s.document_id = d.id
               JOIN document_local_bindings b ON b.source_id = s.id
               WHERE d.collection_id = ?1 AND b.local_path = ?2
               ORDER BY CAST(s.observed_at AS INTEGER) DESC LIMIT 1"#,
            params![input.collection_id, input.local_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;

    let (document_id, source_id, logical_version, current_version_id, unchanged) = match existing {
        Some((document_id, current_version_id, logical_version, current_hash))
            if current_hash.as_deref() == Some(input.plaintext_hash.as_str()) =>
        {
            let source_id: String = conn.query_row(
                "SELECT s.id FROM document_sources s JOIN document_local_bindings b ON b.source_id = s.id WHERE s.document_id = ?1 AND b.local_path = ?2 LIMIT 1",
                params![document_id, input.local_path],
                |row| row.get(0),
            )?;
            (
                document_id,
                source_id,
                logical_version,
                current_version_id,
                true,
            )
        }
        Some((document_id, current_version_id, logical_version, _)) => {
            let source_id: String = conn.query_row(
                "SELECT s.id FROM document_sources s JOIN document_local_bindings b ON b.source_id = s.id WHERE s.document_id = ?1 AND b.local_path = ?2 LIMIT 1",
                params![document_id, input.local_path],
                |row| row.get(0),
            )?;
            (
                document_id,
                source_id,
                logical_version,
                current_version_id,
                false,
            )
        }
        None => (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            0,
            None,
            false,
        ),
    };

    if unchanged {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE document_sources SET observed_at = ?1 WHERE id = ?2",
            params![timestamp, source_id],
        )?;
        tx.execute(
            "UPDATE document_local_bindings SET updated_at = ?1 WHERE source_id = ?2",
            params![timestamp, source_id],
        )?;
        tx.execute(
            "UPDATE documents SET updated_at = ?1, status = 'active' WHERE id = ?2",
            params![timestamp, document_id],
        )?;
        tx.commit()?;
        return Ok(ImportDocumentResult {
            document_id,
            document_version_id: current_version_id.unwrap_or_default(),
            indexed_chunks: 0,
            unchanged: true,
        });
    }

    let version_id = uuid::Uuid::new_v4().to_string();
    let tx = conn.transaction()?;
    ensure_builtin_profiles(&tx, &timestamp)?;
    if current_version_id.is_none() {
        tx.execute(
            "INSERT INTO documents (id, collection_id, title, media_type, current_version_id, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6)",
            params![document_id, input.collection_id, input.title, input.media_type, version_id, timestamp],
        )?;
        tx.execute(
            "INSERT INTO document_sources (id, document_id, source_kind, observed_at, local_binding_id) VALUES (?1, ?2, 'local_file', ?3, ?1)",
            params![source_id, document_id, timestamp],
        )?;
        tx.execute(
            "INSERT INTO document_local_bindings (source_id, local_path, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![source_id, input.local_path, timestamp],
        )?;
    } else {
        tx.execute(
            "UPDATE documents SET title = ?1, media_type = ?2, current_version_id = ?3, status = 'active', updated_at = ?4 WHERE id = ?5",
            params![input.title, input.media_type, version_id, timestamp, document_id],
        )?;
        tx.execute(
            "UPDATE document_sources SET observed_at = ?1 WHERE id = ?2",
            params![timestamp, source_id],
        )?;
        tx.execute(
            "UPDATE document_local_bindings SET updated_at = ?1 WHERE source_id = ?2",
            params![timestamp, source_id],
        )?;
    }
    tx.execute(
        "INSERT INTO document_versions (id, document_id, logical_version, plaintext_hash, size, media_type, parser_profile_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![version_id, document_id, logical_version + 1, input.plaintext_hash, input.size, input.media_type, DEFAULT_PARSER_PROFILE_ID, timestamp],
    )?;
    for (ordinal, chunk) in input.chunks.iter().enumerate() {
        let chunk_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO document_chunks (id, document_version_id, ordinal, content, content_hash, section_path, token_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                chunk_id,
                version_id,
                ordinal as i64,
                chunk.content,
                hash_text(&chunk.content),
                chunk.section_path,
                chunk.token_count
            ],
        )?;
        tx.execute(
            "INSERT INTO document_chunks_fts (chunk_id, document_id, document_version_id, title, content) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                chunk_id,
                document_id,
                version_id,
                input.title,
                chunk.content
            ],
        )?;
    }
    tx.commit()?;
    Ok(ImportDocumentResult {
        document_id,
        document_version_id: version_id,
        indexed_chunks: input.chunks.len(),
        unchanged: false,
    })
}

pub fn chunks_needing_embeddings(
    conn: &Connection,
    model: &str,
    collection_id: Option<&str>,
) -> AppResult<Vec<KnowledgeChunkForEmbedding>> {
    let model = model.trim();
    if model.is_empty() {
        return Err(AppError::Other("Embedding model cannot be empty".into()));
    }
    let sql = match collection_id {
        Some(_) => {
            r#"SELECT ch.id, d.collection_id, ch.content, ch.content_hash
                      FROM document_chunks ch
                      JOIN document_versions v ON v.id = ch.document_version_id
                      JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
                      WHERE d.status = 'active' AND d.deleted_at IS NULL AND d.collection_id = ?1
                        AND NOT EXISTS (
                          SELECT 1 FROM embedding_items e
                          WHERE e.ref_type = 'document_chunk' AND e.ref_id = ch.id
                            AND e.model = ?2 AND e.content_hash = ch.content_hash
                        )
                      ORDER BY d.collection_id, v.id, ch.ordinal"#
        }
        None => {
            r#"SELECT ch.id, d.collection_id, ch.content, ch.content_hash
                   FROM document_chunks ch
                   JOIN document_versions v ON v.id = ch.document_version_id
                   JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
                   WHERE d.status = 'active' AND d.deleted_at IS NULL
                     AND NOT EXISTS (
                       SELECT 1 FROM embedding_items e
                       WHERE e.ref_type = 'document_chunk' AND e.ref_id = ch.id
                         AND e.model = ?1 AND e.content_hash = ch.content_hash
                     )
                   ORDER BY d.collection_id, v.id, ch.ordinal"#
        }
    };
    let mut statement = conn.prepare(sql)?;
    let rows = match collection_id {
        Some(collection_id) => {
            statement.query_map(params![collection_id, model], chunk_for_embedding_from_row)?
        }
        None => statement.query_map([model], chunk_for_embedding_from_row)?,
    };
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn chunk_for_embedding_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<KnowledgeChunkForEmbedding> {
    Ok(KnowledgeChunkForEmbedding {
        id: row.get(0)?,
        collection_id: row.get(1)?,
        content: row.get(2)?,
        content_hash: row.get(3)?,
    })
}

pub fn upsert_chunk_embedding(
    conn: &mut Connection,
    embedding_id: &str,
    chunk_id: &str,
    collection_id: &str,
    model: &str,
    content_hash: &str,
    vector: &[f32],
) -> AppResult<bool> {
    let model = model.trim();
    if model.is_empty() || !valid_vector(vector) {
        return Err(AppError::Other("Invalid knowledge embedding".into()));
    }
    let dims = vector.len();
    let profile_id = embedding_profile_id(model, dims);
    let tx = conn.transaction()?;
    let current: Option<(String, String, String)> = tx
        .query_row(
            r#"SELECT ch.content_hash, d.collection_id, ch.id
               FROM document_chunks ch
               JOIN document_versions v ON v.id = ch.document_version_id
               JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
               WHERE ch.id = ?1 AND d.status = 'active' AND d.deleted_at IS NULL"#,
            [chunk_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((current_hash, current_collection_id, _)) = current else {
        return Ok(false);
    };
    if current_hash != content_hash || current_collection_id != collection_id {
        return Ok(false);
    }

    let old_embeddings = {
        let mut old_statement = tx.prepare(
            "SELECT id, dims FROM embedding_items WHERE ref_type = 'document_chunk' AND ref_id = ?1",
        )?;
        let rows = old_statement.query_map([chunk_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (old_id, old_dims) in old_embeddings {
        if old_dims > 0 {
            let table = rag_vector_table_name(old_dims as usize)?;
            if table_exists(&tx, &table)? {
                tx.execute(
                    &format!("DELETE FROM {table} WHERE embedding_id = ?1"),
                    [old_id],
                )?;
            }
        }
    }
    tx.execute(
        "DELETE FROM embedding_items WHERE ref_type = 'document_chunk' AND ref_id = ?1",
        [chunk_id],
    )?;
    ensure_rag_vector_table(&tx, dims)?;
    tx.execute(
        "INSERT OR IGNORE INTO embedding_profiles \
         (id, model_ref, dims, normalized, instruction_hash, created_at) \
         VALUES (?1, ?2, ?3, 1, 'local-rag-v1', ?4)",
        params![profile_id, model, dims as i64, now()],
    )?;
    tx.execute(
        "INSERT INTO embedding_items \
         (id, ref_type, ref_id, collection_id, embedding_profile_id, model, dims, content_hash, created_at) \
         VALUES (?1, 'document_chunk', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![embedding_id, chunk_id, collection_id, profile_id, model, dims as i64, content_hash, now()],
    )?;
    let table = rag_vector_table_name(dims)?;
    tx.execute(
        &format!(
            "INSERT INTO {table} (embedding_id, collection_id, embedding_profile_id, vector) \
             VALUES (?1, ?2, ?3, ?4)"
        ),
        params![
            embedding_id,
            collection_id,
            profile_id,
            f32_to_bytes(vector)
        ],
    )?;
    tx.commit()?;
    Ok(true)
}

pub fn search_vector(
    conn: &Connection,
    collection_id: &str,
    query: &KnowledgeQueryEmbedding,
    limit: usize,
) -> AppResult<Vec<KnowledgeSearchResult>> {
    if query.model.trim().is_empty() || !valid_vector(&query.vector) {
        return Ok(Vec::new());
    }
    let dims = query.vector.len();
    let table = rag_vector_table_name(dims)?;
    if !table_exists(conn, &table)? {
        return Ok(Vec::new());
    }
    let profile_id = embedding_profile_id(query.model.trim(), dims);
    let sql = format!(
        r#"SELECT d.id, v.id, ch.id, d.title, ch.ordinal, ch.section_path, ch.content
            FROM (SELECT embedding_id, distance FROM {table}
                  WHERE vector MATCH ?1 AND k = ?2 AND collection_id = ?3 AND embedding_profile_id = ?4
                  ORDER BY distance) nearest
            JOIN embedding_items e ON e.id = nearest.embedding_id
            JOIN document_chunks ch ON ch.id = e.ref_id
            JOIN document_versions v ON v.id = ch.document_version_id
            JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
            WHERE e.ref_type = 'document_chunk' AND d.status = 'active' AND d.deleted_at IS NULL
            ORDER BY nearest.distance, d.id, ch.ordinal LIMIT ?2"#,
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        params![
            f32_to_bytes(&query.vector),
            limit.clamp(1, 50) as i64,
            collection_id,
            profile_id
        ],
        search_result_from_row,
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn fts_prefix_query(query: &str) -> String {
    query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn search_result_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<KnowledgeSearchResult> {
    Ok(KnowledgeSearchResult {
        document_id: row.get(0)?,
        document_version_id: row.get(1)?,
        chunk_id: row.get(2)?,
        title: row.get(3)?,
        ordinal: row.get(4)?,
        section_path: row.get(5)?,
        content: row.get(6)?,
    })
}

pub fn search(
    conn: &Connection,
    agent_id: &str,
    query: &str,
    collection_id: Option<&str>,
    limit: usize,
) -> AppResult<Vec<KnowledgeSearchResult>> {
    let fts_query = fts_prefix_query(query);
    if fts_query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 50) as i64;
    let rows = match collection_id {
        Some(collection_id) => {
            let mut stmt = conn.prepare(
                r#"SELECT d.id, v.id, ch.id, d.title, ch.ordinal, ch.section_path, ch.content
                   FROM document_chunks_fts f
                   JOIN document_chunks ch ON ch.id = f.chunk_id
                   JOIN document_versions v ON v.id = ch.document_version_id
                   JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
                   JOIN knowledge_collections c ON c.id = d.collection_id
                   LEFT JOIN collection_agents a ON a.collection_id = c.id AND a.agent_id = ?1
                   WHERE f.document_chunks_fts MATCH ?2 AND d.collection_id = ?3
                     AND d.status = 'active' AND d.deleted_at IS NULL AND c.deleted_at IS NULL
                     AND (c.scope = 'user_global' OR a.agent_id IS NOT NULL)
                   ORDER BY bm25(document_chunks_fts), d.id, ch.ordinal LIMIT ?4"#,
            )?;
            let results = stmt.query_map(
                params![agent_id, fts_query, collection_id, limit],
                search_result_from_row,
            )?;
            results.collect::<Result<Vec<_>, _>>()?
        }
        None => {
            let mut stmt = conn.prepare(
                r#"SELECT d.id, v.id, ch.id, d.title, ch.ordinal, ch.section_path, ch.content
                   FROM document_chunks_fts f
                   JOIN document_chunks ch ON ch.id = f.chunk_id
                   JOIN document_versions v ON v.id = ch.document_version_id
                   JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
                   JOIN knowledge_collections c ON c.id = d.collection_id
                   LEFT JOIN collection_agents a ON a.collection_id = c.id AND a.agent_id = ?1
                   WHERE f.document_chunks_fts MATCH ?2
                     AND d.status = 'active' AND d.deleted_at IS NULL AND c.deleted_at IS NULL
                     AND (c.scope = 'user_global' OR a.agent_id IS NOT NULL)
                   ORDER BY bm25(document_chunks_fts), d.id, ch.ordinal LIMIT ?3"#,
            )?;
            let results =
                stmt.query_map(params![agent_id, fts_query, limit], search_result_from_row)?;
            results.collect::<Result<Vec<_>, _>>()?
        }
    };
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_current_versions_searches_fts_and_enforces_collection_access() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_collection(
            &mut conn,
            &NewKnowledgeCollection {
                id: "collection-1".into(),
                name: "Private notes".into(),
                scope: "agent_private".into(),
                agent_id: "agnes".into(),
            },
        )
        .unwrap();

        let input = NewLocalDocument {
            collection_id: "collection-1".into(),
            agent_id: "agnes".into(),
            title: "Gardening".into(),
            media_type: "text/markdown".into(),
            local_path: "/tmp/gardening.md".into(),
            plaintext_hash: hash_text("Rhubarb prefers cool weather."),
            size: 29,
            chunks: vec![NewDocumentChunk {
                content: "Rhubarb prefers cool weather.".into(),
                section_path: Some("Garden".into()),
                token_count: 4,
            }],
        };
        let imported = import_local_document(&mut conn, &input).unwrap();
        assert!(!imported.unchanged);
        assert_eq!(imported.indexed_chunks, 1);

        let results = search(&conn, "agnes", "rhubarb", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Gardening");
        assert_eq!(results[0].section_path.as_deref(), Some("Garden"));
        assert!(search(&conn, "nova", "rhubarb", None, 10)
            .unwrap()
            .is_empty());

        let unchanged = import_local_document(&mut conn, &input).unwrap();
        assert!(unchanged.unchanged);
        assert_eq!(unchanged.document_version_id, imported.document_version_id);
        assert_eq!(
            list_documents(&conn, "collection-1", "agnes").unwrap()[0].chunk_count,
            1
        );
    }

    #[test]
    fn indexes_and_searches_chunk_vectors_by_collection_and_profile() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_collection(
            &mut conn,
            &NewKnowledgeCollection {
                id: "collection-a".into(),
                name: "A".into(),
                scope: "agent_private".into(),
                agent_id: "agnes".into(),
            },
        )
        .unwrap();
        create_collection(
            &mut conn,
            &NewKnowledgeCollection {
                id: "collection-b".into(),
                name: "B".into(),
                scope: "agent_private".into(),
                agent_id: "agnes".into(),
            },
        )
        .unwrap();
        for (collection_id, path, content) in [
            (
                "collection-a",
                "/tmp/a.md",
                "Rhubarb grows in cool gardens.",
            ),
            ("collection-b", "/tmp/b.md", "Unrelated warm weather notes."),
        ] {
            import_local_document(
                &mut conn,
                &NewLocalDocument {
                    collection_id: collection_id.into(),
                    agent_id: "agnes".into(),
                    title: collection_id.into(),
                    media_type: "text/markdown".into(),
                    local_path: path.into(),
                    plaintext_hash: hash_text(content),
                    size: content.len() as i64,
                    chunks: vec![NewDocumentChunk {
                        content: content.into(),
                        section_path: None,
                        token_count: 5,
                    }],
                },
            )
            .unwrap();
        }
        for chunk in chunks_needing_embeddings(&conn, "test/embed", None).unwrap() {
            let vector = if chunk.collection_id == "collection-a" {
                vec![1.0, 0.0, 0.0]
            } else {
                vec![0.0, 1.0, 0.0]
            };
            assert!(upsert_chunk_embedding(
                &mut conn,
                &format!("embedding-{}", chunk.id),
                &chunk.id,
                &chunk.collection_id,
                "test/embed",
                &chunk.content_hash,
                &vector,
            )
            .unwrap());
        }
        assert!(chunks_needing_embeddings(&conn, "test/embed", None)
            .unwrap()
            .is_empty());
        let results = search_vector(
            &conn,
            "collection-a",
            &KnowledgeQueryEmbedding {
                model: "test/embed".into(),
                vector: vec![0.99, 0.01, 0.0],
            },
            5,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "collection-a");
        assert!(search_vector(
            &conn,
            "collection-a",
            &KnowledgeQueryEmbedding {
                model: "different/model".into(),
                vector: vec![0.99, 0.01, 0.0],
            },
            5,
        )
        .unwrap()
        .is_empty());
    }
}
