//! Local Read With AI domain: user-level books and highlights, with one
//! discussion session per book and agent.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
pub struct ReadingBookRow {
    pub id: String,
    pub collection_id: String,
    pub document_id: String,
    pub local_path: String,
    pub title: String,
    pub author: Option<String>,
    pub source_hash: String,
    pub model_knows_content: bool,
    pub content_context_allowed: bool,
    pub content_context_decided: bool,
    pub progress_cfi: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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
    })
}

const BOOK_COLUMNS: &str =
    "id, collection_id, document_id, local_path, title, author, source_hash, \
    model_knows_content, content_context_allowed, content_context_decided, progress_cfi, created_at, updated_at";

pub fn insert_book(conn: &mut Connection, input: &NewReadingBook) -> AppResult<ReadingBookRow> {
    if input.title.trim().is_empty() || input.source_hash.trim().is_empty() {
        return Err(AppError::Other(
            "Reading book title and source hash are required".into(),
        ));
    }
    let timestamp = now();
    let tx = conn.transaction()?;
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
         (id, collection_id, document_id, local_path, title, author, source_hash, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            input.id,
            input.collection_id,
            input.document_id,
            input.local_path,
            input.title.trim(),
            input.author.as_deref().map(str::trim).filter(|value| !value.is_empty()),
            input.source_hash,
            timestamp,
        ],
    )?;
    let row = get_in_transaction(&tx, &input.id)?
        .ok_or_else(|| AppError::Other("Reading book disappeared during creation".into()))?;
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

pub fn list_books(conn: &Connection) -> AppResult<Vec<ReadingBookRow>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {BOOK_COLUMNS} FROM reading_books WHERE deleted_at IS NULL \
         ORDER BY CAST(updated_at AS INTEGER) DESC, title COLLATE NOCASE"
    ))?;
    let rows = statement
        .query_map([], reading_book_from_row)?
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
            "SELECT b.{BOOK_COLUMNS} FROM reading_books b \
             JOIN reading_book_conversations c ON c.book_id = b.id \
             JOIN sessions s ON s.id = c.session_id AND s.deleted_at IS NULL \
             WHERE c.session_id = ?1 AND b.deleted_at IS NULL"
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
        "INSERT INTO reading_book_conversations (book_id, agent_id, session_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4)",
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
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE reading_books SET model_knows_content = ?1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![i64::from(model_knows_content), timestamp, book_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading book not found".into()));
    }
    let row = get_in_transaction(&tx, book_id)?
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    tx.commit()?;
    Ok(row)
}

pub fn set_content_context_allowed(
    conn: &mut Connection,
    book_id: &str,
    allowed: bool,
) -> AppResult<ReadingBookRow> {
    let timestamp = now();
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE reading_books SET content_context_allowed = ?1, content_context_decided = 1, updated_at = ?2, version = version + 1 \
         WHERE id = ?3 AND deleted_at IS NULL",
        params![i64::from(allowed), timestamp, book_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading book not found".into()));
    }
    let row = get_in_transaction(&tx, book_id)?
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    tx.commit()?;
    Ok(row)
}

pub fn update_progress(conn: &mut Connection, book_id: &str, cfi: &str) -> AppResult<()> {
    let cfi = cfi.trim();
    if cfi.is_empty() || cfi.len() > 4_096 {
        return Err(AppError::Other("Invalid EPUB reading position".into()));
    }
    let changed = conn.execute(
        "UPDATE reading_books SET progress_cfi = ?1, updated_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
        params![cfi, now(), book_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Reading book not found".into()));
    }
    Ok(())
}

pub fn list_highlights(conn: &Connection, book_id: &str) -> AppResult<Vec<ReadingHighlightRow>> {
    let mut statement = conn.prepare(
        "SELECT id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at \
         FROM reading_highlights WHERE book_id = ?1 AND deleted_at IS NULL ORDER BY created_at ASC, id ASC",
    )?;
    let rows = statement
        .query_map([book_id], |row| {
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
            })
        })?
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
    let tx = conn.transaction()?;
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
         (id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
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
        ],
    )?;
    let row = tx.query_row(
        "SELECT id, book_id, cfi_range, quote, context_before, context_after, note, color, created_at, updated_at \
         FROM reading_highlights WHERE id = ?1",
        [&input.id],
        |row| {
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
            })
        },
    )?;
    tx.commit()?;
    Ok(row)
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
    }
}
