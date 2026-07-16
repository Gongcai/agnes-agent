use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::repo::sync::OutboxRow;
use crate::error::{AppError, AppResult};

pub const PROTOCOL_VERSION: u8 = 1;
pub const DEFAULT_PAGE_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    pub protocol_version: u8,
    pub device_id: String,
    pub changes: Vec<PushChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushChange {
    pub change_id: String,
    pub device_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub operation: String,
    pub base_revision: Option<i64>,
    pub hlc: String,
    pub payload_schema_version: i64,
    pub payload_encoding: String,
    pub payload: Option<Value>,
    pub payload_hash: String,
    pub key_version: Option<i64>,
    pub created_at: i64,
}

impl PushRequest {
    pub fn from_outbox(rows: &[OutboxRow]) -> AppResult<Self> {
        let device_id = rows
            .first()
            .map(|row| row.device_id.clone())
            .ok_or_else(|| AppError::Other("cannot build an empty sync push".into()))?;
        let mut changes = Vec::with_capacity(rows.len());
        for row in rows {
            if row.device_id != device_id {
                return Err(AppError::Other(
                    "sync batch contains more than one device id".into(),
                ));
            }
            let payload = row
                .payload
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?;
            changes.push(PushChange {
                change_id: row.change_id.clone(),
                device_id: row.device_id.clone(),
                entity_type: row.entity_type.clone(),
                entity_id: row.entity_id.clone(),
                operation: row.operation.clone(),
                base_revision: row.base_revision,
                hlc: row.hlc.clone(),
                payload_schema_version: row.payload_schema_version,
                payload_encoding: row.payload_encoding.clone(),
                payload,
                payload_hash: row.payload_hash.clone(),
                key_version: row.key_version,
                created_at: row.created_at,
            });
        }
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            device_id,
            changes,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
    #[serde(default)]
    pub accepted: Vec<AcceptedChange>,
    #[serde(default)]
    pub conflicts: Vec<ConflictChange>,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedChange {
    pub change_id: String,
    pub server_seq: i64,
    pub revision: i64,
    #[serde(default)]
    pub idempotent: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictChange {
    pub change_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub current_revision: Option<i64>,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PullResponse {
    #[serde(default)]
    pub changes: Vec<RemoteChange>,
    pub next_cursor: i64,
    pub has_more: bool,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteChange {
    pub protocol_version: u8,
    pub server_seq: i64,
    pub change_id: String,
    pub device_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub operation: String,
    pub base_revision: Option<i64>,
    pub resulting_revision: i64,
    pub hlc: String,
    pub payload_schema_version: i64,
    pub payload_encoding: String,
    pub payload: Option<Value>,
    pub payload_hash: String,
    pub key_version: Option<i64>,
    pub created_at: i64,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootstrapResponse {
    #[serde(default)]
    pub entities: Vec<RemoteEntity>,
    pub snapshot_cursor: i64,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteEntity {
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
    pub changed_by_device_id: String,
    pub latest_server_seq: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckRequest {
    pub protocol_version: u8,
    pub device_id: String,
    pub cursor: i64,
}

impl AckRequest {
    pub fn new(device_id: String, cursor: i64) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            device_id,
            cursor,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AckResponse {
    pub acknowledged_cursor: i64,
    pub server_time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDevice {
    pub id: String,
    pub name: String,
    pub platform: Option<String>,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_ack_cursor: i64,
    pub current: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceListResponse {
    #[serde(default)]
    pub devices: Vec<SyncDevice>,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeDeviceResponse {
    pub device: SyncDevice,
    pub server_time: i64,
}

#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}
