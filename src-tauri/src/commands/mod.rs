use serde::Serialize;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
}

/// 健康检查：验证 React ↔ Rust IPC 通道。
#[tauri::command]
pub async fn ping() -> String {
    "pong from Rust".to_string()
}

/// 列出所有 Agent（角色卡）。数据经 DbActor 单写者线程读取。
#[tauri::command]
pub async fn list_agents(state: tauri::State<'_, AppState>) -> AppResult<Vec<AgentSummary>> {
    let rows = state.db.list_agents().await?;
    Ok(rows
        .into_iter()
        .map(|r| AgentSummary {
            id: r.id,
            name: r.name,
        })
        .collect())
}
