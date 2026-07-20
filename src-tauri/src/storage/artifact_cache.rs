use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tokio::sync::Mutex;

use crate::db::repo::artifacts::ArtifactGcCandidateRow;
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

pub const ARTIFACT_CACHE_QUOTA_SETTING: &str = "storage:artifact_cache_quota_bytes";
pub const DEFAULT_ARTIFACT_CACHE_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MIN_ARTIFACT_CACHE_QUOTA_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_ARTIFACT_CACHE_QUOTA_BYTES: u64 = 100 * 1024 * 1024 * 1024;
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const BACKGROUND_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

static ARTIFACT_GC_GATE: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStorageStatus {
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub outbox_bytes: u64,
    pub installed_bytes: u64,
    pub temporary_bytes: u64,
    pub reclaimable_bytes: u64,
    pub local_artifact_count: usize,
    pub over_quota: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactGcResult {
    pub reclaimed_bytes: u64,
    pub removed_paths: usize,
    pub reconciled_records: usize,
    pub failed_paths: usize,
    pub status: ArtifactStorageStatus,
}

#[derive(Debug)]
struct Inventory {
    used_bytes: u64,
    outbox_bytes: u64,
    installed_bytes: u64,
    temporary_bytes: u64,
    stale_temporary_paths: Vec<SizedPath>,
    current_pointer_artifact_id: Option<String>,
}

#[derive(Debug, Clone)]
struct SizedPath {
    path: PathBuf,
    bytes: u64,
}

#[derive(Debug)]
struct LocalCandidate {
    artifact_id: String,
    database_path: String,
    path: PathBuf,
    bytes: u64,
    created_at: i64,
    outbox: bool,
    missing: bool,
}

pub async fn status(db: &DbActorHandle, root: PathBuf) -> AppResult<ArtifactStorageStatus> {
    validate_root(&root)?;
    let quota = configured_quota(db).await?;
    let rows = db.list_artifact_gc_candidates().await?;
    let inventory = tokio::task::spawn_blocking({
        let root = root.clone();
        move || collect_inventory(&root, STALE_TEMP_AGE)
    })
    .await
    .map_err(|error| AppError::Other(format!("制品容量统计任务异常中止：{error}")))??;
    build_status(quota, &root, &rows, inventory)
}

pub async fn set_quota(
    db: &DbActorHandle,
    root: PathBuf,
    quota_bytes: u64,
) -> AppResult<ArtifactGcResult> {
    validate_quota(quota_bytes)?;
    db.set_setting(ARTIFACT_CACHE_QUOTA_SETTING.into(), quota_bytes.to_string())
        .await?;
    run_gc(db, root, false).await
}

pub async fn cleanup(db: &DbActorHandle, root: PathBuf) -> AppResult<ArtifactGcResult> {
    run_gc(db, root, true).await
}

pub async fn enforce_quota(db: &DbActorHandle, root: PathBuf) -> AppResult<ArtifactGcResult> {
    run_gc(db, root, false).await
}

pub fn start_background(db: DbActorHandle, root: PathBuf) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(15)).await;
        loop {
            if let Err(error) = enforce_quota(&db, root.clone()).await {
                eprintln!("[artifact-gc] background cleanup failed: {error}");
            }
            tokio::time::sleep(BACKGROUND_INTERVAL).await;
        }
    });
}

async fn run_gc(
    db: &DbActorHandle,
    root: PathBuf,
    remove_all_reclaimable: bool,
) -> AppResult<ArtifactGcResult> {
    validate_root(&root)?;
    let _guard = ARTIFACT_GC_GATE.get_or_init(|| Mutex::new(())).lock().await;
    let quota = configured_quota(db).await?;
    let rows = db.list_artifact_gc_candidates().await?;
    let (inventory, mut candidates) = tokio::task::spawn_blocking({
        let root = root.clone();
        let rows = rows.clone();
        move || {
            let inventory = collect_inventory(&root, STALE_TEMP_AGE)?;
            let candidates = collect_local_candidates(&root, &rows, &inventory)?;
            Ok::<_, AppError>((inventory, candidates))
        }
    })
    .await
    .map_err(|error| AppError::Other(format!("制品清理扫描任务异常中止：{error}")))??;

    candidates
        .sort_by_key(|candidate| (!candidate.missing, !candidate.outbox, candidate.created_at));
    let mut estimated_used = inventory.used_bytes;
    let mut reclaimed_bytes = 0_u64;
    let mut removed_paths = 0_usize;
    let mut reconciled_records = 0_usize;
    let mut failed_paths = 0_usize;

    for temporary in &inventory.stale_temporary_paths {
        if remove_path(&temporary.path).is_ok() {
            estimated_used = estimated_used.saturating_sub(temporary.bytes);
            reclaimed_bytes = reclaimed_bytes.saturating_add(temporary.bytes);
            removed_paths += 1;
        } else {
            failed_paths += 1;
        }
    }

    for candidate in candidates {
        if !remove_all_reclaimable && estimated_used <= quota {
            break;
        }
        let removed = if candidate.missing {
            true
        } else {
            match remove_path(&candidate.path) {
                Ok(()) => {
                    estimated_used = estimated_used.saturating_sub(candidate.bytes);
                    reclaimed_bytes = reclaimed_bytes.saturating_add(candidate.bytes);
                    removed_paths += 1;
                    true
                }
                Err(_) => {
                    failed_paths += 1;
                    false
                }
            }
        };
        if removed
            && db
                .clear_artifact_local_path(candidate.artifact_id, candidate.database_path)
                .await?
        {
            reconciled_records += 1;
        }
    }

    let status = status_without_gate(db, root, quota).await?;
    Ok(ArtifactGcResult {
        reclaimed_bytes,
        removed_paths,
        reconciled_records,
        failed_paths,
        status,
    })
}

async fn status_without_gate(
    db: &DbActorHandle,
    root: PathBuf,
    quota: u64,
) -> AppResult<ArtifactStorageStatus> {
    let rows = db.list_artifact_gc_candidates().await?;
    let inventory = tokio::task::spawn_blocking({
        let root = root.clone();
        move || collect_inventory(&root, STALE_TEMP_AGE)
    })
    .await
    .map_err(|error| AppError::Other(format!("制品容量统计任务异常中止：{error}")))??;
    build_status(quota, &root, &rows, inventory)
}

fn build_status(
    quota: u64,
    root: &Path,
    rows: &[ArtifactGcCandidateRow],
    inventory: Inventory,
) -> AppResult<ArtifactStorageStatus> {
    let candidates = collect_local_candidates(root, rows, &inventory)?;
    let reclaimable_paths = candidates
        .iter()
        .filter(|candidate| !candidate.missing)
        .map(|candidate| candidate.path.clone())
        .collect::<HashSet<_>>();
    let candidate_bytes = candidates
        .iter()
        .filter(|candidate| !candidate.missing)
        .map(|candidate| candidate.bytes)
        .sum::<u64>();
    let temporary_bytes = inventory
        .stale_temporary_paths
        .iter()
        .filter(|path| !reclaimable_paths.contains(&path.path))
        .map(|path| path.bytes)
        .sum::<u64>();
    Ok(ArtifactStorageStatus {
        quota_bytes: quota,
        used_bytes: inventory.used_bytes,
        outbox_bytes: inventory.outbox_bytes,
        installed_bytes: inventory.installed_bytes,
        temporary_bytes: inventory.temporary_bytes,
        reclaimable_bytes: candidate_bytes.saturating_add(temporary_bytes),
        local_artifact_count: rows
            .iter()
            .filter(|row| Path::new(&row.local_path).exists())
            .count(),
        over_quota: inventory.used_bytes > quota,
    })
}

async fn configured_quota(db: &DbActorHandle) -> AppResult<u64> {
    let Some(value) = db.get_setting(ARTIFACT_CACHE_QUOTA_SETTING.into()).await? else {
        return Ok(DEFAULT_ARTIFACT_CACHE_QUOTA_BYTES);
    };
    let quota = value
        .parse::<u64>()
        .map_err(|_| AppError::Other("本地制品配额配置无效".into()))?;
    validate_quota(quota)?;
    Ok(quota)
}

fn validate_quota(quota: u64) -> AppResult<()> {
    if !(MIN_ARTIFACT_CACHE_QUOTA_BYTES..=MAX_ARTIFACT_CACHE_QUOTA_BYTES).contains(&quota) {
        return Err(AppError::Other(format!(
            "本地制品配额必须在 {} MiB 到 {} GiB 之间",
            MIN_ARTIFACT_CACHE_QUOTA_BYTES / 1024 / 1024,
            MAX_ARTIFACT_CACHE_QUOTA_BYTES / 1024 / 1024 / 1024,
        )));
    }
    Ok(())
}

fn validate_root(root: &Path) -> AppResult<()> {
    if !root.is_absolute() {
        return Err(AppError::Other("制品存储目录必须是绝对路径".into()));
    }
    Ok(())
}

fn collect_inventory(root: &Path, stale_age: Duration) -> AppResult<Inventory> {
    if !root.exists() {
        return Ok(Inventory {
            used_bytes: 0,
            outbox_bytes: 0,
            installed_bytes: 0,
            temporary_bytes: 0,
            stale_temporary_paths: Vec::new(),
            current_pointer_artifact_id: None,
        });
    }
    let root = root.canonicalize()?;
    let outbox = root.join("outbox");
    let mut inventory = Inventory {
        used_bytes: 0,
        outbox_bytes: 0,
        installed_bytes: 0,
        temporary_bytes: 0,
        stale_temporary_paths: Vec::new(),
        current_pointer_artifact_id: read_current_pointer(&root),
    };
    let mut pending = vec![root.clone()];
    let now = SystemTime::now();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let temporary = is_temporary_path(&root, &outbox, &path);
            if metadata.is_dir() {
                if temporary {
                    let bytes = path_size(&path)?;
                    add_inventory_bytes(&mut inventory, &path, &outbox, bytes, true);
                    if is_stale(&metadata, now, stale_age) {
                        inventory
                            .stale_temporary_paths
                            .push(SizedPath { path, bytes });
                    }
                } else {
                    pending.push(path);
                }
                continue;
            }
            if metadata.is_file() {
                let bytes = metadata.len();
                add_inventory_bytes(&mut inventory, &path, &outbox, bytes, temporary);
                if temporary && is_stale(&metadata, now, stale_age) {
                    inventory
                        .stale_temporary_paths
                        .push(SizedPath { path, bytes });
                }
            }
        }
    }
    Ok(inventory)
}

fn add_inventory_bytes(
    inventory: &mut Inventory,
    path: &Path,
    outbox: &Path,
    bytes: u64,
    temporary: bool,
) {
    inventory.used_bytes = inventory.used_bytes.saturating_add(bytes);
    if temporary {
        inventory.temporary_bytes = inventory.temporary_bytes.saturating_add(bytes);
    } else if path.starts_with(outbox) {
        inventory.outbox_bytes = inventory.outbox_bytes.saturating_add(bytes);
    } else {
        inventory.installed_bytes = inventory.installed_bytes.saturating_add(bytes);
    }
}

fn collect_local_candidates(
    root: &Path,
    rows: &[ArtifactGcCandidateRow],
    inventory: &Inventory,
) -> AppResult<Vec<LocalCandidate>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let canonical_root = root.canonicalize()?;
    let canonical_outbox = canonical_root.join("outbox");
    let mut candidates = Vec::new();
    for row in rows {
        if !row.has_ready_replica {
            continue;
        }
        let path = PathBuf::from(&row.local_path);
        let outbox_name = format!("{}.agnes-artifact", row.artifact_id);
        let expected_outbox =
            path.file_name().and_then(|value| value.to_str()) == Some(&outbox_name);
        let expected_install =
            path.file_name().and_then(|value| value.to_str()) == Some(row.artifact_id.as_str());
        if !path.exists() {
            let lexically_safe = (expected_outbox
                && path.parent() == Some(root.join("outbox").as_path()))
                || (expected_install && path.parent() == Some(root));
            if lexically_safe {
                candidates.push(LocalCandidate {
                    artifact_id: row.artifact_id.clone(),
                    database_path: row.local_path.clone(),
                    path,
                    bytes: 0,
                    created_at: parse_created_at(&row.created_at),
                    outbox: expected_outbox,
                    missing: true,
                });
            }
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let canonical_path = path.canonicalize()?;
        let outbox = expected_outbox
            && metadata.is_file()
            && canonical_path.parent() == Some(canonical_outbox.as_path());
        let installed = expected_install
            && metadata.is_dir()
            && canonical_path.parent() == Some(canonical_root.as_path())
            && !row.current_source_version
            && inventory.current_pointer_artifact_id.as_deref() != Some(row.artifact_id.as_str());
        if !outbox && !installed {
            continue;
        }
        candidates.push(LocalCandidate {
            artifact_id: row.artifact_id.clone(),
            database_path: row.local_path.clone(),
            path: canonical_path.clone(),
            bytes: path_size(&canonical_path)?,
            created_at: parse_created_at(&row.created_at),
            outbox,
            missing: false,
        });
    }
    Ok(candidates)
}

fn path_size(path: &Path) -> AppResult<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0_u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to remove artifact symlink",
        )),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_temporary_path(root: &Path, outbox: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    (path.parent() == Some(root)
        && (name.starts_with(".install-")
            || (name.starts_with(".current-") && name.ends_with(".tmp"))))
        || (path.parent() == Some(outbox)
            && name.starts_with(".artifact-")
            && name.ends_with(".tmp"))
}

fn is_stale(metadata: &fs::Metadata, now: SystemTime, stale_age: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= stale_age)
}

fn read_current_pointer(root: &Path) -> Option<String> {
    let path = root.join("current.json");
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value.get("artifactId")?.as_str().map(str::to_owned)
}

fn parse_created_at(value: &str) -> i64 {
    value.parse::<i64>().unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::artifacts::{ArtifactReplicaRow, UpsertArtifactManifest};
    use crate::sync::artifact::ArtifactManifest;

    fn candidate(
        root: &Path,
        artifact_id: &str,
        outbox: bool,
        current_source_version: bool,
        has_ready_replica: bool,
    ) -> ArtifactGcCandidateRow {
        ArtifactGcCandidateRow {
            artifact_id: artifact_id.into(),
            local_path: if outbox {
                root.join("outbox")
                    .join(format!("{artifact_id}.agnes-artifact"))
            } else {
                root.join(artifact_id)
            }
            .to_string_lossy()
            .to_string(),
            created_at: "1".into(),
            has_ready_replica,
            current_source_version,
        }
    }

    #[test]
    fn gc_only_selects_recoverable_paths_inside_the_artifact_root() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        fs::create_dir_all(root_path.join("outbox")).unwrap();
        fs::write(
            root_path.join("outbox/ready.agnes-artifact"),
            vec![1_u8; 32],
        )
        .unwrap();
        fs::write(
            root_path.join("outbox/unique.agnes-artifact"),
            vec![1_u8; 16],
        )
        .unwrap();
        fs::create_dir_all(root_path.join("old")).unwrap();
        fs::write(root_path.join("old/vectors"), vec![1_u8; 64]).unwrap();
        fs::create_dir_all(root_path.join("current")).unwrap();
        fs::write(root_path.join("current/vectors"), vec![1_u8; 64]).unwrap();
        fs::create_dir_all(root_path.join("pointer")).unwrap();
        fs::write(root_path.join("pointer/vectors"), vec![1_u8; 64]).unwrap();
        fs::write(
            root_path.join("current.json"),
            br#"{"artifactId":"pointer","installedAt":"1"}"#,
        )
        .unwrap();
        let inventory = collect_inventory(&root_path, Duration::ZERO).unwrap();
        let rows = vec![
            candidate(&root_path, "ready", true, true, true),
            candidate(&root_path, "unique", true, false, false),
            candidate(&root_path, "old", false, false, true),
            candidate(&root_path, "current", false, true, true),
            candidate(&root_path, "pointer", false, false, true),
            ArtifactGcCandidateRow {
                local_path: root_path
                    .parent()
                    .unwrap()
                    .join("outside")
                    .to_string_lossy()
                    .to_string(),
                ..candidate(&root_path, "outside", false, false, true)
            },
        ];
        let selected = collect_local_candidates(&root_path, &rows, &inventory).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|candidate| candidate.artifact_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ready", "old"]
        );
    }

    #[test]
    fn inventory_separates_outbox_installed_and_temporary_bytes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("outbox")).unwrap();
        fs::write(root.path().join("outbox/a.agnes-artifact"), vec![0_u8; 10]).unwrap();
        fs::write(
            root.path().join("outbox/.artifact-stale.tmp"),
            vec![0_u8; 5],
        )
        .unwrap();
        fs::create_dir_all(root.path().join("installed")).unwrap();
        fs::write(root.path().join("installed/vectors"), vec![0_u8; 20]).unwrap();
        fs::write(
            root.path().join("installed/.artifact-entry.tmp"),
            vec![0_u8; 7],
        )
        .unwrap();
        let inventory = collect_inventory(root.path(), Duration::ZERO).unwrap();
        assert_eq!(inventory.used_bytes, 42);
        assert_eq!(inventory.outbox_bytes, 10);
        assert_eq!(inventory.installed_bytes, 27);
        assert_eq!(inventory.temporary_bytes, 5);
        assert_eq!(inventory.stale_temporary_paths.len(), 1);
    }

    #[test]
    fn quota_limits_reject_unbounded_or_tiny_values() {
        assert!(validate_quota(MIN_ARTIFACT_CACHE_QUOTA_BYTES).is_ok());
        assert!(validate_quota(MAX_ARTIFACT_CACHE_QUOTA_BYTES).is_ok());
        assert!(validate_quota(MIN_ARTIFACT_CACHE_QUOTA_BYTES - 1).is_err());
        assert!(validate_quota(MAX_ARTIFACT_CACHE_QUOTA_BYTES + 1).is_err());
    }

    #[tokio::test]
    async fn manual_cleanup_removes_ready_outbox_but_preserves_the_only_copy() {
        let database_path =
            std::env::temp_dir().join(format!("agnes-artifact-gc-{}.db", uuid::Uuid::new_v4()));
        let db = crate::db::spawn_db_actor(database_path.clone());
        db.get_sync_status().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let artifact_root = root.path().join("artifacts");
        let outbox = artifact_root.join("outbox");
        fs::create_dir_all(&outbox).unwrap();
        let ready_path = outbox.join("ready.agnes-artifact");
        let unique_path = outbox.join("unique.agnes-artifact");
        fs::write(&ready_path, vec![1_u8; 32]).unwrap();
        fs::write(&unique_path, vec![2_u8; 16]).unwrap();

        for (id, path, fingerprint) in [
            ("ready", &ready_path, "a".repeat(64)),
            ("unique", &unique_path, "d".repeat(64)),
        ] {
            db.upsert_artifact_manifest(UpsertArtifactManifest {
                manifest: ArtifactManifest {
                    id: id.into(),
                    artifact_type: "knowledge_vectors".into(),
                    source_version_id: format!("version-{id}"),
                    build_fingerprint: fingerprint,
                    format_version: 1,
                    plaintext_hash: "b".repeat(64),
                    ciphertext_hash: "c".repeat(64),
                    plaintext_size: 10,
                    size: if id == "ready" { 32 } else { 16 },
                    encryption_scheme: "xchacha20poly1305-chunked-v1".into(),
                    key_version: 1,
                    chunk_size: 1024,
                    chunk_count: 1,
                    created_at: "1".into(),
                },
                local_path: Some(path.to_string_lossy().to_string()),
                local_status: "built".into(),
                installed_at: None,
            })
            .await
            .unwrap();
        }
        db.upsert_artifact_replica(ArtifactReplicaRow {
            artifact_id: "ready".into(),
            provider_account_id: "r2-managed".into(),
            provider_kind: "r2".into(),
            encrypted_locator: "{}".into(),
            provider_revision: None,
            etag: None,
            ciphertext_hash: "c".repeat(64),
            size: 32,
            status: "ready".into(),
            last_error_code: None,
            updated_at: "2".into(),
        })
        .await
        .unwrap();

        let result = cleanup(&db, artifact_root).await.unwrap();
        assert_eq!(result.reclaimed_bytes, 32);
        assert!(!ready_path.exists());
        assert!(unique_path.exists());
        assert!(db
            .get_artifact_manifest("ready".into())
            .await
            .unwrap()
            .unwrap()
            .local_path
            .is_none());
        assert!(db
            .get_artifact_manifest("unique".into())
            .await
            .unwrap()
            .unwrap()
            .local_path
            .is_some());
        drop(db);
        let _ = fs::remove_file(database_path);
    }
}
