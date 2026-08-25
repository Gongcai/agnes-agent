use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSourceRow {
    pub id: String,
    pub local_root: String,
    pub account_id: String,
    pub remote_root_id: Option<String>,
    pub mode: String,
    pub lazy_interval_seconds: i64,
    pub max_delay_seconds: i64,
    pub enabled: bool,
    pub last_reconciled_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewBackupSource {
    pub id: String,
    pub local_root: String,
    pub account_id: String,
    pub remote_root_id: Option<String>,
    pub mode: String,
    pub lazy_interval_seconds: i64,
    pub max_delay_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntryRow {
    pub source_id: String,
    pub relative_path: String,
    pub local_size: Option<i64>,
    pub local_mtime_ns: Option<i64>,
    pub content_hash: Option<String>,
    pub remote_item_id: Option<String>,
    pub status: String,
    pub last_error: Option<String>,
    pub dirty_at: Option<String>,
    pub uploaded_at: Option<String>,
    pub updated_at: String,
}

const SOURCE_SELECT: &str = "SELECT id,local_root,account_id,remote_root_id,mode,\
  lazy_interval_seconds,max_delay_seconds,enabled,last_reconciled_at,created_at,updated_at\
  FROM backup_sources";
const ENTRY_SELECT: &str = "SELECT source_id,relative_path,local_size,local_mtime_ns,\
  content_hash,remote_item_id,status,last_error,dirty_at,uploaded_at,updated_at\
  FROM backup_entries";

pub fn list_sources(conn: &Connection) -> AppResult<Vec<BackupSourceRow>> {
    let mut stmt = conn.prepare(&format!(
        "{SOURCE_SELECT} ORDER BY updated_at DESC,local_root"
    ))?;
    let rows = stmt
        .query_map([], source_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn upsert_source(conn: &Connection, input: &NewBackupSource) -> AppResult<()> {
    if input.id.trim().is_empty()
        || input.local_root.trim().is_empty()
        || input.account_id.trim().is_empty()
    {
        return Err(AppError::Other("Backup source identity is required".into()));
    }
    if !matches!(input.mode.as_str(), "fast" | "lazy") {
        return Err(AppError::Other("Backup source mode is invalid".into()));
    }
    if input.lazy_interval_seconds < 60 || input.max_delay_seconds < 60 {
        return Err(AppError::Other(
            "Backup source intervals are too short".into(),
        ));
    }
    let timestamp = now();
    conn.execute(
        "INSERT INTO backup_sources (id,local_root,account_id,remote_root_id,mode,\
         lazy_interval_seconds,max_delay_seconds,enabled,created_at,updated_at)\
         VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8,?8)\
         ON CONFLICT(id) DO UPDATE SET local_root=excluded.local_root,\
         account_id=excluded.account_id,remote_root_id=excluded.remote_root_id,\
         mode=excluded.mode,lazy_interval_seconds=excluded.lazy_interval_seconds,\
         max_delay_seconds=excluded.max_delay_seconds,enabled=1,updated_at=excluded.updated_at",
        params![
            input.id,
            input.local_root.trim(),
            input.account_id,
            input.remote_root_id,
            input.mode,
            input.lazy_interval_seconds,
            input.max_delay_seconds,
            timestamp,
        ],
    )?;
    Ok(())
}

pub fn delete_source(conn: &Connection, source_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM backup_sources WHERE id=?1", [source_id])?;
    Ok(())
}

pub fn update_source_enabled(conn: &Connection, source_id: &str, enabled: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE backup_sources SET enabled=?1,updated_at=?2 WHERE id=?3",
        params![i64::from(enabled), now(), source_id],
    )?;
    Ok(())
}

pub fn mark_reconciled(conn: &Connection, source_id: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE backup_sources SET last_reconciled_at=?1,updated_at=?1 WHERE id=?2",
        params![now(), source_id],
    )?;
    Ok(())
}

pub fn list_entries(conn: &Connection, source_id: &str) -> AppResult<Vec<BackupEntryRow>> {
    let mut stmt = conn.prepare(&format!(
        "{ENTRY_SELECT} WHERE source_id=?1 ORDER BY relative_path"
    ))?;
    let rows = stmt
        .query_map([source_id], entry_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn upsert_entry(conn: &Connection, entry: &BackupEntryRow) -> AppResult<()> {
    if entry.source_id.trim().is_empty() || entry.relative_path.trim().is_empty() {
        return Err(AppError::Other("Backup entry identity is required".into()));
    }
    conn.execute(
        "INSERT INTO backup_entries (source_id,relative_path,local_size,local_mtime_ns,\
         content_hash,remote_item_id,status,last_error,dirty_at,uploaded_at,updated_at)\
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)\
         ON CONFLICT(source_id,relative_path) DO UPDATE SET local_size=excluded.local_size,\
         local_mtime_ns=excluded.local_mtime_ns,content_hash=excluded.content_hash,\
         remote_item_id=COALESCE(excluded.remote_item_id,backup_entries.remote_item_id),\
         status=excluded.status,last_error=excluded.last_error,dirty_at=excluded.dirty_at,\
         uploaded_at=COALESCE(excluded.uploaded_at,backup_entries.uploaded_at),updated_at=excluded.updated_at",
        params![
            entry.source_id,
            entry.relative_path,
            entry.local_size,
            entry.local_mtime_ns,
            entry.content_hash,
            entry.remote_item_id,
            entry.status,
            entry.last_error,
            entry.dirty_at,
            entry.uploaded_at,
            entry.updated_at,
        ],
    )?;
    Ok(())
}

fn source_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackupSourceRow> {
    Ok(BackupSourceRow {
        id: row.get(0)?,
        local_root: row.get(1)?,
        account_id: row.get(2)?,
        remote_root_id: row.get(3)?,
        mode: row.get(4)?,
        lazy_interval_seconds: row.get(5)?,
        max_delay_seconds: row.get(6)?,
        enabled: row.get::<_, i64>(7)? != 0,
        last_reconciled_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BackupEntryRow> {
    Ok(BackupEntryRow {
        source_id: row.get(0)?,
        relative_path: row.get(1)?,
        local_size: row.get(2)?,
        local_mtime_ns: row.get(3)?,
        content_hash: row.get(4)?,
        remote_item_id: row.get(5)?,
        status: row.get(6)?,
        last_error: row.get(7)?,
        dirty_at: row.get(8)?,
        uploaded_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}
