use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use crate::db::repo::backup::{BackupEntryRow, BackupSourceRow, NewBackupSource};
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

use super::StorageService;

const WATCH_DEBOUNCE: Duration = Duration::from_secs(2);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_LAZY_INTERVAL: i64 = 30 * 60;
const DEFAULT_MAX_DELAY: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupSourceView {
    #[serde(flatten)]
    pub source: BackupSourceRow,
    pub entry_count: usize,
    pub pending_count: usize,
}

#[derive(Debug, Clone)]
struct BackupEvent {
    source_id: String,
    paths: Vec<PathBuf>,
}

pub struct LocalBackupService {
    db: DbActorHandle,
    storage: Arc<StorageService>,
    events: mpsc::UnboundedSender<BackupEvent>,
    event_rx: Mutex<Option<mpsc::UnboundedReceiver<BackupEvent>>>,
    watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
    active: Arc<Mutex<HashSet<(String, String)>>>,
}

impl LocalBackupService {
    pub fn new(db: DbActorHandle, storage: Arc<StorageService>) -> Arc<Self> {
        let (events, event_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            db,
            storage,
            events,
            event_rx: Mutex::new(Some(event_rx)),
            watchers: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    pub async fn list_sources(&self) -> AppResult<Vec<BackupSourceView>> {
        let sources = self.db.list_backup_sources().await?;
        let mut result = Vec::with_capacity(sources.len());
        for source in sources {
            let entries = self.db.list_backup_entries(source.id.clone()).await?;
            let pending_count = entries
                .iter()
                .filter(|entry| {
                    matches!(
                        entry.status.as_str(),
                        "pending" | "queued" | "uploading" | "failed"
                    )
                })
                .count();
            result.push(BackupSourceView {
                source,
                entry_count: entries.len(),
                pending_count,
            });
        }
        Ok(result)
    }

    pub async fn add_source(
        &self,
        local_root: String,
        account_id: String,
        remote_root_id: Option<String>,
        mode: String,
        lazy_interval_seconds: Option<i64>,
        max_delay_seconds: Option<i64>,
    ) -> AppResult<String> {
        let root = canonical_directory(&local_root).await?;
        self.db
            .get_storage_account(account_id.clone())
            .await?
            .ok_or_else(|| AppError::Other("Backup destination account was not found".into()))?;
        if !matches!(mode.as_str(), "fast" | "lazy") {
            return Err(AppError::Other("Backup mode must be fast or lazy".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.db
            .upsert_backup_source(NewBackupSource {
                id: id.clone(),
                local_root: root.to_string_lossy().to_string(),
                account_id,
                remote_root_id,
                mode,
                lazy_interval_seconds: lazy_interval_seconds
                    .unwrap_or(DEFAULT_LAZY_INTERVAL)
                    .clamp(60, 7 * 24 * 60 * 60),
                max_delay_seconds: max_delay_seconds
                    .unwrap_or(DEFAULT_MAX_DELAY)
                    .clamp(60, 30 * 24 * 60 * 60),
            })
            .await?;
        self.ensure_watcher(&id, &root)?;
        self.reconcile_source(id.clone(), false).await?;
        Ok(id)
    }

    pub async fn remove_source(&self, source_id: String) -> AppResult<()> {
        self.watchers.lock().map_err(lock_error)?.remove(&source_id);
        self.db.delete_backup_source(source_id).await
    }

    pub async fn set_enabled(&self, source_id: String, enabled: bool) -> AppResult<()> {
        self.db
            .update_backup_source_enabled(source_id.clone(), enabled)
            .await?;
        if enabled {
            if let Some(source) = self
                .db
                .list_backup_sources()
                .await?
                .into_iter()
                .find(|item| item.id == source_id)
            {
                self.ensure_watcher(&source.id, Path::new(&source.local_root))?;
            }
        }
        Ok(())
    }

    pub async fn sync_now(&self, source_id: String) -> AppResult<()> {
        self.reconcile_source(source_id, true).await
    }

    pub fn start_background(self: Arc<Self>) {
        let service = Arc::clone(&self);
        tauri::async_runtime::spawn(async move {
            if let Err(error) = service.initialize_watchers().await {
                eprintln!("[backup] watcher initialization failed: {error}");
            }
            let mut event_rx = service
                .event_rx
                .lock()
                .ok()
                .and_then(|mut receiver| receiver.take());
            let mut ticker = tokio::time::interval(RECONCILE_INTERVAL);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(error) = service.reconcile_all(false).await {
                            eprintln!("[backup] reconciliation failed: {error}");
                        }
                    }
                    event = async {
                        match event_rx.as_mut() {
                            Some(receiver) => receiver.recv().await,
                            None => None,
                        }
                    } => {
                        if let Some(event) = event {
                            if let Err(error) = service.handle_event(event).await {
                                eprintln!("[backup] event handling failed: {error}");
                            }
                        } else {
                            event_rx = None;
                        }
                    }
                }
            }
        });
    }

    async fn initialize_watchers(&self) -> AppResult<()> {
        for source in self
            .db
            .list_backup_sources()
            .await?
            .into_iter()
            .filter(|source| source.enabled)
        {
            self.ensure_watcher(&source.id, Path::new(&source.local_root))?;
            let _ = self.reconcile_source(source.id, false).await;
        }
        Ok(())
    }

    fn ensure_watcher(&self, source_id: &str, root: &Path) -> AppResult<()> {
        if !root.is_dir() {
            return Err(AppError::Other(format!(
                "Backup folder does not exist: {}",
                root.display()
            )));
        }
        let mut watchers = self.watchers.lock().map_err(lock_error)?;
        if watchers.contains_key(source_id) {
            return Ok(());
        }
        let source_id = source_id.to_owned();
        let callback_source_id = source_id.clone();
        let events = self.events.clone();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else { return };
            if matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                let _ = events.send(BackupEvent {
                    source_id: callback_source_id.clone(),
                    paths: event.paths,
                });
            }
        })
        .map_err(|error| AppError::Other(format!("Unable to watch backup folder: {error}")))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| AppError::Other(format!("Unable to watch backup folder: {error}")))?;
        watchers.insert(source_id, watcher);
        Ok(())
    }

    async fn handle_event(&self, event: BackupEvent) -> AppResult<()> {
        let Some(source) = self
            .db
            .list_backup_sources()
            .await?
            .into_iter()
            .find(|source| source.id == event.source_id && source.enabled)
        else {
            return Ok(());
        };
        if source.mode == "fast" {
            tokio::time::sleep(WATCH_DEBOUNCE).await;
            self.reconcile_paths(&source, &event.paths, false, false)
                .await
        } else {
            self.reconcile_source(source.id, false).await
        }
    }

    async fn reconcile_all(&self, manual: bool) -> AppResult<()> {
        for source in self
            .db
            .list_backup_sources()
            .await?
            .into_iter()
            .filter(|source| source.enabled)
        {
            self.ensure_watcher(&source.id, Path::new(&source.local_root))?;
            self.reconcile_source(source.id, manual).await?;
        }
        Ok(())
    }

    async fn reconcile_source(&self, source_id: String, manual: bool) -> AppResult<()> {
        let Some(source) = self
            .db
            .list_backup_sources()
            .await?
            .into_iter()
            .find(|source| source.id == source_id)
        else {
            return Ok(());
        };
        if !source.enabled {
            return Ok(());
        }
        let files = scan_files(PathBuf::from(&source.local_root)).await?;
        let paths = files
            .into_iter()
            .map(|(path, _, _)| path)
            .collect::<Vec<_>>();
        self.reconcile_paths(&source, &paths, manual, true).await?;
        self.db.mark_backup_source_reconciled(source.id).await
    }

    async fn reconcile_paths(
        &self,
        source: &BackupSourceRow,
        paths: &[PathBuf],
        manual: bool,
        mark_missing: bool,
    ) -> AppResult<()> {
        let existing = self.db.list_backup_entries(source.id.clone()).await?;
        let existing_by_path = existing
            .into_iter()
            .map(|entry| (entry.relative_path.clone(), entry))
            .collect::<HashMap<_, _>>();
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();
        for path in paths {
            if should_ignore(path) || !path.is_file() {
                continue;
            }
            let relative = relative_path(Path::new(&source.local_root), path)?;
            seen.insert(relative.clone());
            let metadata = tokio::fs::metadata(path).await?;
            let size = i64::try_from(metadata.len())
                .map_err(|_| AppError::Other("Backup file is too large".into()))?;
            let mtime = metadata
                .modified()
                .ok()
                .and_then(unix_nanos)
                .and_then(|value| i64::try_from(value).ok());
            let old = existing_by_path.get(&relative);
            let changed = old.is_none()
                || old.is_some_and(|entry| {
                    entry.local_size != Some(size)
                        || entry.local_mtime_ns != mtime
                        || entry.status == "failed"
                });
            let dirty_at = old
                .and_then(|entry| entry.dirty_at.clone())
                .or_else(|| changed.then(now));
            let status = if changed {
                if source.mode == "fast" {
                    "queued"
                } else {
                    "pending"
                }
            } else {
                old.map(|entry| entry.status.as_str()).unwrap_or("pending")
            };
            let entry = BackupEntryRow {
                source_id: source.id.clone(),
                relative_path: relative.clone(),
                local_size: Some(size),
                local_mtime_ns: mtime,
                content_hash: old.and_then(|entry| entry.content_hash.clone()),
                remote_item_id: old.and_then(|entry| entry.remote_item_id.clone()),
                status: status.into(),
                last_error: None,
                dirty_at,
                uploaded_at: old.and_then(|entry| entry.uploaded_at.clone()),
                updated_at: now(),
            };
            self.db.upsert_backup_entry(entry.clone()).await?;
            let due = manual
                || (source.mode == "fast" && changed)
                || (source.mode == "lazy" && due_for_lazy(&entry, source));
            if due {
                candidates.push((relative, path.clone()));
            }
        }
        for entry in existing_by_path
            .values()
            .filter(|entry| mark_missing && !seen.contains(&entry.relative_path))
        {
            if entry.status != "deleted_local" {
                self.db
                    .upsert_backup_entry(BackupEntryRow {
                        status: "deleted_local".into(),
                        last_error: None,
                        updated_at: now(),
                        ..entry.clone()
                    })
                    .await?;
            }
        }
        for (relative, path) in candidates {
            self.upload_entry(source, relative, path).await?;
        }
        Ok(())
    }

    async fn upload_entry(
        &self,
        source: &BackupSourceRow,
        relative: String,
        path: PathBuf,
    ) -> AppResult<()> {
        let key = (source.id.clone(), relative.clone());
        {
            let mut active = self.active.lock().map_err(lock_error)?;
            if !active.insert(key.clone()) {
                return Ok(());
            }
        }
        let result = self.upload_entry_inner(source, &relative, &path).await;
        self.active.lock().map_err(lock_error)?.remove(&key);
        result
    }

    async fn upload_entry_inner(
        &self,
        source: &BackupSourceRow,
        relative: &str,
        path: &Path,
    ) -> AppResult<()> {
        let metadata = tokio::fs::metadata(path).await?;
        tokio::time::sleep(WATCH_DEBOUNCE).await;
        let stable = tokio::fs::metadata(path).await?;
        if metadata.len() != stable.len() || metadata.modified().ok() != stable.modified().ok() {
            return Ok(());
        }
        let hash = hash_file(path).await?;
        let existing = self
            .db
            .list_backup_entries(source.id.clone())
            .await?
            .into_iter()
            .find(|entry| entry.relative_path == relative);
        if existing
            .as_ref()
            .and_then(|entry| entry.content_hash.as_deref())
            == Some(hash.as_str())
            && existing
                .as_ref()
                .is_some_and(|entry| entry.status == "verified")
        {
            return Ok(());
        }
        let remote_name = remote_name(&source.id, relative);
        let file = self
            .storage
            .upload_file_as(
                source.account_id.clone(),
                source.remote_root_id.clone(),
                path.to_string_lossy().to_string(),
                remote_name,
            )
            .await;
        match file {
            Ok(file) => {
                self.db
                    .upsert_backup_entry(BackupEntryRow {
                        source_id: source.id.clone(),
                        relative_path: relative.into(),
                        local_size: Some(i64::try_from(stable.len()).unwrap_or(i64::MAX)),
                        local_mtime_ns: stable
                            .modified()
                            .ok()
                            .and_then(unix_nanos)
                            .and_then(|value| i64::try_from(value).ok()),
                        content_hash: Some(hash),
                        remote_item_id: Some(file.id),
                        status: "verified".into(),
                        last_error: None,
                        dirty_at: None,
                        uploaded_at: Some(now()),
                        updated_at: now(),
                    })
                    .await
            }
            Err(error) => {
                let _ = self
                    .db
                    .upsert_backup_entry(BackupEntryRow {
                        source_id: source.id.clone(),
                        relative_path: relative.into(),
                        local_size: Some(i64::try_from(stable.len()).unwrap_or(i64::MAX)),
                        local_mtime_ns: stable
                            .modified()
                            .ok()
                            .and_then(unix_nanos)
                            .and_then(|value| i64::try_from(value).ok()),
                        content_hash: Some(hash),
                        remote_item_id: existing.and_then(|entry| entry.remote_item_id),
                        status: "failed".into(),
                        last_error: Some(error.to_string()),
                        dirty_at: Some(now()),
                        uploaded_at: None,
                        updated_at: now(),
                    })
                    .await;
                Err(error)
            }
        }
    }
}

fn due_for_lazy(entry: &BackupEntryRow, source: &BackupSourceRow) -> bool {
    let Some(dirty_at) = entry
        .dirty_at
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return false;
    };
    let age = unix_seconds().saturating_sub(dirty_at);
    age >= source.lazy_interval_seconds || age >= source.max_delay_seconds
}

async fn canonical_directory(value: &str) -> AppResult<PathBuf> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(AppError::Other(
            "Backup folder must be an absolute path".into(),
        ));
    }
    let canonical = tokio::fs::canonicalize(path).await?;
    if !canonical.is_dir() {
        return Err(AppError::Other("Backup path must be a folder".into()));
    }
    Ok(canonical)
}

async fn scan_files(root: PathBuf) -> AppResult<Vec<(PathBuf, u64, Option<i64>)>> {
    tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        for item in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if item.file_type().is_file() && !should_ignore(item.path()) {
                let metadata = item
                    .metadata()
                    .map_err(|error| AppError::Other(error.to_string()))?;
                files.push((
                    item.path().to_path_buf(),
                    metadata.len(),
                    metadata
                        .modified()
                        .ok()
                        .and_then(unix_nanos)
                        .and_then(|value| i64::try_from(value).ok()),
                ));
            }
        }
        Ok::<_, AppError>(files)
    })
    .await
    .map_err(|error| AppError::Other(format!("Backup scan failed: {error}")))?
}

async fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn relative_path(root: &Path, path: &Path) -> AppResult<String> {
    path.strip_prefix(root)
        .map_err(|_| AppError::Other("Backup file escaped its root".into()))
        .map(|value| value.to_string_lossy().replace('\\', "/"))
}

fn remote_name(source_id: &str, relative: &str) -> String {
    let source = source_id.chars().take(8).collect::<String>();
    let encoded = relative.replace('/', "__");
    let name = format!("agnes-{source}--{encoded}");
    name.chars().take(240).collect()
}

fn should_ignore(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.ends_with('~')
                || name.ends_with(".tmp")
                || name.ends_with(".swp")
                || name.ends_with(".part")
                || name.starts_with("~$")
        })
}

fn unix_nanos(value: SystemTime) -> Option<u128> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}
fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}
fn now() -> String {
    unix_seconds().to_string()
}
fn lock_error<T>(_: std::sync::PoisonError<T>) -> AppError {
    AppError::Other("Backup service lock is poisoned".into())
}

#[cfg(test)]
mod tests {
    use super::{remote_name, should_ignore};

    #[test]
    fn remote_names_are_stable_and_flatten_relative_paths() {
        assert_eq!(
            remote_name("12345678-rest", "docs/readme.md"),
            "agnes-12345678--docs__readme.md"
        );
    }

    #[test]
    fn temporary_files_are_ignored() {
        assert!(should_ignore(std::path::Path::new("a/file.swp")));
        assert!(!should_ignore(std::path::Path::new("a/file.txt")));
    }
}
