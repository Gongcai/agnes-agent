use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use serde::Serialize;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use crate::db::repo::sync::{
    AcceptedChange, ConflictChange, RemoteEntityInput, SealedOutboxChange, SyncDbStatus,
};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::secrets::{SecretStore, SYNC_CREDENTIAL_SECRET_ID, SYNC_E2EE_KEYSET_SECRET_ID};
use crate::storage::StorageService;
use crate::sync::artifact::{verify_artifact, ArtifactManifest, BuiltArtifact};
use crate::sync::auth::SyncCredential;
use crate::sync::client::{
    FailureKind, HttpSyncTransport, ObjectSyncTransport, SyncTransport, TransportFailure,
};
use crate::sync::crypto::{
    open_json, seal_json, PayloadMetadata, RecoveryMaterial, SyncKeyset, EMPTY_PAYLOAD_HASH,
    PAYLOAD_ENCODING, TOMBSTONE_ENCODING,
};
use crate::sync::hlc::HybridTimestamp;
use crate::sync::knowledge_artifact::{
    build_fingerprint as knowledge_build_fingerprint, build_from_snapshot, cache_built_artifact,
    decode_verified_artifact, KNOWLEDGE_ARTIFACT_TYPE,
};
use crate::sync::pairing::{
    self, PairingDevice, PairingExchange, PairingKey, PreparedPairingTransfer,
};
use crate::sync::protocol::{
    AckRequest, CreatePairingSessionRequest, DeviceListResponse, FinalizePairingSessionRequest,
    JoinPairingSessionRequest, ObjectLocalStatus, ObjectManifestResponse, ObjectStateRequest,
    PairingJoinResponse, PushRequest, PushResponse, RemoteChange, RemoteEntity,
    RevokeDeviceResponse, SyncDevice, DEFAULT_PAGE_LIMIT, PROTOCOL_VERSION,
};
use crate::sync::replication::{
    ArtifactInstaller, ArtifactReplicationCoordinator, ReplicationReport,
};

pub const SYNC_GATEWAY_URL: &str = "https://agnes-sync-api.caiwengong136.workers.dev";
const MAX_BATCHES_PER_RUN: usize = 5;
const MAX_PUSH_CHANGES: usize = 20;
const MAX_REMOTE_PAGES_PER_RUN: usize = 5;
const E2EE_TRANSPORT_READY: bool = true;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncE2eeStatus {
    pub keyset_configured: bool,
    pub confirmed: bool,
    pub active_key_version: Option<i64>,
    pub confirmed_key_version: Option<i64>,
    pub rotation_pending: bool,
    pub transport_ready: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: String,
    pub gateway_url: String,
    pub credential_configured: bool,
    pub syncing: bool,
    pub e2ee: SyncE2eeStatus,
    #[serde(flatten)]
    pub database: SyncDbStatus,
}

#[derive(Debug)]
pub struct PreparedKnowledgeArtifactPublication {
    pub artifact: Option<BuiltArtifact>,
    pub manifest: ArtifactManifest,
    pub object_id: String,
    pub logical_version: u64,
    pub updated_hlc: String,
    pub device_id: String,
    pub local_path: Option<String>,
    pub reused_ready_replica: bool,
}

#[derive(Debug)]
pub struct KnowledgeArtifactPublicationOutcome {
    pub artifact_id: String,
    pub reused: bool,
    pub ready_replica_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairingDevice {
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub platform: Option<String>,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingJoinStarted {
    pub session_id: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCompletion {
    pub status: String,
    pub sync_status: Option<SyncStatus>,
}

enum PendingInitiatorPairing {
    Exchange {
        exchange: PairingExchange,
        expires_at: i64,
    },
    Prepared {
        transfer: PreparedPairingTransfer,
        device: PairingDevice,
        expires_at: i64,
    },
}

struct PendingResponderPairing {
    key: PairingKey,
    device: PairingDevice,
    join_request: JoinPairingSessionRequest,
    expires_at: i64,
}

pub struct SyncService {
    db: DbActorHandle,
    secrets: Arc<dyn SecretStore>,
    client: reqwest::Client,
    run_gate: Mutex<()>,
    artifact_publish_gate: Mutex<()>,
    pairing_initiators: Mutex<HashMap<String, PendingInitiatorPairing>>,
    pairing_responders: Mutex<HashMap<String, PendingResponderPairing>>,
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
            artifact_publish_gate: Mutex::new(()),
            pairing_initiators: Mutex::new(HashMap::new()),
            pairing_responders: Mutex::new(HashMap::new()),
            syncing: AtomicBool::new(false),
            debounce_scheduled: AtomicBool::new(false),
        })
    }

    pub async fn status(&self) -> AppResult<SyncStatus> {
        self.cleanup_expired_pairings(unix_millis()).await;
        let database = self.db.get_sync_status().await?;
        let credential_configured = match self.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await? {
            Some(secret) => {
                drop(Zeroizing::new(secret));
                true
            }
            None => false,
        };
        let keyset = self.load_keyset().await?;
        let e2ee = validate_e2ee_status(keyset.as_ref(), database.e2ee_key_version)?;
        let syncing = self.syncing.load(Ordering::SeqCst);
        let state = if syncing {
            "syncing"
        } else if !credential_configured {
            "auth_required"
        } else if e2ee.rotation_pending {
            "e2ee_pending"
        } else if !e2ee.confirmed {
            "e2ee_required"
        } else if !e2ee.transport_ready {
            "e2ee_pending"
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
            e2ee,
            database,
        })
    }

    pub async fn run_once(&self) -> AppResult<SyncStatus> {
        let status = self.status().await?;
        if !status.e2ee.confirmed || !status.e2ee.transport_ready {
            return Ok(status);
        }
        let keyset = self
            .load_keyset()
            .await?
            .ok_or_else(|| AppError::Other("Sync encryption keys are not configured".into()))?;
        let Some(transport) = self.http_transport().await? else {
            return Ok(status);
        };
        self.run_with_encrypted_transport(&transport, &keyset).await
    }

    pub async fn publish_knowledge_artifact(
        &self,
        storage: Arc<StorageService>,
        source_version_id: String,
        cache_root: std::path::PathBuf,
    ) -> AppResult<KnowledgeArtifactPublicationOutcome> {
        let _guard = self.artifact_publish_gate.lock().await;
        let prepared = self
            .prepare_knowledge_artifact_publication(source_version_id, cache_root)
            .await?;
        let PreparedKnowledgeArtifactPublication {
            artifact,
            manifest,
            object_id,
            logical_version,
            updated_hlc,
            device_id,
            local_path,
            reused_ready_replica,
        } = prepared;
        if let Some(artifact) = artifact {
            storage
                .upload_artifact(
                    crate::storage::MANAGED_R2_ACCOUNT_ID.into(),
                    artifact,
                    object_id.clone(),
                    logical_version,
                    updated_hlc,
                )
                .await?;
        }
        let observed_version = i64::try_from(logical_version)
            .map_err(|_| AppError::Other("知识文档逻辑版本无效".into()))?;
        let checked_at = current_unix_seconds_string();
        self.db
            .upsert_artifact_manifest(crate::db::repo::artifacts::UpsertArtifactManifest {
                manifest: manifest.clone(),
                local_path,
                local_status: "installed".into(),
                installed_at: Some(checked_at.clone()),
            })
            .await?;
        self.db
            .upsert_device_artifact_state(crate::db::repo::artifacts::DeviceArtifactStateRow {
                device_id,
                artifact_id: manifest.id.clone(),
                observed_version,
                local_status: "installed".into(),
                verified_hash: Some(manifest.ciphertext_hash.clone()),
                last_checked_at: checked_at,
                last_error_code: None,
            })
            .await?;
        self.report_installed_object(
            object_id,
            observed_version,
            manifest.id.clone(),
            manifest.ciphertext_hash.clone(),
        )
        .await?;
        let ready_replica_count = self
            .db
            .list_artifact_replicas(manifest.id.clone())
            .await?
            .into_iter()
            .filter(|replica| replica.status == "ready")
            .count();
        Ok(KnowledgeArtifactPublicationOutcome {
            artifact_id: manifest.id,
            reused: reused_ready_replica,
            ready_replica_count,
        })
    }

    async fn prepare_knowledge_artifact_publication(
        &self,
        source_version_id: String,
        cache_root: std::path::PathBuf,
    ) -> AppResult<PreparedKnowledgeArtifactPublication> {
        let status = self.status().await?;
        if !status.credential_configured {
            return Err(AppError::Other("同步凭证尚未配置".into()));
        }
        if !status.e2ee.confirmed || !status.e2ee.transport_ready {
            return Err(AppError::Other("同步端到端加密尚未就绪".into()));
        }
        let keyset = self
            .load_keyset()
            .await?
            .ok_or_else(|| AppError::Other("同步加密密钥尚未配置".into()))?;
        let snapshot = self
            .db
            .export_knowledge_artifact_snapshot(source_version_id.clone())
            .await?;
        let object_id = format!("knowledge:{}", snapshot.document_id);
        if object_id.len() > 160
            || !object_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-'))
        {
            return Err(AppError::Other("知识制品对象 ID 无效".into()));
        }
        let logical_version = u64::try_from(snapshot.logical_version)
            .map_err(|_| AppError::Other("知识文档逻辑版本无效".into()))?;
        let fingerprint = knowledge_build_fingerprint(&snapshot)?;
        let existing = self
            .db
            .find_artifact_manifest_by_fingerprint(
                KNOWLEDGE_ARTIFACT_TYPE.into(),
                source_version_id,
                fingerprint,
            )
            .await?;
        if let Some(existing) = existing {
            let manifest = existing.to_manifest()?;
            let ready = self
                .db
                .list_artifact_replicas(existing.id.clone())
                .await?
                .into_iter()
                .any(|replica| replica.status == "ready");
            if ready {
                return Ok(PreparedKnowledgeArtifactPublication {
                    updated_hlc: knowledge_publish_hlc(&manifest, &status.database.device_id)?,
                    artifact: None,
                    manifest,
                    object_id,
                    logical_version,
                    device_id: status.database.device_id,
                    local_path: existing.local_path,
                    reused_ready_replica: true,
                });
            }
            if let Some(local_path) = existing.local_path.as_deref() {
                let path = std::path::Path::new(local_path);
                if path.is_file() {
                    let key = keyset.key(existing.key_version).ok_or_else(|| {
                        AppError::Other("缓存知识制品所需的历史密钥不可用".into())
                    })?;
                    let bytes = tokio::fs::read(path).await?;
                    let verified = verify_artifact(key, &manifest, &bytes)?;
                    let decoded = decode_verified_artifact(&verified)?;
                    if decoded.source_version_id != snapshot.source_version_id
                        || knowledge_build_fingerprint(&decoded)? != manifest.build_fingerprint
                    {
                        return Err(AppError::Other("缓存知识制品与当前索引不匹配".into()));
                    }
                    return Ok(PreparedKnowledgeArtifactPublication {
                        updated_hlc: knowledge_publish_hlc(&manifest, &status.database.device_id)?,
                        artifact: Some(BuiltArtifact {
                            manifest: manifest.clone(),
                            bytes,
                        }),
                        manifest,
                        object_id,
                        logical_version,
                        device_id: status.database.device_id,
                        local_path: Some(local_path.into()),
                        reused_ready_replica: false,
                    });
                }
            }
            return Err(AppError::Other("现有知识制品缺少可重试的密文缓存".into()));
        }

        let artifact =
            build_from_snapshot(keyset.active_key(), keyset.active_key_version(), &snapshot)?;
        let artifact_for_cache = artifact.clone();
        let cache_path = tokio::task::spawn_blocking(move || {
            cache_built_artifact(&cache_root, &artifact_for_cache)
        })
        .await
        .map_err(|error| AppError::Other(format!("知识制品缓存任务异常中止：{error}")))??;
        let local_path = cache_path.to_string_lossy().to_string();
        self.db
            .upsert_artifact_manifest(crate::db::repo::artifacts::UpsertArtifactManifest {
                manifest: artifact.manifest.clone(),
                local_path: Some(local_path.clone()),
                local_status: "built".into(),
                installed_at: None,
            })
            .await?;
        Ok(PreparedKnowledgeArtifactPublication {
            updated_hlc: knowledge_publish_hlc(&artifact.manifest, &status.database.device_id)?,
            manifest: artifact.manifest.clone(),
            artifact: Some(artifact),
            object_id,
            logical_version,
            device_id: status.database.device_id,
            local_path: Some(local_path),
            reused_ready_replica: false,
        })
    }

    pub async fn get_object_manifest(&self, object_id: &str) -> AppResult<ObjectManifestResponse> {
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("同步凭证尚未配置".into()))?;
        transport
            .get_object_manifest(object_id)
            .await
            .map_err(admin_transport_error)
    }

    pub async fn report_installed_object(
        &self,
        object_id: String,
        logical_version: i64,
        artifact_id: String,
        ciphertext_hash: String,
    ) -> AppResult<()> {
        let status = self.db.get_sync_status().await?;
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("同步凭证尚未配置".into()))?;
        transport
            .update_object_state(&ObjectStateRequest {
                protocol_version: PROTOCOL_VERSION,
                device_id: status.device_id,
                object_id,
                observed_logical_version: logical_version,
                installed_artifact_id: Some(artifact_id),
                local_status: ObjectLocalStatus::Installed,
                verified_ciphertext_hash: Some(ciphertext_hash),
                error_code: None,
            })
            .await
            .map_err(admin_transport_error)?;
        Ok(())
    }

    pub async fn begin_e2ee_setup(&self) -> AppResult<RecoveryMaterial> {
        let _guard = self.run_gate.lock().await;
        let keyset = match self.load_keyset().await? {
            Some(keyset) => keyset,
            None => {
                let keyset = SyncKeyset::generate_initial();
                self.persist_new_keyset(&keyset).await?;
                keyset
            }
        };
        keyset.create_recovery_material()
    }

    pub async fn begin_e2ee_rotation(&self) -> AppResult<RecoveryMaterial> {
        let _guard = self.run_gate.lock().await;
        let confirmed_version = self
            .db
            .get_sync_status()
            .await?
            .e2ee_key_version
            .ok_or_else(|| AppError::Other("Sync encryption keys are not confirmed".into()))?;
        let mut keyset = self
            .load_keyset()
            .await?
            .ok_or_else(|| AppError::Other("Sync encryption keys are not configured".into()))?;
        if keyset.active_key_version() != confirmed_version {
            if keyset.active_key_version() > confirmed_version
                && keyset.key(confirmed_version).is_some()
            {
                return keyset.create_recovery_material();
            }
            return Err(AppError::Other(
                "Local sync encryption state cannot be rotated safely".into(),
            ));
        }

        let previous = Zeroizing::new(keyset.serialize()?);
        keyset.rotate()?;
        let material = keyset.create_recovery_material()?;
        let replacement = Zeroizing::new(keyset.serialize()?);
        self.replace_secret_verified(
            SYNC_E2EE_KEYSET_SECRET_ID,
            Some(replacement.as_str()),
            Some(previous.as_str()),
        )
        .await?;
        Ok(material)
    }

    pub async fn confirm_e2ee_setup(&self) -> AppResult<SyncStatus> {
        let _guard = self.run_gate.lock().await;
        let keyset = self
            .load_keyset()
            .await?
            .ok_or_else(|| AppError::Other("Sync encryption keys are not configured".into()))?;
        self.db
            .set_sync_e2ee_key_version(Some(keyset.active_key_version()))
            .await?;
        drop(_guard);
        self.status().await
    }

    pub async fn restore_e2ee(
        &self,
        recovery_key: &str,
        recovery_bundle: &str,
    ) -> AppResult<SyncStatus> {
        let _guard = self.run_gate.lock().await;
        let recovered = SyncKeyset::recover(recovery_key, recovery_bundle)?;
        let previous = self
            .secrets
            .get(SYNC_E2EE_KEYSET_SECRET_ID)
            .await?
            .map(Zeroizing::new);
        let changed_keyset = match previous.as_ref() {
            Some(raw) => {
                let existing = SyncKeyset::parse(raw.as_str())?;
                if recovered.is_same_keyset(&existing) {
                    false
                } else if recovered.is_compatible_upgrade_from(&existing) {
                    let replacement = Zeroizing::new(recovered.serialize()?);
                    self.replace_secret_verified(
                        SYNC_E2EE_KEYSET_SECRET_ID,
                        Some(replacement.as_str()),
                        Some(raw.as_str()),
                    )
                    .await?;
                    true
                } else {
                    return Err(AppError::Other(
                        "Recovery material is not a monotonic upgrade of the configured keyset"
                            .into(),
                    ));
                }
            }
            None => {
                self.persist_new_keyset(&recovered).await?;
                true
            }
        };
        if let Err(error) = self
            .db
            .set_sync_e2ee_key_version(Some(recovered.active_key_version()))
            .await
        {
            if changed_keyset {
                let rollback = self
                    .restore_secret_value(
                        SYNC_E2EE_KEYSET_SECRET_ID,
                        previous.as_deref().map(String::as_str),
                    )
                    .await
                    .err();
                return Err(AppError::SecretStore(format!(
                    "restore local E2EE state failed: {error}{}",
                    rollback
                        .map(|rollback| format!("; keyring rollback failed: {rollback}"))
                        .unwrap_or_default()
                )));
            }
            return Err(error);
        }
        drop(_guard);
        self.status().await
    }

    pub async fn discard_e2ee_setup(&self) -> AppResult<SyncStatus> {
        let _guard = self.run_gate.lock().await;
        if self.db.get_sync_status().await?.e2ee_key_version.is_some() {
            return Err(AppError::Other(
                "Confirmed sync encryption keys cannot be discarded".into(),
            ));
        }
        let Some(previous) = self.secrets.get(SYNC_E2EE_KEYSET_SECRET_ID).await? else {
            drop(_guard);
            return self.status().await;
        };
        let previous = Zeroizing::new(previous);
        self.secrets.delete(SYNC_E2EE_KEYSET_SECRET_ID).await?;
        let verification_error = match self.secrets.get(SYNC_E2EE_KEYSET_SECRET_ID).await {
            Ok(None) => None,
            Ok(Some(stored)) => {
                drop(Zeroizing::new(stored));
                Some("deleted keyset was still readable".to_string())
            }
            Err(error) => Some(error.to_string()),
        };
        if let Some(verification_error) = verification_error {
            let rollback = self
                .secrets
                .set(SYNC_E2EE_KEYSET_SECRET_ID, previous.as_str())
                .await
                .err();
            return Err(AppError::SecretStore(format!(
                "sync encryption key deletion verification failed: {verification_error}{}",
                rollback
                    .map(|error| format!("; rollback failed: {error}"))
                    .unwrap_or_default()
            )));
        }
        drop(_guard);
        self.status().await
    }

    async fn load_keyset(&self) -> AppResult<Option<SyncKeyset>> {
        let Some(raw) = self.secrets.get(SYNC_E2EE_KEYSET_SECRET_ID).await? else {
            return Ok(None);
        };
        let raw = Zeroizing::new(raw);
        SyncKeyset::parse(raw.as_str()).map(Some)
    }

    async fn cleanup_expired_pairings(&self, now: i64) {
        self.pairing_initiators
            .lock()
            .await
            .retain(|_, pairing| match pairing {
                PendingInitiatorPairing::Exchange { expires_at, .. }
                | PendingInitiatorPairing::Prepared { expires_at, .. } => *expires_at > now,
            });
        self.pairing_responders
            .lock()
            .await
            .retain(|_, pairing| pairing.expires_at > now);
    }

    async fn persist_new_keyset(&self, keyset: &SyncKeyset) -> AppResult<()> {
        if self
            .secrets
            .get(SYNC_E2EE_KEYSET_SECRET_ID)
            .await?
            .is_some()
        {
            return Err(AppError::Other(
                "Sync encryption keys are already configured".into(),
            ));
        }
        let replacement = Zeroizing::new(keyset.serialize()?);
        if let Err(error) = self
            .secrets
            .set(SYNC_E2EE_KEYSET_SECRET_ID, replacement.as_str())
            .await
        {
            return Err(AppError::SecretStore(format!(
                "sync encryption key write failed: {error}"
            )));
        }
        let verification_error = match self.secrets.get(SYNC_E2EE_KEYSET_SECRET_ID).await {
            Ok(Some(stored)) => {
                let stored = Zeroizing::new(stored);
                (stored.as_str() != replacement.as_str())
                    .then(|| "stored keyset did not match".to_string())
            }
            Ok(None) => Some("stored keyset was missing".to_string()),
            Err(error) => Some(error.to_string()),
        };
        if let Some(reason) = verification_error {
            let rollback = self.secrets.delete(SYNC_E2EE_KEYSET_SECRET_ID).await.err();
            return Err(AppError::SecretStore(format!(
                "sync encryption key verification failed: {reason}{}",
                rollback
                    .map(|error| format!("; rollback failed: {error}"))
                    .unwrap_or_default()
            )));
        }
        Ok(())
    }

    async fn replace_secret_verified(
        &self,
        secret_id: &str,
        replacement: Option<&str>,
        previous: Option<&str>,
    ) -> AppResult<()> {
        let write = match replacement {
            Some(value) => self.secrets.set(secret_id, value).await,
            None => self.secrets.delete(secret_id).await,
        };
        if let Err(error) = write {
            let rollback = self.restore_secret_value(secret_id, previous).await.err();
            return Err(AppError::SecretStore(format!(
                "secret write failed: {error}{}",
                rollback
                    .map(|rollback| format!("; rollback failed: {rollback}"))
                    .unwrap_or_default()
            )));
        }
        let verified = self.secrets.get(secret_id).await;
        let matches = verified
            .as_ref()
            .ok()
            .is_some_and(|stored| stored.as_deref() == replacement);
        if !matches {
            if let Ok(Some(stored)) = verified {
                drop(Zeroizing::new(stored));
            }
            let rollback = self.restore_secret_value(secret_id, previous).await.err();
            return Err(AppError::SecretStore(format!(
                "secret verification failed{}",
                rollback
                    .map(|rollback| format!("; rollback failed: {rollback}"))
                    .unwrap_or_default()
            )));
        }
        if let Ok(Some(stored)) = verified {
            drop(Zeroizing::new(stored));
        }
        Ok(())
    }

    async fn restore_secret_value(&self, secret_id: &str, value: Option<&str>) -> AppResult<()> {
        match value {
            Some(value) => self.secrets.set(secret_id, value).await,
            None => self.secrets.delete(secret_id).await,
        }
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

    pub async fn start_pairing(&self) -> AppResult<pairing::PairingInvite> {
        let _guard = self.run_gate.lock().await;
        let status = self.status().await?;
        if !status.credential_configured || !status.e2ee.confirmed {
            return Err(AppError::Other(
                "A confirmed encrypted sync device is required to start pairing".into(),
            ));
        }
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("Sync credential is not configured".into()))?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let started = pairing::start_initiator(&session_id)?;
        let response = transport
            .create_pairing_session(&CreatePairingSessionRequest {
                protocol_version: PROTOCOL_VERSION,
                session_id: session_id.clone(),
                initiator_message: started.initiator_message,
            })
            .await
            .map_err(admin_transport_error)?;
        if response.session_id != session_id
            || response.server_time < 0
            || response.expires_at <= response.server_time
            || response.expires_at.saturating_sub(response.server_time) > 15 * 60 * 1_000
        {
            return Err(AppError::Other(
                "Invalid create pairing session response".into(),
            ));
        }
        let mut pairings = self.pairing_initiators.lock().await;
        pairings.retain(|_, pairing| match pairing {
            PendingInitiatorPairing::Exchange { expires_at, .. }
            | PendingInitiatorPairing::Prepared { expires_at, .. } => {
                *expires_at > response.server_time
            }
        });
        pairings.insert(
            session_id.clone(),
            PendingInitiatorPairing::Exchange {
                exchange: started.exchange,
                expires_at: response.expires_at,
            },
        );
        Ok(pairing::PairingInvite {
            session_id,
            pairing_code: started.pairing_code,
            expires_at: response.expires_at,
        })
    }

    pub async fn pending_pairing_device(
        &self,
        session_id: &str,
    ) -> AppResult<PendingPairingDevice> {
        let session_id = canonical_uuid(session_id, "Pairing session id is invalid")?;
        if !self
            .pairing_initiators
            .lock()
            .await
            .contains_key(&session_id)
        {
            return Err(AppError::Other(
                "Pairing session is not active on this device".into(),
            ));
        }
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("Sync credential is not configured".into()))?;
        let response = transport
            .get_pairing_join(&session_id)
            .await
            .map_err(admin_transport_error)?;
        validate_pairing_join(&session_id, &response)
    }

    pub async fn approve_pairing(&self, session_id: &str) -> AppResult<PendingPairingDevice> {
        let _guard = self.run_gate.lock().await;
        let session_id = canonical_uuid(session_id, "Pairing session id is invalid")?;
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("Sync credential is not configured".into()))?;
        let needs_join = matches!(
            self.pairing_initiators.lock().await.get(&session_id),
            Some(PendingInitiatorPairing::Exchange { .. })
        );
        let join = if needs_join {
            let response = transport
                .get_pairing_join(&session_id)
                .await
                .map_err(admin_transport_error)?;
            let pending = validate_pairing_join(&session_id, &response)?;
            Some((pending, response))
        } else {
            None
        };
        let state = self
            .pairing_initiators
            .lock()
            .await
            .remove(&session_id)
            .ok_or_else(|| {
                AppError::Other("Pairing session is not active on this device".into())
            })?;
        let (transfer, device, expires_at) = match state {
            PendingInitiatorPairing::Exchange {
                exchange,
                expires_at: _,
            } => {
                let (pending, response) =
                    join.expect("exchange pairing always fetches the pending device");
                let device = PairingDevice {
                    device_id: pending.device_id.clone(),
                    device_name: pending.device_name.clone(),
                    platform: pending.platform.clone(),
                };
                let key = pairing::finish_initiator(
                    exchange,
                    &session_id,
                    &response.responder_message,
                    &response.responder_proof,
                    &device,
                )?;
                let keyset = self.load_keyset().await?.ok_or_else(|| {
                    AppError::Other("Sync encryption keys are not configured".into())
                })?;
                let transfer =
                    pairing::prepare_transfer(&key, &session_id, &pending.device_id, &keyset)?;
                (transfer, device, pending.expires_at)
            }
            PendingInitiatorPairing::Prepared {
                transfer,
                device,
                expires_at,
            } => (transfer, device, expires_at),
        };
        let finalize = transport
            .finalize_pairing_session(
                &session_id,
                &FinalizePairingSessionRequest {
                    protocol_version: PROTOCOL_VERSION,
                    device_id: transfer.device_id.clone(),
                    credential_fingerprint: transfer.credential_fingerprint.clone(),
                    transfer_bundle: transfer.transfer_bundle.clone(),
                },
            )
            .await;
        let response = match finalize {
            Ok(response) => response,
            Err(error) => {
                self.pairing_initiators.lock().await.insert(
                    session_id,
                    PendingInitiatorPairing::Prepared {
                        transfer,
                        device,
                        expires_at,
                    },
                );
                return Err(admin_transport_error(error));
            }
        };
        if response.status != "ready"
            || response.device_id != device.device_id
            || response.server_time < 0
        {
            self.pairing_initiators.lock().await.insert(
                session_id,
                PendingInitiatorPairing::Prepared {
                    transfer,
                    device,
                    expires_at,
                },
            );
            return Err(AppError::Other(
                "Invalid finalize pairing session response".into(),
            ));
        }
        Ok(PendingPairingDevice {
            session_id,
            device_id: device.device_id,
            device_name: device.device_name,
            platform: device.platform,
            expires_at,
        })
    }

    pub async fn join_pairing(
        &self,
        pairing_code: &str,
        device_name: &str,
    ) -> AppResult<PairingJoinStarted> {
        let _guard = self.run_gate.lock().await;
        if self.secret_configured(SYNC_CREDENTIAL_SECRET_ID).await?
            || self.secret_configured(SYNC_E2EE_KEYSET_SECRET_ID).await?
            || self.db.get_sync_status().await?.e2ee_key_version.is_some()
        {
            return Err(AppError::Other(
                "Pairing can only be joined on an unconfigured device".into(),
            ));
        }
        let device_name = device_name.trim();
        let session_id = pairing::pairing_session_id(pairing_code)?;
        let transport = self.public_pairing_transport();
        let session = transport
            .get_public_pairing_session(&session_id)
            .await
            .map_err(admin_transport_error)?;
        if session.protocol_version != PROTOCOL_VERSION
            || session.session_id != session_id
            || session.server_time < 0
            || session.expires_at <= session.server_time
        {
            return Err(AppError::Other("Invalid public pairing response".into()));
        }
        let device = PairingDevice {
            device_id: self.db.get_sync_status().await?.device_id,
            device_name: device_name.to_string(),
            platform: Some(std::env::consts::OS.to_string()),
        };
        let (opened_session_id, responder) =
            pairing::start_responder(pairing_code, &session.initiator_message, &device)?;
        if opened_session_id != session_id {
            return Err(AppError::Other("Pairing session did not match".into()));
        }
        let request = JoinPairingSessionRequest {
            protocol_version: PROTOCOL_VERSION,
            device_id: device.device_id.clone(),
            device_name: device.device_name.clone(),
            platform: device.platform.clone(),
            responder_message: responder.responder_message,
            responder_proof: responder.responder_proof,
        };
        let response = transport
            .join_pairing_session(&session_id, &request)
            .await
            .map_err(admin_transport_error)?;
        if response.status != "joined"
            || response.server_time < 0
            || response.expires_at != Some(session.expires_at)
        {
            return Err(AppError::Other("Invalid pairing join response".into()));
        }
        let mut pairings = self.pairing_responders.lock().await;
        pairings.retain(|_, pairing| pairing.expires_at > session.server_time);
        pairings.insert(
            session_id.clone(),
            PendingResponderPairing {
                key: responder.key,
                device,
                join_request: request,
                expires_at: session.expires_at,
            },
        );
        Ok(PairingJoinStarted {
            session_id,
            expires_at: session.expires_at,
        })
    }

    pub async fn finish_pairing(&self, session_id: &str) -> AppResult<PairingCompletion> {
        let _guard = self.run_gate.lock().await;
        let session_id = canonical_uuid(session_id, "Pairing session id is invalid")?;
        let (join_request, expires_at) = {
            let states = self.pairing_responders.lock().await;
            let state = states.get(&session_id).ok_or_else(|| {
                AppError::Other("Pairing session is not active on this device".into())
            })?;
            (state.join_request.clone(), state.expires_at)
        };
        if expires_at <= unix_millis() {
            self.pairing_responders.lock().await.remove(&session_id);
            return Err(AppError::Other("Pairing session expired".into()));
        }
        let public_transport = self.public_pairing_transport();
        public_transport
            .join_pairing_session(&session_id, &join_request)
            .await
            .map_err(admin_transport_error)?;
        let package = public_transport
            .get_pairing_package(&session_id)
            .await
            .map_err(admin_transport_error)?;
        if package.server_time < 0 || package.expires_at != expires_at {
            return Err(AppError::Other("Invalid pairing package response".into()));
        }
        if package.status == "pending" && package.transfer_bundle.is_none() {
            return Ok(PairingCompletion {
                status: "pending".into(),
                sync_status: None,
            });
        }
        if package.status != "ready" || package.transfer_bundle.is_none() {
            return Err(AppError::Other("Invalid pairing package response".into()));
        }
        let state = self
            .pairing_responders
            .lock()
            .await
            .remove(&session_id)
            .ok_or_else(|| {
                AppError::Other("Pairing session is not active on this device".into())
            })?;
        let (keyset, opened) = pairing::open_transfer(
            &state.key,
            &session_id,
            &state.device.device_id,
            package.transfer_bundle.as_deref().unwrap_or_default(),
        )?;
        let credential = SyncCredential::Bearer {
            token: opened.bearer_token.clone(),
        };
        let credential_secret = Zeroizing::new(credential.clone().into_secret()?);
        if let Err(error) = self
            .install_paired_secrets(&keyset, &opened.keyset_json, credential_secret.as_str())
            .await
        {
            self.pairing_responders
                .lock()
                .await
                .insert(session_id.clone(), state);
            return Err(error);
        }
        let authenticated =
            HttpSyncTransport::new(self.client.clone(), SYNC_GATEWAY_URL, credential);
        if let Err(error) = authenticated.consume_pairing_session(&session_id).await {
            eprintln!("[sync] pairing package cleanup failed: {}", error.code);
        }
        Ok(PairingCompletion {
            status: "complete".into(),
            sync_status: Some(self.status().await?),
        })
    }

    async fn install_paired_secrets(
        &self,
        keyset: &SyncKeyset,
        keyset_json: &str,
        credential_secret: &str,
    ) -> AppResult<()> {
        if self.secret_configured(SYNC_E2EE_KEYSET_SECRET_ID).await?
            || self.secret_configured(SYNC_CREDENTIAL_SECRET_ID).await?
            || self.db.get_sync_status().await?.e2ee_key_version.is_some()
        {
            return Err(AppError::Other(
                "Pairing cannot overwrite configured sync secrets".into(),
            ));
        }
        self.replace_secret_verified(SYNC_E2EE_KEYSET_SECRET_ID, Some(keyset_json), None)
            .await?;
        if let Err(error) = self
            .replace_secret_verified(SYNC_CREDENTIAL_SECRET_ID, Some(credential_secret), None)
            .await
        {
            let rollback = self
                .restore_secret_value(SYNC_E2EE_KEYSET_SECRET_ID, None)
                .await
                .err();
            return Err(AppError::SecretStore(format!(
                "paired credential installation failed: {error}{}",
                rollback
                    .map(|rollback| format!("; keyset rollback failed: {rollback}"))
                    .unwrap_or_default()
            )));
        }
        if let Err(error) = self
            .db
            .set_sync_e2ee_key_version(Some(keyset.active_key_version()))
            .await
        {
            let credential_rollback = self
                .restore_secret_value(SYNC_CREDENTIAL_SECRET_ID, None)
                .await
                .err();
            let keyset_rollback = self
                .restore_secret_value(SYNC_E2EE_KEYSET_SECRET_ID, None)
                .await
                .err();
            return Err(AppError::SecretStore(format!(
                "paired E2EE state installation failed: {error}{}{}",
                credential_rollback
                    .map(|rollback| format!("; credential rollback failed: {rollback}"))
                    .unwrap_or_default(),
                keyset_rollback
                    .map(|rollback| format!("; keyset rollback failed: {rollback}"))
                    .unwrap_or_default()
            )));
        }
        Ok(())
    }

    fn public_pairing_transport(&self) -> HttpSyncTransport {
        HttpSyncTransport::new_public(self.client.clone(), SYNC_GATEWAY_URL)
    }

    async fn secret_configured(&self, secret_id: &str) -> AppResult<bool> {
        match self.secrets.get(secret_id).await? {
            Some(secret) => {
                drop(Zeroizing::new(secret));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn http_transport(&self) -> AppResult<Option<HttpSyncTransport>> {
        let Some(secret) = self.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await? else {
            return Ok(None);
        };
        let secret = Zeroizing::new(secret);
        let credential = SyncCredential::parse(secret.as_str())?;
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

    pub fn start_artifact_replication_background(
        self: Arc<Self>,
        storage: Arc<StorageService>,
        install_root: std::path::PathBuf,
    ) {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                match self
                    .run_artifact_replication_once(storage.clone(), install_root.clone())
                    .await
                {
                    Ok(Some(report)) if report.processed > 0 => {
                        eprintln!(
                            "[artifact-replication] processed={} downloaded={} skipped={} missing={} incompatible={} cursor={} more={}",
                            report.processed,
                            report.downloaded,
                            report.skipped,
                            report.missing,
                            report.incompatible,
                            report.next_cursor,
                            report.has_more,
                        );
                    }
                    Ok(Some(_)) | Ok(None) => {}
                    Err(error) => {
                        eprintln!("[artifact-replication] background run failed: {error}");
                    }
                }
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
    }

    async fn run_artifact_replication_once(
        &self,
        storage: Arc<StorageService>,
        install_root: std::path::PathBuf,
    ) -> AppResult<Option<ReplicationReport>> {
        let status = self.status().await?;
        if !status.credential_configured || !status.e2ee.confirmed || !status.e2ee.transport_ready {
            return Ok(None);
        }
        let keyset = self
            .load_keyset()
            .await?
            .ok_or_else(|| AppError::Other("Sync encryption keys are not configured".into()))?;
        let transport = self
            .http_transport()
            .await?
            .ok_or_else(|| AppError::Other("Sync credential is not configured".into()))?;
        let installer: Arc<dyn ArtifactInstaller> = storage;
        let coordinator = ArtifactReplicationCoordinator::new(
            self.db.clone(),
            Arc::new(transport),
            installer,
            crate::storage::MANAGED_R2_ACCOUNT_ID,
            install_root,
        )?;
        coordinator.run_once(&keyset).await.map(Some)
    }

    #[cfg(test)]
    pub(crate) async fn run_with_transport(
        &self,
        transport: &dyn SyncTransport,
    ) -> AppResult<SyncStatus> {
        self.run_with_optional_keyset(transport, None).await
    }

    async fn run_with_encrypted_transport(
        &self,
        transport: &dyn SyncTransport,
        keyset: &SyncKeyset,
    ) -> AppResult<SyncStatus> {
        self.run_with_optional_keyset(transport, Some(keyset)).await
    }

    async fn run_with_optional_keyset(
        &self,
        transport: &dyn SyncTransport,
        keyset: Option<&SyncKeyset>,
    ) -> AppResult<SyncStatus> {
        let Ok(_guard) = self.run_gate.try_lock() else {
            return self.status().await;
        };
        self.syncing.store(true, Ordering::SeqCst);
        let result = self.run_locked(transport, keyset).await;
        self.syncing.store(false, Ordering::SeqCst);
        result?;
        self.status().await
    }

    async fn run_locked(
        &self,
        transport: &dyn SyncTransport,
        keyset: Option<&SyncKeyset>,
    ) -> AppResult<()> {
        let initial_status = self.db.get_sync_status().await?;
        if initial_status
            .backoff_until
            .is_some_and(|value| value > unix_millis())
        {
            return Ok(());
        }
        if !self.run_bootstrap(transport, keyset).await? {
            return Ok(());
        }
        if !self.push_pending(transport, keyset).await? {
            return Ok(());
        }
        self.pull_remote(transport, keyset).await?;
        Ok(())
    }

    async fn run_bootstrap(
        &self,
        transport: &dyn SyncTransport,
        keyset: Option<&SyncKeyset>,
    ) -> AppResult<bool> {
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
                .map(|entity| remote_entity_from_bootstrap(entity, keyset))
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

    async fn push_pending(
        &self,
        transport: &dyn SyncTransport,
        keyset: Option<&SyncKeyset>,
    ) -> AppResult<bool> {
        for _ in 0..MAX_BATCHES_PER_RUN {
            let now_ms = unix_millis();
            let batch = self.db.claim_sync_outbox(MAX_PUSH_CHANGES, now_ms).await?;
            if batch.is_empty() {
                break;
            }
            let change_ids = batch.iter().map(|row| row.change_id.clone()).collect();
            let batch = match keyset {
                Some(keyset) => self.prepare_encrypted_batch(batch, keyset).await,
                None => Ok(batch),
            };
            let batch = match batch {
                Ok(batch) => batch,
                Err(error) => {
                    self.db
                        .mark_sync_dead_letter(
                            change_ids,
                            "INVALID_LOCAL_PAYLOAD".into(),
                            error.to_string(),
                        )
                        .await?;
                    continue;
                }
            };
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

    async fn prepare_encrypted_batch(
        &self,
        batch: Vec<crate::db::repo::sync::OutboxRow>,
        keyset: &SyncKeyset,
    ) -> AppResult<Vec<crate::db::repo::sync::OutboxRow>> {
        let mut sealed = Vec::new();
        for row in &batch {
            if row.payload_encoding != "json" {
                validate_encrypted_outbox_row(row, keyset)?;
                continue;
            }
            let change = if row.operation == "delete" {
                if row.source_payload.is_some()
                    || row.payload.is_some()
                    || row.key_version.is_some()
                {
                    return Err(AppError::Other(format!(
                        "invalid local tombstone `{}`",
                        row.change_id
                    )));
                }
                SealedOutboxChange {
                    change_id: row.change_id.clone(),
                    payload_encoding: TOMBSTONE_ENCODING.into(),
                    payload: None,
                    payload_hash: EMPTY_PAYLOAD_HASH.into(),
                    key_version: None,
                }
            } else if row.operation == "upsert" {
                if row.key_version.is_some() {
                    return Err(AppError::Other(format!(
                        "unsealed outbox change `{}` already has a key version",
                        row.change_id
                    )));
                }
                let source = row.source_payload.as_deref().ok_or_else(|| {
                    AppError::Other(format!("local payload `{}` is missing", row.change_id))
                })?;
                let source = serde_json::from_str(source)?;
                let key_version = keyset.active_key_version();
                let revision = row
                    .base_revision
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| AppError::Other("Sync revision is exhausted".into()))?;
                let metadata = PayloadMetadata {
                    protocol_version: PROTOCOL_VERSION,
                    entity_type: &row.entity_type,
                    entity_id: &row.entity_id,
                    revision,
                    hlc: &row.hlc,
                    payload_schema_version: row.payload_schema_version,
                    origin_device_id: &row.device_id,
                    key_version,
                };
                let encrypted = seal_json(keyset.active_key(), &metadata, &source)?;
                SealedOutboxChange {
                    change_id: row.change_id.clone(),
                    payload_encoding: PAYLOAD_ENCODING.into(),
                    payload: Some(serde_json::to_string(&encrypted.payload)?),
                    payload_hash: encrypted.payload_hash,
                    key_version: Some(key_version),
                }
            } else {
                return Err(AppError::Other(format!(
                    "unsupported local operation `{}`",
                    row.operation
                )));
            };
            sealed.push(change);
        }
        if sealed.is_empty() {
            return Ok(batch);
        }
        let persisted = self.db.persist_sealed_sync_outbox(sealed).await?;
        let mut persisted = persisted
            .into_iter()
            .map(|row| (row.change_id.clone(), row))
            .collect::<std::collections::HashMap<_, _>>();
        batch
            .into_iter()
            .map(|row| {
                if row.payload_encoding == "json" {
                    persisted.remove(&row.change_id).ok_or_else(|| {
                        AppError::Other(format!(
                            "sealed outbox change `{}` was not returned",
                            row.change_id
                        ))
                    })
                } else {
                    Ok(row)
                }
            })
            .collect()
    }

    async fn pull_remote(
        &self,
        transport: &dyn SyncTransport,
        keyset: Option<&SyncKeyset>,
    ) -> AppResult<()> {
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
                .map(|change| remote_entity_from_change(change, keyset))
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

fn validate_encrypted_outbox_row(
    row: &crate::db::repo::sync::OutboxRow,
    keyset: &SyncKeyset,
) -> AppResult<()> {
    if row.operation == "delete" {
        if row.payload_encoding == TOMBSTONE_ENCODING
            && row.payload.is_none()
            && row.source_payload.is_none()
            && row.payload_hash == EMPTY_PAYLOAD_HASH
            && row.key_version.is_none()
        {
            return Ok(());
        }
    } else if row.operation == "upsert" && row.payload_encoding == PAYLOAD_ENCODING {
        let key_version = row
            .key_version
            .filter(|version| *version > 0)
            .ok_or_else(|| {
                AppError::Other(format!(
                    "encrypted outbox key version is missing for `{}`",
                    row.change_id
                ))
            })?;
        let key = keyset.key(key_version).ok_or_else(|| {
            AppError::Other(format!(
                "Sync encryption key version {key_version} is not available"
            ))
        })?;
        let payload: serde_json::Value = row
            .payload
            .as_deref()
            .ok_or_else(|| {
                AppError::Other(format!("encrypted payload `{}` is missing", row.change_id))
            })
            .and_then(|payload| serde_json::from_str(payload).map_err(Into::into))?;
        let source: serde_json::Value = row
            .source_payload
            .as_deref()
            .ok_or_else(|| AppError::Other(format!("local payload `{}` is missing", row.change_id)))
            .and_then(|payload| serde_json::from_str(payload).map_err(Into::into))?;
        let revision = row
            .base_revision
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| AppError::Other("Sync revision is exhausted".into()))?;
        let opened = open_json(
            key,
            &PayloadMetadata {
                protocol_version: PROTOCOL_VERSION,
                entity_type: &row.entity_type,
                entity_id: &row.entity_id,
                revision,
                hlc: &row.hlc,
                payload_schema_version: row.payload_schema_version,
                origin_device_id: &row.device_id,
                key_version,
            },
            &payload,
            &row.payload_hash,
        )?;
        if opened == source {
            return Ok(());
        }
    }
    Err(AppError::Other(format!(
        "invalid encrypted outbox change `{}`",
        row.change_id
    )))
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

fn validate_e2ee_status(
    keyset: Option<&SyncKeyset>,
    confirmed_version: Option<i64>,
) -> AppResult<SyncE2eeStatus> {
    if let Some(version) = confirmed_version {
        let keyset = keyset.ok_or_else(|| {
            AppError::Other("E2EE is enabled but the local keyset is missing".into())
        })?;
        if keyset.key(version).is_none() {
            return Err(AppError::Other(format!(
                "E2EE key version {version} is missing from the local keyset"
            )));
        }
        if keyset.active_key_version() < version {
            return Err(AppError::Other(
                "The active E2EE key version is behind the confirmed version".into(),
            ));
        }
    }
    let active_key_version = keyset.map(SyncKeyset::active_key_version);
    let rotation_pending = confirmed_version
        .zip(active_key_version)
        .is_some_and(|(confirmed, active)| active > confirmed);
    Ok(SyncE2eeStatus {
        keyset_configured: keyset.is_some(),
        confirmed: confirmed_version.is_some() && !rotation_pending,
        active_key_version,
        confirmed_key_version: confirmed_version,
        rotation_pending,
        transport_ready: E2EE_TRANSPORT_READY,
    })
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

fn knowledge_publish_hlc(manifest: &ArtifactManifest, device_id: &str) -> AppResult<String> {
    let physical_ms = manifest
        .created_at
        .parse::<u64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000))
        .ok_or_else(|| AppError::Other("知识制品创建时间无效".into()))?;
    let node = device_id.chars().take(8).collect::<String>();
    HybridTimestamp::tick(None, physical_ms, &node)
        .map(|timestamp| timestamp.to_string())
        .map_err(AppError::Other)
}

fn current_unix_seconds_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

fn canonical_uuid(value: &str, message: &str) -> AppResult<String> {
    uuid::Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| AppError::Other(message.into()))
}

fn validate_pairing_join(
    session_id: &str,
    response: &PairingJoinResponse,
) -> AppResult<PendingPairingDevice> {
    if uuid::Uuid::parse_str(&response.device_id).is_err()
        || response.device_name.trim().is_empty()
        || response.device_name.len() > 80
        || response
            .platform
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 40)
        || response.responder_message.is_empty()
        || response.responder_proof.is_empty()
        || response.server_time < 0
        || response.expires_at <= response.server_time
    {
        return Err(AppError::Other("Invalid pairing join response".into()));
    }
    Ok(PendingPairingDevice {
        session_id: session_id.to_string(),
        device_id: response.device_id.clone(),
        device_name: response.device_name.clone(),
        platform: response.platform.clone(),
        expires_at: response.expires_at,
    })
}

fn remote_entity_from_change(
    change: RemoteChange,
    keyset: Option<&SyncKeyset>,
) -> AppResult<RemoteEntityInput> {
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
    decode_remote_entity(
        RemoteEntityInput {
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
        },
        keyset,
    )
}

fn remote_entity_from_bootstrap(
    entity: RemoteEntity,
    keyset: Option<&SyncKeyset>,
) -> AppResult<RemoteEntityInput> {
    if entity.latest_server_seq <= 0 || entity.updated_at < 0 {
        return Err(AppError::Other(format!(
            "invalid bootstrap entity {} `{}`",
            entity.entity_type, entity.entity_id
        )));
    }
    decode_remote_entity(
        RemoteEntityInput {
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
        },
        keyset,
    )
}

fn decode_remote_entity(
    mut entity: RemoteEntityInput,
    keyset: Option<&SyncKeyset>,
) -> AppResult<RemoteEntityInput> {
    let Some(keyset) = keyset else {
        return Ok(entity);
    };
    if entity.deleted {
        if entity.payload_encoding != TOMBSTONE_ENCODING
            || entity.payload.is_some()
            || entity.key_version.is_some()
            || entity.payload_hash != EMPTY_PAYLOAD_HASH
        {
            return Err(AppError::Other(format!(
                "invalid encrypted tombstone for {} `{}`",
                entity.entity_type, entity.entity_id
            )));
        }
    } else {
        let key_version = entity
            .key_version
            .filter(|version| *version > 0)
            .ok_or_else(|| {
                AppError::Other(format!(
                    "encrypted payload key version is missing for {} `{}`",
                    entity.entity_type, entity.entity_id
                ))
            })?;
        if entity.payload_encoding != PAYLOAD_ENCODING || entity.payload.is_none() {
            return Err(AppError::Other(format!(
                "invalid encrypted payload envelope for {} `{}`",
                entity.entity_type, entity.entity_id
            )));
        }
        let key = keyset.key(key_version).ok_or_else(|| {
            AppError::Other(format!(
                "Sync encryption key version {key_version} is not available"
            ))
        })?;
        let metadata = PayloadMetadata {
            protocol_version: u8::try_from(entity.protocol_version)
                .map_err(|_| AppError::Other("Invalid sync protocol version".into()))?,
            entity_type: &entity.entity_type,
            entity_id: &entity.entity_id,
            revision: entity.revision,
            hlc: &entity.hlc,
            payload_schema_version: entity.payload_schema_version,
            origin_device_id: &entity.origin_device_id,
            key_version,
        };
        entity.payload = Some(open_json(
            key,
            &metadata,
            entity
                .payload
                .as_ref()
                .expect("validated encrypted payload"),
            &entity.payload_hash,
        )?);
    }
    entity.payload_encoding = "json".into();
    entity.key_version = None;
    Ok(entity)
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

    struct EncryptedRetryTransport {
        requests: std::sync::Mutex<Vec<PushRequest>>,
    }

    #[async_trait]
    impl SyncTransport for EncryptedRetryTransport {
        async fn push(&self, request: &PushRequest) -> Result<PushResponse, TransportFailure> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            if call == 1 {
                return Err(TransportFailure {
                    kind: FailureKind::Retryable,
                    code: "SYNC_TEMPORARILY_UNAVAILABLE".into(),
                    message: "temporary outage".into(),
                    retry_after_ms: Some(30_000),
                });
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

    struct EncryptedPullTransport {
        change: RemoteChange,
        acknowledgements: AtomicUsize,
    }

    #[async_trait]
    impl SyncTransport for EncryptedPullTransport {
        async fn push(&self, _request: &PushRequest) -> Result<PushResponse, TransportFailure> {
            panic!("pull-only test must not push")
        }

        async fn pull(&self, after: i64, _limit: usize) -> Result<PullResponse, TransportFailure> {
            assert_eq!(after, 0);
            Ok(PullResponse {
                changes: vec![self.change.clone()],
                next_cursor: 1,
                has_more: false,
                server_time: 1,
            })
        }

        async fn bootstrap(
            &self,
            _cursor: Option<&str>,
            _limit: usize,
        ) -> Result<BootstrapResponse, TransportFailure> {
            panic!("pull-only test must already be bootstrapped")
        }

        async fn ack(&self, request: &AckRequest) -> Result<AckResponse, TransportFailure> {
            self.acknowledgements.fetch_add(1, Ordering::SeqCst);
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

    fn encrypted_remote_agent(keyset: &SyncKeyset) -> RemoteChange {
        let payload = json!({
            "id": "remote-encrypted-agent",
            "name": "Remote encrypted agent",
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
            "origin_device_id": DEVICE_B
        });
        let key_version = keyset.active_key_version();
        let hlc = "1000-0001-remote02";
        let encrypted = seal_json(
            keyset.active_key(),
            &PayloadMetadata {
                protocol_version: PROTOCOL_VERSION,
                entity_type: "agent",
                entity_id: "remote-encrypted-agent",
                revision: 1,
                hlc,
                payload_schema_version: 1,
                origin_device_id: DEVICE_B,
                key_version,
            },
            &payload,
        )
        .unwrap();
        RemoteChange {
            protocol_version: PROTOCOL_VERSION,
            server_seq: 1,
            change_id: "10000000-0000-4000-8000-000000000099".into(),
            device_id: DEVICE_B.into(),
            entity_type: "agent".into(),
            entity_id: "remote-encrypted-agent".into(),
            operation: "upsert".into(),
            base_revision: None,
            resulting_revision: 1,
            hlc: hlc.into(),
            payload_schema_version: 1,
            payload_encoding: PAYLOAD_ENCODING.into(),
            payload: Some(encrypted.payload),
            payload_hash: encrypted.payload_hash,
            key_version: Some(key_version),
            created_at: 1_000,
            accepted_at: 1_001,
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
    async fn encrypted_only_gate_blocks_sync_until_key_confirmation() {
        let (path, service) = test_service("encrypted-only-gate");
        service
            .secrets
            .set(
                SYNC_CREDENTIAL_SECRET_ID,
                &SyncCredential::Bearer {
                    token: "test-device-token".into(),
                }
                .into_secret()
                .unwrap(),
            )
            .await
            .unwrap();
        service
            .db
            .insert_agent(new_agent("encrypted-agent"))
            .await
            .unwrap();

        let required = service.run_once().await.unwrap();
        assert_eq!(required.state, "e2ee_required");
        assert!(!required.e2ee.keyset_configured);
        assert_eq!(required.database.pending_count, 1);

        service.begin_e2ee_setup().await.unwrap();
        let awaiting_confirmation = service.run_once().await.unwrap();
        assert_eq!(awaiting_confirmation.state, "e2ee_required");
        assert!(awaiting_confirmation.e2ee.keyset_configured);
        assert!(!awaiting_confirmation.e2ee.confirmed);

        let confirmed = service.confirm_e2ee_setup().await.unwrap();
        assert_eq!(confirmed.state, "pending");
        assert!(confirmed.e2ee.confirmed);
        assert_eq!(confirmed.e2ee.active_key_version, Some(1));
        assert!(confirmed.e2ee.transport_ready);
        assert_eq!(confirmed.database.pending_count, 1);

        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn recovery_installs_a_new_keyring_but_never_overwrites_a_different_keyset() {
        let (source_path, source) = test_service("recovery-source");
        let material = source.begin_e2ee_setup().await.unwrap();
        source.confirm_e2ee_setup().await.unwrap();

        let (target_path, target) = test_service("recovery-target");
        let restored = target
            .restore_e2ee(&material.recovery_key, &material.recovery_bundle)
            .await
            .unwrap();
        assert!(restored.e2ee.keyset_configured);
        assert!(restored.e2ee.confirmed);
        assert_eq!(restored.e2ee.active_key_version, Some(1));
        assert_eq!(restored.database.e2ee_key_version, Some(1));

        let same = target
            .restore_e2ee(&material.recovery_key, &material.recovery_bundle)
            .await
            .unwrap();
        assert!(same.e2ee.confirmed);

        let (other_path, other) = test_service("recovery-other");
        let other_material = other.begin_e2ee_setup().await.unwrap();
        let error = target
            .restore_e2ee(
                &other_material.recovery_key,
                &other_material.recovery_bundle,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not a monotonic upgrade"));
        assert_eq!(
            target.status().await.unwrap().e2ee.active_key_version,
            Some(1)
        );
        assert!(target.discard_e2ee_setup().await.is_err());
        let discarded = other.discard_e2ee_setup().await.unwrap();
        assert!(!discarded.e2ee.keyset_configured);
        assert!(!discarded.e2ee.confirmed);

        drop(source);
        drop(target);
        drop(other);
        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
        let _ = std::fs::remove_file(other_path);
    }

    #[tokio::test]
    async fn rotation_and_recovery_rehearsal_preserves_old_and_new_key_versions() {
        let (source_path, source) = test_service("rotation-source");
        let initial = source.begin_e2ee_setup().await.unwrap();
        source.confirm_e2ee_setup().await.unwrap();
        let (target_path, target) = test_service("rotation-target");
        target
            .restore_e2ee(&initial.recovery_key, &initial.recovery_bundle)
            .await
            .unwrap();

        let old_keyset = target.load_keyset().await.unwrap().unwrap();
        let old_metadata = PayloadMetadata {
            protocol_version: PROTOCOL_VERSION,
            entity_type: "memory",
            entity_id: "rotation-rehearsal-old",
            revision: 1,
            hlc: "1000-0001-device01",
            payload_schema_version: 1,
            origin_device_id: DEVICE_A,
            key_version: 1,
        };
        let old_payload = seal_json(
            old_keyset.key(1).unwrap(),
            &old_metadata,
            &json!({"content": "before rotation"}),
        )
        .unwrap();

        let rotated_material = source.begin_e2ee_rotation().await.unwrap();
        let pending = source.status().await.unwrap();
        assert_eq!(pending.state, "auth_required");
        assert!(pending.e2ee.rotation_pending);
        assert!(!pending.e2ee.confirmed);
        assert_eq!(pending.e2ee.confirmed_key_version, Some(1));
        assert_eq!(pending.e2ee.active_key_version, Some(2));
        source.confirm_e2ee_setup().await.unwrap();

        let upgraded = target
            .restore_e2ee(
                &rotated_material.recovery_key,
                &rotated_material.recovery_bundle,
            )
            .await
            .unwrap();
        assert!(upgraded.e2ee.confirmed);
        assert!(!upgraded.e2ee.rotation_pending);
        assert_eq!(upgraded.e2ee.active_key_version, Some(2));
        let upgraded_keyset = target.load_keyset().await.unwrap().unwrap();
        assert_eq!(
            open_json(
                upgraded_keyset.key(1).unwrap(),
                &old_metadata,
                &old_payload.payload,
                &old_payload.payload_hash,
            )
            .unwrap(),
            json!({"content": "before rotation"})
        );
        let new_metadata = PayloadMetadata {
            entity_id: "rotation-rehearsal-new",
            key_version: 2,
            ..old_metadata
        };
        let new_payload = seal_json(
            upgraded_keyset.key(2).unwrap(),
            &new_metadata,
            &json!({"content": "after rotation"}),
        )
        .unwrap();
        assert_eq!(
            open_json(
                upgraded_keyset.key(2).unwrap(),
                &new_metadata,
                &new_payload.payload,
                &new_payload.payload_hash,
            )
            .unwrap(),
            json!({"content": "after rotation"})
        );

        drop(source);
        drop(target);
        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
    }

    #[tokio::test]
    async fn encrypted_push_persists_and_reuses_identical_ciphertext_after_retry() {
        let (path, service) = test_service("encrypted-retry");
        service
            .db
            .insert_agent(new_agent("encrypted-retry-agent"))
            .await
            .unwrap();
        let keyset = SyncKeyset::generate_initial();
        let transport = EncryptedRetryTransport {
            requests: std::sync::Mutex::new(Vec::new()),
        };

        service
            .run_with_encrypted_transport(&transport, &keyset)
            .await
            .unwrap();
        let first_request = transport.requests.lock().unwrap()[0].clone();
        let first_change = &first_request.changes[0];
        assert_eq!(first_change.payload_encoding, PAYLOAD_ENCODING);
        assert!(first_change
            .payload
            .as_ref()
            .is_some_and(serde_json::Value::is_string));
        assert_eq!(first_change.key_version, Some(1));
        let conn = rusqlite::Connection::open(&path).unwrap();
        let persisted: (String, String, String, String, i64, String) = conn
            .query_row(
                "SELECT payload_encoding, payload, source_payload, payload_hash, key_version, status \
                 FROM sync_outbox WHERE entity_id = 'encrypted-retry-agent'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(persisted.0, PAYLOAD_ENCODING);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&persisted.1).unwrap(),
            first_change.payload.clone().unwrap()
        );
        assert_ne!(persisted.1, persisted.2);
        assert_eq!(persisted.3, first_change.payload_hash);
        assert_eq!(persisted.4, 1);
        assert_eq!(persisted.5, "pending");
        conn.execute(
            "UPDATE sync_outbox SET next_retry_at = 0 WHERE status = 'pending'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET backoff_until = NULL WHERE singleton = 1",
            [],
        )
        .unwrap();
        drop(conn);

        service
            .run_with_encrypted_transport(&transport, &keyset)
            .await
            .unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let retried = &requests[1].changes[0];
        assert_eq!(retried.payload, first_change.payload);
        assert_eq!(retried.payload_hash, first_change.payload_hash);
        assert_eq!(retried.key_version, first_change.key_version);
        drop(requests);

        let conn = rusqlite::Connection::open(&path).unwrap();
        let state: (String, String) = conn
            .query_row(
                "SELECT o.status, e.base_payload FROM sync_outbox o \
                 JOIN sync_entity_state e ON e.entity_type = o.entity_type AND e.entity_id = o.entity_id \
                 WHERE o.entity_id = 'encrypted-retry-agent'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state.0, "synced");
        assert_eq!(state.1, persisted.2);
        drop(conn);
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn encrypted_pull_decrypts_before_commit_and_rejects_a_tampered_page_atomically() {
        let keyset = SyncKeyset::generate_initial();
        let (valid_path, valid_service) = test_service("encrypted-pull-valid");
        valid_service.db.get_sync_status().await.unwrap();
        let conn = rusqlite::Connection::open(&valid_path).unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET bootstrap_state = 'complete' WHERE singleton = 1",
            [],
        )
        .unwrap();
        drop(conn);
        let valid_transport = EncryptedPullTransport {
            change: encrypted_remote_agent(&keyset),
            acknowledgements: AtomicUsize::new(0),
        };
        valid_service
            .run_with_encrypted_transport(&valid_transport, &keyset)
            .await
            .unwrap();
        assert_eq!(valid_transport.acknowledgements.load(Ordering::SeqCst), 1);
        let conn = rusqlite::Connection::open(&valid_path).unwrap();
        let applied: (String, i64) = conn
            .query_row(
                "SELECT a.name, r.last_pull_cursor FROM agents a \
                 JOIN sync_runtime_state r ON r.singleton = 1 \
                 WHERE a.id = 'remote-encrypted-agent'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(applied, ("Remote encrypted agent".into(), 1));
        drop(conn);

        let (invalid_path, invalid_service) = test_service("encrypted-pull-invalid");
        invalid_service.db.get_sync_status().await.unwrap();
        let conn = rusqlite::Connection::open(&invalid_path).unwrap();
        conn.execute(
            "UPDATE sync_runtime_state SET bootstrap_state = 'complete' WHERE singleton = 1",
            [],
        )
        .unwrap();
        drop(conn);
        let mut tampered = encrypted_remote_agent(&keyset);
        tampered.payload_hash = "f".repeat(64);
        let invalid_transport = EncryptedPullTransport {
            change: tampered,
            acknowledgements: AtomicUsize::new(0),
        };
        assert!(invalid_service
            .run_with_encrypted_transport(&invalid_transport, &keyset)
            .await
            .is_err());
        assert_eq!(invalid_transport.acknowledgements.load(Ordering::SeqCst), 0);
        let conn = rusqlite::Connection::open(&invalid_path).unwrap();
        let unchanged: (i64, i64) = conn
            .query_row(
                "SELECT last_pull_cursor, \
                        (SELECT COUNT(*) FROM agents WHERE id = 'remote-encrypted-agent') \
                 FROM sync_runtime_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(unchanged, (0, 0));
        drop(conn);

        drop(valid_service);
        drop(invalid_service);
        let _ = std::fs::remove_file(valid_path);
        let _ = std::fs::remove_file(invalid_path);
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
    #[ignore = "requires explicit remote credentials and writes disposable encrypted test data"]
    async fn remote_bearer_push_accepts_an_encrypted_fake_agent() {
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
        service.begin_e2ee_setup().await.unwrap();
        service.confirm_e2ee_setup().await.unwrap();
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
