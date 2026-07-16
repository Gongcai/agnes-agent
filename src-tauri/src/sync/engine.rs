use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::db::repo::sync::{AcceptedChange, ConflictChange, RemoteEntityInput, SyncDbStatus};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::secrets::{SecretStore, SYNC_CREDENTIAL_SECRET_ID};
use crate::sync::auth::SyncCredential;
use crate::sync::client::{FailureKind, HttpSyncTransport, SyncTransport, TransportFailure};
use crate::sync::protocol::{
    AckRequest, DeviceListResponse, PushRequest, PushResponse, RemoteChange, RemoteEntity,
    RevokeDeviceResponse, SyncDevice, DEFAULT_PAGE_LIMIT, PROTOCOL_VERSION,
};

pub const SYNC_GATEWAY_URL: &str = "https://agnes-sync-api.caiwengong136.workers.dev";
const MAX_BATCHES_PER_RUN: usize = 5;
const MAX_PUSH_CHANGES: usize = 20;
const MAX_REMOTE_PAGES_PER_RUN: usize = 5;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: String,
    pub gateway_url: String,
    pub credential_configured: bool,
    pub syncing: bool,
    #[serde(flatten)]
    pub database: SyncDbStatus,
}

pub struct SyncService {
    db: DbActorHandle,
    secrets: Arc<dyn SecretStore>,
    client: reqwest::Client,
    run_gate: Mutex<()>,
    syncing: AtomicBool,
    debounce_scheduled: AtomicBool,
}

impl SyncService {
    pub fn new(db: DbActorHandle, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| AppError::Other(format!("build sync HTTP client: {error}")))?;
        Ok(Self {
            db,
            secrets,
            client,
            run_gate: Mutex::new(()),
            syncing: AtomicBool::new(false),
            debounce_scheduled: AtomicBool::new(false),
        })
    }

    pub async fn status(&self) -> AppResult<SyncStatus> {
        let database = self.db.get_sync_status().await?;
        let credential_configured = self.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await?.is_some();
        let syncing = self.syncing.load(Ordering::SeqCst);
        let state = if syncing {
            "syncing"
        } else if !credential_configured {
            "auth_required"
        } else if database.conflict_count > 0 {
            "conflict"
        } else if database.dead_letter_count > 0 || database.last_error_code.is_some() {
            "error"
        } else if database.pending_count > 0
            || database.in_flight_count > 0
            || database.bootstrap_state != "complete"
        {
            "pending"
        } else {
            "idle"
        };
        Ok(SyncStatus {
            state: state.into(),
            gateway_url: SYNC_GATEWAY_URL.into(),
            credential_configured,
            syncing,
            database,
        })
    }

    pub async fn run_once(&self) -> AppResult<SyncStatus> {
        let Some(transport) = self.http_transport().await? else {
            return self.status().await;
        };
        self.run_with_transport(&transport).await
    }

    pub async fn list_devices(&self) -> AppResult<Vec<SyncDevice>> {
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("Sync credential is not configured".into()))?;
        let current_device_id = self.db.get_sync_status().await?.device_id;
        let response = transport
            .list_devices()
            .await
            .map_err(admin_transport_error)?;
        validate_device_list(response, &current_device_id)
    }

    pub async fn revoke_device(&self, target_device_id: &str) -> AppResult<SyncDevice> {
        let target_device_id = uuid::Uuid::parse_str(target_device_id)
            .map_err(|_| AppError::Other("Device id is invalid".into()))?
            .to_string();
        let current_device_id = self.db.get_sync_status().await?.device_id;
        if target_device_id == current_device_id {
            return Err(AppError::Other(
                "The current device cannot revoke itself".into(),
            ));
        }
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("Sync credential is not configured".into()))?;
        let response = transport
            .revoke_device(&target_device_id)
            .await
            .map_err(admin_transport_error)?;
        validate_revoked_device(response, &target_device_id)
    }

    async fn http_transport(&self) -> AppResult<Option<HttpSyncTransport>> {
        let Some(secret) = self.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await? else {
            return Ok(None);
        };
        let credential = SyncCredential::parse(&secret)?;
        Ok(Some(HttpSyncTransport::new(
            self.client.clone(),
            SYNC_GATEWAY_URL,
            credential,
        )))
    }

    pub fn schedule(self: &Arc<Self>) {
        if self.debounce_scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let service = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(750)).await;
            service.debounce_scheduled.store(false, Ordering::SeqCst);
            if let Err(error) = service.run_once().await {
                eprintln!("[sync] debounced run failed: {error}");
            }
        });
    }

    pub fn start_background(self: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                if let Err(error) = self.run_once().await {
                    eprintln!("[sync] background run failed: {error}");
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    pub(crate) async fn run_with_transport(
        &self,
        transport: &dyn SyncTransport,
    ) -> AppResult<SyncStatus> {
        let Ok(_guard) = self.run_gate.try_lock() else {
            return self.status().await;
        };
        self.syncing.store(true, Ordering::SeqCst);
        let result = self.run_locked(transport).await;
        self.syncing.store(false, Ordering::SeqCst);
        result?;
        self.status().await
    }

    async fn run_locked(&self, transport: &dyn SyncTransport) -> AppResult<()> {
        let initial_status = self.db.get_sync_status().await?;
        if initial_status
            .backoff_until
            .is_some_and(|value| value > unix_millis())
        {
            return Ok(());
        }
        if !self.run_bootstrap(transport).await? {
            return Ok(());
        }
        if !self.push_pending(transport).await? {
            return Ok(());
        }
        self.pull_remote(transport).await?;
        Ok(())
    }

    async fn run_bootstrap(&self, transport: &dyn SyncTransport) -> AppResult<bool> {
        for _ in 0..MAX_REMOTE_PAGES_PER_RUN {
            let status = self.db.get_sync_status().await?;
            if status.bootstrap_state == "complete" {
                return Ok(true);
            }
            let (expected_snapshot_cursor, cursor) = if status.bootstrap_state == "required" {
                (None, None)
            } else {
                let value = status
                    .bootstrap_state
                    .strip_prefix("cursor:")
                    .and_then(|value| value.split_once(':'))
                    .ok_or_else(|| {
                        AppError::Other(format!(
                            "invalid local bootstrap state `{}`",
                            status.bootstrap_state
                        ))
                    })?;
                let snapshot_cursor = value.0.parse::<i64>().map_err(|_| {
                    AppError::Other(format!(
                        "invalid local bootstrap state `{}`",
                        status.bootstrap_state
                    ))
                })?;
                (Some(snapshot_cursor), Some(value.1.to_string()))
            };
            let response = match transport
                .bootstrap(cursor.as_deref(), DEFAULT_PAGE_LIMIT)
                .await
            {
                Ok(response) => response,
                Err(failure) => {
                    self.record_runtime_failure(&failure).await?;
                    return Ok(false);
                }
            };
            if expected_snapshot_cursor.is_some_and(|expected| expected != response.snapshot_cursor)
            {
                return Err(AppError::Other(format!(
                    "bootstrap snapshot cursor changed from {} to {}",
                    expected_snapshot_cursor.unwrap_or_default(),
                    response.snapshot_cursor
                )));
            }
            let snapshot_cursor = response.snapshot_cursor;
            let has_more = response.has_more;
            let entities = response
                .entities
                .into_iter()
                .map(remote_entity_from_bootstrap)
                .collect::<AppResult<Vec<_>>>()?;
            self.db
                .apply_sync_bootstrap_page(
                    status.bootstrap_state,
                    entities,
                    snapshot_cursor,
                    response.next_cursor,
                    has_more,
                    unix_millis(),
                )
                .await?;
            if !has_more {
                return self.ack_cursor(transport, snapshot_cursor).await;
            }
        }
        Ok(false)
    }

    async fn push_pending(&self, transport: &dyn SyncTransport) -> AppResult<bool> {
        for _ in 0..MAX_BATCHES_PER_RUN {
            let now_ms = unix_millis();
            let batch = self.db.claim_sync_outbox(MAX_PUSH_CHANGES, now_ms).await?;
            if batch.is_empty() {
                break;
            }
            let request = match PushRequest::from_outbox(&batch) {
                Ok(request) => request,
                Err(error) => {
                    self.db
                        .mark_sync_dead_letter(
                            batch.iter().map(|row| row.change_id.clone()).collect(),
                            "INVALID_LOCAL_PAYLOAD".into(),
                            error.to_string(),
                        )
                        .await?;
                    continue;
                }
            };
            match transport.push(&request).await {
                Ok(response) => self.apply_response(&batch, response).await?,
                Err(failure) => {
                    self.apply_failure(&batch, failure).await?;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    async fn pull_remote(&self, transport: &dyn SyncTransport) -> AppResult<()> {
        for _ in 0..MAX_REMOTE_PAGES_PER_RUN {
            let after = self.db.get_sync_status().await?.last_pull_cursor;
            let response = match transport.pull(after, DEFAULT_PAGE_LIMIT).await {
                Ok(response) => response,
                Err(failure) => {
                    self.record_runtime_failure(&failure).await?;
                    return Ok(());
                }
            };
            let next_cursor = response.next_cursor;
            let has_more = response.has_more;
            let entities = response
                .changes
                .into_iter()
                .map(remote_entity_from_change)
                .collect::<AppResult<Vec<_>>>()?;
            self.db
                .apply_sync_pull_page(after, entities, next_cursor, has_more, unix_millis())
                .await?;
            if !self.ack_cursor(transport, next_cursor).await? || !has_more {
                break;
            }
        }
        Ok(())
    }

    async fn ack_cursor(&self, transport: &dyn SyncTransport, cursor: i64) -> AppResult<bool> {
        let device_id = self.db.get_sync_status().await?.device_id;
        match transport.ack(&AckRequest::new(device_id, cursor)).await {
            Ok(response) if response.acknowledged_cursor >= cursor => {
                self.db.record_sync_runtime_success(unix_millis()).await?;
                Ok(true)
            }
            Ok(response) => {
                self.db
                    .record_sync_runtime_failure(
                        "INVALID_RESPONSE".into(),
                        Some(unix_millis() + 5_000),
                    )
                    .await?;
                Err(AppError::Other(format!(
                    "acknowledged cursor {} is behind local cursor {cursor}",
                    response.acknowledged_cursor
                )))
            }
            Err(failure) => {
                self.record_runtime_failure(&failure).await?;
                Ok(false)
            }
        }
    }

    async fn record_runtime_failure(&self, failure: &TransportFailure) -> AppResult<()> {
        let delay = failure
            .retry_after_ms
            .unwrap_or_else(|| match failure.kind {
                FailureKind::Permanent => 300_000,
                FailureKind::Authentication => 60_000,
                FailureKind::Retryable => retry_delay_ms(1),
            });
        self.db
            .record_sync_runtime_failure(
                failure.code.clone(),
                Some(unix_millis().saturating_add(delay)),
            )
            .await
    }

    async fn apply_response(
        &self,
        batch: &[crate::db::repo::sync::OutboxRow],
        response: PushResponse,
    ) -> AppResult<()> {
        let expected = batch
            .iter()
            .map(|row| row.change_id.as_str())
            .collect::<HashSet<_>>();
        let received = response
            .accepted
            .iter()
            .map(|item| item.change_id.as_str())
            .chain(
                response
                    .conflicts
                    .iter()
                    .map(|item| item.change_id.as_str()),
            )
            .collect::<Vec<_>>();
        let unique = received.iter().copied().collect::<HashSet<_>>();
        if unique != expected || unique.len() != received.len() {
            self.db
                .schedule_sync_retry(
                    batch.iter().map(|row| row.change_id.clone()).collect(),
                    "INVALID_RESPONSE".into(),
                    "push response did not account for every change exactly once".into(),
                    unix_millis().saturating_add(5_000),
                )
                .await?;
            return Ok(());
        }
        let accepted = response
            .accepted
            .into_iter()
            .map(|item| AcceptedChange {
                change_id: item.change_id,
                server_seq: item.server_seq,
                revision: item.revision,
            })
            .collect();
        let conflicts = response
            .conflicts
            .into_iter()
            .map(|item| ConflictChange {
                change_id: item.change_id,
                current_revision: item.current_revision,
            })
            .collect();
        self.db
            .apply_sync_push_result(accepted, conflicts, unix_millis())
            .await
    }

    async fn apply_failure(
        &self,
        batch: &[crate::db::repo::sync::OutboxRow],
        failure: TransportFailure,
    ) -> AppResult<()> {
        let change_ids = batch.iter().map(|row| row.change_id.clone()).collect();
        match failure.kind {
            FailureKind::Permanent => {
                self.db
                    .mark_sync_dead_letter(change_ids, failure.code, failure.message)
                    .await
            }
            FailureKind::Authentication | FailureKind::Retryable => {
                let attempt = batch
                    .iter()
                    .map(|row| row.attempt_count.saturating_add(1))
                    .max()
                    .unwrap_or(1);
                let delay = failure
                    .retry_after_ms
                    .unwrap_or_else(|| retry_delay_ms(attempt));
                self.db
                    .schedule_sync_retry(
                        change_ids,
                        failure.code,
                        failure.message,
                        unix_millis().saturating_add(delay),
                    )
                    .await
            }
        }
    }
}

fn validate_device_list(
    response: DeviceListResponse,
    current_device_id: &str,
) -> AppResult<Vec<SyncDevice>> {
    if response.server_time < 0 || response.devices.is_empty() {
        return Err(AppError::Other("Invalid device list response".into()));
    }
    let mut ids = HashSet::new();
    let mut current_count = 0;
    for device in &response.devices {
        if uuid::Uuid::parse_str(&device.id).is_err()
            || !ids.insert(device.id.as_str())
            || device.name.trim().is_empty()
            || device.created_at < 0
            || device.last_seen_at.is_some_and(|value| value < 0)
            || device.revoked_at.is_some_and(|value| value < 0)
            || device.last_ack_cursor < 0
            || (device.current && device.id != current_device_id)
        {
            return Err(AppError::Other("Invalid device list response".into()));
        }
        if device.current {
            current_count += 1;
        }
    }
    if current_count != 1 {
        return Err(AppError::Other(
            "Device list did not identify the current device".into(),
        ));
    }
    Ok(response.devices)
}

fn validate_revoked_device(
    response: RevokeDeviceResponse,
    target_device_id: &str,
) -> AppResult<SyncDevice> {
    if response.server_time < 0
        || response.device.id != target_device_id
        || response.device.current
        || response.device.revoked_at.is_none()
        || response.device.created_at < 0
        || response.device.last_seen_at.is_some_and(|value| value < 0)
        || response.device.revoked_at.is_some_and(|value| value < 0)
        || response.device.last_ack_cursor < 0
    {
        return Err(AppError::Other("Invalid revoke device response".into()));
    }
    Ok(response.device)
}

fn admin_transport_error(error: TransportFailure) -> AppError {
    AppError::Other(format!("{}: {}", error.code, error.message))
}

fn remote_entity_from_change(change: RemoteChange) -> AppResult<RemoteEntityInput> {
    if change.protocol_version != PROTOCOL_VERSION
        || uuid::Uuid::parse_str(&change.change_id).is_err()
        || change.created_at < 0
        || change.accepted_at < 0
        || change.resulting_revision <= 0
        || change
            .base_revision
            .map_or(change.resulting_revision != 1, |base| {
                base <= 0 || base.saturating_add(1) != change.resulting_revision
            })
    {
        return Err(AppError::Other(format!(
            "invalid remote change `{}`",
            change.change_id
        )));
    }
    let deleted = match change.operation.as_str() {
        "upsert" => false,
        "delete" => true,
        operation => {
            return Err(AppError::Other(format!(
                "unsupported remote operation `{operation}`"
            )));
        }
    };
    Ok(RemoteEntityInput {
        protocol_version: change.protocol_version.into(),
        entity_type: change.entity_type,
        entity_id: change.entity_id,
        revision: change.resulting_revision,
        hlc: change.hlc,
        deleted,
        payload_schema_version: change.payload_schema_version,
        payload_encoding: change.payload_encoding,
        payload: change.payload,
        payload_hash: change.payload_hash,
        key_version: change.key_version,
        origin_device_id: change.device_id,
        server_seq: change.server_seq,
        updated_at: change.accepted_at,
    })
}

fn remote_entity_from_bootstrap(entity: RemoteEntity) -> AppResult<RemoteEntityInput> {
    if entity.latest_server_seq <= 0 || entity.updated_at < 0 {
        return Err(AppError::Other(format!(
            "invalid bootstrap entity {} `{}`",
            entity.entity_type, entity.entity_id
        )));
    }
    Ok(RemoteEntityInput {
        protocol_version: PROTOCOL_VERSION.into(),
        entity_type: entity.entity_type,
        entity_id: entity.entity_id,
        revision: entity.revision,
        hlc: entity.hlc,
        deleted: entity.deleted,
        payload_schema_version: entity.payload_schema_version,
        payload_encoding: entity.payload_encoding,
        payload: entity.payload,
        payload_hash: entity.payload_hash,
        key_version: entity.key_version,
        origin_device_id: entity.changed_by_device_id,
        server_seq: entity.latest_server_seq,
        updated_at: entity.updated_at,
    })
}

fn retry_delay_ms(attempt: i64) -> i64 {
    let exponent = attempt.saturating_sub(1).clamp(0, 6) as u32;
    let base = 5_000_i64.saturating_mul(2_i64.pow(exponent)).min(300_000);
    base.saturating_add(rand::thread_rng().gen_range(0..=1_000))
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::db::repo::agents::NewAgent;
    use crate::secrets::InMemorySecretStore;
    use crate::sync::client::TransportFailure;
    use crate::sync::protocol::{
        AcceptedChange as ProtocolAccepted, AckResponse, BootstrapResponse, PullResponse,
        PushResponse, RemoteEntity,
    };

    const DEVICE_A: &str = "00000000-0000-4000-8000-000000000001";
    const DEVICE_B: &str = "00000000-0000-4000-8000-000000000002";

    fn device(id: &str, current: bool) -> SyncDevice {
        SyncDevice {
            id: id.into(),
            name: if current { "Desktop" } else { "Phone" }.into(),
            platform: Some(if current { "linux" } else { "android" }.into()),
            created_at: 1,
            last_seen_at: Some(2),
            revoked_at: None,
            last_ack_cursor: 3,
            current,
        }
    }

    #[test]
    fn device_responses_require_one_unique_matching_current_device() {
        let valid = DeviceListResponse {
            devices: vec![device(DEVICE_A, true), device(DEVICE_B, false)],
            server_time: 4,
        };
        assert_eq!(validate_device_list(valid, DEVICE_A).unwrap().len(), 2);

        let duplicate = DeviceListResponse {
            devices: vec![device(DEVICE_A, true), device(DEVICE_A, false)],
            server_time: 4,
        };
        assert!(validate_device_list(duplicate, DEVICE_A).is_err());
        let wrong_current = DeviceListResponse {
            devices: vec![device(DEVICE_B, true)],
            server_time: 4,
        };
        assert!(validate_device_list(wrong_current, DEVICE_A).is_err());
    }

    #[test]
    fn revoke_response_must_match_a_non_current_revoked_target() {
        let mut target = device(DEVICE_B, false);
        target.revoked_at = Some(5);
        let response = RevokeDeviceResponse {
            device: target,
            server_time: 6,
        };
        assert_eq!(
            validate_revoked_device(response, DEVICE_B).unwrap().id,
            DEVICE_B
        );

        let invalid = RevokeDeviceResponse {
            device: device(DEVICE_B, false),
            server_time: 6,
        };
        assert!(validate_revoked_device(invalid, DEVICE_B).is_err());
    }

    struct AcceptTransport {
        calls: AtomicUsize,
        delay_ms: u64,
    }

    #[async_trait]
    impl SyncTransport for AcceptTransport {
        async fn push(&self, request: &PushRequest) -> Result<PushResponse, TransportFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            Ok(PushResponse {
                accepted: request
                    .changes
                    .iter()
                    .enumerate()
                    .map(|(index, change)| ProtocolAccepted {
                        change_id: change.change_id.clone(),
                        server_seq: index as i64 + 1,
                        revision: change.base_revision.unwrap_or(0) + 1,
                        idempotent: false,
                    })
                    .collect(),
                conflicts: Vec::new(),
                server_time: 1,
            })
        }

        async fn pull(&self, after: i64, _limit: usize) -> Result<PullResponse, TransportFailure> {
            Ok(PullResponse {
                changes: Vec::new(),
                next_cursor: after,
                has_more: false,
                server_time: 1,
            })
        }

        async fn bootstrap(
            &self,
            _cursor: Option<&str>,
            _limit: usize,
        ) -> Result<BootstrapResponse, TransportFailure> {
            Ok(BootstrapResponse {
                entities: Vec::new(),
                snapshot_cursor: 0,
                next_cursor: None,
                has_more: false,
                server_time: 1,
            })
        }

        async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure> {
            Ok(AckResponse {
                acknowledged_cursor: request.cursor,
                server_time: 1,
            })
        }
    }

    struct RetryTransport;

    #[async_trait]
    impl SyncTransport for RetryTransport {
        async fn push(&self, _request: &PushRequest) -> Result<PushResponse, TransportFailure> {
            Err(TransportFailure {
                kind: FailureKind::Retryable,
                code: "SYNC_TEMPORARILY_UNAVAILABLE".into(),
                message: "temporary outage".into(),
                retry_after_ms: Some(30_000),
            })
        }

        async fn pull(&self, after: i64, _limit: usize) -> Result<PullResponse, TransportFailure> {
            Ok(PullResponse {
                changes: Vec::new(),
                next_cursor: after,
                has_more: false,
                server_time: 1,
            })
        }

        async fn bootstrap(
            &self,
            _cursor: Option<&str>,
            _limit: usize,
        ) -> Result<BootstrapResponse, TransportFailure> {
            Ok(BootstrapResponse {
                entities: Vec::new(),
                snapshot_cursor: 0,
                next_cursor: None,
                has_more: false,
                server_time: 1,
            })
        }

        async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure> {
            Ok(AckResponse {
                acknowledged_cursor: request.cursor,
                server_time: 1,
            })
        }
    }

    struct BootstrapCommitTransport {
        path: PathBuf,
        acknowledgements: AtomicUsize,
    }

    #[async_trait]
    impl SyncTransport for BootstrapCommitTransport {
        async fn push(&self, _request: &PushRequest) -> Result<PushResponse, TransportFailure> {
            panic!("bootstrap-only test must not push")
        }

        async fn pull(&self, after: i64, _limit: usize) -> Result<PullResponse, TransportFailure> {
            Ok(PullResponse {
                changes: Vec::new(),
                next_cursor: after,
                has_more: false,
                server_time: 1,
            })
        }

        async fn bootstrap(
            &self,
            cursor: Option<&str>,
            _limit: usize,
        ) -> Result<BootstrapResponse, TransportFailure> {
            assert!(cursor.is_none());
            Ok(BootstrapResponse {
                entities: vec![RemoteEntity {
                    entity_type: "agent".into(),
                    entity_id: "remote-agent".into(),
                    revision: 1,
                    hlc: "1000-0001-remote02".into(),
                    deleted: false,
                    payload_schema_version: 1,
                    payload_encoding: "json".into(),
                    payload: Some(json!({
                        "id": "remote-agent",
                        "name": "Remote agent",
                        "persona": "",
                        "scenario": "",
                        "system_prompt": "",
                        "greeting": "",
                        "example_dialogue": "",
                        "model": "",
                        "tool_policy": "{}",
                        "tags": "",
                        "thinking_mode": "off",
                        "thinking_budget": 0,
                        "created_at": "1",
                        "updated_at": "1",
                        "version": 1,
                        "deleted_at": null,
                        "origin_device_id": "00000000-0000-4000-8000-000000000002"
                    })),
                    payload_hash: "a".repeat(64),
                    key_version: None,
                    changed_by_device_id: "00000000-0000-4000-8000-000000000002".into(),
                    latest_server_seq: 1,
                    updated_at: 1_000,
                }],
                snapshot_cursor: 1,
                next_cursor: None,
                has_more: false,
                server_time: 1,
            })
        }

        async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure> {
            let conn = rusqlite::Connection::open(&self.path).unwrap();
            let committed: (String, i64, i64) = conn
                .query_row(
                    "SELECT r.bootstrap_state, r.last_pull_cursor, \
                            (SELECT COUNT(*) FROM agents WHERE id = 'remote-agent') \
                     FROM sync_runtime_state r WHERE r.singleton = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(committed, ("complete".into(), 1, 1));
            self.acknowledgements.fetch_add(1, Ordering::SeqCst);
            Ok(AckResponse {
                acknowledged_cursor: request.cursor,
                server_time: 1,
            })
        }
    }

    fn new_agent(id: &str) -> NewAgent {
        NewAgent {
            id: id.into(),
            name: "Sync test".into(),
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
        }
    }

    fn test_service(label: &str) -> (std::path::PathBuf, Arc<SyncService>) {
        let path = std::env::temp_dir().join(format!(
            "agnes-sync-engine-{label}-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = crate::db::spawn_db_actor(path.clone());
        let secrets = Arc::new(InMemorySecretStore::default());
        let service = Arc::new(SyncService::new(db, secrets).unwrap());
        (path, service)
    }

    #[tokio::test]
    async fn accepted_push_advances_entity_state_and_clears_pending() {
        let (path, service) = test_service("accepted");
        service
            .db
            .insert_agent(new_agent("sync-agent"))
            .await
            .unwrap();
        let transport = AcceptTransport {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
        };

        service.run_with_transport(&transport).await.unwrap();

        let status = service.db.get_sync_status().await.unwrap();
        assert_eq!(status.pending_count, 0);
        assert_eq!(status.in_flight_count, 0);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        let conn = rusqlite::Connection::open(&path).unwrap();
        let remote_revision: i64 = conn
            .query_row(
                "SELECT remote_revision FROM sync_entity_state \
                 WHERE entity_type = 'agent' AND entity_id = 'sync-agent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remote_revision, 1);
        drop(conn);
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn retryable_failure_restores_pending_with_backoff() {
        let (path, service) = test_service("retry");
        service
            .db
            .insert_agent(new_agent("retry-agent"))
            .await
            .unwrap();

        service.run_with_transport(&RetryTransport).await.unwrap();

        let status = service.db.get_sync_status().await.unwrap();
        assert_eq!(status.pending_count, 1);
        assert_eq!(status.in_flight_count, 0);
        assert_eq!(
            status.last_error_code.as_deref(),
            Some("SYNC_TEMPORARILY_UNAVAILABLE")
        );
        assert!(status.backoff_until.is_some());

        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE sync_outbox SET next_retry_at = 0 WHERE entity_id = 'retry-agent'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET backoff_until = NULL WHERE singleton = 1",
            [],
        )
        .unwrap();
        drop(conn);
        let recovered = AcceptTransport {
            calls: AtomicUsize::new(0),
            delay_ms: 0,
        };
        service.run_with_transport(&recovered).await.unwrap();
        assert_eq!(service.db.get_sync_status().await.unwrap().pending_count, 0);
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn concurrent_runs_share_one_single_flight() {
        let (path, service) = test_service("single-flight");
        service
            .db
            .insert_agent(new_agent("single-flight-agent"))
            .await
            .unwrap();
        let transport = AcceptTransport {
            calls: AtomicUsize::new(0),
            delay_ms: 50,
        };

        let (first, second) = tokio::join!(
            service.run_with_transport(&transport),
            service.run_with_transport(&transport)
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(service.db.get_sync_status().await.unwrap().pending_count, 0);
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn bootstrap_ack_happens_only_after_the_page_transaction_commits() {
        let (path, service) = test_service("bootstrap-commit-before-ack");
        service.db.get_sync_status().await.unwrap();
        let transport = BootstrapCommitTransport {
            path: path.clone(),
            acknowledgements: AtomicUsize::new(0),
        };

        service.run_with_transport(&transport).await.unwrap();

        assert_eq!(transport.acknowledgements.load(Ordering::SeqCst), 2);
        assert_eq!(
            service.db.get_sync_status().await.unwrap().last_pull_cursor,
            1
        );
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    #[ignore = "requires a temporary remote bearer identity"]
    async fn remote_bearer_push_accepts_a_fake_agent() {
        let token = std::env::var("AGNES_SYNC_E2E_TOKEN")
            .expect("AGNES_SYNC_E2E_TOKEN must contain the temporary bearer token");
        let device_id = std::env::var("AGNES_SYNC_E2E_DEVICE_ID")
            .expect("AGNES_SYNC_E2E_DEVICE_ID must match the remote identity mapping");
        uuid::Uuid::parse_str(&device_id).expect("the E2E device id must be a UUID");

        let path = std::env::temp_dir().join(format!(
            "agnes-sync-engine-remote-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = crate::db::spawn_db_actor(path.clone());
        db.get_sync_status().await.unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET device_id = ?1 WHERE singleton = 1",
            [&device_id],
        )
        .unwrap();
        drop(conn);

        let secrets = Arc::new(InMemorySecretStore::default());
        secrets
            .set(
                SYNC_CREDENTIAL_SECRET_ID,
                &SyncCredential::Bearer { token }.into_secret().unwrap(),
            )
            .await
            .unwrap();
        let service = Arc::new(SyncService::new(db, secrets).unwrap());
        let agent_id = format!("e2e-{}", uuid::Uuid::new_v4());
        service.db.insert_agent(new_agent(&agent_id)).await.unwrap();

        let status = service.run_once().await.unwrap();

        assert_eq!(status.database.pending_count, 0);
        assert_eq!(status.database.in_flight_count, 0);
        assert_eq!(status.database.conflict_count, 0);
        assert_eq!(status.database.dead_letter_count, 0);
        let conn = rusqlite::Connection::open(&path).unwrap();
        let remote_revision: i64 = conn
            .query_row(
                "SELECT remote_revision FROM sync_entity_state \
                 WHERE entity_type = 'agent' AND entity_id = ?1",
                [&agent_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remote_revision, 1);
        drop(conn);
        drop(service);
        let _ = std::fs::remove_file(path);
    }
}
