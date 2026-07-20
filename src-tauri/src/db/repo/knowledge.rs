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
    pub embedded_chunk_count: i64,
    pub artifact_id: Option<String>,
    pub artifact_status: Option<String>,
    pub ready_replica_count: i64,
    pub device_artifact_status: Option<String>,
    pub device_artifact_error: Option<String>,
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
pub struct KnowledgeArtifactEmbeddingProfile {
    pub id: String,
    pub model_ref: String,
    pub model_revision: Option<String>,
    pub dims: usize,
    pub normalized: bool,
    pub instruction_hash: String,
    pub tokenizer_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct KnowledgeArtifactChunk {
    pub id: String,
    pub ordinal: i64,
    pub content: String,
    pub content_hash: String,
    pub page: Option<i64>,
    pub section_path: Option<String>,
    pub token_count: i64,
    pub metadata: String,
    pub embedding_id: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct KnowledgeArtifactSnapshot {
    pub source_version_id: String,
    pub document_id: String,
    pub collection_id: String,
    pub title: String,
    pub media_type: String,
    pub source_plaintext_hash: String,
    pub source_size: u64,
    pub logical_version: i64,
    pub parser_profile_id: Option<String>,
    pub chunker_profile_id: String,
    pub profile: KnowledgeArtifactEmbeddingProfile,
    pub chunks: Vec<KnowledgeArtifactChunk>,
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
pub struct DocumentParserProfile {
    pub id: String,
    pub name: String,
    pub version: String,
    pub options_hash: String,
}

impl DocumentParserProfile {
    pub fn builtin_text() -> Self {
        Self {
            id: DEFAULT_PARSER_PROFILE_ID.into(),
            name: "utf8_text".into(),
            version: "1".into(),
            options_hash: "builtin".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewDocumentChunk {
    pub content: String,
    pub page: Option<i64>,
    pub section_path: Option<String>,
    pub token_count: i64,
    pub metadata: String,
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
    pub parser_profile: DocumentParserProfile,
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

fn is_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
           WHERE c.deleted_at IS NULL
             AND NOT EXISTS (
               SELECT 1 FROM reading_books b
               WHERE b.collection_id = c.id AND b.deleted_at IS NULL
             )
             AND (c.scope = 'user_global' OR a.agent_id IS NOT NULL)
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
             COUNT(DISTINCT ch.id), COUNT(DISTINCT e.ref_id), am.id, am.local_status,
             COALESCE((SELECT COUNT(*) FROM artifact_replicas r
                       WHERE r.artifact_id = am.id AND r.status = 'ready'), 0),
             das.local_status, das.last_error_code, d.updated_at
           FROM documents d
           LEFT JOIN document_chunks ch ON ch.document_version_id = d.current_version_id
           LEFT JOIN embedding_items e ON e.ref_type = 'document_chunk' AND e.ref_id = ch.id
           LEFT JOIN artifact_manifests am ON am.id = (
             SELECT candidate.id FROM artifact_manifests candidate
             WHERE candidate.artifact_type = 'knowledge_vectors'
               AND candidate.source_version_id = d.current_version_id
             ORDER BY CASE candidate.local_status
                        WHEN 'installed' THEN 3 WHEN 'available' THEN 2 WHEN 'built' THEN 1 ELSE 0
                      END DESC,
                      CAST(candidate.created_at AS INTEGER) DESC,
                      candidate.id DESC
             LIMIT 1
           )
           LEFT JOIN device_artifact_states das ON das.artifact_id = am.id
             AND das.device_id = (SELECT device_id FROM sync_runtime_state WHERE singleton = 1)
           WHERE d.collection_id = ?1 AND d.deleted_at IS NULL
           GROUP BY d.id, d.collection_id, d.title, d.media_type, d.status, d.current_version_id,
             am.id, am.local_status, das.local_status, das.last_error_code, d.updated_at
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
            embedded_chunk_count: row.get(7)?,
            artifact_id: row.get(8)?,
            artifact_status: row.get(9)?,
            ready_replica_count: row.get(10)?,
            device_artifact_status: row.get(11)?,
            device_artifact_error: row.get(12)?,
            updated_at: row.get(13)?,
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

fn ensure_parser_profile(
    tx: &Transaction<'_>,
    profile: &DocumentParserProfile,
    timestamp: &str,
) -> AppResult<()> {
    if profile.id.trim().is_empty()
        || profile.name.trim().is_empty()
        || profile.version.trim().is_empty()
        || profile.options_hash.trim().is_empty()
    {
        return Err(AppError::Other("Document parser profile is invalid".into()));
    }
    tx.execute(
        "INSERT OR IGNORE INTO parser_profiles (id, parser_name, parser_version, options_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            profile.id,
            profile.name,
            profile.version,
            profile.options_hash,
            timestamp
        ],
    )?;
    let stored: (String, String, String) = tx.query_row(
        "SELECT parser_name, parser_version, options_hash FROM parser_profiles WHERE id = ?1",
        [&profile.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if stored
        != (
            profile.name.clone(),
            profile.version.clone(),
            profile.options_hash.clone(),
        )
    {
        return Err(AppError::Other(
            "Document parser profile identifier conflicts with an existing profile".into(),
        ));
    }
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
    if input.chunks.iter().any(|chunk| {
        chunk.content.trim().is_empty()
            || chunk.token_count <= 0
            || serde_json::from_str::<serde_json::Value>(&chunk.metadata).is_err()
    }) {
        return Err(AppError::Other("Document chunk metadata is invalid".into()));
    }
    if !can_write_collection(conn, &input.collection_id, &input.agent_id)? {
        return Err(AppError::Other(
            "No write permission for this knowledge collection".into(),
        ));
    }

    let timestamp = now();
    let existing: Option<(
        String,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = conn
        .query_row(
            r#"SELECT d.id, d.current_version_id,
                 COALESCE((SELECT MAX(v.logical_version) FROM document_versions v WHERE v.document_id = d.id), 0),
                 (SELECT v.plaintext_hash FROM document_versions v WHERE v.id = d.current_version_id),
                 (SELECT v.parser_profile_id FROM document_versions v WHERE v.id = d.current_version_id),
                 (SELECT v.media_type FROM document_versions v WHERE v.id = d.current_version_id)
               FROM documents d
               JOIN document_sources s ON s.document_id = d.id
               JOIN document_local_bindings b ON b.source_id = s.id
               WHERE d.collection_id = ?1 AND b.local_path = ?2
               ORDER BY CAST(s.observed_at AS INTEGER) DESC LIMIT 1"#,
            params![input.collection_id, input.local_path],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;

    let (document_id, source_id, logical_version, current_version_id, unchanged) = match existing {
        Some((
            document_id,
            current_version_id,
            logical_version,
            current_hash,
            current_parser_profile_id,
            current_media_type,
        )) if current_hash.as_deref() == Some(input.plaintext_hash.as_str())
            && current_parser_profile_id.as_deref() == Some(input.parser_profile.id.as_str())
            && current_media_type.as_deref() == Some(input.media_type.as_str()) =>
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
        Some((document_id, current_version_id, logical_version, _, _, _)) => {
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
    ensure_parser_profile(&tx, &input.parser_profile, &timestamp)?;
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
        params![version_id, document_id, logical_version + 1, input.plaintext_hash, input.size, input.media_type, input.parser_profile.id, timestamp],
    )?;
    for (ordinal, chunk) in input.chunks.iter().enumerate() {
        let chunk_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO document_chunks (id, document_version_id, ordinal, content, content_hash, page, section_path, token_count, metadata) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                chunk_id,
                version_id,
                ordinal as i64,
                chunk.content,
                hash_text(&chunk.content),
                chunk.page,
                chunk.section_path,
                chunk.token_count,
                chunk.metadata
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

pub fn export_artifact_snapshot(
    conn: &Connection,
    source_version_id: &str,
) -> AppResult<KnowledgeArtifactSnapshot> {
    let source = conn
        .query_row(
            r#"SELECT d.id, d.collection_id, d.title, v.media_type, v.plaintext_hash,
                 v.size, v.logical_version, v.parser_profile_id
               FROM document_versions v
               JOIN documents d ON d.id = v.document_id AND d.current_version_id = v.id
               WHERE v.id = ?1 AND d.status = 'active' AND d.deleted_at IS NULL"#,
            [source_version_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::Other("Knowledge source version is not current".into()))?;
    let (
        document_id,
        collection_id,
        title,
        media_type,
        plaintext_hash,
        size,
        logical_version,
        parser_profile_id,
    ) = source;
    let source_size = u64::try_from(size)
        .map_err(|_| AppError::Other("Knowledge source size is invalid".into()))?;

    let profile = conn
        .query_row(
            r#"SELECT p.id, p.model_ref, p.model_revision, p.dims, p.normalized,
                 p.instruction_hash, p.tokenizer_ref
               FROM document_chunks ch
               JOIN embedding_items e ON e.ref_type = 'document_chunk' AND e.ref_id = ch.id
               JOIN embedding_profiles p ON p.id = e.embedding_profile_id
               WHERE ch.document_version_id = ?1
               ORDER BY ch.ordinal LIMIT 1"#,
            [source_version_id],
            |row| {
                Ok(KnowledgeArtifactEmbeddingProfile {
                    id: row.get(0)?,
                    model_ref: row.get(1)?,
                    model_revision: row.get(2)?,
                    dims: row.get::<_, i64>(3)? as usize,
                    normalized: row.get::<_, i64>(4)? != 0,
                    instruction_hash: row.get(5)?,
                    tokenizer_ref: row.get(6)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| {
            AppError::Other("Knowledge source has no complete embedding profile".into())
        })?;
    if profile.model_ref.trim().is_empty()
        || profile.instruction_hash.trim().is_empty()
        || profile.dims == 0
        || profile.dims > MAX_EMBEDDING_DIMS
    {
        return Err(AppError::Other(
            "Knowledge embedding profile is invalid".into(),
        ));
    }
    let table = rag_vector_table_name(profile.dims)?;
    if !table_exists(conn, &table)? {
        return Err(AppError::Other(
            "Knowledge vector partition is missing".into(),
        ));
    }
    let sql = format!(
        r#"SELECT ch.id, ch.ordinal, ch.content, ch.content_hash, ch.page,
             ch.section_path, ch.token_count, ch.metadata, e.id, e.collection_id,
             e.embedding_profile_id, e.model, e.dims, e.content_hash, vec.vector
           FROM document_chunks ch
           JOIN embedding_items e ON e.ref_type = 'document_chunk' AND e.ref_id = ch.id
           JOIN {table} vec ON vec.embedding_id = e.id
           WHERE ch.document_version_id = ?1
           ORDER BY ch.ordinal"#,
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([source_version_id], |row| {
        Ok((
            KnowledgeArtifactChunk {
                id: row.get(0)?,
                ordinal: row.get(1)?,
                content: row.get(2)?,
                content_hash: row.get(3)?,
                page: row.get(4)?,
                section_path: row.get(5)?,
                token_count: row.get(6)?,
                metadata: row.get(7)?,
                embedding_id: row.get(8)?,
                vector: vector_from_bytes(&row.get::<_, Vec<u8>>(14)?, profile.dims)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?,
            },
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, i64>(12)?,
            row.get::<_, String>(13)?,
        ))
    })?;
    let mut chunks = Vec::new();
    for row in rows {
        let (chunk, row_collection_id, profile_id, model, dims, embedding_hash) = row?;
        if row_collection_id != collection_id
            || profile_id != profile.id
            || model != profile.model_ref
            || dims != profile.dims as i64
            || embedding_hash != chunk.content_hash
            || hash_text(&chunk.content) != chunk.content_hash
            || chunk.ordinal != chunks.len() as i64
            || !valid_vector(&chunk.vector)
        {
            return Err(AppError::Other(
                "Knowledge artifact source is internally inconsistent".into(),
            ));
        }
        chunks.push(chunk);
    }
    let expected_chunks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM document_chunks WHERE document_version_id = ?1",
        [source_version_id],
        |row| row.get(0),
    )?;
    if chunks.is_empty() || expected_chunks != chunks.len() as i64 {
        return Err(AppError::Other(
            "Knowledge source embeddings are incomplete".into(),
        ));
    }
    Ok(KnowledgeArtifactSnapshot {
        source_version_id: source_version_id.into(),
        document_id,
        collection_id,
        title,
        media_type,
        source_plaintext_hash: plaintext_hash,
        source_size,
        logical_version,
        parser_profile_id,
        chunker_profile_id: DEFAULT_CHUNKER_PROFILE_ID.into(),
        profile,
        chunks,
    })
}

fn vector_from_bytes(bytes: &[u8], dims: usize) -> AppResult<Vec<f32>> {
    if bytes.len() != dims.saturating_mul(std::mem::size_of::<f32>()) {
        return Err(AppError::Other(
            "Knowledge vector byte length is invalid".into(),
        ));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte chunk")))
        .collect::<Vec<_>>();
    if !valid_vector(&vector) {
        return Err(AppError::Other("Knowledge vector is invalid".into()));
    }
    Ok(vector)
}

pub fn import_artifact_snapshot(
    conn: &mut Connection,
    input: &KnowledgeArtifactSnapshot,
) -> AppResult<usize> {
    if input.source_version_id.trim().is_empty()
        || input.document_id.trim().is_empty()
        || input.collection_id.trim().is_empty()
        || input.title.trim().is_empty()
        || input.media_type.trim().is_empty()
        || input.chunks.is_empty()
        || !is_hash(&input.source_plaintext_hash)
        || input.logical_version <= 0
        || input.chunker_profile_id.trim().is_empty()
        || input.profile.id.trim().is_empty()
        || input.profile.model_ref.trim().is_empty()
        || input.profile.instruction_hash.trim().is_empty()
        || input.profile.dims == 0
        || input.profile.dims > MAX_EMBEDDING_DIMS
    {
        return Err(AppError::Other(
            "Knowledge artifact manifest is invalid".into(),
        ));
    }
    let expected_profile_id = embedding_profile_id(&input.profile.model_ref, input.profile.dims);
    if input.profile.id != expected_profile_id {
        return Err(AppError::Other(
            "Knowledge artifact embedding profile is invalid".into(),
        ));
    }
    let table = rag_vector_table_name(input.profile.dims)?;
    let timestamp = now();
    let source_size = i64::try_from(input.source_size)
        .map_err(|_| AppError::Other("Knowledge source size is invalid".into()))?;
    let tx = conn.transaction()?;
    let collection_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM knowledge_collections WHERE id = ?1 AND deleted_at IS NULL)",
        [&input.collection_id],
        |row| row.get(0),
    )?;
    if !collection_exists {
        return Err(AppError::Other(
            "Knowledge collection is unavailable".into(),
        ));
    }
    let existing_document: Option<(String, Option<String>, Option<i64>)> = tx
        .query_row(
            r#"SELECT d.collection_id, d.current_version_id, v.logical_version
               FROM documents d
               LEFT JOIN document_versions v ON v.id = d.current_version_id
               WHERE d.id = ?1"#,
            [&input.document_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if existing_document
        .as_ref()
        .is_some_and(|(collection_id, _, _)| collection_id != &input.collection_id)
    {
        return Err(AppError::Other(
            "Knowledge document belongs to a different collection".into(),
        ));
    }
    let advance_current = match &existing_document {
        None | Some((_, None, None)) => true,
        Some((_, Some(current_version_id), Some(current_logical_version)))
            if current_version_id == &input.source_version_id =>
        {
            true
        }
        Some((_, Some(_), Some(current_logical_version)))
            if *current_logical_version < input.logical_version =>
        {
            true
        }
        Some((_, Some(_), Some(current_logical_version)))
            if *current_logical_version == input.logical_version =>
        {
            return Err(AppError::Other(
                "Knowledge document logical version conflicts with the current version".into(),
            ));
        }
        Some((_, Some(_), Some(_))) => false,
        Some(_) => {
            return Err(AppError::Other(
                "Knowledge document current version is invalid".into(),
            ));
        }
    };
    if existing_document.is_none() {
        tx.execute(
            "INSERT INTO documents (id, collection_id, title, media_type, current_version_id, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6)",
            params![
                input.document_id,
                input.collection_id,
                input.title,
                input.media_type,
                input.source_version_id,
                timestamp
            ],
        )?;
    } else if advance_current {
        tx.execute(
            "UPDATE documents SET title = ?1, media_type = ?2, current_version_id = ?3, status = 'active', deleted_at = NULL, updated_at = ?4 WHERE id = ?5",
            params![
                input.title,
                input.media_type,
                input.source_version_id,
                timestamp,
                input.document_id
            ],
        )?;
    }
    let existing_version: Option<(String, i64, String, i64, String, Option<String>)> = tx
        .query_row(
            "SELECT document_id, logical_version, plaintext_hash, size, media_type, parser_profile_id FROM document_versions WHERE id = ?1",
            [&input.source_version_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .optional()?;
    match existing_version {
        Some((
            document_id,
            logical_version,
            plaintext_hash,
            size,
            media_type,
            parser_profile_id,
        )) if document_id == input.document_id
            && logical_version == input.logical_version
            && plaintext_hash == input.source_plaintext_hash
            && size == source_size
            && media_type == input.media_type
            && parser_profile_id == input.parser_profile_id => {}
        Some(_) => {
            return Err(AppError::Other(
                "Knowledge source version is immutable and does not match".into(),
            ));
        }
        None => {
            tx.execute(
                "INSERT INTO document_versions (id, document_id, logical_version, plaintext_hash, size, media_type, parser_profile_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    input.source_version_id,
                    input.document_id,
                    input.logical_version,
                    input.source_plaintext_hash,
                    source_size,
                    input.media_type,
                    input.parser_profile_id,
                    timestamp
                ],
            )?;
        }
    }
    tx.execute(
        "INSERT OR IGNORE INTO document_sources (id, document_id, source_kind, observed_at) VALUES (?1, ?2, 'artifact', ?3)",
        params![format!("artifact-source-{}", input.source_version_id), input.document_id, timestamp],
    )?;
    tx.execute(
        "DELETE FROM document_chunks_fts WHERE document_version_id = ?1",
        [&input.source_version_id],
    )?;
    let mut old_embeddings = Vec::new();
    {
        let mut statement = tx.prepare(
            "SELECT e.id, e.dims FROM embedding_items e JOIN document_chunks ch ON ch.id = e.ref_id WHERE ch.document_version_id = ?1",
        )?;
        let rows = statement.query_map([&input.source_version_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            old_embeddings.push(row?);
        }
    }
    for (embedding_id, dims) in old_embeddings {
        let old_table = rag_vector_table_name(dims as usize)?;
        if table_exists(&tx, &old_table)? {
            tx.execute(
                &format!("DELETE FROM {old_table} WHERE embedding_id = ?1"),
                [&embedding_id],
            )?;
        }
    }
    tx.execute(
        "DELETE FROM embedding_items WHERE ref_type = 'document_chunk' AND ref_id IN (SELECT id FROM document_chunks WHERE document_version_id = ?1)",
        [&input.source_version_id],
    )?;
    tx.execute(
        "DELETE FROM document_chunks WHERE document_version_id = ?1",
        [&input.source_version_id],
    )?;
    ensure_rag_vector_table(&tx, input.profile.dims)?;
    tx.execute(
        "INSERT OR IGNORE INTO embedding_profiles (id, model_ref, model_revision, dims, normalized, instruction_hash, tokenizer_ref, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            input.profile.id,
            input.profile.model_ref,
            input.profile.model_revision,
            input.profile.dims as i64,
            i64::from(input.profile.normalized),
            input.profile.instruction_hash,
            input.profile.tokenizer_ref,
            timestamp
        ],
    )?;
    let stored_profile: (String, Option<String>, i64, i64, String, Option<String>) = tx.query_row(
        "SELECT model_ref, model_revision, dims, normalized, instruction_hash, tokenizer_ref FROM embedding_profiles WHERE id = ?1",
        [&input.profile.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    )?;
    if stored_profile.0 != input.profile.model_ref
        || stored_profile.1 != input.profile.model_revision
        || stored_profile.2 != input.profile.dims as i64
        || stored_profile.3 != i64::from(input.profile.normalized)
        || stored_profile.4 != input.profile.instruction_hash
        || stored_profile.5 != input.profile.tokenizer_ref
    {
        return Err(AppError::Other(
            "Knowledge embedding profile ID is bound to different metadata".into(),
        ));
    }
    for (expected_ordinal, chunk) in input.chunks.iter().enumerate() {
        if chunk.ordinal != expected_ordinal as i64
            || chunk.id.trim().is_empty()
            || chunk.embedding_id.trim().is_empty()
            || hash_text(&chunk.content) != chunk.content_hash
            || chunk.vector.len() != input.profile.dims
            || !valid_vector(&chunk.vector)
            || serde_json::from_str::<serde_json::Value>(&chunk.metadata).is_err()
        {
            return Err(AppError::Other(
                "Knowledge artifact chunk or vector is invalid".into(),
            ));
        }
        let embedding_owner: Option<String> = tx
            .query_row(
                "SELECT ref_id FROM embedding_items WHERE id = ?1",
                [&chunk.embedding_id],
                |row| row.get(0),
            )
            .optional()?;
        if embedding_owner.is_some_and(|owner| owner != chunk.id) {
            return Err(AppError::Other(
                "Knowledge artifact embedding ID is already in use".into(),
            ));
        }
        tx.execute(
            "INSERT INTO document_chunks (id, document_version_id, ordinal, content, content_hash, page, section_path, token_count, metadata) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                chunk.id,
                input.source_version_id,
                chunk.ordinal,
                chunk.content,
                chunk.content_hash,
                chunk.page,
                chunk.section_path,
                chunk.token_count,
                chunk.metadata
            ],
        )?;
        tx.execute(
            "INSERT INTO document_chunks_fts (chunk_id, document_id, document_version_id, title, content) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![chunk.id, input.document_id, input.source_version_id, input.title, chunk.content],
        )?;
        tx.execute(
            "INSERT INTO embedding_items (id, ref_type, ref_id, collection_id, embedding_profile_id, model, dims, content_hash, created_at) VALUES (?1, 'document_chunk', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                chunk.embedding_id,
                chunk.id,
                input.collection_id,
                input.profile.id,
                input.profile.model_ref,
                input.profile.dims as i64,
                chunk.content_hash,
                timestamp
            ],
        )?;
        tx.execute(
            &format!(
                "INSERT INTO {table} (embedding_id, collection_id, embedding_profile_id, vector) VALUES (?1, ?2, ?3, ?4)"
            ),
            params![
                chunk.embedding_id,
                input.collection_id,
                input.profile.id,
                f32_to_bytes(&chunk.vector)
            ],
        )?;
    }
    tx.commit()?;
    Ok(input.chunks.len())
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
                     AND NOT EXISTS (
                       SELECT 1 FROM reading_books b
                       WHERE b.collection_id = c.id AND b.deleted_at IS NULL
                     )
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
            parser_profile: DocumentParserProfile::builtin_text(),
            chunks: vec![NewDocumentChunk {
                content: "Rhubarb prefers cool weather.".into(),
                page: Some(12),
                section_path: Some("Garden".into()),
                token_count: 4,
                metadata: r#"{"kind":"paragraph"}"#.into(),
            }],
        };
        let imported = import_local_document(&mut conn, &input).unwrap();
        assert!(!imported.unchanged);
        assert_eq!(imported.indexed_chunks, 1);
        let (page, metadata, parser_profile_id): (Option<i64>, String, Option<String>) = conn
            .query_row(
                "SELECT ch.page, ch.metadata, v.parser_profile_id FROM document_chunks ch JOIN document_versions v ON v.id = ch.document_version_id WHERE v.id = ?1",
                [&imported.document_version_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(page, Some(12));
        assert_eq!(metadata, r#"{"kind":"paragraph"}"#);
        assert_eq!(
            parser_profile_id.as_deref(),
            Some(DEFAULT_PARSER_PROFILE_ID)
        );

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

        let mut reparsed = input.clone();
        reparsed.parser_profile = DocumentParserProfile {
            id: "test-parser-v2".into(),
            name: "test_parser".into(),
            version: "2".into(),
            options_hash: "updated-options".into(),
        };
        let reparsed_result = import_local_document(&mut conn, &reparsed).unwrap();
        assert!(!reparsed_result.unchanged);
        assert_ne!(
            reparsed_result.document_version_id,
            imported.document_version_id
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
                    parser_profile: DocumentParserProfile::builtin_text(),
                    chunks: vec![NewDocumentChunk {
                        content: content.into(),
                        page: None,
                        section_path: None,
                        token_count: 5,
                        metadata: "{}".into(),
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

        let (document_id, source_version_id): (String, String) = conn
            .query_row(
                "SELECT id, current_version_id FROM documents WHERE collection_id = 'collection-a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let snapshot = export_artifact_snapshot(&conn, &source_version_id).unwrap();
        let built = crate::sync::knowledge_artifact::build_from_snapshot(
            &crate::sync::crypto::SyncMasterKey::generate(),
            1,
            &snapshot,
        )
        .unwrap();
        crate::db::repo::artifacts::upsert_manifest(
            &conn,
            &crate::db::repo::artifacts::UpsertArtifactManifest {
                manifest: built.manifest.clone(),
                local_path: None,
                local_status: "installed".into(),
                installed_at: Some("1".into()),
            },
        )
        .unwrap();
        crate::db::repo::artifacts::upsert_replica(
            &conn,
            &crate::db::repo::artifacts::ArtifactReplicaRow {
                artifact_id: built.manifest.id.clone(),
                provider_account_id: "r2-managed".into(),
                provider_kind: "r2".into(),
                encrypted_locator: "{}".into(),
                provider_revision: Some("revision-1".into()),
                etag: None,
                ciphertext_hash: built.manifest.ciphertext_hash.clone(),
                size: built.manifest.size as i64,
                status: "ready".into(),
                last_error_code: None,
                updated_at: "1".into(),
            },
        )
        .unwrap();
        let device_id: String = conn
            .query_row(
                "SELECT device_id FROM sync_runtime_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::db::repo::artifacts::upsert_device_state(
            &conn,
            &crate::db::repo::artifacts::DeviceArtifactStateRow {
                device_id,
                artifact_id: built.manifest.id.clone(),
                observed_version: snapshot.logical_version,
                local_status: "installed".into(),
                verified_hash: Some(built.manifest.ciphertext_hash),
                last_checked_at: "1".into(),
                last_error_code: None,
            },
        )
        .unwrap();
        let document = list_documents(&conn, "collection-a", "agnes")
            .unwrap()
            .into_iter()
            .find(|document| document.id == document_id)
            .unwrap();
        assert_eq!(document.embedded_chunk_count, document.chunk_count);
        assert_eq!(
            document.artifact_id.as_deref(),
            Some(built.manifest.id.as_str())
        );
        assert_eq!(document.artifact_status.as_deref(), Some("installed"));
        assert_eq!(document.ready_replica_count, 1);
        assert_eq!(
            document.device_artifact_status.as_deref(),
            Some("installed")
        );
    }

    #[test]
    fn artifact_snapshot_round_trip_is_transactional_across_fts_and_vectors() {
        unsafe {
            let _ = rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let mut source = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut source).unwrap();
        create_collection(
            &mut source,
            &NewKnowledgeCollection {
                id: "portable-collection".into(),
                name: "Portable".into(),
                scope: "agent_private".into(),
                agent_id: "agnes".into(),
            },
        )
        .unwrap();
        let imported = import_local_document(
            &mut source,
            &NewLocalDocument {
                collection_id: "portable-collection".into(),
                agent_id: "agnes".into(),
                title: "Portable rhubarb".into(),
                media_type: "text/plain".into(),
                local_path: "/tmp/portable.txt".into(),
                plaintext_hash: hash_text("Portable rhubarb knowledge."),
                size: 28,
                parser_profile: DocumentParserProfile::builtin_text(),
                chunks: vec![NewDocumentChunk {
                    content: "Portable rhubarb knowledge.".into(),
                    page: Some(7),
                    section_path: Some("Portable".into()),
                    token_count: 3,
                    metadata: r#"{"kind":"table","sheet":"Portable"}"#.into(),
                }],
            },
        )
        .unwrap();
        let chunk = chunks_needing_embeddings(&source, "test/embed", Some("portable-collection"))
            .unwrap()
            .remove(0);
        assert!(upsert_chunk_embedding(
            &mut source,
            "portable-embedding",
            &chunk.id,
            &chunk.collection_id,
            "test/embed",
            &chunk.content_hash,
            &[1.0, 0.0, 0.0],
        )
        .unwrap());
        let snapshot = export_artifact_snapshot(&source, &imported.document_version_id).unwrap();
        assert_eq!(snapshot.chunks.len(), 1);
        assert_eq!(snapshot.chunks[0].vector, vec![1.0, 0.0, 0.0]);
        assert_eq!(snapshot.chunks[0].page, Some(7));
        assert_eq!(
            snapshot.chunks[0].metadata,
            r#"{"kind":"table","sheet":"Portable"}"#
        );

        let mut target = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut target).unwrap();
        create_collection(
            &mut target,
            &NewKnowledgeCollection {
                id: "portable-collection".into(),
                name: "Portable".into(),
                scope: "agent_private".into(),
                agent_id: "agnes".into(),
            },
        )
        .unwrap();
        assert_eq!(import_artifact_snapshot(&mut target, &snapshot).unwrap(), 1);
        let (page, metadata): (Option<i64>, String) = target
            .query_row(
                "SELECT page, metadata FROM document_chunks WHERE document_version_id = ?1",
                [&snapshot.source_version_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(page, Some(7));
        assert_eq!(metadata, snapshot.chunks[0].metadata);
        assert_eq!(
            search(&target, "agnes", "rhubarb", None, 5).unwrap().len(),
            1
        );
        assert_eq!(
            search_vector(
                &target,
                "portable-collection",
                &KnowledgeQueryEmbedding {
                    model: "test/embed".into(),
                    vector: vec![0.99, 0.01, 0.0],
                },
                5,
            )
            .unwrap()
            .len(),
            1
        );

        let mut newer = snapshot.clone();
        newer.source_version_id = "portable-version-2".into();
        newer.title = "Newer portable".into();
        newer.source_plaintext_hash = hash_text("newer portable source");
        newer.source_size = 21;
        newer.logical_version = 2;
        newer.chunks[0].id = "portable-chunk-2".into();
        newer.chunks[0].content = "Newer turnip knowledge.".into();
        newer.chunks[0].content_hash = hash_text(&newer.chunks[0].content);
        newer.chunks[0].embedding_id = "portable-embedding-2".into();
        newer.chunks[0].vector = vec![0.0, 1.0, 0.0];
        assert_eq!(import_artifact_snapshot(&mut target, &newer).unwrap(), 1);
        assert_eq!(
            target
                .query_row(
                    "SELECT current_version_id FROM documents WHERE id = ?1",
                    [&snapshot.document_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            newer.source_version_id
        );
        assert_eq!(
            search(&target, "agnes", "turnip", None, 5).unwrap().len(),
            1
        );
        assert!(search(&target, "agnes", "rhubarb", None, 5)
            .unwrap()
            .is_empty());

        assert_eq!(import_artifact_snapshot(&mut target, &snapshot).unwrap(), 1);
        assert_eq!(
            target
                .query_row(
                    "SELECT current_version_id FROM documents WHERE id = ?1",
                    [&snapshot.document_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            newer.source_version_id
        );
        assert_eq!(
            search(&target, "agnes", "turnip", None, 5).unwrap().len(),
            1
        );

        let mut conflicting = newer.clone();
        conflicting.source_version_id = "portable-version-conflict".into();
        assert!(import_artifact_snapshot(&mut target, &conflicting).is_err());

        let mut invalid = newer.clone();
        invalid.chunks[0].content = "tampered".into();
        assert!(import_artifact_snapshot(&mut target, &invalid).is_err());
        assert_eq!(
            search(&target, "agnes", "turnip", None, 5).unwrap().len(),
            1
        );
        assert_eq!(
            search_vector(
                &target,
                "portable-collection",
                &KnowledgeQueryEmbedding {
                    model: "test/embed".into(),
                    vector: vec![0.01, 0.99, 0.0],
                },
                5,
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn excludes_reading_collections_from_generic_search() {
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
                id: "reading-collection".into(),
                name: "Read With AI".into(),
                scope: "custom".into(),
                agent_id: "agnes".into(),
            },
        )
        .unwrap();
        let imported = import_local_document(
            &mut conn,
            &NewLocalDocument {
                collection_id: "reading-collection".into(),
                agent_id: "agnes".into(),
                title: "Private book".into(),
                media_type: "application/epub+zip".into(),
                local_path: "/tmp/private-book.epub".into(),
                plaintext_hash: hash_text("Private reading passage about rhubarb."),
                size: 38,
                parser_profile: DocumentParserProfile::builtin_text(),
                chunks: vec![NewDocumentChunk {
                    content: "Private reading passage about rhubarb.".into(),
                    page: None,
                    section_path: Some("Chapter 1".into()),
                    token_count: 5,
                    metadata: "{}".into(),
                }],
            },
        )
        .unwrap();
        crate::db::repo::reading::insert_book(
            &mut conn,
            &crate::db::repo::reading::NewReadingBook {
                id: "reading-book".into(),
                collection_id: "reading-collection".into(),
                document_id: imported.document_id,
                local_path: "/tmp/private-book.epub".into(),
                title: "Private book".into(),
                author: None,
                source_hash: "private-book-hash".into(),
            },
        )
        .unwrap();

        assert!(list_collections(&conn, "agnes").unwrap().is_empty());
        assert!(search(&conn, "agnes", "rhubarb", None, 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            search(&conn, "agnes", "rhubarb", Some("reading-collection"), 10)
                .unwrap()
                .len(),
            1
        );
    }
}
