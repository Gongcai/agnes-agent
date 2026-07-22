//! Local Read With AI domain: user-level books and highlights, with one
//! discussion session per book and agent.

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::sync::payload::SyncEntityType;

#[derive(Debug, Clone, Serialize)]
pub struct ReadingBookRow {
    pub id: String,
    pub collection_id: Option<String>,
    pub document_id: Option<String>,
    pub local_path: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub source_hash: String,
    pub model_knows_content: bool,
    pub content_context_allowed: bool,
    pub content_context_decided: bool,
    pub progress_cfi: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
    pub artifact_id: Option<String>,
    pub artifact_status: Option<String>,
    pub ready_replica_count: i64,
    pub local_artifact_status: Option<String>,
    pub local_artifact_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadingHighlightRow {
    pub id: String,
    pub book_id: String,
    pub cfi_range: String,
    pub quote: String,
    pub context_before: String,
    pub context_after: String,
    pub note: Option<String>,
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
    pub version: i64,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadingConversationRow {
    pub session_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub is_current: bool,
}

#[derive(Debug, Clone)]
pub struct NewReadingBook {
    pub id: String,
    pub collection_id: String,
    pub document_id: String,
    pub local_path: String,
    pub title: String,
    pub author: Option<String>,
    pub source_hash: String,
}

#[derive(Debug, Clone)]
pub struct NewReadingHighlight {
    pub id: String,
    pub book_id: String,
    pub cfi_range: String,
    pub quote: String,
    pub context_before: String,
    pub context_after: String,
    pub note: Option<String>,
    pub color: String,
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn reading_book_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReadingBookRow> {
    Ok(ReadingBookRow {
        id: row.get(0)?,
        collection_id: row.get(1)?,
        document_id: row.get(2)?,
        local_path: row.get(3)?,
        title: row.get(4)?,
        author: row.get(5)?,
        source_hash: row.get(6)?,
        model_knows_content: row.get::<_, i64>(7)? != 0,
        content_context_allowed: row.get::<_, i64>(8)? != 0,
        content_context_decided: row.get::<_, i64>(9)? != 0,
        progress_cfi: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        version: row.get(13)?,
        deleted_at: row.get(14)?,
        origin_device_id: row.get(15)?,
        artifact_id: None,
        artifact_status: None,
        ready_replica_count: 0,
        local_artifact_status: None,
        local_artifact_error: None,
    })
}

const BOOK_COLUMNS: &str =
    "id, collection_id, document_id, local_path, title, author, source_hash, \
    model_knows_content, content_context_allowed, content_context_decided, progress_cfi, created_at, updated_at, \
    version, deleted_at, origin_device_id";

const BOOK_COLUMNS_QUALIFIED: &str =
    "b.id, b.collection_id, b.document_id, b.local_path, b.title, b.author, b.source_hash, \
    b.model_knows_content, b.content_context_allowed, b.content_context_decided, b.progress_cfi, b.created_at, b.updated_at, \
    b.version, b.deleted_at, b.origin_device_id";

pub fn insert_book(conn: &mut Connection, input: &NewReadingBook) -> AppResult<ReadingBookRow> {
    if input.title.trim().is_empty() || input.source_hash.trim().is_empty() {
        return Err(AppError::Other(
            "Reading book title and source hash are required".into(),
        ));
    }
    let timestamp = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let document_matches_collection: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ?1 AND collection_id = ?2 AND deleted_at IS NULL)",
        params![input.document_id, input.collection_id],
        |row| row.get(0),
    )?;
    if !document_matches_collection {
        return Err(AppError::Other(
            "Reading book document is not part of its collection".into(),
        ));
    }
    tx.execute(
        "INSERT INTO reading_books \
         (id, collection_id, document_id, local_path, title, author, source_hash, created_at, updated_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
        params![
            input.id,
            input.collection_id,
            input.document_id,
            input.local_path,
            input.title.trim(),
            input.author.as_deref().map(str::trim).filter(|value| !value.is_empty()),
            input.source_hash,
            timestamp,
            device_id,
        ],
    )?;
    let row = get_in_transaction(&tx, &input.id)?
        .ok_or_else(|| AppError::Other("Reading book disappeared during creation".into()))?;
    enqueue_book(&tx, &row)?;
    tx.commit()?;
    Ok(row)
}

pub fn find_by_source_hash(
    conn: &Connection,
    source_hash: &str,
) -> AppResult<Option<ReadingBookRow>> {
    conn.query_row(
        &format!("SELECT {BOOK_COLUMNS} FROM reading_books WHERE source_hash = ?1 AND deleted_at IS NULL"),
        [source_hash],
        reading_book_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_book(conn: &Connection, book_id: &str) -> AppResult<Option<ReadingBookRow>> {
    conn.query_row(
        &format!("SELECT {BOOK_COLUMNS} FROM reading_books WHERE id = ?1 AND deleted_at IS NULL"),
        [book_id],
        reading_book_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn bind_local_epub(
    conn: &Connection,
    book_id: &str,
    source_hash: &str,
    local_path: &str,
) -> AppResult<()> {
    if !std::path::Path::new(local_path).is_absolute() || local_path.len() > 4_096 {
        return Err(AppError::Other("Invalid local EPUB binding".into()));
    }
    let changed = conn.execute(
        "UPDATE reading_books SET local_path = ?1 WHERE id = ?2 AND source_hash = ?3 AND deleted_at IS NULL",
        params![local_path, book_id, source_hash],
    )?;
    if changed != 1 {
        return Err(AppError::Other(
            "Reading book metadata is unavailable or does not match the EPUB".into(),
        ));
    }
    Ok(())
}

pub fn bind_local_index(
    conn: &Connection,
    book_id: &str,
    collection_id: &str,
    document_id: &str,
) -> AppResult<()> {
    let matches: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM documents WHERE id=?1 AND collection_id=?2 AND deleted_at IS NULL)",
        params![document_id, collection_id],
        |row| row.get(0),
    )?;
    if !matches {
        return Err(AppError::Other(
            "Reading index document does not match its collection".into(),
        ));
    }
    let changed = conn.execute(
        "UPDATE reading_books SET collection_id=?1,document_id=?2 WHERE id=?3 AND deleted_at IS NULL",
        params![collection_id, document_id, book_id],
    )?;
    if changed != 1 {
        return Err(AppError::Other("Reading book not found".into()));
    }
    Ok(())
}

pub fn list_books(conn: &Connection) -> AppResult<Vec<ReadingBookRow>> {
    let mut statement = conn.prepare(&format!(
        "SELECT reading_books.id, reading_books.collection_id, reading_books.document_id, reading_books.local_path, \
                reading_books.title, reading_books.author, reading_books.source_hash, reading_books.model_knows_content, \
                reading_books.content_context_allowed, reading_books.content_context_decided, reading_books.progress_cfi, \
                reading_books.created_at, reading_books.updated_at, reading_books.version, reading_books.deleted_at, \
                reading_books.origin_device_id, am.id, am.local_status, \
                COALESCE((SELECT COUNT(*) FROM artifact_replicas r WHERE r.artifact_id = am.id AND r.status = 'ready'), 0), \
                das.local_status, das.last_error_code \
         FROM reading_books \
         LEFT JOIN artifact_manifests am ON am.id = ( \
           SELECT candidate.id FROM artifact_manifests candidate \
           WHERE candidate.artifact_type = 'reading_epub' AND candidate.source_version_id = reading_books.id \
           ORDER BY CASE candidate.local_status WHEN 'installed' THEN 3 WHEN 'available' THEN 2 WHEN 'built' THEN 1 ELSE 0 END DESC, \
                    CAST(candidate.created_at AS INTEGER) DESC, candidate.id DESC LIMIT 1 \
         ) \
         LEFT JOIN device_artifact_states das ON das.artifact_id = am.id \
           AND das.device_id = (SELECT device_id FROM sync_runtime_state WHERE singleton = 1) \
         WHERE reading_books.deleted_at IS NULL \
         ORDER BY CAST(updated_at AS INTEGER) DESC, title COLLATE NOCASE"
    ))?;
    let rows = statement
        .query_map([], |row| {
            let mut book = reading_book_from_row(row)?;
            book.artifact_id = row.get(16)?;
            book.artifact_status = row.get(17)?;
            book.ready_replica_count = row.get(18)?;
            book.local_artifact_status = row.get(19)?;
            book.local_artifact_error = row.get(20)?;
            Ok(book)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into);
    rows
}

pub fn get_book_for_session(
    conn: &Connection,
    session_id: &str,
) -> AppResult<Option<ReadingBookRow>> {
    conn.query_row(
        &format!(
            "SELECT {BOOK_COLUMNS_QUALIFIED} FROM reading_books b \
             JOIN reading_book_conversation_sessions h ON h.book_id = b.id \
             JOIN sessions s ON s.id = h.session_id AND s.deleted_at IS NULL \
             WHERE h.session_id = ?1 AND b.deleted_at IS NULL"
        ),
        [session_id],
        reading_book_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_conversation_session(
    conn: &Connection,
    book_id: &str,
    agent_id: &str,
) -> AppResult<Option<String>> {
    conn.query_row(
        "SELECT c.session_id FROM reading_book_conversations c \
         JOIN sessions s ON s.id = c.session_id AND s.deleted_at IS NULL \
         WHERE c.book_id = ?1 AND c.agent_id = ?2",
        params![book_id, agent_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_conversations(
    conn: &Connection,
    book_id: &str,
    agent_id: &str,
) -> AppResult<Vec<ReadingConversationRow>> {
    let mut statement = conn.prepare(
        "SELECT h.session_id, s.title, COALESCE(s.created_at, '0'), \
                COALESCE(s.updated_at, s.created_at, '0'), \
                CASE WHEN current.session_id = h.session_id THEN 1 ELSE 0 END \
         FROM reading_book_conversation_sessions h \
         JOIN sessions s ON s.id = h.session_id AND s.deleted_at IS NULL \
         LEFT JOIN reading_book_conversations current \
           ON current.book_id = h.book_id AND current.agent_id = h.agent_id \
         WHERE h.book_id = ?1 AND h.agent_id = ?2 \
         ORDER BY 5 DESC, CAST(s.created_at AS INTEGER) DESC, s.id DESC",
    )?;
    let rows = statement.query_map(params![book_id, agent_id], |row| {
        Ok(ReadingConversationRow {
            session_id: row.get(0)?,
            title: row.get(1)?,
            created_at: row.get(2)?,
            updated_at: row.get(3)?,
            is_current: row.get::<_, i64>(4)? != 0,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn select_conversation(
    conn: &mut Connection,
    book_id: &str,
    agent_id: &str,
    session_id: &str,
) -> AppResult<()> {
    let timestamp = now();
    let tx = conn.transaction()?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM reading_book_conversation_sessions h \
         JOIN sessions s ON s.id = h.session_id AND s.deleted_at IS NULL \
         WHERE h.book_id = ?1 AND h.agent_id = ?2 AND h.session_id = ?3)",
        params![book_id, agent_id, session_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(AppError::Other("Reading conversation not found".into()));
    }
    tx.execute(
        "INSERT INTO reading_book_conversations \
         (book_id, agent_id, session_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4) \
         ON CONFLICT(book_id, agent_id) DO UPDATE SET \
           session_id = excluded.session_id, updated_at = excluded.updated_at",
        params![book_id, agent_id, session_id, timestamp],
    )?;
    tx.execute(
        "UPDATE reading_book_conversation_sessions SET updated_at = ?1 \
         WHERE book_id = ?2 AND agent_id = ?3 AND session_id = ?4",
        params![timestamp, book_id, agent_id, session_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn grant_book_agent_access(
    conn: &mut Connection,
    book_id: &str,
    agent_id: &str,
) -> AppResult<()> {
    let timestamp = now();
    let tx = conn.transaction()?;
    let collection_id: Option<String> = tx
        .query_row(
            "SELECT collection_id FROM reading_books WHERE id = ?1 AND deleted_at IS NULL",
            [book_id],
            |row| row.get(0),
        )
        .optional()?;
    let collection_id =
        collection_id.ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    let agent_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1 AND deleted_at IS NULL)",
        [agent_id],
        |row| row.get(0),
    )?;
    if !agent_exists {
        return Err(AppError::Other("Agent not found".into()));
    }
    tx.execute(
        "INSERT INTO collection_agents (collection_id, agent_id, permission, created_at, updated_at) \
         VALUES (?1, ?2, 'read', ?3, ?3) \
         ON CONFLICT(collection_id, agent_id) DO UPDATE SET updated_at = excluded.updated_at",
        params![collection_id, agent_id, timestamp],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn create_conversation(
    conn: &mut Connection,
    book_id: &str,
    agent_id: &str,
    session_id: &str,
) -> AppResult<()> {
    let timestamp = now();
    let tx = conn.transaction()?;
    let book_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM reading_books WHERE id = ?1 AND deleted_at IS NULL)",
        [book_id],
        |row| row.get(0),
    )?;
    let session_matches_agent: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1 AND agent_id = ?2 AND deleted_at IS NULL)",
        params![session_id, agent_id],
        |row| row.get(0),
    )?;
    if !book_exists || !session_matches_agent {
        return Err(AppError::Other(
            "Invalid reading conversation association".into(),
        ));
    }
    tx.execute(
        "INSERT INTO reading_book_conversation_sessions \
         (book_id, agent_id, session_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4) \
         ON CONFLICT(book_id, agent_id, session_id) DO UPDATE SET updated_at = excluded.updated_at",
        params![book_id, agent_id, session_id, timestamp],
    )?;
    tx.execute(
        "INSERT INTO reading_book_conversations (book_id, agent_id, session_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4) \
         ON CONFLICT(book_id, agent_id) DO UPDATE SET session_id = excluded.session_id, updated_at = excluded.updated_at",
        params![book_id, agent_id, session_id, timestamp],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn update_book_mode(
    conn: &mut Connection,
    book_id: &str,
    model_knows_content: bool,
) -> AppResult<ReadingBookRow> {
    let timestamp = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE reading_books SET model_knows_content = ?1, updated_at = ?2, version = version + 1, origin_device_id = ?3 \
         WHERE id = ?4 AND deleted_at IS NULL",
        params![i64::from(model_knows_content), timestamp, device_id, book_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading book not found".into()));
    }
    let row = get_in_transaction(&tx, book_id)?
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    enqueue_book(&tx, &row)?;
    tx.commit()?;
    Ok(row)
}

pub fn set_content_context_allowed(
    conn: &mut Connection,
    book_id: &str,
    allowed: bool,
) -> AppResult<ReadingBookRow> {
    let timestamp = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE reading_books SET content_context_allowed = ?1, content_context_decided = 1, updated_at = ?2, \
         version = version + 1, origin_device_id = ?3 WHERE id = ?4 AND deleted_at IS NULL",
        params![i64::from(allowed), timestamp, device_id, book_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading book not found".into()));
    }
    let row = get_in_transaction(&tx, book_id)?
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    enqueue_book(&tx, &row)?;
    tx.commit()?;
    Ok(row)
}

pub fn update_progress(conn: &mut Connection, book_id: &str, cfi: &str) -> AppResult<()> {
    let cfi = cfi.trim();
    if cfi.is_empty() || cfi.len() > 4_096 {
        return Err(AppError::Other("Invalid EPUB reading position".into()));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE reading_books SET progress_cfi = ?1, updated_at = ?2, version = version + 1, origin_device_id = ?3 \
         WHERE id = ?4 AND deleted_at IS NULL AND progress_cfi IS NOT ?1",
        params![cfi, now(), device_id, book_id],
    )?;
    if changed == 0 {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM reading_books WHERE id = ?1 AND deleted_at IS NULL)",
            [book_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(AppError::Other("Reading book not found".into()));
        }
        tx.commit()?;
        return Ok(());
    }
    let row = get_in_transaction(&tx, book_id)?
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    enqueue_book(&tx, &row)?;
    tx.commit()?;
    Ok(())
}

pub fn list_highlights(conn: &Connection, book_id: &str) -> AppResult<Vec<ReadingHighlightRow>> {
    let mut statement = conn.prepare(
        "SELECT id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at, \
                version, deleted_at, origin_device_id \
         FROM reading_highlights WHERE book_id = ?1 AND deleted_at IS NULL ORDER BY created_at ASC, id ASC",
    )?;
    let rows = statement
        .query_map([book_id], reading_highlight_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into);
    rows
}

pub fn insert_highlight(
    conn: &mut Connection,
    input: &NewReadingHighlight,
) -> AppResult<ReadingHighlightRow> {
    let quote = input.quote.trim();
    let cfi_range = input.cfi_range.trim();
    if quote.is_empty() || quote.len() > 20_000 || cfi_range.is_empty() || cfi_range.len() > 8_192 {
        return Err(AppError::Other("Invalid reading highlight".into()));
    }
    let timestamp = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let book_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM reading_books WHERE id = ?1 AND deleted_at IS NULL)",
        [&input.book_id],
        |row| row.get(0),
    )?;
    if !book_exists {
        return Err(AppError::Other("Reading book not found".into()));
    }
    tx.execute(
        "INSERT INTO reading_highlights \
         (id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)",
        params![
            input.id,
            input.book_id,
            cfi_range,
            quote,
            truncate_context(&input.context_before),
            truncate_context(&input.context_after),
            input.note.as_deref().map(str::trim).filter(|value| !value.is_empty()),
            normalize_color(&input.color),
            timestamp,
            device_id,
        ],
    )?;
    let row = tx.query_row(
        "SELECT id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at, \
                version, deleted_at, origin_device_id \
         FROM reading_highlights WHERE id = ?1",
        [&input.id],
        reading_highlight_from_row,
    )?;
    enqueue_highlight(&tx, &row)?;
    tx.commit()?;
    Ok(row)
}

pub fn update_highlight(
    conn: &mut Connection,
    highlight_id: &str,
    note: Option<&str>,
    color: &str,
) -> AppResult<ReadingHighlightRow> {
    let note = note.map(str::trim).filter(|value| !value.is_empty());
    if note.is_some_and(|value| value.len() > 20_000) {
        return Err(AppError::Other("Reading highlight note is too long".into()));
    }
    let timestamp = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE reading_highlights SET note = ?1, color = ?2, updated_at = ?3, \
         version = version + 1, origin_device_id = ?4 \
         WHERE id = ?5 AND deleted_at IS NULL",
        params![
            note,
            normalize_color(color),
            timestamp,
            device_id,
            highlight_id
        ],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading highlight not found".into()));
    }
    let row = get_highlight_in_transaction(&tx, highlight_id)?
        .ok_or_else(|| AppError::Other("Reading highlight not found".into()))?;
    enqueue_highlight(&tx, &row)?;
    tx.commit()?;
    Ok(row)
}

pub fn delete_highlight(conn: &mut Connection, highlight_id: &str) -> AppResult<()> {
    let timestamp = now();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&tx)?;
    let changed = tx.execute(
        "UPDATE reading_highlights SET deleted_at = ?1, updated_at = ?1, \
         version = version + 1, origin_device_id = ?2 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![timestamp, device_id, highlight_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading highlight not found".into()));
    }
    let row = get_highlight_in_transaction(&tx, highlight_id)?
        .ok_or_else(|| AppError::Other("Reading highlight not found".into()))?;
    enqueue_highlight(&tx, &row)?;
    tx.commit()?;
    Ok(())
}

fn enqueue_book(conn: &Connection, row: &ReadingBookRow) -> AppResult<()> {
    conn.execute(
        "DELETE FROM sync_outbox WHERE entity_type = 'reading_book' AND entity_id = ?1 \
         AND status = 'pending' AND payload_encoding = 'json'",
        [&row.id],
    )?;
    super::sync::enqueue_projection(
        conn,
        SyncEntityType::ReadingBook,
        &row.id,
        row.version,
        row.deleted_at.is_some(),
        &serde_json::to_value(row)?,
    )?;
    Ok(())
}

fn enqueue_highlight(conn: &Connection, row: &ReadingHighlightRow) -> AppResult<()> {
    conn.execute(
        "DELETE FROM sync_outbox WHERE entity_type = 'reading_highlight' AND entity_id = ?1 \
         AND status = 'pending' AND payload_encoding = 'json'",
        [&row.id],
    )?;
    super::sync::enqueue_projection(
        conn,
        SyncEntityType::ReadingHighlight,
        &row.id,
        row.version,
        row.deleted_at.is_some(),
        &serde_json::to_value(row)?,
    )?;
    Ok(())
}

pub(crate) fn enqueue_existing_sync_entities(conn: &Connection) -> AppResult<()> {
    let book_ids = {
        let mut statement = conn.prepare(
            "SELECT id FROM reading_books b WHERE NOT EXISTS (
               SELECT 1 FROM sync_entity_state s WHERE s.entity_type='reading_book' AND s.entity_id=b.id
             ) AND NOT EXISTS (
               SELECT 1 FROM sync_outbox o WHERE o.entity_type='reading_book' AND o.entity_id=b.id
                 AND o.status IN ('pending','in_flight','conflict','dead_letter')
             ) ORDER BY id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };
    for id in book_ids {
        let row = conn.query_row(
            &format!("SELECT {BOOK_COLUMNS} FROM reading_books WHERE id=?1"),
            [&id],
            reading_book_from_row,
        )?;
        enqueue_book(conn, &row)?;
    }

    let highlight_ids = {
        let mut statement = conn.prepare(
            "SELECT id FROM reading_highlights h WHERE NOT EXISTS (
               SELECT 1 FROM sync_entity_state s WHERE s.entity_type='reading_highlight' AND s.entity_id=h.id
             ) AND NOT EXISTS (
               SELECT 1 FROM sync_outbox o WHERE o.entity_type='reading_highlight' AND o.entity_id=h.id
                 AND o.status IN ('pending','in_flight','conflict','dead_letter')
             ) ORDER BY id",
        )?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids
    };
    for id in highlight_ids {
        let row = conn.query_row(
            "SELECT id,book_id,cfi_range,quote,context_before,context_after,note,color,created_at,
                    updated_at,version,deleted_at,origin_device_id FROM reading_highlights WHERE id=?1",
            [&id],
            reading_highlight_from_row,
        )?;
        enqueue_highlight(conn, &row)?;
    }
    Ok(())
}

fn reading_highlight_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReadingHighlightRow> {
    Ok(ReadingHighlightRow {
        id: row.get(0)?,
        book_id: row.get(1)?,
        cfi_range: row.get(2)?,
        quote: row.get(3)?,
        context_before: row.get(4)?,
        context_after: row.get(5)?,
        note: row.get(6)?,
        color: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        version: row.get(10)?,
        deleted_at: row.get(11)?,
        origin_device_id: row.get(12)?,
    })
}

fn get_highlight_in_transaction(
    tx: &Transaction<'_>,
    highlight_id: &str,
) -> AppResult<Option<ReadingHighlightRow>> {
    tx.query_row(
        "SELECT id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at, \
                version, deleted_at, origin_device_id FROM reading_highlights WHERE id = ?1",
        [highlight_id],
        reading_highlight_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn get_in_transaction(tx: &Transaction<'_>, book_id: &str) -> AppResult<Option<ReadingBookRow>> {
    tx.query_row(
        &format!("SELECT {BOOK_COLUMNS} FROM reading_books WHERE id = ?1 AND deleted_at IS NULL"),
        [book_id],
        reading_book_from_row,
    )
    .optional()
    .map_err(Into::into)
}

fn truncate_context(value: &str) -> String {
    value.trim().chars().take(8_000).collect()
}

fn normalize_color(value: &str) -> &str {
    match value {
        "green" | "blue" | "pink" => value,
        _ => "yellow",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(conn: &Connection) {
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute_batch(crate::db::schema::KNOWLEDGE_SCHEMA)
            .unwrap();
        conn.execute_batch(crate::db::schema::READING_SCHEMA)
            .unwrap();
        conn.execute_batch(crate::db::schema::ARTIFACT_SCHEMA)
            .unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) VALUES (1, ?1)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agents (id, name) VALUES ('agent', 'Agent')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO knowledge_collections (id, name, scope, created_at, updated_at) VALUES ('collection', 'Reading', 'custom', '1', '1')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO collection_agents (collection_id, agent_id, permission, created_at, updated_at) VALUES ('collection', 'agent', 'manage', '1', '1')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO documents (id, collection_id, title, media_type, created_at, updated_at) VALUES ('document', 'collection', 'Book', 'application/epub+zip', '1', '1')",
            [],
        ).unwrap();
    }

    #[test]
    fn creates_books_and_persists_highlights() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        let book = insert_book(
            &mut conn,
            &NewReadingBook {
                id: "book".into(),
                collection_id: "collection".into(),
                document_id: "document".into(),
                local_path: "/tmp/book.epub".into(),
                title: "Book".into(),
                author: None,
                source_hash: "hash".into(),
            },
        )
        .unwrap();
        assert_eq!(book.title, "Book");
        let highlight = insert_highlight(
            &mut conn,
            &NewReadingHighlight {
                id: "highlight".into(),
                book_id: "book".into(),
                cfi_range: "epubcfi(/6/2!/4/2/1:0)".into(),
                quote: "A selected passage".into(),
                context_before: "Before".into(),
                context_after: "After".into(),
                note: None,
                color: "yellow".into(),
            },
        )
        .unwrap();
        assert_eq!(highlight.quote, "A selected passage");
        assert_eq!(list_highlights(&conn, "book").unwrap().len(), 1);

        let updated = update_highlight(&mut conn, "highlight", Some("Important"), "blue").unwrap();
        assert_eq!(updated.note.as_deref(), Some("Important"));
        assert_eq!(updated.color, "blue");
        assert_eq!(updated.version, highlight.version + 1);

        delete_highlight(&mut conn, "highlight").unwrap();
        assert!(list_highlights(&conn, "book").unwrap().is_empty());
        let deleted_at: Option<String> = conn
            .query_row(
                "SELECT deleted_at FROM reading_highlights WHERE id = 'highlight'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(deleted_at.is_some());
    }

    #[test]
    fn new_conversation_replaces_current_link_without_deleting_old_session() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        insert_book(
            &mut conn,
            &NewReadingBook {
                id: "book".into(),
                collection_id: "collection".into(),
                document_id: "document".into(),
                local_path: "/tmp/book.epub".into(),
                title: "Book".into(),
                author: None,
                source_hash: "hash".into(),
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, title) VALUES ('session-1', 'agent', 'First')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, title) VALUES ('session-2', 'agent', 'Second')",
            [],
        )
        .unwrap();

        create_conversation(&mut conn, "book", "agent", "session-1").unwrap();
        create_conversation(&mut conn, "book", "agent", "session-2").unwrap();

        assert_eq!(
            get_conversation_session(&conn, "book", "agent")
                .unwrap()
                .as_deref(),
            Some("session-2")
        );
        let old_session_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = 'session-1')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(old_session_exists);
        let conversations = list_conversations(&conn, "book", "agent").unwrap();
        assert_eq!(conversations.len(), 2);
        assert_eq!(conversations[0].session_id, "session-2");
        assert!(conversations[0].is_current);
        assert_eq!(
            get_book_for_session(&conn, "session-1")
                .unwrap()
                .unwrap()
                .id,
            "book"
        );

        select_conversation(&mut conn, "book", "agent", "session-1").unwrap();
        assert_eq!(
            get_conversation_session(&conn, "book", "agent")
                .unwrap()
                .as_deref(),
            Some("session-1")
        );
    }

    #[test]
    fn gets_book_for_session_with_qualified_columns() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed(&conn);
        insert_book(
            &mut conn,
            &NewReadingBook {
                id: "book".into(),
                collection_id: "collection".into(),
                document_id: "document".into(),
                local_path: "/tmp/book.epub".into(),
                title: "Book".into(),
                author: Some("Author".into()),
                source_hash: "hash".into(),
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, title) VALUES ('session', 'agent', 'Reading')",
            [],
        )
        .unwrap();
        create_conversation(&mut conn, "book", "agent", "session").unwrap();

        let book = get_book_for_session(&conn, "session").unwrap().unwrap();
        assert_eq!(book.id, "book");
        assert_eq!(book.title, "Book");
    }
}
