use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Deserialize;
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
            "agent" | "workspace" | "session"
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
    } else {
        apply_remote_business_row(tx, entity)?;
    }
    upsert_remote_state(tx, entity, now_ms)?;
    Ok(())
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
    tx.execute(
        "INSERT INTO sessions (id, agent_id, title, context_limit, compress_threshold, recency_window, \
         reserved_output_tokens, summarizer_model, model, thinking_mode, thinking_budget, \
         permission_mode, workspace_id, summary, summary_updated_at, created_at, updated_at, \
         version, deleted_at, origin_device_id, pinned) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, 'auto', ?11, ?12, ?13, \
                 ?14, ?15, ?16, NULL, ?17, ?18) \
         ON CONFLICT(id) DO UPDATE SET agent_id = excluded.agent_id, title = excluded.title, \
         context_limit = excluded.context_limit, compress_threshold = excluded.compress_threshold, \
         recency_window = excluded.recency_window, reserved_output_tokens = excluded.reserved_output_tokens, \
         model = excluded.model, thinking_mode = excluded.thinking_mode, \
         thinking_budget = excluded.thinking_budget, workspace_id = excluded.workspace_id, \
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
}
