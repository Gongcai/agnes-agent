use futures_util::StreamExt;

use crate::error::{AppError, AppResult};
use crate::sync::artifact::{verify_artifact, BuiltArtifact, VerifiedArtifact};
use crate::sync::crypto::SyncMasterKey;

use super::domain::{
    BeginObjectUploadRequest, DownloadObjectRequest, ObjectPublishMetadata, ProviderError,
    ProviderErrorCategory, RemoteObjectLocator, RemoteObjectState, UploadObjectChunkRequest,
};
use super::ports::ObjectStorageProvider;

const ARTIFACT_MEDIA_TYPE: &str = "application/vnd.agnes.encrypted-artifact";
const DEFAULT_UPLOAD_CHUNK: u64 = 8 * 1024 * 1024;
// R2 requires every non-final multipart part to be at least 5 MiB.
const MIN_UPLOAD_CHUNK: u64 = 5 * 1024 * 1024;
const MAX_UPLOAD_CHUNK: u64 = 64 * 1024 * 1024;
const MAX_DOWNLOAD_ATTEMPTS: usize = 3;

#[derive(Debug, Clone)]
pub struct RemoteArtifactDescriptor {
    pub artifact_id: String,
    pub artifact_type: String,
    pub ciphertext_hash: String,
    pub size: u64,
    pub key_version: i64,
    pub updated_at: i64,
}

/// Uploads an already-encrypted artifact and verifies the final remote object
/// before returning a locator suitable for a replica manifest.
pub async fn upload_artifact(
    provider: &dyn ObjectStorageProvider,
    artifact: &BuiltArtifact,
    preferred_chunk_size: Option<u64>,
    publish: ObjectPublishMetadata,
) -> AppResult<RemoteObjectState> {
    let total_size = artifact.bytes.len() as u64;
    if total_size == 0 || total_size != artifact.manifest.size {
        return Err(AppError::Other(
            "Artifact upload bytes do not match the manifest".into(),
        ));
    }
    let chunk_size = preferred_chunk_size
        .unwrap_or(DEFAULT_UPLOAD_CHUNK)
        .clamp(MIN_UPLOAD_CHUNK, MAX_UPLOAD_CHUNK);
    let session = provider
        .begin_object_upload(BeginObjectUploadRequest {
            opaque_name: format!("{}.agnes-artifact", artifact.manifest.id),
            size: total_size,
            content_hash: artifact.manifest.ciphertext_hash.clone(),
            media_type: ARTIFACT_MEDIA_TYPE.into(),
            chunk_size,
            publish: Some(publish),
        })
        .await
        .map_err(provider_error)?;
    if session.next_offset > total_size {
        let _ = provider.abort_object_upload(&session.session_id).await;
        return Err(AppError::Other(
            "Object Provider returned an invalid upload offset".into(),
        ));
    }
    if session.next_offset == total_size {
        return provider
            .stat_object(&RemoteObjectLocator {
                opaque_id: artifact.manifest.id.clone(),
                revision: None,
            })
            .await
            .map_err(provider_error);
    }
    let result = upload_chunks(
        provider,
        artifact,
        &session.session_id,
        session.next_offset,
        chunk_size,
    )
    .await;
    let object = match result {
        Ok(object) => object,
        Err(error) => {
            let _ = provider.abort_object_upload(&session.session_id).await;
            return Err(error);
        }
    };
    let state = provider
        .stat_object(&object.locator)
        .await
        .map_err(provider_error)?;
    if state.size != artifact.manifest.size
        || state
            .content_hash
            .as_deref()
            .is_some_and(|hash| hash != artifact.manifest.ciphertext_hash)
    {
        let _ = provider.delete_object(&state.locator).await;
        return Err(AppError::Other(
            "Uploaded artifact failed remote size/hash verification".into(),
        ));
    }
    Ok(state)
}

/// Downloads with Range-based resumption, then performs full ciphertext, AEAD,
/// inner-manifest, and plaintext-entry verification.
pub async fn download_artifact(
    provider: &dyn ObjectStorageProvider,
    locator: RemoteObjectLocator,
    expected: &crate::sync::artifact::ArtifactManifest,
    master_key: &SyncMasterKey,
) -> AppResult<VerifiedArtifact> {
    let bytes =
        download_ciphertext(provider, locator, expected.size, &expected.ciphertext_hash).await?;
    verify_artifact(master_key, expected, &bytes)
}

/// Downloads an object when the control plane has only the safe manifest
/// fields. The immutable artifact header supplies the remaining local fields.
pub async fn download_remote_artifact(
    provider: &dyn ObjectStorageProvider,
    locator: RemoteObjectLocator,
    remote: &RemoteArtifactDescriptor,
    master_key: &SyncMasterKey,
) -> AppResult<VerifiedArtifact> {
    let bytes =
        download_ciphertext(provider, locator, remote.size, &remote.ciphertext_hash).await?;
    let manifest = crate::sync::artifact::manifest_from_ciphertext(
        &bytes,
        &remote.artifact_id,
        &remote.artifact_type,
        &remote.ciphertext_hash,
        remote.size,
        remote.key_version,
        remote.updated_at.to_string(),
    )?;
    verify_artifact(master_key, &manifest, &bytes)
}

async fn download_ciphertext(
    provider: &dyn ObjectStorageProvider,
    locator: RemoteObjectLocator,
    expected_size: u64,
    expected_hash: &str,
) -> AppResult<Vec<u8>> {
    let state = provider
        .stat_object(&locator)
        .await
        .map_err(provider_error)?;
    if state.size != expected_size
        || state
            .content_hash
            .as_deref()
            .is_some_and(|hash| hash != expected_hash)
    {
        return Err(AppError::Other(
            "Remote artifact does not match its replica manifest".into(),
        ));
    }
    let capacity = usize::try_from(expected_size)
        .map_err(|_| AppError::Other("Artifact exceeds local memory limits".into()))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut attempts = 0_usize;
    while bytes.len() < capacity {
        let start = bytes.len() as u64;
        let mut stream = provider
            .download_object(DownloadObjectRequest {
                locator: state.locator.clone(),
                range_start: (start > 0).then_some(start),
                range_end_inclusive: None,
            })
            .await
            .map_err(provider_error)?;
        let mut interruption = None;
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => {
                    if bytes.len().saturating_add(chunk.len()) > capacity {
                        return Err(AppError::Other(
                            "Remote artifact exceeded its declared size".into(),
                        ));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Err(error) => {
                    interruption = Some(error);
                    break;
                }
            }
        }
        if bytes.len() == capacity {
            break;
        }
        if attempts >= MAX_DOWNLOAD_ATTEMPTS {
            return Err(interruption.map(provider_error).unwrap_or_else(|| {
                AppError::Other("Remote artifact download ended before the declared size".into())
            }));
        }
        attempts += 1;
    }
    Ok(bytes)
}

async fn upload_chunks(
    provider: &dyn ObjectStorageProvider,
    artifact: &BuiltArtifact,
    session_id: &str,
    mut offset: u64,
    chunk_size: u64,
) -> AppResult<RemoteObjectState> {
    let total_size = artifact.bytes.len() as u64;
    while offset < total_size {
        let end = offset.saturating_add(chunk_size).min(total_size);
        let bytes = artifact.bytes[offset as usize..end as usize].to_vec();
        let result = provider
            .upload_object_chunk(UploadObjectChunkRequest {
                session_id: session_id.into(),
                offset,
                total_size,
                bytes,
            })
            .await
            .map_err(provider_error)?;
        if result.next_offset != end {
            return Err(AppError::Other(
                "Object Provider returned an unexpected upload offset".into(),
            ));
        }
        offset = end;
        if result.complete {
            if offset != total_size {
                return Err(AppError::Other(
                    "Object Provider completed an artifact upload early".into(),
                ));
            }
            return result.object.ok_or_else(|| {
                AppError::Other("Object Provider omitted completed artifact metadata".into())
            });
        }
    }
    Err(AppError::Other(
        "Object Provider did not complete the final artifact chunk".into(),
    ))
}

fn provider_error(error: ProviderError) -> AppError {
    AppError::Other(format!(
        "Object Provider {} error: {}",
        match error.category {
            ProviderErrorCategory::Authentication => "authentication",
            ProviderErrorCategory::Permission => "permission",
            ProviderErrorCategory::RateLimit => "rate-limit",
            ProviderErrorCategory::Network => "network",
            ProviderErrorCategory::NotFound => "not-found",
            ProviderErrorCategory::Conflict => "conflict",
            ProviderErrorCategory::Unsupported => "unsupported",
            ProviderErrorCategory::InvalidRequest => "invalid-request",
            ProviderErrorCategory::RemoteUnavailable => "unavailable",
            ProviderErrorCategory::Cancelled => "cancelled",
            ProviderErrorCategory::InvalidResponse => "invalid-response",
        },
        error.message
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures_util::stream;

    use super::*;
    use crate::storage::domain::{
        ObjectUploadSession, ProviderByteStream, ProviderResult, UploadedObjectChunk,
    };
    use crate::sync::artifact::{
        build_artifact, ArtifactBuildInputs, ArtifactEntry, ARTIFACT_FORMAT_VERSION,
    };

    struct FakeObjectProvider {
        bytes: Mutex<Vec<u8>>,
        hash: Mutex<Option<String>>,
        aborted: AtomicBool,
        downloads: AtomicUsize,
        interrupt_once: bool,
    }

    impl FakeObjectProvider {
        fn new(interrupt_once: bool) -> Self {
            Self {
                bytes: Mutex::new(Vec::new()),
                hash: Mutex::new(None),
                aborted: AtomicBool::new(false),
                downloads: AtomicUsize::new(0),
                interrupt_once,
            }
        }

        fn state(&self) -> RemoteObjectState {
            RemoteObjectState {
                locator: RemoteObjectLocator {
                    opaque_id: "object-1".into(),
                    revision: Some("1".into()),
                },
                size: self.bytes.lock().unwrap().len() as u64,
                content_hash: self.hash.lock().unwrap().clone(),
                modified_at: None,
            }
        }
    }

    #[async_trait]
    impl ObjectStorageProvider for FakeObjectProvider {
        async fn stat_object(
            &self,
            _locator: &RemoteObjectLocator,
        ) -> ProviderResult<RemoteObjectState> {
            Ok(self.state())
        }

        async fn download_object(
            &self,
            request: DownloadObjectRequest,
        ) -> ProviderResult<ProviderByteStream> {
            let start = request.range_start.unwrap_or_default() as usize;
            let bytes = self.bytes.lock().unwrap()[start..].to_vec();
            let attempt = self.downloads.fetch_add(1, Ordering::SeqCst);
            if self.interrupt_once && attempt == 0 && bytes.len() > 1 {
                let midpoint = bytes.len() / 2;
                Ok(Box::pin(stream::iter(vec![
                    Ok(bytes[..midpoint].to_vec()),
                    Err(ProviderError::new(
                        ProviderErrorCategory::Network,
                        "interrupted",
                    )),
                ])))
            } else {
                Ok(Box::pin(stream::iter(vec![Ok(bytes)])))
            }
        }

        async fn begin_object_upload(
            &self,
            request: BeginObjectUploadRequest,
        ) -> ProviderResult<ObjectUploadSession> {
            *self.bytes.lock().unwrap() = Vec::with_capacity(request.size as usize);
            *self.hash.lock().unwrap() = Some(request.content_hash);
            Ok(ObjectUploadSession {
                session_id: "upload-1".into(),
                next_offset: 0,
                expires_at: None,
            })
        }

        async fn upload_object_chunk(
            &self,
            request: UploadObjectChunkRequest,
        ) -> ProviderResult<UploadedObjectChunk> {
            let mut bytes = self.bytes.lock().unwrap();
            if bytes.len() as u64 != request.offset {
                return Err(ProviderError::new(
                    ProviderErrorCategory::Conflict,
                    "offset",
                ));
            }
            bytes.extend_from_slice(&request.bytes);
            let next_offset = bytes.len() as u64;
            let complete = next_offset == request.total_size;
            drop(bytes);
            Ok(UploadedObjectChunk {
                next_offset,
                complete,
                object: complete.then(|| self.state()),
            })
        }

        async fn abort_object_upload(&self, _session_id: &str) -> ProviderResult<()> {
            self.aborted.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn delete_object(&self, _locator: &RemoteObjectLocator) -> ProviderResult<()> {
            self.bytes.lock().unwrap().clear();
            Ok(())
        }
    }

    fn built(key: &SyncMasterKey) -> BuiltArtifact {
        build_artifact(
            key,
            1,
            "knowledge_vectors",
            "version-1",
            &ArtifactBuildInputs {
                source_plaintext_hash: "a".repeat(64),
                parser_profile_fingerprint: None,
                chunker_profile_fingerprint: None,
                embedding_model: Some("test".into()),
                embedding_model_revision: None,
                dims: Some(3),
                normalized: Some(true),
                embedding_instruction_hash: None,
                tokenizer_ref: None,
                artifact_format_version: ARTIFACT_FORMAT_VERSION,
            },
            vec![ArtifactEntry {
                name: "chunks.jsonl".into(),
                media_type: "application/jsonl".into(),
                bytes: vec![7; 700_000],
            }],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn uploads_and_resumes_download_before_verification() {
        let key = SyncMasterKey::generate();
        let artifact = built(&key);
        let provider = FakeObjectProvider::new(true);
        let state = upload_artifact(
            &provider,
            &artifact,
            Some(256 * 1024),
            ObjectPublishMetadata {
                object_id: "object-1".into(),
                object_kind: "knowledge_vectors".into(),
                logical_version: 1,
                key_version: 1,
                updated_hlc: "1-0000-device".into(),
            },
        )
        .await
        .unwrap();
        let verified = download_artifact(&provider, state.locator, &artifact.manifest, &key)
            .await
            .unwrap();
        assert_eq!(verified.entries[0].bytes.len(), 700_000);
        assert_eq!(provider.downloads.load(Ordering::SeqCst), 2);
    }
}
