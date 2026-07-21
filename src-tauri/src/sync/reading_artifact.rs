//! Encrypted portable EPUB payloads for Read With AI.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::artifact::{
    build_artifact, ArtifactBuildInputs, ArtifactEntry, BuiltArtifact, VerifiedArtifact,
    ARTIFACT_FORMAT_VERSION,
};
use super::crypto::SyncMasterKey;

pub const READING_EPUB_ARTIFACT_TYPE: &str = "reading_epub";
const PAYLOAD_FORMAT_VERSION: u16 = 1;
const MANIFEST_ENTRY: &str = "book.json";
const EPUB_ENTRY: &str = "book.epub";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingEpubPayloadManifest {
    pub format_version: u16,
    pub book_id: String,
    pub title: String,
    pub author: Option<String>,
    pub source_hash: String,
    pub source_size: u64,
}

pub fn build(
    master_key: &SyncMasterKey,
    key_version: i64,
    book_id: &str,
    title: &str,
    author: Option<&str>,
    source_hash: &str,
    epub: Vec<u8>,
) -> AppResult<BuiltArtifact> {
    let payload = ReadingEpubPayloadManifest {
        format_version: PAYLOAD_FORMAT_VERSION,
        book_id: book_id.into(),
        title: title.trim().into(),
        author: author
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        source_hash: source_hash.into(),
        source_size: epub.len() as u64,
    };
    validate_manifest(&payload)?;
    if sha256_hex(&epub) != source_hash {
        return Err(AppError::Other(
            "EPUB source hash does not match the book metadata".into(),
        ));
    }
    let manifest_bytes = serde_json::to_vec(&payload)?;
    build_artifact(
        master_key,
        key_version,
        READING_EPUB_ARTIFACT_TYPE,
        book_id,
        &ArtifactBuildInputs {
            source_plaintext_hash: source_hash.into(),
            parser_profile_fingerprint: Some("epub-2-3-v1".into()),
            chunker_profile_fingerprint: None,
            embedding_model: None,
            embedding_model_revision: None,
            dims: None,
            normalized: None,
            embedding_instruction_hash: None,
            tokenizer_ref: None,
            artifact_format_version: ARTIFACT_FORMAT_VERSION,
        },
        vec![
            ArtifactEntry {
                name: MANIFEST_ENTRY.into(),
                media_type: "application/json".into(),
                bytes: manifest_bytes,
            },
            ArtifactEntry {
                name: EPUB_ENTRY.into(),
                media_type: "application/epub+zip".into(),
                bytes: epub,
            },
        ],
    )
}

pub fn build_fingerprint(source_hash: &str) -> AppResult<String> {
    ArtifactBuildInputs {
        source_plaintext_hash: source_hash.into(),
        parser_profile_fingerprint: Some("epub-2-3-v1".into()),
        chunker_profile_fingerprint: None,
        embedding_model: None,
        embedding_model_revision: None,
        dims: None,
        normalized: None,
        embedding_instruction_hash: None,
        tokenizer_ref: None,
        artifact_format_version: ARTIFACT_FORMAT_VERSION,
    }
    .build_fingerprint()
}

pub fn decode(artifact: &VerifiedArtifact) -> AppResult<(ReadingEpubPayloadManifest, Vec<u8>)> {
    if artifact.manifest.artifact_type != READING_EPUB_ARTIFACT_TYPE || artifact.entries.len() != 2
    {
        return Err(AppError::Other("Artifact is not a reading EPUB".into()));
    }
    let entry = |name: &str| {
        artifact
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| AppError::Other(format!("Reading EPUB artifact is missing {name}")))
    };
    let payload: ReadingEpubPayloadManifest = serde_json::from_slice(&entry(MANIFEST_ENTRY)?.bytes)
        .map_err(|_| AppError::Other("Reading EPUB artifact manifest is invalid".into()))?;
    let epub = entry(EPUB_ENTRY)?.bytes.clone();
    validate_manifest(&payload)?;
    if artifact.manifest.source_version_id != payload.book_id
        || artifact.manifest.build_fingerprint != build_fingerprint(&payload.source_hash)?
        || epub.len() as u64 != payload.source_size
        || sha256_hex(&epub) != payload.source_hash
    {
        return Err(AppError::Other(
            "Reading EPUB artifact does not match its envelope".into(),
        ));
    }
    crate::reading::parse_epub_bytes(&epub)?;
    Ok((payload, epub))
}

pub fn cache_built_artifact(root: &Path, artifact: &BuiltArtifact) -> AppResult<PathBuf> {
    if !root.is_absolute() {
        return Err(AppError::Other(
            "Reading artifact cache root must be absolute".into(),
        ));
    }
    fs::create_dir_all(root)?;
    let destination = root.join(format!("{}.agnes-artifact", artifact.manifest.id));
    if destination.exists() {
        if fs::read(&destination)? == artifact.bytes {
            return Ok(destination);
        }
        return Err(AppError::Other(
            "Reading artifact cache contains different bytes".into(),
        ));
    }
    let temporary = root.join(format!(".artifact-{}.tmp", Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&artifact.bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)?;
        File::open(root)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(destination)
}

pub fn install_epub(
    data_dir: &Path,
    payload: &ReadingEpubPayloadManifest,
    epub: &[u8],
) -> AppResult<PathBuf> {
    if !data_dir.is_absolute() {
        return Err(AppError::Other(
            "Reading data directory must be absolute".into(),
        ));
    }
    validate_manifest(payload)?;
    if epub.len() as u64 != payload.source_size || sha256_hex(epub) != payload.source_hash {
        return Err(AppError::Other(
            "Reading EPUB installation bytes are invalid".into(),
        ));
    }
    crate::reading::parse_epub_bytes(epub)?;
    let root = data_dir.join("reading-books");
    fs::create_dir_all(&root)?;
    let target = root.join(format!("{}.epub", payload.book_id));
    let temporary = root.join(format!(".{}-{}.partial", payload.book_id, Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(epub)?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        File::open(&root)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(target)
}

fn validate_manifest(payload: &ReadingEpubPayloadManifest) -> AppResult<()> {
    if payload.format_version != PAYLOAD_FORMAT_VERSION
        || uuid::Uuid::parse_str(&payload.book_id).is_err()
        || payload.title.trim().is_empty()
        || payload.title.chars().count() > 1_024
        || !is_sha256(&payload.source_hash)
        || payload.source_size == 0
        || payload.source_size > crate::reading::MAX_EPUB_ARCHIVE_BYTES
        || payload
            .author
            .as_deref()
            .is_some_and(|value| value.chars().count() > 1_024)
    {
        return Err(AppError::Other(
            "Reading EPUB artifact manifest is invalid".into(),
        ));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn fixture_epub() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (path, content) in [
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#,
            ),
            (
                "OPS/book.opf",
                r#"<?xml version="1.0"?><package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Portable Book</dc:title><dc:creator>Reader</dc:creator></metadata><manifest><item id="one" href="chapter.xhtml"/></manifest><spine><itemref idref="one"/></spine></package>"#,
            ),
            (
                "OPS/chapter.xhtml",
                r#"<html><head><title>Chapter</title></head><body><p>Portable text.</p></body></html>"#,
            ),
        ] {
            archive.start_file(path, options).unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn valid_epub_round_trips_and_installs_to_the_reading_library() {
        let key = SyncMasterKey::generate();
        let book_id = Uuid::new_v4().to_string();
        let bytes = fixture_epub();
        let hash = sha256_hex(&bytes);
        let built = build(
            &key,
            1,
            &book_id,
            "Portable Book",
            Some("Reader"),
            &hash,
            bytes.clone(),
        )
        .unwrap();
        let verified =
            super::super::artifact::verify_artifact(&key, &built.manifest, &built.bytes).unwrap();
        let (payload, decoded) = decode(&verified).unwrap();
        assert_eq!(payload.book_id, book_id);
        assert_eq!(decoded, bytes);

        let root = tempdir().unwrap();
        let installed = install_epub(root.path(), &payload, &decoded).unwrap();
        assert_eq!(
            installed,
            root.path()
                .join("reading-books")
                .join(format!("{book_id}.epub"))
        );
        assert_eq!(fs::read(installed).unwrap(), bytes);
    }

    #[test]
    fn rejects_epub_without_a_valid_container() {
        let key = SyncMasterKey::generate();
        let bytes = b"not an epub".to_vec();
        let hash = sha256_hex(&bytes);
        let built = build(
            &key,
            1,
            &Uuid::new_v4().to_string(),
            "Book",
            None,
            &hash,
            bytes,
        )
        .unwrap();
        let verified =
            super::super::artifact::verify_artifact(&key, &built.manifest, &built.bytes).unwrap();
        assert!(decode(&verified).is_err());
        let root = tempdir().unwrap();
        assert!(cache_built_artifact(root.path(), &built).is_ok());
    }
}
