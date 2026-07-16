use std::sync::Arc;

use crate::agent::AgentManager;
use crate::db::DbActorHandle;
use crate::secrets::SecretStore;
use crate::sync::engine::SyncService;

/// 应用托管的全局状态（经 Tauri `State` 注入命令）。
pub struct AppState {
    pub db: DbActorHandle,
    pub agent: Arc<AgentManager>,
    pub secrets: Arc<dyn SecretStore>,
    pub sync: Arc<SyncService>,
    pub secret_store_startup_error: Option<String>,
}
