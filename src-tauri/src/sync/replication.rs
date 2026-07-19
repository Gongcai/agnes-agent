use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;

use crate::db::repo::artifacts::{ArtifactManifestRow, DeviceArtifactStateRow};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::storage::artifact_transfer::RemoteArtifactDescriptor;
use crate::storage::domain::RemoteObjectLocator;
use crate::storage::StorageService;

use super::client::{ObjectSyncTransport, TransportFailure};
use super::crypto::{SyncKeyset, SyncMasterKey};
use super::protocol::{
    ObjectLocalStatus, ObjectManifestResponse, ObjectStateRequest, ObjectStateResponse,
    PROTOCOL_VERSION,
};

const OBJECT_PAGE_LIMIT: usize = 100;
const MAX_OBJECT_PAGES_PER_RUN: usize = 5;

#[async_trait]
pub trait ArtifactInstaller: Send + Sync {
    async fn download_and_install_remote_artifact(
        &self,
        account_id: String,
        descriptor: RemoteArtifactDescriptor,
        locator: RemoteObjectLocator,
        master_key: &SyncMasterKey,
        install_root: PathBuf,
    ) -> AppResult<(PathBuf, crate::sync::artifact::ArtifactManifest)>;
}

#[async_trait]
impl ArtifactInstaller for StorageService {
    async fn download_and_install_remote_artifact(
        &self,
        account_id: String,
        descriptor: RemoteArtifactDescriptor,
        locator: RemoteObjectLocator,
        master_key: &SyncMasterKey,
        install_root: PathBuf,
    ) -> AppResult<(PathBuf, crate::sync::artifact::ArtifactManifest)> {
        self.download_and_install_remote_artifact(
            account_id,
            descriptor,
            locator,
            master_key,
            install_root,
        )
        .await
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicationReport {
    pub processed: usize,
    pub skipped: usize,
    pub downloaded: usize,
    pub missing: usize,
    pub incompatible: usize,
    pub next_cursor: i64,
    pub has_more: bool,
}

pub struct ArtifactReplicationCoordinator {
    db: DbActorHandle,
    transport: Arc<dyn ObjectSyncTransport>,
    installer: Arc<dyn ArtifactInstaller>,
    provider_account_id: String,
    install_root: PathBuf,
}

impl ArtifactReplicationCoordinator {
    pub fn new(
        db: DbActorHandle,
        transport: Arc<dyn ObjectSyncTransport>,
        installer: Arc<dyn ArtifactInstaller>,
        provider_account_id: impl Into<String>,
        install_root: PathBuf,
    ) -> AppResult<Self> {
        let provider_account_id = provider_account_id.into();
        if provider_account_id.trim().is_empty() {
            return Err(AppError::Other(
                "Artifact replication provider account is required".into(),
            ));
        }
        if !install_root.is_absolute() {
            return Err(AppError::Other(
                "Artifact replication install root must be absolute".into(),
            ));
        }
        Ok(Self {
            db,
            transport,
            installer,
            provider_account_id: provider_account_id.into(),
            install_root,
        })
    }

    pub async fn run_once(&self, keyset: &SyncKeyset) -> AppResult<ReplicationReport> {
        let status = self.db.get_sync_status().await?;
        let device_id = status.device_id;
        let mut report = ReplicationReport {
            next_cursor: status.last_object_cursor,
            ..ReplicationReport::default()
        };
        let mut after = status.last_object_cursor;
        for _ in 0..MAX_OBJECT_PAGES_PER_RUN {
            let page = self
                .transport
                .list_object_changes(after, OBJECT_PAGE_LIMIT)
                .await
                .map_err(transport_error)?;
            if page.next_cursor < after {
                return Err(AppError::Other(
                    "Object control plane returned a regressing cursor".into(),
                ));
            }
            if page.next_cursor == after && (page.has_more || !page.changes.is_empty()) {
                return Err(AppError::Other(
                    "Object control plane returned a non-progressing cursor".into(),
                ));
            }
            let mut previous_seq = after;
            for change in &page.changes {
                if change.server_seq <= previous_seq {
                    return Err(AppError::Other(
                        "Object changes are not strictly ordered by server sequence".into(),
                    ));
                }
                previous_seq = change.server_seq;
            }
            if previous_seq != page.next_cursor {
                return Err(AppError::Other(
                    "Object change cursor does not match the page".into(),
                ));
            }
            for change in page.changes {
                report.processed += 1;
                let Some(artifact_id) = change.artifact_id.clone() else {
                    self.report_state(
                        &device_id,
                        &change.object_id,
                        change.logical_version,
                        None,
                        ObjectLocalStatus::Missing,
                        None,
                        Some("NO_ARTIFACT"),
                    )
                    .await?;
                    report.missing += 1;
                    continue;
                };
                let manifest = self
                    .transport
                    .get_object_manifest(&change.object_id)
                    .await
                    .map_err(transport_error)?;
                validate_manifest_change(&change, &manifest, &artifact_id)?;
                let remote = &manifest.manifest;
                let local = self
                    .db
                    .get_artifact_manifest(remote.artifact_id.clone())
                    .await?;
                if local
                    .as_ref()
                    .is_some_and(|local| local_matches_remote(local, remote))
                    && local
                        .as_ref()
                        .is_some_and(|local| local.local_status == "installed")
                {
                    self.persist_local_state(
                        &device_id,
                        local.as_ref().unwrap(),
                        remote.logical_version,
                    )
                    .await?;
                    self.report_state(
                        &device_id,
                        &remote.object_id,
                        remote.logical_version,
                        Some(remote.artifact_id.clone()),
                        ObjectLocalStatus::Installed,
                        Some(remote.ciphertext_hash.clone()),
                        None,
                    )
                    .await?;
                    report.skipped += 1;
                    continue;
                }

                let Some(master_key) = keyset.key(remote.key_version) else {
                    self.report_state(
                        &device_id,
                        &remote.object_id,
                        remote.logical_version,
                        None,
                        ObjectLocalStatus::Incompatible,
                        None,
                        Some("KEY_VERSION_UNAVAILABLE"),
                    )
                    .await?;
                    report.incompatible += 1;
                    continue;
                };
                let Some(replica) = manifest
                    .replicas
                    .iter()
                    .find(|replica| replica.provider_kind == "r2" && replica.status == "ready")
                else {
                    self.report_state(
                        &device_id,
                        &remote.object_id,
                        remote.logical_version,
                        None,
                        ObjectLocalStatus::Missing,
                        None,
                        Some("NO_READY_REPLICA"),
                    )
                    .await?;
                    report.missing += 1;
                    continue;
                };
                self.report_state(
                    &device_id,
                    &remote.object_id,
                    remote.logical_version,
                    None,
                    ObjectLocalStatus::Downloading,
                    None,
                    None,
                )
                .await?;
                let install_result = self
                    .installer
                    .download_and_install_remote_artifact(
                        self.provider_account_id.clone(),
                        RemoteArtifactDescriptor {
                            artifact_id: remote.artifact_id.clone(),
                            artifact_type: remote.object_kind.clone(),
                            ciphertext_hash: remote.ciphertext_hash.clone(),
                            size: remote.size,
                            key_version: remote.key_version,
                            updated_at: remote.updated_at,
                        },
                        RemoteObjectLocator {
                            opaque_id: remote.artifact_id.clone(),
                            revision: replica.provider_revision.clone(),
                        },
                        master_key,
                        self.install_root.clone(),
                    )
                    .await;
                let (installed, installed_manifest) = match install_result {
                    Ok(result) => result,
                    Err(error) => {
                        let _ = self
                            .report_state(
                                &device_id,
                                &remote.object_id,
                                remote.logical_version,
                                None,
                                ObjectLocalStatus::Failed,
                                None,
                                Some("DOWNLOAD_FAILED"),
                            )
                            .await;
                        return Err(error);
                    }
                };
                if !local_matches_remote_manifest(&installed_manifest, remote) {
                    return Err(AppError::Other(
                        "Installed artifact does not match the control manifest".into(),
                    ));
                }
                self.db
                    .upsert_artifact_manifest(crate::db::repo::artifacts::UpsertArtifactManifest {
                        manifest: installed_manifest,
                        local_path: Some(installed.to_string_lossy().to_string()),
                        local_status: "installed".into(),
                        installed_at: Some(now_string()),
                    })
                    .await?;
                let local = self
                    .db
                    .get_artifact_manifest(remote.artifact_id.clone())
                    .await?
                    .ok_or_else(|| {
                        AppError::Other("Installed artifact manifest was not persisted".into())
                    })?;
                self.persist_local_state(&device_id, &local, remote.logical_version)
                    .await?;
                self.report_state(
                    &device_id,
                    &remote.object_id,
                    remote.logical_version,
                    Some(remote.artifact_id.clone()),
                    ObjectLocalStatus::Installed,
                    Some(remote.ciphertext_hash.clone()),
                    None,
                )
                .await?;
                report.downloaded += 1;
            }
            self.db
                .advance_sync_object_cursor(after, page.next_cursor)
                .await
                .map_err(|error| {
                    AppError::Other(format!("advance object cursor failed: {error}"))
                })?;
            after = page.next_cursor;
            report.next_cursor = after;
            report.has_more = page.has_more;
            if !page.has_more {
                break;
            }
        }
        Ok(report)
    }

    async fn persist_local_state(
        &self,
        device_id: &str,
        local: &ArtifactManifestRow,
        observed_version: i64,
    ) -> AppResult<()> {
        self.persist_device_state(
            device_id,
            &local.id,
            observed_version,
            ObjectLocalStatus::Installed,
            Some(local.ciphertext_hash.clone()),
            None,
        )
        .await
    }

    async fn persist_device_state(
        &self,
        device_id: &str,
        artifact_id: &str,
        observed_version: i64,
        local_status: ObjectLocalStatus,
        verified_hash: Option<String>,
        error_code: Option<&str>,
    ) -> AppResult<()> {
        self.db
            .upsert_device_artifact_state(DeviceArtifactStateRow {
                device_id: device_id.into(),
                artifact_id: artifact_id.into(),
                observed_version,
                local_status: local_status_name(local_status).into(),
                verified_hash,
                last_checked_at: now_string(),
                last_error_code: error_code.map(str::to_owned),
            })
            .await
    }

    async fn report_state(
        &self,
        device_id: &str,
        object_id: &str,
        observed_version: i64,
        installed_artifact_id: Option<String>,
        local_status: ObjectLocalStatus,
        verified_ciphertext_hash: Option<String>,
        error_code: Option<&str>,
    ) -> AppResult<ObjectStateResponse> {
        self.transport
            .update_object_state(&ObjectStateRequest {
                protocol_version: PROTOCOL_VERSION,
                device_id: device_id.into(),
                object_id: object_id.into(),
                observed_logical_version: observed_version,
                installed_artifact_id,
                local_status,
                verified_ciphertext_hash,
                error_code: error_code.map(str::to_string),
            })
            .await
            .map_err(transport_error)
    }
}

fn validate_manifest_change(
    change: &super::protocol::ObjectChange,
    response: &ObjectManifestResponse,
    artifact_id: &str,
) -> AppResult<()> {
    let manifest = &response.manifest;
    if manifest.object_id != change.object_id
        || manifest.logical_version < change.logical_version
        || (manifest.logical_version == change.logical_version
            && manifest.artifact_id != artifact_id)
    {
        return Err(AppError::Other(
            "Object manifest does not match its change record".into(),
        ));
    }
    Ok(())
}

fn local_matches_remote(
    local: &ArtifactManifestRow,
    remote: &super::protocol::RemoteObjectManifest,
) -> bool {
    local.id == remote.artifact_id
        && local.artifact_type == remote.object_kind
        && local.ciphertext_hash == remote.ciphertext_hash
        && local.key_version == remote.key_version
        && u64::try_from(local.size).ok() == Some(remote.size)
}

fn local_matches_remote_manifest(
    local: &crate::sync::artifact::ArtifactManifest,
    remote: &super::protocol::RemoteObjectManifest,
) -> bool {
    local.id == remote.artifact_id
        && local.artifact_type == remote.object_kind
        && local.ciphertext_hash == remote.ciphertext_hash
        && local.size == remote.size
        && local.key_version == remote.key_version
}

fn transport_error(error: TransportFailure) -> AppError {
    AppError::Other(format!(
        "Object sync {} error: {}",
        error.code, error.message
    ))
}

fn local_status_name(status: ObjectLocalStatus) -> &'static str {
    match status {
        ObjectLocalStatus::Missing => "missing",
        ObjectLocalStatus::Downloading => "downloading",
        ObjectLocalStatus::Verifying => "verifying",
        ObjectLocalStatus::Installed => "installed",
        ObjectLocalStatus::Failed => "failed",
        ObjectLocalStatus::Incompatible => "incompatible",
    }
}

fn now_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;
    use crate::sync::artifact::{
        ArtifactManifest, ARTIFACT_ENCRYPTION_SCHEME, ARTIFACT_FORMAT_VERSION,
    };
    use crate::sync::client::ObjectSyncTransport;
    use crate::sync::protocol::{
        ObjectChange, ObjectChangesResponse, ObjectManifestResponse, ObjectStateResponse,
        RemoteObjectManifest, RemoteObjectReplica,
    };

    struct FakeTransport {
        states: Mutex<Vec<ObjectStateRequest>>,
        changes: Vec<ObjectChange>,
        manifest: ObjectManifestResponse,
        page_limit: usize,
    }

    #[async_trait]
    impl ObjectSyncTransport for FakeTransport {
        async fn list_object_changes(
            &self,
            after: i64,
            _limit: usize,
        ) -> Result<ObjectChangesResponse, TransportFailure> {
            let available = self
                .changes
                .iter()
                .filter(|change| change.server_seq > after)
                .cloned()
                .collect::<Vec<_>>();
            let changes = available
                .iter()
                .take(self.page_limit)
                .cloned()
                .collect::<Vec<_>>();
            Ok(ObjectChangesResponse {
                next_cursor: changes
                    .last()
                    .map(|change| change.server_seq)
                    .unwrap_or(after),
                has_more: available.len() > changes.len(),
                changes,
            })
        }

        async fn get_object_manifest(
            &self,
            _object_id: &str,
        ) -> Result<ObjectManifestResponse, TransportFailure> {
            Ok(self.manifest.clone())
        }

        async fn update_object_state(
            &self,
            request: &ObjectStateRequest,
        ) -> Result<ObjectStateResponse, TransportFailure> {
            self.states.lock().unwrap().push(request.clone());
            Ok(ObjectStateResponse {
                status: "recorded".into(),
                object_id: request.object_id.clone(),
            })
        }
    }

    struct FakeInstaller {
        fail: bool,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ArtifactInstaller for FakeInstaller {
        async fn download_and_install_remote_artifact(
            &self,
            _account_id: String,
            _descriptor: RemoteArtifactDescriptor,
            _locator: RemoteObjectLocator,
            _master_key: &SyncMasterKey,
            _install_root: PathBuf,
        ) -> AppResult<(PathBuf, ArtifactManifest)> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(AppError::Other("fake download failure".into()));
            }
            Ok((
                PathBuf::from("/tmp/artifact-1"),
                ArtifactManifest {
                    id: "artifact-1".into(),
                    artifact_type: "knowledge_vectors".into(),
                    source_version_id: "version-1".into(),
                    build_fingerprint: "b".repeat(64),
                    format_version: ARTIFACT_FORMAT_VERSION,
                    plaintext_hash: "c".repeat(64),
                    ciphertext_hash: "a".repeat(64),
                    plaintext_size: 1,
                    size: 1,
                    encryption_scheme: ARTIFACT_ENCRYPTION_SCHEME.into(),
                    key_version: 1,
                    chunk_size: 1,
                    chunk_count: 1,
                    created_at: "1".into(),
                },
            ))
        }
    }

    fn change(server_seq: i64, artifact_id: &str, logical_version: i64) -> ObjectChange {
        ObjectChange {
            server_seq,
            object_id: "knowledge:collection-1".into(),
            artifact_id: Some(artifact_id.into()),
            operation: "upsert".into(),
            logical_version,
            changed_at: server_seq,
        }
    }

    fn manifest(
        artifact_id: &str,
        logical_version: i64,
        key_version: i64,
        replicas: Vec<RemoteObjectReplica>,
    ) -> ObjectManifestResponse {
        ObjectManifestResponse {
            manifest: RemoteObjectManifest {
                object_id: "knowledge:collection-1".into(),
                object_kind: "knowledge_vectors".into(),
                logical_version,
                artifact_id: artifact_id.into(),
                ciphertext_hash: "a".repeat(64),
                size: 1,
                key_version,
                updated_hlc: "1-0000-device01".into(),
                deleted_at: None,
                updated_at: logical_version,
            },
            replicas,
        }
    }

    fn ready_replica() -> RemoteObjectReplica {
        RemoteObjectReplica {
            provider_kind: "r2".into(),
            provider_account_id: "r2-managed".into(),
            provider_revision: Some("etag-1".into()),
            etag: Some("etag-1".into()),
            ciphertext_hash: "a".repeat(64),
            size: 1,
            status: "ready".into(),
            updated_at: 1,
        }
    }

    fn test_coordinator(
        transport: Arc<FakeTransport>,
        installer: Arc<FakeInstaller>,
    ) -> (
        crate::db::DbActorHandle,
        ArtifactReplicationCoordinator,
        PathBuf,
    ) {
        let path =
            std::env::temp_dir().join(format!("agnes-replication-{}.db", uuid::Uuid::new_v4()));
        let db = crate::db::spawn_db_actor(path.clone());
        let coordinator = ArtifactReplicationCoordinator::new(
            db.clone(),
            transport,
            installer,
            "r2-managed",
            PathBuf::from("/tmp/agnes-artifacts"),
        )
        .unwrap();
        (db, coordinator, path)
    }

    fn transport(
        changes: Vec<ObjectChange>,
        manifest: ObjectManifestResponse,
    ) -> Arc<FakeTransport> {
        Arc::new(FakeTransport {
            states: Mutex::new(Vec::new()),
            changes,
            manifest,
            page_limit: 100,
        })
    }

    #[tokio::test]
    async fn coordinator_installs_reports_and_advances_object_cursor() {
        let transport = transport(
            vec![change(1, "artifact-1", 1)],
            manifest("artifact-1", 1, 1, vec![ready_replica()]),
        );
        let installer = Arc::new(FakeInstaller {
            fail: false,
            calls: AtomicUsize::new(0),
        });
        let (db, coordinator, path) = test_coordinator(transport.clone(), installer);
        let report = coordinator
            .run_once(&SyncKeyset::generate_initial())
            .await
            .unwrap();
        assert_eq!(report.downloaded, 1);
        assert_eq!(report.next_cursor, 1);
        assert_eq!(db.get_sync_status().await.unwrap().last_object_cursor, 1);
        let states = transport.states.lock().unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].local_status, ObjectLocalStatus::Downloading);
        assert_eq!(states[1].local_status, ObjectLocalStatus::Installed);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn missing_key_reports_incompatible_and_advances_cursor() {
        let transport = transport(
            vec![change(1, "artifact-2", 1)],
            manifest("artifact-2", 1, 2, vec![ready_replica()]),
        );
        let installer = Arc::new(FakeInstaller {
            fail: false,
            calls: AtomicUsize::new(0),
        });
        let installer_ref = installer.clone();
        let (db, coordinator, path) = test_coordinator(transport.clone(), installer);
        let report = coordinator
            .run_once(&SyncKeyset::generate_initial())
            .await
            .unwrap();
        assert_eq!(report.incompatible, 1);
        assert_eq!(report.next_cursor, 1);
        assert_eq!(installer_ref.calls.load(Ordering::SeqCst), 0);
        let states = transport.states.lock().unwrap();
        assert_eq!(states[0].local_status, ObjectLocalStatus::Incompatible);
        assert_eq!(
            states[0].error_code.as_deref(),
            Some("KEY_VERSION_UNAVAILABLE")
        );
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn no_ready_replica_reports_missing_and_advances_cursor() {
        let transport = transport(
            vec![change(1, "artifact-1", 1)],
            manifest("artifact-1", 1, 1, Vec::new()),
        );
        let installer = Arc::new(FakeInstaller {
            fail: false,
            calls: AtomicUsize::new(0),
        });
        let installer_ref = installer.clone();
        let (db, coordinator, path) = test_coordinator(transport.clone(), installer);
        let report = coordinator
            .run_once(&SyncKeyset::generate_initial())
            .await
            .unwrap();
        assert_eq!(report.missing, 1);
        assert_eq!(installer_ref.calls.load(Ordering::SeqCst), 0);
        let states = transport.states.lock().unwrap();
        assert_eq!(states[0].local_status, ObjectLocalStatus::Missing);
        assert_eq!(states[0].error_code.as_deref(), Some("NO_READY_REPLICA"));
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn download_failure_reports_failed_without_advancing_cursor() {
        let transport = transport(
            vec![change(1, "artifact-1", 1)],
            manifest("artifact-1", 1, 1, vec![ready_replica()]),
        );
        let installer = Arc::new(FakeInstaller {
            fail: true,
            calls: AtomicUsize::new(0),
        });
        let (db, coordinator, path) = test_coordinator(transport.clone(), installer);
        assert!(coordinator
            .run_once(&SyncKeyset::generate_initial())
            .await
            .is_err());
        assert_eq!(db.get_sync_status().await.unwrap().last_object_cursor, 0);
        let states = transport.states.lock().unwrap();
        assert_eq!(states[0].local_status, ObjectLocalStatus::Downloading);
        assert_eq!(states[1].local_status, ObjectLocalStatus::Failed);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn newer_manifest_allows_an_older_change_in_the_same_page_to_converge() {
        let transport = transport(
            vec![change(1, "artifact-old", 1), change(2, "artifact-1", 2)],
            manifest("artifact-1", 2, 1, vec![ready_replica()]),
        );
        let installer = Arc::new(FakeInstaller {
            fail: false,
            calls: AtomicUsize::new(0),
        });
        let (db, coordinator, path) = test_coordinator(transport, installer);
        let report = coordinator
            .run_once(&SyncKeyset::generate_initial())
            .await
            .unwrap();
        assert_eq!(report.processed, 2);
        assert_eq!(report.downloaded, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.next_cursor, 2);
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn object_pull_is_bounded_to_five_pages_per_run() {
        let transport = Arc::new(FakeTransport {
            states: Mutex::new(Vec::new()),
            changes: (1..=7).map(|seq| change(seq, "artifact-1", 1)).collect(),
            manifest: manifest("artifact-1", 1, 1, vec![ready_replica()]),
            page_limit: 1,
        });
        let installer = Arc::new(FakeInstaller {
            fail: false,
            calls: AtomicUsize::new(0),
        });
        let (db, coordinator, path) = test_coordinator(transport, installer);
        let report = coordinator
            .run_once(&SyncKeyset::generate_initial())
            .await
            .unwrap();
        assert_eq!(report.processed, 5);
        assert_eq!(report.next_cursor, 5);
        assert!(report.has_more);
        assert_eq!(db.get_sync_status().await.unwrap().last_object_cursor, 5);
        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
