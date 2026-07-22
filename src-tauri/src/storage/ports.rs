use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use zeroize::Zeroizing;

use super::domain::{
    BeginFileUploadRequest, BeginObjectUploadRequest, DownloadFileRequest, DownloadObjectRequest,
    FileUploadSession, ListFilesRequest, ObjectUploadSession, ProviderAuthorizationRequest,
    ProviderByteStream, ProviderDescriptor, ProviderError, ProviderQuota, ProviderResult,
    RemoteFileItem, RemoteFilePage, RemoteObjectLocator, RemoteObjectState, SearchFilesRequest,
    StorageProviderAccount, UploadFileChunkRequest, UploadObjectChunkRequest, UploadedFileChunk,
    UploadedObjectChunk,
};

pub struct ProviderAuthorizationResult {
    pub account: StorageProviderAccount,
    pub credential: Zeroizing<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderAuthorizationChallenge {
    pub challenge_id: String,
    pub provider_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub expires_at: Option<String>,
}

pub enum ProviderAuthorizationStep {
    Pending,
    Authorized(ProviderAuthorizationResult),
}

#[async_trait]
pub trait ProviderCredentialStore: Send + Sync {
    async fn load(&self, account_id: &str) -> ProviderResult<Option<Zeroizing<String>>>;
    async fn store(&self, account_id: &str, credential: Zeroizing<String>) -> ProviderResult<()>;
    async fn delete(&self, account_id: &str) -> ProviderResult<()>;
}

#[async_trait]
pub trait ProviderCredentialAccess: Send + Sync {
    async fn load(&self) -> ProviderResult<Option<Zeroizing<String>>>;
    async fn store(&self, credential: Zeroizing<String>) -> ProviderResult<()>;
    async fn delete(&self) -> ProviderResult<()>;
}

#[async_trait]
pub trait FileSourceProvider: Send + Sync {
    async fn list_files(&self, request: ListFilesRequest) -> ProviderResult<RemoteFilePage>;

    async fn search_files(&self, _request: SearchFilesRequest) -> ProviderResult<RemoteFilePage> {
        Err(ProviderError::unsupported("file search"))
    }

    async fn get_file(&self, file_id: &str) -> ProviderResult<RemoteFileItem>;
    async fn download_file(
        &self,
        request: DownloadFileRequest,
    ) -> ProviderResult<ProviderByteStream>;
}

#[async_trait]
pub trait FileUploadProvider: Send + Sync {
    async fn begin_file_upload(
        &self,
        request: BeginFileUploadRequest,
    ) -> ProviderResult<FileUploadSession>;
    async fn upload_file_chunk(
        &self,
        request: UploadFileChunkRequest,
    ) -> ProviderResult<UploadedFileChunk>;
    async fn abort_file_upload(&self, session_id: &str) -> ProviderResult<()>;
}

#[async_trait]
pub trait FileManagementProvider: Send + Sync {
    async fn trash_files(&self, file_ids: Vec<String>) -> ProviderResult<()>;

    async fn move_files(
        &self,
        _file_ids: Vec<String>,
        _target_folder_id: Option<String>,
    ) -> ProviderResult<()> {
        Err(ProviderError::unsupported("moving files"))
    }
}

#[async_trait]
pub trait QuotaProvider: Send + Sync {
    async fn quota(&self) -> ProviderResult<ProviderQuota>;
}

#[async_trait]
pub trait ObjectStorageProvider: Send + Sync {
    async fn stat_object(&self, locator: &RemoteObjectLocator)
        -> ProviderResult<RemoteObjectState>;
    async fn download_object(
        &self,
        request: DownloadObjectRequest,
    ) -> ProviderResult<ProviderByteStream>;
    async fn begin_object_upload(
        &self,
        request: BeginObjectUploadRequest,
    ) -> ProviderResult<ObjectUploadSession>;
    async fn upload_object_chunk(
        &self,
        request: UploadObjectChunkRequest,
    ) -> ProviderResult<UploadedObjectChunk>;
    async fn abort_object_upload(&self, session_id: &str) -> ProviderResult<()>;
    async fn delete_object(&self, locator: &RemoteObjectLocator) -> ProviderResult<()>;
}

pub trait ProviderSession: Send + Sync {
    fn file_source(&self) -> Option<&dyn FileSourceProvider> {
        None
    }

    fn quota_source(&self) -> Option<&dyn QuotaProvider> {
        None
    }

    fn file_upload(&self) -> Option<&dyn FileUploadProvider> {
        None
    }

    fn file_management(&self) -> Option<&dyn FileManagementProvider> {
        None
    }

    fn object_storage(&self) -> Option<&dyn ObjectStorageProvider> {
        None
    }
}

#[async_trait]
pub trait ProviderFactory: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn authorize(
        &self,
        _request: ProviderAuthorizationRequest,
    ) -> ProviderResult<ProviderAuthorizationResult> {
        Err(ProviderError::unsupported("interactive authorization"))
    }

    async fn begin_authorization(
        &self,
        _request: ProviderAuthorizationRequest,
    ) -> ProviderResult<ProviderAuthorizationChallenge> {
        Err(ProviderError::unsupported("authorization challenge"))
    }

    async fn poll_authorization(
        &self,
        _challenge_id: &str,
    ) -> ProviderResult<ProviderAuthorizationStep> {
        Err(ProviderError::unsupported("authorization challenge"))
    }

    async fn connect(
        &self,
        account: &StorageProviderAccount,
        credentials: Arc<dyn ProviderCredentialAccess>,
    ) -> ProviderResult<Arc<dyn ProviderSession>>;
}
