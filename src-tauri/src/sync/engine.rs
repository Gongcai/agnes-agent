use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::db::repo::sync::{AcceptedChange, ConflictChange, SyncDbStatus};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::secrets::{SecretStore, SYNC_CREDENTIAL_SECRET_ID};
use crate::sync::auth::SyncCredential;
use crate::sync::client::{FailureKind, HttpSyncTransport, SyncTransport, TransportFailure};
use crate::sync::protocol::{PushRequest, PushResponse};

pub const SYNC_GATEWAY_URL: &str = "https://agnes-sync-api.caiwengong136.workers.dev";
const MAX_BATCHES_PER_RUN: usize = 5;
const MAX_PUSH_CHANGES: usize = 20;

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
        } else if database.pending_count > 0 || database.in_flight_count > 0 {
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
        let Some(secret) = self.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await? else {
            return self.status().await;
        };
        let credential = SyncCredential::parse(&secret)?;
        let transport = HttpSyncTransport::new(self.client.clone(), SYNC_GATEWAY_URL, credential);
        self.run_with_transport(&transport).await
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
                    break;
                }
            }
        }
        Ok(())
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::db::repo::agents::NewAgent;
    use crate::secrets::InMemorySecretStore;
    use crate::sync::client::TransportFailure;
    use crate::sync::protocol::{AcceptedChange as ProtocolAccepted, PushResponse};

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
}
