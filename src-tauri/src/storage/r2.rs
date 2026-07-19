use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, ETAG, RANGE, RETRY_AFTER};
use reqwest::{Method, Response, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use super::domain::{
    BeginObjectUploadRequest, DownloadObjectRequest, ObjectPublishMetadata, ObjectUploadSession,
    ProviderAuthKind, ProviderByteStream, ProviderDescriptor, ProviderError, ProviderErrorCategory,
    ProviderResult, ProviderStability, RemoteObjectLocator, RemoteObjectState, StorageCapabilities,
    StorageProviderAccount, UploadObjectChunkRequest, UploadedObjectChunk,
};
use super::ports::{
    ObjectStorageProvider, ProviderCredentialAccess, ProviderFactory, ProviderSession,
};

const PROVIDER_ID: &str = "r2";
const USER_AGENT: &str = "Agnes-Agent/0.1 R2WorkerProvider";
const ARTIFACT_SUFFIX: &str = ".agnes-artifact";
const MIN_MULTIPART_PART_BYTES: u64 = 5 * 1024 * 1024;

pub struct R2Factory {
    client: reqwest::Client,
}

impl R2Factory {
    pub fn new() -> ProviderResult<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .build()
            .map_err(network_error)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl ProviderFactory for R2Factory {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: PROVIDER_ID.into(),
            display_name: "Cloudflare R2".into(),
            auth_kind: ProviderAuthKind::Managed,
            stability: ProviderStability::Official,
            implementation_version: "worker-r2-v1".into(),
            capabilities: StorageCapabilities {
                object_storage: true,
                range_download: true,
                resumable_upload: true,
                conditional_write: true,
                stable_revisions: true,
                stable_file_sizes: true,
                worker_proxy: true,
                max_object_bytes: Some(5 * 1024 * 1024 * 1024 * 1024),
                recommended_chunk_bytes: Some(8 * 1024 * 1024),
                ..StorageCapabilities::default()
            },
        }
    }

    async fn connect(
        &self,
        account: &StorageProviderAccount,
        credentials: Arc<dyn ProviderCredentialAccess>,
    ) -> ProviderResult<Arc<dyn ProviderSession>> {
        if account.provider_id != PROVIDER_ID {
            return Err(invalid_request("R2 received another provider account"));
        }
        let base_url = account
            .config
            .get("base_url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_request("R2 Worker base_url is required"))?;
        let base_url = parse_base_url(base_url)?;
        let token = credentials.load().await?.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCategory::Authentication,
                "R2 Worker bearer credential is missing on this device",
            )
        })?;
        if token.is_empty() || token.len() > 512 {
            return Err(ProviderError::new(
                ProviderErrorCategory::Authentication,
                "R2 Worker bearer credential is invalid",
            ));
        }
        Ok(Arc::new(R2Session {
            client: self.client.clone(),
            base_url,
            bearer: token,
            uploads: Mutex::new(HashMap::new()),
        }))
    }
}

struct R2Session {
    client: reqwest::Client,
    base_url: Url,
    bearer: Zeroizing<String>,
    uploads: Mutex<HashMap<String, UploadState>>,
}

struct UploadState {
    artifact_id: String,
    chunk_size: u64,
    total_size: u64,
    next_offset: u64,
}

impl ProviderSession for R2Session {
    fn object_storage(&self) -> Option<&dyn ObjectStorageProvider> {
        Some(self)
    }
}

#[async_trait]
impl ObjectStorageProvider for R2Session {
    async fn stat_object(
        &self,
        locator: &RemoteObjectLocator,
    ) -> ProviderResult<RemoteObjectState> {
        validate_artifact_id(&locator.opaque_id)?;
        let response = self
            .send(
                Method::HEAD,
                &format!("v1/objects/{}", locator.opaque_id),
                None,
                None,
            )
            .await?;
        let size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| invalid_response("R2 Worker omitted object size"))?;
        let revision = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if locator
            .revision
            .as_deref()
            .is_some_and(|expected| revision.as_deref() != Some(expected))
        {
            return Err(ProviderError::new(
                ProviderErrorCategory::Conflict,
                "R2 object revision changed",
            ));
        }
        Ok(RemoteObjectState {
            locator: RemoteObjectLocator {
                opaque_id: locator.opaque_id.clone(),
                revision,
            },
            size,
            content_hash: response
                .headers()
                .get("x-agnes-ciphertext-hash")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
            modified_at: None,
        })
    }

    async fn download_object(
        &self,
        request: DownloadObjectRequest,
    ) -> ProviderResult<ProviderByteStream> {
        validate_artifact_id(&request.locator.opaque_id)?;
        let range = encode_range(request.range_start, request.range_end_inclusive)?;
        let response = self
            .send(
                Method::GET,
                &format!("v1/objects/{}", request.locator.opaque_id),
                None,
                range.as_deref(),
            )
            .await?;
        let stream = response
            .bytes_stream()
            .map(|result| result.map(|bytes| bytes.to_vec()).map_err(network_error));
        Ok(Box::pin(stream))
    }

    async fn begin_object_upload(
        &self,
        request: BeginObjectUploadRequest,
    ) -> ProviderResult<ObjectUploadSession> {
        let publish = request
            .publish
            .ok_or_else(|| invalid_request("R2 artifact publish metadata is required"))?;
        let artifact_id = request
            .opaque_name
            .strip_suffix(ARTIFACT_SUFFIX)
            .unwrap_or(request.opaque_name.as_str())
            .to_string();
        validate_artifact_id(&artifact_id)?;
        validate_publish_metadata(&publish, request.size)?;
        if request.chunk_size < MIN_MULTIPART_PART_BYTES {
            return Err(invalid_request(
                "R2 upload chunk size must be at least 5 MiB",
            ));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct BeginResponse {
            status: String,
            upload_session_id: Option<String>,
        }
        let response: BeginResponse = self
            .send_json(
                Method::POST,
                "v1/objects/uploads",
                &json!({
                    "protocolVersion": 1,
                    "objectId": publish.object_id,
                    "objectKind": publish.object_kind,
                    "logicalVersion": publish.logical_version,
                    "artifactId": artifact_id,
                    "ciphertextHash": request.content_hash,
                    "size": request.size,
                    "keyVersion": publish.key_version,
                    "updatedHlc": publish.updated_hlc,
                }),
            )
            .await?;
        if response.status == "ready" {
            return Ok(ObjectUploadSession {
                session_id: format!("ready:{artifact_id}"),
                next_offset: request.size,
                expires_at: None,
            });
        }
        if response.status != "pending" {
            return Err(invalid_response(
                "R2 Worker returned an unknown upload status",
            ));
        }
        let session_id = response
            .upload_session_id
            .ok_or_else(|| invalid_response("R2 Worker omitted upload session ID"))?;
        self.uploads.lock().await.insert(
            session_id.clone(),
            UploadState {
                artifact_id,
                chunk_size: request.chunk_size,
                total_size: request.size,
                next_offset: 0,
            },
        );
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
        let mut uploads = self.uploads.lock().await;
        let state = uploads
            .get_mut(&request.session_id)
            .ok_or_else(|| invalid_request("R2 upload session is unknown or already completed"))?;
        if request.offset != state.next_offset
            || request.total_size != state.total_size
            || request.bytes.is_empty()
            || request.bytes.len() as u64 > state.chunk_size
            || request.offset.saturating_add(request.bytes.len() as u64) > state.total_size
        {
            return Err(ProviderError::new(
                ProviderErrorCategory::Conflict,
                "R2 upload offset does not match the next part",
            ));
        }
        let part_number = request.offset / state.chunk_size + 1;
        if part_number > 10_000 {
            return Err(invalid_request("R2 upload has too many parts"));
        }
        let checksum = sha256_hex(&request.bytes);
        let chunk_len = request.bytes.len() as u64;
        let path = format!(
            "v1/objects/uploads/{}/parts/{part_number}",
            request.session_id
        );
        drop(uploads);
        self.send_binary(Method::PUT, &path, request.bytes, &checksum)
            .await?;
        let mut uploads = self.uploads.lock().await;
        let state = uploads
            .get_mut(&request.session_id)
            .ok_or_else(|| invalid_request("R2 upload session disappeared"))?;
        // The caller sends chunks at the configured size, except the final chunk.
        state.next_offset = request.offset + chunk_len;
        let complete = state.next_offset == request.total_size;
        if !complete {
            return Ok(UploadedObjectChunk {
                next_offset: state.next_offset,
                complete: false,
                object: None,
            });
        }
        let session_id = request.session_id.clone();
        let artifact_id = state.artifact_id.clone();
        drop(uploads);
        self.send_empty(
            Method::POST,
            &format!("v1/objects/uploads/{session_id}/complete"),
        )
        .await?;
        let object = self
            .stat_object(&RemoteObjectLocator {
                opaque_id: artifact_id,
                revision: None,
            })
            .await?;
        self.uploads.lock().await.remove(&request.session_id);
        Ok(UploadedObjectChunk {
            next_offset: request.total_size,
            complete: true,
            object: Some(object),
        })
    }

    async fn abort_object_upload(&self, session_id: &str) -> ProviderResult<()> {
        if session_id.starts_with("ready:") {
            return Ok(());
        }
        self.send_empty(Method::DELETE, &format!("v1/objects/uploads/{session_id}"))
            .await?;
        self.uploads.lock().await.remove(session_id);
        Ok(())
    }

    async fn delete_object(&self, locator: &RemoteObjectLocator) -> ProviderResult<()> {
        validate_artifact_id(&locator.opaque_id)?;
        self.send_empty(Method::DELETE, &format!("v1/objects/{}", locator.opaque_id))
            .await
    }
}

impl R2Session {
    fn url(&self, path: &str) -> ProviderResult<Url> {
        self.base_url
            .join(path)
            .map_err(|_| invalid_request("R2 Worker URL is invalid"))
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        range: Option<&str>,
    ) -> ProviderResult<Response> {
        let url = self.url(path)?;
        let mut request = self.client.request(method, url);
        request = request.header(AUTHORIZATION, format!("Bearer {}", self.bearer.as_str()));
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(body);
        }
        let response = request.send().await.map_err(network_error)?;
        if response.status().is_success() {
            return Ok(response);
        }
        Err(status_error(&response))
    }

    async fn send_json<T: serde::Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &T,
    ) -> ProviderResult<R> {
        let response = self
            .send(
                method,
                path,
                Some(serde_json::to_string(body).map_err(json_error)?),
                None,
            )
            .await?;
        response
            .json()
            .await
            .map_err(|_| invalid_response("R2 Worker returned invalid JSON"))
    }

    async fn send_empty(&self, method: Method, path: &str) -> ProviderResult<()> {
        let _ = self.send(method, path, None, None).await?;
        Ok(())
    }

    async fn send_binary(
        &self,
        method: Method,
        path: &str,
        bytes: Vec<u8>,
        checksum: &str,
    ) -> ProviderResult<()> {
        let url = self.url(path)?;
        let response = self
            .client
            .request(method, url)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer.as_str()))
            .header(CONTENT_TYPE, "application/octet-stream")
            .header("Content-Length", bytes.len())
            .header("X-Agnes-Part-Sha256", checksum)
            .body(bytes)
            .send()
            .await
            .map_err(network_error)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(status_error(&response))
        }
    }
}

fn parse_base_url(value: &str) -> ProviderResult<Url> {
    let mut url =
        Url::parse(value).map_err(|_| invalid_request("R2 Worker base_url is invalid"))?;
    if !matches!(url.scheme(), "https" | "http") || url.username() != "" || url.password().is_some()
    {
        return Err(invalid_request(
            "R2 Worker base_url must be an HTTP(S) URL without credentials",
        ));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

fn validate_artifact_id(value: &str) -> ProviderResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(invalid_request("R2 artifact ID is invalid"));
    }
    Ok(())
}

fn validate_publish_metadata(metadata: &ObjectPublishMetadata, size: u64) -> ProviderResult<()> {
    validate_artifact_id(&metadata.object_id)?;
    if metadata.object_kind.is_empty()
        || metadata.object_kind.len() > 64
        || metadata.logical_version == 0
        || metadata.key_version <= 0
        || metadata.updated_hlc.is_empty()
        || size == 0
    {
        return Err(invalid_request("R2 artifact publish metadata is invalid"));
    }
    Ok(())
}

fn encode_range(start: Option<u64>, end: Option<u64>) -> ProviderResult<Option<String>> {
    match (start, end) {
        (None, None) => Ok(None),
        (Some(start), Some(end)) if end >= start => Ok(Some(format!("bytes={start}-{end}"))),
        (Some(start), None) => Ok(Some(format!("bytes={start}-"))),
        _ => Err(invalid_request("R2 object range is invalid")),
    }
}

fn status_error(response: &Response) -> ProviderError {
    let category = match response.status() {
        StatusCode::UNAUTHORIZED => ProviderErrorCategory::Authentication,
        StatusCode::FORBIDDEN => ProviderErrorCategory::Permission,
        StatusCode::NOT_FOUND => ProviderErrorCategory::NotFound,
        StatusCode::CONFLICT => ProviderErrorCategory::Conflict,
        StatusCode::TOO_MANY_REQUESTS => ProviderErrorCategory::RateLimit,
        status if status.is_server_error() => ProviderErrorCategory::RemoteUnavailable,
        _ => ProviderErrorCategory::InvalidResponse,
    };
    let mut error = ProviderError::new(
        category,
        format!("R2 Worker returned HTTP {}", response.status()),
    );
    if category == ProviderErrorCategory::RateLimit {
        error.retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
    }
    error
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
        format!("R2 Worker network error: {error}"),
    )
}

fn json_error(error: serde_json::Error) -> ProviderError {
    ProviderError::new(
        ProviderErrorCategory::InvalidRequest,
        format!("R2 request JSON error: {error}"),
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_worker_urls_and_ranges() {
        assert!(parse_base_url("https://sync.example.test/").is_ok());
        assert!(parse_base_url("https://user:pass@sync.example.test/").is_err());
        assert_eq!(
            encode_range(Some(4), Some(8)).unwrap().as_deref(),
            Some("bytes=4-8")
        );
        assert!(encode_range(Some(8), Some(4)).is_err());
    }

    #[test]
    fn descriptor_is_managed_worker_object_storage() {
        let factory = R2Factory::new().unwrap();
        let descriptor = factory.descriptor();
        assert_eq!(descriptor.id, "r2");
        assert!(descriptor.capabilities.worker_proxy);
        assert!(descriptor.capabilities.resumable_upload);
    }
}
