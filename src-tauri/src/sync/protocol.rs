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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectChangesResponse {
    #[serde(default)]
    pub changes: Vec<ObjectChange>,
    pub next_cursor: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectChange {
    pub server_seq: i64,
    pub object_id: String,
    pub artifact_id: Option<String>,
    pub operation: String,
    pub logical_version: i64,
    pub changed_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectManifestResponse {
    pub manifest: RemoteObjectManifest,
    #[serde(default)]
    pub replicas: Vec<RemoteObjectReplica>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteObjectManifest {
    pub object_id: String,
    pub object_kind: String,
    pub logical_version: i64,
    pub artifact_id: String,
    pub ciphertext_hash: String,
    pub size: u64,
    pub key_version: i64,
    pub updated_hlc: String,
    pub deleted_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteObjectReplica {
    pub provider_kind: String,
    pub provider_account_id: String,
    pub provider_revision: Option<String>,
    pub etag: Option<String>,
    pub ciphertext_hash: String,
    pub size: u64,
    pub status: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectLocalStatus {
    Missing,
    Downloading,
    Verifying,
    Installed,
    Failed,
    Incompatible,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectStateRequest {
    pub protocol_version: u8,
    pub device_id: String,
    pub object_id: String,
    pub observed_logical_version: i64,
    pub installed_artifact_id: Option<String>,
    pub local_status: ObjectLocalStatus,
    pub verified_ciphertext_hash: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectStateResponse {
    pub status: String,
    pub object_id: String,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePairingSessionRequest {
    pub protocol_version: u8,
    pub session_id: String,
    pub initiator_message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreatePairingSessionResponse {
    pub session_id: String,
    pub expires_at: i64,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicPairingSessionResponse {
    pub protocol_version: u8,
    pub session_id: String,
    pub initiator_message: String,
    pub expires_at: i64,
    pub server_time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinPairingSessionRequest {
    pub protocol_version: u8,
    pub device_id: String,
    pub device_name: String,
    pub platform: Option<String>,
    pub responder_message: String,
    pub responder_proof: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingStatusResponse {
    pub status: String,
    pub expires_at: Option<i64>,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingJoinResponse {
    pub device_id: String,
    pub device_name: String,
    pub platform: Option<String>,
    pub responder_message: String,
    pub responder_proof: String,
    pub expires_at: i64,
    pub server_time: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizePairingSessionRequest {
    pub protocol_version: u8,
    pub device_id: String,
    pub credential_fingerprint: String,
    pub transfer_bundle: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinalizePairingSessionResponse {
    pub status: String,
    pub device_id: String,
    pub server_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingPackageResponse {
    pub status: String,
    pub transfer_bundle: Option<String>,
    pub expires_at: i64,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_control_plane_contract_matches_worker_payloads() {
        let changes: ObjectChangesResponse = serde_json::from_value(serde_json::json!({
            "changes": [{
                "serverSeq": 7,
                "objectId": "knowledge:collection-1",
                "artifactId": "artifact-1",
                "operation": "upsert",
                "logicalVersion": 2,
                "changedAt": 1234
            }],
            "nextCursor": 7,
            "hasMore": false
        }))
        .unwrap();
        assert_eq!(changes.next_cursor, 7);
        assert_eq!(changes.changes[0].logical_version, 2);

        let response: ObjectManifestResponse = serde_json::from_value(serde_json::json!({
            "manifest": {
                "objectId": "knowledge:collection-1",
                "objectKind": "knowledge_index",
                "logicalVersion": 2,
                "artifactId": "artifact-1",
                "ciphertextHash": "a".repeat(64),
                "size": 1024,
                "keyVersion": 1,
                "updatedHlc": "1-0000-device01",
                "deletedAt": null,
                "updatedAt": 1234
            },
            "replicas": [{
                "providerKind": "r2",
                "providerAccountId": "r2",
                "providerRevision": "revision-1",
                "etag": "etag-1",
                "ciphertextHash": "a".repeat(64),
                "size": 1024,
                "status": "ready",
                "updatedAt": 1234
            }]
        }))
        .unwrap();
        assert_eq!(response.manifest.artifact_id, "artifact-1");
        assert_eq!(response.replicas[0].provider_kind, "r2");

        let state = ObjectStateRequest {
            protocol_version: PROTOCOL_VERSION,
            device_id: "00000000-0000-4000-8000-000000000001".into(),
            object_id: "knowledge:collection-1".into(),
            observed_logical_version: 2,
            installed_artifact_id: Some("artifact-1".into()),
            local_status: ObjectLocalStatus::Installed,
            verified_ciphertext_hash: Some("a".repeat(64)),
            error_code: None,
        };
        let state = serde_json::to_value(state).unwrap();
        assert_eq!(state["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(state["localStatus"], "installed");
        assert_eq!(state["installedArtifactId"], "artifact-1");
    }
}
