use std::path::PathBuf;
use std::sync::Arc;

use crate::agent::AgentManager;
use crate::db::DbActorHandle;
use crate::document_parser::DocumentParserManager;
use crate::mcp::McpManager;
use crate::notifications::NotificationService;
use crate::pdf_models::PdfModelPackageManager;
use crate::secrets::SecretStore;
use crate::storage::StorageService;
use crate::sync::engine::SyncService;

/// 应用托管的全局状态（经 Tauri `State` 注入命令）。
pub struct AppState {
    pub app_handle: tauri::AppHandle,
    pub data_dir: PathBuf,
    pub db: DbActorHandle,
    pub agent: Arc<AgentManager>,
    pub mcp: Arc<McpManager>,
    pub secrets: Arc<dyn SecretStore>,
    pub storage: Arc<StorageService>,
    pub sync: Arc<SyncService>,
    pub notifications: Arc<NotificationService>,
    pub document_parser: Arc<DocumentParserManager>,
    pub pdf_models: Arc<PdfModelPackageManager>,
    pub secret_store_startup_error: Option<String>,
}
