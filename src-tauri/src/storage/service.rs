use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use md5::Digest;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

use crate::db::repo::storage::{
    NewStorageTransferJob, StorageAccountRow, StorageTransferJobRow, StorageTransferProgress,
    UpsertStorageAccount,
};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

use super::domain::{
    BeginFileUploadRequest, ContentHashAlgorithm, ListFilesRequest, ProviderAuthorizationRequest,
    ProviderDescriptor, ProviderError, ProviderQuota, RemoteFileItem, RemoteFileKind,
    RemoteFilePage, StorageProviderAccount, UploadFileChunkRequest,
};
use super::ports::{
    ProviderAuthorizationChallenge, ProviderAuthorizationStep, ProviderCredentialStore,
    ProviderSession,
};
use super::registry::StorageProviderRegistry;
use super::ScopedProviderCredentialAccess;

#[derive(Debug, Clone, Serialize)]
pub struct StorageAccountView {
    #[serde(flatten)]
    pub account: StorageAccountRow,
    pub provider_installed: bool,
    pub has_credential: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageAuthorizationProgress {
    pub status: String,
    pub account_id: Option<String>,
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
        let (row, session) = self.connect_account_with_row(&account_id).await?;
        let file_source = session.file_source().ok_or_else(|| {
            AppError::Other("Storage provider does not support file browsing".into())
        })?;
        match file_source.list_files(request.normalized()).await {
            Ok(page) => {
                self.update_provider_status(&row, None).await;
                Ok(page)
            }
            Err(error) => {
                self.update_provider_status(&row, Some(&error)).await;
                Err(provider_error(error))
            }
        }
    }

    pub async fn refresh_quota(&self, account_id: String) -> AppResult<ProviderQuota> {
        let (row, session) = self.connect_account_with_row(&account_id).await?;
        let quota_source = session.quota_source().ok_or_else(|| {
            AppError::Other("Storage provider does not expose account quota".into())
        })?;
        let quota = match quota_source.quota().await {
            Ok(quota) => quota,
            Err(error) => {
                self.update_provider_status(&row, Some(&error)).await;
                return Err(provider_error(error));
            }
        };
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

    pub async fn download_file(
        &self,
        account_id: String,
        file_id: String,
        expected_revision: Option<String>,
        expected_size: Option<u64>,
        destination: String,
    ) -> AppResult<()> {
        let destination = std::path::PathBuf::from(destination);
        validate_download_destination(&destination)?;
        let (row, session) = self.connect_account_with_row(&account_id).await?;
        let file_source = session.file_source().ok_or_else(|| {
            AppError::Other("Storage provider does not support file downloads".into())
        })?;
        let remote_file = match file_source.get_file(&file_id).await {
            Ok(file) => file,
            Err(error) => {
                self.update_provider_status(&row, Some(&error)).await;
                return Err(provider_error(error));
            }
        };
        let descriptor = self
            .registry
            .descriptor(&row.provider_id)
            .map_err(provider_error)?;
        let job_id = uuid::Uuid::new_v4().to_string();
        let bytes_total = descriptor
            .capabilities
            .stable_file_sizes
            .then(|| remote_file.size)
            .flatten()
            .or_else(|| expected_size)
            .and_then(|value| i64::try_from(value).ok());
        self.db
            .insert_storage_transfer_job(NewStorageTransferJob {
                id: job_id.clone(),
                account_id: row.id.clone(),
                operation: "file_download".into(),
                remote_item_id: Some(file_id.clone()),
                display_name: remote_file.name,
                destination_kind: Some("local_file".into()),
                destination_id: Some(destination.to_string_lossy().to_string()),
                bytes_total,
            })
            .await?;
        self.update_transfer(&job_id, "running", 0, bytes_total, None)
            .await?;

        let result = self
            .stream_file_to_destination(
                file_source,
                super::domain::DownloadFileRequest {
                    file_id,
                    range_start: None,
                    range_end_inclusive: None,
                    expected_revision,
                },
                &destination,
                &job_id,
                bytes_total,
                descriptor.capabilities.stable_file_sizes,
            )
            .await;
        match result {
            Ok(bytes_transferred) => {
                self.update_transfer(
                    &job_id,
                    "completed",
                    bytes_transferred,
                    if descriptor.capabilities.stable_file_sizes {
                        bytes_total.or(Some(bytes_transferred))
                    } else {
                        Some(bytes_transferred)
                    },
                    None,
                )
                .await?;
                self.update_provider_status(&row, None).await;
                Ok(())
            }
            Err(DownloadFailure::Provider(error)) => {
                self.update_provider_status(&row, Some(&error)).await;
                let app_error = provider_error(error.clone());
                let _ = self
                    .update_transfer(
                        &job_id,
                        "failed",
                        0,
                        bytes_total,
                        Some((error.category.as_str(), error.message.as_str())),
                    )
                    .await;
                Err(app_error)
            }
            Err(DownloadFailure::Local(error)) => {
                let message = error.to_string();
                let _ = self
                    .update_transfer(
                        &job_id,
                        "failed",
                        0,
                        bytes_total,
                        Some(("local_io", message.as_str())),
                    )
                    .await;
                Err(error)
            }
        }
    }

    pub async fn download_folder(
        &self,
        account_id: String,
        folder_id: String,
        folder_name: String,
        destination_directory: String,
    ) -> AppResult<usize> {
        const MAX_FOLDER_DEPTH: usize = 64;
        const MAX_FOLDER_ITEMS: usize = 10_000;

        let destination_directory = std::path::PathBuf::from(destination_directory);
        if !destination_directory.is_absolute() || !destination_directory.is_dir() {
            return Err(AppError::Other(
                "Folder download destination must be an existing absolute directory".into(),
            ));
        }
        let mut planned_paths = std::collections::HashSet::new();
        let root = available_child_path(
            &destination_directory,
            &safe_local_name(&folder_name),
            &folder_id,
            &mut planned_paths,
        );
        tokio::fs::create_dir_all(&root).await?;
        let mut queue = std::collections::VecDeque::from([(folder_id, root, 0_usize)]);
        let mut visited = std::collections::HashSet::new();
        let mut discovered = 0_usize;
        let mut downloaded = 0_usize;

        while let Some((current_id, local_directory, depth)) = queue.pop_front() {
            if depth > MAX_FOLDER_DEPTH {
                return Err(AppError::Other(
                    "Storage folder exceeds the recursive download depth limit".into(),
                ));
            }
            if !visited.insert(current_id.clone()) {
                continue;
            }
            let mut page_token = None;
            loop {
                let page = self
                    .list_files(
                        account_id.clone(),
                        ListFilesRequest {
                            parent_id: Some(current_id.clone()),
                            page_token,
                            page_size: 200,
                        },
                    )
                    .await?;
                for item in page.items {
                    discovered += 1;
                    if discovered > MAX_FOLDER_ITEMS {
                        return Err(AppError::Other(
                            "Storage folder exceeds the 10000 item download limit".into(),
                        ));
                    }
                    let path = available_child_path(
                        &local_directory,
                        &safe_local_name(&item.name),
                        &item.id,
                        &mut planned_paths,
                    );
                    match item.kind {
                        RemoteFileKind::Folder => {
                            tokio::fs::create_dir_all(&path).await?;
                            queue.push_back((item.id, path, depth + 1));
                        }
                        RemoteFileKind::File if item.downloadable => {
                            self.download_file(
                                account_id.clone(),
                                item.id,
                                item.revision,
                                item.size,
                                path.to_string_lossy().to_string(),
                            )
                            .await?;
                            downloaded += 1;
                        }
                        RemoteFileKind::File | RemoteFileKind::Shortcut => {}
                    }
                }
                page_token = page.next_page_token;
                if page_token.is_none() {
                    break;
                }
            }
        }
        Ok(downloaded)
    }

    pub async fn upload_file(
        &self,
        account_id: String,
        parent_id: Option<String>,
        source: String,
    ) -> AppResult<RemoteFileItem> {
        const DEFAULT_UPLOAD_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

        let source = std::path::PathBuf::from(source);
        if !source.is_absolute() {
            return Err(AppError::Other(
                "Upload source must be an absolute file path".into(),
            ));
        }
        let metadata = tokio::fs::metadata(&source).await?;
        if !metadata.is_file() {
            return Err(AppError::Other(
                "Upload source must be a regular file".into(),
            ));
        }
        let name = source
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Other("Upload source has no file name".into()))?;
        let size = metadata.len();
        let bytes_total = i64::try_from(size)
            .map_err(|_| AppError::Other("Upload file exceeds the local transfer range".into()))?;
        let media_type = mime_guess::from_path(&source)
            .first_or_octet_stream()
            .essence_str()
            .to_string();
        let (row, session) = self.connect_account_with_row(&account_id).await?;
        let descriptor = self
            .registry
            .descriptor(&row.provider_id)
            .map_err(provider_error)?;
        let content_hashes =
            hash_local_file(&source, &descriptor.capabilities.required_upload_hashes).await?;
        let upload_chunk_bytes = descriptor
            .capabilities
            .recommended_chunk_bytes
            .unwrap_or(DEFAULT_UPLOAD_CHUNK_BYTES)
            .clamp(256 * 1024, 32 * 1024 * 1024);
        let uploader = session.file_upload().ok_or_else(|| {
            AppError::Other("Storage provider does not support user file uploads".into())
        })?;
        let job_id = uuid::Uuid::new_v4().to_string();
        self.db
            .insert_storage_transfer_job(NewStorageTransferJob {
                id: job_id.clone(),
                account_id: row.id.clone(),
                operation: "file_upload".into(),
                remote_item_id: None,
                display_name: name.clone(),
                destination_kind: Some("remote_folder".into()),
                destination_id: parent_id.clone(),
                bytes_total: Some(bytes_total),
            })
            .await?;
        self.update_transfer(&job_id, "running", 0, Some(bytes_total), None)
            .await?;

        let session = match uploader
            .begin_file_upload(BeginFileUploadRequest {
                parent_id,
                name,
                size,
                media_type,
                chunk_size: upload_chunk_bytes,
                content_hashes,
            })
            .await
        {
            Ok(session) => session,
            Err(error) => {
                self.finish_upload_failure(&row, &job_id, 0, bytes_total, &error)
                    .await;
                return Err(provider_error(error));
            }
        };
        if session.next_offset != 0 {
            let error = ProviderError::new(
                super::domain::ProviderErrorCategory::InvalidResponse,
                "Storage provider started a new upload at a non-zero offset",
            );
            let _ = uploader.abort_file_upload(&session.session_id).await;
            self.finish_upload_failure(&row, &job_id, 0, bytes_total, &error)
                .await;
            return Err(provider_error(error));
        }

        let upload_result = self
            .stream_file_to_provider(
                uploader,
                FileUploadStream {
                    source: &source,
                    session_id: &session.session_id,
                    size,
                    chunk_size: upload_chunk_bytes,
                    job_id: &job_id,
                    bytes_total,
                },
            )
            .await;
        match upload_result {
            Ok(file) => {
                self.update_transfer(&job_id, "completed", bytes_total, Some(bytes_total), None)
                    .await?;
                self.update_provider_status(&row, None).await;
                Ok(file)
            }
            Err(UploadFailure::Provider { error, transferred }) => {
                let _ = uploader.abort_file_upload(&session.session_id).await;
                self.finish_upload_failure(&row, &job_id, transferred, bytes_total, &error)
                    .await;
                Err(provider_error(error))
            }
            Err(UploadFailure::Local { error, transferred }) => {
                let _ = uploader.abort_file_upload(&session.session_id).await;
                let message = error.to_string();
                let _ = self
                    .update_transfer(
                        &job_id,
                        "failed",
                        transferred,
                        Some(bytes_total),
                        Some(("local_io", message.as_str())),
                    )
                    .await;
                Err(error)
            }
        }
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

    pub async fn trash_files(&self, account_id: String, file_ids: Vec<String>) -> AppResult<usize> {
        let mut unique_ids = file_ids
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        unique_ids.sort();
        unique_ids.dedup();
        if unique_ids.is_empty() || unique_ids.len() > 100 {
            return Err(AppError::Other(
                "Moving to trash requires between 1 and 100 file IDs".into(),
            ));
        }
        let (row, session) = self.connect_account_with_row(&account_id).await?;
        let manager = session.file_management().ok_or_else(|| {
            AppError::Other("Storage provider does not support moving files to trash".into())
        })?;
        match manager.trash_files(unique_ids.clone()).await {
            Ok(()) => {
                self.update_provider_status(&row, None).await;
                Ok(unique_ids.len())
            }
            Err(error) => {
                self.update_provider_status(&row, Some(&error)).await;
                Err(provider_error(error))
            }
        }
    }

    pub async fn authorize_account(
        &self,
        provider_id: String,
        request: ProviderAuthorizationRequest,
    ) -> AppResult<String> {
        let factory = self
            .registry
            .factory(&provider_id)
            .map_err(provider_error)?;
        let descriptor = self
            .registry
            .descriptor(&provider_id)
            .map_err(provider_error)?;
        if !descriptor.capabilities.user_authorization {
            return Err(AppError::Other(
                "Storage provider does not support interactive authorization".into(),
            ));
        }
        let authorized = factory.authorize(request).await.map_err(provider_error)?;
        if authorized.account.provider_id != provider_id {
            return Err(AppError::Other(
                "Storage provider authorization returned a mismatched provider ID".into(),
            ));
        }
        let account_id = authorized.account.id.clone();
        self.save_connected_account(authorized.account, authorized.credential)
            .await?;
        Ok(account_id)
    }

    pub async fn begin_authorization(
        &self,
        provider_id: String,
        request: ProviderAuthorizationRequest,
    ) -> AppResult<ProviderAuthorizationChallenge> {
        let factory = self
            .registry
            .factory(&provider_id)
            .map_err(provider_error)?;
        let descriptor = self
            .registry
            .descriptor(&provider_id)
            .map_err(provider_error)?;
        if !descriptor.capabilities.user_authorization {
            return Err(AppError::Other(
                "Storage provider does not support interactive authorization".into(),
            ));
        }
        let challenge = factory
            .begin_authorization(request)
            .await
            .map_err(provider_error)?;
        if challenge.provider_id != provider_id {
            return Err(AppError::Other(
                "Storage provider authorization returned a mismatched provider ID".into(),
            ));
        }
        Ok(challenge)
    }

    pub async fn poll_authorization(
        &self,
        provider_id: String,
        challenge_id: String,
    ) -> AppResult<StorageAuthorizationProgress> {
        let factory = self
            .registry
            .factory(&provider_id)
            .map_err(provider_error)?;
        let step = factory
            .poll_authorization(&challenge_id)
            .await
            .map_err(provider_error)?;
        match step {
            ProviderAuthorizationStep::Pending => Ok(StorageAuthorizationProgress {
                status: "pending".into(),
                account_id: None,
            }),
            ProviderAuthorizationStep::Authorized(authorized) => {
                if authorized.account.provider_id != provider_id {
                    return Err(AppError::Other(
                        "Storage provider authorization returned a mismatched provider ID".into(),
                    ));
                }
                let account_id = authorized.account.id.clone();
                self.save_connected_account(authorized.account, authorized.credential)
                    .await?;
                Ok(StorageAuthorizationProgress {
                    status: "completed".into(),
                    account_id: Some(account_id),
                })
            }
        }
    }

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
        let session = match factory.connect(&account, credential_access).await {
            Ok(session) => session,
            Err(error) => {
                self.update_provider_status(&row, Some(&error)).await;
                return Err(provider_error(error));
            }
        };
        Ok((row, session))
    }

    async fn update_provider_status(&self, row: &StorageAccountRow, error: Option<&ProviderError>) {
        let auth_state = if error.is_some_and(|error| {
            error.category == super::domain::ProviderErrorCategory::Authentication
        }) {
            "auth_required"
        } else {
            row.auth_state.as_str()
        };
        let normalized_error = error.map(|error| {
            (
                error.category.as_str().to_string(),
                error.message.chars().take(2048).collect::<String>(),
            )
        });
        let _ = self
            .db
            .update_storage_binding(
                row.id.clone(),
                auth_state.into(),
                row.enabled,
                row.capabilities_json.clone(),
                row.quota_used_bytes,
                row.quota_total_bytes,
                normalized_error,
            )
            .await;
    }

    async fn stream_file_to_destination(
        &self,
        file_source: &dyn super::ports::FileSourceProvider,
        request: super::domain::DownloadFileRequest,
        destination: &std::path::Path,
        job_id: &str,
        bytes_total: Option<i64>,
        exact_size: bool,
    ) -> Result<i64, DownloadFailure> {
        let parent = destination.parent().ok_or_else(|| {
            DownloadFailure::Local(AppError::Other("Invalid download destination".into()))
        })?;
        let temporary = parent.join(format!(".agnes-download-{}.partial", uuid::Uuid::new_v4()));
        let result = async {
            let mut output = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await
                .map_err(AppError::from)?;
            let mut stream = file_source
                .download_file(request)
                .await
                .map_err(DownloadFailure::Provider)?;
            let mut transferred = 0_i64;
            let mut last_reported = 0_i64;
            let mut last_reported_at = Instant::now();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(DownloadFailure::Provider)?;
                output.write_all(&chunk).await.map_err(AppError::from)?;
                let chunk_size = i64::try_from(chunk.len()).map_err(|_| {
                    AppError::Other("Downloaded storage file exceeds the local size range".into())
                })?;
                transferred = transferred.checked_add(chunk_size).ok_or_else(|| {
                    AppError::Other("Downloaded storage file exceeds the local size range".into())
                })?;
                let mut progress_total = bytes_total;
                if progress_total.is_some_and(|total| transferred > total) {
                    progress_total = Some(transferred);
                }
                if transferred - last_reported >= 1024 * 1024
                    || last_reported == 0
                    || last_reported_at.elapsed() >= Duration::from_millis(250)
                {
                    self.update_transfer(job_id, "running", transferred, progress_total, None)
                        .await?;
                    last_reported = transferred;
                    last_reported_at = Instant::now();
                }
            }
            if exact_size && bytes_total.is_some_and(|expected| transferred != expected) {
                return Err(DownloadFailure::Provider(ProviderError::new(
                    super::domain::ProviderErrorCategory::InvalidResponse,
                    "Storage provider download ended before the declared file size",
                )));
            }
            output.flush().await.map_err(AppError::from)?;
            output.sync_all().await.map_err(AppError::from)?;
            drop(output);
            install_download(&temporary, destination).await?;
            Ok::<_, DownloadFailure>(transferred)
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        result
    }

    async fn update_transfer(
        &self,
        job_id: &str,
        status: &str,
        bytes_transferred: i64,
        bytes_total: Option<i64>,
        error: Option<(&str, &str)>,
    ) -> AppResult<()> {
        self.db
            .update_storage_transfer_job(
                job_id.to_string(),
                StorageTransferProgress {
                    status: status.into(),
                    bytes_transferred,
                    bytes_total,
                    error_category: error.map(|value| value.0.to_string()),
                    error_message: error.map(|value| value.1.chars().take(2048).collect()),
                },
            )
            .await
    }

    async fn stream_file_to_provider(
        &self,
        uploader: &dyn super::ports::FileUploadProvider,
        request: FileUploadStream<'_>,
    ) -> Result<RemoteFileItem, UploadFailure> {
        let mut input = tokio::fs::File::open(request.source)
            .await
            .map_err(|error| UploadFailure::local(error.into(), 0))?;
        let mut offset = 0_u64;
        loop {
            let remaining = request.size.saturating_sub(offset);
            let current_size = remaining.min(request.chunk_size);
            let mut bytes = vec![
                0_u8;
                usize::try_from(current_size).map_err(|_| {
                    UploadFailure::local(
                        AppError::Other("Upload chunk exceeds memory limits".into()),
                        offset,
                    )
                })?
            ];
            if current_size > 0 {
                input
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|error| UploadFailure::local(error.into(), offset))?;
            }
            let result = uploader
                .upload_file_chunk(UploadFileChunkRequest {
                    session_id: request.session_id.to_string(),
                    offset,
                    total_size: request.size,
                    bytes,
                })
                .await
                .map_err(|error| UploadFailure::provider(error, offset))?;
            let expected_next = offset.saturating_add(current_size);
            if result.next_offset != expected_next {
                return Err(UploadFailure::provider(
                    ProviderError::new(
                        super::domain::ProviderErrorCategory::InvalidResponse,
                        "Storage provider returned an unexpected upload offset",
                    ),
                    offset,
                ));
            }
            offset = result.next_offset;
            let transferred = i64::try_from(offset).map_err(|_| {
                UploadFailure::local(
                    AppError::Other("Upload exceeds the local transfer range".into()),
                    offset,
                )
            })?;
            self.update_transfer(
                request.job_id,
                "running",
                transferred,
                Some(request.bytes_total),
                None,
            )
            .await
            .map_err(|error| UploadFailure::local(error, offset))?;
            if result.complete {
                if offset != request.size {
                    return Err(UploadFailure::provider(
                        ProviderError::new(
                            super::domain::ProviderErrorCategory::InvalidResponse,
                            "Storage provider completed upload before the full file was sent",
                        ),
                        offset,
                    ));
                }
                return result.file.ok_or_else(|| {
                    UploadFailure::provider(
                        ProviderError::new(
                            super::domain::ProviderErrorCategory::InvalidResponse,
                            "Storage provider completed upload without file metadata",
                        ),
                        offset,
                    )
                });
            }
            if offset >= request.size {
                return Err(UploadFailure::provider(
                    ProviderError::new(
                        super::domain::ProviderErrorCategory::InvalidResponse,
                        "Storage provider did not complete the final upload chunk",
                    ),
                    offset,
                ));
            }
        }
    }

    async fn finish_upload_failure(
        &self,
        row: &StorageAccountRow,
        job_id: &str,
        transferred: u64,
        bytes_total: i64,
        error: &ProviderError,
    ) {
        self.update_provider_status(row, Some(error)).await;
        let transferred = i64::try_from(transferred).unwrap_or(bytes_total);
        let _ = self
            .update_transfer(
                job_id,
                "failed",
                transferred,
                Some(bytes_total),
                Some((error.category.as_str(), error.message.as_str())),
            )
            .await;
    }
}

enum DownloadFailure {
    Provider(ProviderError),
    Local(AppError),
}

impl From<AppError> for DownloadFailure {
    fn from(error: AppError) -> Self {
        Self::Local(error)
    }
}

enum UploadFailure {
    Provider {
        error: ProviderError,
        transferred: u64,
    },
    Local {
        error: AppError,
        transferred: i64,
    },
}

impl UploadFailure {
    fn provider(error: ProviderError, transferred: u64) -> Self {
        Self::Provider { error, transferred }
    }

    fn local(error: AppError, transferred: u64) -> Self {
        Self::Local {
            error,
            transferred: i64::try_from(transferred).unwrap_or(i64::MAX),
        }
    }
}

struct FileUploadStream<'a> {
    source: &'a std::path::Path,
    session_id: &'a str,
    size: u64,
    chunk_size: u64,
    job_id: &'a str,
    bytes_total: i64,
}

async fn hash_local_file(
    path: &std::path::Path,
    algorithms: &[ContentHashAlgorithm],
) -> AppResult<std::collections::BTreeMap<ContentHashAlgorithm, String>> {
    if algorithms.is_empty() {
        return Ok(Default::default());
    }
    let path = path.to_path_buf();
    let algorithms = algorithms.to_vec();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;

        let mut file = std::fs::File::open(path)?;
        let mut md5 = algorithms
            .contains(&ContentHashAlgorithm::Md5)
            .then(md5::Md5::new);
        let mut sha1 = algorithms
            .contains(&ContentHashAlgorithm::Sha1)
            .then(sha1::Sha1::new);
        let mut sha256 = algorithms
            .contains(&ContentHashAlgorithm::Sha256)
            .then(sha2::Sha256::new);
        let mut buffer = [0_u8; 1024 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            if let Some(hash) = md5.as_mut() {
                hash.update(&buffer[..read]);
            }
            if let Some(hash) = sha1.as_mut() {
                hash.update(&buffer[..read]);
            }
            if let Some(hash) = sha256.as_mut() {
                hash.update(&buffer[..read]);
            }
        }
        let mut hashes = std::collections::BTreeMap::new();
        if let Some(hash) = md5 {
            hashes.insert(ContentHashAlgorithm::Md5, format!("{:x}", hash.finalize()));
        }
        if let Some(hash) = sha1 {
            hashes.insert(ContentHashAlgorithm::Sha1, format!("{:x}", hash.finalize()));
        }
        if let Some(hash) = sha256 {
            hashes.insert(
                ContentHashAlgorithm::Sha256,
                format!("{:x}", hash.finalize()),
            );
        }
        Ok::<_, std::io::Error>(hashes)
    })
    .await
    .map_err(|error| AppError::Other(format!("Unable to calculate upload hashes: {error}")))?
    .map_err(AppError::from)
}

fn validate_download_destination(path: &std::path::Path) -> AppResult<()> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(AppError::Other(
            "Download destination must be an absolute file path".into(),
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("Invalid download destination".into()))?;
    if !parent.is_dir() {
        return Err(AppError::Other(
            "Download destination directory does not exist".into(),
        ));
    }
    Ok(())
}

async fn install_download(
    temporary: &std::path::Path,
    destination: &std::path::Path,
) -> AppResult<()> {
    if !destination.exists() {
        tokio::fs::rename(temporary, destination).await?;
        return Ok(());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Other("Invalid download destination".into()))?;
    let backup = parent.join(format!(".agnes-download-{}.backup", uuid::Uuid::new_v4()));
    tokio::fs::rename(destination, &backup).await?;
    match tokio::fs::rename(temporary, destination).await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(backup).await;
            Ok(())
        }
        Err(error) => {
            let _ = tokio::fs::rename(&backup, destination).await;
            Err(error.into())
        }
    }
}

fn safe_local_name(value: &str) -> String {
    let name = value
        .chars()
        .map(|character| {
            if character == '/' || character == '\\' || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect::<String>();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        "unnamed".into()
    } else {
        name.chars().take(240).collect()
    }
}

fn available_child_path(
    parent: &std::path::Path,
    name: &str,
    remote_id: &str,
    planned: &mut std::collections::HashSet<std::path::PathBuf>,
) -> std::path::PathBuf {
    let preferred = parent.join(name);
    if !preferred.exists() && planned.insert(preferred.clone()) {
        return preferred;
    }
    let suffix = remote_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(8)
        .collect::<String>();
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    for ordinal in 1..=10_000 {
        let marker = if ordinal == 1 {
            suffix.clone()
        } else {
            format!("{suffix}-{ordinal}")
        };
        let candidate_name = match extension {
            Some(extension) if !extension.is_empty() => format!("{stem} ({marker}).{extension}"),
            _ => format!("{stem} ({marker})"),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() && planned.insert(candidate.clone()) {
            return candidate;
        }
    }
    parent.join(format!("download-{}", uuid::Uuid::new_v4()))
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
        BeginFileUploadRequest, DownloadFileRequest, FileUploadSession, ProviderAuthKind,
        ProviderByteStream, ProviderResult, ProviderStability, RemoteFileItem, RemoteFileKind,
        StorageCapabilities, UploadFileChunkRequest, UploadedFileChunk,
    };
    use crate::storage::ports::{
        FileManagementProvider, FileSourceProvider, FileUploadProvider,
        ProviderAuthorizationResult, ProviderCredentialAccess, ProviderFactory, QuotaProvider,
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
                downloadable: file_id != "remote-hint-disabled",
            })
        }

        async fn download_file(
            &self,
            _request: DownloadFileRequest,
        ) -> ProviderResult<ProviderByteStream> {
            Ok(Box::pin(stream::iter(vec![Ok(vec![b'x'; 128])])))
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

    #[async_trait]
    impl FileUploadProvider for FakeDrive {
        async fn begin_file_upload(
            &self,
            request: BeginFileUploadRequest,
        ) -> ProviderResult<FileUploadSession> {
            assert_eq!(request.name, "upload.txt");
            assert_eq!(request.size, 128);
            assert_eq!(request.chunk_size, 8 * 1024 * 1024);
            Ok(FileUploadSession {
                session_id: "upload-session".into(),
                next_offset: 0,
                expires_at: None,
            })
        }

        async fn upload_file_chunk(
            &self,
            request: UploadFileChunkRequest,
        ) -> ProviderResult<UploadedFileChunk> {
            assert_eq!(request.session_id, "upload-session");
            assert_eq!(request.offset, 0);
            assert_eq!(request.bytes, vec![b'u'; 128]);
            Ok(UploadedFileChunk {
                next_offset: request.total_size,
                complete: true,
                file: Some(RemoteFileItem {
                    id: "uploaded-1".into(),
                    parent_id: None,
                    name: "upload.txt".into(),
                    kind: RemoteFileKind::File,
                    media_type: Some("text/plain".into()),
                    size: Some(128),
                    modified_at: None,
                    revision: Some("revision-2".into()),
                    downloadable: true,
                }),
            })
        }

        async fn abort_file_upload(&self, _session_id: &str) -> ProviderResult<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl FileManagementProvider for FakeDrive {
        async fn trash_files(&self, file_ids: Vec<String>) -> ProviderResult<()> {
            assert_eq!(file_ids, vec!["remote-1"]);
            Ok(())
        }
    }

    impl ProviderSession for FakeDrive {
        fn file_source(&self) -> Option<&dyn FileSourceProvider> {
            Some(self)
        }

        fn quota_source(&self) -> Option<&dyn QuotaProvider> {
            Some(self)
        }

        fn file_upload(&self) -> Option<&dyn FileUploadProvider> {
            Some(self)
        }

        fn file_management(&self) -> Option<&dyn FileManagementProvider> {
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
                    write_files: true,
                    delete_files: true,
                    quota: true,
                    user_authorization: true,
                    ..StorageCapabilities::default()
                },
            }
        }

        async fn authorize(
            &self,
            _request: ProviderAuthorizationRequest,
        ) -> ProviderResult<ProviderAuthorizationResult> {
            Ok(ProviderAuthorizationResult {
                account: StorageProviderAccount {
                    id: "account-1".into(),
                    provider_id: "future_drive".into(),
                    display_name: "Personal".into(),
                    account_subject: None,
                    config: serde_json::json!({}),
                },
                credential: Zeroizing::new("credential".into()),
            })
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
        let account_id = service
            .authorize_account(
                "future_drive".into(),
                ProviderAuthorizationRequest {
                    input: serde_json::json!({}),
                },
            )
            .await
            .unwrap();
        assert_eq!(account_id, "account-1");

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
        let destination = std::env::temp_dir().join(format!(
            "agnes-storage-service-download-{}.md",
            uuid::Uuid::new_v4()
        ));
        service
            .download_file(
                "account-1".into(),
                "remote-1".into(),
                Some("revision-1".into()),
                None,
                destination.to_string_lossy().to_string(),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), vec![b'x'; 128]);
        let advisory_destination = std::env::temp_dir().join(format!(
            "agnes-storage-service-advisory-download-{}.md",
            uuid::Uuid::new_v4()
        ));
        service
            .download_file(
                "account-1".into(),
                "remote-hint-disabled".into(),
                Some("revision-1".into()),
                Some(64),
                advisory_destination.to_string_lossy().to_string(),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(&advisory_destination).unwrap(),
            vec![b'x'; 128]
        );
        let batch_directory = std::env::temp_dir().join(format!(
            "agnes-storage-batch-download-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&batch_directory).unwrap();
        let downloaded = service
            .download_folder(
                "account-1".into(),
                "folder-1".into(),
                "Folder/Name".into(),
                batch_directory.to_string_lossy().to_string(),
            )
            .await
            .unwrap();
        assert_eq!(downloaded, 1);
        assert_eq!(
            std::fs::read(batch_directory.join("Folder_Name/Notes.md")).unwrap(),
            vec![b'x'; 128]
        );
        let upload_directory =
            std::env::temp_dir().join(format!("agnes-storage-upload-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&upload_directory).unwrap();
        let upload_source = upload_directory.join("upload.txt");
        std::fs::write(&upload_source, vec![b'u'; 128]).unwrap();
        let uploaded = service
            .upload_file(
                "account-1".into(),
                None,
                upload_source.to_string_lossy().to_string(),
            )
            .await
            .unwrap();
        assert_eq!(uploaded.id, "uploaded-1");
        let transfers = service
            .list_transfers(Some("account-1".into()), 10)
            .await
            .unwrap();
        assert_eq!(transfers.len(), 4);
        assert!(transfers
            .iter()
            .all(|transfer| transfer.status == "completed"));
        assert!(transfers
            .iter()
            .any(|transfer| transfer.operation == "file_upload"));
        assert_eq!(
            service
                .trash_files(
                    "account-1".into(),
                    vec!["remote-1".into(), "remote-1".into()]
                )
                .await
                .unwrap(),
            1
        );
        let quota = service.refresh_quota("account-1".into()).await.unwrap();
        assert_eq!(quota.total_bytes, Some(1024));
        service.remove_account("account-1".into()).await.unwrap();
        assert!(service.list_accounts().await.unwrap().is_empty());

        drop(service);
        let _ = std::fs::remove_file(destination);
        let _ = std::fs::remove_file(advisory_destination);
        let _ = std::fs::remove_dir_all(batch_directory);
        let _ = std::fs::remove_dir_all(upload_directory);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn completed_download_replaces_the_destination_without_leaving_partials() {
        let directory =
            std::env::temp_dir().join(format!("agnes-storage-download-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("notes.txt");
        let temporary = directory.join("notes.partial");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::write(&temporary, b"new").unwrap();

        install_download(&temporary, &destination).await.unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        assert!(!temporary.exists());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert!(validate_download_destination(&destination).is_ok());
        assert!(validate_download_destination(std::path::Path::new("relative.txt")).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
