use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::sync::hlc::HybridTimestamp;
use crate::sync::payload::{self, SyncEntityType};

#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub change_id: String,
    pub device_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub operation: String,
    pub base_revision: Option<i64>,
    pub local_version: i64,
    pub hlc: String,
    pub payload_schema_version: i64,
    pub payload_encoding: String,
    pub payload: Option<String>,
    pub payload_hash: String,
    pub key_version: Option<i64>,
    pub attempt_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct AcceptedChange {
    pub change_id: String,
    pub server_seq: i64,
    pub revision: i64,
}

#[derive(Debug, Clone)]
pub struct ConflictChange {
    pub change_id: String,
    pub current_revision: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDbStatus {
    pub device_id: String,
    pub pending_count: i64,
    pub in_flight_count: i64,
    pub conflict_count: i64,
    pub dead_letter_count: i64,
    pub last_pull_cursor: i64,
    pub bootstrap_state: String,
    pub last_success_at: Option<i64>,
    pub last_error_code: Option<String>,
    pub backoff_until: Option<i64>,
}

pub fn device_id(conn: &Connection) -> AppResult<String> {
    conn.query_row(
        "SELECT device_id FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn enqueue_projection(
    conn: &Connection,
    entity_type: SyncEntityType,
    entity_id: &str,
    local_version: i64,
    deleted: bool,
    source: &Value,
) -> AppResult<String> {
    let entity_type_name = entity_type.as_str();
    let operation = if deleted { "delete" } else { "upsert" };
    let payload = if deleted {
        None
    } else {
        Some(serde_json::to_string(&payload::project(
            entity_type,
            source,
        )?)?)
    };
    let payload_hash = sha256_hex(payload.as_deref().unwrap_or_default().as_bytes());
    let base_revision = planned_base_revision(conn, entity_type_name, entity_id)?;
    let (device_id, last_hlc): (String, Option<String>) = conn.query_row(
        "SELECT device_id, last_hlc FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let node = device_id.chars().take(8).collect::<String>();
    let created_at = unix_millis();
    let hlc = HybridTimestamp::tick(last_hlc.as_deref(), created_at as u64, &node)
        .map_err(AppError::Other)?
        .to_string();
    let change_id = uuid::Uuid::new_v4().to_string();

    conn.execute(
        "INSERT INTO sync_outbox (change_id, device_id, entity_type, entity_id, operation, \
         base_revision, local_version, hlc, payload_schema_version, payload_encoding, payload, \
         payload_hash, key_version, status, attempt_count, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 'json', ?9, ?10, NULL, 'pending', 0, ?11)",
        params![
            change_id,
            device_id,
            entity_type_name,
            entity_id,
            operation,
            base_revision,
            local_version,
            hlc,
            payload,
            payload_hash,
            created_at,
        ],
    )?;
    conn.execute(
        "UPDATE sync_runtime_state SET last_hlc = ?1 WHERE singleton = 1",
        [&hlc],
    )?;
    Ok(change_id)
}

pub fn status(conn: &Connection) -> AppResult<SyncDbStatus> {
    conn.query_row(
        "SELECT r.device_id, \
           (SELECT COUNT(*) FROM sync_outbox WHERE status = 'pending'), \
           (SELECT COUNT(*) FROM sync_outbox WHERE status = 'in_flight'), \
           (SELECT COUNT(*) FROM sync_outbox WHERE status = 'conflict'), \
           (SELECT COUNT(*) FROM sync_outbox WHERE status = 'dead_letter'), \
           r.last_pull_cursor, r.bootstrap_state, r.last_success_at, r.last_error_code, \
           r.backoff_until FROM sync_runtime_state r WHERE r.singleton = 1",
        [],
        |row| {
            Ok(SyncDbStatus {
                device_id: row.get(0)?,
                pending_count: row.get(1)?,
                in_flight_count: row.get(2)?,
                conflict_count: row.get(3)?,
                dead_letter_count: row.get(4)?,
                last_pull_cursor: row.get(5)?,
                bootstrap_state: row.get(6)?,
                last_success_at: row.get(7)?,
                last_error_code: row.get(8)?,
                backoff_until: row.get(9)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn claim_pending(
    conn: &mut Connection,
    limit: usize,
    now_ms: i64,
) -> AppResult<Vec<OutboxRow>> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let backoff_until: Option<i64> = tx.query_row(
        "SELECT backoff_until FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if backoff_until.is_some_and(|value| value > now_ms) {
        tx.commit()?;
        return Ok(Vec::new());
    }
    let rows = {
        let mut statement = tx.prepare(
            "SELECT o.change_id, o.device_id, o.entity_type, o.entity_id, o.operation, \
                    o.base_revision, o.local_version, o.hlc, o.payload_schema_version, \
                    o.payload_encoding, o.payload, o.payload_hash, o.key_version, \
                    o.attempt_count, o.created_at \
             FROM sync_outbox o \
             WHERE o.status = 'pending' AND COALESCE(o.next_retry_at, 0) <= ?1 \
               AND NOT EXISTS ( \
                 SELECT 1 FROM sync_outbox blocker \
                 WHERE blocker.entity_type = o.entity_type AND blocker.entity_id = o.entity_id \
                   AND blocker.status IN ('conflict', 'dead_letter', 'in_flight') \
                   AND (blocker.created_at < o.created_at OR \
                        (blocker.created_at = o.created_at AND blocker.rowid < o.rowid)) \
               ) \
             ORDER BY o.created_at, o.rowid LIMIT ?2",
        )?;
        let selected = statement
            .query_map(params![now_ms, limit.min(20) as i64], map_outbox_row)?
            .collect::<Result<Vec<_>, _>>()?;
        selected
    };
    for row in &rows {
        tx.execute(
            "UPDATE sync_outbox SET status = 'in_flight', attempt_count = attempt_count + 1, \
             last_error_code = NULL, last_error_message = NULL WHERE change_id = ?1",
            [&row.change_id],
        )?;
    }
    tx.commit()?;
    Ok(rows)
}

pub fn apply_push_result(
    conn: &mut Connection,
    accepted: &[AcceptedChange],
    conflicts: &[ConflictChange],
    now_ms: i64,
) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for result in accepted {
        let row = get_outbox(&tx, &result.change_id)?.ok_or_else(|| {
            AppError::Other(format!(
                "accepted outbox change `{}` does not exist",
                result.change_id
            ))
        })?;
        if row.operation == "delete" {
            tx.execute(
                "INSERT INTO sync_entity_state (entity_type, entity_id, remote_revision, \
                 last_server_seq, last_payload_hash, last_synced_hlc, base_payload, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7) \
                 ON CONFLICT(entity_type, entity_id) DO UPDATE SET \
                   remote_revision = excluded.remote_revision, \
                   last_server_seq = excluded.last_server_seq, \
                   last_payload_hash = excluded.last_payload_hash, \
                   last_synced_hlc = excluded.last_synced_hlc, base_payload = NULL, \
                   updated_at = excluded.updated_at",
                params![
                    row.entity_type,
                    row.entity_id,
                    result.revision,
                    result.server_seq,
                    row.payload_hash,
                    row.hlc,
                    now_ms,
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO sync_entity_state (entity_type, entity_id, remote_revision, \
                 last_server_seq, last_payload_hash, last_synced_hlc, base_payload, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(entity_type, entity_id) DO UPDATE SET \
                   remote_revision = excluded.remote_revision, \
                   last_server_seq = excluded.last_server_seq, \
                   last_payload_hash = excluded.last_payload_hash, \
                   last_synced_hlc = excluded.last_synced_hlc, \
                   base_payload = excluded.base_payload, updated_at = excluded.updated_at",
                params![
                    row.entity_type,
                    row.entity_id,
                    result.revision,
                    result.server_seq,
                    row.payload_hash,
                    row.hlc,
                    row.payload,
                    now_ms,
                ],
            )?;
        }
        tx.execute(
            "UPDATE sync_outbox SET status = 'synced', synced_at = ?1, next_retry_at = NULL \
             WHERE change_id = ?2 AND status = 'in_flight'",
            params![now_ms, result.change_id],
        )?;
    }
    for conflict in conflicts {
        if let Some(remote_revision) = conflict.current_revision {
            tx.execute(
                "INSERT INTO sync_entity_state (entity_type, entity_id, remote_revision, updated_at) \
                 SELECT entity_type, entity_id, ?1, ?2 FROM sync_outbox WHERE change_id = ?3 \
                 ON CONFLICT(entity_type, entity_id) DO UPDATE SET \
                   remote_revision = excluded.remote_revision, updated_at = excluded.updated_at",
                params![remote_revision, now_ms, conflict.change_id],
            )?;
        }
        tx.execute(
            "UPDATE sync_outbox SET status = 'conflict', next_retry_at = NULL, \
             last_error_code = 'REVISION_CONFLICT', last_error_message = ?1 \
             WHERE change_id = ?2 AND status = 'in_flight'",
            params![
                format!("remote revision is {:?}", conflict.current_revision),
                conflict.change_id,
            ],
        )?;
    }
    tx.execute(
        "UPDATE sync_runtime_state SET last_success_at = ?1, last_error_code = NULL, \
         backoff_until = NULL WHERE singleton = 1",
        [now_ms],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn schedule_retry(
    conn: &mut Connection,
    change_ids: &[String],
    error_code: &str,
    error_message: &str,
    retry_at: i64,
) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for change_id in change_ids {
        tx.execute(
            "UPDATE sync_outbox SET status = 'pending', next_retry_at = ?1, \
             last_error_code = ?2, last_error_message = ?3 \
             WHERE change_id = ?4 AND status = 'in_flight'",
            params![retry_at, error_code, error_message, change_id],
        )?;
    }
    tx.execute(
        "UPDATE sync_runtime_state SET last_error_code = ?1, backoff_until = ?2 \
         WHERE singleton = 1",
        params![error_code, retry_at],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn mark_dead_letter(
    conn: &mut Connection,
    change_ids: &[String],
    error_code: &str,
    error_message: &str,
) -> AppResult<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for change_id in change_ids {
        tx.execute(
            "UPDATE sync_outbox SET status = 'dead_letter', next_retry_at = NULL, \
             last_error_code = ?1, last_error_message = ?2 \
             WHERE change_id = ?3 AND status = 'in_flight'",
            params![error_code, error_message, change_id],
        )?;
    }
    tx.execute(
        "UPDATE sync_runtime_state SET last_error_code = ?1 WHERE singleton = 1",
        [error_code],
    )?;
    tx.commit()?;
    Ok(())
}

fn get_outbox(conn: &Connection, change_id: &str) -> AppResult<Option<OutboxRow>> {
    conn.query_row(
        "SELECT change_id, device_id, entity_type, entity_id, operation, base_revision, \
                local_version, hlc, payload_schema_version, payload_encoding, payload, \
                payload_hash, key_version, attempt_count, created_at \
         FROM sync_outbox WHERE change_id = ?1",
        [change_id],
        map_outbox_row,
    )
    .optional()
    .map_err(Into::into)
}

fn map_outbox_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxRow> {
    Ok(OutboxRow {
        change_id: row.get(0)?,
        device_id: row.get(1)?,
        entity_type: row.get(2)?,
        entity_id: row.get(3)?,
        operation: row.get(4)?,
        base_revision: row.get(5)?,
        local_version: row.get(6)?,
        hlc: row.get(7)?,
        payload_schema_version: row.get(8)?,
        payload_encoding: row.get(9)?,
        payload: row.get(10)?,
        payload_hash: row.get(11)?,
        key_version: row.get(12)?,
        attempt_count: row.get(13)?,
        created_at: row.get(14)?,
    })
}

fn planned_base_revision(
    conn: &Connection,
    entity_type: &str,
    entity_id: &str,
) -> AppResult<Option<i64>> {
    let queued_base = conn
        .query_row(
            "SELECT base_revision FROM sync_outbox \
             WHERE entity_type = ?1 AND entity_id = ?2 AND status IN ('pending', 'in_flight') \
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![entity_type, entity_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?;
    if let Some(base_revision) = queued_base {
        return Ok(Some(base_revision.unwrap_or(0).saturating_add(1)));
    }
    let remote_revision = conn
        .query_row(
        "SELECT remote_revision FROM sync_entity_state WHERE entity_type = ?1 AND entity_id = ?2",
        params![entity_type, entity_id],
        |row| row.get::<_, Option<i64>>(0),
    )
        .optional()?;
    Ok(remote_revision.flatten())
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
