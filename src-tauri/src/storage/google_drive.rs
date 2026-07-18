use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::{Mutex as StdMutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rand::RngCore;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, LOCATION,
    RANGE, RETRY_AFTER,
};
use reqwest::{Method, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout};
use zeroize::{Zeroize, Zeroizing};

use super::domain::{
    BeginFileUploadRequest, BeginObjectUploadRequest, DownloadFileRequest, DownloadObjectRequest,
    FileUploadSession, ListFilesRequest, ObjectUploadSession, ProviderAuthKind,
    ProviderAuthorizationRequest, ProviderByteStream, ProviderDescriptor, ProviderError,
    ProviderErrorCategory, ProviderQuota, ProviderResult, ProviderStability, RemoteFileItem,
    RemoteFileKind, RemoteFilePage, RemoteObjectLocator, RemoteObjectState, StorageCapabilities,
    StorageProviderAccount, UploadFileChunkRequest, UploadObjectChunkRequest, UploadedFileChunk,
    UploadedObjectChunk,
};
use super::ports::{
    FileSourceProvider, FileUploadProvider, ObjectStorageProvider, ProviderAuthorizationResult,
    ProviderCredentialAccess, ProviderFactory, ProviderSession, QuotaProvider,
};

const PROVIDER_ID: &str = "google_drive";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const DRIVE_UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";
const DRIVE_APPDATA_SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
const GOOGLE_FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const GOOGLE_SHORTCUT_MIME: &str = "application/vnd.google-apps.shortcut";
const FILE_FIELDS: &str = "id,name,mimeType,size,modifiedTime,version,md5Checksum,parents,capabilities(canDownload),appProperties";
const MAX_CLIENT_FILE_BYTES: u64 = 64 * 1024;
const OAUTH_TIMEOUT: Duration = Duration::from_secs(300);
const TOKEN_EXPIRY_SKEW_SECONDS: u64 = 60;
const MAX_API_ATTEMPTS: usize = 3;

pub struct GoogleDriveFactory {
    client: reqwest::Client,
    sessions: StdMutex<std::collections::HashMap<String, Weak<GoogleDriveSession>>>,
}

impl GoogleDriveFactory {
    pub fn new() -> ProviderResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Agnes-Agent/0.1 GoogleDriveProvider")
            .build()
            .map_err(network_error)?;
        Ok(Self {
            client,
            sessions: StdMutex::new(std::collections::HashMap::new()),
        })
    }
}

#[async_trait]
impl ProviderFactory for GoogleDriveFactory {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: PROVIDER_ID.into(),
            display_name: "Google Drive".into(),
            auth_kind: ProviderAuthKind::OAuth2Pkce,
            stability: ProviderStability::Official,
            implementation_version: "drive-v3-pkce-v1".into(),
            capabilities: StorageCapabilities {
                browse_files: true,
                read_files: true,
                write_files: true,
                object_storage: true,
                range_download: true,
                resumable_upload: true,
                stable_revisions: true,
                quota: true,
                user_authorization: true,
                max_object_bytes: Some(5 * 1024 * 1024 * 1024 * 1024),
                recommended_chunk_bytes: Some(8 * 1024 * 1024),
                ..StorageCapabilities::default()
            },
        }
    }

    async fn authorize(
        &self,
        request: ProviderAuthorizationRequest,
    ) -> ProviderResult<ProviderAuthorizationResult> {
        let client_path = request
            .input
            .get("client_credentials_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request("Google Desktop OAuth client file is required"))?;
        let client_config = read_client_config(Path::new(client_path)).await?;
        let token = authorize_with_pkce(&self.client, &client_config).await?;
        let granted_scopes = token.scope.as_deref().unwrap_or_default();
        if !scope_granted(granted_scopes, DRIVE_SCOPE)
            || !scope_granted(granted_scopes, DRIVE_APPDATA_SCOPE)
        {
            return Err(ProviderError::new(
                ProviderErrorCategory::Permission,
                "Google did not grant the required Drive permissions",
            ));
        }
        let about = fetch_about_with_token(&self.client, token.access_token.as_str()).await?;
        let subject = about
            .user
            .email_address
            .clone()
            .filter(|value| !value.trim().is_empty());
        let stable_subject = about
            .user
            .permission_id
            .as_deref()
            .or(subject.as_deref())
            .ok_or_else(|| invalid_response("Google Drive account identity was omitted"))?;
        let display_subject = about
            .user
            .display_name
            .as_deref()
            .or(subject.as_deref())
            .unwrap_or("Account");

        let credential = Zeroizing::new(GoogleCredential {
            client_id: client_config.client_id.clone(),
            client_secret: client_config.client_secret.clone(),
            access_token: token.access_token.clone(),
            refresh_token: token.refresh_token.clone().ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCategory::Authentication,
                    "Google did not return an offline refresh token; revoke the app grant and authorize again",
                )
            })?,
            token_type: token.token_type.clone().unwrap_or_else(|| "Bearer".into()),
            scope: token.scope.clone(),
            expires_at: now_epoch().saturating_add(token.expires_in.unwrap_or(3600)),
        });
        let serialized = serde_json::to_string(&*credential)
            .map_err(|_| invalid_response("Unable to encode Google Drive credentials"))?;

        Ok(ProviderAuthorizationResult {
            account: StorageProviderAccount {
                id: stable_account_id(stable_subject),
                provider_id: PROVIDER_ID.into(),
                display_name: format!("Google Drive - {}", truncate(display_subject, 105)),
                account_subject: subject.filter(|value| value.chars().count() <= 512),
                config: json!({"space": "drive"}),
            },
            credential: Zeroizing::new(serialized),
        })
    }

    async fn connect(
        &self,
        account: &StorageProviderAccount,
        credentials: Arc<dyn ProviderCredentialAccess>,
    ) -> ProviderResult<Arc<dyn ProviderSession>> {
        if account.provider_id != PROVIDER_ID {
            return Err(invalid_request(
                "Google Drive received another provider account",
            ));
        }
        let stored = credentials.load().await?.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCategory::Authentication,
                "Google Drive authorization is missing on this device",
            )
        })?;
        parse_credential(stored.as_str())?;
        let mut sessions = self.sessions.lock().map_err(|_| {
            ProviderError::new(
                ProviderErrorCategory::RemoteUnavailable,
                "Google Drive session registry is unavailable",
            )
        })?;
        if let Some(session) = sessions.get(&account.id).and_then(Weak::upgrade) {
            return Ok(session);
        }
        sessions.retain(|_, session| session.strong_count() > 0);
        let session = Arc::new(GoogleDriveSession {
            client: self.client.clone(),
            credentials,
            refresh_lock: Mutex::new(()),
        });
        sessions.insert(account.id.clone(), Arc::downgrade(&session));
        Ok(session)
    }
}

struct GoogleDriveSession {
    client: reqwest::Client,
    credentials: Arc<dyn ProviderCredentialAccess>,
    refresh_lock: Mutex<()>,
}

#[async_trait]
impl FileSourceProvider for GoogleDriveSession {
    async fn list_files(&self, request: ListFilesRequest) -> ProviderResult<RemoteFilePage> {
        let request = request.normalized();
        let parent_id = request.parent_id.as_deref().unwrap_or("root");
        let query = vec![
            (
                "q".into(),
                format!(
                    "'{}' in parents and trashed = false",
                    escape_drive_query(parent_id)
                ),
            ),
            ("pageSize".into(), request.page_size.to_string()),
            (
                "fields".into(),
                format!("nextPageToken,files({FILE_FIELDS})"),
            ),
            ("orderBy".into(), "folder,name_natural".into()),
            ("spaces".into(), "drive".into()),
            ("supportsAllDrives".into(), "true".into()),
            ("includeItemsFromAllDrives".into(), "true".into()),
        ];
        let mut query = query;
        if let Some(page_token) = request.page_token {
            query.push(("pageToken".into(), page_token));
        }
        let response = self
            .send_authorized(
                Method::GET,
                &format!("{DRIVE_API}/files"),
                &query,
                HeaderMap::new(),
                RequestBody::Empty,
                true,
                &[],
            )
            .await?;
        let page: GoogleFileList = parse_json_response(response).await?;
        if page.files.len() > request.page_size
            || page
                .next_page_token
                .as_deref()
                .is_some_and(|value| value.len() > 4096)
        {
            return Err(invalid_response(
                "Google Drive returned an invalid file page",
            ));
        }
        let items = page
            .files
            .into_iter()
            .map(remote_file_item)
            .collect::<ProviderResult<Vec<_>>>()?;
        Ok(RemoteFilePage {
            items,
            next_page_token: page.next_page_token,
        })
    }

    async fn get_file(&self, file_id: &str) -> ProviderResult<RemoteFileItem> {
        remote_file_item(self.get_google_file(file_id).await?)
    }

    async fn download_file(
        &self,
        request: DownloadFileRequest,
    ) -> ProviderResult<ProviderByteStream> {
        validate_range(request.range_start, request.range_end_inclusive)?;
        let file = self.get_google_file(&request.file_id).await?;
        let current_revision = file.version.as_deref().or(file.md5_checksum.as_deref());
        if request
            .expected_revision
            .as_deref()
            .is_some_and(|expected| current_revision != Some(expected))
        {
            return Err(ProviderError::new(
                ProviderErrorCategory::Conflict,
                "Google Drive file revision changed before download",
            ));
        }
        if file.mime_type == GOOGLE_FOLDER_MIME || file.mime_type == GOOGLE_SHORTCUT_MIME {
            return Err(ProviderError::unsupported("downloading this Drive item"));
        }

        let (url, query) = if let Some(export_mime) = google_export_mime(&file.mime_type) {
            if request.range_start.is_some() || request.range_end_inclusive.is_some() {
                return Err(ProviderError::unsupported(
                    "range downloads for exported Google Workspace documents",
                ));
            }
            (
                format!(
                    "{DRIVE_API}/files/{}/export",
                    path_segment(&request.file_id)
                ),
                vec![("mimeType".into(), export_mime.into())],
            )
        } else {
            (
                format!("{DRIVE_API}/files/{}", path_segment(&request.file_id)),
                vec![
                    ("alt".into(), "media".into()),
                    ("supportsAllDrives".into(), "true".into()),
                ],
            )
        };
        let mut headers = HeaderMap::new();
        if let Some(value) = range_header(request.range_start, request.range_end_inclusive)? {
            headers.insert(RANGE, value);
        }
        let requested_range = request.range_start.is_some();
        let response = self
            .send_authorized(
                Method::GET,
                &url,
                &query,
                headers,
                RequestBody::Empty,
                true,
                &[],
            )
            .await?;
        if requested_range && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(invalid_response(
                "Google Drive ignored the requested download byte range",
            ));
        }
        response_stream(response)
    }
}

#[async_trait]
impl QuotaProvider for GoogleDriveSession {
    async fn quota(&self) -> ProviderResult<ProviderQuota> {
        let response = self
            .send_authorized(
                Method::GET,
                &format!("{DRIVE_API}/about"),
                &[(
                    "fields".into(),
                    "storageQuota(limit,usage,usageInDriveTrash)".into(),
                )],
                HeaderMap::new(),
                RequestBody::Empty,
                true,
                &[],
            )
            .await?;
        let about: GoogleAbout = parse_json_response(response).await?;
        Ok(ProviderQuota {
            used_bytes: parse_optional_u64(about.storage_quota.usage.as_deref(), "quota usage")?,
            total_bytes: parse_optional_u64(about.storage_quota.limit.as_deref(), "quota limit")?,
            trashed_bytes: parse_optional_u64(
                about.storage_quota.usage_in_drive_trash.as_deref(),
                "trashed quota",
            )?,
            checked_at: now_epoch().to_string(),
        })
    }
}

#[async_trait]
impl FileUploadProvider for GoogleDriveSession {
    async fn begin_file_upload(
        &self,
        request: BeginFileUploadRequest,
    ) -> ProviderResult<FileUploadSession> {
        validate_file_upload_request(&request)?;
        self.require_scope(DRIVE_SCOPE).await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-upload-content-length"),
            header_value(&request.size.to_string(), "file size")?,
        );
        headers.insert(
            HeaderName::from_static("x-upload-content-type"),
            header_value(&request.media_type, "file media type")?,
        );
        let response = self
            .send_authorized(
                Method::POST,
                &format!("{DRIVE_UPLOAD_API}/files"),
                &[
                    ("uploadType".into(), "resumable".into()),
                    ("fields".into(), FILE_FIELDS.into()),
                    ("supportsAllDrives".into(), "true".into()),
                ],
                headers,
                RequestBody::Json(json!({
                    "name": request.name,
                    "parents": [request.parent_id.unwrap_or_else(|| "root".into())],
                })),
                false,
                &[],
            )
            .await?;
        let session_id = resumable_session_id(&response)?;
        Ok(FileUploadSession {
            session_id,
            next_offset: 0,
            expires_at: None,
        })
    }

    async fn upload_file_chunk(
        &self,
        request: UploadFileChunkRequest,
    ) -> ProviderResult<UploadedFileChunk> {
        let result = self
            .send_upload_chunk(
                &request.session_id,
                request.offset,
                request.total_size,
                request.bytes,
            )
            .await?;
        let complete = result.file.is_some();
        Ok(UploadedFileChunk {
            next_offset: result.next_offset,
            complete,
            file: result.file.map(remote_file_item).transpose()?,
        })
    }

    async fn abort_file_upload(&self, session_id: &str) -> ProviderResult<()> {
        self.abort_upload(session_id).await
    }
}

#[async_trait]
impl ObjectStorageProvider for GoogleDriveSession {
    async fn stat_object(
        &self,
        locator: &RemoteObjectLocator,
    ) -> ProviderResult<RemoteObjectState> {
        let file = self.get_google_file(&locator.opaque_id).await?;
        let state = remote_object_state(file)?;
        if locator
            .revision
            .as_deref()
            .is_some_and(|expected| state.locator.revision.as_deref() != Some(expected))
        {
            return Err(ProviderError::new(
                ProviderErrorCategory::Conflict,
                "Google Drive object revision changed",
            ));
        }
        Ok(state)
    }

    async fn download_object(
        &self,
        request: DownloadObjectRequest,
    ) -> ProviderResult<ProviderByteStream> {
        validate_range(request.range_start, request.range_end_inclusive)?;
        self.stat_object(&request.locator).await?;
        let mut headers = HeaderMap::new();
        if let Some(value) = range_header(request.range_start, request.range_end_inclusive)? {
            headers.insert(RANGE, value);
        }
        let requested_range = request.range_start.is_some();
        let response = self
            .send_authorized(
                Method::GET,
                &format!(
                    "{DRIVE_API}/files/{}",
                    path_segment(&request.locator.opaque_id)
                ),
                &[("alt".into(), "media".into())],
                headers,
                RequestBody::Empty,
                true,
                &[],
            )
            .await?;
        if requested_range && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(invalid_response(
                "Google Drive ignored the requested object byte range",
            ));
        }
        response_stream(response)
    }

    async fn begin_object_upload(
        &self,
        request: BeginObjectUploadRequest,
    ) -> ProviderResult<ObjectUploadSession> {
        validate_upload_request(&request)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-upload-content-length"),
            header_value(&request.size.to_string(), "object size")?,
        );
        headers.insert(
            HeaderName::from_static("x-upload-content-type"),
            header_value(&request.media_type, "object media type")?,
        );
        let response = self
            .send_authorized(
                Method::POST,
                &format!("{DRIVE_UPLOAD_API}/files"),
                &[
                    ("uploadType".into(), "resumable".into()),
                    ("fields".into(), FILE_FIELDS.into()),
                ],
                headers,
                RequestBody::Json(json!({
                    "name": request.opaque_name,
                    "parents": ["appDataFolder"],
                    "appProperties": {"content_hash": request.content_hash},
                })),
                false,
                &[],
            )
            .await?;
        let session_id = resumable_session_id(&response)?;
        Ok(ObjectUploadSession {
            session_id,
            next_offset: 0,
            expires_at: None,
        })
    }

    async fn upload_object_chunk(
        &self,
        request: UploadObjectChunkRequest,
    ) -> ProviderResult<UploadedObjectChunk> {
        let result = self
            .send_upload_chunk(
                &request.session_id,
                request.offset,
                request.total_size,
                request.bytes,
            )
            .await?;
        let complete = result.file.is_some();
        Ok(UploadedObjectChunk {
            next_offset: result.next_offset,
            complete,
            object: result.file.map(remote_object_state).transpose()?,
        })
    }

    async fn abort_object_upload(&self, session_id: &str) -> ProviderResult<()> {
        self.abort_upload(session_id).await
    }

    async fn delete_object(&self, locator: &RemoteObjectLocator) -> ProviderResult<()> {
        self.send_authorized(
            Method::DELETE,
            &format!("{DRIVE_API}/files/{}", path_segment(&locator.opaque_id)),
            &[],
            HeaderMap::new(),
            RequestBody::Empty,
            true,
            &[StatusCode::NOT_FOUND],
        )
        .await?;
        Ok(())
    }
}

impl ProviderSession for GoogleDriveSession {
    fn file_source(&self) -> Option<&dyn FileSourceProvider> {
        Some(self)
    }

    fn quota_source(&self) -> Option<&dyn QuotaProvider> {
        Some(self)
    }

    fn file_upload(&self) -> Option<&dyn FileUploadProvider> {
        Some(self)
    }

    fn object_storage(&self) -> Option<&dyn ObjectStorageProvider> {
        Some(self)
    }
}

impl GoogleDriveSession {
    async fn get_google_file(&self, file_id: &str) -> ProviderResult<GoogleFile> {
        validate_opaque_id(file_id, "file ID")?;
        let response = self
            .send_authorized(
                Method::GET,
                &format!("{DRIVE_API}/files/{}", path_segment(file_id)),
                &[
                    ("fields".into(), FILE_FIELDS.into()),
                    ("supportsAllDrives".into(), "true".into()),
                ],
                HeaderMap::new(),
                RequestBody::Empty,
                true,
                &[],
            )
            .await?;
        parse_json_response(response).await
    }

    async fn access_token(&self, force_refresh: bool) -> ProviderResult<Zeroizing<String>> {
        let _guard = self.refresh_lock.lock().await;
        let stored = self.credentials.load().await?.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCategory::Authentication,
                "Google Drive authorization is missing on this device",
            )
        })?;
        let mut credential = Zeroizing::new(parse_credential(stored.as_str())?);
        if force_refresh
            || credential.expires_at <= now_epoch().saturating_add(TOKEN_EXPIRY_SKEW_SECONDS)
        {
            refresh_credential(&self.client, &mut credential).await?;
            let serialized = serde_json::to_string(&*credential)
                .map_err(|_| invalid_response("Unable to encode refreshed Google credentials"))?;
            self.credentials.store(Zeroizing::new(serialized)).await?;
        }
        Ok(Zeroizing::new(credential.access_token.clone()))
    }

    async fn require_scope(&self, required: &str) -> ProviderResult<()> {
        let stored = self.credentials.load().await?.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCategory::Authentication,
                "Google Drive authorization is missing on this device",
            )
        })?;
        let credential = Zeroizing::new(parse_credential(stored.as_str())?);
        if credential
            .scope
            .as_deref()
            .is_some_and(|scopes| scope_granted(scopes, required))
        {
            Ok(())
        } else {
            Err(ProviderError::new(
                ProviderErrorCategory::Permission,
                "Google Drive upload permission is missing; reconnect the account to grant the new scope",
            ))
        }
    }

    async fn send_upload_chunk(
        &self,
        session_id: &str,
        offset: u64,
        total_size: u64,
        bytes: Vec<u8>,
    ) -> ProviderResult<UploadChunkResult> {
        validate_upload_session_url(session_id)?;
        let byte_count =
            u64::try_from(bytes.len()).map_err(|_| invalid_request("Upload chunk is too large"))?;
        if offset > total_size || offset.saturating_add(byte_count) > total_size {
            return Err(invalid_request(
                "Upload chunk exceeds the declared file size",
            ));
        }
        if byte_count == 0 && total_size != 0 {
            return Err(invalid_request("Upload chunk cannot be empty"));
        }
        let content_range = if total_size == 0 {
            "bytes */0".into()
        } else {
            format!(
                "bytes {}-{}/{}",
                offset,
                offset + byte_count - 1,
                total_size
            )
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_LENGTH,
            header_value(&byte_count.to_string(), "chunk size")?,
        );
        headers.insert(
            CONTENT_RANGE,
            header_value(&content_range, "content range")?,
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        let response = self
            .send_authorized(
                Method::PUT,
                session_id,
                &[],
                headers,
                RequestBody::Bytes(bytes),
                true,
                &[StatusCode::PERMANENT_REDIRECT],
            )
            .await?;
        if response.status() == StatusCode::PERMANENT_REDIRECT {
            let next_offset = response
                .headers()
                .get(RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(parse_uploaded_range)
                .unwrap_or(0);
            if next_offset > total_size {
                return Err(invalid_response(
                    "Google Drive returned an invalid resumable upload offset",
                ));
            }
            return Ok(UploadChunkResult {
                next_offset,
                file: None,
            });
        }
        let file: GoogleFile = parse_json_response(response).await?;
        Ok(UploadChunkResult {
            next_offset: total_size,
            file: Some(file),
        })
    }

    async fn abort_upload(&self, session_id: &str) -> ProviderResult<()> {
        validate_upload_session_url(session_id)?;
        self.send_authorized(
            Method::DELETE,
            session_id,
            &[],
            HeaderMap::new(),
            RequestBody::Empty,
            false,
            &[StatusCode::NOT_FOUND],
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_authorized(
        &self,
        method: Method,
        url: &str,
        query: &[(String, String)],
        headers: HeaderMap,
        body: RequestBody,
        retry_remote: bool,
        accepted_statuses: &[StatusCode],
    ) -> ProviderResult<Response> {
        let mut auth_retried = false;
        for attempt in 0..MAX_API_ATTEMPTS {
            let token = self.access_token(false).await?;
            let mut request = self
                .client
                .request(method.clone(), url)
                .bearer_auth(token.as_str())
                .query(query)
                .headers(headers.clone());
            request = match &body {
                RequestBody::Empty => request,
                RequestBody::Json(value) => request.json(value),
                RequestBody::Bytes(value) => request.body(value.clone()),
            };
            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status == StatusCode::UNAUTHORIZED && !auth_retried {
                        self.access_token(true).await?;
                        auth_retried = true;
                        continue;
                    }
                    if retry_remote && retryable_status(status) && attempt + 1 < MAX_API_ATTEMPTS {
                        sleep(retry_delay(&response, attempt)).await;
                        continue;
                    }
                    if status.is_success() || accepted_statuses.contains(&status) {
                        return Ok(response);
                    }
                    return Err(response_error(response).await);
                }
                Err(error)
                    if retry_remote
                        && attempt + 1 < MAX_API_ATTEMPTS
                        && (error.is_connect() || error.is_timeout()) =>
                {
                    sleep(Duration::from_millis(250 * (1_u64 << attempt))).await;
                }
                Err(error) => return Err(network_error(error)),
            }
        }
        Err(ProviderError::new(
            ProviderErrorCategory::RemoteUnavailable,
            "Google Drive request exhausted its retry budget",
        ))
    }
}

struct UploadChunkResult {
    next_offset: u64,
    file: Option<GoogleFile>,
}

#[derive(Clone)]
enum RequestBody {
    Empty,
    Json(Value),
    Bytes(Vec<u8>),
}

#[derive(Debug, Deserialize, Zeroize)]
struct ClientFile {
    installed: Option<InstalledClient>,
}

#[derive(Debug, Deserialize, Zeroize)]
struct InstalledClient {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Zeroize)]
struct GoogleCredential {
    client_id: String,
    client_secret: String,
    access_token: String,
    refresh_token: String,
    token_type: String,
    scope: Option<String>,
    expires_at: u64,
}

#[derive(Debug, Deserialize, Zeroize)]
struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleAbout {
    #[serde(default)]
    user: GoogleUser,
    #[serde(default)]
    storage_quota: GoogleStorageQuota,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleUser {
    display_name: Option<String>,
    email_address: Option<String>,
    permission_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleStorageQuota {
    limit: Option<String>,
    usage: Option<String>,
    usage_in_drive_trash: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleFileList {
    #[serde(default)]
    files: Vec<GoogleFile>,
    next_page_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleFile {
    id: String,
    name: String,
    mime_type: String,
    size: Option<String>,
    modified_time: Option<String>,
    version: Option<String>,
    md5_checksum: Option<String>,
    #[serde(default)]
    parents: Vec<String>,
    #[serde(default)]
    capabilities: GoogleFileCapabilities,
    #[serde(default)]
    app_properties: std::collections::HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleFileCapabilities {
    #[serde(default)]
    can_download: bool,
}

async fn read_client_config(path: &Path) -> ProviderResult<Zeroizing<InstalledClient>> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| invalid_request("Unable to read the Google Desktop OAuth client file"))?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CLIENT_FILE_BYTES {
        return Err(invalid_request(
            "Google Desktop OAuth client file must be a non-empty JSON file under 64 KiB",
        ));
    }
    let bytes = Zeroizing::new(
        tokio::fs::read(path)
            .await
            .map_err(|_| invalid_request("Unable to read the Google Desktop OAuth client file"))?,
    );
    let mut parsed = Zeroizing::new(
        serde_json::from_slice::<ClientFile>(&bytes)
            .map_err(|_| invalid_request("Invalid Google Desktop OAuth client JSON"))?,
    );
    let installed = parsed
        .installed
        .take()
        .ok_or_else(|| invalid_request("OAuth client must use Google's Desktop app type"))?;
    validate_client_config(&installed)?;
    Ok(Zeroizing::new(installed))
}

fn validate_client_config(config: &InstalledClient) -> ProviderResult<()> {
    if config.client_id.len() > 512
        || !config.client_id.ends_with(".apps.googleusercontent.com")
        || config.client_secret.is_empty()
        || config.client_secret.len() > 512
        || config.auth_uri != "https://accounts.google.com/o/oauth2/auth"
        || config.token_uri != TOKEN_URL
        || !config.redirect_uris.iter().any(|uri| {
            uri == "http://localhost" || uri == "http://127.0.0.1" || uri == "http://[::1]"
        })
    {
        return Err(invalid_request(
            "OAuth JSON is not a supported Google Desktop client configuration",
        ));
    }
    Ok(())
}

async fn authorize_with_pkce(
    client: &reqwest::Client,
    config: &InstalledClient,
) -> ProviderResult<Zeroizing<OAuthTokenResponse>> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| invalid_request("Unable to open the local OAuth callback port"))?;
    let port = listener
        .local_addr()
        .map_err(|_| invalid_request("Unable to inspect the local OAuth callback port"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let state = random_base64(32);
    let verifier = Zeroizing::new(random_base64(64));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(AUTH_URL).map_err(|_| invalid_response("Invalid Google OAuth URL"))?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &format!("{DRIVE_SCOPE} {DRIVE_APPDATA_SCOPE}"))
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("include_granted_scopes", "true")
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    open::that(url.as_str()).map_err(|_| {
        invalid_request("Unable to open the system browser for Google authorization")
    })?;
    let code = wait_for_oauth_callback(listener, &state).await?;

    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let token = Zeroizing::new(parse_json_response::<OAuthTokenResponse>(response).await?);
    if token.access_token.is_empty() {
        return Err(invalid_response(
            "Google OAuth response omitted the access token",
        ));
    }
    Ok(token)
}

async fn wait_for_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> ProviderResult<Zeroizing<String>> {
    timeout(OAUTH_TIMEOUT, async {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| invalid_request("Unable to accept the local OAuth callback"))?;
        let mut request = vec![0_u8; 8192];
        let read = timeout(Duration::from_secs(10), stream.read(&mut request))
            .await
            .map_err(|_| invalid_request("Google OAuth callback timed out"))?
            .map_err(|_| invalid_request("Unable to read the Google OAuth callback"))?;
        let result = parse_oauth_callback(&request[..read], expected_state);
        request.zeroize();
        let success = result.is_ok();
        let body = if success {
            "<!doctype html><meta charset=utf-8><title>Agnes</title><p>Google Drive authorization completed. You may close this tab.</p>"
        } else {
            "<!doctype html><meta charset=utf-8><title>Agnes</title><p>Google Drive authorization failed. Return to Agnes for details.</p>"
        };
        let response = format!(
            "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
            if success { "200 OK" } else { "400 Bad Request" },
            body.len(),
            body,
        );
        let _ = stream.write_all(response.as_bytes()).await;
        result
    })
    .await
    .map_err(|_| invalid_request("Google authorization was not completed within five minutes"))?
}

fn parse_oauth_callback(request: &[u8], expected_state: &str) -> ProviderResult<Zeroizing<String>> {
    let request = std::str::from_utf8(request)
        .map_err(|_| invalid_request("Google OAuth callback was not valid HTTP"))?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| invalid_request("Google OAuth callback omitted its request target"))?;
    let url = Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| invalid_request("Google OAuth callback URL was invalid"))?;
    let parameters = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if parameters.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err(invalid_request("Google OAuth callback state did not match"));
    }
    if let Some(error) = parameters.get("error") {
        return Err(ProviderError::new(
            ProviderErrorCategory::Authentication,
            format!("Google authorization was denied: {}", truncate(error, 200)),
        ));
    }
    parameters
        .get("code")
        .map(|value| Zeroizing::new(value.to_string()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid_request("Google OAuth callback omitted the authorization code"))
}

async fn refresh_credential(
    client: &reqwest::Client,
    credential: &mut GoogleCredential,
) -> ProviderResult<()> {
    let response = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", credential.client_id.as_str()),
            ("client_secret", credential.client_secret.as_str()),
            ("refresh_token", credential.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(network_error)?;
    if !response.status().is_success() {
        let mut error = response_error(response).await;
        if error.category == ProviderErrorCategory::InvalidRequest {
            error.category = ProviderErrorCategory::Authentication;
        }
        return Err(error);
    }
    let token = Zeroizing::new(parse_json_response::<OAuthTokenResponse>(response).await?);
    if token.access_token.is_empty() {
        return Err(invalid_response(
            "Google token refresh omitted the access token",
        ));
    }
    credential.access_token.clone_from(&token.access_token);
    credential.token_type = token.token_type.clone().unwrap_or_else(|| "Bearer".into());
    if token.scope.is_some() {
        credential.scope.clone_from(&token.scope);
    }
    credential.expires_at = now_epoch().saturating_add(token.expires_in.unwrap_or(3600));
    Ok(())
}

async fn fetch_about_with_token(
    client: &reqwest::Client,
    access_token: &str,
) -> ProviderResult<GoogleAbout> {
    let response = client
        .get(format!("{DRIVE_API}/about"))
        .bearer_auth(access_token)
        .query(&[(
            "fields",
            "user(displayName,emailAddress,permissionId),storageQuota(limit,usage,usageInDriveTrash)",
        )])
        .send()
        .await
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    parse_json_response(response).await
}

fn parse_credential(value: &str) -> ProviderResult<GoogleCredential> {
    let credential: GoogleCredential = serde_json::from_str(value).map_err(|_| {
        ProviderError::new(
            ProviderErrorCategory::Authentication,
            "Stored Google Drive authorization is invalid",
        )
    })?;
    if credential.client_id.is_empty()
        || credential.client_secret.is_empty()
        || credential.access_token.is_empty()
        || credential.refresh_token.is_empty()
    {
        return Err(ProviderError::new(
            ProviderErrorCategory::Authentication,
            "Stored Google Drive authorization is incomplete",
        ));
    }
    Ok(credential)
}

fn remote_file_item(file: GoogleFile) -> ProviderResult<RemoteFileItem> {
    validate_google_file(&file)?;
    let native_export = google_export_mime(&file.mime_type).is_some();
    let kind = match file.mime_type.as_str() {
        GOOGLE_FOLDER_MIME => RemoteFileKind::Folder,
        GOOGLE_SHORTCUT_MIME => RemoteFileKind::Shortcut,
        _ => RemoteFileKind::File,
    };
    Ok(RemoteFileItem {
        id: file.id,
        parent_id: file.parents.into_iter().next(),
        name: file.name,
        kind,
        media_type: Some(file.mime_type),
        size: parse_optional_u64(file.size.as_deref(), "file size")?,
        modified_at: file.modified_time,
        revision: file.version.or(file.md5_checksum),
        downloadable: matches!(kind, RemoteFileKind::File)
            && (file.capabilities.can_download || native_export),
    })
}

fn remote_object_state(file: GoogleFile) -> ProviderResult<RemoteObjectState> {
    validate_google_file(&file)?;
    Ok(RemoteObjectState {
        locator: RemoteObjectLocator {
            opaque_id: file.id,
            revision: file.version.or(file.md5_checksum),
        },
        size: parse_optional_u64(file.size.as_deref(), "object size")?
            .ok_or_else(|| invalid_response("Google Drive object omitted its size"))?,
        content_hash: file.app_properties.get("content_hash").cloned(),
        modified_at: file.modified_time,
    })
}

fn validate_google_file(file: &GoogleFile) -> ProviderResult<()> {
    validate_opaque_id(&file.id, "file ID")?;
    if file.name.is_empty() || file.name.chars().count() > 1024 || file.mime_type.len() > 512 {
        return Err(invalid_response(
            "Google Drive returned invalid file metadata",
        ));
    }
    Ok(())
}

fn google_export_mime(source: &str) -> Option<&'static str> {
    match source {
        "application/vnd.google-apps.document" => Some("text/plain"),
        "application/vnd.google-apps.spreadsheet" => Some("text/csv"),
        "application/vnd.google-apps.presentation" => Some("text/plain"),
        "application/vnd.google-apps.drawing" => Some("image/png"),
        "application/vnd.google-apps.script" => Some("application/vnd.google-apps.script+json"),
        _ => None,
    }
}

fn validate_upload_request(request: &BeginObjectUploadRequest) -> ProviderResult<()> {
    if request.opaque_name.is_empty()
        || request.opaque_name.chars().count() > 255
        || request.content_hash.is_empty()
        || request.content_hash.len() > 128
        || request.media_type.is_empty()
        || request.media_type.len() > 255
        || request.chunk_size == 0
        || !request.chunk_size.is_multiple_of(256 * 1024)
    {
        return Err(invalid_request(
            "Google Drive object metadata or upload chunk size is invalid",
        ));
    }
    Ok(())
}

fn validate_file_upload_request(request: &BeginFileUploadRequest) -> ProviderResult<()> {
    if request.name.trim().is_empty()
        || request.name.chars().count() > 1024
        || request.name.chars().any(char::is_control)
        || request.media_type.is_empty()
        || request.media_type.len() > 255
        || request.chunk_size == 0
        || !request.chunk_size.is_multiple_of(256 * 1024)
    {
        return Err(invalid_request(
            "Google Drive file metadata or upload chunk size is invalid",
        ));
    }
    if let Some(parent_id) = request.parent_id.as_deref() {
        validate_opaque_id(parent_id, "parent folder ID")?;
    }
    Ok(())
}

fn resumable_session_id(response: &Response) -> ProviderResult<String> {
    let session_id = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| invalid_response("Google Drive omitted the resumable upload URL"))?;
    validate_upload_session_url(&session_id)?;
    Ok(session_id)
}

fn validate_upload_session_url(value: &str) -> ProviderResult<()> {
    let url = Url::parse(value).map_err(|_| invalid_request("Invalid resumable upload URL"))?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !(host == "www.googleapis.com" || host.ends_with(".googleapis.com"))
        || !url.path().starts_with("/upload/drive/")
    {
        return Err(invalid_request("Untrusted resumable upload URL"));
    }
    Ok(())
}

fn validate_range(start: Option<u64>, end: Option<u64>) -> ProviderResult<()> {
    if end.is_some() && start.is_none() || start.zip(end).is_some_and(|(start, end)| end < start) {
        Err(invalid_request("Invalid byte range"))
    } else {
        Ok(())
    }
}

fn range_header(start: Option<u64>, end: Option<u64>) -> ProviderResult<Option<HeaderValue>> {
    start
        .map(|start| {
            header_value(
                &match end {
                    Some(end) => format!("bytes={start}-{end}"),
                    None => format!("bytes={start}-"),
                },
                "byte range",
            )
        })
        .transpose()
}

fn parse_uploaded_range(value: &str) -> Option<u64> {
    value
        .strip_prefix("bytes=0-")
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|last| last.checked_add(1))
}

fn parse_optional_u64(value: Option<&str>, label: &str) -> ProviderResult<Option<u64>> {
    value
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| invalid_response(&format!("Google Drive returned an invalid {label}")))
        })
        .transpose()
}

fn validate_opaque_id(value: &str, label: &str) -> ProviderResult<()> {
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        Err(invalid_request(&format!("Invalid Google Drive {label}")))
    } else {
        Ok(())
    }
}

fn stable_account_id(subject: &str) -> String {
    let digest = Sha256::digest(format!("{PROVIDER_ID}:{subject}").as_bytes());
    let mut encoded = String::with_capacity(32);
    for byte in &digest[..16] {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    format!("google-drive-{encoded}")
}

fn path_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn escape_drive_query(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn random_base64(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn scope_granted(scopes: &str, required: &str) -> bool {
    scopes.split_whitespace().any(|scope| scope == required)
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn header_value(value: &str, label: &str) -> ProviderResult<HeaderValue> {
    HeaderValue::from_str(value).map_err(|_| invalid_request(&format!("Invalid {label}")))
}

fn response_stream(response: Response) -> ProviderResult<ProviderByteStream> {
    Ok(Box::pin(response.bytes_stream().map(|result| {
        result.map(|bytes| bytes.to_vec()).map_err(network_error)
    })))
}

async fn parse_json_response<T: for<'de> Deserialize<'de>>(
    response: Response,
) -> ProviderResult<T> {
    response
        .json::<T>()
        .await
        .map_err(|_| invalid_response("Google Drive returned malformed JSON"))
}

async fn response_error(response: Response) -> ProviderError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let bytes = response.bytes().await.unwrap_or_default();
    let body = String::from_utf8_lossy(&bytes);
    let message = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    value
                        .get("error_description")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        })
        .unwrap_or_else(|| format!("Google Drive request failed with HTTP {status}"));
    let category = match status {
        StatusCode::UNAUTHORIZED => ProviderErrorCategory::Authentication,
        StatusCode::FORBIDDEN
            if body.contains("rateLimitExceeded") || body.contains("userRateLimitExceeded") =>
        {
            ProviderErrorCategory::RateLimit
        }
        StatusCode::FORBIDDEN => ProviderErrorCategory::Permission,
        StatusCode::NOT_FOUND => ProviderErrorCategory::NotFound,
        StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => ProviderErrorCategory::Conflict,
        StatusCode::TOO_MANY_REQUESTS => ProviderErrorCategory::RateLimit,
        StatusCode::BAD_REQUEST | StatusCode::RANGE_NOT_SATISFIABLE => {
            ProviderErrorCategory::InvalidRequest
        }
        status if status.is_server_error() => ProviderErrorCategory::RemoteUnavailable,
        _ => ProviderErrorCategory::InvalidResponse,
    };
    let mut error = ProviderError::new(category, truncate(&message, 1000));
    if let Some(seconds) = retry_after {
        error = error.with_retry_after(seconds);
    }
    error
}

fn retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn retry_delay(response: &Response, attempt: usize) -> Duration {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.min(5)))
        .unwrap_or_else(|| Duration::from_millis(250 * (1_u64 << attempt)))
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn invalid_request(message: &str) -> ProviderError {
    ProviderError::new(ProviderErrorCategory::InvalidRequest, message)
}

fn invalid_response(message: &str) -> ProviderError {
    ProviderError::new(ProviderErrorCategory::InvalidResponse, message)
}

fn network_error(error: reqwest::Error) -> ProviderError {
    ProviderError::new(
        ProviderErrorCategory::Network,
        format!("Google Drive network request failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCredentialAccess;

    #[async_trait]
    impl ProviderCredentialAccess for TestCredentialAccess {
        async fn load(&self) -> ProviderResult<Option<Zeroizing<String>>> {
            Ok(Some(Zeroizing::new(
                serde_json::to_string(&GoogleCredential {
                    client_id: "123.apps.googleusercontent.com".into(),
                    client_secret: "secret".into(),
                    access_token: "access".into(),
                    refresh_token: "refresh".into(),
                    token_type: "Bearer".into(),
                    scope: None,
                    expires_at: u64::MAX,
                })
                .unwrap(),
            )))
        }

        async fn store(&self, _credential: Zeroizing<String>) -> ProviderResult<()> {
            Ok(())
        }

        async fn delete(&self) -> ProviderResult<()> {
            Ok(())
        }
    }

    #[test]
    fn desktop_client_validation_rejects_web_and_untrusted_endpoints() {
        let valid = InstalledClient {
            client_id: "123.apps.googleusercontent.com".into(),
            client_secret: "secret".into(),
            auth_uri: "https://accounts.google.com/o/oauth2/auth".into(),
            token_uri: TOKEN_URL.into(),
            redirect_uris: vec!["http://localhost".into()],
        };
        validate_client_config(&valid).unwrap();
        let mut invalid = valid;
        invalid.token_uri = "https://example.com/token".into();
        assert!(validate_client_config(&invalid).is_err());
    }

    #[test]
    fn callback_requires_matching_state_and_extracts_code() {
        let request = b"GET /?state=expected&code=oauth-code HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(
            parse_oauth_callback(request, "expected").unwrap().as_str(),
            "oauth-code"
        );
        assert!(parse_oauth_callback(request, "different").is_err());
    }

    #[test]
    fn drive_items_and_ranges_are_normalized() {
        let item = remote_file_item(GoogleFile {
            id: "id-1".into(),
            name: "Notes".into(),
            mime_type: "application/vnd.google-apps.document".into(),
            version: Some("7".into()),
            capabilities: GoogleFileCapabilities { can_download: true },
            ..GoogleFile::default()
        })
        .unwrap();
        assert_eq!(item.kind, RemoteFileKind::File);
        assert!(item.downloadable);
        assert_eq!(
            google_export_mime(item.media_type.as_deref().unwrap()),
            Some("text/plain")
        );
        assert_eq!(
            range_header(Some(10), Some(20)).unwrap().unwrap(),
            "bytes=10-20"
        );
        assert!(validate_range(None, Some(20)).is_err());
    }

    #[test]
    fn upload_sessions_are_restricted_to_google_drive() {
        assert!(validate_upload_session_url(
            "https://www.googleapis.com/upload/drive/v3/files?upload_id=opaque"
        )
        .is_ok());
        assert!(validate_upload_session_url("https://example.com/upload/drive/v3/files").is_err());
        assert_eq!(parse_uploaded_range("bytes=0-1048575"), Some(1048576));
        assert!(scope_granted(
            &format!("{DRIVE_SCOPE} {DRIVE_APPDATA_SCOPE}"),
            DRIVE_SCOPE
        ));
        assert!(!scope_granted(DRIVE_APPDATA_SCOPE, DRIVE_SCOPE));
        assert!(validate_file_upload_request(&BeginFileUploadRequest {
            parent_id: Some("folder-1".into()),
            name: "notes.txt".into(),
            size: 128,
            media_type: "text/plain".into(),
            chunk_size: 8 * 1024 * 1024,
            content_hashes: Default::default(),
        })
        .is_ok());
    }

    #[tokio::test]
    async fn factory_reuses_one_refresh_lock_per_account() {
        let factory = GoogleDriveFactory::new().unwrap();
        let account = StorageProviderAccount {
            id: "account-1".into(),
            provider_id: PROVIDER_ID.into(),
            display_name: "Google Drive".into(),
            account_subject: None,
            config: json!({}),
        };
        let first = factory
            .connect(&account, Arc::new(TestCredentialAccess))
            .await
            .unwrap();
        let second = factory
            .connect(&account, Arc::new(TestCredentialAccess))
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }
}
