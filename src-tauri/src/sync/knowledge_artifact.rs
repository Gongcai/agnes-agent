//! Canonical knowledge chunk and vector payloads carried by encrypted artifacts.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::repo::knowledge::{
    KnowledgeArtifactChunk, KnowledgeArtifactEmbeddingProfile, KnowledgeArtifactSnapshot,
};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

use super::artifact::{
    build_artifact, ArtifactBuildInputs, ArtifactEntry, BuiltArtifact, VerifiedArtifact,
    ARTIFACT_FORMAT_VERSION,
};
use super::crypto::SyncMasterKey;

pub const KNOWLEDGE_ARTIFACT_TYPE: &str = "knowledge_vectors";
const PAYLOAD_FORMAT_VERSION: u16 = 1;
const MANIFEST_ENTRY: &str = "manifest.json";
const CHUNKS_ENTRY: &str = "chunks.jsonl";
const VECTORS_ENTRY: &str = "vectors.f32le";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KnowledgePayloadManifest {
    format_version: u16,
    source_version_id: String,
    document_id: String,
    collection_id: String,
    title: String,
    media_type: String,
    source_plaintext_hash: String,
    source_size: u64,
    logical_version: i64,
    parser_profile_id: Option<String>,
    chunker_profile_id: String,
    embedding_profile: KnowledgePayloadEmbeddingProfile,
    chunk_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KnowledgePayloadEmbeddingProfile {
    id: String,
    model_ref: String,
    model_revision: Option<String>,
    dims: usize,
    normalized: bool,
    instruction_hash: String,
    tokenizer_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KnowledgePayloadChunk {
    id: String,
    ordinal: i64,
    content: String,
    content_hash: String,
    page: Option<i64>,
    section_path: Option<String>,
    token_count: i64,
    metadata: serde_json::Value,
    embedding_id: String,
}

pub async fn build_for_document_version(
    db: &DbActorHandle,
    master_key: &SyncMasterKey,
    key_version: i64,
    source_version_id: String,
) -> AppResult<BuiltArtifact> {
    let snapshot = db
        .export_knowledge_artifact_snapshot(source_version_id)
        .await?;
    build_from_snapshot(master_key, key_version, &snapshot)
}

pub async fn apply_verified_artifact(
    db: &DbActorHandle,
    artifact: &VerifiedArtifact,
) -> AppResult<usize> {
    let snapshot = decode_verified_artifact(artifact)?;
    db.import_knowledge_artifact_snapshot(snapshot).await
}

pub fn build_from_snapshot(
    master_key: &SyncMasterKey,
    key_version: i64,
    snapshot: &KnowledgeArtifactSnapshot,
) -> AppResult<BuiltArtifact> {
    validate_snapshot(snapshot)?;
    let payload_manifest = manifest_from_snapshot(snapshot);
    let manifest_bytes = serde_json::to_vec(&payload_manifest)?;
    let mut chunks_bytes = Vec::new();
    let vector_capacity = snapshot
        .chunks
        .len()
        .checked_mul(snapshot.profile.dims)
        .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| AppError::Other("Knowledge artifact vector size overflow".into()))?;
    let mut vectors_bytes = Vec::with_capacity(vector_capacity);
    for chunk in &snapshot.chunks {
        let metadata = serde_json::from_str(&chunk.metadata)
            .map_err(|_| AppError::Other("Knowledge chunk metadata is invalid".into()))?;
        let encoded = serde_json::to_vec(&KnowledgePayloadChunk {
            id: chunk.id.clone(),
            ordinal: chunk.ordinal,
            content: chunk.content.clone(),
            content_hash: chunk.content_hash.clone(),
            page: chunk.page,
            section_path: chunk.section_path.clone(),
            token_count: chunk.token_count,
            metadata,
            embedding_id: chunk.embedding_id.clone(),
        })?;
        chunks_bytes.extend_from_slice(&encoded);
        chunks_bytes.push(b'\n');
        for value in &chunk.vector {
            vectors_bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    build_artifact(
        master_key,
        key_version,
        KNOWLEDGE_ARTIFACT_TYPE,
        &snapshot.source_version_id,
        &build_inputs(snapshot)?,
        vec![
            ArtifactEntry {
                name: MANIFEST_ENTRY.into(),
                media_type: "application/json".into(),
                bytes: manifest_bytes,
            },
            ArtifactEntry {
                name: CHUNKS_ENTRY.into(),
                media_type: "application/x-ndjson".into(),
                bytes: chunks_bytes,
            },
            ArtifactEntry {
                name: VECTORS_ENTRY.into(),
                media_type: "application/vnd.agnes.f32le".into(),
                bytes: vectors_bytes,
            },
        ],
    )
}

pub fn decode_verified_artifact(
    artifact: &VerifiedArtifact,
) -> AppResult<KnowledgeArtifactSnapshot> {
    if artifact.manifest.artifact_type != KNOWLEDGE_ARTIFACT_TYPE {
        return Err(AppError::Other(
            "Artifact is not a knowledge vector snapshot".into(),
        ));
    }
    let entry = |name: &str| {
        artifact
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| AppError::Other(format!("Knowledge artifact is missing {name}")))
    };
    if artifact.entries.len() != 3 {
        return Err(AppError::Other(
            "Knowledge artifact has unexpected entries".into(),
        ));
    }
    let payload: KnowledgePayloadManifest =
        serde_json::from_slice(&entry(MANIFEST_ENTRY)?.bytes)
            .map_err(|_| AppError::Other("Knowledge artifact manifest is invalid".into()))?;
    if payload.format_version != PAYLOAD_FORMAT_VERSION
        || payload.source_version_id != artifact.manifest.source_version_id
        || payload.chunk_count == 0
    {
        return Err(AppError::Other(
            "Knowledge artifact manifest does not match its envelope".into(),
        ));
    }
    let chunks_text = std::str::from_utf8(&entry(CHUNKS_ENTRY)?.bytes)
        .map_err(|_| AppError::Other("Knowledge artifact chunks are not UTF-8".into()))?;
    let mut payload_chunks = Vec::new();
    for line in chunks_text.lines() {
        if line.trim().is_empty() {
            return Err(AppError::Other(
                "Knowledge artifact contains an empty chunk record".into(),
            ));
        }
        payload_chunks.push(
            serde_json::from_str::<KnowledgePayloadChunk>(line)
                .map_err(|_| AppError::Other("Knowledge artifact chunk is invalid".into()))?,
        );
    }
    if payload_chunks.len() != payload.chunk_count {
        return Err(AppError::Other(
            "Knowledge artifact chunk count does not match".into(),
        ));
    }
    let vector_bytes = &entry(VECTORS_ENTRY)?.bytes;
    let expected_vector_bytes = payload
        .chunk_count
        .checked_mul(payload.embedding_profile.dims)
        .and_then(|values| values.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| AppError::Other("Knowledge artifact vector size overflow".into()))?;
    if vector_bytes.len() != expected_vector_bytes {
        return Err(AppError::Other(
            "Knowledge artifact vector byte length does not match".into(),
        ));
    }
    let mut vectors = vector_bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four-byte vector element")));
    let chunks = payload_chunks
        .into_iter()
        .map(|chunk| {
            let vector = vectors
                .by_ref()
                .take(payload.embedding_profile.dims)
                .collect::<Vec<_>>();
            KnowledgeArtifactChunk {
                id: chunk.id,
                ordinal: chunk.ordinal,
                content: chunk.content,
                content_hash: chunk.content_hash,
                page: chunk.page,
                section_path: chunk.section_path,
                token_count: chunk.token_count,
                metadata: serde_json::to_string(&chunk.metadata)
                    .expect("validated JSON value serializes"),
                embedding_id: chunk.embedding_id,
                vector,
            }
        })
        .collect::<Vec<_>>();
    let snapshot = KnowledgeArtifactSnapshot {
        source_version_id: payload.source_version_id,
        document_id: payload.document_id,
        collection_id: payload.collection_id,
        title: payload.title,
        media_type: payload.media_type,
        source_plaintext_hash: payload.source_plaintext_hash,
        source_size: payload.source_size,
        logical_version: payload.logical_version,
        parser_profile_id: payload.parser_profile_id,
        chunker_profile_id: payload.chunker_profile_id,
        profile: KnowledgeArtifactEmbeddingProfile {
            id: payload.embedding_profile.id,
            model_ref: payload.embedding_profile.model_ref,
            model_revision: payload.embedding_profile.model_revision,
            dims: payload.embedding_profile.dims,
            normalized: payload.embedding_profile.normalized,
            instruction_hash: payload.embedding_profile.instruction_hash,
            tokenizer_ref: payload.embedding_profile.tokenizer_ref,
        },
        chunks,
    };
    validate_snapshot(&snapshot)?;
    if build_inputs(&snapshot)?.build_fingerprint()? != artifact.manifest.build_fingerprint {
        return Err(AppError::Other(
            "Knowledge artifact build fingerprint does not match".into(),
        ));
    }
    Ok(snapshot)
}

fn manifest_from_snapshot(snapshot: &KnowledgeArtifactSnapshot) -> KnowledgePayloadManifest {
    KnowledgePayloadManifest {
        format_version: PAYLOAD_FORMAT_VERSION,
        source_version_id: snapshot.source_version_id.clone(),
        document_id: snapshot.document_id.clone(),
        collection_id: snapshot.collection_id.clone(),
        title: snapshot.title.clone(),
        media_type: snapshot.media_type.clone(),
        source_plaintext_hash: snapshot.source_plaintext_hash.clone(),
        source_size: snapshot.source_size,
        logical_version: snapshot.logical_version,
        parser_profile_id: snapshot.parser_profile_id.clone(),
        chunker_profile_id: snapshot.chunker_profile_id.clone(),
        embedding_profile: KnowledgePayloadEmbeddingProfile {
            id: snapshot.profile.id.clone(),
            model_ref: snapshot.profile.model_ref.clone(),
            model_revision: snapshot.profile.model_revision.clone(),
            dims: snapshot.profile.dims,
            normalized: snapshot.profile.normalized,
            instruction_hash: snapshot.profile.instruction_hash.clone(),
            tokenizer_ref: snapshot.profile.tokenizer_ref.clone(),
        },
        chunk_count: snapshot.chunks.len(),
    }
}

fn build_inputs(snapshot: &KnowledgeArtifactSnapshot) -> AppResult<ArtifactBuildInputs> {
    let dims = u32::try_from(snapshot.profile.dims)
        .map_err(|_| AppError::Other("Knowledge embedding dimensions are invalid".into()))?;
    Ok(ArtifactBuildInputs {
        source_plaintext_hash: snapshot.source_plaintext_hash.clone(),
        parser_profile_fingerprint: snapshot.parser_profile_id.clone(),
        chunker_profile_fingerprint: Some(snapshot.chunker_profile_id.clone()),
        embedding_model: Some(snapshot.profile.model_ref.clone()),
        embedding_model_revision: snapshot.profile.model_revision.clone(),
        dims: Some(dims),
        normalized: Some(snapshot.profile.normalized),
        embedding_instruction_hash: Some(snapshot.profile.instruction_hash.clone()),
        tokenizer_ref: snapshot.profile.tokenizer_ref.clone(),
        artifact_format_version: ARTIFACT_FORMAT_VERSION,
    })
}

fn validate_snapshot(snapshot: &KnowledgeArtifactSnapshot) -> AppResult<()> {
    if snapshot.source_version_id.trim().is_empty()
        || snapshot.document_id.trim().is_empty()
        || snapshot.collection_id.trim().is_empty()
        || snapshot.title.trim().is_empty()
        || snapshot.media_type.trim().is_empty()
        || !is_sha256(&snapshot.source_plaintext_hash)
        || snapshot.logical_version <= 0
        || snapshot.chunker_profile_id.trim().is_empty()
        || snapshot.profile.id.trim().is_empty()
        || snapshot.profile.model_ref.trim().is_empty()
        || snapshot.profile.instruction_hash.trim().is_empty()
        || snapshot.profile.dims == 0
        || snapshot.profile.dims > 8_192
        || snapshot.chunks.is_empty()
    {
        return Err(AppError::Other(
            "Knowledge artifact snapshot is invalid".into(),
        ));
    }
    for (ordinal, chunk) in snapshot.chunks.iter().enumerate() {
        if chunk.ordinal != ordinal as i64
            || chunk.id.trim().is_empty()
            || chunk.embedding_id.trim().is_empty()
            || chunk.vector.len() != snapshot.profile.dims
            || chunk.vector.iter().any(|value| !value.is_finite())
            || sha256_hex(chunk.content.as_bytes()) != chunk.content_hash
            || serde_json::from_str::<serde_json::Value>(&chunk.metadata).is_err()
        {
            return Err(AppError::Other(
                "Knowledge artifact chunk is invalid".into(),
            ));
        }
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> KnowledgeArtifactSnapshot {
        KnowledgeArtifactSnapshot {
            source_version_id: "version-1".into(),
            document_id: "document-1".into(),
            collection_id: "collection-1".into(),
            title: "Test document".into(),
            media_type: "text/plain".into(),
            source_plaintext_hash: sha256_hex(b"source"),
            source_size: 6,
            logical_version: 1,
            parser_profile_id: Some("parser-v1".into()),
            chunker_profile_id: "chunker-v1".into(),
            profile: KnowledgeArtifactEmbeddingProfile {
                id: "local-rag-9de115734643c3c721f27a09".into(),
                model_ref: "test/embed".into(),
                model_revision: Some("r1".into()),
                dims: 3,
                normalized: true,
                instruction_hash: "local-rag-v1".into(),
                tokenizer_ref: None,
            },
            chunks: vec![KnowledgeArtifactChunk {
                id: "chunk-1".into(),
                ordinal: 0,
                content: "portable chunk".into(),
                content_hash: sha256_hex(b"portable chunk"),
                page: None,
                section_path: Some("Section".into()),
                token_count: 2,
                metadata: "{}".into(),
                embedding_id: "embedding-1".into(),
                vector: vec![1.0, 0.0, 0.0],
            }],
        }
    }

    #[test]
    fn knowledge_payload_round_trips_through_verified_artifact() {
        let key = SyncMasterKey::generate();
        let source = snapshot();
        let built = build_from_snapshot(&key, 1, &source).unwrap();
        let verified =
            super::super::artifact::verify_artifact(&key, &built.manifest, &built.bytes).unwrap();
        let decoded = decode_verified_artifact(&verified).unwrap();
        assert_eq!(decoded.source_version_id, source.source_version_id);
        assert_eq!(decoded.chunks[0].content, source.chunks[0].content);
        assert_eq!(decoded.chunks[0].vector, source.chunks[0].vector);
    }

    #[test]
    fn malformed_vector_payload_is_rejected_before_database_import() {
        let key = SyncMasterKey::generate();
        let source = snapshot();
        let built = build_from_snapshot(&key, 1, &source).unwrap();
        let mut verified =
            super::super::artifact::verify_artifact(&key, &built.manifest, &built.bytes).unwrap();
        verified
            .entries
            .iter_mut()
            .find(|entry| entry.name == VECTORS_ENTRY)
            .unwrap()
            .bytes
            .pop();
        assert!(decode_verified_artifact(&verified).is_err());
    }
}
