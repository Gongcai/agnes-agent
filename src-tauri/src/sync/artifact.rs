//! Portable encrypted artifacts for source/chunk/vector snapshots.
//!
//! The format intentionally keeps the control-plane manifest separate from the
//! encrypted payload. The payload contains a small canonical manifest followed
//! by independently compressed entries, all protected by chunked AEAD.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::crypto::{
    decrypt_artifact_chunk, encrypt_artifact_chunk, generate_artifact_data_key,
    unwrap_artifact_data_key, wrap_artifact_data_key, SyncMasterKey, ARTIFACT_NONCE_BYTES,
    ARTIFACT_TAG_BYTES,
};

pub const ARTIFACT_FORMAT_VERSION: u16 = 1;
pub const ARTIFACT_ENCRYPTION_SCHEME: &str = "xchacha20poly1305-chunked-v1";
pub const ARTIFACT_INNER_FORMAT: &str = "manifest+zstd-v1";
pub const DEFAULT_CHUNK_SIZE: usize = 1024 * 1024;
const OUTER_MAGIC: &[u8; 8] = b"AGNSART1";
const INNER_MAGIC: &[u8; 8] = b"AGNSINR1";
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_ENTRY_COUNT: usize = 100_000;
const MAX_ENTRY_NAME_BYTES: usize = 256;
const MAX_CHUNK_SIZE: usize = 16 * 1024 * 1024;
const MAX_CHUNK_COUNT: u64 = 16 * 1024 * 1024;
const ARTIFACT_AAD: &[u8] = b"agnes-artifact-chunk-aad-v1\0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactBuildInputs {
    pub source_plaintext_hash: String,
    pub parser_profile_fingerprint: Option<String>,
    pub chunker_profile_fingerprint: Option<String>,
    pub embedding_model: Option<String>,
    pub embedding_model_revision: Option<String>,
    pub dims: Option<u32>,
    pub normalized: Option<bool>,
    pub embedding_instruction_hash: Option<String>,
    pub tokenizer_ref: Option<String>,
    pub artifact_format_version: u16,
}

impl ArtifactBuildInputs {
    pub fn build_fingerprint(&self) -> AppResult<String> {
        if self.artifact_format_version != ARTIFACT_FORMAT_VERSION
            || !is_sha256_hex(&self.source_plaintext_hash)
        {
            return Err(AppError::Other("Artifact build inputs are invalid".into()));
        }
        let encoded = serde_json::to_vec(self)?;
        Ok(sha256_hex(&encoded))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactEntryDescriptor {
    pub name: String,
    pub media_type: String,
    pub compression: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InnerManifest {
    format_version: u16,
    artifact_id: String,
    build_fingerprint: String,
    inner_format: String,
    entries: Vec<ArtifactEntryDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WrappedKeyManifest {
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OuterHeader {
    format_version: u16,
    artifact_id: String,
    artifact_type: String,
    source_version_id: String,
    build_fingerprint: String,
    inner_format: String,
    plaintext_hash: String,
    plaintext_size: u64,
    key_version: i64,
    encryption_scheme: String,
    chunk_size: u32,
    chunk_count: u64,
    wrapped_key: WrappedKeyManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactManifest {
    pub id: String,
    pub artifact_type: String,
    pub source_version_id: String,
    pub build_fingerprint: String,
    pub format_version: u16,
    pub plaintext_hash: String,
    pub ciphertext_hash: String,
    pub plaintext_size: u64,
    pub size: u64,
    pub encryption_scheme: String,
    pub key_version: i64,
    pub chunk_size: u32,
    pub chunk_count: u64,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactEntry {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BuiltArtifact {
    pub manifest: ArtifactManifest,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct VerifiedArtifact {
    pub manifest: ArtifactManifest,
    pub entries: Vec<ArtifactEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallPointer {
    artifact_id: String,
    installed_at: String,
}

/// Builds one immutable artifact. The caller can cache the result by
/// `(source_version_id, build_fingerprint)` before uploading it to a Provider.
pub fn build_artifact(
    master_key: &SyncMasterKey,
    key_version: i64,
    artifact_type: &str,
    source_version_id: &str,
    build_inputs: &ArtifactBuildInputs,
    entries: Vec<ArtifactEntry>,
) -> AppResult<BuiltArtifact> {
    validate_identity(artifact_type, "artifact type", 80)?;
    validate_identity(source_version_id, "source version ID", 160)?;
    if key_version <= 0 || entries.is_empty() || entries.len() > MAX_ENTRY_COUNT {
        return Err(AppError::Other("Artifact build request is invalid".into()));
    }
    let build_fingerprint = build_inputs.build_fingerprint()?;
    let artifact_id = format!("artifact-{}", Uuid::new_v4());
    let inner = build_inner_payload(&artifact_id, &build_fingerprint, entries)?;
    let plaintext_hash = sha256_hex(&inner);
    let plaintext_size = inner.len() as u64;
    let chunk_size = DEFAULT_CHUNK_SIZE;
    let data_key = generate_artifact_data_key();
    let wrapped = wrap_artifact_data_key(master_key, &artifact_id, key_version, &data_key)?;
    let chunk_count = div_ceil(plaintext_size, chunk_size as u64);
    if chunk_count == 0 || chunk_count > MAX_CHUNK_COUNT {
        return Err(AppError::Other("Artifact is too large".into()));
    }
    let wrapped_key = WrappedKeyManifest {
        nonce: STANDARD_NO_PAD.encode(wrapped.nonce),
        ciphertext: STANDARD_NO_PAD.encode(wrapped.ciphertext),
    };
    let header = OuterHeader {
        format_version: ARTIFACT_FORMAT_VERSION,
        artifact_id: artifact_id.clone(),
        artifact_type: artifact_type.to_string(),
        source_version_id: source_version_id.to_string(),
        build_fingerprint: build_fingerprint.clone(),
        inner_format: ARTIFACT_INNER_FORMAT.into(),
        plaintext_hash: plaintext_hash.clone(),
        plaintext_size,
        key_version,
        encryption_scheme: ARTIFACT_ENCRYPTION_SCHEME.into(),
        chunk_size: chunk_size as u32,
        chunk_count,
        wrapped_key,
    };
    let header_bytes = serde_json::to_vec(&header)?;
    if header_bytes.len() > MAX_HEADER_BYTES {
        return Err(AppError::Other("Artifact header is too large".into()));
    }
    let header_hash = Sha256::digest(&header_bytes);
    let mut output = Vec::with_capacity(OUTER_MAGIC.len() + 4 + header_bytes.len() + inner.len());
    output.extend_from_slice(OUTER_MAGIC);
    output.extend_from_slice(&(header_bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(&header_bytes);
    for (index, chunk) in inner.chunks(chunk_size).enumerate() {
        let mut nonce = [0_u8; ARTIFACT_NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce);
        let aad = chunk_aad(&header_hash, index as u64, chunk.len() as u64);
        let ciphertext = encrypt_artifact_chunk(&data_key, &nonce, &aad, chunk)?;
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&(ciphertext.len() as u32).to_be_bytes());
        output.extend_from_slice(&ciphertext);
    }
    let manifest = ArtifactManifest {
        id: artifact_id,
        artifact_type: artifact_type.to_string(),
        source_version_id: source_version_id.to_string(),
        build_fingerprint,
        format_version: ARTIFACT_FORMAT_VERSION,
        plaintext_hash,
        ciphertext_hash: sha256_hex(&output),
        plaintext_size,
        size: output.len() as u64,
        encryption_scheme: ARTIFACT_ENCRYPTION_SCHEME.into(),
        key_version,
        chunk_size: chunk_size as u32,
        chunk_count,
        created_at: now_string(),
    };
    Ok(BuiltArtifact {
        manifest,
        bytes: output,
    })
}

/// Verifies ciphertext framing, AEAD tags, the outer manifest, and every
/// compressed entry before exposing any plaintext to the installer.
pub fn verify_artifact(
    master_key: &SyncMasterKey,
    expected: &ArtifactManifest,
    bytes: &[u8],
) -> AppResult<VerifiedArtifact> {
    if expected.format_version != ARTIFACT_FORMAT_VERSION
        || expected.encryption_scheme != ARTIFACT_ENCRYPTION_SCHEME
        || expected.size != bytes.len() as u64
        || expected.ciphertext_hash != sha256_hex(bytes)
    {
        return Err(AppError::Other(
            "Artifact ciphertext hash or size is invalid".into(),
        ));
    }
    let (header, header_bytes, mut cursor) = parse_header(bytes)?;
    if header.format_version != expected.format_version
        || header.artifact_id != expected.id
        || header.artifact_type != expected.artifact_type
        || header.source_version_id != expected.source_version_id
        || header.build_fingerprint != expected.build_fingerprint
        || header.inner_format != ARTIFACT_INNER_FORMAT
        || header.plaintext_hash != expected.plaintext_hash
        || header.plaintext_size != expected.plaintext_size
        || header.key_version != expected.key_version
        || header.encryption_scheme != expected.encryption_scheme
        || header.chunk_size != expected.chunk_size
        || header.chunk_count != expected.chunk_count
        || header.chunk_count != div_ceil(header.plaintext_size, header.chunk_size as u64)
        || header.plaintext_size > bytes.len() as u64
    {
        return Err(AppError::Other(
            "Artifact header does not match its manifest".into(),
        ));
    }
    let nonce = STANDARD_NO_PAD
        .decode(header.wrapped_key.nonce.as_bytes())
        .map_err(|_| AppError::Other("Artifact wrapped-key nonce is invalid".into()))?;
    let wrapped = STANDARD_NO_PAD
        .decode(header.wrapped_key.ciphertext.as_bytes())
        .map_err(|_| AppError::Other("Artifact wrapped key is invalid".into()))?;
    let data_key = unwrap_artifact_data_key(
        master_key,
        &header.artifact_id,
        header.key_version,
        &nonce,
        &wrapped,
    )?;
    let header_hash = Sha256::digest(&header_bytes);
    let mut plaintext = Vec::with_capacity(header.plaintext_size as usize);
    for index in 0..header.chunk_count {
        if cursor + ARTIFACT_NONCE_BYTES + 4 > bytes.len() {
            return Err(AppError::Other(
                "Artifact chunk framing is truncated".into(),
            ));
        }
        let mut nonce = [0_u8; ARTIFACT_NONCE_BYTES];
        nonce.copy_from_slice(&bytes[cursor..cursor + ARTIFACT_NONCE_BYTES]);
        cursor += ARTIFACT_NONCE_BYTES;
        let ciphertext_len = u32::from_be_bytes(
            bytes[cursor..cursor + 4]
                .try_into()
                .map_err(|_| AppError::Other("Artifact chunk length is invalid".into()))?,
        ) as usize;
        cursor += 4;
        if ciphertext_len < ARTIFACT_TAG_BYTES || cursor + ciphertext_len > bytes.len() {
            return Err(AppError::Other("Artifact chunk length is invalid".into()));
        }
        let expected_plaintext_len = if index + 1 == header.chunk_count {
            let offset = (header.chunk_size as u64)
                .checked_mul(index)
                .ok_or_else(|| AppError::Other("Artifact chunk offset overflow".into()))?;
            usize::try_from(
                header
                    .plaintext_size
                    .checked_sub(offset)
                    .ok_or_else(|| AppError::Other("Artifact chunk offset is invalid".into()))?,
            )
            .map_err(|_| AppError::Other("Artifact chunk size exceeds local limits".into()))?
        } else {
            header.chunk_size as usize
        };
        if expected_plaintext_len == 0
            || ciphertext_len != expected_plaintext_len + ARTIFACT_TAG_BYTES
        {
            return Err(AppError::Other("Artifact chunk size is invalid".into()));
        }
        let aad = chunk_aad(&header_hash, index, expected_plaintext_len as u64);
        let chunk = decrypt_artifact_chunk(
            &data_key,
            &nonce,
            &aad,
            &bytes[cursor..cursor + ciphertext_len],
        )?;
        cursor += ciphertext_len;
        plaintext.extend_from_slice(&chunk);
    }
    if cursor != bytes.len()
        || plaintext.len() as u64 != header.plaintext_size
        || sha256_hex(&plaintext) != header.plaintext_hash
    {
        return Err(AppError::Other("Artifact plaintext hash is invalid".into()));
    }
    let entries = parse_inner_payload(&header, &plaintext)?;
    Ok(VerifiedArtifact {
        manifest: expected.clone(),
        entries,
    })
}

/// Reconstructs the complete local manifest from the authenticated outer
/// header when the control plane only exposes opaque artifact metadata.
pub(crate) fn manifest_from_ciphertext(
    bytes: &[u8],
    artifact_id: &str,
    artifact_type: &str,
    ciphertext_hash: &str,
    size: u64,
    key_version: i64,
    created_at: String,
) -> AppResult<ArtifactManifest> {
    if size != bytes.len() as u64 || sha256_hex(bytes) != ciphertext_hash {
        return Err(AppError::Other(
            "Artifact ciphertext hash or size is invalid".into(),
        ));
    }
    let (header, _, _) = parse_header(bytes)?;
    if header.artifact_id != artifact_id
        || header.artifact_type != artifact_type
        || header.key_version != key_version
    {
        return Err(AppError::Other(
            "Artifact header does not match the remote object manifest".into(),
        ));
    }
    Ok(ArtifactManifest {
        id: header.artifact_id,
        artifact_type: header.artifact_type,
        source_version_id: header.source_version_id,
        build_fingerprint: header.build_fingerprint,
        format_version: header.format_version,
        plaintext_hash: header.plaintext_hash,
        ciphertext_hash: ciphertext_hash.into(),
        plaintext_size: header.plaintext_size,
        size,
        encryption_scheme: header.encryption_scheme,
        key_version: header.key_version,
        chunk_size: header.chunk_size,
        chunk_count: header.chunk_count,
        created_at,
    })
}

/// Installs a verified artifact into a versioned directory and atomically
/// updates `<root>/current.json`. Existing pointers remain untouched on error.
pub fn install_artifact(root: &Path, artifact: &VerifiedArtifact) -> AppResult<PathBuf> {
    fs::create_dir_all(root)?;
    let stage = root.join(format!(".install-{}", Uuid::new_v4()));
    let destination = root.join(&artifact.manifest.id);
    if destination.exists() {
        write_pointer(root, &artifact.manifest.id)?;
        return Ok(destination);
    }
    if let Err(error) = install_entries(&stage, &artifact.entries) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    if let Err(error) = fs::rename(&stage, &destination) {
        let _ = fs::remove_dir_all(&stage);
        return Err(error.into());
    }
    write_pointer(root, &artifact.manifest.id)?;
    Ok(destination)
}

fn build_inner_payload(
    artifact_id: &str,
    build_fingerprint: &str,
    entries: Vec<ArtifactEntry>,
) -> AppResult<Vec<u8>> {
    let mut names = BTreeSet::new();
    let mut compressed = Vec::with_capacity(entries.len());
    for entry in entries {
        validate_entry_name(&entry.name)?;
        if !names.insert(entry.name.clone()) {
            return Err(AppError::Other(
                "Artifact entry names must be unique".into(),
            ));
        }
        if entry.media_type.trim().is_empty() || entry.media_type.len() > 160 {
            return Err(AppError::Other(
                "Artifact entry media type is invalid".into(),
            ));
        }
        let bytes = zstd::stream::encode_all(entry.bytes.as_slice(), 3)?;
        compressed.push((entry, bytes));
    }
    let mut descriptors = Vec::with_capacity(compressed.len());
    for (entry, compressed_bytes) in &compressed {
        descriptors.push(ArtifactEntryDescriptor {
            name: entry.name.clone(),
            media_type: entry.media_type.clone(),
            compression: "zstd".into(),
            compressed_size: compressed_bytes.len() as u64,
            uncompressed_size: entry.bytes.len() as u64,
            content_hash: sha256_hex(&entry.bytes),
        });
    }
    let manifest = InnerManifest {
        format_version: ARTIFACT_FORMAT_VERSION,
        artifact_id: artifact_id.into(),
        build_fingerprint: build_fingerprint.into(),
        inner_format: ARTIFACT_INNER_FORMAT.into(),
        entries: descriptors,
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let stored_size = compressed.iter().try_fold(0_usize, |total, (_, bytes)| {
        total
            .checked_add(bytes.len())
            .ok_or_else(|| AppError::Other("Artifact size overflow".into()))
    })?;
    let mut payload =
        Vec::with_capacity(INNER_MAGIC.len() + 4 + manifest_bytes.len() + stored_size);
    payload.extend_from_slice(INNER_MAGIC);
    payload.extend_from_slice(&(manifest_bytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(&manifest_bytes);
    for (_, bytes) in compressed {
        payload.extend_from_slice(&bytes);
    }
    Ok(payload)
}

fn parse_header(bytes: &[u8]) -> AppResult<(OuterHeader, Vec<u8>, usize)> {
    if bytes.len() < OUTER_MAGIC.len() + 4 || &bytes[..OUTER_MAGIC.len()] != OUTER_MAGIC {
        return Err(AppError::Other("Artifact magic is invalid".into()));
    }
    let header_len = u32::from_be_bytes(
        bytes[OUTER_MAGIC.len()..OUTER_MAGIC.len() + 4]
            .try_into()
            .map_err(|_| AppError::Other("Artifact header length is invalid".into()))?,
    ) as usize;
    let header_start = OUTER_MAGIC.len() + 4;
    if header_len == 0 || header_len > MAX_HEADER_BYTES || header_start + header_len > bytes.len() {
        return Err(AppError::Other("Artifact header length is invalid".into()));
    }
    let header_bytes = bytes[header_start..header_start + header_len].to_vec();
    let header: OuterHeader = serde_json::from_slice(&header_bytes)
        .map_err(|_| AppError::Other("Artifact header JSON is invalid".into()))?;
    if header.chunk_size == 0
        || header.chunk_size as usize > MAX_CHUNK_SIZE
        || header.chunk_count == 0
        || header.chunk_count > MAX_CHUNK_COUNT
    {
        return Err(AppError::Other(
            "Artifact chunk configuration is invalid".into(),
        ));
    }
    Ok((header, header_bytes, header_start + header_len))
}

fn parse_inner_manifest(payload: &[u8]) -> AppResult<InnerManifest> {
    if payload.len() < INNER_MAGIC.len() + 4 || &payload[..INNER_MAGIC.len()] != INNER_MAGIC {
        return Err(AppError::Other("Artifact inner magic is invalid".into()));
    }
    let start = INNER_MAGIC.len();
    let len = u32::from_be_bytes(
        payload[start..start + 4]
            .try_into()
            .map_err(|_| AppError::Other("Artifact inner manifest length is invalid".into()))?,
    ) as usize;
    let begin = start + 4;
    if len == 0 || begin + len > payload.len() {
        return Err(AppError::Other(
            "Artifact inner manifest length is invalid".into(),
        ));
    }
    let manifest: InnerManifest = serde_json::from_slice(&payload[begin..begin + len])
        .map_err(|_| AppError::Other("Artifact inner manifest JSON is invalid".into()))?;
    if manifest.format_version != ARTIFACT_FORMAT_VERSION
        || manifest.inner_format != ARTIFACT_INNER_FORMAT
        || manifest.entries.len() > MAX_ENTRY_COUNT
    {
        return Err(AppError::Other("Artifact inner manifest is invalid".into()));
    }
    Ok(manifest)
}

fn parse_inner_payload(header: &OuterHeader, payload: &[u8]) -> AppResult<Vec<ArtifactEntry>> {
    let manifest = parse_inner_manifest(payload)?;
    if manifest.artifact_id != header.artifact_id
        || manifest.build_fingerprint != header.build_fingerprint
    {
        // The inner hash is verified by the outer header; this comparison also
        // prevents a stale inner manifest being paired with a valid ciphertext.
        return Err(AppError::Other(
            "Artifact inner manifest does not match header".into(),
        ));
    }
    let manifest_len = serde_json::to_vec(&manifest)?.len();
    let mut cursor = INNER_MAGIC.len() + 4 + manifest_len;
    let mut names = BTreeSet::new();
    let mut entries = Vec::with_capacity(manifest.entries.len());
    for descriptor in manifest.entries {
        validate_entry_name(&descriptor.name)?;
        if !names.insert(descriptor.name.clone())
            || descriptor.compression != "zstd"
            || descriptor.media_type.trim().is_empty()
            || descriptor.media_type.len() > 160
            || !is_sha256_hex(&descriptor.content_hash)
        {
            return Err(AppError::Other(
                "Artifact entry descriptor is invalid".into(),
            ));
        }
        let end = cursor
            .checked_add(descriptor.compressed_size as usize)
            .ok_or_else(|| AppError::Other("Artifact entry bounds overflow".into()))?;
        if end > payload.len() {
            return Err(AppError::Other("Artifact entry is truncated".into()));
        }
        let decoder = zstd::stream::read::Decoder::new(&payload[cursor..end])?;
        let mut bytes = Vec::new();
        decoder
            .take(descriptor.uncompressed_size.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != descriptor.uncompressed_size
            || sha256_hex(&bytes) != descriptor.content_hash
        {
            return Err(AppError::Other("Artifact entry hash is invalid".into()));
        }
        entries.push(ArtifactEntry {
            name: descriptor.name,
            media_type: descriptor.media_type,
            bytes,
        });
        cursor = end;
    }
    if cursor != payload.len() {
        return Err(AppError::Other(
            "Artifact contains trailing plaintext bytes".into(),
        ));
    }
    Ok(entries)
}

fn install_entries(root: &Path, entries: &[ArtifactEntry]) -> AppResult<()> {
    fs::create_dir_all(root)?;
    for entry in entries {
        validate_entry_name(&entry.name)?;
        let path = root.join(&entry.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        file.write_all(&entry.bytes)?;
        file.sync_all()?;
    }
    let dir = File::open(root)?;
    dir.sync_all()?;
    Ok(())
}

fn write_pointer(root: &Path, artifact_id: &str) -> AppResult<()> {
    let pointer = root.join("current.json");
    let temporary = root.join(format!(".current-{}.tmp", Uuid::new_v4()));
    let value = InstallPointer {
        artifact_id: artifact_id.into(),
        installed_at: now_string(),
    };
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(&value)?)?;
    file.sync_all()?;
    fs::rename(&temporary, pointer)?;
    Ok(())
}

fn chunk_aad(header_hash: &[u8], index: u64, plaintext_size: u64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(ARTIFACT_AAD.len() + 32 + 16);
    aad.extend_from_slice(ARTIFACT_AAD);
    aad.extend_from_slice(header_hash);
    aad.extend_from_slice(&index.to_be_bytes());
    aad.extend_from_slice(&plaintext_size.to_be_bytes());
    aad
}

fn validate_entry_name(name: &str) -> AppResult<()> {
    if name.is_empty() || name.len() > MAX_ENTRY_NAME_BYTES || !name.is_ascii() {
        return Err(AppError::Other("Artifact entry name is invalid".into()));
    }
    let path = Path::new(name);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(AppError::Other(
            "Artifact entry path escapes its root".into(),
        ));
    }
    Ok(())
}

fn validate_identity(value: &str, label: &str, max: usize) -> AppResult<()> {
    if value.trim().is_empty() || value.len() > max || !value.is_ascii() {
        return Err(AppError::Other(format!("Artifact {label} is invalid")));
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn div_ceil(value: u64, divisor: u64) -> u64 {
    value / divisor + u64::from(value % divisor != 0)
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn inputs() -> ArtifactBuildInputs {
        ArtifactBuildInputs {
            source_plaintext_hash: sha256_hex(b"source"),
            parser_profile_fingerprint: Some("parser-v1".into()),
            chunker_profile_fingerprint: Some("chunker-v1".into()),
            embedding_model: Some("embed-v1".into()),
            embedding_model_revision: Some("rev-1".into()),
            dims: Some(3),
            normalized: Some(true),
            embedding_instruction_hash: Some("instruction-hash".into()),
            tokenizer_ref: None,
            artifact_format_version: ARTIFACT_FORMAT_VERSION,
        }
    }

    fn entries() -> Vec<ArtifactEntry> {
        vec![
            ArtifactEntry {
                name: "chunks.jsonl".into(),
                media_type: "application/jsonl".into(),
                bytes: b"{\"id\":1}\n".repeat(100),
            },
            ArtifactEntry {
                name: "vectors.f32le".into(),
                media_type: "application/octet-stream".into(),
                bytes: (0..100)
                    .flat_map(|value| (value as f32).to_le_bytes())
                    .collect(),
            },
        ]
    }

    #[test]
    fn build_fingerprint_changes_when_any_input_changes() {
        let first = inputs().build_fingerprint().unwrap();
        let mut changed = inputs();
        changed.dims = Some(4);
        assert_ne!(first, changed.build_fingerprint().unwrap());
    }

    #[test]
    fn artifact_round_trip_verifies_manifest_entries_and_installs_atomically() {
        let key = SyncMasterKey::generate();
        let built = build_artifact(
            &key,
            1,
            "knowledge_vectors",
            "version-1",
            &inputs(),
            entries(),
        )
        .unwrap();
        assert_eq!(built.manifest.size, built.bytes.len() as u64);
        let verified = verify_artifact(&key, &built.manifest, &built.bytes).unwrap();
        assert_eq!(verified.entries.len(), 2);
        let root = tempdir().unwrap();
        let installed = install_artifact(root.path(), &verified).unwrap();
        assert_eq!(installed.join("chunks.jsonl").exists(), true);
        let pointer: InstallPointer =
            serde_json::from_slice(&fs::read(root.path().join("current.json")).unwrap()).unwrap();
        assert_eq!(pointer.artifact_id, built.manifest.id);
    }

    #[test]
    fn reconstructs_remote_manifest_from_the_authenticated_outer_header() {
        let key = SyncMasterKey::generate();
        let built = build_artifact(
            &key,
            1,
            "knowledge_vectors",
            "version-1",
            &inputs(),
            entries(),
        )
        .unwrap();
        let reconstructed = manifest_from_ciphertext(
            &built.bytes,
            &built.manifest.id,
            &built.manifest.artifact_type,
            &built.manifest.ciphertext_hash,
            built.manifest.size,
            built.manifest.key_version,
            "remote-updated-at".into(),
        )
        .unwrap();
        assert_eq!(
            reconstructed.build_fingerprint,
            built.manifest.build_fingerprint
        );
        assert_eq!(reconstructed.plaintext_hash, built.manifest.plaintext_hash);
        assert!(verify_artifact(&key, &reconstructed, &built.bytes).is_ok());

        let mut incompatible = reconstructed.clone();
        incompatible.build_fingerprint = "b".repeat(64);
        assert!(verify_artifact(&key, &incompatible, &built.bytes).is_err());
    }

    #[test]
    fn tampering_ciphertext_or_metadata_never_replaces_current_pointer() {
        let key = SyncMasterKey::generate();
        let built = build_artifact(
            &key,
            1,
            "knowledge_vectors",
            "version-1",
            &inputs(),
            entries(),
        )
        .unwrap();
        let root = tempdir().unwrap();
        let verified = verify_artifact(&key, &built.manifest, &built.bytes).unwrap();
        install_artifact(root.path(), &verified).unwrap();
        let before = fs::read(root.path().join("current.json")).unwrap();
        let mut tampered = built.bytes.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(verify_artifact(&key, &built.manifest, &tampered).is_err());
        assert_eq!(fs::read(root.path().join("current.json")).unwrap(), before);
    }

    #[test]
    fn rejects_traversal_and_duplicate_entries() {
        let key = SyncMasterKey::generate();
        let mut invalid = entries();
        invalid[0].name = "../escape".into();
        assert!(build_artifact(
            &key,
            1,
            "knowledge_vectors",
            "version-1",
            &inputs(),
            invalid
        )
        .is_err());
        let duplicate = vec![
            ArtifactEntry {
                name: "same".into(),
                media_type: "text/plain".into(),
                bytes: vec![1],
            },
            ArtifactEntry {
                name: "same".into(),
                media_type: "text/plain".into(),
                bytes: vec![2],
            },
        ];
        assert!(build_artifact(
            &key,
            1,
            "knowledge_vectors",
            "version-1",
            &inputs(),
            duplicate
        )
        .is_err());
    }
}
