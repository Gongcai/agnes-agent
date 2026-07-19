use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::sync::artifact::ArtifactManifest;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactManifestRow {
    pub id: String,
    pub artifact_type: String,
    pub source_version_id: String,
    pub build_fingerprint: String,
    pub format_version: i64,
    pub plaintext_hash: String,
    pub ciphertext_hash: String,
    pub plaintext_size: i64,
    pub size: i64,
    pub encryption_scheme: String,
    pub key_version: i64,
    pub chunk_size: i64,
    pub chunk_count: i64,
    pub local_path: Option<String>,
    pub local_status: String,
    pub created_at: String,
    pub installed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpsertArtifactManifest {
    pub manifest: ArtifactManifest,
    pub local_path: Option<String>,
    pub local_status: String,
    pub installed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactReplicaRow {
    pub artifact_id: String,
    pub provider_account_id: String,
    pub provider_kind: String,
    pub encrypted_locator: String,
    pub provider_revision: Option<String>,
    pub etag: Option<String>,
    pub ciphertext_hash: String,
    pub size: i64,
    pub status: String,
    pub last_error_code: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceArtifactStateRow {
    pub device_id: String,
    pub artifact_id: String,
    pub observed_version: i64,
    pub local_status: String,
    pub verified_hash: Option<String>,
    pub last_checked_at: String,
    pub last_error_code: Option<String>,
}

const MANIFEST_SELECT: &str = "SELECT id,artifact_type,source_version_id,build_fingerprint,\
  format_version,plaintext_hash,ciphertext_hash,plaintext_size,size,encryption_scheme,\
  key_version,chunk_size,chunk_count,local_path,local_status,created_at,installed_at \
  FROM artifact_manifests";

pub fn get_manifest(
    conn: &Connection,
    artifact_id: &str,
) -> AppResult<Option<ArtifactManifestRow>> {
    Ok(conn
        .query_row(
            &format!("{MANIFEST_SELECT} WHERE id=?1"),
            [artifact_id],
            manifest_from_row,
        )
        .optional()?)
}

pub fn find_manifest_by_fingerprint(
    conn: &Connection,
    artifact_type: &str,
    source_version_id: &str,
    build_fingerprint: &str,
) -> AppResult<Option<ArtifactManifestRow>> {
    Ok(conn
        .query_row(
            &format!(
                "{MANIFEST_SELECT} WHERE artifact_type=?1 AND source_version_id=?2 \
                 AND build_fingerprint=?3"
            ),
            params![artifact_type, source_version_id, build_fingerprint],
            manifest_from_row,
        )
        .optional()?)
}

pub fn upsert_manifest(conn: &Connection, input: &UpsertArtifactManifest) -> AppResult<()> {
    validate_manifest(input)?;
    if let Some(existing) = get_manifest(conn, &input.manifest.id)? {
        if !same_immutable_manifest(&existing, &input.manifest)? {
            return Err(AppError::Other(
                "Artifact IDs cannot be reused for different immutable content".into(),
            ));
        }
        conn.execute(
            "UPDATE artifact_manifests SET
               local_path=COALESCE(?1,local_path),
               local_status=CASE
                 WHEN local_status='installed' AND ?2 IN ('built','available') THEN local_status
                 ELSE ?2
               END,
               installed_at=COALESCE(?3,installed_at)
             WHERE id=?4",
            params![
                input.local_path,
                input.local_status,
                input.installed_at,
                input.manifest.id
            ],
        )?;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO artifact_manifests \
         (id,artifact_type,source_version_id,build_fingerprint,format_version,plaintext_hash,\
          ciphertext_hash,plaintext_size,size,encryption_scheme,key_version,chunk_size,chunk_count,\
          local_path,local_status,created_at,installed_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        params![
            input.manifest.id,
            input.manifest.artifact_type,
            input.manifest.source_version_id,
            input.manifest.build_fingerprint,
            i64::from(input.manifest.format_version),
            input.manifest.plaintext_hash,
            input.manifest.ciphertext_hash,
            to_i64(input.manifest.plaintext_size)?,
            to_i64(input.manifest.size)?,
            input.manifest.encryption_scheme,
            input.manifest.key_version,
            i64::from(input.manifest.chunk_size),
            to_i64(input.manifest.chunk_count)?,
            input.local_path,
            input.local_status,
            input.manifest.created_at,
            input.installed_at,
        ],
    )?;
    Ok(())
}

pub fn upsert_replica(conn: &Connection, input: &ArtifactReplicaRow) -> AppResult<()> {
    validate_replica(input)?;
    conn.execute(
        "INSERT INTO artifact_replicas \
         (artifact_id,provider_account_id,provider_kind,encrypted_locator,provider_revision,etag,\
          ciphertext_hash,size,status,last_error_code,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) \
         ON CONFLICT(artifact_id,provider_account_id) DO UPDATE SET \
           provider_kind=excluded.provider_kind,encrypted_locator=excluded.encrypted_locator,\
           provider_revision=excluded.provider_revision,etag=excluded.etag,\
           ciphertext_hash=excluded.ciphertext_hash,size=excluded.size,status=excluded.status,\
           last_error_code=excluded.last_error_code,updated_at=excluded.updated_at",
        params![
            input.artifact_id,
            input.provider_account_id,
            input.provider_kind,
            input.encrypted_locator,
            input.provider_revision,
            input.etag,
            input.ciphertext_hash,
            input.size,
            input.status,
            input.last_error_code,
            input.updated_at,
        ],
    )?;
    Ok(())
}

pub fn list_replicas(conn: &Connection, artifact_id: &str) -> AppResult<Vec<ArtifactReplicaRow>> {
    let mut statement = conn.prepare(
        "SELECT artifact_id,provider_account_id,provider_kind,encrypted_locator,provider_revision,\
         etag,ciphertext_hash,size,status,last_error_code,updated_at FROM artifact_replicas \
         WHERE artifact_id=?1 ORDER BY status='ready' DESC,updated_at DESC,provider_account_id",
    )?;
    let rows = statement.query_map([artifact_id], replica_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn upsert_device_state(conn: &Connection, input: &DeviceArtifactStateRow) -> AppResult<()> {
    validate_device_state(input)?;
    conn.execute(
        "INSERT INTO device_artifact_states \
         (device_id,artifact_id,observed_version,local_status,verified_hash,last_checked_at,last_error_code) \
         VALUES (?1,?2,?3,?4,?5,?6,?7) \
         ON CONFLICT(device_id,artifact_id) DO UPDATE SET \
           observed_version=MAX(device_artifact_states.observed_version,excluded.observed_version),\
           local_status=excluded.local_status,verified_hash=excluded.verified_hash,\
           last_checked_at=excluded.last_checked_at,last_error_code=excluded.last_error_code\
         WHERE device_artifact_states.observed_version <= excluded.observed_version",
        params![
            input.device_id,
            input.artifact_id,
            input.observed_version,
            input.local_status,
            input.verified_hash,
            input.last_checked_at,
            input.last_error_code,
        ],
    )?;
    Ok(())
}

fn manifest_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactManifestRow> {
    Ok(ArtifactManifestRow {
        id: row.get(0)?,
        artifact_type: row.get(1)?,
        source_version_id: row.get(2)?,
        build_fingerprint: row.get(3)?,
        format_version: row.get(4)?,
        plaintext_hash: row.get(5)?,
        ciphertext_hash: row.get(6)?,
        plaintext_size: row.get(7)?,
        size: row.get(8)?,
        encryption_scheme: row.get(9)?,
        key_version: row.get(10)?,
        chunk_size: row.get(11)?,
        chunk_count: row.get(12)?,
        local_path: row.get(13)?,
        local_status: row.get(14)?,
        created_at: row.get(15)?,
        installed_at: row.get(16)?,
    })
}

fn replica_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactReplicaRow> {
    Ok(ArtifactReplicaRow {
        artifact_id: row.get(0)?,
        provider_account_id: row.get(1)?,
        provider_kind: row.get(2)?,
        encrypted_locator: row.get(3)?,
        provider_revision: row.get(4)?,
        etag: row.get(5)?,
        ciphertext_hash: row.get(6)?,
        size: row.get(7)?,
        status: row.get(8)?,
        last_error_code: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn validate_manifest(input: &UpsertArtifactManifest) -> AppResult<()> {
    let manifest = &input.manifest;
    if manifest.id.is_empty()
        || manifest.artifact_type.is_empty()
        || manifest.source_version_id.is_empty()
        || !is_hash(&manifest.build_fingerprint)
        || !is_hash(&manifest.plaintext_hash)
        || !is_hash(&manifest.ciphertext_hash)
        || manifest.size == 0
        || manifest.key_version <= 0
        || manifest.chunk_size == 0
        || manifest.chunk_count == 0
        || !matches!(
            input.local_status.as_str(),
            "built" | "available" | "installed" | "invalid" | "garbage"
        )
    {
        return Err(AppError::Other("Artifact manifest is invalid".into()));
    }
    if input
        .local_path
        .as_ref()
        .is_some_and(|path| path.len() > 4096)
        || input
            .installed_at
            .as_ref()
            .is_some_and(|value| value.len() > 80)
    {
        return Err(AppError::Other("Artifact local metadata is invalid".into()));
    }
    Ok(())
}

fn validate_replica(input: &ArtifactReplicaRow) -> AppResult<()> {
    if input.artifact_id.is_empty()
        || input.provider_account_id.is_empty()
        || input.provider_kind.is_empty()
        || input.encrypted_locator.is_empty()
        || !is_hash(&input.ciphertext_hash)
        || input.size <= 0
        || !matches!(
            input.status.as_str(),
            "uploading" | "ready" | "failed" | "deleted"
        )
    {
        return Err(AppError::Other("Artifact replica is invalid".into()));
    }
    Ok(())
}

fn validate_device_state(input: &DeviceArtifactStateRow) -> AppResult<()> {
    if input.device_id.is_empty()
        || input.artifact_id.is_empty()
        || input.observed_version < 0
        || input
            .verified_hash
            .as_ref()
            .is_some_and(|value| !is_hash(value))
        || !matches!(
            input.local_status.as_str(),
            "missing" | "downloading" | "verifying" | "installed" | "failed" | "incompatible"
        )
    {
        return Err(AppError::Other("Device artifact state is invalid".into()));
    }
    Ok(())
}

fn same_immutable_manifest(
    existing: &ArtifactManifestRow,
    manifest: &ArtifactManifest,
) -> AppResult<bool> {
    Ok(existing.artifact_type == manifest.artifact_type
        && existing.source_version_id == manifest.source_version_id
        && existing.build_fingerprint == manifest.build_fingerprint
        && existing.format_version == i64::from(manifest.format_version)
        && existing.plaintext_hash == manifest.plaintext_hash
        && existing.ciphertext_hash == manifest.ciphertext_hash
        && existing.plaintext_size == to_i64(manifest.plaintext_size)?
        && existing.size == to_i64(manifest.size)?
        && existing.encryption_scheme == manifest.encryption_scheme
        && existing.key_version == manifest.key_version
        && existing.chunk_size == i64::from(manifest.chunk_size)
        && existing.chunk_count == to_i64(manifest.chunk_count)?)
}

fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn to_i64(value: u64) -> AppResult<i64> {
    i64::try_from(value).map_err(|_| AppError::Other("Artifact size exceeds SQLite limits".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str) -> ArtifactManifest {
        ArtifactManifest {
            id: id.into(),
            artifact_type: "knowledge_vectors".into(),
            source_version_id: "version-1".into(),
            build_fingerprint: "a".repeat(64),
            format_version: 1,
            plaintext_hash: "b".repeat(64),
            ciphertext_hash: "c".repeat(64),
            plaintext_size: 10,
            size: 20,
            encryption_scheme: "xchacha20poly1305-chunked-v1".into(),
            key_version: 1,
            chunk_size: 1024,
            chunk_count: 1,
            created_at: "1".into(),
        }
    }

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(crate::db::schema::ARTIFACT_SCHEMA)
            .unwrap();
        connection
    }

    #[test]
    fn immutable_manifests_are_idempotent_but_cannot_be_reused() {
        let connection = connection();
        let input = UpsertArtifactManifest {
            manifest: manifest("artifact-1"),
            local_path: Some("/tmp/artifact-1".into()),
            local_status: "built".into(),
            installed_at: None,
        };
        upsert_manifest(&connection, &input).unwrap();
        let mut installed = input.clone();
        installed.local_status = "installed".into();
        installed.installed_at = Some("2".into());
        upsert_manifest(&connection, &installed).unwrap();
        assert_eq!(
            get_manifest(&connection, "artifact-1")
                .unwrap()
                .unwrap()
                .local_status,
            "installed"
        );
        upsert_manifest(
            &connection,
            &UpsertArtifactManifest {
                manifest: input.manifest.clone(),
                local_path: None,
                local_status: "available".into(),
                installed_at: None,
            },
        )
        .unwrap();
        let preserved = get_manifest(&connection, "artifact-1").unwrap().unwrap();
        assert_eq!(preserved.local_status, "installed");
        assert_eq!(preserved.local_path.as_deref(), Some("/tmp/artifact-1"));
        assert_eq!(preserved.installed_at.as_deref(), Some("2"));

        let mut changed = installed;
        changed.manifest.ciphertext_hash = "d".repeat(64);
        assert!(upsert_manifest(&connection, &changed).is_err());
    }

    #[test]
    fn replicas_and_device_state_are_upserted_without_regressing_observed_version() {
        let connection = connection();
        upsert_manifest(
            &connection,
            &UpsertArtifactManifest {
                manifest: manifest("artifact-1"),
                local_path: None,
                local_status: "available".into(),
                installed_at: None,
            },
        )
        .unwrap();
        upsert_replica(
            &connection,
            &ArtifactReplicaRow {
                artifact_id: "artifact-1".into(),
                provider_account_id: "drive-1".into(),
                provider_kind: "google_drive".into(),
                encrypted_locator: "opaque".into(),
                provider_revision: Some("1".into()),
                etag: None,
                ciphertext_hash: "c".repeat(64),
                size: 20,
                status: "ready".into(),
                last_error_code: None,
                updated_at: "2".into(),
            },
        )
        .unwrap();
        assert_eq!(list_replicas(&connection, "artifact-1").unwrap().len(), 1);

        let mut state = DeviceArtifactStateRow {
            device_id: "device-1".into(),
            artifact_id: "artifact-1".into(),
            observed_version: 3,
            local_status: "installed".into(),
            verified_hash: Some("c".repeat(64)),
            last_checked_at: "3".into(),
            last_error_code: None,
        };
        upsert_device_state(&connection, &state).unwrap();
        state.observed_version = 2;
        state.local_status = "failed".into();
        upsert_device_state(&connection, &state).unwrap();
        let state: (i64, String) = connection
            .query_row(
                "SELECT observed_version,local_status FROM device_artifact_states WHERE device_id='device-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (3, "installed".into()));
    }
}
