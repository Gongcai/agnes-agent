use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::agent::protocol::{msg_type, Envelope};
use crate::db::repo::agents::{AgentRow, AgentUpdate, NewAgent};
use crate::db::repo::messages::{NewMessagePart, NewUserMessagePart};
use crate::db::repo::sessions::NewSession;
use crate::error::{AppError, AppResult};
use crate::model_registry::{
    descriptor_from_api, load_model_roles, normalize_model_catalog, parse_model_catalog,
    save_model_roles, ModelDescriptor, ModelRole, ModelRoleAssignments,
};
use crate::secrets::{provider_api_key_secret_id, SecretStore, SYNC_CREDENTIAL_SECRET_ID};
use crate::state::AppState;
use crate::sync::auth::SyncCredential;
use crate::tools::ToolPolicy;

const MAX_OUTPUT_TOKENS: i64 = 1_048_576;
const DEFAULT_MAX_OUTPUT_TOKENS: i64 = 131_072;
const DEFAULT_MAX_OUTPUT_TOKENS_SETTING: &str = "ui:default_max_output_tokens";

fn normalize_default_max_output_tokens(value: Option<&str>) -> i64 {
    value
        .and_then(|raw| raw.parse::<i64>().ok())
        .map(|value| value.clamp(128, MAX_OUTPUT_TOKENS))
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
}

async fn load_default_max_output_tokens(state: &AppState) -> AppResult<i64> {
    let value = state
        .db
        .get_setting(DEFAULT_MAX_OUTPUT_TOKENS_SETTING.to_string())
        .await?;
    Ok(normalize_default_max_output_tokens(value.as_deref()))
}

fn deserialize_double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub persona: String,
    pub scenario: String,
    pub system_prompt: String,
    pub greeting: String,
    pub example_dialogue: String,
    pub model: String,
    pub tool_policy: String,
    pub avatar: String,
    pub tags: String,
    pub thinking_mode: String,
    pub thinking_budget: i64,
}

#[derive(Serialize)]
pub struct SessionDto {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
    pub model: String,
    pub thinking_mode: String,
    pub thinking_budget: i64,
    pub max_tokens: i64,
    pub context_limit: Option<i64>,
    pub compress_threshold: f64,
    pub permission_mode: String,
    pub workspace_id: Option<String>,
}

#[derive(Serialize)]
pub struct WorkspaceDto {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub folder_path: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 调试面板：当前拼装好、待发送给 LLM 的提示词。
#[derive(Serialize)]
pub struct DebugPromptDto {
    pub system_prompt: String,
    pub messages: Vec<serde_json::Value>,
    pub tools: Vec<serde_json::Value>,
    pub discarded_count: usize,
}

#[derive(Serialize)]
pub struct ToolCallDto {
    pub id: String,
    pub tool: String,
    pub args: String,
    pub risk: String,
    pub status: String,
    pub output: Option<String>,
}

#[derive(Serialize)]
pub struct MessagePartDto {
    pub id: String,
    pub kind: String, // "text" | "thought" | "tool_call" | "tool_result" | "model_fallback" | "attachment"
    pub content: String,
    pub mime_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub tool_call: Option<ToolCallDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatAttachmentInput {
    pub id: String,
    pub kind: String,
    pub path: Option<String>,
    pub collection_id: Option<String>,
    pub skill_id: Option<String>,
}

#[derive(Serialize)]
pub struct MessageDto {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub seq: i32,
    pub status: String,
    pub parts: Vec<MessagePartDto>,
    pub created_at: String,
    pub parent_id: Option<String>,
    pub version_index: usize,
    pub version_count: usize,
    pub is_leaf: bool,
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub context_tokens: i64,
}

#[derive(Serialize)]
pub struct TokenUsageStatsDto {
    pub input_tokens: i64,
    pub cached_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

fn usage_from_metadata(metadata: Option<&str>) -> (i64, i64, i64, i64) {
    let usage = metadata
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .and_then(|value| value.get("usage").cloned())
        .unwrap_or_default();
    (
        usage
            .get("input_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            .max(0),
        usage
            .get("cached_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            .max(0),
        usage
            .get("output_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            .max(0),
        usage
            .get("context_tokens")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            .max(0),
    )
}

/// 健康检查：验证 React ↔ Rust IPC 通道。
#[tauri::command]
pub async fn ping() -> String {
    "pong from Rust".to_string()
}

/// 列出所有 Agent（角色卡）。
#[tauri::command]
pub async fn list_agents(state: tauri::State<'_, AppState>) -> AppResult<Vec<AgentSummary>> {
    let rows = state.db.list_agents().await?;
    Ok(rows
        .into_iter()
        .map(|r| AgentSummary {
            id: r.id,
            name: r.name,
            persona: r.persona,
            scenario: r.scenario,
            system_prompt: r.system_prompt,
            greeting: r.greeting,
            example_dialogue: r.example_dialogue,
            model: r.model,
            tool_policy: r.tool_policy,
            avatar: r.avatar,
            tags: r.tags,
            thinking_mode: r.thinking_mode,
            thinking_budget: r.thinking_budget,
        })
        .collect())
}

#[tauri::command]
pub async fn update_agent_model(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    model: String,
) -> AppResult<()> {
    state.db.update_agent_model(agent_id, model).await?;
    state.sync.schedule();
    Ok(())
}

/// 角色卡可编辑字段（前端提交的载荷）。
#[derive(Deserialize)]
pub struct UpsertAgentPayload {
    pub id: Option<String>,
    pub name: String,
    pub persona: String,
    pub scenario: String,
    pub system_prompt: String,
    pub greeting: String,
    pub example_dialogue: String,
    pub model: String,
    pub tool_policy: String,
    pub avatar: String,
    pub tags: String,
    pub thinking_mode: String,
    pub thinking_budget: i64,
}

/// 创建或更新角色卡：id 为空则新建（生成 uuid），否则全量更新。
#[tauri::command]
pub async fn upsert_agent(
    state: tauri::State<'_, AppState>,
    payload: UpsertAgentPayload,
) -> AppResult<String> {
    match payload.id {
        Some(id) if !id.trim().is_empty() => {
            let changes = AgentUpdate {
                name: payload.name.trim().to_string(),
                persona: payload.persona,
                scenario: payload.scenario,
                system_prompt: payload.system_prompt,
                greeting: payload.greeting,
                example_dialogue: payload.example_dialogue,
                model: payload.model,
                tool_policy: payload.tool_policy,
                avatar: payload.avatar,
                tags: payload.tags,
                thinking_mode: payload.thinking_mode,
                thinking_budget: payload.thinking_budget,
            };
            state.db.update_agent(id.clone(), changes).await?;
            state.sync.schedule();
            Ok(id)
        }
        _ => {
            let id = uuid::Uuid::new_v4().to_string();
            let row = NewAgent {
                id: id.clone(),
                name: payload.name.trim().to_string(),
                persona: payload.persona,
                scenario: payload.scenario,
                system_prompt: payload.system_prompt,
                greeting: payload.greeting,
                example_dialogue: payload.example_dialogue,
                model: payload.model,
                tool_policy: payload.tool_policy,
                avatar: payload.avatar,
                tags: payload.tags,
                thinking_mode: payload.thinking_mode,
                thinking_budget: payload.thinking_budget,
            };
            let id = state.db.insert_agent(row).await?;
            state.sync.schedule();
            Ok(id)
        }
    }
}

/// Soft-delete an agent; dependent rows remain until sync-safe compaction.
#[tauri::command]
pub async fn delete_agent(state: tauri::State<'_, AppState>, agent_id: String) -> AppResult<()> {
    state.db.delete_agent(agent_id).await?;
    state.sync.schedule();
    Ok(())
}

/// 创建新会话。
#[tauri::command]
pub async fn create_session(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    title: String,
    workspace_id: Option<String>,
) -> AppResult<String> {
    // New sessions inherit the agent model and application output-token default.
    let agents = state.db.list_agents().await?;
    let default_max_output_tokens = load_default_max_output_tokens(&state).await?;
    let default_agent = agents.iter().find(|a| a.id == agent_id);
    let (default_model, default_thinking_mode, default_thinking_budget) = match default_agent {
        Some(a) => (a.model.clone(), a.thinking_mode.clone(), a.thinking_budget),
        None => (String::new(), String::new(), 0),
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    let new_sess = NewSession {
        id: session_id.clone(),
        agent_id,
        title,
        context_limit: None,
        compress_threshold: None,
        recency_window: None,
        reserved_output_tokens: Some(default_max_output_tokens),
        summarizer_model: None,
        model: if default_model.is_empty() {
            None
        } else {
            Some(default_model)
        },
        thinking_mode: if default_thinking_mode.is_empty() {
            None
        } else {
            Some(default_thinking_mode)
        },
        thinking_budget: if default_thinking_budget == 0 {
            None
        } else {
            Some(default_thinking_budget)
        },
        permission_mode: "auto".to_string(),
        workspace_id,
        origin_device_id: None,
    };
    state.db.insert_session(new_sess).await?;
    state.sync.schedule();
    Ok(session_id)
}

/// 列出某个 Agent 的所有会话。
#[tauri::command]
pub async fn list_sessions(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<Vec<SessionDto>> {
    let rows = state.db.list_sessions(agent_id).await?;
    let default_max_output_tokens = load_default_max_output_tokens(&state).await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionDto {
            id: r.id,
            agent_id: r.agent_id,
            title: r.title,
            summary: r.summary,
            created_at: r.created_at,
            updated_at: r.updated_at,
            pinned: r.pinned != 0,
            model: r.model.unwrap_or_default(),
            thinking_mode: r.thinking_mode.unwrap_or_default(),
            thinking_budget: r.thinking_budget.unwrap_or(0),
            max_tokens: r
                .reserved_output_tokens
                .unwrap_or(default_max_output_tokens),
            context_limit: r.context_limit,
            compress_threshold: r.compress_threshold,
            permission_mode: r.permission_mode,
            workspace_id: r.workspace_id,
        })
        .collect())
}

/// 列出某个 Agent 的所有工作区。
#[tauri::command]
pub async fn list_workspaces(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<Vec<WorkspaceDto>> {
    let rows = state.db.list_workspaces(agent_id).await?;
    Ok(rows
        .into_iter()
        .map(|r| WorkspaceDto {
            id: r.id,
            agent_id: r.agent_id,
            name: r.name,
            folder_path: r.folder_path,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// 新建工作区（绑定一个文件夹作为 AI 默认工作环境）。
#[tauri::command]
pub async fn create_workspace(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    name: String,
    folder_path: String,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .insert_workspace(crate::db::repo::workspaces::NewWorkspace {
            id: id.clone(),
            agent_id,
            name,
            folder_path,
        })
        .await?;
    state.sync.schedule();
    Ok(id)
}

/// 重命名工作区。
#[tauri::command]
pub async fn rename_workspace(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
    name: String,
) -> AppResult<()> {
    state.db.rename_workspace(workspace_id, name).await?;
    state.sync.schedule();
    Ok(())
}

/// 删除工作区（会话的 workspace_id 置 NULL，会话保留为普通对话）。
#[tauri::command]
pub async fn delete_workspace(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
) -> AppResult<()> {
    state.db.delete_workspace(workspace_id).await?;
    state.sync.schedule();
    Ok(())
}

/// 设置会话级模型与思考配置（输入框切换模型 / 思考强度时调用）。
#[tauri::command]
pub async fn set_session_llm(
    state: tauri::State<'_, AppState>,
    session_id: String,
    model: String,
    thinking_mode: String,
    thinking_budget: i64,
    max_tokens: i64,
) -> AppResult<()> {
    if !(128..=MAX_OUTPUT_TOKENS).contains(&max_tokens) {
        return Err(AppError::Other(format!(
            "max_tokens 必须在 128 到 {MAX_OUTPUT_TOKENS} 之间"
        )));
    }
    state
        .db
        .update_session_llm(
            session_id,
            model,
            thinking_mode,
            thinking_budget,
            max_tokens,
        )
        .await?;
    state.sync.schedule();
    Ok(())
}

/// Set the session threshold that triggers rolling conversation summarization.
#[tauri::command]
pub async fn set_session_compress_threshold(
    state: tauri::State<'_, AppState>,
    session_id: String,
    compress_threshold: f64,
) -> AppResult<()> {
    if !compress_threshold.is_finite() || !(0.0..=1.0).contains(&compress_threshold) {
        return Err(AppError::Other("总结阈值必须是 0 到 1 之间的数值".into()));
    }
    state
        .db
        .update_session_compress_threshold(session_id, compress_threshold)
        .await?;
    state.sync.schedule();
    Ok(())
}

/// Set the session-level tool permission mode.
#[tauri::command]
pub async fn set_session_permission_mode(
    state: tauri::State<'_, AppState>,
    session_id: String,
    permission_mode: String,
) -> AppResult<()> {
    let mode = permission_mode
        .parse::<crate::tools::PermissionMode>()
        .map_err(AppError::Other)?;
    state
        .db
        .update_session_permission_mode(session_id, mode.as_str().to_string())
        .await
}

/// 删除某个会话（软删除）。
#[tauri::command]
pub async fn delete_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<()> {
    state.db.delete_session(session_id).await?;
    state.sync.schedule();
    Ok(())
}

/// 置顶或取消置顶某个会话。
#[tauri::command]
pub async fn set_session_pin(
    state: tauri::State<'_, AppState>,
    session_id: String,
    pinned: bool,
) -> AppResult<()> {
    state.db.set_session_pin(session_id, pinned).await?;
    state.sync.schedule();
    Ok(())
}

/// 重命名某个会话。
#[tauri::command]
pub async fn rename_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
    title: String,
) -> AppResult<()> {
    state
        .db
        .update_session_title(session_id, title.trim().to_string())
        .await?;
    state.sync.schedule();
    Ok(())
}

#[tauri::command]
pub async fn get_sync_status(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::engine::SyncStatus> {
    state.sync.status().await
}

#[tauri::command]
pub async fn list_sync_conflicts(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::db::repo::sync::SyncConflictRow>> {
    state.db.list_sync_conflicts().await
}

#[tauri::command]
pub async fn resolve_sync_conflict(
    state: tauri::State<'_, AppState>,
    conflict_id: String,
    resolution: String,
) -> AppResult<()> {
    state
        .db
        .resolve_sync_conflict(conflict_id, resolution)
        .await?;
    state.sync.schedule();
    Ok(())
}

#[tauri::command]
pub async fn list_sync_devices(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::sync::protocol::SyncDevice>> {
    state.sync.list_devices().await
}

#[tauri::command]
pub async fn revoke_sync_device(
    state: tauri::State<'_, AppState>,
    device_id: String,
) -> AppResult<crate::sync::protocol::SyncDevice> {
    state.sync.revoke_device(&device_id).await
}

#[tauri::command]
pub async fn sync_now(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::engine::SyncStatus> {
    state.sync.sync_now().await
}

#[tauri::command]
pub async fn set_sync_credential(
    state: tauri::State<'_, AppState>,
    credential: Option<SyncCredential>,
) -> AppResult<crate::sync::engine::SyncStatus> {
    let previous = state.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await?;
    let replacement = credential.map(SyncCredential::into_secret).transpose()?;
    let write_result = match replacement.as_deref() {
        Some(secret) => state.secrets.set(SYNC_CREDENTIAL_SECRET_ID, secret).await,
        None => state.secrets.delete(SYNC_CREDENTIAL_SECRET_ID).await,
    };
    if let Err(write_error) = write_result {
        let rollback_error = restore_secret(
            state.secrets.as_ref(),
            SYNC_CREDENTIAL_SECRET_ID,
            previous.as_deref(),
        )
        .await
        .err();
        return Err(AppError::SecretStore(format!(
            "sync credential write failed: {write_error}{}",
            rollback_error
                .map(|error| format!("; rollback failed: {error}"))
                .unwrap_or_default()
        )));
    }

    let verification_error = match state.secrets.get(SYNC_CREDENTIAL_SECRET_ID).await {
        Ok(stored) if stored == replacement => None,
        Ok(_) => Some("stored credential did not match the submitted value".to_string()),
        Err(error) => Some(error.to_string()),
    };
    if let Some(verification_error) = verification_error {
        let rollback_error = restore_secret(
            state.secrets.as_ref(),
            SYNC_CREDENTIAL_SECRET_ID,
            previous.as_deref(),
        )
        .await
        .err();
        return Err(AppError::SecretStore(format!(
            "sync credential verification failed: {verification_error}{}",
            rollback_error
                .map(|error| format!("; rollback failed: {error}"))
                .unwrap_or_default()
        )));
    }
    if let Err(storage_error) = state
        .storage
        .ensure_managed_r2_account(crate::sync::engine::SYNC_GATEWAY_URL)
        .await
    {
        let credential_rollback_error = restore_secret(
            state.secrets.as_ref(),
            SYNC_CREDENTIAL_SECRET_ID,
            previous.as_deref(),
        )
        .await
        .err();
        let binding_rollback_error = state
            .storage
            .ensure_managed_r2_account(crate::sync::engine::SYNC_GATEWAY_URL)
            .await
            .err();
        return Err(AppError::Other(format!(
            "managed R2 account refresh failed: {storage_error}{}{}",
            credential_rollback_error
                .map(|error| format!("; credential rollback failed: {error}"))
                .unwrap_or_default(),
            binding_rollback_error
                .map(|error| format!("; binding rollback failed: {error}"))
                .unwrap_or_default()
        )));
    }
    state.sync.status().await
}

#[tauri::command]
pub async fn begin_sync_e2ee_setup(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::crypto::RecoveryMaterial> {
    state.sync.begin_e2ee_setup().await
}

#[tauri::command]
pub async fn begin_sync_e2ee_rotation(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::crypto::RecoveryMaterial> {
    state.sync.begin_e2ee_rotation().await
}

#[tauri::command]
pub async fn confirm_sync_e2ee_setup(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::engine::SyncStatus> {
    state.sync.confirm_e2ee_setup().await
}

#[tauri::command]
pub async fn restore_sync_e2ee(
    state: tauri::State<'_, AppState>,
    mut recovery_key: String,
    recovery_bundle: String,
) -> AppResult<crate::sync::engine::SyncStatus> {
    let result = state
        .sync
        .restore_e2ee(&recovery_key, &recovery_bundle)
        .await;
    recovery_key.zeroize();
    result
}

#[tauri::command]
pub async fn discard_sync_e2ee_setup(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::engine::SyncStatus> {
    state.sync.discard_e2ee_setup().await
}

#[tauri::command]
pub async fn start_sync_pairing(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::sync::pairing::PairingInvite> {
    state.sync.start_pairing().await
}

#[tauri::command]
pub async fn get_sync_pairing_request(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<crate::sync::engine::PendingPairingDevice> {
    state.sync.pending_pairing_device(&session_id).await
}

#[tauri::command]
pub async fn approve_sync_pairing(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<crate::sync::engine::PendingPairingDevice> {
    state.sync.approve_pairing(&session_id).await
}

#[tauri::command]
pub async fn join_sync_pairing(
    state: tauri::State<'_, AppState>,
    mut pairing_code: String,
    device_name: String,
) -> AppResult<crate::sync::engine::PairingJoinStarted> {
    let result = state.sync.join_pairing(&pairing_code, &device_name).await;
    pairing_code.zeroize();
    result
}

#[tauri::command]
pub async fn finish_sync_pairing(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<crate::sync::engine::PairingCompletion> {
    state.sync.finish_pairing(&session_id).await
}

/// 调试面板：调用 Python 框架拼装当前智能体（含可选会话历史）将要发送给 LLM 的完整提示词。
#[tauri::command]
pub async fn get_debug_prompt(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    session_id: Option<String>,
) -> AppResult<DebugPromptDto> {
    let agents = state.db.list_agents().await?;
    let agent = agents
        .iter()
        .find(|a| a.id == agent_id)
        .ok_or_else(|| AppError::Other(format!("智能体 `{agent_id}` 不存在")))?;

    // 会话级模型/思考优先，回退角色卡默认
    let session_opt = match &session_id {
        Some(sid) => state.db.get_session(sid.clone()).await?,
        None => None,
    };
    let session_model = session_opt
        .as_ref()
        .and_then(|s| s.model.clone())
        .unwrap_or_default();
    let model_roles = load_model_roles(&state.db).await?;
    let effective_model = if session_model.is_empty() {
        if agent.model.is_empty() {
            model_roles
                .main_model
                .clone()
                .unwrap_or_else(|| "gpt-4o".to_string())
        } else {
            agent.model.clone()
        }
    } else {
        session_model
    };
    let effective_thinking_mode = {
        let m = session_opt
            .as_ref()
            .and_then(|s| s.thinking_mode.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| agent.thinking_mode.clone());
        if m.is_empty() {
            "off".to_string()
        } else {
            m
        }
    };
    let effective_thinking_budget = session_opt
        .as_ref()
        .and_then(|s| s.thinking_budget)
        .or(Some(agent.thinking_budget))
        .unwrap_or(0);
    let default_max_output_tokens = load_default_max_output_tokens(&state).await?;
    let effective_max_tokens = session_opt
        .as_ref()
        .and_then(|session| session.reserved_output_tokens)
        .unwrap_or(default_max_output_tokens)
        .clamp(128, MAX_OUTPUT_TOKENS);

    // 复用 send_message 的 LLM 配置解析逻辑
    let agent_model_str = effective_model;
    let (provider_id_opt, model_name) = if let Some(idx) = agent_model_str.find('/') {
        (
            Some(agent_model_str[..idx].to_string()),
            agent_model_str[idx + 1..].to_string(),
        )
    } else {
        (None, agent_model_str.clone())
    };
    let provider = if let Some(ref pid) = provider_id_opt {
        state.db.get_model_provider(pid.clone()).await?
    } else {
        state.db.get_default_model_provider().await?
    };
    let (provider_kind, api_base, provider_resolved_id) = if let Some(ref p) = provider {
        (p.kind.clone(), p.api_base.clone(), Some(p.id.clone()))
    } else {
        (String::new(), None, None)
    };
    let api_key = if let Some(ref pid) = provider_resolved_id {
        state.secrets.get(&provider_api_key_secret_id(pid)).await?
    } else {
        None
    };
    let litellm_model = match provider_kind.as_str() {
        "openai" => model_name.clone(),
        "anthropic" => model_name.clone(),
        "ollama" => format!("ollama/{}", model_name),
        "openai_compatible" => format!("openai/{}", model_name),
        "google" => format!("gemini/{}", model_name),
        _ => model_name.clone(),
    };

    // 读取 USER.md / MEMORY.md（SQLite canonical 真相源）
    let (user_md, memory_md) =
        crate::user_profile::load_effective_explicit_memories(&state.db, &agent.id).await?;

    // 读取会话历史与摘要（如有）
    let mut history_json = Vec::new();
    let mut summary = None;
    let mut attachments_context = Vec::new();
    let mut attached_collection_id = None;
    if let Some(ref sid) = session_id {
        if let Ok(Some(sess)) = state.db.get_session(sid.clone()).await {
            summary = sess.summary.clone();
        }
        if let Ok(history) = state.db.list_messages_with_parts(sid.clone()).await {
            if let Some((_, parts)) = history
                .iter()
                .rev()
                .find(|(message, _)| message.role == "user")
            {
                (attachments_context, attached_collection_id) = chat_attachment_context(parts)?;
            }
            for (m, parts) in history {
                let mut parts_json = Vec::new();
                for p in parts {
                    let mut tc_json = serde_json::Value::Null;
                    if let Some(ref tc_id) = p.tool_call_id {
                        if let Ok(Some(tc)) = state.db.get_tool_call(tc_id.clone()).await {
                            tc_json = json!({
                                "id": tc.id,
                                "tool": tc.tool,
                                "args": tc.params.unwrap_or_default(),
                            });
                        }
                    }
                    parts_json.push(json!({
                        "id": p.id,
                        "kind": p.kind,
                        "content": p.content,
                        "toolCall": tc_json,
                    }));
                }
                history_json.push(json!({
                    "id": m.id,
                    "role": m.role,
                    "parts": parts_json,
                }));
            }
        }
    }
    let reading_book = match session_id.as_deref() {
        Some(session_id) => {
            state
                .db
                .get_reading_book_for_session(session_id.to_string())
                .await?
        }
        None => None,
    };
    let reading_collection_id = reading_book
        .as_ref()
        .filter(|book| !book.model_knows_content && book.content_context_allowed)
        .and_then(|book| book.collection_id.as_deref());
    let retrieval_collection_id = attached_collection_id.as_deref().or(reading_collection_id);
    let allow_hidden_collection =
        attached_collection_id.is_none() && reading_collection_id.is_some();
    let retrieved_knowledge = if reading_book.is_some() && retrieval_collection_id.is_none() {
        Vec::new()
    } else {
        retrieve_knowledge_for_history(
            &state,
            &agent.id,
            &history_json,
            retrieval_collection_id,
            allow_hidden_collection,
        )
        .await
    };
    let workspace_context =
        resolve_workspace_prompt_context(&state.db, session_id.as_deref()).await?;

    let task_llm_configs = resolve_task_llm_configs(&state, &model_roles).await?;
    let fallback_llm_configs = resolve_fallback_llm_configs(
        &state,
        &model_roles,
        &agent_model_str,
        &effective_thinking_mode,
        effective_thinking_budget,
        effective_max_tokens,
    )
    .await?;
    let parsed_tool_policy =
        serde_json::from_str::<ToolPolicy>(&agent.tool_policy).unwrap_or_default();
    let tool_policy_json = serde_json::to_value(&parsed_tool_policy)?;
    let mcp_tools = state.mcp.dynamic_tools(&parsed_tool_policy).await;
    let snapshot = json!({
        "input": "",
        "context": {
            "agent": {
                "persona": agent.persona,
                "systemPrompt": agent.system_prompt,
                "model": agent_model_str,
                "toolPolicy": tool_policy_json
            },
            "mcpTools": mcp_tools,
            "llmConfig": {
                "modelRef": agent_model_str,
                "provider": provider_kind,
                "apiBase": api_base,
                "apiKey": api_key,
                "model": model_name,
                "litellmModel": litellm_model,
                "maxTokens": effective_max_tokens,
                "thinking": {
                    "mode": effective_thinking_mode,
                    "budget": effective_thinking_budget
                }
            },
            "fallbackLlmConfigs": fallback_llm_configs,
            "taskLlmConfigs": task_llm_configs,
            "settings": { "user_context_limit": serde_json::Value::Null },
            "currentDateTime": chrono::Local::now().to_rfc3339(),
            "recentMessages": history_json,
            "summary": summary,
            "explicitMemories": { "user_md": user_md, "memory_md": memory_md },
            "retrievedMemories": [],
            "retrievedKnowledge": retrieved_knowledge,
            "readingContext": reading_book.as_ref().map(reading_prompt_context),
            "workspace": workspace_context,
            "projectContext": [],
            "attachmentsContext": attachments_context
        }
    });

    let debug_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
    state.agent.register_debug(debug_id.clone(), tx);

    let env = Envelope {
        protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
        id: debug_id.clone(),
        run_id: String::new(),
        session_id: session_id.clone().unwrap_or_default(),
        msg_type: msg_type::DEBUG_PROMPT.to_string(),
        created_at: String::new(),
        payload: snapshot,
    };

    if let Err(e) = state.agent.send_to_agent(env) {
        // 清理已注册的等待项，避免泄漏
        state
            .agent
            .resolve_debug(&debug_id, serde_json::json!({ "error": e.to_string() }));
        return Err(e);
    }

    // 等待 Python 回包，带超时保护
    let payload = match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(p)) => p,
        Ok(Err(_)) => return Err(AppError::Other("调试请求通道已断开".into())),
        Err(_) => {
            state
                .agent
                .resolve_debug(&debug_id, serde_json::json!({ "error": "timeout" }));
            return Err(AppError::Other(
                "调试提示词拼装超时（Python sidecar 未响应）".into(),
            ));
        }
    };

    if let Some(err) = payload.get("error").and_then(|x| x.as_str()) {
        return Err(AppError::Other(err.to_string()));
    }

    let system_prompt = payload
        .get("system_prompt")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let messages = payload
        .get("messages")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let tools = payload
        .get("tools")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let discarded = payload
        .get("discarded_messages")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(DebugPromptDto {
        system_prompt,
        messages,
        tools,
        discarded_count: discarded.len(),
    })
}

/// 获取某个会话中所有的消息与片段，并翻译成前端所用 Dto。
#[tauri::command]
pub async fn list_messages(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<Vec<MessageDto>> {
    let path = state.db.list_active_with_parts(session_id).await?;
    let mut msgs_dto = Vec::new();

    for am in path.messages {
        let msg = am.message;
        let (input_tokens, cached_tokens, output_tokens, context_tokens) =
            usage_from_metadata(msg.metadata.as_deref());
        let mut parts_dto = Vec::new();
        for p in am.parts {
            let mut tool_call_dto = None;
            if let Some(ref tc_id) = p.tool_call_id {
                if let Ok(Some(tc_row)) = state.db.get_tool_call(tc_id.clone()).await {
                    let status = match tc_row.status.as_str() {
                        "done" => "succeeded",
                        "rejected" => "denied",
                        "cancelled" => "failed",
                        value => value,
                    }
                    .to_string();
                    tool_call_dto = Some(ToolCallDto {
                        id: tc_row.id,
                        tool: tc_row.tool,
                        args: tc_row.params.unwrap_or_default(),
                        risk: tc_row.risk_level.unwrap_or_else(|| "Low".to_string()),
                        status,
                        output: tc_row.stdout.or(tc_row.stderr),
                    });
                }
            }

            // 将数据库的 "reasoning" 种类翻译为前端的 "thought"
            let is_attachment = p.kind == "attachment";
            let kind_mapped = if p.kind == "reasoning" {
                "thought".to_string()
            } else {
                p.kind
            };

            parts_dto.push(MessagePartDto {
                id: p.id,
                kind: kind_mapped,
                content: if is_attachment {
                    String::new()
                } else {
                    p.content
                },
                mime_type: p.mime_type,
                metadata: p
                    .metadata
                    .as_deref()
                    .and_then(|value| serde_json::from_str(value).ok()),
                tool_call: tool_call_dto,
            });
        }

        msgs_dto.push(MessageDto {
            id: msg.id,
            session_id: msg.session_id,
            role: msg.role,
            seq: msg.seq,
            status: msg.status,
            parts: parts_dto,
            created_at: msg.created_at,
            parent_id: msg.parent_id,
            version_index: am.version_index,
            version_count: am.version_count,
            is_leaf: am.is_leaf,
            input_tokens,
            cached_tokens,
            output_tokens,
            context_tokens,
        });
    }

    Ok(msgs_dto)
}

#[tauri::command]
pub async fn get_token_usage_stats(
    state: tauri::State<'_, AppState>,
    agent_id: Option<String>,
) -> AppResult<TokenUsageStatsDto> {
    let (input_tokens, cached_tokens, output_tokens) =
        state.db.token_usage_totals(agent_id).await?;
    Ok(TokenUsageStatsDto {
        input_tokens,
        cached_tokens,
        output_tokens,
        total_tokens: input_tokens + output_tokens,
    })
}

/// 切换某消息的版本（prev/next 同级）。同级共享 parent_id；切换即把父的
/// selected_child_id 指向目标同级，活动路径随之改走该同级的子树。
#[tauri::command]
pub async fn switch_version(
    state: tauri::State<'_, AppState>,
    message_id: String,
    direction: String, // "prev" | "next"
) -> AppResult<()> {
    let msg = state
        .db
        .get_message(message_id.clone())
        .await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    let parent_id = msg
        .parent_id
        .clone()
        .ok_or_else(|| AppError::Other("根消息无同级版本".into()))?;
    let all = state
        .db
        .list_messages_with_parts(msg.session_id.clone())
        .await?;
    // 同级：parent_id 相同，按 seq 升序（list 已排序）
    let siblings: Vec<String> = all
        .iter()
        .filter(|(m, _)| m.parent_id.as_deref() == Some(parent_id.as_str()))
        .map(|(m, _)| m.id.clone())
        .collect();
    let idx = siblings
        .iter()
        .position(|id| id == &message_id)
        .ok_or_else(|| AppError::Other("找不到当前消息".into()))?;
    let new_idx = match direction.as_str() {
        "prev" => {
            if idx == 0 {
                return Ok(());
            } else {
                idx - 1
            }
        }
        "next" => {
            if idx + 1 >= siblings.len() {
                return Ok(());
            } else {
                idx + 1
            }
        }
        _ => return Ok(()),
    };
    let new_id = siblings[new_idx].clone();
    state.db.set_selected_child(parent_id, Some(new_id)).await?;
    state.sync.schedule();
    Ok(())
}

/// 创建分支：把该消息设为活动叶子（selected_child_id=NULL），其后代保留为
/// 可切回的同级；下次发消息即从该点长新枝。
#[tauri::command]
pub async fn create_branch(state: tauri::State<'_, AppState>, message_id: String) -> AppResult<()> {
    state.db.set_selected_child(message_id, None).await?;
    state.sync.schedule();
    Ok(())
}

/// 删除单条消息（仅叶子、非 pending/streaming）。若是父的 selected_child，
/// 把父的 selected_child 置 NULL，活动路径回退到父。
#[tauri::command]
pub async fn delete_message(
    state: tauri::State<'_, AppState>,
    message_id: String,
) -> AppResult<()> {
    let msg = state
        .db
        .get_message(message_id.clone())
        .await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.status == "pending" || msg.status == "streaming" {
        return Err(AppError::Other("消息正在生成，无法删除".into()));
    }
    let cnt = state.db.count_children(message_id.clone()).await?;
    if cnt > 0 {
        return Err(AppError::Other("仅可删除末梢消息（无后续消息）".into()));
    }
    state.db.delete_message(message_id).await?;
    state.sync.schedule();
    Ok(())
}

/// 编辑并重发：把编辑后的文本作为旧 user 消息的同级新版本插入（共享 parent），
/// 切换活动路径到新 user，造新 pending assistant，启动运行。旧 user 子树保留可切回。
#[tauri::command]
pub async fn edit_and_resend(
    state: tauri::State<'_, AppState>,
    message_id: String,
    text: String,
) -> AppResult<()> {
    let msg = state
        .db
        .get_message(message_id.clone())
        .await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.role != "user" {
        return Err(AppError::Other("仅可编辑用户消息".into()));
    }
    let cfg = resolve_llm(&state, &msg.session_id).await?;
    let parent_id = msg.parent_id.clone();
    let retained_attachments = state
        .db
        .list_messages_with_parts(msg.session_id.clone())
        .await?
        .into_iter()
        .find(|(message, _)| message.id == message_id)
        .map(|(_, parts)| {
            parts
                .into_iter()
                .filter(|part| part.kind == "attachment")
                .map(|part| NewUserMessagePart {
                    kind: part.kind,
                    mime_type: part.mime_type,
                    content: part.content,
                    metadata: part.metadata,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut user_parts = vec![text_user_message_part(text)];
    user_parts.extend(retained_attachments);
    let (_user_id, assistant_msg_id) = state
        .db
        .append_user_and_assistant(
            msg.session_id.clone(),
            parent_id,
            user_parts,
            cfg.model_name.clone(),
        )
        .await?;
    state.sync.schedule();
    start_agent_run_or_mark_failed(
        &state,
        &cfg,
        &msg.session_id,
        &assistant_msg_id,
        false,
        false,
        None,
    )
    .await
}

/// 单条重新生成：为 AI 消息造一个同级新版本（共享父 user 消息），切换活动路径
/// 到新 AI，启动运行。旧 AI 子树保留可切回。
#[tauri::command]
pub async fn regenerate_message(
    state: tauri::State<'_, AppState>,
    message_id: String,
) -> AppResult<()> {
    let msg = state
        .db
        .get_message(message_id.clone())
        .await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.role != "assistant" {
        return Err(AppError::Other("仅可重新生成 AI 消息".into()));
    }
    let parent_user_id = msg
        .parent_id
        .clone()
        .ok_or_else(|| AppError::Other("AI 消息无父用户消息".into()))?;
    // The rolling summary may include the rejected answer even though the
    // selected message branch changes to the new pending sibling. Invalidate
    // it before rebuilding context so retry never feeds that answer back.
    state
        .db
        .update_session_summary(msg.session_id.clone(), String::new())
        .await?;
    let cfg = resolve_llm(&state, &msg.session_id).await?;
    let assistant_msg_id = state
        .db
        .append_assistant_sibling(
            msg.session_id.clone(),
            parent_user_id,
            cfg.model_name.clone(),
        )
        .await?;
    state.sync.schedule();
    start_agent_run_or_mark_failed(
        &state,
        &cfg,
        &msg.session_id,
        &assistant_msg_id,
        true,
        false,
        None,
    )
    .await
}

/// 修改记忆：替换某条 AI 消息的全部片段（用户在弹窗里编辑/删除/新增片段）。
/// 仅 complete 的 AI 消息可改。kind 泛型，不特判 thought/tool_call 等。
#[derive(Deserialize)]
pub struct PartInput {
    pub kind: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub metadata: Option<String>,
}

#[tauri::command]
pub async fn replace_message_parts(
    state: tauri::State<'_, AppState>,
    message_id: String,
    parts: Vec<PartInput>,
) -> AppResult<()> {
    let msg = state
        .db
        .get_message(message_id.clone())
        .await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.role != "assistant" {
        return Err(AppError::Other("仅可修改 AI 消息".into()));
    }
    if msg.status != "complete" {
        return Err(AppError::Other("仅可修改已完成的 AI 消息".into()));
    }
    let new_parts: Vec<NewMessagePart> = parts
        .into_iter()
        .map(|p| NewMessagePart {
            id: uuid::Uuid::new_v4().to_string(),
            message_id: message_id.clone(),
            kind: p.kind,
            ordinal: 0, // actor handler 会按顺序重排
            mime_type: None,
            tool_call_id: p.tool_call_id,
            content: p.content,
            metadata: p.metadata,
        })
        .collect();
    state
        .db
        .replace_message_parts(message_id, new_parts)
        .await?;
    state.sync.schedule();
    Ok(())
}

/// 取消某会话当前活跃的运行：取出 session→run_id 映射，向 Python 发 RUN_CANCEL。
/// 同时触发挂起审批的取消信号，解除 ws_server 在 approval oneshot 上的阻塞
/// （否则 ws 循环卡死，RUN_ERROR 与后续消息都无法处理）。Python 收到后取消对应任务。
#[tauri::command]
pub async fn cancel_run(state: tauri::State<'_, AppState>, session_id: String) -> AppResult<()> {
    if let Some(run_id) = state.agent.remove_session_run(&session_id) {
        state.agent.mark_run_cancelled(&run_id);
        // 先触发审批取消信号，解除 ws_server 的 approval await 阻塞
        // （用 select! 分支，handler 不会发 TOOL_RESULT，避免 Python 任务被工具结果唤醒继续）
        if let Some(tx) = state.agent.take_run_cancel_signal(&run_id) {
            let _ = tx.send(());
        }
        let env = Envelope {
            protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            run_id,
            session_id,
            msg_type: msg_type::RUN_CANCEL.to_string(),
            created_at: String::new(),
            payload: serde_json::json!({}),
        };
        state.agent.send_to_agent(env)?;
    }
    Ok(())
}

/// 将占位提示文本视为空，避免被当作真实记忆发送给 AI（占位仅由前端展示）。
/// 解析出的 LLM 运行配置（会话级优先，回退角色卡默认）。
struct ResolvedLlm {
    agent: AgentRow,
    session_context_limit: Option<i64>,
    compress_threshold: f64,
    session_summary: Option<String>,
    effective_model: String,
    model_name: String,
    provider_kind: String,
    api_base: Option<String>,
    api_key: Option<String>,
    litellm_model: String,
    thinking_mode: String,
    thinking_budget: i64,
    max_tokens: i64,
    task_llm_configs: serde_json::Value,
    fallback_llm_configs: serde_json::Value,
}

const DEFAULT_SESSION_TITLE: &str = "新会话";
const SESSION_TITLE_MAX_CHARS: usize = 40;

fn is_automatic_session_title(value: &str) -> bool {
    let title = value.trim();
    if title == DEFAULT_SESSION_TITLE {
        return true;
    }
    ["新会话 #", "会话 #"].iter().any(|prefix| {
        title.strip_prefix(prefix).is_some_and(|number| {
            !number.is_empty() && number.chars().all(|char| char.is_ascii_digit())
        })
    })
}

pub(crate) fn normalize_session_title(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized
        .trim_matches(|character: char| {
            matches!(
                character,
                '`' | '*' | '_' | '#' | '"' | '\'' | '“' | '”' | '‘' | '’'
            )
        })
        .trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut chars = trimmed.chars();
    let mut title = chars
        .by_ref()
        .take(SESSION_TITLE_MAX_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        title = title.trim_end().to_string();
        title.push('…');
    }
    Some(title)
}

fn fallback_session_title(source_text: &str) -> String {
    normalize_session_title(source_text).unwrap_or_else(|| DEFAULT_SESSION_TITLE.to_string())
}

/// Resolve a routed model reference into the same provider configuration used by the main model.
async fn resolve_routed_llm_config(
    state: &AppState,
    model_ref: Option<&str>,
) -> AppResult<Option<serde_json::Value>> {
    let Some(model_ref) = model_ref.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some((provider_id, model_name)) = model_ref.split_once('/') else {
        return Ok(None);
    };
    let Some(provider) = state.db.get_model_provider(provider_id.to_string()).await? else {
        return Ok(None);
    };
    let api_key = state
        .secrets
        .get(&provider_api_key_secret_id(&provider.id))
        .await?;
    let litellm_model = match provider.kind.as_str() {
        "openai" | "anthropic" => model_name.to_string(),
        "ollama" => format!("ollama/{model_name}"),
        "openai_compatible" => format!("openai/{model_name}"),
        "google" => format!("gemini/{model_name}"),
        _ => model_name.to_string(),
    };
    Ok(Some(json!({
        "modelRef": model_ref,
        "provider": provider.kind,
        "apiBase": provider.api_base,
        "apiKey": api_key,
        "model": model_name,
        "litellmModel": litellm_model,
        "thinking": { "mode": "off", "budget": 0 }
    })))
}

/// Build task-specific LLM configs. Unimplemented consumers can adopt these keys without a DB change.
async fn resolve_task_llm_configs(
    state: &AppState,
    roles: &ModelRoleAssignments,
) -> AppResult<serde_json::Value> {
    let mut configs = serde_json::Map::new();
    for (key, selection) in [
        ("image", roles.image_model.as_deref()),
        ("summary", roles.summary_model.as_deref()),
        ("memory", roles.memory_model.as_deref()),
        ("speech", roles.speech_model.as_deref()),
        ("quick", roles.quick_model.as_deref()),
        ("embedding", roles.embedding_model.as_deref()),
    ] {
        if let Some(config) = resolve_routed_llm_config(state, selection).await? {
            configs.insert(key.to_string(), config);
        }
    }
    Ok(serde_json::Value::Object(configs))
}

async fn resolve_fallback_llm_configs(
    state: &AppState,
    roles: &ModelRoleAssignments,
    effective_model: &str,
    thinking_mode: &str,
    thinking_budget: i64,
    max_tokens: i64,
) -> AppResult<serde_json::Value> {
    let mut configs = Vec::new();
    for model_ref in &roles.fallback_models {
        if model_ref == effective_model {
            continue;
        }
        let Some(mut config) = resolve_routed_llm_config(state, Some(model_ref)).await? else {
            continue;
        };
        if let Some(object) = config.as_object_mut() {
            object.insert("maxTokens".into(), json!(max_tokens));
            object.insert(
                "thinking".into(),
                json!({ "mode": thinking_mode, "budget": thinking_budget }),
            );
        }
        configs.push(config);
    }
    Ok(serde_json::Value::Array(configs))
}

async fn resolve_workspace_prompt_context(
    db: &crate::db::DbActorHandle,
    session_id: Option<&str>,
) -> AppResult<Option<serde_json::Value>> {
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    let Some(session) = db.get_session(session_id.to_string()).await? else {
        return Ok(None);
    };
    let Some(workspace_id) = session.workspace_id else {
        return Ok(Some(json!({
            "name": "Home",
            "mode": "home",
            "hasLocalFolderBinding": true,
        })));
    };
    let Some(workspace) = db.get_workspace(workspace_id).await? else {
        return Ok(None);
    };

    Ok(Some(json!({
        "name": workspace.name,
        "mode": "code",
        "hasLocalFolderBinding": !workspace.folder_path.trim().is_empty(),
    })))
}

/// 解析会话当前生效的 LLM 配置（模型/思考/provider/密钥）。
async fn resolve_llm(state: &AppState, session_id: &str) -> AppResult<ResolvedLlm> {
    let session = state
        .db
        .get_session(session_id.to_string())
        .await?
        .ok_or_else(|| AppError::Other(format!("会话 `{session_id}` 不存在")))?;
    let agents = state.db.list_agents().await?;
    let agent = agents
        .iter()
        .find(|a| a.id == session.agent_id)
        .ok_or_else(|| AppError::Other(format!("关联的智能体 `{}` 不存在", session.agent_id)))?
        .clone();

    let model_roles = load_model_roles(&state.db).await?;
    let session_model = session.model.clone().unwrap_or_default();
    let effective_model = if session_model.is_empty() {
        if agent.model.is_empty() {
            model_roles
                .main_model
                .clone()
                .unwrap_or_else(|| "gpt-4o".to_string())
        } else {
            agent.model.clone()
        }
    } else {
        session_model
    };
    let (provider_id_opt, model_name) = if let Some(idx) = effective_model.find('/') {
        (
            Some(effective_model[..idx].to_string()),
            effective_model[idx + 1..].to_string(),
        )
    } else {
        (None, effective_model.clone())
    };
    let provider = if let Some(ref pid) = provider_id_opt {
        state.db.get_model_provider(pid.clone()).await?
    } else {
        state.db.get_default_model_provider().await?
    };
    let (provider_kind, api_base, provider_resolved_id) = if let Some(ref p) = provider {
        (p.kind.clone(), p.api_base.clone(), Some(p.id.clone()))
    } else {
        (String::new(), None, None)
    };
    let api_key = if let Some(ref pid) = provider_resolved_id {
        state.secrets.get(&provider_api_key_secret_id(pid)).await?
    } else {
        None
    };
    let litellm_model = match provider_kind.as_str() {
        "openai" => model_name.clone(),
        "anthropic" => model_name.clone(),
        "ollama" => format!("ollama/{}", model_name),
        "openai_compatible" => format!("openai/{}", model_name),
        "google" => format!("gemini/{}", model_name),
        _ => model_name.clone(),
    };
    let model_context_limit = provider
        .as_ref()
        .and_then(|provider| {
            parse_model_catalog(provider.models_json.as_deref(), &provider.kind)
                .into_iter()
                .find(|model| model.id == model_name)
        })
        .and_then(|model| model.context_window)
        .and_then(|value| i64::try_from(value).ok());
    let thinking_mode = {
        let m = session
            .thinking_mode
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| agent.thinking_mode.clone());
        if m.is_empty() {
            "off".to_string()
        } else {
            m
        }
    };
    let thinking_budget = session
        .thinking_budget
        .or(Some(agent.thinking_budget))
        .unwrap_or(0);
    let default_max_output_tokens = load_default_max_output_tokens(state).await?;
    let max_tokens = session
        .reserved_output_tokens
        .unwrap_or(default_max_output_tokens)
        .clamp(128, MAX_OUTPUT_TOKENS);

    let task_llm_configs = resolve_task_llm_configs(state, &model_roles).await?;
    let fallback_llm_configs = resolve_fallback_llm_configs(
        state,
        &model_roles,
        &effective_model,
        &thinking_mode,
        thinking_budget,
        max_tokens,
    )
    .await?;
    Ok(ResolvedLlm {
        agent,
        session_context_limit: session.context_limit.or(model_context_limit),
        compress_threshold: session.compress_threshold.clamp(0.0, 1.0),
        session_summary: session.summary,
        effective_model,
        model_name,
        provider_kind,
        api_base,
        api_key,
        litellm_model,
        thinking_mode,
        thinking_budget,
        max_tokens,
        task_llm_configs,
        fallback_llm_configs,
    })
}

fn chat_attachment_context(
    parts: &[crate::db::repo::messages::MessagePartRow],
) -> AppResult<(Vec<serde_json::Value>, Option<String>)> {
    let mut context = Vec::new();
    let mut collection_id = None;
    for part in parts.iter().filter(|part| part.kind == "attachment") {
        let Some(metadata) = part
            .metadata
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        else {
            continue;
        };
        match metadata
            .get("attachmentKind")
            .and_then(serde_json::Value::as_str)
        {
            Some("local_file") => context.push(json!({
                "kind": "local_file",
                "name": metadata.get("name").and_then(serde_json::Value::as_str).unwrap_or("未命名附件"),
                "path": metadata.get("path").and_then(serde_json::Value::as_str),
                "mediaType": part.mime_type.as_deref(),
                "content": part.content,
            })),
            Some("knowledge_collection") => {
                if let Some(id) = metadata
                    .get("collectionId")
                    .and_then(serde_json::Value::as_str)
                {
                    collection_id = Some(id.to_string());
                    context.push(json!({
                        "kind": "knowledge_collection",
                        "name": metadata.get("name").and_then(serde_json::Value::as_str).unwrap_or("未命名知识库"),
                        "collectionId": id,
                    }));
                }
            }
            Some("skill") => {
                if let Some(id) = metadata.get("skillId").and_then(serde_json::Value::as_str) {
                    let skill = crate::skills::load_for_prompt(id)?;
                    context.push(json!({
                        "kind": "skill",
                        "id": skill.id,
                        "name": skill.name,
                        "description": skill.description,
                        "instructions": skill.instructions,
                        "rootPath": skill.root_path,
                        "resources": skill.resources,
                    }));
                }
            }
            _ => {}
        }
    }
    Ok((context, collection_id))
}

/// 用已解析配置启动一次 agent 运行：构建活动路径历史（跳过 pending assistant）→
/// 编译 snapshot（input="" 避免与 recentMessages 重复）→ 注册 run_id → 发 RUN_REQUEST。
async fn start_agent_run(
    state: &AppState,
    cfg: &ResolvedLlm,
    session_id: &str,
    assistant_msg_id: &str,
    is_regeneration: bool,
    include_reading_context: bool,
    session_title_request: Option<serde_json::Value>,
) -> AppResult<()> {
    eprintln!("[agent][run] Preparing run for session={session_id} assistant={assistant_msg_id}");
    let (user_md, memory_md) =
        crate::user_profile::load_effective_explicit_memories(&state.db, &cfg.agent.id).await?;
    eprintln!("[agent][run] Explicit memories loaded session={session_id}");

    let path = state
        .db
        .list_active_with_parts(session_id.to_string())
        .await?;
    eprintln!("[agent][run] Active history loaded session={session_id}");
    let (attachments_context, attached_collection_id) = match path
        .messages
        .iter()
        .rev()
        .find(|message| message.message.role == "user")
    {
        Some(message) => chat_attachment_context(&message.parts)?,
        None => (Vec::new(), None),
    };
    let mut history_json = Vec::new();
    for am in path.messages {
        // 跳过 pending assistant 占位消息，只把完整历史发给 Python
        if am.message.id == assistant_msg_id {
            continue;
        }
        let mut parts_json = Vec::new();
        for p in am.parts {
            let mut tc_json = serde_json::Value::Null;
            if let Some(ref tc_id) = p.tool_call_id {
                if let Ok(Some(tc)) = state.db.get_tool_call(tc_id.clone()).await {
                    tc_json = json!({
                        "id": tc.id,
                        "tool": tc.tool,
                        "args": tc.params.unwrap_or_default(),
                    });
                }
            }
            parts_json.push(json!({
                "id": p.id,
                "kind": p.kind,
                "content": p.content,
                "toolCall": tc_json,
            }));
        }
        history_json.push(json!({
            "id": am.message.id,
            "role": am.message.role,
            "parts": parts_json,
        }));
    }
    let reading_book = if include_reading_context {
        match tokio::time::timeout(
            std::time::Duration::from_millis(500),
            state
                .db
                .get_reading_book_for_session(session_id.to_string()),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                eprintln!(
                    "[agent][run] Reading context lookup timed out; continuing without book context session={session_id}"
                );
                None
            }
        }
    } else {
        None
    };
    eprintln!("[agent][run] Reading context loaded session={session_id}");
    let reading_collection_id = reading_book
        .as_ref()
        .filter(|book| !book.model_knows_content && book.content_context_allowed)
        .and_then(|book| book.collection_id.as_deref());
    let retrieval_collection_id = attached_collection_id.as_deref().or(reading_collection_id);
    let allow_hidden_collection =
        attached_collection_id.is_none() && reading_collection_id.is_some();
    let retrieved_knowledge = if reading_book.is_some() && retrieval_collection_id.is_none() {
        Vec::new()
    } else {
        let retrieval = retrieve_knowledge_for_history(
            state,
            &cfg.agent.id,
            &history_json,
            retrieval_collection_id,
            allow_hidden_collection,
        );
        match tokio::time::timeout(std::time::Duration::from_secs(5), retrieval).await {
            Ok(results) => results,
            Err(_) => {
                eprintln!(
                    "[rag] Knowledge retrieval timed out; continuing without retrieved context session={session_id}"
                );
                Vec::new()
            }
        }
    };
    eprintln!(
        "[agent][run] Context ready session={session_id} history={} knowledge={} max_tokens={}",
        history_json.len(),
        retrieved_knowledge.len(),
        cfg.max_tokens,
    );
    let workspace_context = match tokio::time::timeout(
        std::time::Duration::from_millis(500),
        resolve_workspace_prompt_context(&state.db, Some(session_id)),
    )
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            eprintln!(
                "[agent][run] Workspace context lookup timed out; continuing without workspace context session={session_id}"
            );
            None
        }
    };

    let parsed_tool_policy =
        serde_json::from_str::<ToolPolicy>(&cfg.agent.tool_policy).unwrap_or_default();
    let tool_policy_json = serde_json::to_value(&parsed_tool_policy)?;
    let mcp_tools = state.mcp.dynamic_tools(&parsed_tool_policy).await;
    let run_id = uuid::Uuid::new_v4().to_string();
    let context_snapshot = json!({
        "input": "",
        "context": {
            "agent": {
                "persona": cfg.agent.persona,
                "systemPrompt": cfg.agent.system_prompt,
                "model": cfg.effective_model,
                "toolPolicy": tool_policy_json
            },
            "mcpTools": mcp_tools,
            "llmConfig": {
                "modelRef": cfg.effective_model,
                "provider": cfg.provider_kind,
                "apiBase": cfg.api_base,
                "apiKey": cfg.api_key,
                "model": cfg.model_name,
                "litellmModel": cfg.litellm_model,
                "maxTokens": cfg.max_tokens,
                "thinking": {
                    "mode": cfg.thinking_mode,
                    "budget": cfg.thinking_budget
                }
            },
            "fallbackLlmConfigs": cfg.fallback_llm_configs,
            "taskLlmConfigs": cfg.task_llm_configs,
            "sessionTitleRequest": session_title_request,
            "settings": {
                "user_context_limit": cfg.session_context_limit,
                "compress_threshold": cfg.compress_threshold
            },
            "currentDateTime": chrono::Local::now().to_rfc3339(),
            "recentMessages": history_json,
            "summary": if is_regeneration { None } else { cfg.session_summary.clone() },
            "explicitMemories": {
                "user_md": user_md,
                "memory_md": memory_md
            },
            "retrievedMemories": [],
            "retrievedKnowledge": retrieved_knowledge,
            "readingContext": reading_book.as_ref().map(reading_prompt_context),
            "workspace": workspace_context,
            "projectContext": [],
            "attachmentsContext": attachments_context
        }
    });

    // 显式注册 run_id → assistant_msg_id，供 ws_server 精确定位 pending 消息
    state
        .agent
        .register_run(run_id.clone(), assistant_msg_id.to_string());
    // 记录 session_id → run_id，供 cancel_run 按会话取消
    state
        .agent
        .set_session_run(session_id.to_string(), run_id.clone());

    let run_req = Envelope {
        protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        run_id: run_id.clone(),
        session_id: session_id.to_string(),
        msg_type: msg_type::RUN_REQUEST.to_string(),
        created_at: String::new(),
        payload: context_snapshot,
    };

    if let Err(error) = state.agent.send_to_agent(run_req) {
        let _ = state.agent.remove_run(&run_id);
        let _ = state.agent.remove_session_run(session_id);
        let _ = state
            .db
            .fail_pending_assistant(
                assistant_msg_id.to_string(),
                format!("（无法启动本次回复：{error}）"),
            )
            .await;
        return Err(error);
    }
    eprintln!("[agent][run] RUN_REQUEST queued run={run_id}");
    Ok(())
}

async fn start_agent_run_or_mark_failed(
    state: &AppState,
    cfg: &ResolvedLlm,
    session_id: &str,
    assistant_msg_id: &str,
    is_regeneration: bool,
    include_reading_context: bool,
    session_title_request: Option<serde_json::Value>,
) -> AppResult<()> {
    let result = start_agent_run(
        state,
        cfg,
        session_id,
        assistant_msg_id,
        is_regeneration,
        include_reading_context,
        session_title_request,
    )
    .await;
    if let Err(error) = &result {
        let _ = state
            .db
            .fail_pending_assistant(
                assistant_msg_id.to_string(),
                format!("（无法启动本次回复：{error}）"),
            )
            .await;
    }
    result
}

/// 发送消息给 Agent，启动推理引擎运行（Tauri 主入口）。
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    session_id: String,
    text: String,
    reading_book_id: Option<String>,
    attachments: Vec<ChatAttachmentInput>,
) -> AppResult<()> {
    let cfg = resolve_llm(&state, &session_id).await?;
    let mut user_parts = vec![text_user_message_part(text.clone())];
    user_parts.extend(prepare_chat_attachment_parts(&state, &cfg.agent.id, attachments).await?);

    // 取当前活动叶子作为新 user 消息的 parent（首条消息时为 None）
    let path = state.db.list_active_with_parts(session_id.clone()).await?;
    let leaf_id = path.messages.last().map(|am| am.message.id.clone());
    let title_source = if path.messages.is_empty() {
        Some(text.clone())
    } else {
        path.messages
            .iter()
            .find(|message| message.message.role == "user")
            .map(|message| {
                message
                    .parts
                    .iter()
                    .filter(|part| part.kind == "text" && !part.content.trim().is_empty())
                    .map(|part| part.content.trim())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|source| !source.is_empty())
    };
    let session_title_request = if reading_book_id.is_none() && title_source.is_some() {
        let session = state.db.get_session(session_id.clone()).await?;
        if session
            .as_ref()
            .is_some_and(|session| is_automatic_session_title(&session.title))
        {
            let source_text = title_source.expect("title source was checked above");
            let fallback_title = fallback_session_title(&source_text);
            state
                .db
                .update_session_title(session_id.clone(), fallback_title.clone())
                .await?;
            Some(json!({
                "sourceText": source_text,
                "fallbackTitle": fallback_title,
            }))
        } else {
            None
        }
    } else {
        None
    };

    // 原子插入 user + pending assistant 并链接版本树
    let (_user_id, assistant_msg_id) = state
        .db
        .append_user_and_assistant(
            session_id.clone(),
            leaf_id,
            user_parts,
            cfg.model_name.clone(),
        )
        .await?;
    state.sync.schedule();

    start_agent_run_or_mark_failed(
        &state,
        &cfg,
        &session_id,
        &assistant_msg_id,
        false,
        reading_book_id.is_some(),
        session_title_request,
    )
    .await
}

/// 批准或拒绝挂起的工具调用。由 React 用户点击卡片同意/拒绝时触发。
#[tauri::command]
pub async fn approve_tool(
    state: tauri::State<'_, AppState>,
    tool_call_id: String,
    approved: bool,
) -> AppResult<()> {
    let resolved = state.agent.resolve_approval(&tool_call_id, approved);
    if resolved {
        state.notifications.resolve_approval(&tool_call_id).await?;
        Ok(())
    } else {
        Err(AppError::Other(format!(
            "审批 ID `{tool_call_id}` 未找到或已失效"
        )))
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ExplicitMemoriesDto {
    pub user_md: String,
    pub memory_md: String,
}

#[tauri::command]
pub async fn get_explicit_memories(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<ExplicitMemoriesDto> {
    let (user_md, memory_md) = crate::memory::load_explicit_memories(&state.db, &agent_id).await?;
    Ok(ExplicitMemoriesDto { user_md, memory_md })
}

#[tauri::command]
pub async fn save_explicit_memories(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    user_md: String,
    memory_md: String,
) -> AppResult<()> {
    crate::memory::save_explicit_memories(&state.db, &agent_id, &user_md, &memory_md).await?;
    state.sync.schedule();
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUserProfileInheritanceDto {
    pub mode: crate::user_profile::UserProfileInheritanceMode,
    pub effective: bool,
    pub user_md_empty: bool,
}

async fn agent_user_profile_inheritance_dto(
    state: &AppState,
    agent_id: &str,
) -> AppResult<AgentUserProfileInheritanceDto> {
    let (user_md, _) = crate::memory::load_explicit_memories(&state.db, agent_id).await?;
    let mode = crate::user_profile::load_agent_inheritance_mode(&state.db, agent_id).await?;
    Ok(AgentUserProfileInheritanceDto {
        mode,
        effective: crate::user_profile::should_inherit(mode, &user_md),
        user_md_empty: user_md.trim().is_empty(),
    })
}

#[tauri::command]
pub async fn get_user_profile(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::user_profile::UserProfile> {
    crate::user_profile::load_user_profile(&state.db).await
}

#[tauri::command]
pub async fn save_user_profile(
    state: tauri::State<'_, AppState>,
    profile: crate::user_profile::UserProfile,
) -> AppResult<()> {
    crate::user_profile::save_user_profile(&state.db, &profile).await?;
    state.sync.schedule();
    Ok(())
}

#[tauri::command]
pub async fn get_agent_user_profile_inheritance(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<AgentUserProfileInheritanceDto> {
    agent_user_profile_inheritance_dto(&state, &agent_id).await
}

#[tauri::command]
pub async fn set_agent_user_profile_inheritance(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    mode: crate::user_profile::UserProfileInheritanceMode,
) -> AppResult<AgentUserProfileInheritanceDto> {
    crate::user_profile::save_agent_inheritance_mode(&state.db, &agent_id, mode).await?;
    state.sync.schedule();
    agent_user_profile_inheritance_dto(&state, &agent_id).await
}

#[tauri::command]
pub async fn list_memories(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<Vec<crate::db::repo::memory::MemoryRow>> {
    state.db.list_memories(agent_id).await
}

#[derive(Serialize)]
pub struct MemoryVectorizationResult {
    pub indexed_now: usize,
    pub status: crate::embeddings::MemoryIndexStatus,
}

#[tauri::command]
pub async fn get_memory_embedding_status(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<crate::embeddings::MemoryIndexStatus> {
    let roles = load_model_roles(&state.db).await?;
    crate::embeddings::agent_memory_index_status(
        &state.db,
        &agent_id,
        roles.embedding_model.as_deref(),
    )
    .await
}

#[tauri::command]
pub async fn vectorize_memories(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<MemoryVectorizationResult> {
    let roles = load_model_roles(&state.db).await?;
    let model_ref = roles
        .embedding_model
        .as_deref()
        .ok_or_else(|| AppError::Other("尚未配置嵌入模型".into()))?;
    let config = resolve_routed_llm_config(&state, Some(model_ref))
        .await?
        .ok_or_else(|| AppError::Other("嵌入模型配置不可用".into()))?;
    let indexed_now =
        crate::embeddings::ensure_agent_memory_index(&state.db, &state.agent, &agent_id, &config)
            .await?;
    let status =
        crate::embeddings::agent_memory_index_status(&state.db, &agent_id, Some(model_ref)).await?;
    Ok(MemoryVectorizationResult {
        indexed_now,
        status,
    })
}

#[tauri::command]
pub async fn create_memory(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    name: String,
    keywords: Vec<String>,
    content: String,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let inserted = state
        .db
        .insert_memory(crate::db::repo::memory::NewMemory {
            id: id.clone(),
            agent_id,
            name,
            keywords,
            content,
            creator: "user".into(),
            memory_type: "Note".into(),
            scope: "agent".into(),
            source: "memory_manager".into(),
            confidence: 1.0,
            embedding_id: None,
        })
        .await?;
    if !inserted {
        return Err(AppError::Other("已存在名称和内容完全相同的记忆".into()));
    }
    state.sync.schedule();
    Ok(id)
}

#[tauri::command]
pub async fn update_memory(
    state: tauri::State<'_, AppState>,
    memory_id: String,
    agent_id: String,
    name: String,
    keywords: Vec<String>,
    content: String,
) -> AppResult<()> {
    state
        .db
        .update_memory(
            memory_id,
            agent_id,
            crate::db::repo::memory::MemoryUpdate {
                name,
                keywords,
                content,
            },
        )
        .await?;
    state.sync.schedule();
    Ok(())
}

#[tauri::command]
pub async fn delete_memory(
    state: tauri::State<'_, AppState>,
    memory_id: String,
    agent_id: String,
) -> AppResult<()> {
    state.db.delete_memory(memory_id, agent_id).await?;
    state.sync.schedule();
    Ok(())
}

const MAX_LOCAL_KNOWLEDGE_DOCUMENT_BYTES: u64 = 50 * 1024 * 1024;
const MAX_LOCAL_CHAT_ATTACHMENT_BYTES: u64 = 512 * 1024;
const MAX_CHAT_ATTACHMENT_TOTAL_BYTES: u64 = 1024 * 1024;
const MAX_CHAT_ATTACHMENTS: usize = 8;
const KNOWLEDGE_CHUNK_SIZE: usize = 1200;
const KNOWLEDGE_CHUNK_OVERLAP: usize = 200;

fn text_media_type(path: &std::path::Path) -> AppResult<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("md" | "markdown") => Ok("text/markdown"),
        Some("txt" | "rst" | "log") => Ok("text/plain"),
        Some("csv") => Ok("text/csv"),
        Some("json") => Ok("application/json"),
        _ => Err(AppError::Other(
            "当前仅支持 UTF-8 的 Markdown、文本、CSV 和 JSON 文件".into(),
        )),
    }
}

fn read_local_chat_attachment(path: String) -> AppResult<(String, String, i64, String)> {
    let path = std::path::PathBuf::from(path);
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() {
        return Err(AppError::Other("附件路径必须是普通文件".into()));
    }
    if metadata.len() > MAX_LOCAL_CHAT_ATTACHMENT_BYTES {
        return Err(AppError::Other(format!(
            "单个附件不能超过 {} KiB；较大的资料请导入知识库后再附加",
            MAX_LOCAL_CHAT_ATTACHMENT_BYTES / 1024
        )));
    }
    let media_type = text_media_type(&path)?.to_string();
    let bytes = std::fs::read(&path)?;
    let content = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::Other("附件目前仅支持 UTF-8 编码的文本文件".into()))?
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    if content.contains('\0') {
        return Err(AppError::Other("附件包含 NUL 字符，已拒绝读取".into()));
    }
    if content.trim().is_empty() {
        return Err(AppError::Other("附件没有可读取的文本内容".into()));
    }
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "未命名附件".into());
    Ok((name, media_type, bytes.len() as i64, content))
}

fn text_user_message_part(text: String) -> NewUserMessagePart {
    NewUserMessagePart {
        kind: "text".into(),
        mime_type: Some("text/plain".into()),
        content: text,
        metadata: None,
    }
}

async fn prepare_chat_attachment_parts(
    state: &AppState,
    agent_id: &str,
    attachments: Vec<ChatAttachmentInput>,
) -> AppResult<Vec<NewUserMessagePart>> {
    if attachments.len() > MAX_CHAT_ATTACHMENTS {
        return Err(AppError::Other(format!(
            "每条消息最多添加 {MAX_CHAT_ATTACHMENTS} 个附件"
        )));
    }

    let mut ids = HashSet::new();
    let mut total_bytes = 0_u64;
    let mut parts = Vec::with_capacity(attachments.len());
    let mut visible_collections = None;
    let mut selected_collection = false;

    for attachment in attachments {
        if attachment.id.trim().is_empty() || !ids.insert(attachment.id.clone()) {
            return Err(AppError::Other("附件标识为空或重复".into()));
        }
        match attachment.kind.as_str() {
            "local_file" => {
                let path = attachment
                    .path
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| AppError::Other("本地文件附件缺少路径".into()))?;
                let stored_path = path.clone();
                let (name, media_type, size, content) =
                    tokio::task::spawn_blocking(move || read_local_chat_attachment(path))
                        .await
                        .map_err(|error| {
                            AppError::Other(format!("附件读取任务异常中止：{error}"))
                        })??;
                total_bytes = total_bytes.saturating_add(size as u64);
                if total_bytes > MAX_CHAT_ATTACHMENT_TOTAL_BYTES {
                    return Err(AppError::Other(format!(
                        "单条消息的本地附件总大小不能超过 {} MiB",
                        MAX_CHAT_ATTACHMENT_TOTAL_BYTES / 1024 / 1024
                    )));
                }
                parts.push(NewUserMessagePart {
                    kind: "attachment".into(),
                    mime_type: Some(media_type.clone()),
                    content,
                    metadata: Some(
                        json!({
                            "attachmentKind": "local_file",
                            "id": attachment.id,
                            "name": name,
                            "path": stored_path,
                            "mediaType": media_type,
                            "size": size,
                        })
                        .to_string(),
                    ),
                });
            }
            "knowledge_collection" => {
                if selected_collection {
                    return Err(AppError::Other("每条消息目前只能指定一个知识库".into()));
                }
                let collection_id = attachment
                    .collection_id
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| AppError::Other("知识库附件缺少集合标识".into()))?;
                if visible_collections.is_none() {
                    visible_collections = Some(
                        state
                            .db
                            .list_knowledge_collections(agent_id.to_string())
                            .await?,
                    );
                }
                let collection = visible_collections
                    .as_ref()
                    .and_then(|items| items.iter().find(|item| item.id == collection_id))
                    .ok_or_else(|| {
                        AppError::Other("指定知识库不存在或当前 Agent 无权访问".into())
                    })?;
                selected_collection = true;
                parts.push(NewUserMessagePart {
                    kind: "attachment".into(),
                    mime_type: Some("application/x-agnes-knowledge-collection".into()),
                    content: String::new(),
                    metadata: Some(
                        json!({
                            "attachmentKind": "knowledge_collection",
                            "id": attachment.id,
                            "name": collection.name,
                            "collectionId": collection.id,
                        })
                        .to_string(),
                    ),
                });
            }
            "skill" => {
                let skill_id = attachment
                    .skill_id
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| AppError::Other("Skill 附件缺少标识".into()))?;
                let skill = crate::skills::get_installed(&skill_id)?;
                if !skill.enabled {
                    return Err(AppError::Other(format!("Skill 已停用：{}", skill.name)));
                }
                parts.push(NewUserMessagePart {
                    kind: "attachment".into(),
                    mime_type: Some("application/x-agnes-skill".into()),
                    content: String::new(),
                    metadata: Some(
                        json!({
                            "attachmentKind": "skill",
                            "id": attachment.id,
                            "name": skill.name,
                            "skillId": skill.id,
                            "description": skill.description,
                            "version": skill.version,
                        })
                        .to_string(),
                    ),
                });
            }
            _ => return Err(AppError::Other("不支持的附件类型".into())),
        }
    }
    Ok(parts)
}

fn trim_to_char_boundary(value: &str, max_chars: usize) -> &str {
    match value.char_indices().nth(max_chars) {
        Some((index, _)) => &value[..index],
        None => value,
    }
}

fn trailing_chars(value: &str, max_chars: usize) -> String {
    let start = value
        .char_indices()
        .rev()
        .nth(max_chars)
        .map(|(index, _)| index)
        .unwrap_or(0);
    value[start..].to_string()
}

fn push_text_chunk(
    chunks: &mut Vec<crate::db::repo::knowledge::NewDocumentChunk>,
    content: String,
    section_path: &Option<String>,
) {
    let content = content.trim().to_string();
    if content.is_empty() {
        return;
    }
    chunks.push(crate::db::repo::knowledge::NewDocumentChunk {
        token_count: content.split_whitespace().count().max(1) as i64,
        content,
        page: None,
        section_path: section_path.clone(),
        metadata: r#"{"kind":"text"}"#.into(),
    });
}

fn chunk_local_text(content: &str) -> Vec<crate::db::repo::knowledge::NewDocumentChunk> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut section_path: Option<String> = None;

    for paragraph in content.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if let Some(heading) = paragraph
            .lines()
            .next()
            .and_then(|line| line.strip_prefix('#'))
            .map(str::trim)
            .filter(|heading| !heading.is_empty())
        {
            section_path = Some(heading.to_string());
        }

        let mut remainder = paragraph;
        while !remainder.is_empty() {
            let separator_len = usize::from(!current.is_empty());
            let available =
                KNOWLEDGE_CHUNK_SIZE.saturating_sub(current.chars().count() + separator_len);
            if available == 0 {
                let emitted = std::mem::take(&mut current);
                current = trailing_chars(&emitted, KNOWLEDGE_CHUNK_OVERLAP);
                push_text_chunk(&mut chunks, emitted, &section_path);
                continue;
            }

            let head = trim_to_char_boundary(remainder, available);
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(head);
            remainder = &remainder[head.len()..];
            if !remainder.is_empty() {
                let emitted = std::mem::take(&mut current);
                current = trailing_chars(&emitted, KNOWLEDGE_CHUNK_OVERLAP);
                push_text_chunk(&mut chunks, emitted, &section_path);
            }
        }
    }

    push_text_chunk(&mut chunks, current, &section_path);
    chunks
}

struct ParsedKnowledgeDocument {
    title: String,
    media_type: String,
    source_hash: String,
    size: i64,
    parser_profile: crate::db::repo::knowledge::DocumentParserProfile,
    chunks: Vec<crate::db::repo::knowledge::NewDocumentChunk>,
}

fn knowledge_media_type(
    path: &std::path::Path,
    title_hint: Option<&str>,
    remote_media_type: Option<&str>,
) -> AppResult<String> {
    let normalized = remote_media_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let media_type = match normalized {
        Some(
            "application/vnd.google-apps.document" | "application/vnd.google-apps.presentation",
        ) => Some("text/plain"),
        Some("application/vnd.google-apps.spreadsheet") => Some("text/csv"),
        Some("application/vnd.google-apps.script" | "application/vnd.google-apps.script+json") => {
            Some("application/json")
        }
        Some("text/markdown" | "text/plain" | "text/csv" | "application/json") => normalized,
        Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/pdf",
        ) => normalized,
        Some("application/octet-stream") | None => None,
        Some(_) => None,
    };
    media_type
        .map(str::to_string)
        .or_else(|| {
            let extension_path = title_hint.map(std::path::Path::new).unwrap_or(path);
            match extension_path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase())
                .as_deref()
            {
                Some("docx") => Some(
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                        .into(),
                ),
                Some("pptx") => Some(
                    "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                        .into(),
                ),
                Some("xlsx") => {
                    Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into())
                }
                Some("pdf") => Some("application/pdf".into()),
                _ => text_media_type(extension_path).ok().map(str::to_string),
            }
        })
        .ok_or_else(|| {
            AppError::Other(
                "当前支持 UTF-8 文本、PDF、DOCX、PPTX、XLSX 和可导出的 Google 文档".into(),
            )
        })
}

fn read_text_knowledge_document(
    path: String,
    title_hint: Option<String>,
    remote_media_type: Option<String>,
) -> AppResult<ParsedKnowledgeDocument> {
    let path = std::path::PathBuf::from(path);
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() {
        return Err(AppError::Other("知识库导入路径必须是普通文件".into()));
    }
    if metadata.len() > MAX_LOCAL_KNOWLEDGE_DOCUMENT_BYTES {
        return Err(AppError::Other(format!(
            "文件超过 {} MiB 的本地导入上限",
            MAX_LOCAL_KNOWLEDGE_DOCUMENT_BYTES / 1024 / 1024
        )));
    }
    let media_type =
        knowledge_media_type(&path, title_hint.as_deref(), remote_media_type.as_deref())?;
    let bytes = std::fs::read(&path)?;
    let content = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::Other("仅支持 UTF-8 编码的文本文件".into()))?
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    if content.contains('\0') {
        return Err(AppError::Other("文本文件包含 NUL 字符，已拒绝导入".into()));
    }
    let chunks = chunk_local_text(&content);
    if chunks.is_empty() {
        return Err(AppError::Other("文件没有可索引的文本内容".into()));
    }
    let title = title_hint
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            path.file_stem()
                .map(|name| name.to_string_lossy().trim().to_string())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| "未命名文档".into());
    let hash = format!("{:x}", Sha256::digest(&bytes));
    Ok(ParsedKnowledgeDocument {
        title,
        media_type,
        source_hash: hash,
        size: bytes.len() as i64,
        parser_profile: crate::db::repo::knowledge::DocumentParserProfile::builtin_text(),
        chunks,
    })
}

async fn read_knowledge_document(
    state: &AppState,
    path: String,
    title_hint: Option<String>,
    remote_media_type: Option<String>,
    context: Option<&crate::document_parser::DocumentImportContext>,
    cancellation: tokio::sync::watch::Receiver<bool>,
) -> AppResult<ParsedKnowledgeDocument> {
    crate::document_parser::ensure_not_cancelled(&cancellation)?;
    let path_buf = std::path::PathBuf::from(&path);
    let media_type = knowledge_media_type(
        &path_buf,
        title_hint.as_deref(),
        remote_media_type.as_deref(),
    )?;
    let is_structured = matches!(
        media_type.as_str(),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/pdf"
    );
    if is_structured {
        let pdf_runtime = if media_type == "application/pdf" {
            #[cfg(debug_assertions)]
            {
                state.pdf_models.runtime().ok()
            }
            #[cfg(not(debug_assertions))]
            {
                Some(state.pdf_models.runtime()?)
            }
        } else {
            None
        };
        let parsed = crate::document_parser::parse_structured_document(
            &state.app_handle,
            &path_buf,
            title_hint.as_deref(),
            &media_type,
            pdf_runtime.as_ref(),
            context,
            cancellation,
        )
        .await?;
        let parser_profile = parsed.profile();
        let title = parsed.title.clone();
        let parsed_media_type = parsed.media_type.clone();
        let source_hash = parsed.source_hash.clone();
        let size = parsed.size;
        let chunks = parsed.into_chunks()?;
        return Ok(ParsedKnowledgeDocument {
            title,
            media_type: parsed_media_type,
            source_hash,
            size,
            parser_profile,
            chunks,
        });
    }

    crate::document_parser::emit_import_progress(
        &state.app_handle,
        context,
        "reading",
        35,
        "正在读取文本内容",
    );
    let parsed = tokio::task::spawn_blocking(move || {
        read_text_knowledge_document(path, title_hint, remote_media_type)
    })
    .await
    .map_err(|error| AppError::Other(format!("知识库文本解析任务异常中止：{error}")))??;
    crate::document_parser::ensure_not_cancelled(&cancellation)?;
    crate::document_parser::emit_import_progress(
        &state.app_handle,
        context,
        "chunking",
        80,
        "文本分块已生成",
    );
    Ok(parsed)
}

fn knowledge_storage_path(
    data_dir: &std::path::Path,
    account_id: &str,
    file_id: &str,
) -> std::path::PathBuf {
    let key = format!("{account_id}\0{file_id}");
    let digest = Sha256::digest(key.as_bytes());
    data_dir
        .join("knowledge-sources")
        .join(format!("{digest:x}.source"))
}

struct ManagedFileInstall {
    target: std::path::PathBuf,
    backup: Option<std::path::PathBuf>,
}

fn install_managed_file(
    source: &std::path::Path,
    target: &std::path::Path,
) -> AppResult<ManagedFileInstall> {
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Other("Invalid managed source path".into()))?;
    std::fs::create_dir_all(parent)?;
    let backup = if target.exists() {
        let backup = parent.join(format!(".agnes-source-{}.backup", uuid::Uuid::new_v4()));
        std::fs::rename(target, &backup)?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = std::fs::rename(source, target) {
        if let Some(backup) = backup.as_ref() {
            let _ = std::fs::rename(backup, target);
        }
        return Err(error.into());
    }
    Ok(ManagedFileInstall {
        target: target.to_path_buf(),
        backup,
    })
}

fn commit_managed_file(install: ManagedFileInstall) {
    if let Some(backup) = install.backup {
        let _ = std::fs::remove_file(backup);
    }
}

fn rollback_managed_file(install: ManagedFileInstall) {
    let _ = std::fs::remove_file(&install.target);
    if let Some(backup) = install.backup {
        let _ = std::fs::rename(backup, install.target);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingHighlightPayload {
    pub cfi_range: String,
    pub quote: String,
    #[serde(default)]
    pub context_before: String,
    #[serde(default)]
    pub context_after: String,
    pub note: Option<String>,
    pub color: Option<String>,
}

fn reading_prompt_context(book: &crate::db::repo::reading::ReadingBookRow) -> serde_json::Value {
    json!({
        "bookId": book.id,
        "title": book.title,
        "author": book.author,
        "modelKnowsContent": book.model_knows_content,
        "contentContextAllowed": book.content_context_allowed,
    })
}

fn reading_book_storage_path(data_dir: &std::path::Path, book_id: &str) -> std::path::PathBuf {
    data_dir
        .join("reading-books")
        .join(format!("{book_id}.epub"))
}

fn persist_reading_epub(
    data_dir: std::path::PathBuf,
    book_id: String,
    bytes: Vec<u8>,
) -> AppResult<std::path::PathBuf> {
    let target = reading_book_storage_path(&data_dir, &book_id);
    let parent = target
        .parent()
        .ok_or_else(|| AppError::Other("Invalid reading storage path".into()))?;
    std::fs::create_dir_all(parent)?;
    let temporary = target.with_extension("epub.partial");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &target)?;
    Ok(target)
}

#[tauri::command]
pub async fn list_reading_books(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::db::repo::reading::ReadingBookRow>> {
    state.db.list_reading_books().await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingEpubPublishResult {
    pub artifact_id: String,
    pub reused: bool,
    pub ready_replica_count: usize,
}

#[tauri::command]
pub async fn publish_reading_epub(
    state: tauri::State<'_, AppState>,
    book_id: String,
) -> AppResult<ReadingEpubPublishResult> {
    let outcome = state
        .sync
        .publish_reading_epub(
            state.storage.clone(),
            book_id,
            state.data_dir.join("artifacts").join("outbox"),
        )
        .await?;
    if let Err(error) =
        crate::storage::artifact_cache::enforce_quota(&state.db, state.data_dir.join("artifacts"))
            .await
    {
        eprintln!("[artifact-gc] post-publish cleanup failed: {error}");
    }
    Ok(ReadingEpubPublishResult {
        artifact_id: outcome.artifact_id,
        reused: outcome.reused,
        ready_replica_count: outcome.ready_replica_count,
    })
}

#[tauri::command]
pub async fn import_reading_book(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    path: String,
) -> AppResult<crate::db::repo::reading::ReadingBookRow> {
    import_reading_book_from_path(&state, agent_id, path).await
}

async fn import_reading_book_from_path(
    state: &AppState,
    agent_id: String,
    path: String,
) -> AppResult<crate::db::repo::reading::ReadingBookRow> {
    let path_for_parse = path.clone();
    let (bytes, source_hash, parsed) = tokio::task::spawn_blocking(move || {
        let source = std::path::PathBuf::from(path_for_parse);
        let metadata = std::fs::metadata(&source)?;
        if !metadata.is_file() {
            return Err(AppError::Other(
                "EPUB import path must be a regular file".into(),
            ));
        }
        if metadata.len() > crate::reading::MAX_EPUB_ARCHIVE_BYTES {
            return Err(AppError::Other(format!(
                "EPUB exceeds the {} MiB import limit",
                crate::reading::MAX_EPUB_ARCHIVE_BYTES / 1024 / 1024
            )));
        }
        let bytes = std::fs::read(&source)?;
        let parsed = crate::reading::parse_epub_bytes(&bytes)?;
        let source_hash = format!("{:x}", Sha256::digest(&bytes));
        Ok::<_, AppError>((bytes, source_hash, parsed))
    })
    .await
    .map_err(|error| AppError::Other(format!("EPUB import task aborted: {error}")))??;

    if let Some(existing) = state
        .db
        .find_reading_book_by_source_hash(source_hash.clone())
        .await?
    {
        if existing
            .local_path
            .as_deref()
            .is_some_and(|path| std::path::Path::new(path).is_file())
        {
            return Ok(existing);
        }
        let known_agent = state
            .db
            .list_agents()
            .await?
            .into_iter()
            .any(|agent| agent.id == agent_id);
        if !known_agent {
            return Err(AppError::Other("Agent not found".into()));
        }
        let stored_path = tokio::task::spawn_blocking({
            let data_dir = state.data_dir.clone();
            let book_id = existing.id.clone();
            move || persist_reading_epub(data_dir, book_id, bytes)
        })
        .await
        .map_err(|error| AppError::Other(format!("EPUB storage task aborted: {error}")))??;
        state
            .db
            .bind_local_reading_epub(
                existing.id.clone(),
                source_hash,
                stored_path.to_string_lossy().to_string(),
            )
            .await?;
        if let Err(error) = ensure_local_reading_index(state, &existing.id, &agent_id).await {
            let _ = std::fs::remove_file(&stored_path);
            return Err(error);
        }
        return state
            .db
            .get_reading_book(existing.id)
            .await?
            .ok_or_else(|| AppError::Other("Reading book disappeared after import".into()));
    }

    let known_agent = state
        .db
        .list_agents()
        .await?
        .into_iter()
        .any(|agent| agent.id == agent_id);
    if !known_agent {
        return Err(AppError::Other("Agent not found".into()));
    }

    let book_id = uuid::Uuid::new_v4().to_string();
    let stored_path = tokio::task::spawn_blocking({
        let data_dir = state.data_dir.clone();
        let book_id = book_id.clone();
        move || persist_reading_epub(data_dir, book_id, bytes)
    })
    .await
    .map_err(|error| AppError::Other(format!("EPUB storage task aborted: {error}")))??;

    let collection_id = uuid::Uuid::new_v4().to_string();
    let collection_name = format!("Read With AI · {}", parsed.title.trim());
    let import_result = async {
        state
            .db
            .create_knowledge_collection(crate::db::repo::knowledge::NewKnowledgeCollection {
                id: collection_id.clone(),
                name: collection_name,
                scope: "custom".into(),
                agent_id: agent_id.clone(),
            })
            .await?;

        let mut chunks = Vec::new();
        for chapter in &parsed.chapters {
            for mut chunk in chunk_local_text(&chapter.text) {
                chunk.section_path = Some(chapter.title.clone());
                chunks.push(chunk);
            }
        }
        let document = state
            .db
            .import_local_knowledge_document(crate::db::repo::knowledge::NewLocalDocument {
                collection_id: collection_id.clone(),
                agent_id: agent_id.clone(),
                title: parsed.title.clone(),
                media_type: "application/epub+zip".into(),
                local_path: stored_path.to_string_lossy().to_string(),
                plaintext_hash: source_hash.clone(),
                size: std::fs::metadata(&stored_path)?.len() as i64,
                parser_profile: crate::db::repo::knowledge::DocumentParserProfile::builtin_text(),
                chunks,
            })
            .await?;
        state
            .db
            .insert_reading_book(crate::db::repo::reading::NewReadingBook {
                id: book_id.clone(),
                collection_id,
                document_id: document.document_id,
                local_path: stored_path.to_string_lossy().to_string(),
                title: parsed.title,
                author: parsed.author,
                source_hash,
            })
            .await
    }
    .await;
    if import_result.is_err() {
        let _ = std::fs::remove_file(&stored_path);
    }
    import_result
}

#[tauri::command]
pub async fn import_storage_reading_book(
    state: tauri::State<'_, AppState>,
    account_id: String,
    file_id: String,
    file_name: String,
    file_media_type: Option<String>,
    expected_revision: Option<String>,
    expected_size: Option<u64>,
    agent_id: String,
) -> AppResult<crate::db::repo::reading::ReadingBookRow> {
    let staged = state
        .storage
        .stage_file_import(
            account_id,
            file_id,
            file_name,
            file_media_type,
            expected_revision,
            expected_size,
            state.data_dir.join("storage-imports"),
            crate::storage::StorageImportKind::Reading,
            agent_id.clone(),
            crate::reading::MAX_EPUB_ARCHIVE_BYTES,
        )
        .await?;
    let result = import_reading_book_from_path(
        &state,
        agent_id,
        staged.local_path.to_string_lossy().to_string(),
    )
    .await;
    let finish_error = result.as_ref().err();
    let finish_result = state
        .storage
        .finish_file_import(&staged, finish_error)
        .await;
    let _ = tokio::fs::remove_file(&staged.local_path).await;
    match result {
        Ok(book) => {
            finish_result?;
            Ok(book)
        }
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub async fn open_reading_book_conversation(
    state: tauri::State<'_, AppState>,
    book_id: String,
    agent_id: String,
) -> AppResult<String> {
    if let Some(session_id) = state
        .db
        .get_reading_conversation_session(book_id.clone(), agent_id.clone())
        .await?
    {
        return Ok(session_id);
    }
    state
        .db
        .grant_reading_book_agent_access(book_id.clone(), agent_id.clone())
        .await?;
    let book = state
        .db
        .list_reading_books()
        .await?
        .into_iter()
        .find(|book| book.id == book_id)
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    let default_max_output_tokens = load_default_max_output_tokens(&state).await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .insert_session(NewSession {
            id: session_id.clone(),
            agent_id: agent_id.clone(),
            title: format!("阅读 · {}", book.title),
            context_limit: None,
            compress_threshold: None,
            recency_window: None,
            reserved_output_tokens: Some(default_max_output_tokens),
            summarizer_model: None,
            model: None,
            thinking_mode: None,
            thinking_budget: None,
            permission_mode: "auto".into(),
            workspace_id: None,
            origin_device_id: None,
        })
        .await?;
    if let Err(error) = state
        .db
        .create_reading_conversation(book_id, agent_id, session_id.clone())
        .await
    {
        let _ = state.db.delete_session(session_id.clone()).await;
        return Err(error);
    }
    Ok(session_id)
}

#[tauri::command]
pub async fn list_reading_book_conversations(
    state: tauri::State<'_, AppState>,
    book_id: String,
    agent_id: String,
) -> AppResult<Vec<crate::db::repo::reading::ReadingConversationRow>> {
    state.db.list_reading_conversations(book_id, agent_id).await
}

#[tauri::command]
pub async fn select_reading_book_conversation(
    state: tauri::State<'_, AppState>,
    book_id: String,
    agent_id: String,
    session_id: String,
) -> AppResult<()> {
    state
        .db
        .select_reading_conversation(book_id, agent_id, session_id)
        .await
}

/// Create a fresh discussion session for a book while keeping the previous
/// session available in the normal session list.
#[tauri::command]
pub async fn new_reading_book_conversation(
    state: tauri::State<'_, AppState>,
    book_id: String,
    agent_id: String,
) -> AppResult<String> {
    ensure_local_reading_index(&state, &book_id, &agent_id).await?;
    state
        .db
        .grant_reading_book_agent_access(book_id.clone(), agent_id.clone())
        .await?;
    let book = state
        .db
        .list_reading_books()
        .await?
        .into_iter()
        .find(|book| book.id == book_id)
        .ok_or_else(|| AppError::Other("Reading book not found".into()))?;
    let default_max_output_tokens = load_default_max_output_tokens(&state).await?;
    let session_id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .insert_session(NewSession {
            id: session_id.clone(),
            agent_id: agent_id.clone(),
            title: format!("阅读 · {}", book.title),
            context_limit: None,
            compress_threshold: None,
            recency_window: None,
            reserved_output_tokens: Some(default_max_output_tokens),
            summarizer_model: None,
            model: None,
            thinking_mode: None,
            thinking_budget: None,
            permission_mode: "auto".into(),
            workspace_id: None,
            origin_device_id: None,
        })
        .await?;
    if let Err(error) = state
        .db
        .create_reading_conversation(book_id, agent_id, session_id.clone())
        .await
    {
        let _ = state.db.delete_session(session_id.clone()).await;
        return Err(error);
    }
    Ok(session_id)
}

async fn ensure_local_reading_index(
    state: &AppState,
    book_id: &str,
    agent_id: &str,
) -> AppResult<()> {
    let book = state
        .db
        .get_reading_book(book_id.to_string())
        .await?
        .ok_or_else(|| AppError::Other("书籍不存在".into()))?;
    if book.collection_id.is_some() && book.document_id.is_some() {
        return Ok(());
    }
    let local_path = book
        .local_path
        .as_deref()
        .filter(|path| std::path::Path::new(path).is_file())
        .ok_or_else(|| AppError::Other("请先等待 EPUB 下载并安装到本机".into()))?;
    let local_path_for_parse = local_path.to_string();
    let parsed = tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(local_path_for_parse)?;
        crate::reading::parse_epub_bytes(&bytes)
    })
    .await
    .map_err(|error| AppError::Other(format!("EPUB 索引任务异常中止：{error}")))??;
    let collection_id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .create_knowledge_collection(crate::db::repo::knowledge::NewKnowledgeCollection {
            id: collection_id.clone(),
            name: format!("Read With AI · {}", book.title.trim()),
            scope: "custom".into(),
            agent_id: agent_id.into(),
        })
        .await?;
    let mut chunks = Vec::new();
    for chapter in &parsed.chapters {
        for mut chunk in chunk_local_text(&chapter.text) {
            chunk.section_path = Some(chapter.title.clone());
            chunks.push(chunk);
        }
    }
    let document = state
        .db
        .import_local_knowledge_document(crate::db::repo::knowledge::NewLocalDocument {
            collection_id: collection_id.clone(),
            agent_id: agent_id.into(),
            title: book.title,
            media_type: "application/epub+zip".into(),
            local_path: local_path.into(),
            plaintext_hash: book.source_hash,
            size: std::fs::metadata(local_path)?.len() as i64,
            parser_profile: crate::db::repo::knowledge::DocumentParserProfile::builtin_text(),
            chunks,
        })
        .await?;
    state
        .db
        .bind_local_reading_index(book_id.into(), collection_id, document.document_id)
        .await
}

#[tauri::command]
pub async fn update_reading_book_mode(
    state: tauri::State<'_, AppState>,
    book_id: String,
    model_knows_content: bool,
) -> AppResult<crate::db::repo::reading::ReadingBookRow> {
    state
        .db
        .update_reading_book_mode(book_id, model_knows_content)
        .await
}

#[tauri::command]
pub async fn set_reading_book_content_context_allowed(
    state: tauri::State<'_, AppState>,
    book_id: String,
    allowed: bool,
) -> AppResult<crate::db::repo::reading::ReadingBookRow> {
    state
        .db
        .set_reading_book_content_context_allowed(book_id, allowed)
        .await
}

#[tauri::command]
pub fn set_reading_context_menu_active(active: bool) {
    crate::reading_context_menu::set_active(active);
}

#[tauri::command]
pub async fn update_reading_book_progress(
    state: tauri::State<'_, AppState>,
    book_id: String,
    cfi: String,
) -> AppResult<()> {
    state.db.update_reading_book_progress(book_id, cfi).await
}

#[tauri::command]
pub async fn list_reading_highlights(
    state: tauri::State<'_, AppState>,
    book_id: String,
) -> AppResult<Vec<crate::db::repo::reading::ReadingHighlightRow>> {
    state.db.list_reading_highlights(book_id).await
}

#[tauri::command]
pub async fn create_reading_highlight(
    state: tauri::State<'_, AppState>,
    book_id: String,
    payload: ReadingHighlightPayload,
) -> AppResult<crate::db::repo::reading::ReadingHighlightRow> {
    state
        .db
        .insert_reading_highlight(crate::db::repo::reading::NewReadingHighlight {
            id: uuid::Uuid::new_v4().to_string(),
            book_id,
            cfi_range: payload.cfi_range,
            quote: payload.quote,
            context_before: payload.context_before,
            context_after: payload.context_after,
            note: payload.note,
            color: payload.color.unwrap_or_else(|| "yellow".into()),
        })
        .await
}

#[tauri::command]
pub async fn list_knowledge_collections(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<Vec<crate::db::repo::knowledge::KnowledgeCollectionRow>> {
    state.db.list_knowledge_collections(agent_id).await
}

#[tauri::command]
pub async fn create_knowledge_collection(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    name: String,
    scope: String,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .create_knowledge_collection(crate::db::repo::knowledge::NewKnowledgeCollection {
            id: id.clone(),
            name,
            scope,
            agent_id,
        })
        .await?;
    Ok(id)
}

#[tauri::command]
pub async fn list_knowledge_documents(
    state: tauri::State<'_, AppState>,
    collection_id: String,
    agent_id: String,
) -> AppResult<Vec<crate::db::repo::knowledge::KnowledgeDocumentRow>> {
    state
        .db
        .list_knowledge_documents(collection_id, agent_id)
        .await
}

#[tauri::command]
pub async fn import_local_knowledge_document(
    state: tauri::State<'_, AppState>,
    collection_id: String,
    agent_id: String,
    path: String,
    job_id: Option<String>,
) -> AppResult<crate::db::repo::knowledge::ImportDocumentResult> {
    let context = if let Some(job_id) = job_id.as_deref() {
        uuid::Uuid::parse_str(job_id)
            .map_err(|_| AppError::Other("文档导入任务 ID 无效".into()))?;
        let file_name = std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "未命名文档".into());
        Some(crate::document_parser::DocumentImportContext {
            job_id: job_id.to_string(),
            file_name,
        })
    } else {
        None
    };
    let cancellation = if let Some(context) = context.as_ref() {
        state.document_parser.register(&context.job_id)?
    } else {
        crate::document_parser::detached_cancellation()
    };
    crate::document_parser::emit_import_progress(
        &state.app_handle,
        context.as_ref(),
        "validating",
        10,
        "正在检查导入文件",
    );
    let outcome = async {
        let parsed = read_knowledge_document(
            &state,
            path.clone(),
            None,
            None,
            context.as_ref(),
            cancellation.clone(),
        )
        .await?;
        crate::document_parser::ensure_not_cancelled(&cancellation)?;
        crate::document_parser::emit_import_progress(
            &state.app_handle,
            context.as_ref(),
            "storing",
            98,
            "正在写入知识库",
        );
        let result = state
            .db
            .import_local_knowledge_document(crate::db::repo::knowledge::NewLocalDocument {
                collection_id,
                agent_id,
                title: parsed.title,
                media_type: parsed.media_type,
                local_path: path,
                plaintext_hash: parsed.source_hash,
                size: parsed.size,
                parser_profile: parsed.parser_profile,
                chunks: parsed.chunks,
            })
            .await?;
        crate::document_parser::emit_import_progress(
            &state.app_handle,
            context.as_ref(),
            "completed",
            100,
            "文档导入完成",
        );
        Ok(result)
    }
    .await;
    if let Some(context) = context.as_ref() {
        state.document_parser.finish(&context.job_id);
    }
    outcome
}

#[tauri::command]
pub async fn cancel_knowledge_import(
    state: tauri::State<'_, AppState>,
    job_id: String,
) -> AppResult<()> {
    uuid::Uuid::parse_str(&job_id).map_err(|_| AppError::Other("文档导入任务 ID 无效".into()))?;
    state.document_parser.cancel(&job_id)
}

#[tauri::command]
pub async fn get_pdf_model_package_status(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::pdf_models::PdfModelPackageStatus> {
    Ok(state.pdf_models.status())
}

#[tauri::command]
pub async fn install_pdf_model_package(
    state: tauri::State<'_, AppState>,
    path: String,
) -> AppResult<crate::pdf_models::PdfModelPackageStatus> {
    let manager = state.pdf_models.clone();
    tokio::task::spawn_blocking(move || manager.install_archive(std::path::Path::new(&path)))
        .await
        .map_err(|error| AppError::Other(format!("PDF 模型包安装任务异常中止：{error}")))?
}

#[tauri::command]
pub async fn remove_pdf_model_package(state: tauri::State<'_, AppState>) -> AppResult<()> {
    let manager = state.pdf_models.clone();
    tokio::task::spawn_blocking(move || manager.remove())
        .await
        .map_err(|error| AppError::Other(format!("PDF 模型包移除任务异常中止：{error}")))?
}

#[tauri::command]
pub async fn import_storage_knowledge_document(
    state: tauri::State<'_, AppState>,
    account_id: String,
    file_id: String,
    file_name: String,
    file_media_type: Option<String>,
    expected_revision: Option<String>,
    expected_size: Option<u64>,
    collection_id: String,
    agent_id: String,
) -> AppResult<crate::db::repo::knowledge::ImportDocumentResult> {
    let stable_path = knowledge_storage_path(&state.data_dir, &account_id, &file_id);
    let staged = state
        .storage
        .stage_file_import(
            account_id,
            file_id,
            file_name,
            file_media_type,
            expected_revision,
            expected_size,
            state.data_dir.join("storage-imports"),
            crate::storage::StorageImportKind::Knowledge,
            collection_id.clone(),
            MAX_LOCAL_KNOWLEDGE_DOCUMENT_BYTES,
        )
        .await?;
    let parse_path = staged.local_path.to_string_lossy().to_string();
    let remote_name = staged.remote_file.name.clone();
    let remote_media_type = staged.remote_file.media_type.clone();
    let outcome = async {
        let parsed = read_knowledge_document(
            &state,
            parse_path,
            Some(remote_name),
            remote_media_type,
            None,
            crate::document_parser::detached_cancellation(),
        )
        .await?;
        let install = tokio::task::spawn_blocking({
            let source = staged.local_path.clone();
            let target = stable_path.clone();
            move || install_managed_file(&source, &target)
        })
        .await
        .map_err(|error| AppError::Other(format!("知识库源文件安装任务异常中止：{error}")))??;
        let result = state
            .db
            .import_local_knowledge_document(crate::db::repo::knowledge::NewLocalDocument {
                collection_id,
                agent_id,
                title: parsed.title,
                media_type: parsed.media_type,
                local_path: stable_path.to_string_lossy().to_string(),
                plaintext_hash: parsed.source_hash,
                size: parsed.size,
                parser_profile: parsed.parser_profile,
                chunks: parsed.chunks,
            })
            .await;
        match result {
            Ok(result) => {
                commit_managed_file(install);
                Ok(result)
            }
            Err(error) => {
                rollback_managed_file(install);
                Err(error)
            }
        }
    }
    .await;
    let finish_error = outcome.as_ref().err();
    let finish_result = state
        .storage
        .finish_file_import(&staged, finish_error)
        .await;
    let _ = tokio::fs::remove_file(&staged.local_path).await;
    match outcome {
        Ok(result) => {
            finish_result?;
            Ok(result)
        }
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub async fn search_knowledge(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    query: String,
    collection_id: Option<String>,
    limit: Option<usize>,
) -> AppResult<Vec<crate::db::repo::knowledge::KnowledgeSearchResult>> {
    state
        .db
        .search_knowledge(agent_id, query, collection_id, limit.unwrap_or(10))
        .await
}

#[derive(Serialize)]
pub struct KnowledgeVectorizationResult {
    pub indexed_now: usize,
    pub model_ref: String,
}

#[tauri::command]
pub async fn vectorize_knowledge(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    collection_id: Option<String>,
) -> AppResult<KnowledgeVectorizationResult> {
    let roles = load_model_roles(&state.db).await?;
    let model_ref = roles
        .embedding_model
        .as_deref()
        .ok_or_else(|| AppError::Other("尚未配置嵌入模型".into()))?;
    let config = resolve_routed_llm_config(&state, Some(model_ref))
        .await?
        .ok_or_else(|| AppError::Other("嵌入模型配置不可用".into()))?;
    let visible_collections = state.db.list_knowledge_collections(agent_id).await?;
    let collection_ids = match collection_id {
        Some(collection_id) => {
            if visible_collections
                .iter()
                .any(|collection| collection.id == collection_id)
            {
                vec![collection_id]
            } else {
                return Err(AppError::Other("知识库不可用".into()));
            }
        }
        None => visible_collections
            .into_iter()
            .map(|collection| collection.id)
            .collect(),
    };
    let mut indexed_now = 0;
    for collection_id in collection_ids {
        indexed_now += crate::embeddings::ensure_knowledge_index(
            &state.db,
            &state.agent,
            &config,
            Some(&collection_id),
        )
        .await?;
    }
    Ok(KnowledgeVectorizationResult {
        indexed_now,
        model_ref: model_ref.to_string(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeArtifactPublishResult {
    pub artifact_id: String,
    pub reused: bool,
    pub ready_replica_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeArtifactDeviceCoverage {
    pub device_id: String,
    pub device_name: Option<String>,
    pub current: bool,
    pub observed_logical_version: i64,
    pub installed_artifact_id: Option<String>,
    pub local_status: String,
    pub checked_at: i64,
    pub error_code: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeArtifactCoverage {
    pub artifact_id: String,
    pub logical_version: i64,
    pub ready_replica_count: usize,
    pub devices: Vec<KnowledgeArtifactDeviceCoverage>,
}

#[tauri::command]
pub async fn publish_knowledge_artifact(
    state: tauri::State<'_, AppState>,
    collection_id: String,
    agent_id: String,
    document_id: String,
) -> AppResult<KnowledgeArtifactPublishResult> {
    let documents = state
        .db
        .list_knowledge_documents(collection_id, agent_id)
        .await?;
    let source_version_id = documents
        .into_iter()
        .find(|document| document.id == document_id)
        .and_then(|document| document.current_version_id)
        .ok_or_else(|| AppError::Other("知识文档不可用".into()))?;
    let prepared = state
        .sync
        .publish_knowledge_artifact(
            state.storage.clone(),
            source_version_id,
            state.data_dir.join("artifacts").join("outbox"),
        )
        .await?;
    if let Err(error) =
        crate::storage::artifact_cache::enforce_quota(&state.db, state.data_dir.join("artifacts"))
            .await
    {
        eprintln!("[artifact-gc] post-publish cleanup failed: {error}");
    }
    Ok(KnowledgeArtifactPublishResult {
        artifact_id: prepared.artifact_id,
        reused: prepared.reused,
        ready_replica_count: prepared.ready_replica_count,
    })
}

#[tauri::command]
pub async fn get_knowledge_artifact_coverage(
    state: tauri::State<'_, AppState>,
    collection_id: String,
    agent_id: String,
    document_id: String,
) -> AppResult<KnowledgeArtifactCoverage> {
    let document_exists = state
        .db
        .list_knowledge_documents(collection_id, agent_id)
        .await?
        .into_iter()
        .any(|document| document.id == document_id && document.current_version_id.is_some());
    if !document_exists {
        return Err(AppError::Other("知识文档不可用".into()));
    }
    let response = state
        .sync
        .get_object_manifest(&format!("knowledge:{document_id}"))
        .await?;
    let devices = state.sync.list_devices().await?;
    let coverage = response
        .device_states
        .into_iter()
        .map(|device_state| {
            let device = devices
                .iter()
                .find(|device| device.id == device_state.device_id);
            KnowledgeArtifactDeviceCoverage {
                device_id: device_state.device_id,
                device_name: device.map(|device| device.name.clone()),
                current: device.is_some_and(|device| device.current),
                observed_logical_version: device_state.observed_logical_version,
                installed_artifact_id: device_state.installed_artifact_id,
                local_status: device_state.local_status,
                checked_at: device_state.checked_at,
                error_code: device_state.error_code,
            }
        })
        .collect();
    Ok(KnowledgeArtifactCoverage {
        artifact_id: response.manifest.artifact_id,
        logical_version: response.manifest.logical_version,
        ready_replica_count: response
            .replicas
            .iter()
            .filter(|replica| replica.status == "ready")
            .count(),
        devices: coverage,
    })
}

async fn retrieve_knowledge_hybrid(
    state: &AppState,
    agent_id: &str,
    query: &str,
    collection_id: Option<&str>,
    allow_hidden_collection: bool,
    limit: usize,
) -> AppResult<Vec<crate::db::repo::knowledge::KnowledgeSearchResult>> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 20);
    let candidate_limit = limit.saturating_mul(3).clamp(limit, 60);
    let visible_collection_ids = state
        .db
        .list_knowledge_collections(agent_id.to_string())
        .await?
        .into_iter()
        .map(|collection| collection.id)
        .collect::<Vec<_>>();
    let selected_collection_id = collection_id.map(ToString::to_string);
    if let Some(requested_collection_id) = selected_collection_id.as_deref() {
        if !visible_collection_ids
            .iter()
            .any(|collection_id| collection_id == requested_collection_id)
            && !allow_hidden_collection
        {
            return Ok(Vec::new());
        }
    } else if visible_collection_ids.is_empty() {
        return Ok(Vec::new());
    }
    let text_results = state
        .db
        .search_knowledge(
            agent_id.to_string(),
            query.to_string(),
            selected_collection_id.clone(),
            candidate_limit,
        )
        .await?;
    let mut fused = HashMap::new();
    for (rank, result) in text_results.into_iter().enumerate() {
        fused.insert(
            result.chunk_id.clone(),
            (1.0 / (60.0 + rank as f64 + 1.0), result),
        );
    }

    let roles = load_model_roles(&state.db).await?;
    let Some(model_ref) = roles.embedding_model.as_deref() else {
        return Ok(rank_knowledge_results(fused, limit));
    };
    let Some(config) = resolve_routed_llm_config(state, Some(model_ref)).await? else {
        return Ok(rank_knowledge_results(fused, limit));
    };
    let vector = match crate::embeddings::request_embeddings_with_timeout(
        &state.agent,
        config,
        vec![query.to_string()],
        std::time::Duration::from_secs(5),
    )
    .await
    {
        Ok(mut vectors) if vectors.len() == 1 => vectors.remove(0),
        Ok(_) => return Ok(rank_knowledge_results(fused, limit)),
        Err(error) => {
            eprintln!("[rag] Query embedding failed; using FTS only: {error}");
            return Ok(rank_knowledge_results(fused, limit));
        }
    };
    let collections = if let Some(collection_id) = selected_collection_id {
        vec![collection_id]
    } else {
        visible_collection_ids
    };
    for collection_id in collections {
        let results = state
            .db
            .search_knowledge_vectors(
                collection_id,
                crate::db::repo::knowledge::KnowledgeQueryEmbedding {
                    model: model_ref.to_string(),
                    vector: vector.clone(),
                },
                candidate_limit,
            )
            .await?;
        for (rank, result) in results.into_iter().enumerate() {
            let score = 1.0 / (60.0 + rank as f64 + 1.0);
            fused
                .entry(result.chunk_id.clone())
                .and_modify(
                    |entry: &mut (f64, crate::db::repo::knowledge::KnowledgeSearchResult)| {
                        entry.0 += score
                    },
                )
                .or_insert((score, result));
        }
    }
    Ok(rank_knowledge_results(fused, limit))
}

fn latest_user_query(history: &[serde_json::Value]) -> Option<String> {
    history.iter().rev().find_map(|message| {
        if message.get("role").and_then(serde_json::Value::as_str) != Some("user") {
            return None;
        }
        let query = message
            .get("parts")
            .and_then(serde_json::Value::as_array)?
            .iter()
            .filter(|part| part.get("kind").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|part| part.get("content").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        (!query.trim().is_empty()).then(|| query.trim().to_string())
    })
}

async fn retrieve_knowledge_for_history(
    state: &AppState,
    agent_id: &str,
    history: &[serde_json::Value],
    collection_id: Option<&str>,
    allow_hidden_collection: bool,
) -> Vec<serde_json::Value> {
    let Some(query) = latest_user_query(history) else {
        return Vec::new();
    };
    match retrieve_knowledge_hybrid(
        state,
        agent_id,
        &query,
        collection_id,
        allow_hidden_collection,
        6,
    )
    .await
    {
        Ok(results) => results
            .into_iter()
            .map(|result| {
                json!({
                    "documentId": result.document_id,
                    "documentVersionId": result.document_version_id,
                    "chunkId": result.chunk_id,
                    "title": result.title,
                    "sectionPath": result.section_path,
                    "content": result.content,
                })
            })
            .collect(),
        Err(error) => {
            eprintln!("[rag] Knowledge retrieval failed; continuing without it: {error}");
            Vec::new()
        }
    }
}

fn rank_knowledge_results(
    fused: HashMap<String, (f64, crate::db::repo::knowledge::KnowledgeSearchResult)>,
    limit: usize,
) -> Vec<crate::db::repo::knowledge::KnowledgeSearchResult> {
    let mut ranked = fused.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.document_id.cmp(&right.1.document_id))
            .then_with(|| left.1.ordinal.cmp(&right.1.ordinal))
    });
    ranked
        .into_iter()
        .map(|(_, result)| result)
        .take(limit)
        .collect()
}

#[tauri::command]
pub async fn search_knowledge_hybrid(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    query: String,
    collection_id: Option<String>,
    limit: Option<usize>,
) -> AppResult<Vec<crate::db::repo::knowledge::KnowledgeSearchResult>> {
    retrieve_knowledge_hybrid(
        &state,
        &agent_id,
        query.trim(),
        collection_id.as_deref(),
        false,
        limit.unwrap_or(10),
    )
    .await
}

#[tauri::command]
pub fn list_storage_provider_catalog(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::storage::ProviderDescriptor>> {
    state.storage.catalog()
}

#[tauri::command]
pub async fn list_storage_accounts(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::storage::service::StorageAccountView>> {
    state.storage.list_accounts().await
}

#[tauri::command]
pub async fn authorize_storage_provider(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    input: serde_json::Value,
) -> AppResult<String> {
    state
        .storage
        .authorize_account(
            provider_id,
            crate::storage::ProviderAuthorizationRequest { input },
        )
        .await
}

#[tauri::command]
pub async fn begin_storage_provider_authorization(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    input: serde_json::Value,
) -> AppResult<crate::storage::ports::ProviderAuthorizationChallenge> {
    state
        .storage
        .begin_authorization(
            provider_id,
            crate::storage::ProviderAuthorizationRequest { input },
        )
        .await
}

#[tauri::command]
pub async fn poll_storage_provider_authorization(
    state: tauri::State<'_, AppState>,
    provider_id: String,
    challenge_id: String,
) -> AppResult<crate::storage::service::StorageAuthorizationProgress> {
    state
        .storage
        .poll_authorization(provider_id, challenge_id)
        .await
}

#[tauri::command]
pub async fn list_storage_files(
    state: tauri::State<'_, AppState>,
    account_id: String,
    parent_id: Option<String>,
    page_token: Option<String>,
    page_size: Option<usize>,
) -> AppResult<crate::storage::RemoteFilePage> {
    state
        .storage
        .list_files(
            account_id,
            crate::storage::ListFilesRequest {
                parent_id,
                page_token,
                page_size: page_size.unwrap_or(100),
            },
        )
        .await
}

#[tauri::command]
pub async fn download_storage_file(
    state: tauri::State<'_, AppState>,
    account_id: String,
    file_id: String,
    expected_revision: Option<String>,
    expected_size: Option<u64>,
    destination: String,
) -> AppResult<()> {
    state
        .storage
        .download_file(
            account_id,
            file_id,
            expected_revision,
            expected_size,
            destination,
        )
        .await
}

#[tauri::command]
pub async fn download_storage_folder(
    state: tauri::State<'_, AppState>,
    account_id: String,
    folder_id: String,
    folder_name: String,
    destination_directory: String,
) -> AppResult<usize> {
    state
        .storage
        .download_folder(account_id, folder_id, folder_name, destination_directory)
        .await
}

#[tauri::command]
pub async fn upload_storage_file(
    state: tauri::State<'_, AppState>,
    account_id: String,
    parent_id: Option<String>,
    source: String,
) -> AppResult<crate::storage::RemoteFileItem> {
    state
        .storage
        .upload_file(account_id, parent_id, source)
        .await
}

#[tauri::command]
pub async fn trash_storage_files(
    state: tauri::State<'_, AppState>,
    account_id: String,
    file_ids: Vec<String>,
) -> AppResult<usize> {
    state.storage.trash_files(account_id, file_ids).await
}

#[tauri::command]
pub async fn move_storage_files(
    state: tauri::State<'_, AppState>,
    account_id: String,
    file_ids: Vec<String>,
    target_folder_id: Option<String>,
) -> AppResult<usize> {
    state
        .storage
        .move_files(account_id, file_ids, target_folder_id)
        .await
}

#[tauri::command]
pub async fn refresh_storage_quota(
    state: tauri::State<'_, AppState>,
    account_id: String,
) -> AppResult<crate::storage::ProviderQuota> {
    state.storage.refresh_quota(account_id).await
}

#[tauri::command]
pub async fn list_storage_transfers(
    state: tauri::State<'_, AppState>,
    account_id: Option<String>,
    limit: Option<usize>,
) -> AppResult<Vec<crate::db::repo::storage::StorageTransferJobRow>> {
    state
        .storage
        .list_transfers(account_id, limit.unwrap_or(100))
        .await
}

#[tauri::command]
pub async fn remove_storage_account(
    state: tauri::State<'_, AppState>,
    account_id: String,
) -> AppResult<()> {
    state.storage.remove_account(account_id).await
}

#[tauri::command]
pub async fn get_artifact_storage_status(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::storage::artifact_cache::ArtifactStorageStatus> {
    crate::storage::artifact_cache::status(&state.db, state.data_dir.join("artifacts")).await
}

#[tauri::command]
pub async fn set_artifact_storage_quota(
    state: tauri::State<'_, AppState>,
    quota_bytes: u64,
) -> AppResult<crate::storage::artifact_cache::ArtifactGcResult> {
    crate::storage::artifact_cache::set_quota(
        &state.db,
        state.data_dir.join("artifacts"),
        quota_bytes,
    )
    .await
}

#[tauri::command]
pub async fn cleanup_artifact_storage(
    state: tauri::State<'_, AppState>,
) -> AppResult<crate::storage::artifact_cache::ArtifactGcResult> {
    crate::storage::artifact_cache::cleanup(&state.db, state.data_dir.join("artifacts")).await
}

#[tauri::command]
pub async fn list_calendars(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::db::repo::planner::CalendarRow>> {
    state.db.list_calendars().await
}
#[tauri::command]
pub async fn create_calendar(
    state: tauri::State<'_, AppState>,
    name: String,
    color: Option<String>,
    timezone: String,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .create_calendar(id.clone(), name, color, timezone)
        .await?;
    Ok(id)
}
#[tauri::command]
pub async fn list_calendar_events(
    state: tauri::State<'_, AppState>,
    calendar_id: String,
    range_start: String,
    range_end: String,
) -> AppResult<Vec<crate::db::repo::planner::EventRow>> {
    state
        .db
        .list_calendar_events(calendar_id, range_start, range_end)
        .await
}

#[tauri::command]
pub async fn get_calendar_event(
    state: tauri::State<'_, AppState>,
    event_id: String,
) -> AppResult<crate::db::repo::planner::EventRow> {
    state.db.get_calendar_event(event_id).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEventUpdatePayload {
    pub title: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub timezone: Option<String>,
    pub all_day: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub recurrence_rule: Option<Option<String>>,
}

impl From<CalendarEventUpdatePayload> for crate::db::repo::planner::EventUpdate {
    fn from(value: CalendarEventUpdatePayload) -> Self {
        Self {
            title: value.title,
            starts_at: value.starts_at,
            ends_at: value.ends_at,
            timezone: value.timezone,
            all_day: value.all_day,
            recurrence_rule: value.recurrence_rule,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarOccurrenceUpdatePayload {
    pub title: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub timezone: Option<String>,
    pub all_day: Option<bool>,
}

impl From<CalendarOccurrenceUpdatePayload> for crate::db::repo::planner::OccurrenceUpdate {
    fn from(value: CalendarOccurrenceUpdatePayload) -> Self {
        Self {
            title: value.title,
            starts_at: value.starts_at,
            ends_at: value.ends_at,
            timezone: value.timezone,
            all_day: value.all_day,
        }
    }
}

#[tauri::command]
pub async fn create_calendar_event(
    state: tauri::State<'_, AppState>,
    calendar_id: String,
    title: String,
    starts_at: String,
    ends_at: String,
    timezone: String,
    all_day: bool,
    recurrence_rule: Option<String>,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .create_calendar_event(
            id.clone(),
            calendar_id,
            title,
            starts_at,
            ends_at,
            timezone,
            all_day,
            recurrence_rule,
        )
        .await?;
    Ok(id)
}

#[tauri::command]
pub async fn update_calendar_event(
    state: tauri::State<'_, AppState>,
    event_id: String,
    changes: CalendarEventUpdatePayload,
) -> AppResult<crate::db::repo::planner::EventRow> {
    state
        .db
        .update_calendar_event(event_id, changes.into())
        .await
}

#[tauri::command]
pub async fn update_calendar_occurrence(
    state: tauri::State<'_, AppState>,
    event_id: String,
    original_occurrence: String,
    changes: CalendarOccurrenceUpdatePayload,
) -> AppResult<crate::db::repo::planner::EventRow> {
    state
        .db
        .update_calendar_occurrence(
            uuid::Uuid::new_v4().to_string(),
            event_id,
            original_occurrence,
            changes.into(),
        )
        .await
}

#[tauri::command]
pub async fn cancel_calendar_occurrence(
    state: tauri::State<'_, AppState>,
    event_id: String,
    original_occurrence: String,
) -> AppResult<()> {
    state
        .db
        .cancel_calendar_occurrence(event_id, original_occurrence)
        .await
}

#[tauri::command]
pub async fn restore_calendar_occurrence(
    state: tauri::State<'_, AppState>,
    event_id: String,
    original_occurrence: String,
) -> AppResult<()> {
    state
        .db
        .restore_calendar_occurrence(event_id, original_occurrence)
        .await
}

#[tauri::command]
pub async fn delete_calendar_event(
    state: tauri::State<'_, AppState>,
    event_id: String,
) -> AppResult<()> {
    state.db.delete_calendar_event(event_id).await
}

#[tauri::command]
pub async fn list_task_lists(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::db::repo::planner::TaskListRow>> {
    state.db.list_task_lists().await
}
#[tauri::command]
pub async fn create_task_list(
    state: tauri::State<'_, AppState>,
    name: String,
    color: Option<String>,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    state.db.create_task_list(id.clone(), name, color).await?;
    Ok(id)
}
#[tauri::command]
pub async fn list_tasks(
    state: tauri::State<'_, AppState>,
    task_list_id: String,
) -> AppResult<Vec<crate::db::repo::planner::TaskRow>> {
    state.db.list_tasks(task_list_id).await
}

#[tauri::command]
pub async fn list_all_tasks(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<crate::db::repo::planner::TaskRow>> {
    state.db.list_all_tasks().await
}

#[tauri::command]
pub async fn create_task(
    state: tauri::State<'_, AppState>,
    task_list_id: String,
    parent_id: Option<String>,
    title: String,
    description: Option<String>,
    priority: i64,
    due_date: Option<String>,
    due_at: Option<String>,
    due_timezone: Option<String>,
    is_important: Option<bool>,
    my_day_date: Option<String>,
    recurrence_rule: Option<String>,
    sort_order: f64,
) -> AppResult<String> {
    let id = uuid::Uuid::new_v4().to_string();
    state
        .db
        .create_task(crate::db::repo::planner::NewTask {
            id: id.clone(),
            task_list_id,
            parent_id,
            title,
            description,
            priority,
            due_date,
            due_at,
            due_timezone,
            is_important: is_important.unwrap_or(false),
            my_day_date,
            recurrence_rule,
            sort_order,
        })
        .await?;
    Ok(id)
}
#[tauri::command]
pub async fn complete_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
    completed: bool,
) -> AppResult<()> {
    state.db.complete_task(task_id, completed).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdatePayload {
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub description: Option<Option<String>>,
    pub priority: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub due_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub due_at: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub due_timezone: Option<Option<String>>,
    pub is_important: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub my_day_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub recurrence_rule: Option<Option<String>>,
    pub sort_order: Option<f64>,
}

impl From<TaskUpdatePayload> for crate::db::repo::planner::TaskUpdate {
    fn from(value: TaskUpdatePayload) -> Self {
        Self {
            title: value.title,
            description: value.description,
            priority: value.priority,
            due_date: value.due_date,
            due_at: value.due_at,
            due_timezone: value.due_timezone,
            is_important: value.is_important,
            my_day_date: value.my_day_date,
            recurrence_rule: value.recurrence_rule,
            sort_order: value.sort_order,
        }
    }
}

#[tauri::command]
pub async fn update_task(
    state: tauri::State<'_, AppState>,
    task_id: String,
    changes: TaskUpdatePayload,
) -> AppResult<crate::db::repo::planner::TaskRow> {
    state.db.update_task(task_id, changes.into()).await
}

#[tauri::command]
pub async fn delete_task(state: tauri::State<'_, AppState>, task_id: String) -> AppResult<()> {
    state.db.delete_task(task_id).await
}

/// Device-local notification inbox. Notifications are derived data and are not
/// included in the encrypted entity sync payload.
#[tauri::command]
pub async fn list_notifications(
    state: tauri::State<'_, AppState>,
    limit: Option<usize>,
) -> AppResult<Vec<crate::db::repo::notifications::NotificationRow>> {
    state.db.list_notifications(limit.unwrap_or(50)).await
}

#[tauri::command]
pub async fn mark_notification_read(
    state: tauri::State<'_, AppState>,
    notification_id: String,
) -> AppResult<()> {
    state.notifications.mark_read(&notification_id).await
}

#[tauri::command]
pub async fn mark_all_notifications_read(state: tauri::State<'_, AppState>) -> AppResult<()> {
    state.notifications.mark_all_read().await
}

#[derive(serde::Serialize)]
pub struct AuditLogDto {
    pub id: String,
    pub time: String,
    pub tool: String,
    pub params: String,
    pub status: String,
    pub risk: String,
}

#[tauri::command]
pub async fn list_audit_logs(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<Vec<AuditLogDto>> {
    let rows = state.db.list_tool_calls(session_id).await?;
    Ok(rows
        .into_iter()
        .map(|r| AuditLogDto {
            id: r.id,
            time: r.created_at,
            tool: r.tool,
            params: r.params.unwrap_or_default(),
            status: r.status,
            risk: r.risk_level.unwrap_or_else(|| "Low".to_string()),
        })
        .collect())
}

// ── Model Provider DTOs & Commands ──────────────────────────────────────────

#[derive(Serialize)]
pub struct ModelProviderDto {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub api_base: Option<String>,
    pub is_default: bool,
    pub models: Vec<ModelDescriptor>,
    /// 是否已保存 API Key（出于安全不返回明文，仅暴露是否配置）
    pub has_api_key: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct UpsertProviderInput {
    pub id: Option<String>,
    pub name: String,
    pub kind: String,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub is_default: Option<bool>,
    pub models: Option<Vec<ModelDescriptor>>,
}

#[derive(Serialize)]
pub struct TestProviderResult {
    pub success: bool,
    pub message: String,
}

async fn restore_secret(
    store: &dyn SecretStore,
    secret_id: &str,
    previous: Option<&str>,
) -> AppResult<()> {
    match previous {
        Some(value) => store.set(secret_id, value).await,
        None => store.delete(secret_id).await,
    }
}

fn normalized_api_base(value: Option<&str>) -> &str {
    value.unwrap_or_default().trim().trim_end_matches('/')
}

fn saved_provider_endpoint_matches(
    stored_kind: &str,
    stored_api_base: Option<&str>,
    requested_kind: &str,
    requested_api_base: Option<&str>,
) -> bool {
    stored_kind == requested_kind
        && normalized_api_base(stored_api_base) == normalized_api_base(requested_api_base)
}

/// 列出所有已配置的模型提供商。
#[tauri::command]
pub async fn list_providers(state: tauri::State<'_, AppState>) -> AppResult<Vec<ModelProviderDto>> {
    let rows = state.db.list_model_providers().await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let models = parse_model_catalog(r.models_json.as_deref(), &r.kind);
        // Only expose whether the OS keyring contains a credential.
        let stored_key = state
            .secrets
            .get(&provider_api_key_secret_id(&r.id))
            .await?;
        let has_api_key = stored_key.map(|k| !k.is_empty()).unwrap_or(false);
        out.push(ModelProviderDto {
            id: r.id,
            name: r.name,
            kind: r.kind,
            api_base: r.api_base,
            is_default: r.is_default != 0,
            models,
            has_api_key,
            created_at: r.created_at,
            updated_at: r.updated_at,
        });
    }
    Ok(out)
}

/// 新增或更新模型提供商。如果 id 为 None 则自动生成 UUID。
#[tauri::command]
pub async fn upsert_provider(
    state: tauri::State<'_, AppState>,
    provider: UpsertProviderInput,
) -> AppResult<String> {
    let id = provider
        .id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let secret_id = provider_api_key_secret_id(&id);
    let replacement_key = provider.api_key.filter(|key| !key.is_empty());
    let previous_key = if replacement_key.is_some() {
        state.secrets.get(&secret_id).await?
    } else {
        None
    };
    if let Some(ref key) = replacement_key {
        if let Err(write_error) = state.secrets.set(&secret_id, key).await {
            let rollback_error =
                restore_secret(state.secrets.as_ref(), &secret_id, previous_key.as_deref())
                    .await
                    .err();
            return Err(AppError::SecretStore(format!(
                "credential write failed: {write_error}{}",
                rollback_error
                    .map(|error| format!("; rollback failed: {error}"))
                    .unwrap_or_default()
            )));
        }
        let verification_error = match state.secrets.get(&secret_id).await {
            Ok(Some(stored)) if stored == *key => None,
            Ok(_) => Some("stored credential did not match the submitted value".to_string()),
            Err(error) => Some(error.to_string()),
        };
        if let Some(verification_error) = verification_error {
            let rollback_error =
                restore_secret(state.secrets.as_ref(), &secret_id, previous_key.as_deref())
                    .await
                    .err();
            return Err(AppError::SecretStore(format!(
                "credential verification failed: {verification_error}{}",
                rollback_error
                    .map(|error| format!("; rollback failed: {error}"))
                    .unwrap_or_default()
            )));
        }
    }

    let normalized_models =
        normalize_model_catalog(provider.models.unwrap_or_default(), &provider.kind);
    let models_json = Some(serde_json::to_string(&normalized_models)?);

    let set_default = provider.is_default.unwrap_or(false);

    let new_row = crate::db::repo::model_providers::NewModelProvider {
        id: id.clone(),
        name: provider.name,
        kind: provider.kind,
        api_base: provider.api_base,
        is_default: if set_default { 1 } else { 0 },
        models_json,
        extra_config: None,
    };

    if let Err(db_error) = state.db.upsert_model_provider(new_row, set_default).await {
        if replacement_key.is_some() {
            if let Err(rollback_error) =
                restore_secret(state.secrets.as_ref(), &secret_id, previous_key.as_deref()).await
            {
                return Err(AppError::SecretStore(format!(
                    "provider save failed: {db_error}; credential rollback failed: {rollback_error}"
                )));
            }
        }
        return Err(db_error);
    }
    let mut roles = load_model_roles(&state.db).await?;
    roles.retain_valid_provider_models(&id, &normalized_models);
    save_model_roles(&state.db, &roles).await?;
    Ok(id)
}

/// 删除模型提供商及其关联的 API Key。
#[tauri::command]
pub async fn delete_provider(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> AppResult<()> {
    let secret_id = provider_api_key_secret_id(&provider_id);
    let previous_key = state.secrets.get(&secret_id).await?;
    state.secrets.delete(&secret_id).await?;
    if let Err(db_error) = state.db.delete_model_provider(provider_id.clone()).await {
        if let Err(rollback_error) =
            restore_secret(state.secrets.as_ref(), &secret_id, previous_key.as_deref()).await
        {
            return Err(AppError::SecretStore(format!(
                "provider delete failed: {db_error}; credential rollback failed: {rollback_error}"
            )));
        }
        return Err(db_error);
    }
    let mut roles = load_model_roles(&state.db).await?;
    roles.clear_provider(&provider_id);
    save_model_roles(&state.db, &roles).await?;
    Ok(())
}

/// Return the global model role assignments.
#[tauri::command]
pub async fn get_model_roles(state: tauri::State<'_, AppState>) -> AppResult<ModelRoleAssignments> {
    load_model_roles(&state.db).await
}

/// Validate and persist the global model role assignments.
#[tauri::command]
pub async fn set_model_roles(
    state: tauri::State<'_, AppState>,
    mut roles: ModelRoleAssignments,
) -> AppResult<()> {
    roles.normalize_fallback_models().map_err(AppError::Other)?;
    let providers = state.db.list_model_providers().await?;
    for (role, selection) in roles.selections() {
        let Some(selection) = selection.filter(|value| !value.is_empty()) else {
            continue;
        };
        let (provider_id, model_id) = selection.split_once('/').ok_or_else(|| {
            AppError::Other(format!("{}的模型引用格式无效: {selection}", role.label()))
        })?;
        let provider = providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| AppError::Other(format!("模型服务商 `{provider_id}` 不存在")))?;
        let models = parse_model_catalog(provider.models_json.as_deref(), &provider.kind);
        let model = models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| AppError::Other(format!("模型 `{selection}` 不存在")))?;
        if !role.accepts(&model.capabilities) {
            return Err(AppError::Other(format!(
                "模型 `{selection}` 不满足{}所需的能力标签",
                role.label()
            )));
        }
    }
    for selection in &roles.fallback_models {
        let (provider_id, model_id) = selection
            .split_once('/')
            .ok_or_else(|| AppError::Other(format!("备用模型引用格式无效: {selection}")))?;
        let provider = providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(|| AppError::Other(format!("模型服务商 `{provider_id}` 不存在")))?;
        let models = parse_model_catalog(provider.models_json.as_deref(), &provider.kind);
        let model = models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| AppError::Other(format!("模型 `{selection}` 不存在")))?;
        if !ModelRole::Main.accepts(&model.capabilities) {
            return Err(AppError::Other(format!(
                "模型 `{selection}` 不满足备用主模型所需的文本生成能力"
            )));
        }
    }
    save_model_roles(&state.db, &roles).await
}

#[derive(Serialize)]
pub struct SearchProviderSettingsDto {
    pub fallback_order: Vec<String>,
    pub searxng_base_url: Option<String>,
    pub has_brave_api_key: bool,
}

#[derive(Deserialize)]
pub struct SearchProviderSettingsInput {
    pub fallback_order: Vec<String>,
    pub searxng_base_url: Option<String>,
    pub brave_api_key: Option<String>,
    #[serde(default)]
    pub clear_brave_api_key: bool,
}

#[derive(Serialize)]
pub struct SearchProviderTestResult {
    pub success: bool,
    pub provider: String,
    pub category: Option<String>,
    pub message: String,
    pub result_count: usize,
    pub latency_ms: u128,
}

/// Return device-local search routing without exposing the Brave API key.
#[tauri::command]
pub async fn get_search_provider_settings(
    state: tauri::State<'_, AppState>,
) -> AppResult<SearchProviderSettingsDto> {
    let settings = crate::web::load_search_provider_settings(&state.db).await?;
    let has_brave_api_key = state
        .secrets
        .get(crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID)
        .await
        .ok()
        .flatten()
        .is_some_and(|key| !key.is_empty());
    Ok(SearchProviderSettingsDto {
        fallback_order: settings.fallback_order,
        searxng_base_url: settings.searxng_base_url,
        has_brave_api_key,
    })
}

/// Persist search routing and update the Brave credential transactionally.
#[tauri::command]
pub async fn set_search_provider_settings(
    state: tauri::State<'_, AppState>,
    input: SearchProviderSettingsInput,
) -> AppResult<()> {
    persist_search_provider_settings(&state.db, state.secrets.as_ref(), input).await
}

async fn persist_search_provider_settings(
    db: &crate::db::DbActorHandle,
    secrets: &dyn SecretStore,
    mut input: SearchProviderSettingsInput,
) -> AppResult<()> {
    let mut settings = crate::web::SearchProviderSettings {
        fallback_order: input.fallback_order,
        searxng_base_url: input.searxng_base_url,
    };
    settings.normalize()?;

    let secret_id = crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID;
    let supplied_key = input
        .brave_api_key
        .take()
        .map(|key| Zeroizing::new(key.trim().to_string()))
        .filter(|key| !key.is_empty());
    let supplied_key_value = supplied_key.as_ref().map(|key| key.as_str());
    if supplied_key_value
        .is_some_and(|key| key.len() > 1024 || !key.bytes().all(|value| value.is_ascii_graphic()))
    {
        return Err(AppError::Other(
            "Brave Search API Key 必须是 1 到 1024 字节的可打印 ASCII 字符".into(),
        ));
    }
    let brave_in_chain = settings
        .fallback_order
        .iter()
        .any(|provider| provider == "brave");
    let credential_access_needed =
        brave_in_chain || input.clear_brave_api_key || supplied_key_value.is_some();
    let previous_key = if credential_access_needed {
        secrets.get(secret_id).await?.map(Zeroizing::new)
    } else {
        None
    };
    let previous_key_value = previous_key.as_ref().map(|key| key.as_str());
    let desired_key = if input.clear_brave_api_key {
        None
    } else {
        supplied_key_value.or_else(|| previous_key_value.filter(|key| !key.is_empty()))
    };
    if brave_in_chain && desired_key.is_none() {
        return Err(AppError::Other(
            "回退链包含 Brave Search，但尚未配置 API Key".into(),
        ));
    }

    let credential_changed = input.clear_brave_api_key || supplied_key.is_some();
    if input.clear_brave_api_key {
        secrets.delete(secret_id).await?;
    } else if let Some(key) = supplied_key_value {
        secrets.set(secret_id, key).await?;
    }
    if credential_changed {
        let verified = secrets.get(secret_id).await?;
        if verified.as_deref() != desired_key {
            let rollback_error = restore_secret(secrets, secret_id, previous_key_value)
                .await
                .err();
            return Err(AppError::SecretStore(format!(
                "search credential verification failed{}",
                rollback_error
                    .map(|error| format!("; rollback failed: {error}"))
                    .unwrap_or_default()
            )));
        }
    }

    if let Err(db_error) = crate::web::save_search_provider_settings(db, &settings).await {
        let rollback_error = if credential_changed {
            restore_secret(secrets, secret_id, previous_key_value)
                .await
                .err()
        } else {
            None
        };
        if let Some(rollback_error) = rollback_error {
            return Err(AppError::SecretStore(format!(
                "search settings save failed: {db_error}; credential rollback failed: {rollback_error}"
            )));
        }
        return Err(db_error);
    }
    Ok(())
}

/// Probe one configured search Provider with a fixed non-sensitive query.
#[tauri::command]
pub async fn test_search_provider(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> AppResult<SearchProviderTestResult> {
    let provider = provider_id.trim().to_ascii_lowercase();
    if !crate::web::SEARCH_PROVIDER_IDS.contains(&provider.as_str()) {
        return Err(AppError::Other(format!(
            "不支持的搜索 Provider `{provider}`"
        )));
    }
    let settings = crate::web::load_search_provider_settings(&state.db).await?;
    let api_key = if provider == "brave" {
        state
            .secrets
            .get(crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID)
            .await?
    } else {
        None
    };
    let started = std::time::Instant::now();
    let probe =
        crate::web::probe_search_provider(&provider, &settings, api_key.as_deref(), 15).await;
    let latency_ms = started.elapsed().as_millis();
    Ok(match probe {
        Ok(result_count) => SearchProviderTestResult {
            success: true,
            provider,
            category: None,
            message: format!("连接正常，返回 {result_count} 条测试结果"),
            result_count,
            latency_ms,
        },
        Err(category) => SearchProviderTestResult {
            success: false,
            provider,
            category: Some(category.as_str().into()),
            message: category.display_message().into(),
            result_count: 0,
            latency_ms,
        },
    })
}

#[derive(Serialize)]
pub struct SecretStoreStatusDto {
    pub available: bool,
    pub backend: &'static str,
    pub error: Option<String>,
}

/// Report whether the native credential store is available without exposing any secret.
#[tauri::command]
pub async fn get_secret_store_status(
    state: tauri::State<'_, AppState>,
) -> AppResult<SecretStoreStatusDto> {
    let runtime_error = crate::secrets::verify_secret_store(state.secrets.as_ref())
        .await
        .err()
        .map(|error| error.to_string());
    let error = state.secret_store_startup_error.clone().or(runtime_error);
    Ok(SecretStoreStatusDto {
        available: error.is_none(),
        backend: "OS Keyring",
        error,
    })
}

/// Test that a model provider can authenticate without exposing its credential.
#[tauri::command]
pub async fn test_provider(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> AppResult<TestProviderResult> {
    let provider = state.db.get_model_provider(provider_id.clone()).await?;
    match provider {
        None => Ok(TestProviderResult {
            success: false,
            message: format!("Provider `{}` not found", provider_id),
        }),
        Some(p) => {
            let api_key = state
                .secrets
                .get(&provider_api_key_secret_id(&provider_id))
                .await?;
            if p.kind != "ollama" && api_key.as_deref().is_none_or(str::is_empty) {
                return Ok(TestProviderResult {
                    success: false,
                    message: format!("Provider `{}` has no API key configured", p.name),
                });
            }

            let base = p.api_base.as_deref().unwrap_or(match p.kind.as_str() {
                "openai" => "https://api.openai.com/v1",
                "ollama" => "http://127.0.0.1:11434",
                _ => {
                    return Ok(TestProviderResult {
                        success: true,
                        message: format!("Provider `{}` has a configured credential", p.name),
                    });
                }
            });
            let endpoint = if p.kind == "ollama" {
                format!("{}/api/tags", base.trim_end_matches('/'))
            } else {
                format!("{}/models", base.trim_end_matches('/'))
            };
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|error| AppError::Other(format!("创建连接测试客户端失败: {error}")))?;
            let mut request = client.get(&endpoint);
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                request = request.bearer_auth(key);
            }
            match request.send().await {
                Ok(response) if response.status().is_success() => Ok(TestProviderResult {
                    success: true,
                    message: format!("Provider `{}` connection verified", p.name),
                }),
                Ok(response) => Ok(TestProviderResult {
                    success: false,
                    message: format!(
                        "Provider `{}` rejected the connection (HTTP {})",
                        p.name,
                        response.status()
                    ),
                }),
                Err(error) => Ok(TestProviderResult {
                    success: false,
                    message: format!("Provider `{}` connection failed: {error}", p.name),
                }),
            }
        }
    }
}

/// 从服务端自动获取可用模型列表
#[tauri::command]
pub async fn fetch_provider_models(
    state: tauri::State<'_, AppState>,
    provider_id: Option<String>,
    kind: String,
    api_base: Option<String>,
    api_key: Option<String>,
) -> AppResult<Vec<ModelDescriptor>> {
    let client = reqwest::Client::new();
    let mut models = Vec::new();
    let api_key = match api_key.filter(|key| !key.is_empty()) {
        Some(key) => Some(key),
        None => match provider_id {
            Some(provider_id) => {
                let provider = state
                    .db
                    .get_model_provider(provider_id)
                    .await?
                    .ok_or_else(|| AppError::Other("模型服务商不存在".into()))?;
                if !saved_provider_endpoint_matches(
                    &provider.kind,
                    provider.api_base.as_deref(),
                    &kind,
                    api_base.as_deref(),
                ) {
                    return Err(AppError::Other(
                        "服务商类型或 API 地址已修改；请重新输入 API Key 后再获取模型".into(),
                    ));
                }
                state
                    .secrets
                    .get(&provider_api_key_secret_id(&provider.id))
                    .await?
            }
            None => None,
        },
    };

    if kind == "openai" || kind == "openai_compatible" {
        let base = api_base.unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        // Handle trailing slashes gracefully
        let mut url = base.trim_end_matches('/').to_string();
        if !url.ends_with("/v1") && !url.contains("/v1") && kind == "openai" {
            url.push_str("/v1");
        }
        url.push_str("/models");

        let mut req = client.get(&url);
        if let Some(key) = api_key {
            if !key.is_empty() {
                req = req.bearer_auth(key);
            }
        }

        let resp = req
            .send()
            .await
            .map_err(|e| AppError::Other(format!("请求失败: {}", e)))?;
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::Other(format!("解析JSON失败: {}", e)))?;

        if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
            for item in data {
                if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                    models.push(descriptor_from_api(&kind, id, item));
                }
            }
        } else {
            return Err(AppError::Other("未在响应中找到模型数据".into()));
        }
    } else if kind == "ollama" {
        let base = api_base.unwrap_or_else(|| "http://localhost:11434".to_string());
        let url = format!("{}/api/tags", base.trim_end_matches('/'));

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Other(format!("请求失败: {}", e)))?;
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AppError::Other(format!("解析JSON失败: {}", e)))?;

        if let Some(models_arr) = json.get("models").and_then(|m| m.as_array()) {
            for item in models_arr {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    let metadata = match client
                        .post(format!("{}/api/show", base.trim_end_matches('/')))
                        .json(&json!({ "model": name }))
                        .send()
                        .await
                    {
                        Ok(response) => response
                            .json::<serde_json::Value>()
                            .await
                            .unwrap_or_else(|_| item.clone()),
                        Err(_) => item.clone(),
                    };
                    models.push(descriptor_from_api(&kind, name, &metadata));
                }
            }
        } else {
            return Err(AppError::Other("未在响应中找到模型数据".into()));
        }
    } else {
        return Err(AppError::Other(format!(
            "暂不支持自动获取 {} 的模型列表，请手动输入",
            kind
        )));
    }

    Ok(normalize_model_catalog(models, &kind))
}

#[tauri::command]
pub async fn list_installed_skills() -> AppResult<Vec<crate::skills::InstalledSkill>> {
    tokio::task::spawn_blocking(crate::skills::list_installed)
        .await
        .map_err(|error| AppError::Other(format!("Skill 列表任务异常中止：{error}")))?
}

#[tauri::command]
pub async fn install_skills_from_path(
    path: String,
) -> AppResult<Vec<crate::skills::InstalledSkill>> {
    tokio::task::spawn_blocking(move || crate::skills::install_from_path(path))
        .await
        .map_err(|error| AppError::Other(format!("Skill 安装任务异常中止：{error}")))?
}

#[tauri::command]
pub async fn install_skills_from_git(url: String) -> AppResult<Vec<crate::skills::InstalledSkill>> {
    crate::skills::install_from_git(url).await
}

#[tauri::command]
pub async fn set_skill_enabled(
    skill_id: String,
    enabled: bool,
) -> AppResult<crate::skills::InstalledSkill> {
    tokio::task::spawn_blocking(move || crate::skills::set_enabled(&skill_id, enabled))
        .await
        .map_err(|error| AppError::Other(format!("Skill 状态更新任务异常中止：{error}")))?
}

#[tauri::command]
pub async fn uninstall_skill(skill_id: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || crate::skills::uninstall(&skill_id))
        .await
        .map_err(|error| AppError::Other(format!("Skill 卸载任务异常中止：{error}")))?
}

#[tauri::command]
pub async fn open_skill_directory(skill_id: String) -> AppResult<()> {
    tokio::task::spawn_blocking(move || crate::skills::open_directory(&skill_id))
        .await
        .map_err(|error| AppError::Other(format!("Skill 目录打开任务异常中止：{error}")))?
}

/// 读取一个 settings 键值（供前端持久化 UI 状态，如上次选中的 agent/session）。
#[tauri::command]
pub async fn get_setting(
    state: tauri::State<'_, AppState>,
    key: String,
) -> AppResult<Option<String>> {
    if !crate::sync::settings::renderer_access_allowed(&key) {
        return Err(AppError::Other(format!(
            "Setting `{key}` is not available through generic renderer IPC"
        )));
    }
    state.db.get_setting(key).await
}

/// 写入一个 settings 键值。
#[tauri::command]
pub async fn set_setting(
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) -> AppResult<()> {
    if !crate::sync::settings::renderer_access_allowed(&key) {
        return Err(AppError::Other(format!(
            "Setting `{key}` is not writable through generic renderer IPC"
        )));
    }
    state.db.set_setting(key, value).await
}

#[cfg(test)]
mod tests {
    use super::{
        commit_managed_file, install_managed_file, is_automatic_session_title, latest_user_query,
        normalize_default_max_output_tokens, normalize_session_title,
        persist_search_provider_settings, read_local_chat_attachment, read_text_knowledge_document,
        rollback_managed_file, saved_provider_endpoint_matches, CalendarEventUpdatePayload,
        SearchProviderSettingsInput, TaskUpdatePayload, DEFAULT_MAX_OUTPUT_TOKENS,
    };
    use crate::secrets::{InMemorySecretStore, SecretStore};

    struct UnavailableSecretStore;

    #[test]
    fn default_max_output_tokens_are_128k_and_clamp_settings() {
        assert_eq!(
            normalize_default_max_output_tokens(None),
            DEFAULT_MAX_OUTPUT_TOKENS
        );
        assert_eq!(normalize_default_max_output_tokens(Some("131072")), 131_072);
        assert_eq!(normalize_default_max_output_tokens(Some("127")), 128);
        assert_eq!(
            normalize_default_max_output_tokens(Some("invalid")),
            DEFAULT_MAX_OUTPUT_TOKENS
        );
    }

    #[async_trait::async_trait]
    impl SecretStore for UnavailableSecretStore {
        async fn get(&self, _secret_id: &str) -> crate::error::AppResult<Option<String>> {
            Err(crate::error::AppError::SecretStore("unavailable".into()))
        }

        async fn set(&self, _secret_id: &str, _value: &str) -> crate::error::AppResult<()> {
            Err(crate::error::AppError::SecretStore("unavailable".into()))
        }

        async fn delete(&self, _secret_id: &str) -> crate::error::AppResult<()> {
            Err(crate::error::AppError::SecretStore("unavailable".into()))
        }
    }

    #[test]
    fn latest_user_query_uses_the_newest_text_message() {
        let history = vec![
            serde_json::json!({
                "role": "user",
                "parts": [{"kind": "text", "content": "older question"}],
            }),
            serde_json::json!({
                "role": "assistant",
                "parts": [{"kind": "text", "content": "assistant reply"}],
            }),
            serde_json::json!({
                "role": "user",
                "parts": [
                    {"kind": "tool_result", "content": "ignore this"},
                    {"kind": "text", "content": " latest "},
                    {"kind": "text", "content": "question "},
                ],
            }),
        ];

        assert_eq!(
            latest_user_query(&history).as_deref(),
            Some("latest \nquestion")
        );
    }

    #[test]
    fn stored_credentials_are_only_reused_for_the_saved_endpoint() {
        assert!(saved_provider_endpoint_matches(
            "openai_compatible",
            Some("https://models.example.test/v1/"),
            "openai_compatible",
            Some("https://models.example.test/v1"),
        ));
        assert!(!saved_provider_endpoint_matches(
            "openai_compatible",
            Some("https://models.example.test/v1"),
            "openai_compatible",
            Some("https://attacker.example.test/v1"),
        ));
        assert!(!saved_provider_endpoint_matches(
            "openai",
            None,
            "openai_compatible",
            None,
        ));
    }

    #[test]
    fn session_title_normalization_is_short_and_single_line() {
        assert_eq!(normalize_session_title("  \n\t"), None);
        assert_eq!(
            normalize_session_title("`  日历  同步  `").as_deref(),
            Some("日历 同步")
        );
        let expected = format!("{}…", "x".repeat(40));
        assert_eq!(normalize_session_title(&"x".repeat(41)), Some(expected));
    }

    #[test]
    fn automatic_session_titles_include_numbered_sidebar_placeholders() {
        assert!(is_automatic_session_title("新会话"));
        assert!(is_automatic_session_title("新会话 #48"));
        assert!(is_automatic_session_title("会话 #3"));
        assert!(!is_automatic_session_title("新会话 #daily"));
        assert!(!is_automatic_session_title("阅读 · Jane Eyre"));
    }

    #[tokio::test]
    async fn search_provider_settings_keep_credentials_out_of_db_and_validate_brave() {
        let db_path =
            std::env::temp_dir().join(format!("agnes-search-settings-{}.db", uuid::Uuid::new_v4()));
        let db = crate::db::spawn_db_actor(db_path);
        let secrets = InMemorySecretStore::default();

        persist_search_provider_settings(
            &db,
            &secrets,
            SearchProviderSettingsInput {
                fallback_order: vec!["brave".into(), "duckduckgo".into()],
                searxng_base_url: None,
                brave_api_key: Some("brave-secret".into()),
                clear_brave_api_key: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            secrets
                .get(crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID)
                .await
                .unwrap()
                .as_deref(),
            Some("brave-secret")
        );
        let stored = db
            .get_setting(crate::web::SEARCH_PROVIDER_SETTINGS_KEY.into())
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.contains("brave-secret"));

        persist_search_provider_settings(
            &db,
            &secrets,
            SearchProviderSettingsInput {
                fallback_order: vec!["duckduckgo".into()],
                searxng_base_url: None,
                brave_api_key: None,
                clear_brave_api_key: true,
            },
        )
        .await
        .unwrap();
        assert!(secrets
            .get(crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID)
            .await
            .unwrap()
            .is_none());

        let result = persist_search_provider_settings(
            &db,
            &secrets,
            SearchProviderSettingsInput {
                fallback_order: vec!["brave".into()],
                searxng_base_url: None,
                brave_api_key: None,
                clear_brave_api_key: false,
            },
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn no_key_search_settings_do_not_depend_on_the_secret_store() {
        let db_path = std::env::temp_dir().join(format!(
            "agnes-no-key-search-settings-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = crate::db::spawn_db_actor(db_path);
        persist_search_provider_settings(
            &db,
            &UnavailableSecretStore,
            SearchProviderSettingsInput {
                fallback_order: vec!["duckduckgo".into(), "bing".into()],
                searxng_base_url: None,
                brave_api_key: None,
                clear_brave_api_key: false,
            },
        )
        .await
        .unwrap();
        assert!(db
            .get_setting(crate::web::SEARCH_PROVIDER_SETTINGS_KEY.into())
            .await
            .unwrap()
            .is_some());
    }

    #[test]
    fn planner_updates_distinguish_omitted_and_null_fields() {
        let omitted: TaskUpdatePayload = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(omitted.description.is_none());
        assert!(omitted.due_date.is_none());

        let cleared: TaskUpdatePayload = serde_json::from_value(serde_json::json!({
            "description": null,
            "dueDate": null,
            "recurrenceRule": null
        }))
        .unwrap();
        assert_eq!(cleared.description, Some(None));
        assert_eq!(cleared.due_date, Some(None));
        assert_eq!(cleared.recurrence_rule, Some(None));

        let assigned: TaskUpdatePayload = serde_json::from_value(serde_json::json!({
            "dueDate": "2026-07-18"
        }))
        .unwrap();
        assert_eq!(assigned.due_date, Some(Some("2026-07-18".to_string())));

        let event: CalendarEventUpdatePayload =
            serde_json::from_value(serde_json::json!({ "recurrenceRule": null })).unwrap();
        assert_eq!(event.recurrence_rule, Some(None));
    }

    #[test]
    fn remote_knowledge_parser_uses_google_export_mime_without_an_extension() {
        let path =
            std::env::temp_dir().join(format!("agnes-google-document-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "# Roadmap\n\nDirect import content.").unwrap();
        let parsed = read_text_knowledge_document(
            path.to_string_lossy().to_string(),
            Some("Roadmap".into()),
            Some("application/vnd.google-apps.document".into()),
        )
        .unwrap();
        assert_eq!(parsed.title, "Roadmap");
        assert_eq!(parsed.media_type, "text/plain");
        assert!(parsed.size > 0);
        assert_eq!(parsed.chunks.len(), 1);
        assert_eq!(parsed.chunks[0].section_path.as_deref(), Some("Roadmap"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_knowledge_parser_normalizes_text_and_tracks_markdown_section() {
        let path = std::env::temp_dir().join(format!("agnes-text-{}.md", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            "\u{feff}# Notes\r\n\r\nFirst paragraph.\r\n\r\nSecond paragraph.",
        )
        .unwrap();

        let parsed =
            read_text_knowledge_document(path.to_string_lossy().to_string(), None, None).unwrap();

        assert_eq!(
            parsed.title,
            path.file_stem().unwrap().to_string_lossy().to_string()
        );
        assert_eq!(parsed.media_type, "text/markdown");
        assert_eq!(parsed.chunks.len(), 1);
        assert_eq!(parsed.chunks[0].section_path.as_deref(), Some("Notes"));
        assert!(!parsed.chunks[0].content.contains('\r'));
        assert!(parsed.chunks[0].content.contains("First paragraph."));
        assert!(parsed.chunks[0].content.contains("Second paragraph."));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_knowledge_parser_rejects_invalid_text_and_empty_documents() {
        let invalid =
            std::env::temp_dir().join(format!("agnes-invalid-text-{}", uuid::Uuid::new_v4()));
        std::fs::write(&invalid, [0xff, 0xfe]).unwrap();
        let error =
            match read_text_knowledge_document(invalid.to_string_lossy().to_string(), None, None) {
                Ok(_) => panic!("invalid UTF-8 should be rejected"),
                Err(error) => error.to_string(),
            };
        assert!(error.contains("UTF-8"));
        let _ = std::fs::remove_file(&invalid);

        let nul = std::env::temp_dir().join(format!("agnes-nul-text-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&nul, b"valid\0content").unwrap();
        let error =
            match read_text_knowledge_document(nul.to_string_lossy().to_string(), None, None) {
                Ok(_) => panic!("NUL content should be rejected"),
                Err(error) => error.to_string(),
            };
        assert!(error.contains("NUL"));
        let _ = std::fs::remove_file(&nul);

        let empty =
            std::env::temp_dir().join(format!("agnes-empty-text-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&empty, b"\r\n\n  \t").unwrap();
        let error =
            match read_text_knowledge_document(empty.to_string_lossy().to_string(), None, None) {
                Ok(_) => panic!("empty content should be rejected"),
                Err(error) => error.to_string(),
            };
        assert!(error.contains("没有可索引"));
        let _ = std::fs::remove_file(empty);
    }

    #[test]
    fn local_knowledge_parser_splits_large_paragraphs_with_metadata() {
        let path =
            std::env::temp_dir().join(format!("agnes-long-text-{}.md", uuid::Uuid::new_v4()));
        let content = format!("# Long\n\n{}", "word ".repeat(2_000));
        std::fs::write(&path, content).unwrap();

        let parsed =
            read_text_knowledge_document(path.to_string_lossy().to_string(), None, None).unwrap();

        assert!(parsed.chunks.len() > 1);
        assert!(parsed
            .chunks
            .iter()
            .all(|chunk| chunk.metadata == r#"{"kind":"text"}"#));
        assert!(parsed
            .chunks
            .iter()
            .all(|chunk| chunk.section_path.as_deref() == Some("Long")));
        assert!(parsed
            .chunks
            .iter()
            .all(|chunk| chunk.content.chars().count() <= super::KNOWLEDGE_CHUNK_SIZE));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn local_chat_attachment_reads_supported_utf8_text() {
        let path =
            std::env::temp_dir().join(format!("agnes-chat-attachment-{}.md", uuid::Uuid::new_v4()));
        std::fs::write(&path, "# Notes\r\n\r\nReference content.").unwrap();
        let (name, media_type, size, content) =
            read_local_chat_attachment(path.to_string_lossy().to_string()).unwrap();
        assert!(name.ends_with(".md"));
        assert_eq!(media_type, "text/markdown");
        assert!(size > 0);
        assert_eq!(content, "# Notes\n\nReference content.");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn managed_knowledge_source_install_supports_rollback_and_commit() {
        let directory =
            std::env::temp_dir().join(format!("agnes-managed-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let target = directory.join("source");
        let first = directory.join("first.staged");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&first, b"new").unwrap();
        let install = install_managed_file(&first, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        rollback_managed_file(install);
        assert_eq!(std::fs::read(&target).unwrap(), b"old");

        let second = directory.join("second.staged");
        std::fs::write(&second, b"committed").unwrap();
        let install = install_managed_file(&second, &target).unwrap();
        commit_managed_file(install);
        assert_eq!(std::fs::read(&target).unwrap(), b"committed");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
