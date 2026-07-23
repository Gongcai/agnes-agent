use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
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
    pub source_payload: Option<String>,
    pub payload_hash: String,
    pub key_version: Option<i64>,
    pub attempt_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct SealedOutboxChange {
    pub change_id: String,
    pub payload_encoding: String,
    pub payload: Option<String>,
    pub payload_hash: String,
    pub key_version: Option<i64>,
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
    pub last_object_cursor: i64,
    pub bootstrap_state: String,
    pub last_success_at: Option<i64>,
    pub last_error_code: Option<String>,
    pub backoff_until: Option<i64>,
    pub e2ee_key_version: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflictRow {
    pub id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub base_revision: Option<i64>,
    pub remote_revision: Option<i64>,
    pub base_payload: Option<Value>,
    pub local_payload: Option<Value>,
    pub remote_payload: Option<Value>,
    pub local_deleted: bool,
    pub remote_deleted: bool,
    pub remote_ready: bool,
    pub conflicting_fields: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug)]
struct ConflictRecord {
    entity_type: String,
    entity_id: String,
    remote_revision: Option<i64>,
    base_payload: Option<Value>,
    local_payload: Option<Value>,
    remote_payload: Option<Value>,
    local_deleted: bool,
    remote_deleted: bool,
    remote_ready: bool,
    local_version: i64,
    local_hlc: String,
    remote_hlc: Option<String>,
    remote_payload_hash: Option<String>,
    remote_origin_device_id: Option<String>,
    remote_server_seq: Option<i64>,
    remote_updated_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RemoteEntityInput {
    pub protocol_version: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub revision: i64,
    pub hlc: String,
    pub deleted: bool,
    pub payload_schema_version: i64,
    pub payload_encoding: String,
    pub payload: Option<Value>,
    pub payload_hash: String,
    pub key_version: Option<i64>,
    pub origin_device_id: String,
    pub server_seq: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentPayload {
    id: String,
    name: String,
    persona: String,
    scenario: String,
    system_prompt: String,
    greeting: String,
    example_dialogue: String,
    model: String,
    tool_policy: String,
    tags: String,
    thinking_mode: String,
    thinking_budget: i64,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspacePayload {
    id: String,
    agent_id: String,
    name: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionPayload {
    id: String,
    agent_id: String,
    title: String,
    context_limit: Option<i64>,
    compress_threshold: f64,
    recency_window: i64,
    reserved_output_tokens: Option<i64>,
    model: Option<String>,
    thinking_mode: Option<String>,
    thinking_budget: Option<i64>,
    workspace_id: Option<String>,
    #[serde(default)]
    selected_root_id: Option<String>,
    summary: Option<String>,
    summary_updated_at: Option<String>,
    pinned: i64,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessagePayload {
    id: String,
    session_id: String,
    role: String,
    seq: i32,
    parent_id: Option<String>,
    selected_child_id: Option<String>,
    parts: Vec<MessageTextPartPayload>,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageTextPartPayload {
    kind: String,
    content: String,
    ordinal: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplicitMemoryPayload {
    id: String,
    agent_id: String,
    kind: String,
    content: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryPayload {
    id: String,
    agent_id: String,
    name: String,
    keywords: Vec<String>,
    content: String,
    creator: String,
    status: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarPayload {
    id: String,
    name: String,
    color: Option<String>,
    timezone: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarEventPayload {
    id: String,
    calendar_id: String,
    title: String,
    description: Option<String>,
    location: Option<String>,
    starts_at: String,
    ends_at: String,
    timezone: String,
    all_day: i64,
    recurrence_rule: Option<String>,
    recurrence_id: Option<String>,
    status: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EventExceptionPayload {
    id: String,
    event_id: String,
    original_occurrence: String,
    replacement_event_id: Option<String>,
    is_cancelled: i64,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskListPayload {
    id: String,
    name: String,
    color: Option<String>,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskPayload {
    id: String,
    task_list_id: String,
    parent_id: Option<String>,
    title: String,
    description: Option<String>,
    status: String,
    priority: i64,
    starts_at: Option<String>,
    due_date: Option<String>,
    due_at: Option<String>,
    due_timezone: Option<String>,
    is_important: i64,
    my_day_date: Option<String>,
    completed_at: Option<String>,
    recurrence_rule: Option<String>,
    recurrence_anchor: Option<String>,
    recurrence_source_id: Option<String>,
    sort_order: f64,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadingBookPayload {
    id: String,
    title: String,
    author: Option<String>,
    source_hash: String,
    model_knows_content: bool,
    content_context_allowed: bool,
    content_context_decided: bool,
    progress_cfi: Option<String>,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadingHighlightPayload {
    id: String,
    book_id: String,
    cfi_range: String,
    quote: String,
    context_before: String,
    context_after: String,
    note: Option<String>,
    color: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    #[serde(rename = "origin_device_id")]
    _origin_device_id: Option<String>,
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
         source_payload, payload_hash, key_version, status, attempt_count, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 'json', ?9, ?9, ?10, NULL, 'pending', 0, ?11)",
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
           (SELECT COUNT(*) FROM sync_conflicts WHERE status = 'pending'), \
           (SELECT COUNT(*) FROM sync_outbox WHERE status = 'dead_letter'), \
           r.last_pull_cursor, r.last_object_cursor, r.bootstrap_state, r.last_success_at, r.last_error_code, \
           r.backoff_until, r.e2ee_key_version \
         FROM sync_runtime_state r WHERE r.singleton = 1",
        [],
        |row| {
            Ok(SyncDbStatus {
                device_id: row.get(0)?,
                pending_count: row.get(1)?,
                in_flight_count: row.get(2)?,
                conflict_count: row.get(3)?,
                dead_letter_count: row.get(4)?,
                last_pull_cursor: row.get(5)?,
                last_object_cursor: row.get(6)?,
                bootstrap_state: row.get(7)?,
                last_success_at: row.get(8)?,
                last_error_code: row.get(9)?,
                backoff_until: row.get(10)?,
                e2ee_key_version: row.get(11)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn set_e2ee_key_version(conn: &Connection, key_version: Option<i64>) -> AppResult<()> {
    if key_version.is_some_and(|version| version <= 0) {
        return Err(AppError::Other(
            "E2EE key version must be a positive integer".into(),
        ));
    }
    let changed = conn.execute(
        "UPDATE sync_runtime_state SET e2ee_key_version = ?1 WHERE singleton = 1",
        [key_version],
    )?;
    if changed != 1 {
        return Err(AppError::Other("Sync runtime state is missing".into()));
    }
    Ok(())
}

pub fn advance_object_cursor(
    conn: &Connection,
    expected_cursor: i64,
    next_cursor: i64,
) -> AppResult<i64> {
    if expected_cursor < 0 || next_cursor < expected_cursor {
        return Err(AppError::Other(
            "Object cursor must advance monotonically".into(),
        ));
    }
    let changed = conn.execute(
        "UPDATE sync_runtime_state SET last_object_cursor = ?1 \
         WHERE singleton = 1 AND last_object_cursor = ?2",
        params![next_cursor, expected_cursor],
    )?;
    if changed == 1 {
        return Ok(next_cursor);
    }
    let current = conn
        .query_row(
            "SELECT last_object_cursor FROM sync_runtime_state WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .ok_or_else(|| AppError::Other("Sync runtime state is missing".into()))?;
    if current == next_cursor {
        Ok(current)
    } else {
        Err(AppError::Other(format!(
            "Object cursor changed concurrently from {expected_cursor} to {current}"
        )))
    }
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
                    o.payload_encoding, o.payload, o.source_payload, o.payload_hash, o.key_version, \
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

pub fn persist_sealed_outbox(
    conn: &mut Connection,
    changes: &[SealedOutboxChange],
) -> AppResult<Vec<OutboxRow>> {
    if changes.is_empty() {
        return Ok(Vec::new());
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut seen = HashSet::new();
    for change in changes {
        if !seen.insert(change.change_id.as_str()) {
            return Err(AppError::Other(format!(
                "sealed outbox batch repeats change `{}`",
                change.change_id
            )));
        }
        validate_sealed_outbox_change(change)?;
        let changed = tx.execute(
            "UPDATE sync_outbox SET payload_encoding = ?1, payload = ?2, payload_hash = ?3, \
             key_version = ?4 WHERE change_id = ?5 AND status = 'in_flight' \
             AND payload_encoding = 'json' AND ( \
               (?1 = 'xchacha20poly1305-v1' AND operation = 'upsert' AND source_payload IS NOT NULL) OR \
               (?1 = 'tombstone-v1' AND operation = 'delete' AND source_payload IS NULL) \
             )",
            params![
                change.payload_encoding,
                change.payload,
                change.payload_hash,
                change.key_version,
                change.change_id,
            ],
        )?;
        if changed != 1 {
            return Err(AppError::Other(format!(
                "outbox change `{}` is not eligible for encryption",
                change.change_id
            )));
        }
    }
    let rows = changes
        .iter()
        .map(|change| {
            get_outbox(&tx, &change.change_id)?.ok_or_else(|| {
                AppError::Other(format!(
                    "sealed outbox change `{}` disappeared",
                    change.change_id
                ))
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    tx.commit()?;
    Ok(rows)
}

fn validate_sealed_outbox_change(change: &SealedOutboxChange) -> AppResult<()> {
    let valid_hash = change.payload_hash.len() == 64
        && change
            .payload_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let valid_encrypted = change.payload_encoding == "xchacha20poly1305-v1"
        && change.key_version.is_some_and(|version| version > 0)
        && change.payload.as_deref().is_some_and(|payload| {
            serde_json::from_str::<String>(payload).is_ok_and(|encoded| !encoded.is_empty())
        });
    let valid_tombstone = change.payload_encoding == "tombstone-v1"
        && change.payload.is_none()
        && change.key_version.is_none()
        && change.payload_hash
            == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    if !valid_hash || (!valid_encrypted && !valid_tombstone) {
        return Err(AppError::Other(format!(
            "invalid sealed outbox change `{}`",
            change.change_id
        )));
    }
    Ok(())
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
                    row.source_payload,
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
        tx.execute(
            "UPDATE sync_outbox SET status = 'conflict', next_retry_at = NULL, \
             last_error_code = 'REVISION_CONFLICT', last_error_message = ?1 \
             WHERE change_id = ?2 AND status = 'in_flight'",
            params![
                format!("remote revision is {:?}", conflict.current_revision),
                conflict.change_id,
            ],
        )?;
        record_push_conflict(&tx, &conflict.change_id, conflict.current_revision, now_ms)?;
    }
    tx.execute(
        "UPDATE sync_runtime_state SET last_success_at = ?1, last_error_code = NULL, \
         backoff_until = NULL WHERE singleton = 1",
        [now_ms],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn apply_bootstrap_page(
    conn: &mut Connection,
    expected_state: &str,
    entities: &[RemoteEntityInput],
    snapshot_cursor: i64,
    next_cursor: Option<&str>,
    has_more: bool,
    now_ms: i64,
) -> AppResult<i64> {
    if snapshot_cursor < 0 {
        return Err(AppError::Other(
            "bootstrap snapshot cursor cannot be negative".into(),
        ));
    }
    if has_more != next_cursor.is_some() || (has_more && entities.is_empty()) {
        return Err(AppError::Other(
            "bootstrap pagination metadata is inconsistent".into(),
        ));
    }
    if entities
        .iter()
        .any(|entity| entity.server_seq > snapshot_cursor)
    {
        return Err(AppError::Other(
            "bootstrap entity is newer than the snapshot cursor".into(),
        ));
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current_state: String = tx.query_row(
        "SELECT bootstrap_state FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if current_state != expected_state {
        return Err(AppError::Other(format!(
            "bootstrap state changed from `{expected_state}` to `{current_state}`"
        )));
    }

    apply_remote_entities(&tx, entities, now_ms)?;
    let next_state = next_cursor
        .map(|cursor| format!("cursor:{snapshot_cursor}:{cursor}"))
        .unwrap_or_else(|| "complete".into());
    tx.execute(
        "UPDATE sync_runtime_state SET bootstrap_state = ?1, \
         last_pull_cursor = CASE WHEN ?2 = 'complete' THEN MAX(last_pull_cursor, ?3) \
                                 ELSE last_pull_cursor END, \
         last_success_at = ?4, last_error_code = NULL, backoff_until = NULL \
         WHERE singleton = 1",
        params![next_state, next_state, snapshot_cursor, now_ms],
    )?;
    tx.commit()?;
    Ok(if has_more { 0 } else { snapshot_cursor })
}

pub fn apply_pull_page(
    conn: &mut Connection,
    after: i64,
    entities: &[RemoteEntityInput],
    next_cursor: i64,
    has_more: bool,
    now_ms: i64,
) -> AppResult<i64> {
    if after < 0 || next_cursor < after || (has_more && entities.is_empty()) {
        return Err(AppError::Other(
            "pull pagination metadata is inconsistent".into(),
        ));
    }
    let mut previous_seq = after;
    for entity in entities {
        if entity.server_seq <= previous_seq {
            return Err(AppError::Other(
                "pull changes are not strictly ordered by server sequence".into(),
            ));
        }
        previous_seq = entity.server_seq;
    }
    if previous_seq != next_cursor {
        return Err(AppError::Other(
            "pull next cursor does not match the applied page".into(),
        ));
    }

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let (current_cursor, bootstrap_state): (i64, String) = tx.query_row(
        "SELECT last_pull_cursor, bootstrap_state FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if bootstrap_state != "complete" {
        return Err(AppError::Other(
            "pull cannot run before bootstrap completes".into(),
        ));
    }
    if current_cursor != after {
        return Err(AppError::Other(format!(
            "pull cursor changed from {after} to {current_cursor}"
        )));
    }

    apply_remote_entities(&tx, entities, now_ms)?;
    tx.execute(
        "UPDATE sync_runtime_state SET last_pull_cursor = ?1, last_success_at = ?2, \
         last_error_code = NULL, backoff_until = NULL WHERE singleton = 1",
        params![next_cursor, now_ms],
    )?;
    tx.commit()?;
    Ok(next_cursor)
}

pub fn record_runtime_failure(
    conn: &Connection,
    error_code: &str,
    backoff_until: Option<i64>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE sync_runtime_state SET last_error_code = ?1, backoff_until = ?2 \
         WHERE singleton = 1",
        params![error_code, backoff_until],
    )?;
    Ok(())
}

pub fn record_runtime_success(conn: &Connection, now_ms: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE sync_runtime_state SET last_success_at = ?1, last_error_code = NULL, \
         backoff_until = NULL WHERE singleton = 1",
        [now_ms],
    )?;
    Ok(())
}

fn apply_remote_entities(
    tx: &Transaction<'_>,
    entities: &[RemoteEntityInput],
    now_ms: i64,
) -> AppResult<()> {
    let (device_id, mut last_hlc): (String, Option<String>) = tx.query_row(
        "SELECT device_id, last_hlc FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let node = device_id.chars().take(8).collect::<String>();

    for entity in entities {
        validate_remote_entity(entity)?;
        let merged = HybridTimestamp::merge(
            last_hlc.as_deref(),
            Some(&entity.hlc),
            now_ms.max(0) as u64,
            &node,
        )
        .map_err(AppError::Other)?
        .to_string();
        last_hlc = Some(merged);
        apply_remote_entity(tx, entity, now_ms)?;
    }
    if let Some(last_hlc) = last_hlc {
        tx.execute(
            "UPDATE sync_runtime_state SET last_hlc = ?1 WHERE singleton = 1",
            [last_hlc],
        )?;
    }
    Ok(())
}

fn validate_remote_entity(entity: &RemoteEntityInput) -> AppResult<()> {
    if entity.protocol_version != 1
        || entity.revision <= 0
        || entity.server_seq <= 0
        || entity.payload_schema_version != 1
        || entity.payload_encoding != "json"
        || entity.key_version.is_some()
        || !matches!(
            entity.entity_type.as_str(),
            "agent"
                | "workspace"
                | "session"
                | "message"
                | "explicit_memory"
                | "memory"
                | "calendar"
                | "calendar_event"
                | "event_exception"
                | "task_list"
                | "task"
                | "reading_book"
                | "reading_highlight"
        )
    {
        return Err(AppError::Other(format!(
            "unsupported sync envelope for {} `{}`",
            entity.entity_type, entity.entity_id
        )));
    }
    if !is_valid_entity_id(&entity.entity_id)
        || uuid::Uuid::parse_str(&entity.origin_device_id).is_err()
        || HybridTimestamp::parse(&entity.hlc).is_err()
        || entity.payload_hash.len() != 64
        || !entity
            .payload_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AppError::Other(format!(
            "invalid sync metadata for {} `{}`",
            entity.entity_type, entity.entity_id
        )));
    }
    if entity.deleted == entity.payload.is_some() {
        return Err(AppError::Other(format!(
            "invalid payload/tombstone combination for {} `{}`",
            entity.entity_type, entity.entity_id
        )));
    }
    Ok(())
}

fn is_valid_entity_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || (index > 0 && b"._:-".contains(&byte))
        })
}

fn apply_remote_entity(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    now_ms: i64,
) -> AppResult<()> {
    let current = tx
        .query_row(
            "SELECT remote_revision, last_payload_hash FROM sync_entity_state \
             WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity.entity_type, entity.entity_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?;
    if let Some((Some(current_revision), current_hash)) = current {
        if current_revision == entity.revision
            && current_hash
                .as_deref()
                .is_some_and(|hash| hash != entity.payload_hash)
        {
            return Err(AppError::Other(format!(
                "remote revision {} for {} `{}` changed payload hash",
                entity.revision, entity.entity_type, entity.entity_id
            )));
        }
        if current_revision >= entity.revision {
            // A push conflict may arrive after this remote revision is already
            // cached locally. Hydrate the pending conflict before returning.
            hydrate_pending_conflict(tx, entity, now_ms)?;
            update_remote_server_seq(tx, entity, now_ms)?;
            return Ok(());
        }
    }

    let unsynced_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sync_outbox WHERE entity_type = ?1 AND entity_id = ?2 \
         AND status IN ('pending', 'in_flight', 'conflict', 'dead_letter')",
        params![entity.entity_type, entity.entity_id],
        |row| row.get(0),
    )?;
    if unsynced_count > 0 {
        tx.execute(
            "UPDATE sync_outbox SET status = 'conflict', next_retry_at = NULL, \
             last_error_code = 'REMOTE_CHANGE_CONFLICT', last_error_message = ?1 \
             WHERE entity_type = ?2 AND entity_id = ?3 \
               AND status IN ('pending', 'in_flight')",
            params![
                format!(
                    "remote revision {} arrived before the local change was resolved",
                    entity.revision
                ),
                entity.entity_type,
                entity.entity_id,
            ],
        )?;
        let conflict_id = record_remote_conflict(tx, entity, now_ms)?;
        upsert_remote_state(tx, entity, now_ms)?;
        try_auto_resolve_conflict(tx, &conflict_id, now_ms)?;
    } else {
        apply_remote_business_row(tx, entity)?;
        upsert_remote_state(tx, entity, now_ms)?;
    }
    Ok(())
}

fn hydrate_pending_conflict(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    now_ms: i64,
) -> AppResult<bool> {
    let expected_revision = tx
        .query_row(
            "SELECT remote_revision FROM sync_conflicts \
             WHERE entity_type = ?1 AND entity_id = ?2 AND status = 'pending' \
               AND remote_ready = 0",
            params![entity.entity_type, entity.entity_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()?;
    if expected_revision
        .flatten()
        .is_some_and(|revision| revision != entity.revision)
    {
        return Ok(false);
    }
    if expected_revision.is_none() {
        return Ok(false);
    }

    let conflict_id = record_remote_conflict(tx, entity, now_ms)?;
    try_auto_resolve_conflict(tx, &conflict_id, now_ms)?;
    Ok(true)
}

fn apply_remote_business_row(tx: &Transaction<'_>, entity: &RemoteEntityInput) -> AppResult<()> {
    if entity.deleted {
        return apply_remote_delete(tx, entity);
    }
    let payload = entity
        .payload
        .clone()
        .ok_or_else(|| AppError::Other("remote upsert payload is missing".into()))?;
    match entity.entity_type.as_str() {
        "agent" => apply_remote_agent(tx, entity, serde_json::from_value(payload)?),
        "workspace" => apply_remote_workspace(tx, entity, serde_json::from_value(payload)?),
        "session" => apply_remote_session(tx, entity, serde_json::from_value(payload)?),
        "message" => apply_remote_message(tx, entity, serde_json::from_value(payload)?),
        "explicit_memory" => {
            apply_remote_explicit_memory(tx, entity, serde_json::from_value(payload)?)
        }
        "memory" => apply_remote_memory(tx, entity, serde_json::from_value(payload)?),
        "calendar" => apply_remote_calendar(tx, entity, serde_json::from_value(payload)?),
        "calendar_event" => {
            apply_remote_calendar_event(tx, entity, serde_json::from_value(payload)?)
        }
        "event_exception" => {
            apply_remote_event_exception(tx, entity, serde_json::from_value(payload)?)
        }
        "task_list" => apply_remote_task_list(tx, entity, serde_json::from_value(payload)?),
        "task" => apply_remote_task(tx, entity, serde_json::from_value(payload)?),
        "reading_book" => apply_remote_reading_book(tx, entity, serde_json::from_value(payload)?),
        "reading_highlight" => {
            apply_remote_reading_highlight(tx, entity, serde_json::from_value(payload)?)
        }
        other => Err(AppError::Other(format!(
            "remote entity type `{other}` is not enabled yet"
        ))),
    }
}

fn apply_remote_agent(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: AgentPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    tx.execute(
        "INSERT INTO agents (id, name, persona, scenario, system_prompt, greeting, \
         example_dialogue, model, tool_policy, avatar, tags, thinking_mode, thinking_budget, \
         created_at, updated_at, version, deleted_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '', ?10, ?11, ?12, ?13, ?14, ?15, NULL, ?16) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, persona = excluded.persona, \
         scenario = excluded.scenario, system_prompt = excluded.system_prompt, \
         greeting = excluded.greeting, example_dialogue = excluded.example_dialogue, \
         model = excluded.model, tool_policy = excluded.tool_policy, tags = excluded.tags, \
         thinking_mode = excluded.thinking_mode, thinking_budget = excluded.thinking_budget, \
         created_at = excluded.created_at, updated_at = excluded.updated_at, \
         version = excluded.version, deleted_at = NULL, origin_device_id = excluded.origin_device_id",
        params![
            payload.id,
            payload.name,
            payload.persona,
            payload.scenario,
            payload.system_prompt,
            payload.greeting,
            payload.example_dialogue,
            payload.model,
            payload.tool_policy,
            payload.tags,
            payload.thinking_mode,
            payload.thinking_budget,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    Ok(())
}

fn apply_remote_workspace(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: WorkspacePayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    tx.execute(
        "INSERT INTO workspaces (id, agent_id, name, created_at, updated_at, version, deleted_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7) \
         ON CONFLICT(id) DO UPDATE SET agent_id = excluded.agent_id, name = excluded.name, \
         created_at = excluded.created_at, updated_at = excluded.updated_at, \
         version = excluded.version, deleted_at = NULL, origin_device_id = excluded.origin_device_id",
        params![
            payload.id,
            payload.agent_id,
            payload.name,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    Ok(())
}

fn apply_remote_session(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: SessionPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if !payload.compress_threshold.is_finite() || !matches!(payload.pinned, 0 | 1) {
        return Err(AppError::Other(format!(
            "invalid session payload for `{}`",
            entity.entity_id
        )));
    }
    if payload
        .selected_root_id
        .as_deref()
        .is_some_and(|id| !is_valid_entity_id(id))
    {
        return Err(AppError::Other(format!(
            "invalid session root for `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO sessions (id, agent_id, title, context_limit, compress_threshold, recency_window, \
         reserved_output_tokens, summarizer_model, model, thinking_mode, thinking_budget, \
         permission_mode, workspace_id, selected_root_id, summary, summary_updated_at, created_at, updated_at, \
         version, deleted_at, origin_device_id, pinned) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, 'auto', ?11, ?12, ?13, ?14, \
                 ?15, ?16, ?17, NULL, ?18, ?19) \
         ON CONFLICT(id) DO UPDATE SET agent_id = excluded.agent_id, title = excluded.title, \
         context_limit = excluded.context_limit, compress_threshold = excluded.compress_threshold, \
         recency_window = excluded.recency_window, reserved_output_tokens = excluded.reserved_output_tokens, \
         model = excluded.model, thinking_mode = excluded.thinking_mode, \
         thinking_budget = excluded.thinking_budget, workspace_id = excluded.workspace_id, \
         selected_root_id = excluded.selected_root_id, \
         summary = excluded.summary, summary_updated_at = excluded.summary_updated_at, \
         created_at = excluded.created_at, updated_at = excluded.updated_at, version = excluded.version, \
         deleted_at = NULL, origin_device_id = excluded.origin_device_id, pinned = excluded.pinned",
        params![
            payload.id,
            payload.agent_id,
            payload.title,
            payload.context_limit,
            payload.compress_threshold,
            payload.recency_window,
            payload.reserved_output_tokens,
            payload.model,
            payload.thinking_mode,
            payload.thinking_budget,
            payload.workspace_id,
            payload.selected_root_id,
            payload.summary,
            payload.summary_updated_at,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
            payload.pinned,
        ],
    )?;
    Ok(())
}

fn validate_payload_identity(
    entity: &RemoteEntityInput,
    payload_id: &str,
    payload_version: i64,
    deleted_at: Option<&str>,
) -> AppResult<()> {
    if payload_id != entity.entity_id || payload_version <= 0 || deleted_at.is_some() {
        return Err(AppError::Other(format!(
            "payload identity does not match {} `{}`",
            entity.entity_type, entity.entity_id
        )));
    }
    Ok(())
}

fn apply_remote_message(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: MessagePayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.seq < 0
        || !matches!(payload.role.as_str(), "user" | "assistant" | "system")
        || !is_valid_entity_id(&payload.session_id)
        || payload
            .parent_id
            .as_deref()
            .is_some_and(|id| !is_valid_entity_id(id) || id == entity.entity_id)
        || payload
            .selected_child_id
            .as_deref()
            .is_some_and(|id| !is_valid_entity_id(id) || id == entity.entity_id)
    {
        return Err(AppError::Other(format!(
            "invalid message payload for `{}`",
            entity.entity_id
        )));
    }
    let mut ordinals = HashSet::new();
    for part in &payload.parts {
        if part.kind != "text" || part.ordinal < 0 || !ordinals.insert(part.ordinal) {
            return Err(AppError::Other(format!(
                "invalid message text parts for `{}`",
                entity.entity_id
            )));
        }
    }

    tx.execute(
        "INSERT INTO messages (id, session_id, role, seq, status, model, token_count, metadata, \
         parent_id, selected_child_id, created_at, updated_at, version, deleted_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, 'complete', NULL, NULL, NULL, ?5, ?6, ?7, ?8, ?9, NULL, ?10) \
         ON CONFLICT(id) DO UPDATE SET session_id = excluded.session_id, role = excluded.role, \
         seq = excluded.seq, status = 'complete', parent_id = excluded.parent_id, \
         selected_child_id = excluded.selected_child_id, created_at = excluded.created_at, \
         updated_at = excluded.updated_at, version = excluded.version, deleted_at = NULL, \
         origin_device_id = excluded.origin_device_id",
        params![
            payload.id,
            payload.session_id,
            payload.role,
            payload.seq,
            payload.parent_id,
            payload.selected_child_id,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    tx.execute(
        "DELETE FROM message_parts WHERE message_id = ?1 AND kind = 'text'",
        [&entity.entity_id],
    )?;
    for part in payload.parts {
        tx.execute(
            "INSERT INTO message_parts (id, message_id, kind, ordinal, content) \
             VALUES (?1, ?2, 'text', ?3, ?4)",
            params![
                format!("{}:sync-text:{}", entity.entity_id, part.ordinal),
                entity.entity_id,
                part.ordinal,
                part.content,
            ],
        )?;
    }
    Ok(())
}

fn apply_remote_explicit_memory(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: ExplicitMemoryPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    let expected_id = super::explicit_memories::sync_entity_id(&payload.agent_id, &payload.kind)?;
    if !is_valid_entity_id(&payload.agent_id) || expected_id != entity.entity_id {
        return Err(AppError::Other(format!(
            "explicit memory identity does not match `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO explicit_memories (id, agent_id, kind, content, created_at, updated_at, \
         version, deleted_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8) \
         ON CONFLICT(agent_id, kind) DO UPDATE SET content = excluded.content, \
         created_at = excluded.created_at, updated_at = excluded.updated_at, \
         version = excluded.version, deleted_at = NULL, origin_device_id = excluded.origin_device_id",
        params![
            entity.entity_id,
            payload.agent_id,
            payload.kind,
            payload.content,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    Ok(())
}

fn apply_remote_memory(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: MemoryPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.name.trim().is_empty()
        || payload.content.trim().is_empty()
        || !is_valid_entity_id(&payload.agent_id)
        || !matches!(payload.creator.as_str(), "user" | "ai")
        || payload.status != "active"
        || super::memory::normalize_keywords(&payload.keywords) != payload.keywords
    {
        return Err(AppError::Other(format!(
            "invalid memory payload for `{}`",
            entity.entity_id
        )));
    }
    let previous = tx
        .query_row(
            "SELECT content, embedding_id FROM memory_store WHERE id = ?1",
            [&entity.entity_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    let keywords = serde_json::to_string(&payload.keywords)?;
    tx.execute(
        "INSERT INTO memory_store (id, agent_id, name, keywords, content, creator, type, scope, \
         source, confidence, status, created_at, updated_at, embedding_id, version, deleted_at, \
         origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Note', 'agent', 'sync', 1.0, 'active', ?7, ?8, \
                 NULL, ?9, NULL, ?10) \
         ON CONFLICT(id) DO UPDATE SET agent_id = excluded.agent_id, name = excluded.name, \
         keywords = excluded.keywords, content = excluded.content, creator = excluded.creator, \
         status = 'active', created_at = excluded.created_at, updated_at = excluded.updated_at, \
         embedding_id = CASE WHEN memory_store.content = excluded.content \
                             THEN memory_store.embedding_id ELSE NULL END, \
         version = excluded.version, deleted_at = NULL, origin_device_id = excluded.origin_device_id",
        params![
            payload.id,
            payload.agent_id,
            payload.name,
            keywords,
            payload.content,
            payload.creator,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    if let Some((old_content, Some(embedding_id))) = previous {
        if old_content != payload.content {
            super::memory::delete_embedding_by_id(tx, &embedding_id)?;
        }
    }
    Ok(())
}

fn apply_remote_calendar(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: CalendarPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.name.trim().is_empty() || payload.timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(AppError::Other(format!(
            "invalid calendar payload for `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO calendars (id,name,color,timezone,provider_account_id,created_at,updated_at,version,deleted_at,origin_device_id) \
         VALUES (?1,?2,?3,?4,NULL,?5,?6,?7,NULL,?8) \
         ON CONFLICT(id) DO UPDATE SET name=excluded.name,color=excluded.color,timezone=excluded.timezone, \
         created_at=excluded.created_at,updated_at=excluded.updated_at,version=excluded.version, \
         deleted_at=NULL,origin_device_id=excluded.origin_device_id",
        params![payload.id, payload.name, payload.color, payload.timezone, payload.created_at,
                payload.updated_at, payload.version, entity.origin_device_id],
    )?;
    Ok(())
}

fn apply_remote_calendar_event(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: CalendarEventPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.title.trim().is_empty()
        || !is_valid_entity_id(&payload.calendar_id)
        || !matches!(payload.all_day, 0 | 1)
        || !matches!(
            payload.status.as_str(),
            "confirmed" | "tentative" | "cancelled"
        )
        || payload.timezone.parse::<chrono_tz::Tz>().is_err()
        || payload
            .recurrence_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
    {
        return Err(AppError::Other(format!(
            "invalid calendar event payload for `{}`",
            entity.entity_id
        )));
    }
    let starts_at = chrono::DateTime::parse_from_rfc3339(&payload.starts_at).map_err(|_| {
        AppError::Other(format!(
            "invalid calendar event start for `{}`",
            entity.entity_id
        ))
    })?;
    let ends_at = chrono::DateTime::parse_from_rfc3339(&payload.ends_at).map_err(|_| {
        AppError::Other(format!(
            "invalid calendar event end for `{}`",
            entity.entity_id
        ))
    })?;
    if ends_at <= starts_at
        || payload
            .recurrence_rule
            .as_deref()
            .is_some_and(|rule| !rule.starts_with("RRULE:"))
    {
        return Err(AppError::Other(format!(
            "invalid calendar event schedule for `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO calendar_events (id,calendar_id,title,description,location,starts_at,ends_at,timezone,all_day, \
         recurrence_rule,recurrence_id,status,created_at,updated_at,version,deleted_at,origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,NULL,?16) \
         ON CONFLICT(id) DO UPDATE SET calendar_id=excluded.calendar_id,title=excluded.title, \
         description=excluded.description,location=excluded.location,starts_at=excluded.starts_at,ends_at=excluded.ends_at, \
         timezone=excluded.timezone,all_day=excluded.all_day,recurrence_rule=excluded.recurrence_rule, \
         recurrence_id=excluded.recurrence_id,status=excluded.status,created_at=excluded.created_at, \
         updated_at=excluded.updated_at,version=excluded.version,deleted_at=NULL,origin_device_id=excluded.origin_device_id",
        params![payload.id, payload.calendar_id, payload.title, payload.description, payload.location,
                payload.starts_at, payload.ends_at, payload.timezone, payload.all_day, payload.recurrence_rule,
                payload.recurrence_id, payload.status, payload.created_at, payload.updated_at, payload.version,
                entity.origin_device_id],
    )?;
    Ok(())
}

fn apply_remote_event_exception(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: EventExceptionPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.id
        != super::planner::event_exception_entity_id(
            &payload.event_id,
            &payload.original_occurrence,
        )
        || !is_valid_entity_id(&payload.event_id)
        || chrono::DateTime::parse_from_rfc3339(&payload.original_occurrence).is_err()
        || !matches!(payload.is_cancelled, 0 | 1)
        || (payload.is_cancelled == 1 && payload.replacement_event_id.is_some())
        || payload
            .replacement_event_id
            .as_deref()
            .is_some_and(|id| !is_valid_entity_id(id))
    {
        return Err(AppError::Other(format!(
            "invalid event exception payload for `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO event_exceptions (event_id,original_occurrence,replacement_event_id,is_cancelled,created_at,updated_at, \
         version,deleted_at,origin_device_id) VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8) \
         ON CONFLICT(event_id,original_occurrence) DO UPDATE SET replacement_event_id=excluded.replacement_event_id, \
         is_cancelled=excluded.is_cancelled,created_at=excluded.created_at,updated_at=excluded.updated_at, \
         version=excluded.version,deleted_at=NULL,origin_device_id=excluded.origin_device_id",
        params![payload.event_id, payload.original_occurrence, payload.replacement_event_id, payload.is_cancelled,
                payload.created_at, payload.updated_at, payload.version, entity.origin_device_id],
    )?;
    Ok(())
}

fn apply_remote_task_list(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: TaskListPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.name.trim().is_empty() {
        return Err(AppError::Other(format!(
            "invalid task list payload for `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO task_lists (id,name,color,provider_account_id,created_at,updated_at,version,deleted_at,origin_device_id) \
         VALUES (?1,?2,?3,NULL,?4,?5,?6,NULL,?7) \
         ON CONFLICT(id) DO UPDATE SET name=excluded.name,color=excluded.color,created_at=excluded.created_at, \
         updated_at=excluded.updated_at,version=excluded.version,deleted_at=NULL,origin_device_id=excluded.origin_device_id",
        params![payload.id, payload.name, payload.color, payload.created_at, payload.updated_at,
                payload.version, entity.origin_device_id],
    )?;
    Ok(())
}

fn task_parent_exists(tx: &Transaction<'_>, parent_id: &str) -> AppResult<bool> {
    tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1)",
        [parent_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn reconcile_remote_task_parents(tx: &Transaction<'_>) -> AppResult<()> {
    tx.execute(
        "UPDATE tasks SET parent_id=(SELECT parent_id FROM task_sync_parents p WHERE p.task_id=tasks.id) \
         WHERE EXISTS (SELECT 1 FROM task_sync_parents p JOIN tasks parent ON parent.id=p.parent_id \
                       WHERE p.task_id=tasks.id)",
        [],
    )?;
    Ok(())
}

fn apply_remote_task(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: TaskPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.title.trim().is_empty()
        || !is_valid_entity_id(&payload.task_list_id)
        || payload
            .parent_id
            .as_deref()
            .is_some_and(|id| !is_valid_entity_id(id) || id == payload.id)
        || payload
            .recurrence_source_id
            .as_deref()
            .is_some_and(|id| !is_valid_entity_id(id) || id == payload.id)
        || !matches!(payload.status.as_str(), "open" | "completed" | "cancelled")
        || !(0..=4).contains(&payload.priority)
        || !matches!(payload.is_important, 0 | 1)
        || !payload.sort_order.is_finite()
        || (payload.due_date.is_some() && payload.due_at.is_some())
    {
        return Err(AppError::Other(format!(
            "invalid task payload for `{}`",
            entity.entity_id
        )));
    }
    if payload
        .due_at
        .as_deref()
        .is_some_and(|value| chrono::DateTime::parse_from_rfc3339(value).is_err())
        || payload
            .due_date
            .as_deref()
            .is_some_and(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err())
        || payload
            .my_day_date
            .as_deref()
            .is_some_and(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err())
        || payload
            .due_timezone
            .as_deref()
            .is_some_and(|value| value.parse::<chrono_tz::Tz>().is_err())
    {
        return Err(AppError::Other(format!(
            "invalid task schedule for `{}`",
            entity.entity_id
        )));
    }
    let persisted_parent = match payload.parent_id.as_deref() {
        Some(parent_id) if task_parent_exists(tx, parent_id)? => Some(parent_id),
        _ => None,
    };
    tx.execute(
        "INSERT INTO tasks (id,task_list_id,parent_id,title,description,status,priority,starts_at,due_date,due_at, \
         due_timezone,is_important,my_day_date,completed_at,recurrence_rule,recurrence_anchor,recurrence_source_id, \
         sort_order,created_at,updated_at,version,deleted_at,origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,NULL,?22) \
         ON CONFLICT(id) DO UPDATE SET task_list_id=excluded.task_list_id,parent_id=excluded.parent_id, \
         title=excluded.title,description=excluded.description,status=excluded.status,priority=excluded.priority, \
         starts_at=excluded.starts_at,due_date=excluded.due_date,due_at=excluded.due_at,due_timezone=excluded.due_timezone, \
         is_important=excluded.is_important,my_day_date=excluded.my_day_date,completed_at=excluded.completed_at, \
         recurrence_rule=excluded.recurrence_rule,recurrence_anchor=excluded.recurrence_anchor, \
         recurrence_source_id=excluded.recurrence_source_id,sort_order=excluded.sort_order,created_at=excluded.created_at, \
         updated_at=excluded.updated_at,version=excluded.version,deleted_at=NULL,origin_device_id=excluded.origin_device_id",
        params![payload.id, payload.task_list_id, persisted_parent, payload.title, payload.description, payload.status,
                payload.priority, payload.starts_at, payload.due_date, payload.due_at, payload.due_timezone,
                payload.is_important, payload.my_day_date, payload.completed_at, payload.recurrence_rule,
                payload.recurrence_anchor, payload.recurrence_source_id, payload.sort_order, payload.created_at,
                payload.updated_at, payload.version, entity.origin_device_id],
    )?;
    tx.execute(
        "INSERT INTO task_sync_parents (task_id,parent_id) VALUES (?1,?2) \
         ON CONFLICT(task_id) DO UPDATE SET parent_id=excluded.parent_id",
        params![entity.entity_id, payload.parent_id],
    )?;
    reconcile_remote_task_parents(tx)?;
    Ok(())
}

fn apply_remote_reading_book(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: ReadingBookPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if payload.title.trim().is_empty()
        || payload.title.chars().count() > 1_024
        || !is_sha256_hex(&payload.source_hash)
        || payload
            .progress_cfi
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 4_096)
        || payload
            .author
            .as_deref()
            .is_some_and(|value| value.chars().count() > 1_024)
    {
        return Err(AppError::Other(format!(
            "invalid reading book payload for `{}`",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO reading_books
         (id,collection_id,document_id,local_path,title,author,source_hash,model_knows_content,
          content_context_allowed,content_context_decided,progress_cfi,created_at,updated_at,version,
          deleted_at,origin_device_id)
         VALUES (?1,NULL,NULL,NULL,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL,?12)
         ON CONFLICT(id) DO UPDATE SET title=excluded.title,author=excluded.author,
         source_hash=excluded.source_hash,model_knows_content=excluded.model_knows_content,
         content_context_allowed=excluded.content_context_allowed,
         content_context_decided=excluded.content_context_decided,progress_cfi=excluded.progress_cfi,
         created_at=excluded.created_at,updated_at=excluded.updated_at,version=excluded.version,
         deleted_at=NULL,origin_device_id=excluded.origin_device_id",
        params![
            payload.id,
            payload.title.trim(),
            payload.author.as_deref().map(str::trim).filter(|value| !value.is_empty()),
            payload.source_hash,
            i64::from(payload.model_knows_content),
            i64::from(payload.content_context_allowed),
            i64::from(payload.content_context_decided),
            payload.progress_cfi,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    Ok(())
}

fn apply_remote_reading_highlight(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    payload: ReadingHighlightPayload,
) -> AppResult<()> {
    validate_payload_identity(
        entity,
        &payload.id,
        payload.version,
        payload.deleted_at.as_deref(),
    )?;
    if !is_valid_entity_id(&payload.book_id)
        || payload.cfi_range.trim().is_empty()
        || payload.cfi_range.len() > 8_192
        || payload.quote.trim().is_empty()
        || payload.quote.len() > 20_000
        || payload.context_before.len() > 8_000
        || payload.context_after.len() > 8_000
        || payload
            .note
            .as_deref()
            .is_some_and(|value| value.len() > 20_000)
        || !matches!(payload.color.as_str(), "yellow" | "green" | "blue" | "pink")
    {
        return Err(AppError::Other(format!(
            "invalid reading highlight payload for `{}`",
            entity.entity_id
        )));
    }
    let book_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM reading_books WHERE id=?1 AND deleted_at IS NULL)",
        [&payload.book_id],
        |row| row.get(0),
    )?;
    if !book_exists {
        return Err(AppError::Other(format!(
            "reading highlight `{}` references a missing book",
            entity.entity_id
        )));
    }
    tx.execute(
        "INSERT INTO reading_highlights
         (id,book_id,cfi_range,quote,context_before,context_after,note,color,created_at,updated_at,
          version,deleted_at,origin_device_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL,?12)
         ON CONFLICT(id) DO UPDATE SET book_id=excluded.book_id,cfi_range=excluded.cfi_range,
         quote=excluded.quote,context_before=excluded.context_before,context_after=excluded.context_after,
         note=excluded.note,color=excluded.color,created_at=excluded.created_at,
         updated_at=excluded.updated_at,version=excluded.version,deleted_at=NULL,
         origin_device_id=excluded.origin_device_id",
        params![
            payload.id,
            payload.book_id,
            payload.cfi_range,
            payload.quote,
            payload.context_before,
            payload.context_after,
            payload.note,
            payload.color,
            payload.created_at,
            payload.updated_at,
            payload.version,
            entity.origin_device_id,
        ],
    )?;
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn apply_remote_delete(tx: &Transaction<'_>, entity: &RemoteEntityInput) -> AppResult<()> {
    let deleted_at = entity.updated_at.to_string();
    match entity.entity_type.as_str() {
        "agent" => {
            tx.execute(
                "UPDATE agents SET deleted_at = ?1, updated_at = ?1, \
                 version = MAX(version + 1, ?2), origin_device_id = ?3 WHERE id = ?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
        }
        "workspace" => {
            tx.execute(
                "UPDATE sessions SET workspace_id = NULL, updated_at = ?1, version = version + 1, \
                 origin_device_id = ?2 WHERE workspace_id = ?3 AND NOT EXISTS ( \
                   SELECT 1 FROM sync_outbox o WHERE o.entity_type = 'session' \
                     AND o.entity_id = sessions.id \
                     AND o.status IN ('pending', 'in_flight', 'conflict', 'dead_letter'))",
                params![deleted_at, entity.origin_device_id, entity.entity_id],
            )?;
            tx.execute(
                "UPDATE workspaces SET deleted_at = ?1, updated_at = ?1, \
                 version = MAX(version + 1, ?2), origin_device_id = ?3 WHERE id = ?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
            tx.execute(
                "DELETE FROM workspace_bindings WHERE workspace_id = ?1",
                [&entity.entity_id],
            )?;
        }
        "session" => {
            tx.execute(
                "UPDATE sessions SET deleted_at = ?1, updated_at = ?1, \
                 version = MAX(version + 1, ?2), origin_device_id = ?3 WHERE id = ?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
        }
        "message" => {
            tx.execute(
                "UPDATE messages SET deleted_at = ?1, updated_at = ?1, \
                 version = MAX(version + 1, ?2), origin_device_id = ?3 WHERE id = ?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
        }
        "reading_book" => {
            tx.execute(
                "UPDATE reading_books SET deleted_at=?1,updated_at=?1,
                 version=MAX(version+1,?2),origin_device_id=?3 WHERE id=?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
        }
        "reading_highlight" => {
            tx.execute(
                "UPDATE reading_highlights SET deleted_at=?1,updated_at=?1,
                 version=MAX(version+1,?2),origin_device_id=?3 WHERE id=?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
        }
        "explicit_memory" => {
            let rows = {
                let mut statement = tx.prepare(
                    "SELECT id, agent_id, kind FROM explicit_memories WHERE deleted_at IS NULL",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            if let Some((local_id, _, _)) = rows.into_iter().find(|(_, agent_id, kind)| {
                matches!(
                    super::explicit_memories::sync_entity_id(agent_id, kind),
                    Ok(sync_id) if sync_id == entity.entity_id
                )
            }) {
                tx.execute(
                    "UPDATE explicit_memories SET deleted_at = ?1, updated_at = ?1, \
                     version = MAX(version + 1, ?2), origin_device_id = ?3 WHERE id = ?4",
                    params![
                        deleted_at,
                        entity.revision,
                        entity.origin_device_id,
                        local_id
                    ],
                )?;
            }
        }
        "memory" => {
            let embedding_id = tx
                .query_row(
                    "SELECT embedding_id FROM memory_store WHERE id = ?1",
                    [&entity.entity_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            tx.execute(
                "UPDATE memory_store SET status = 'deleted', embedding_id = NULL, deleted_at = ?1, \
                 updated_at = ?1, version = MAX(version + 1, ?2), origin_device_id = ?3 \
                 WHERE id = ?4",
                params![
                    deleted_at,
                    entity.revision,
                    entity.origin_device_id,
                    entity.entity_id
                ],
            )?;
            if let Some(embedding_id) = embedding_id {
                super::memory::delete_embedding_by_id(tx, &embedding_id)?;
            }
        }
        "calendar" => {
            tx.execute(
                "UPDATE calendars SET deleted_at=?1,updated_at=?1,version=MAX(version+1,?2),origin_device_id=?3 \
                 WHERE id=?4",
                params![deleted_at, entity.revision, entity.origin_device_id, entity.entity_id],
            )?;
        }
        "calendar_event" => {
            tx.execute(
                "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=MAX(version+1,?2),origin_device_id=?3 \
                 WHERE id=?4",
                params![deleted_at, entity.revision, entity.origin_device_id, entity.entity_id],
            )?;
        }
        "event_exception" => {
            let payload = tx
                .query_row(
                    "SELECT base_payload FROM sync_entity_state WHERE entity_type='event_exception' AND entity_id=?1",
                    [&entity.entity_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten()
                .map(|raw| serde_json::from_str::<EventExceptionPayload>(&raw))
                .transpose()?;
            if let Some(payload) = payload {
                tx.execute(
                    "UPDATE event_exceptions SET deleted_at=?1,updated_at=?1,version=MAX(version+1,?2), \
                     origin_device_id=?3 WHERE event_id=?4 AND original_occurrence=?5",
                    params![deleted_at, entity.revision, entity.origin_device_id, payload.event_id, payload.original_occurrence],
                )?;
            }
        }
        "task_list" => {
            tx.execute(
                "UPDATE task_lists SET deleted_at=?1,updated_at=?1,version=MAX(version+1,?2),origin_device_id=?3 \
                 WHERE id=?4",
                params![deleted_at, entity.revision, entity.origin_device_id, entity.entity_id],
            )?;
        }
        "task" => {
            tx.execute(
                "UPDATE tasks SET deleted_at=?1,updated_at=?1,version=MAX(version+1,?2),origin_device_id=?3 \
                 WHERE id=?4",
                params![deleted_at, entity.revision, entity.origin_device_id, entity.entity_id],
            )?;
        }
        other => {
            return Err(AppError::Other(format!(
                "remote entity type `{other}` is not enabled yet"
            )));
        }
    }
    Ok(())
}

fn upsert_remote_state(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    now_ms: i64,
) -> AppResult<()> {
    let base_payload = entity
        .payload
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "INSERT INTO sync_entity_state (entity_type, entity_id, remote_revision, last_server_seq, \
         last_payload_hash, last_synced_hlc, base_payload, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(entity_type, entity_id) DO UPDATE SET \
           remote_revision = excluded.remote_revision, \
           last_server_seq = MAX(COALESCE(sync_entity_state.last_server_seq, 0), excluded.last_server_seq), \
           last_payload_hash = excluded.last_payload_hash, last_synced_hlc = excluded.last_synced_hlc, \
           base_payload = excluded.base_payload, updated_at = excluded.updated_at",
        params![
            entity.entity_type,
            entity.entity_id,
            entity.revision,
            entity.server_seq,
            entity.payload_hash,
            entity.hlc,
            base_payload,
            now_ms,
        ],
    )?;
    Ok(())
}

fn update_remote_server_seq(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    now_ms: i64,
) -> AppResult<()> {
    tx.execute(
        "UPDATE sync_entity_state SET last_server_seq = MAX(COALESCE(last_server_seq, 0), ?1), \
         updated_at = ?2 WHERE entity_type = ?3 AND entity_id = ?4",
        params![
            entity.server_seq,
            now_ms,
            entity.entity_type,
            entity.entity_id
        ],
    )?;
    Ok(())
}

fn record_push_conflict(
    tx: &Transaction<'_>,
    change_id: &str,
    remote_revision: Option<i64>,
    now_ms: i64,
) -> AppResult<String> {
    let row = get_outbox(tx, change_id)?.ok_or_else(|| {
        AppError::Other(format!(
            "conflicting outbox change `{change_id}` does not exist"
        ))
    })?;
    upsert_local_conflict(
        tx,
        &row.entity_type,
        &row.entity_id,
        remote_revision,
        now_ms,
    )
}

fn record_remote_conflict(
    tx: &Transaction<'_>,
    entity: &RemoteEntityInput,
    now_ms: i64,
) -> AppResult<String> {
    let conflict_id = upsert_local_conflict(
        tx,
        &entity.entity_type,
        &entity.entity_id,
        Some(entity.revision),
        now_ms,
    )?;
    let remote_payload = entity
        .payload
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "UPDATE sync_conflicts SET remote_revision = ?1, remote_payload = ?2, \
         remote_deleted = ?3, remote_ready = 1, remote_hlc = ?4, remote_payload_hash = ?5, \
         remote_origin_device_id = ?6, remote_server_seq = ?7, remote_updated_at = ?8, \
         updated_at = ?9 WHERE id = ?10 AND status = 'pending'",
        params![
            entity.revision,
            remote_payload,
            entity.deleted,
            entity.hlc,
            entity.payload_hash,
            entity.origin_device_id,
            entity.server_seq,
            entity.updated_at,
            now_ms,
            conflict_id,
        ],
    )?;
    update_conflicting_fields(tx, &conflict_id)?;
    Ok(conflict_id)
}

fn upsert_local_conflict(
    tx: &Transaction<'_>,
    entity_type: &str,
    entity_id: &str,
    remote_revision: Option<i64>,
    now_ms: i64,
) -> AppResult<String> {
    let local = latest_unsynced_outbox(tx, entity_type, entity_id)?.ok_or_else(|| {
        AppError::Other(format!(
            "conflicting local change for {entity_type} `{entity_id}` does not exist"
        ))
    })?;
    let base = tx
        .query_row(
            "SELECT remote_revision, base_payload FROM sync_entity_state \
             WHERE entity_type = ?1 AND entity_id = ?2",
            params![entity_type, entity_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()?;
    let (base_revision, base_payload) = base.unwrap_or((None, None));
    let existing = tx
        .query_row(
            "SELECT id FROM sync_conflicts WHERE entity_type = ?1 AND entity_id = ?2 \
             AND status = 'pending'",
            params![entity_type, entity_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(id) = existing {
        tx.execute(
            "UPDATE sync_conflicts SET remote_revision = COALESCE(?1, remote_revision), \
             local_payload = ?2, local_deleted = ?3, local_version = ?4, local_hlc = ?5, \
             updated_at = ?6 WHERE id = ?7",
            params![
                remote_revision,
                local.source_payload,
                local.operation == "delete",
                local.local_version,
                local.hlc,
                now_ms,
                id,
            ],
        )?;
        return Ok(id);
    }

    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO sync_conflicts (id, entity_type, entity_id, base_revision, remote_revision, \
         base_payload, local_payload, local_deleted, local_version, local_hlc, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id,
            entity_type,
            entity_id,
            base_revision,
            remote_revision,
            base_payload,
            local.source_payload,
            local.operation == "delete",
            local.local_version,
            local.hlc,
            now_ms,
        ],
    )?;
    Ok(id)
}

fn latest_unsynced_outbox(
    conn: &Connection,
    entity_type: &str,
    entity_id: &str,
) -> AppResult<Option<OutboxRow>> {
    conn.query_row(
        "SELECT change_id, device_id, entity_type, entity_id, operation, base_revision, \
                local_version, hlc, payload_schema_version, payload_encoding, payload, \
                source_payload, payload_hash, key_version, attempt_count, created_at FROM sync_outbox \
         WHERE entity_type = ?1 AND entity_id = ?2 \
           AND status IN ('pending', 'in_flight', 'conflict', 'dead_letter') \
         ORDER BY created_at DESC, rowid DESC LIMIT 1",
        params![entity_type, entity_id],
        map_outbox_row,
    )
    .optional()
    .map_err(Into::into)
}

fn update_conflicting_fields(tx: &Transaction<'_>, conflict_id: &str) -> AppResult<()> {
    let conflict = get_conflict(tx, conflict_id)?;
    let fields = if conflict.local_deleted || conflict.remote_deleted {
        vec!["deleted_at".to_string()]
    } else if let (Some(local), Some(remote)) = (
        conflict.local_payload.as_ref(),
        conflict.remote_payload.as_ref(),
    ) {
        match merge_conflict_payload(&conflict, local, remote)? {
            crate::sync::merge::MergeOutcome::Merged {
                resolved_fields, ..
            } => resolved_fields,
            crate::sync::merge::MergeOutcome::Conflict(fields) => fields,
        }
    } else {
        Vec::new()
    };
    tx.execute(
        "UPDATE sync_conflicts SET conflicting_fields = ?1 WHERE id = ?2",
        params![serde_json::to_string(&fields)?, conflict_id],
    )?;
    Ok(())
}

fn try_auto_resolve_conflict(
    tx: &Transaction<'_>,
    conflict_id: &str,
    now_ms: i64,
) -> AppResult<bool> {
    let conflict = get_conflict(tx, conflict_id)?;
    if !conflict.remote_ready
        || conflict.local_deleted
        || conflict.remote_deleted
        || !matches!(
            conflict.entity_type.as_str(),
            "agent"
                | "session"
                | "workspace"
                | "message"
                | "explicit_memory"
                | "calendar"
                | "calendar_event"
                | "task_list"
                | "task"
        )
    {
        return Ok(false);
    }
    let (Some(local), Some(remote)) = (
        conflict.local_payload.as_ref(),
        conflict.remote_payload.as_ref(),
    ) else {
        return Ok(false);
    };
    let crate::sync::merge::MergeOutcome::Merged {
        payload: merged, ..
    } = merge_conflict_payload(&conflict, local, remote)?
    else {
        return Ok(false);
    };

    supersede_conflicting_outbox(tx, &conflict, "CONFLICT_AUTO_MERGED", now_ms)?;
    apply_local_resolution(tx, &conflict, merged, now_ms)?;
    mark_conflict_resolved(tx, conflict_id, "auto_merge", now_ms)?;
    Ok(true)
}

fn merge_conflict_payload(
    conflict: &ConflictRecord,
    local: &Value,
    remote: &Value,
) -> AppResult<crate::sync::merge::MergeOutcome> {
    let remote_hlc = conflict
        .remote_hlc
        .as_deref()
        .ok_or_else(|| AppError::Other("Remote conflict HLC is missing".into()))?;
    crate::sync::merge::merge_entity(
        &conflict.entity_type,
        conflict.base_payload.as_ref(),
        local,
        remote,
        &conflict.local_hlc,
        remote_hlc,
    )
}

pub fn list_conflicts(conn: &Connection) -> AppResult<Vec<SyncConflictRow>> {
    let mut statement = conn.prepare(
        "SELECT id, entity_type, entity_id, base_revision, remote_revision, base_payload, \
         local_payload, remote_payload, local_deleted, remote_deleted, remote_ready, \
         conflicting_fields, created_at, updated_at FROM sync_conflicts \
         WHERE status = 'pending' ORDER BY updated_at DESC, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, bool>(8)?,
            row.get::<_, bool>(9)?,
            row.get::<_, bool>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, i64>(12)?,
            row.get::<_, i64>(13)?,
        ))
    })?;
    rows.map(|row| {
        let row = row?;
        Ok(SyncConflictRow {
            id: row.0,
            entity_type: row.1,
            entity_id: row.2,
            base_revision: row.3,
            remote_revision: row.4,
            base_payload: parse_optional_json(row.5)?,
            local_payload: parse_optional_json(row.6)?,
            remote_payload: parse_optional_json(row.7)?,
            local_deleted: row.8,
            remote_deleted: row.9,
            remote_ready: row.10,
            conflicting_fields: serde_json::from_str(&row.11)?,
            created_at: row.12,
            updated_at: row.13,
        })
    })
    .collect()
}

pub fn resolve_conflict(
    conn: &mut Connection,
    conflict_id: &str,
    resolution: &str,
    now_ms: i64,
) -> AppResult<()> {
    if !matches!(resolution, "keep_local" | "keep_remote") {
        return Err(AppError::Other(format!(
            "unsupported conflict resolution `{resolution}`"
        )));
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let conflict = get_conflict(&tx, conflict_id)?;
    if !conflict.remote_ready {
        return Err(AppError::Other(
            "Remote conflict payload has not been downloaded yet".into(),
        ));
    }
    supersede_conflicting_outbox(&tx, &conflict, "CONFLICT_RESOLVED", now_ms)?;
    match resolution {
        "keep_local" if conflict.local_deleted => {
            apply_local_delete_resolution(&tx, &conflict, now_ms)?;
        }
        "keep_local" => {
            let payload = conflict
                .local_payload
                .clone()
                .ok_or_else(|| AppError::Other("Local conflict payload is missing".into()))?;
            apply_local_resolution(&tx, &conflict, payload, now_ms)?;
        }
        "keep_remote" => apply_remote_resolution(&tx, &conflict)?,
        _ => unreachable!(),
    }
    mark_conflict_resolved(&tx, conflict_id, resolution, now_ms)?;
    tx.commit()?;
    Ok(())
}

fn apply_local_resolution(
    tx: &Transaction<'_>,
    conflict: &ConflictRecord,
    payload: Value,
    now_ms: i64,
) -> AppResult<()> {
    let (payload, local_version) = normalize_resolution_payload(tx, conflict, payload, now_ms)?;
    let remote_revision = conflict
        .remote_revision
        .ok_or_else(|| AppError::Other("Remote conflict revision is missing".into()))?;
    let entity = RemoteEntityInput {
        protocol_version: 1,
        entity_type: conflict.entity_type.clone(),
        entity_id: conflict.entity_id.clone(),
        revision: remote_revision,
        hlc: conflict.local_hlc.clone(),
        deleted: false,
        payload_schema_version: 1,
        payload_encoding: "json".into(),
        payload: Some(payload.clone()),
        payload_hash: sha256_hex(serde_json::to_string(&payload)?.as_bytes()),
        key_version: None,
        origin_device_id: device_id(tx)?,
        server_seq: conflict.remote_server_seq.unwrap_or(1),
        updated_at: now_ms,
    };
    apply_remote_business_row(tx, &entity)?;
    enqueue_projection(
        tx,
        parse_sync_entity_type(&conflict.entity_type)?,
        &conflict.entity_id,
        local_version,
        false,
        &payload,
    )?;
    Ok(())
}

fn apply_local_delete_resolution(
    tx: &Transaction<'_>,
    conflict: &ConflictRecord,
    now_ms: i64,
) -> AppResult<()> {
    let remote_revision = conflict
        .remote_revision
        .ok_or_else(|| AppError::Other("Remote conflict revision is missing".into()))?;
    let local_version = conflict
        .local_version
        .max(remote_revision)
        .saturating_add(1);
    let entity = RemoteEntityInput {
        protocol_version: 1,
        entity_type: conflict.entity_type.clone(),
        entity_id: conflict.entity_id.clone(),
        revision: local_version,
        hlc: conflict.local_hlc.clone(),
        deleted: true,
        payload_schema_version: 1,
        payload_encoding: "json".into(),
        payload: None,
        payload_hash: sha256_hex(&[]),
        key_version: None,
        origin_device_id: device_id(tx)?,
        server_seq: conflict.remote_server_seq.unwrap_or(1),
        updated_at: now_ms,
    };
    apply_remote_delete(tx, &entity)?;
    enqueue_projection(
        tx,
        parse_sync_entity_type(&conflict.entity_type)?,
        &conflict.entity_id,
        local_version,
        true,
        &Value::Object(Default::default()),
    )?;
    Ok(())
}

fn apply_remote_resolution(tx: &Transaction<'_>, conflict: &ConflictRecord) -> AppResult<()> {
    let entity = RemoteEntityInput {
        protocol_version: 1,
        entity_type: conflict.entity_type.clone(),
        entity_id: conflict.entity_id.clone(),
        revision: conflict
            .remote_revision
            .ok_or_else(|| AppError::Other("Remote conflict revision is missing".into()))?,
        hlc: conflict
            .remote_hlc
            .clone()
            .ok_or_else(|| AppError::Other("Remote conflict HLC is missing".into()))?,
        deleted: conflict.remote_deleted,
        payload_schema_version: 1,
        payload_encoding: "json".into(),
        payload: conflict.remote_payload.clone(),
        payload_hash: conflict
            .remote_payload_hash
            .clone()
            .ok_or_else(|| AppError::Other("Remote conflict payload hash is missing".into()))?,
        key_version: None,
        origin_device_id: conflict
            .remote_origin_device_id
            .clone()
            .ok_or_else(|| AppError::Other("Remote conflict device is missing".into()))?,
        server_seq: conflict
            .remote_server_seq
            .ok_or_else(|| AppError::Other("Remote conflict server sequence is missing".into()))?,
        updated_at: conflict
            .remote_updated_at
            .ok_or_else(|| AppError::Other("Remote conflict timestamp is missing".into()))?,
    };
    apply_remote_business_row(tx, &entity)
}

fn normalize_resolution_payload(
    tx: &Transaction<'_>,
    conflict: &ConflictRecord,
    mut payload: Value,
    now_ms: i64,
) -> AppResult<(Value, i64)> {
    let object = payload
        .as_object_mut()
        .ok_or_else(|| AppError::Other("Resolved payload is not an object".into()))?;
    let remote_version = conflict
        .remote_payload
        .as_ref()
        .and_then(|value| value.get("version"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let local_version = conflict.local_version.max(remote_version).saturating_add(1);
    object.insert("version".into(), local_version.into());
    object.insert("updated_at".into(), (now_ms / 1_000).to_string().into());
    object.insert("origin_device_id".into(), device_id(tx)?.into());
    object.insert("deleted_at".into(), Value::Null);
    Ok((payload, local_version))
}

fn supersede_conflicting_outbox(
    tx: &Transaction<'_>,
    conflict: &ConflictRecord,
    code: &str,
    now_ms: i64,
) -> AppResult<()> {
    tx.execute(
        "UPDATE sync_outbox SET status = 'synced', synced_at = ?1, next_retry_at = NULL, \
         last_error_code = ?2, last_error_message = 'superseded by conflict resolution' \
         WHERE entity_type = ?3 AND entity_id = ?4 \
           AND status IN ('pending', 'in_flight', 'conflict', 'dead_letter')",
        params![now_ms, code, conflict.entity_type, conflict.entity_id],
    )?;
    Ok(())
}

fn mark_conflict_resolved(
    tx: &Transaction<'_>,
    conflict_id: &str,
    resolution: &str,
    now_ms: i64,
) -> AppResult<()> {
    tx.execute(
        "UPDATE sync_conflicts SET status = 'resolved', resolution = ?1, \
         resolved_at = ?2, updated_at = ?2 WHERE id = ?3 AND status = 'pending'",
        params![resolution, now_ms, conflict_id],
    )?;
    Ok(())
}

fn get_conflict(conn: &Connection, conflict_id: &str) -> AppResult<ConflictRecord> {
    let row = conn
        .query_row(
        "SELECT id, entity_type, entity_id, remote_revision, base_payload, local_payload, \
         remote_payload, local_deleted, remote_deleted, remote_ready, local_version, local_hlc, \
         remote_hlc, remote_payload_hash, remote_origin_device_id, remote_server_seq, remote_updated_at \
         FROM sync_conflicts WHERE id = ?1 AND status = 'pending'",
        [conflict_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, bool>(8)?,
                row.get::<_, bool>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<i64>>(15)?,
                row.get::<_, Option<i64>>(16)?,
            ))
        },
    )
        .optional()?
        .ok_or_else(|| {
            AppError::Other(format!(
                "Pending sync conflict `{conflict_id}` was not found"
            ))
        })?;
    Ok(ConflictRecord {
        entity_type: row.1,
        entity_id: row.2,
        remote_revision: row.3,
        base_payload: parse_optional_json(row.4)?,
        local_payload: parse_optional_json(row.5)?,
        remote_payload: parse_optional_json(row.6)?,
        local_deleted: row.7,
        remote_deleted: row.8,
        remote_ready: row.9,
        local_version: row.10,
        local_hlc: row.11,
        remote_hlc: row.12,
        remote_payload_hash: row.13,
        remote_origin_device_id: row.14,
        remote_server_seq: row.15,
        remote_updated_at: row.16,
    })
}

fn parse_optional_json(raw: Option<String>) -> AppResult<Option<Value>> {
    raw.map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

fn parse_sync_entity_type(value: &str) -> AppResult<SyncEntityType> {
    match value {
        "agent" => Ok(SyncEntityType::Agent),
        "workspace" => Ok(SyncEntityType::Workspace),
        "session" => Ok(SyncEntityType::Session),
        "message" => Ok(SyncEntityType::Message),
        "explicit_memory" => Ok(SyncEntityType::ExplicitMemory),
        "memory" => Ok(SyncEntityType::Memory),
        "calendar" => Ok(SyncEntityType::Calendar),
        "calendar_event" => Ok(SyncEntityType::CalendarEvent),
        "event_exception" => Ok(SyncEntityType::EventException),
        "task_list" => Ok(SyncEntityType::TaskList),
        "task" => Ok(SyncEntityType::Task),
        "reading_book" => Ok(SyncEntityType::ReadingBook),
        "reading_highlight" => Ok(SyncEntityType::ReadingHighlight),
        _ => Err(AppError::Other(format!(
            "unsupported conflict entity type `{value}`"
        ))),
    }
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
                source_payload, payload_hash, key_version, attempt_count, created_at \
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
        source_payload: row.get(11)?,
        payload_hash: row.get(12)?,
        key_version: row.get(13)?,
        attempt_count: row.get(14)?,
        created_at: row.get(15)?,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::repo::agents::NewAgent;

    const LOCAL_DEVICE: &str = "00000000-0000-4000-8000-000000000001";
    const REMOTE_DEVICE: &str = "00000000-0000-4000-8000-000000000002";

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) VALUES (1, ?1)",
            [LOCAL_DEVICE],
        )
        .unwrap();
        conn
    }

    fn agent_payload(id: &str, name: &str, version: i64) -> Value {
        json!({
            "id": id,
            "name": name,
            "persona": "persona",
            "scenario": "scenario",
            "system_prompt": "prompt",
            "greeting": "hello",
            "example_dialogue": "example",
            "model": "provider/model",
            "tool_policy": "{}",
            "tags": "test",
            "thinking_mode": "off",
            "thinking_budget": 0,
            "created_at": "1",
            "updated_at": version.to_string(),
            "version": version,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn reading_book_payload(id: &str, model_knows_content: bool, version: i64) -> Value {
        json!({
            "id": id,
            "title": "Jane Eyre",
            "author": "Charlotte Bronte",
            "source_hash": "a".repeat(64),
            "model_knows_content": model_knows_content,
            "content_context_allowed": false,
            "content_context_decided": false,
            "progress_cfi": null,
            "created_at": "1",
            "updated_at": version.to_string(),
            "version": version,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn workspace_payload(id: &str, agent_id: &str) -> Value {
        json!({
            "id": id,
            "agent_id": agent_id,
            "name": "Remote workspace",
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn session_payload(id: &str, agent_id: &str, workspace_id: &str) -> Value {
        json!({
            "id": id,
            "agent_id": agent_id,
            "title": "Remote session",
            "context_limit": null,
            "compress_threshold": 0.85,
            "recency_window": 20,
            "reserved_output_tokens": null,
            "model": null,
            "thinking_mode": null,
            "thinking_budget": null,
            "workspace_id": workspace_id,
            "summary": null,
            "summary_updated_at": null,
            "pinned": 0,
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn message_payload(id: &str, session_id: &str) -> Value {
        json!({
            "id": id,
            "session_id": session_id,
            "role": "assistant",
            "seq": 2,
            "parent_id": null,
            "selected_child_id": null,
            "parts": [{"kind": "text", "content": "Remote answer", "ordinal": 0}],
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn explicit_memory_payload(id: &str, agent_id: &str, content: &str, version: i64) -> Value {
        explicit_memory_payload_for_kind(id, agent_id, "memory_md", content, version)
    }

    fn explicit_memory_payload_for_kind(
        id: &str,
        agent_id: &str,
        kind: &str,
        content: &str,
        version: i64,
    ) -> Value {
        json!({
            "id": id,
            "agent_id": agent_id,
            "kind": kind,
            "content": content,
            "created_at": "1",
            "updated_at": version.to_string(),
            "version": version,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn memory_payload(id: &str, agent_id: &str, content: &str, version: i64) -> Value {
        json!({
            "id": id,
            "agent_id": agent_id,
            "name": "Remote fact",
            "keywords": ["remote", "fact"],
            "content": content,
            "creator": "ai",
            "status": "active",
            "created_at": "1",
            "updated_at": version.to_string(),
            "version": version,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn calendar_payload(id: &str) -> Value {
        json!({
            "id": id,
            "name": "Remote calendar",
            "color": "#0ea5e9",
            "timezone": "Asia/Shanghai",
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn calendar_event_payload(id: &str, calendar_id: &str) -> Value {
        json!({
            "id": id,
            "calendar_id": calendar_id,
            "title": "Remote planning",
            "description": null,
            "location": null,
            "starts_at": "2026-07-20T01:00:00Z",
            "ends_at": "2026-07-20T02:00:00Z",
            "timezone": "Asia/Shanghai",
            "all_day": 0,
            "recurrence_rule": "RRULE:FREQ=DAILY;COUNT=2",
            "recurrence_id": null,
            "status": "confirmed",
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn event_exception_payload(id: &str, event_id: &str, original_occurrence: &str) -> Value {
        json!({
            "id": id,
            "event_id": event_id,
            "original_occurrence": original_occurrence,
            "replacement_event_id": null,
            "is_cancelled": 1,
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn task_list_payload(id: &str) -> Value {
        json!({
            "id": id,
            "name": "Remote tasks",
            "color": "#22c55e",
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn task_payload(id: &str, task_list_id: &str, parent_id: Option<&str>) -> Value {
        json!({
            "id": id,
            "task_list_id": task_list_id,
            "parent_id": parent_id,
            "title": format!("Task {id}"),
            "description": null,
            "status": "open",
            "priority": 2,
            "starts_at": null,
            "due_date": "2026-07-20",
            "due_at": null,
            "due_timezone": "Asia/Shanghai",
            "is_important": 0,
            "my_day_date": null,
            "completed_at": null,
            "recurrence_rule": null,
            "recurrence_anchor": null,
            "recurrence_source_id": null,
            "sort_order": 0.0,
            "created_at": "1",
            "updated_at": "1",
            "version": 1,
            "deleted_at": null,
            "origin_device_id": REMOTE_DEVICE
        })
    }

    fn remote(
        entity_type: &str,
        entity_id: &str,
        revision: i64,
        server_seq: i64,
        payload: Value,
    ) -> RemoteEntityInput {
        RemoteEntityInput {
            protocol_version: 1,
            entity_type: entity_type.into(),
            entity_id: entity_id.into(),
            revision,
            hlc: format!("1000-{server_seq:04}-remote02"),
            deleted: false,
            payload_schema_version: 1,
            payload_encoding: "json".into(),
            payload: Some(payload),
            payload_hash: format!("{:064x}", server_seq),
            key_version: None,
            origin_device_id: REMOTE_DEVICE.into(),
            server_seq,
            updated_at: 1_000 + server_seq,
        }
    }

    #[test]
    fn multi_page_bootstrap_applies_dependencies_and_finishes_at_snapshot_cursor() {
        let mut conn = setup();
        apply_bootstrap_page(
            &mut conn,
            "required",
            &[remote(
                "agent",
                "agent-1",
                1,
                1,
                agent_payload("agent-1", "Agent", 1),
            )],
            3,
            Some("page-one"),
            true,
            2_000,
        )
        .unwrap();
        apply_bootstrap_page(
            &mut conn,
            "cursor:3:page-one",
            &[remote(
                "workspace",
                "workspace-1",
                1,
                2,
                workspace_payload("workspace-1", "agent-1"),
            )],
            3,
            Some("page-two"),
            true,
            2_001,
        )
        .unwrap();
        apply_bootstrap_page(
            &mut conn,
            "cursor:3:page-two",
            &[remote(
                "session",
                "session-1",
                1,
                3,
                session_payload("session-1", "agent-1", "workspace-1"),
            )],
            3,
            None,
            false,
            2_002,
        )
        .unwrap();

        let (state, cursor, workspace_id): (String, i64, Option<String>) = conn
            .query_row(
                "SELECT r.bootstrap_state, r.last_pull_cursor, s.workspace_id \
                 FROM sync_runtime_state r JOIN sessions s ON s.id = 'session-1' \
                 WHERE r.singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "complete");
        assert_eq!(cursor, 3);
        assert_eq!(workspace_id.as_deref(), Some("workspace-1"));
        let binding_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM workspace_bindings", [], |row| {
                row.get(0)
            })
            .unwrap();
        let outbox_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(binding_count, 0);
        assert_eq!(outbox_count, 0);
    }

    #[test]
    fn planner_bootstrap_applies_without_echo_and_reconciles_deferred_task_parents() {
        let mut conn = setup();
        let exception_id = super::super::planner::event_exception_entity_id(
            "calendar-event-1",
            "2026-07-20T01:00:00Z",
        );
        apply_bootstrap_page(
            &mut conn,
            "required",
            &[
                remote(
                    "calendar",
                    "calendar-1",
                    1,
                    1,
                    calendar_payload("calendar-1"),
                ),
                remote(
                    "calendar_event",
                    "calendar-event-1",
                    1,
                    2,
                    calendar_event_payload("calendar-event-1", "calendar-1"),
                ),
                remote(
                    "event_exception",
                    &exception_id,
                    1,
                    3,
                    event_exception_payload(
                        &exception_id,
                        "calendar-event-1",
                        "2026-07-20T01:00:00Z",
                    ),
                ),
                remote(
                    "task_list",
                    "task-list-1",
                    1,
                    4,
                    task_list_payload("task-list-1"),
                ),
                remote(
                    "task",
                    "task-child-1",
                    1,
                    5,
                    task_payload("task-child-1", "task-list-1", Some("task-parent-1")),
                ),
                remote(
                    "task",
                    "task-parent-1",
                    1,
                    6,
                    task_payload("task-parent-1", "task-list-1", None),
                ),
            ],
            6,
            None,
            false,
            2_000,
        )
        .unwrap();

        let state: String = conn
            .query_row(
                "SELECT bootstrap_state FROM sync_runtime_state WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let child_parent: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM tasks WHERE id='task-child-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let outbox_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state, "complete");
        assert_eq!(child_parent.as_deref(), Some("task-parent-1"));
        assert_eq!(outbox_count, 0);

        let deleted_exception = RemoteEntityInput {
            protocol_version: 1,
            entity_type: "event_exception".into(),
            entity_id: exception_id,
            revision: 2,
            hlc: "1000-0007-remote02".into(),
            deleted: true,
            payload_schema_version: 1,
            payload_encoding: "json".into(),
            payload: None,
            payload_hash: sha256_hex(&[]),
            key_version: None,
            origin_device_id: REMOTE_DEVICE.into(),
            server_seq: 7,
            updated_at: 2_007,
        };
        apply_pull_page(&mut conn, 6, &[deleted_exception], 7, false, 2_007).unwrap();
        let deleted: bool = conn
            .query_row(
                "SELECT deleted_at IS NOT NULL FROM event_exceptions \
                 WHERE event_id='calendar-event-1' AND original_occurrence='2026-07-20T01:00:00Z'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(deleted);
    }

    #[test]
    fn malformed_pull_page_rolls_back_business_rows_entity_state_and_cursor() {
        let mut conn = setup();
        apply_bootstrap_page(
            &mut conn,
            "required",
            &[remote(
                "agent",
                "agent-1",
                1,
                1,
                agent_payload("agent-1", "Before", 1),
            )],
            1,
            None,
            false,
            2_000,
        )
        .unwrap();
        let mut malformed_session = session_payload("session-1", "agent-1", "workspace-1");
        malformed_session
            .as_object_mut()
            .unwrap()
            .insert("unknown_field".into(), json!(true));
        let result = apply_pull_page(
            &mut conn,
            1,
            &[
                remote(
                    "agent",
                    "agent-1",
                    2,
                    2,
                    agent_payload("agent-1", "After", 2),
                ),
                remote("session", "session-1", 1, 3, malformed_session),
            ],
            3,
            false,
            3_000,
        );
        assert!(result.is_err());

        let name: String = conn
            .query_row("SELECT name FROM agents WHERE id = 'agent-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let (cursor, revision): (i64, i64) = conn
            .query_row(
                "SELECT r.last_pull_cursor, e.remote_revision FROM sync_runtime_state r \
                 JOIN sync_entity_state e ON e.entity_type = 'agent' AND e.entity_id = 'agent-1' \
                 WHERE r.singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Before");
        assert_eq!(cursor, 1);
        assert_eq!(revision, 1);
    }

    #[test]
    fn remote_change_marks_pending_local_entity_as_conflict_without_overwrite() {
        let mut conn = setup();
        crate::db::repo::agents::insert(
            &mut conn,
            &NewAgent {
                id: "agent-1".into(),
                name: "Local".into(),
                persona: String::new(),
                scenario: String::new(),
                system_prompt: String::new(),
                greeting: String::new(),
                example_dialogue: String::new(),
                model: String::new(),
                tool_policy: "{}".into(),
                avatar: String::new(),
                tags: String::new(),
                thinking_mode: "off".into(),
                thinking_budget: 0,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET bootstrap_state = 'complete' WHERE singleton = 1",
            [],
        )
        .unwrap();
        apply_pull_page(
            &mut conn,
            0,
            &[remote(
                "agent",
                "agent-1",
                1,
                1,
                agent_payload("agent-1", "Remote", 1),
            )],
            1,
            false,
            2_000,
        )
        .unwrap();

        let name: String = conn
            .query_row("SELECT name FROM agents WHERE id = 'agent-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let outbox_status: String = conn
            .query_row("SELECT status FROM sync_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "Local");
        assert_eq!(outbox_status, "conflict");
        assert_eq!(status(&conn).unwrap().last_pull_cursor, 1);
    }

    #[test]
    fn sealed_outbox_is_atomic_and_preserves_the_local_source_payload() {
        let mut conn = setup();
        crate::db::repo::agents::insert(
            &mut conn,
            &NewAgent {
                id: "agent-1".into(),
                name: "Local".into(),
                persona: String::new(),
                scenario: String::new(),
                system_prompt: String::new(),
                greeting: String::new(),
                example_dialogue: String::new(),
                model: String::new(),
                tool_policy: "{}".into(),
                avatar: String::new(),
                tags: String::new(),
                thinking_mode: "off".into(),
                thinking_budget: 0,
            },
        )
        .unwrap();
        let claimed = claim_pending(&mut conn, 20, i64::MAX).unwrap();
        let original = claimed[0].source_payload.clone();
        let sealed = SealedOutboxChange {
            change_id: claimed[0].change_id.clone(),
            payload_encoding: "xchacha20poly1305-v1".into(),
            payload: Some(serde_json::to_string("YWJj").unwrap()),
            payload_hash: "b".repeat(64),
            key_version: Some(1),
        };
        let invalid = SealedOutboxChange {
            change_id: "missing-change".into(),
            ..sealed.clone()
        };
        assert!(persist_sealed_outbox(&mut conn, &[sealed.clone(), invalid]).is_err());
        let unchanged = get_outbox(&conn, &sealed.change_id).unwrap().unwrap();
        assert_eq!(unchanged.payload_encoding, "json");
        assert_eq!(unchanged.payload, original);

        let persisted = persist_sealed_outbox(&mut conn, &[sealed]).unwrap();
        assert_eq!(persisted[0].payload_encoding, "xchacha20poly1305-v1");
        assert_eq!(persisted[0].payload.as_deref(), Some("\"YWJj\""));
        assert_eq!(persisted[0].source_payload, original);
        assert_eq!(persisted[0].key_version, Some(1));
    }

    #[test]
    fn pulling_an_already_pushed_local_revision_is_idempotent_and_does_not_echo() {
        let mut conn = setup();
        crate::db::repo::agents::insert(
            &mut conn,
            &NewAgent {
                id: "agent-1".into(),
                name: "Local".into(),
                persona: String::new(),
                scenario: String::new(),
                system_prompt: String::new(),
                greeting: String::new(),
                example_dialogue: String::new(),
                model: String::new(),
                tool_policy: "{}".into(),
                avatar: String::new(),
                tags: String::new(),
                thinking_mode: "off".into(),
                thinking_budget: 0,
            },
        )
        .unwrap();
        let claimed = claim_pending(&mut conn, 20, i64::MAX).unwrap();
        let payload: Value = serde_json::from_str(claimed[0].payload.as_deref().unwrap()).unwrap();
        apply_push_result(
            &mut conn,
            &[AcceptedChange {
                change_id: claimed[0].change_id.clone(),
                server_seq: 1,
                revision: 1,
            }],
            &[],
            2_000,
        )
        .unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET bootstrap_state = 'complete' WHERE singleton = 1",
            [],
        )
        .unwrap();
        let mut own_change = remote("agent", "agent-1", 1, 1, payload);
        own_change.origin_device_id = LOCAL_DEVICE.into();
        own_change.payload_hash = claimed[0].payload_hash.clone();
        own_change.hlc = claimed[0].hlc.clone();
        apply_pull_page(&mut conn, 0, &[own_change], 1, false, 2_001).unwrap();

        let rows: (i64, String) = conn
            .query_row("SELECT COUNT(*), MIN(status) FROM sync_outbox", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(rows, (1, "synced".into()));
        assert_eq!(status(&conn).unwrap().last_pull_cursor, 1);
    }

    #[test]
    fn remote_message_and_memories_apply_without_echo_and_invalidate_stale_vectors() {
        let mut conn = setup();
        let explicit_id =
            crate::db::repo::explicit_memories::sync_entity_id("agent-1", "memory_md").unwrap();
        let entities = vec![
            remote(
                "agent",
                "agent-1",
                1,
                1,
                agent_payload("agent-1", "Agent", 1),
            ),
            remote(
                "workspace",
                "workspace-1",
                1,
                2,
                workspace_payload("workspace-1", "agent-1"),
            ),
            remote(
                "session",
                "session-1",
                1,
                3,
                session_payload("session-1", "agent-1", "workspace-1"),
            ),
            remote(
                "message",
                "message-1",
                1,
                4,
                message_payload("message-1", "session-1"),
            ),
            remote(
                "explicit_memory",
                &explicit_id,
                1,
                5,
                explicit_memory_payload(&explicit_id, "agent-1", "# Remote memory", 1),
            ),
            remote(
                "memory",
                "memory-1",
                1,
                6,
                memory_payload("memory-1", "agent-1", "First remote content", 1),
            ),
        ];
        apply_bootstrap_page(&mut conn, "required", &entities, 6, None, false, 2_000).unwrap();

        let message: (String, String) = conn
            .query_row(
                "SELECT m.status, p.content FROM messages m JOIN message_parts p ON p.message_id = m.id \
                 WHERE m.id = 'message-1' AND p.kind = 'text'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(message, ("complete".into(), "Remote answer".into()));
        let explicit_content: String = conn
            .query_row(
                "SELECT content FROM explicit_memories WHERE agent_id = 'agent-1' AND kind = 'memory_md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(explicit_content, "# Remote memory");
        assert_eq!(status(&conn).unwrap().pending_count, 0);

        conn.execute(
            "INSERT INTO embedding_items (id, ref_type, ref_id, model, dims, content_hash, created_at) \
             VALUES ('embedding-1', 'memory', 'memory-1', 'embed/model', 3, 'old', '1')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE memory_store SET embedding_id = 'embedding-1' WHERE id = 'memory-1'",
            [],
        )
        .unwrap();
        apply_pull_page(
            &mut conn,
            6,
            &[
                remote(
                    "explicit_memory",
                    &explicit_id,
                    2,
                    7,
                    explicit_memory_payload(&explicit_id, "agent-1", "# Updated remote memory", 2),
                ),
                remote(
                    "memory",
                    "memory-1",
                    2,
                    8,
                    memory_payload("memory-1", "agent-1", "Updated remote content", 2),
                ),
            ],
            8,
            false,
            3_000,
        )
        .unwrap();

        let memory: (String, Option<String>) = conn
            .query_row(
                "SELECT content, embedding_id FROM memory_store WHERE id = 'memory-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(memory, ("Updated remote content".into(), None));
        let embedding_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM embedding_items", [], |row| row.get(0))
            .unwrap();
        assert_eq!(embedding_count, 0);
        let explicit_content: String = conn
            .query_row(
                "SELECT content FROM explicit_memories WHERE agent_id = 'agent-1' AND kind = 'memory_md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(explicit_content, "# Updated remote memory");
        assert_eq!(status(&conn).unwrap().last_pull_cursor, 8);
        assert_eq!(status(&conn).unwrap().pending_count, 0);
    }

    fn synced_agent_with_local_model_change() -> (Connection, OutboxRow) {
        let mut conn = setup();
        crate::db::repo::agents::insert(
            &mut conn,
            &NewAgent {
                id: "agent-1".into(),
                name: "Base".into(),
                persona: "persona".into(),
                scenario: "scenario".into(),
                system_prompt: "prompt".into(),
                greeting: "hello".into(),
                example_dialogue: "example".into(),
                model: "provider/model".into(),
                tool_policy: "{}".into(),
                avatar: String::new(),
                tags: "test".into(),
                thinking_mode: "off".into(),
                thinking_budget: 0,
            },
        )
        .unwrap();
        let initial = claim_pending(&mut conn, 20, i64::MAX).unwrap();
        apply_push_result(
            &mut conn,
            &[AcceptedChange {
                change_id: initial[0].change_id.clone(),
                server_seq: 1,
                revision: 1,
            }],
            &[],
            1_000,
        )
        .unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET bootstrap_state = 'complete' WHERE singleton = 1",
            [],
        )
        .unwrap();
        crate::db::repo::agents::update_model(&mut conn, "agent-1", "local/model").unwrap();
        let local = claim_pending(&mut conn, 20, i64::MAX).unwrap();
        (conn, local[0].clone())
    }

    #[test]
    fn push_conflict_waits_for_pull_then_preserves_all_three_payloads() {
        let (mut conn, local) = synced_agent_with_local_model_change();
        apply_push_result(
            &mut conn,
            &[],
            &[ConflictChange {
                change_id: local.change_id,
                current_revision: Some(2),
            }],
            2_000,
        )
        .unwrap();

        let state_revision: i64 = conn
            .query_row(
                "SELECT remote_revision FROM sync_entity_state \
                 WHERE entity_type = 'agent' AND entity_id = 'agent-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state_revision, 1);
        let waiting = list_conflicts(&conn).unwrap();
        assert_eq!(waiting.len(), 1);
        assert!(!waiting[0].remote_ready);

        let mut remote_payload = agent_payload("agent-1", "Base", 2);
        remote_payload["model"] = json!("remote/model");
        apply_pull_page(
            &mut conn,
            0,
            &[remote("agent", "agent-1", 2, 2, remote_payload)],
            2,
            false,
            3_000,
        )
        .unwrap();

        let conflicts = list_conflicts(&conn).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].remote_ready);
        assert_eq!(
            conflicts[0].base_payload.as_ref().unwrap()["model"],
            "provider/model"
        );
        assert_eq!(
            conflicts[0].local_payload.as_ref().unwrap()["model"],
            "local/model"
        );
        assert_eq!(
            conflicts[0].remote_payload.as_ref().unwrap()["model"],
            "remote/model"
        );
        assert_eq!(conflicts[0].conflicting_fields, vec!["model"]);

        resolve_conflict(&mut conn, &conflicts[0].id, "keep_local", 4_000).unwrap();
        assert!(list_conflicts(&conn).unwrap().is_empty());
        let model: String = conn
            .query_row("SELECT model FROM agents WHERE id = 'agent-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(model, "local/model");
        let next: (String, Option<i64>) = conn
            .query_row(
                "SELECT status, base_revision FROM sync_outbox \
                 WHERE entity_type = 'agent' AND entity_id = 'agent-1' \
                 ORDER BY rowid DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(next, ("pending".into(), Some(2)));
    }

    #[test]
    fn push_conflict_hydrates_when_pull_matches_cached_revision() {
        let mut conn = setup();
        conn.execute_batch(crate::db::schema::KNOWLEDGE_SCHEMA)
            .unwrap();
        conn.execute_batch(crate::db::schema::READING_SCHEMA)
            .unwrap();
        let remote_payload = reading_book_payload("book-1", false, 1);
        apply_bootstrap_page(
            &mut conn,
            "required",
            &[remote(
                "reading_book",
                "book-1",
                1,
                1,
                remote_payload.clone(),
            )],
            1,
            None,
            false,
            1_000,
        )
        .unwrap();
        crate::db::repo::reading::update_book_mode(&mut conn, "book-1", true).unwrap();
        let local = claim_pending(&mut conn, 20, i64::MAX).unwrap();
        apply_push_result(
            &mut conn,
            &[],
            &[ConflictChange {
                change_id: local[0].change_id.clone(),
                current_revision: Some(1),
            }],
            2_000,
        )
        .unwrap();

        let payload_hash: String = conn
            .query_row(
                "SELECT last_payload_hash FROM sync_entity_state \
                 WHERE entity_type = 'reading_book' AND entity_id = 'book-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut remote_entity = remote("reading_book", "book-1", 1, 2, remote_payload);
        remote_entity.payload_hash = payload_hash;
        apply_pull_page(&mut conn, 1, &[remote_entity], 2, false, 3_000).unwrap();

        let conflicts = list_conflicts(&conn).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert!(conflicts[0].remote_ready);
        assert_eq!(
            conflicts[0].remote_payload.as_ref().unwrap()["model_knows_content"],
            false
        );
        assert_eq!(
            conflicts[0].local_payload.as_ref().unwrap()["model_knows_content"],
            true
        );
    }

    #[test]
    fn non_overlapping_agent_fields_auto_merge_against_remote_revision() {
        let (mut conn, local) = synced_agent_with_local_model_change();
        apply_push_result(
            &mut conn,
            &[],
            &[ConflictChange {
                change_id: local.change_id,
                current_revision: Some(2),
            }],
            2_000,
        )
        .unwrap();
        apply_pull_page(
            &mut conn,
            0,
            &[remote(
                "agent",
                "agent-1",
                2,
                2,
                agent_payload("agent-1", "Remote name", 2),
            )],
            2,
            false,
            3_000,
        )
        .unwrap();

        assert!(list_conflicts(&conn).unwrap().is_empty());
        let merged: (String, String, i64) = conn
            .query_row(
                "SELECT name, model, version FROM agents WHERE id = 'agent-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(merged, ("Remote name".into(), "local/model".into(), 3));
        let pending_base: i64 = conn
            .query_row(
                "SELECT base_revision FROM sync_outbox WHERE status = 'pending' \
                 ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_base, 2);
    }

    #[test]
    fn session_same_field_conflict_uses_hlc_and_records_auto_merge_event() {
        let mut conn = setup();
        let mut base_session = session_payload("session-1", "agent-1", "workspace-1");
        base_session["title"] = json!("Base title");
        apply_bootstrap_page(
            &mut conn,
            "required",
            &[
                remote(
                    "agent",
                    "agent-1",
                    1,
                    1,
                    agent_payload("agent-1", "Agent", 1),
                ),
                remote(
                    "workspace",
                    "workspace-1",
                    1,
                    2,
                    workspace_payload("workspace-1", "agent-1"),
                ),
                remote("session", "session-1", 1, 3, base_session),
            ],
            3,
            None,
            false,
            2_000,
        )
        .unwrap();
        crate::db::repo::sessions::update_title(&mut conn, "session-1", "Local title").unwrap();
        let local = claim_pending(&mut conn, 20, i64::MAX).unwrap();
        assert_eq!(local.len(), 1);

        let mut remote_session = session_payload("session-1", "agent-1", "workspace-1");
        remote_session["title"] = json!("Remote title");
        apply_pull_page(
            &mut conn,
            3,
            &[remote("session", "session-1", 2, 4, remote_session)],
            4,
            false,
            3_000,
        )
        .unwrap();

        assert!(list_conflicts(&conn).unwrap().is_empty());
        let session: (String, i64) = conn
            .query_row(
                "SELECT title, version FROM sessions WHERE id = 'session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(session, ("Local title".into(), 3));
        let event: (String, String, String) = conn
            .query_row(
                "SELECT status, resolution, conflicting_fields FROM sync_conflicts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            event,
            ("resolved".into(), "auto_merge".into(), "[\"title\"]".into())
        );
        let pending_base: i64 = conn
            .query_row(
                "SELECT base_revision FROM sync_outbox WHERE status = 'pending' \
                 ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_base, 2);
    }

    #[test]
    fn explicit_memory_diff3_auto_merges_and_requeues_against_remote_revision() {
        let mut conn = setup();
        let user_id =
            crate::db::repo::explicit_memories::sync_entity_id("agent-1", "user_md").unwrap();
        let memory_id =
            crate::db::repo::explicit_memories::sync_entity_id("agent-1", "memory_md").unwrap();
        let base = "# Profile\n\nLanguage: C++\n\n# Preferences\n\nTheme: light\n";
        let local = "# Profile\n\nLanguage: C++ and Rust\n\n# Preferences\n\nTheme: light\n";
        let remote_content = "# Profile\n\nLanguage: C++\n\n# Preferences\n\nTheme: dark\n";
        apply_bootstrap_page(
            &mut conn,
            "required",
            &[
                remote(
                    "agent",
                    "agent-1",
                    1,
                    1,
                    agent_payload("agent-1", "Agent", 1),
                ),
                remote(
                    "explicit_memory",
                    &user_id,
                    1,
                    2,
                    explicit_memory_payload_for_kind(&user_id, "agent-1", "user_md", "# User\n", 1),
                ),
                remote(
                    "explicit_memory",
                    &memory_id,
                    1,
                    3,
                    explicit_memory_payload(&memory_id, "agent-1", base, 1),
                ),
            ],
            3,
            None,
            false,
            2_000,
        )
        .unwrap();
        crate::db::repo::explicit_memories::save_pair(
            &mut conn,
            "agent-1",
            "unused-user-id",
            "# User\n",
            "unused-memory-id",
            local,
        )
        .unwrap();
        assert_eq!(claim_pending(&mut conn, 20, i64::MAX).unwrap().len(), 1);

        apply_pull_page(
            &mut conn,
            3,
            &[remote(
                "explicit_memory",
                &memory_id,
                2,
                4,
                explicit_memory_payload(&memory_id, "agent-1", remote_content, 2),
            )],
            4,
            false,
            3_000,
        )
        .unwrap();

        assert!(list_conflicts(&conn).unwrap().is_empty());
        let content: String = conn
            .query_row(
                "SELECT content FROM explicit_memories \
                 WHERE agent_id = 'agent-1' AND kind = 'memory_md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            content,
            "# Profile\n\nLanguage: C++ and Rust\n\n# Preferences\n\nTheme: dark\n"
        );
        let event: (String, String, String) = conn
            .query_row(
                "SELECT status, resolution, conflicting_fields FROM sync_conflicts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            event,
            (
                "resolved".into(),
                "auto_merge".into(),
                "[\"content\"]".into()
            )
        );
        let pending_base: i64 = conn
            .query_row(
                "SELECT base_revision FROM sync_outbox WHERE status = 'pending' \
                 ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_base, 2);
    }

    #[test]
    fn accepting_remote_conflict_replaces_local_row_without_sync_echo() {
        let (mut conn, local) = synced_agent_with_local_model_change();
        apply_push_result(
            &mut conn,
            &[],
            &[ConflictChange {
                change_id: local.change_id,
                current_revision: Some(2),
            }],
            2_000,
        )
        .unwrap();
        let mut remote_payload = agent_payload("agent-1", "Remote", 2);
        remote_payload["model"] = json!("remote/model");
        apply_pull_page(
            &mut conn,
            0,
            &[remote("agent", "agent-1", 2, 2, remote_payload)],
            2,
            false,
            3_000,
        )
        .unwrap();
        let conflict_id = list_conflicts(&conn).unwrap()[0].id.clone();

        resolve_conflict(&mut conn, &conflict_id, "keep_remote", 4_000).unwrap();

        let row: (String, String, i64) = conn
            .query_row(
                "SELECT name, model, version FROM agents WHERE id = 'agent-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("Remote".into(), "remote/model".into(), 2));
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_outbox WHERE status = 'pending'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
        assert!(list_conflicts(&conn).unwrap().is_empty());
    }

    #[test]
    fn object_cursor_advances_independently_and_rejects_stale_pages() {
        let conn = setup();
        assert_eq!(status(&conn).unwrap().last_pull_cursor, 0);
        assert_eq!(status(&conn).unwrap().last_object_cursor, 0);

        assert_eq!(advance_object_cursor(&conn, 0, 4).unwrap(), 4);
        assert_eq!(advance_object_cursor(&conn, 0, 4).unwrap(), 4);
        assert!(advance_object_cursor(&conn, 0, 5).is_err());
        assert!(advance_object_cursor(&conn, 4, 3).is_err());

        let status = status(&conn).unwrap();
        assert_eq!(status.last_object_cursor, 4);
        assert_eq!(status.last_pull_cursor, 0);
    }
}
