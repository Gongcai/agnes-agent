use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageAccountRow {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub account_subject: Option<String>,
    pub config_json: String,
    pub auth_state: String,
    pub enabled: bool,
    pub capabilities_json: String,
    pub quota_used_bytes: Option<i64>,
    pub quota_total_bytes: Option<i64>,
    pub last_error_category: Option<String>,
    pub last_error_message: Option<String>,
    pub last_checked_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct UpsertStorageAccount {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub account_subject: Option<String>,
    pub config_json: String,
    pub auth_state: String,
    pub enabled: bool,
    pub capabilities_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTransferJobRow {
    pub id: String,
    pub account_id: String,
    pub operation: String,
    pub remote_item_id: Option<String>,
    pub display_name: String,
    pub destination_kind: Option<String>,
    pub destination_id: Option<String>,
    pub status: String,
    pub bytes_transferred: i64,
    pub bytes_total: Option<i64>,
    pub error_category: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewStorageTransferJob {
    pub id: String,
    pub account_id: String,
    pub operation: String,
    pub remote_item_id: Option<String>,
    pub display_name: String,
    pub destination_kind: Option<String>,
    pub destination_id: Option<String>,
    pub bytes_total: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct StorageTransferProgress {
    pub status: String,
    pub bytes_transferred: i64,
    pub bytes_total: Option<i64>,
    pub error_category: Option<String>,
    pub error_message: Option<String>,
}

const ACCOUNT_SELECT: &str = "SELECT a.id,a.provider_id,a.display_name,a.account_subject,\
  a.config_json,COALESCE(b.auth_state,'disconnected'),COALESCE(b.enabled,0),\
  COALESCE(b.capabilities_json,'{}'),b.quota_used_bytes,b.quota_total_bytes,\
  b.last_error_category,b.last_error_message,b.last_checked_at,a.created_at,a.updated_at \
  FROM storage_provider_accounts a \
  LEFT JOIN storage_provider_bindings b ON b.account_id=a.id";

const TRANSFER_SELECT: &str = "SELECT id,account_id,operation,remote_item_id,display_name,\
  destination_kind,destination_id,status,bytes_transferred,bytes_total,error_category,\
  error_message,created_at,updated_at,completed_at FROM storage_transfer_jobs";

pub fn list_accounts(conn: &Connection) -> AppResult<Vec<StorageAccountRow>> {
    let mut statement = conn.prepare(&format!(
        "{ACCOUNT_SELECT} WHERE a.deleted_at IS NULL ORDER BY a.updated_at DESC,a.id"
    ))?;
    let rows = statement.query_map([], account_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_account(conn: &Connection, account_id: &str) -> AppResult<Option<StorageAccountRow>> {
    Ok(conn
        .query_row(
            &format!("{ACCOUNT_SELECT} WHERE a.id=?1 AND a.deleted_at IS NULL"),
            [account_id],
            account_from_row,
        )
        .optional()?)
}

pub fn upsert_account(conn: &mut Connection, input: &UpsertStorageAccount) -> AppResult<()> {
    validate_account(input)?;
    let timestamp = now();
    let transaction = conn.transaction()?;
    transaction.execute(
        "INSERT INTO storage_provider_accounts \
         (id,provider_id,display_name,account_subject,config_json,created_at,updated_at,version) \
         VALUES (?1,?2,?3,?4,?5,?6,?6,1) \
         ON CONFLICT(id) DO UPDATE SET \
           provider_id=excluded.provider_id,display_name=excluded.display_name,\
           account_subject=excluded.account_subject,config_json=excluded.config_json,\
           updated_at=excluded.updated_at,version=storage_provider_accounts.version+1,deleted_at=NULL",
        params![
            input.id,
            input.provider_id,
            input.display_name.trim(),
            trim_optional(input.account_subject.as_deref()),
            input.config_json,
            timestamp,
        ],
    )?;
    transaction.execute(
        "INSERT INTO storage_provider_bindings \
         (account_id,auth_state,enabled,capabilities_json,created_at,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?5) \
         ON CONFLICT(account_id) DO UPDATE SET \
           auth_state=excluded.auth_state,enabled=excluded.enabled,\
           capabilities_json=excluded.capabilities_json,last_error_category=NULL,\
           last_error_message=NULL,updated_at=excluded.updated_at",
        params![
            input.id,
            input.auth_state,
            i64::from(input.enabled),
            input.capabilities_json,
            timestamp,
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn update_binding_status(
    conn: &Connection,
    account_id: &str,
    auth_state: &str,
    enabled: bool,
    capabilities_json: &str,
    quota_used_bytes: Option<i64>,
    quota_total_bytes: Option<i64>,
    error: Option<(&str, &str)>,
) -> AppResult<()> {
    validate_auth_state(auth_state)?;
    validate_json_object(capabilities_json, "capabilities")?;
    validate_bytes(quota_used_bytes, "used quota")?;
    validate_bytes(quota_total_bytes, "total quota")?;
    if let Some((category, message)) = error {
        if category.len() > 64 || message.chars().count() > 2048 {
            return Err(AppError::Other(
                "Storage provider error is too large".into(),
            ));
        }
    }
    let timestamp = now();
    let changed = conn.execute(
        "UPDATE storage_provider_bindings SET \
         auth_state=?1,enabled=?2,capabilities_json=?3,quota_used_bytes=?4,quota_total_bytes=?5,\
         last_error_category=?6,last_error_message=?7,last_checked_at=?8,updated_at=?8 \
         WHERE account_id=?9",
        params![
            auth_state,
            i64::from(enabled),
            capabilities_json,
            quota_used_bytes,
            quota_total_bytes,
            error.map(|value| value.0),
            error.map(|value| value.1),
            timestamp,
            account_id,
        ],
    )?;
    if changed == 0 {
        return Err(AppError::Other(
            "Storage provider account was not found".into(),
        ));
    }
    Ok(())
}

pub fn delete_account(conn: &mut Connection, account_id: &str) -> AppResult<()> {
    let timestamp = now();
    let transaction = conn.transaction()?;
    let changed = transaction.execute(
        "UPDATE storage_provider_accounts SET deleted_at=?1,updated_at=?1,version=version+1 \
         WHERE id=?2 AND deleted_at IS NULL",
        params![timestamp, account_id],
    )?;
    if changed == 0 {
        return Err(AppError::Other(
            "Storage provider account was not found".into(),
        ));
    }
    transaction.execute(
        "UPDATE storage_provider_bindings SET enabled=0,auth_state='disconnected',updated_at=?1 \
         WHERE account_id=?2",
        params![timestamp, account_id],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn list_transfer_jobs(
    conn: &Connection,
    account_id: Option<&str>,
    limit: usize,
) -> AppResult<Vec<StorageTransferJobRow>> {
    let limit = limit.clamp(1, 200) as i64;
    let mut statement = conn.prepare(&format!(
        "{TRANSFER_SELECT} WHERE (?1 IS NULL OR account_id=?1) \
         ORDER BY created_at DESC,id DESC LIMIT ?2"
    ))?;
    let rows = statement.query_map(params![account_id, limit], transfer_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn insert_transfer_job(conn: &Connection, input: &NewStorageTransferJob) -> AppResult<()> {
    validate_transfer_operation(&input.operation)?;
    if input.display_name.trim().is_empty() || input.display_name.chars().count() > 512 {
        return Err(AppError::Other(
            "Storage transfer display name must contain between 1 and 512 characters".into(),
        ));
    }
    validate_bytes(input.bytes_total, "transfer size")?;
    let account_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM storage_provider_accounts WHERE id=?1 AND deleted_at IS NULL)",
        [&input.account_id],
        |row| row.get(0),
    )?;
    if !account_exists {
        return Err(AppError::Other(
            "Cannot enqueue a transfer for a missing storage account".into(),
        ));
    }
    let timestamp = now();
    conn.execute(
        "INSERT INTO storage_transfer_jobs \
         (id,account_id,operation,remote_item_id,display_name,destination_kind,destination_id,\
          status,bytes_transferred,bytes_total,created_at,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,'queued',0,?8,?9,?9)",
        params![
            input.id,
            input.account_id,
            input.operation,
            trim_optional(input.remote_item_id.as_deref()),
            input.display_name.trim(),
            trim_optional(input.destination_kind.as_deref()),
            trim_optional(input.destination_id.as_deref()),
            input.bytes_total,
            timestamp,
        ],
    )?;
    Ok(())
}

pub fn update_transfer_job(
    conn: &Connection,
    job_id: &str,
    progress: &StorageTransferProgress,
) -> AppResult<()> {
    validate_transfer_status(&progress.status)?;
    validate_bytes(Some(progress.bytes_transferred), "transferred bytes")?;
    validate_bytes(progress.bytes_total, "transfer size")?;
    if progress
        .bytes_total
        .is_some_and(|total| progress.bytes_transferred > total)
    {
        return Err(AppError::Other(
            "Transferred bytes cannot exceed total bytes".into(),
        ));
    }
    if progress
        .error_category
        .as_ref()
        .is_some_and(|value| value.len() > 64)
        || progress
            .error_message
            .as_ref()
            .is_some_and(|value| value.chars().count() > 2048)
    {
        return Err(AppError::Other(
            "Storage transfer error is too large".into(),
        ));
    }
    let current_status: Option<String> = conn
        .query_row(
            "SELECT status FROM storage_transfer_jobs WHERE id=?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()?;
    let current_status = current_status
        .ok_or_else(|| AppError::Other("Storage transfer job was not found".into()))?;
    if !valid_transfer_transition(&current_status, &progress.status) {
        return Err(AppError::Other(format!(
            "Invalid storage transfer transition from {current_status} to {}",
            progress.status
        )));
    }
    let timestamp = now();
    let completed_at = matches!(
        progress.status.as_str(),
        "completed" | "failed" | "cancelled"
    )
    .then_some(timestamp.as_str());
    let changed = conn.execute(
        "UPDATE storage_transfer_jobs SET status=?1,bytes_transferred=?2,bytes_total=?3,\
         error_category=?4,error_message=?5,updated_at=?6,completed_at=?7 WHERE id=?8",
        params![
            progress.status,
            progress.bytes_transferred,
            progress.bytes_total,
            trim_optional(progress.error_category.as_deref()),
            trim_optional(progress.error_message.as_deref()),
            timestamp,
            completed_at,
            job_id,
        ],
    )?;
    debug_assert_eq!(changed, 1);
    Ok(())
}

fn account_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StorageAccountRow> {
    Ok(StorageAccountRow {
        id: row.get(0)?,
        provider_id: row.get(1)?,
        display_name: row.get(2)?,
        account_subject: row.get(3)?,
        config_json: row.get(4)?,
        auth_state: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
        capabilities_json: row.get(7)?,
        quota_used_bytes: row.get(8)?,
        quota_total_bytes: row.get(9)?,
        last_error_category: row.get(10)?,
        last_error_message: row.get(11)?,
        last_checked_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn transfer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StorageTransferJobRow> {
    Ok(StorageTransferJobRow {
        id: row.get(0)?,
        account_id: row.get(1)?,
        operation: row.get(2)?,
        remote_item_id: row.get(3)?,
        display_name: row.get(4)?,
        destination_kind: row.get(5)?,
        destination_id: row.get(6)?,
        status: row.get(7)?,
        bytes_transferred: row.get(8)?,
        bytes_total: row.get(9)?,
        error_category: row.get(10)?,
        error_message: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        completed_at: row.get(14)?,
    })
}

fn validate_account(input: &UpsertStorageAccount) -> AppResult<()> {
    crate::storage::domain::validate_account_id(&input.id)
        .map_err(|error| AppError::Other(error.to_string()))?;
    crate::storage::domain::validate_provider_id(&input.provider_id)
        .map_err(|error| AppError::Other(error.to_string()))?;
    if input.display_name.trim().is_empty() || input.display_name.chars().count() > 120 {
        return Err(AppError::Other(
            "Storage account name must contain between 1 and 120 characters".into(),
        ));
    }
    if input
        .account_subject
        .as_ref()
        .is_some_and(|value| value.chars().count() > 512)
    {
        return Err(AppError::Other(
            "Storage account subject is too long".into(),
        ));
    }
    validate_auth_state(&input.auth_state)?;
    validate_json_object(&input.config_json, "account config")?;
    validate_json_object(&input.capabilities_json, "capabilities")
}

fn validate_auth_state(value: &str) -> AppResult<()> {
    if matches!(
        value,
        "disconnected" | "authorizing" | "connected" | "auth_required" | "error"
    ) {
        Ok(())
    } else {
        Err(AppError::Other(
            "Invalid storage authorization state".into(),
        ))
    }
}

fn validate_transfer_operation(value: &str) -> AppResult<()> {
    if matches!(
        value,
        "file_download"
            | "file_upload"
            | "knowledge_import"
            | "reading_import"
            | "object_upload"
            | "object_download"
    ) {
        Ok(())
    } else {
        Err(AppError::Other("Invalid storage transfer operation".into()))
    }
}

fn validate_transfer_status(value: &str) -> AppResult<()> {
    if matches!(
        value,
        "queued" | "running" | "paused" | "completed" | "failed" | "cancelled"
    ) {
        Ok(())
    } else {
        Err(AppError::Other("Invalid storage transfer status".into()))
    }
}

fn valid_transfer_transition(current: &str, next: &str) -> bool {
    current == next
        || matches!(
            (current, next),
            ("queued", "running" | "paused" | "failed" | "cancelled")
                | ("running", "paused" | "completed" | "failed" | "cancelled")
                | ("paused", "queued" | "running" | "failed" | "cancelled")
        )
}

fn validate_json_object(value: &str, label: &str) -> AppResult<()> {
    if value.len() > 32 * 1024 {
        return Err(AppError::Other(format!("Storage {label} is too large")));
    }
    let parsed: serde_json::Value = serde_json::from_str(value)?;
    if !parsed.is_object() {
        return Err(AppError::Other(format!(
            "Storage {label} must be a JSON object"
        )));
    }
    Ok(())
}

fn validate_bytes(value: Option<i64>, label: &str) -> AppResult<()> {
    if value.is_some_and(|value| value < 0) {
        Err(AppError::Other(format!(
            "Storage {label} cannot be negative"
        )))
    } else {
        Ok(())
    }
}

fn trim_optional(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn database() -> Connection {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::apply(&mut connection).unwrap();
        connection
    }

    fn account() -> UpsertStorageAccount {
        UpsertStorageAccount {
            id: "account-1".into(),
            provider_id: "future_drive".into(),
            display_name: "Personal drive".into(),
            account_subject: Some("user@example.com".into()),
            config_json: "{}".into(),
            auth_state: "connected".into(),
            enabled: true,
            capabilities_json: "{\"browse_files\":true}".into(),
        }
    }

    #[test]
    fn logical_accounts_and_device_bindings_round_trip_separately() {
        let mut connection = database();
        upsert_account(&mut connection, &account()).unwrap();
        let row = get_account(&connection, "account-1").unwrap().unwrap();
        assert_eq!(row.provider_id, "future_drive");
        assert_eq!(row.auth_state, "connected");
        assert!(row.enabled);

        update_binding_status(
            &connection,
            "account-1",
            "auth_required",
            false,
            "{\"browse_files\":true}",
            Some(25),
            Some(100),
            Some(("authentication", "Authorization expired")),
        )
        .unwrap();
        let row = get_account(&connection, "account-1").unwrap().unwrap();
        assert_eq!(row.quota_used_bytes, Some(25));
        assert_eq!(row.last_error_category.as_deref(), Some("authentication"));

        upsert_account(&mut connection, &account()).unwrap();
        let row = get_account(&connection, "account-1").unwrap().unwrap();
        assert_eq!(row.auth_state, "connected");
        assert_eq!(row.quota_used_bytes, Some(25));
        assert!(row.last_error_category.is_none());
        assert!(row.last_error_message.is_none());

        delete_account(&mut connection, "account-1").unwrap();
        assert!(get_account(&connection, "account-1").unwrap().is_none());
    }

    #[test]
    fn transfer_queue_uses_provider_independent_progress_states() {
        let mut connection = database();
        upsert_account(&mut connection, &account()).unwrap();
        insert_transfer_job(
            &connection,
            &NewStorageTransferJob {
                id: "job-1".into(),
                account_id: "account-1".into(),
                operation: "knowledge_import".into(),
                remote_item_id: Some("opaque-file".into()),
                display_name: "Notes.md".into(),
                destination_kind: Some("knowledge_collection".into()),
                destination_id: Some("collection-1".into()),
                bytes_total: Some(100),
            },
        )
        .unwrap();
        assert!(update_transfer_job(
            &connection,
            "job-1",
            &StorageTransferProgress {
                status: "completed".into(),
                bytes_transferred: 100,
                bytes_total: Some(100),
                error_category: None,
                error_message: None,
            },
        )
        .is_err());
        update_transfer_job(
            &connection,
            "job-1",
            &StorageTransferProgress {
                status: "running".into(),
                bytes_transferred: 50,
                bytes_total: Some(100),
                error_category: None,
                error_message: None,
            },
        )
        .unwrap();
        update_transfer_job(
            &connection,
            "job-1",
            &StorageTransferProgress {
                status: "completed".into(),
                bytes_transferred: 100,
                bytes_total: Some(100),
                error_category: None,
                error_message: None,
            },
        )
        .unwrap();
        let jobs = list_transfer_jobs(&connection, Some("account-1"), 10).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].status, "completed");
        assert!(jobs[0].completed_at.is_some());
        assert!(update_transfer_job(
            &connection,
            "job-1",
            &StorageTransferProgress {
                status: "running".into(),
                bytes_transferred: 100,
                bytes_total: Some(100),
                error_category: None,
                error_message: None,
            },
        )
        .is_err());
    }
}
