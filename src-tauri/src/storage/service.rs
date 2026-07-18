use std::sync::Arc;

use serde::Serialize;
use zeroize::Zeroizing;

use crate::db::repo::storage::{StorageAccountRow, StorageTransferJobRow, UpsertStorageAccount};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

use super::domain::{
    ListFilesRequest, ProviderDescriptor, ProviderError, ProviderQuota, RemoteFilePage,
    StorageProviderAccount,
};
use super::ports::{ProviderCredentialStore, ProviderSession};
use super::registry::StorageProviderRegistry;
use super::ScopedProviderCredentialAccess;

#[derive(Debug, Clone, Serialize)]
pub struct StorageAccountView {
    #[serde(flatten)]
    pub account: StorageAccountRow,
    pub provider_installed: bool,
    pub has_credential: bool,
}

pub struct StorageService {
    db: DbActorHandle,
    registry: Arc<StorageProviderRegistry>,
    credentials: Arc<dyn ProviderCredentialStore>,
}

impl StorageService {
    pub fn new(
        db: DbActorHandle,
        registry: Arc<StorageProviderRegistry>,
        credentials: Arc<dyn ProviderCredentialStore>,
    ) -> Self {
        Self {
            db,
            registry,
            credentials,
        }
    }

    pub fn catalog(&self) -> AppResult<Vec<ProviderDescriptor>> {
        self.registry.descriptors().map_err(provider_error)
    }

    pub async fn list_accounts(&self) -> AppResult<Vec<StorageAccountView>> {
        let rows = self.db.list_storage_accounts().await?;
        let mut accounts = Vec::with_capacity(rows.len());
        for account in rows {
            let provider_installed = self.registry.factory(&account.provider_id).is_ok();
            let has_credential = self
                .credentials
                .load(&account.id)
                .await
                .map_err(provider_error)?
                .is_some();
            accounts.push(StorageAccountView {
                account,
                provider_installed,
                has_credential,
            });
        }
        Ok(accounts)
    }

    pub async fn list_files(
        &self,
        account_id: String,
        request: ListFilesRequest,
    ) -> AppResult<RemoteFilePage> {
        let session = self.connect_account(&account_id).await?;
        let file_source = session.file_source().ok_or_else(|| {
            AppError::Other("Storage provider does not support file browsing".into())
        })?;
        file_source
            .list_files(request.normalized())
            .await
            .map_err(provider_error)
    }

    pub async fn refresh_quota(&self, account_id: String) -> AppResult<ProviderQuota> {
        let (row, session) = self.connect_account_with_row(&account_id).await?;
        let quota_source = session.quota_source().ok_or_else(|| {
            AppError::Other("Storage provider does not expose account quota".into())
        })?;
        let quota = quota_source.quota().await.map_err(provider_error)?;
        self.db
            .update_storage_binding(
                row.id,
                row.auth_state,
                row.enabled,
                row.capabilities_json,
                quota.used_bytes.map(as_sqlite_i64).transpose()?,
                quota.total_bytes.map(as_sqlite_i64).transpose()?,
                None,
            )
            .await?;
        Ok(quota)
    }

    pub async fn list_transfers(
        &self,
        account_id: Option<String>,
        limit: usize,
    ) -> AppResult<Vec<StorageTransferJobRow>> {
        self.db
            .list_storage_transfer_jobs(account_id, limit.clamp(1, 200))
            .await
    }

    #[allow(dead_code)]
    pub async fn save_connected_account(
        &self,
        account: StorageProviderAccount,
        credential: Zeroizing<String>,
    ) -> AppResult<()> {
        account.validate().map_err(provider_error)?;
        let descriptor = self
            .registry
            .descriptor(&account.provider_id)
            .map_err(provider_error)?;
        let config_json = serde_json::to_string(&account.config)?;
        let capabilities_json = serde_json::to_string(&descriptor.capabilities)?;
        let previous_credential = self
            .credentials
            .load(&account.id)
            .await
            .map_err(provider_error)?;
        let expected_credential = credential.clone();
        self.credentials
            .store(&account.id, credential)
            .await
            .map_err(provider_error)?;
        let verified = match self.credentials.load(&account.id).await {
            Ok(value) => value,
            Err(error) => {
                let verification_error = provider_error(error);
                if let Err(rollback_error) =
                    restore_credential(self.credentials.as_ref(), &account.id, previous_credential)
                        .await
                {
                    return Err(AppError::SecretStore(format!(
                        "storage credential verification failed: {verification_error}; rollback failed: {rollback_error}"
                    )));
                }
                return Err(verification_error);
            }
        };
        if verified.as_ref().map(|value| value.as_str()) != Some(expected_credential.as_str()) {
            restore_credential(self.credentials.as_ref(), &account.id, previous_credential).await?;
            return Err(AppError::Other(
                "Storage credential verification failed".into(),
            ));
        }
        let result = self
            .db
            .upsert_storage_account(UpsertStorageAccount {
                id: account.id.clone(),
                provider_id: account.provider_id,
                display_name: account.display_name,
                account_subject: account.account_subject,
                config_json,
                auth_state: "connected".into(),
                enabled: true,
                capabilities_json,
            })
            .await;
        if let Err(error) = result {
            restore_credential(self.credentials.as_ref(), &account.id, previous_credential).await?;
            return Err(error);
        }
        Ok(())
    }

    pub async fn remove_account(&self, account_id: String) -> AppResult<()> {
        let previous_credential = self
            .credentials
            .load(&account_id)
            .await
            .map_err(provider_error)?;
        self.credentials
            .delete(&account_id)
            .await
            .map_err(provider_error)?;
        if let Err(error) = self.db.delete_storage_account(account_id.clone()).await {
            restore_credential(self.credentials.as_ref(), &account_id, previous_credential).await?;
            return Err(error);
        }
        Ok(())
    }

    async fn connect_account(&self, account_id: &str) -> AppResult<Arc<dyn ProviderSession>> {
        self.connect_account_with_row(account_id)
            .await
            .map(|(_, session)| session)
    }

    async fn connect_account_with_row(
        &self,
        account_id: &str,
    ) -> AppResult<(StorageAccountRow, Arc<dyn ProviderSession>)> {
        let row = self
            .db
            .get_storage_account(account_id.to_string())
            .await?
            .ok_or_else(|| AppError::Other("Storage provider account was not found".into()))?;
        if !row.enabled {
            return Err(AppError::Other(
                "Storage provider account is disabled".into(),
            ));
        }
        if row.auth_state != "connected" {
            return Err(AppError::Other(format!(
                "Storage provider account requires authorization ({})",
                row.auth_state
            )));
        }
        let account = StorageProviderAccount {
            id: row.id.clone(),
            provider_id: row.provider_id.clone(),
            display_name: row.display_name.clone(),
            account_subject: row.account_subject.clone(),
            config: serde_json::from_str(&row.config_json)?,
        };
        account.validate().map_err(provider_error)?;
        let factory = self
            .registry
            .factory(&row.provider_id)
            .map_err(provider_error)?;
        let credential_access = Arc::new(
            ScopedProviderCredentialAccess::new(row.id.clone(), self.credentials.clone())
                .map_err(provider_error)?,
        );
        let session = factory
            .connect(&account, credential_access)
            .await
            .map_err(provider_error)?;
        Ok((row, session))
    }
}

fn provider_error(error: ProviderError) -> AppError {
    AppError::Other(format!(
        "Storage provider error ({}): {}",
        error.category.as_str(),
        error.message,
    ))
}

fn as_sqlite_i64(value: u64) -> AppResult<i64> {
    i64::try_from(value)
        .map_err(|_| AppError::Other("Storage quota exceeds the supported local range".into()))
}

async fn restore_credential(
    credentials: &dyn ProviderCredentialStore,
    account_id: &str,
    previous: Option<Zeroizing<String>>,
) -> AppResult<()> {
    match previous {
        Some(value) => credentials.store(account_id, value).await,
        None => credentials.delete(account_id).await,
    }
    .map_err(provider_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures_util::stream;

    use crate::secrets::InMemorySecretStore;
    use crate::storage::domain::{
        DownloadFileRequest, ProviderAuthKind, ProviderByteStream, ProviderResult,
        ProviderStability, RemoteFileItem, RemoteFileKind, StorageCapabilities,
    };
    use crate::storage::ports::{
        FileSourceProvider, ProviderCredentialAccess, ProviderFactory, QuotaProvider,
    };

    struct FakeDrive;

    #[async_trait]
    impl FileSourceProvider for FakeDrive {
        async fn list_files(&self, request: ListFilesRequest) -> ProviderResult<RemoteFilePage> {
            assert_eq!(request.page_size, 200);
            Ok(RemoteFilePage {
                items: vec![self.get_file("remote-1").await?],
                next_page_token: None,
            })
        }

        async fn get_file(&self, file_id: &str) -> ProviderResult<RemoteFileItem> {
            Ok(RemoteFileItem {
                id: file_id.into(),
                parent_id: None,
                name: "Notes.md".into(),
                kind: RemoteFileKind::File,
                media_type: Some("text/markdown".into()),
                size: Some(128),
                modified_at: None,
                revision: Some("revision-1".into()),
                downloadable: true,
            })
        }

        async fn download_file(
            &self,
            _request: DownloadFileRequest,
        ) -> ProviderResult<ProviderByteStream> {
            Ok(Box::pin(stream::empty()))
        }
    }

    #[async_trait]
    impl QuotaProvider for FakeDrive {
        async fn quota(&self) -> ProviderResult<ProviderQuota> {
            Ok(ProviderQuota {
                used_bytes: Some(128),
                total_bytes: Some(1024),
                trashed_bytes: None,
                checked_at: "1".into(),
            })
        }
    }

    impl ProviderSession for FakeDrive {
        fn file_source(&self) -> Option<&dyn FileSourceProvider> {
            Some(self)
        }

        fn quota_source(&self) -> Option<&dyn QuotaProvider> {
            Some(self)
        }
    }

    struct FakeFactory;

    #[async_trait]
    impl ProviderFactory for FakeFactory {
        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor {
                id: "future_drive".into(),
                display_name: "Future Drive".into(),
                auth_kind: ProviderAuthKind::ApiToken,
                stability: ProviderStability::Experimental,
                implementation_version: "test-v1".into(),
                capabilities: StorageCapabilities {
                    browse_files: true,
                    read_files: true,
                    quota: true,
                    user_authorization: true,
                    ..StorageCapabilities::default()
                },
            }
        }

        async fn connect(
            &self,
            _account: &StorageProviderAccount,
            credentials: Arc<dyn ProviderCredentialAccess>,
        ) -> ProviderResult<Arc<dyn ProviderSession>> {
            assert_eq!(credentials.load().await?.unwrap().as_str(), "credential");
            Ok(Arc::new(FakeDrive))
        }
    }

    #[tokio::test]
    async fn application_service_depends_only_on_registered_provider_ports() {
        let path =
            std::env::temp_dir().join(format!("agnes-storage-service-{}.db", uuid::Uuid::new_v4()));
        let db = crate::db::spawn_db_actor(path.clone());
        let registry = Arc::new(StorageProviderRegistry::new());
        registry.register(Arc::new(FakeFactory)).unwrap();
        let credential_store = Arc::new(super::super::KeyringProviderCredentialStore::new(
            Arc::new(InMemorySecretStore::default()),
        ));
        let service = StorageService::new(db, registry, credential_store);
        service
            .save_connected_account(
                StorageProviderAccount {
                    id: "account-1".into(),
                    provider_id: "future_drive".into(),
                    display_name: "Personal".into(),
                    account_subject: None,
                    config: serde_json::json!({}),
                },
                Zeroizing::new("credential".into()),
            )
            .await
            .unwrap();

        let accounts = service.list_accounts().await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert!(accounts[0].provider_installed);
        assert!(accounts[0].has_credential);
        let files = service
            .list_files(
                "account-1".into(),
                ListFilesRequest {
                    parent_id: None,
                    page_token: None,
                    page_size: usize::MAX,
                },
            )
            .await
            .unwrap();
        assert_eq!(files.items[0].name, "Notes.md");
        let quota = service.refresh_quota("account-1".into()).await.unwrap();
        assert_eq!(quota.total_bytes, Some(1024));
        service.remove_account("account-1".into()).await.unwrap();
        assert!(service.list_accounts().await.unwrap().is_empty());

        drop(service);
        let _ = std::fs::remove_file(path);
    }
}
