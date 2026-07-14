use std::sync::Arc;

use crate::agent::AgentManager;
use crate::db::DbActorHandle;

/// 应用托管的全局状态（经 Tauri `State` 注入命令）。
pub struct AppState {
    pub db: DbActorHandle,
    pub agent: Arc<AgentManager>,
}
