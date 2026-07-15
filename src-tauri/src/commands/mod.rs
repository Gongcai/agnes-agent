use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::agent::protocol::{msg_type, Envelope};
use crate::db::repo::messages::{NewMessage, NewMessagePart};
use crate::db::repo::agents::{AgentRow, AgentUpdate, NewAgent};
use crate::db::repo::sessions::NewSession;
use crate::model_registry::{
    descriptor_from_api, load_model_roles, normalize_model_catalog, parse_model_catalog,
    save_model_roles, ModelDescriptor, ModelRoleAssignments,
};

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
    pub kind: String, // "text" | "thought" | "tool_call" | "tool_result"
    pub content: String,
    pub tool_call: Option<ToolCallDto>,
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
    state.db.update_agent_model(agent_id, model).await
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
            state.db.insert_agent(row).await
        }
    }
}

/// 删除角色卡（含其依赖的会话与消息）。
#[tauri::command]
pub async fn delete_agent(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<()> {
    state.db.delete_agent(agent_id).await
}

/// 创建新会话。
#[tauri::command]
pub async fn create_session(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    title: String,
    workspace_id: Option<String>,
) -> AppResult<String> {
    // 新会话沿用角色卡的默认模型与思考配置，输入框可随后覆盖
    let agents = state.db.list_agents().await?;
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
        reserved_output_tokens: None,
        summarizer_model: None,
        model: if default_model.is_empty() { None } else { Some(default_model) },
        thinking_mode: if default_thinking_mode.is_empty() { None } else { Some(default_thinking_mode) },
        thinking_budget: if default_thinking_budget == 0 { None } else { Some(default_thinking_budget) },
        permission_mode: "auto".to_string(),
        workspace_id,
        origin_device_id: None,
    };
    state.db.insert_session(new_sess).await?;
    Ok(session_id)
}

/// 列出某个 Agent 的所有会话。
#[tauri::command]
pub async fn list_sessions(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> AppResult<Vec<SessionDto>> {
    let rows = state.db.list_sessions(agent_id).await?;
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
    state.db.insert_workspace(crate::db::repo::workspaces::NewWorkspace {
        id: id.clone(),
        agent_id,
        name,
        folder_path,
    }).await?;
    Ok(id)
}

/// 重命名工作区。
#[tauri::command]
pub async fn rename_workspace(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
    name: String,
) -> AppResult<()> {
    state.db.rename_workspace(workspace_id, name).await
}

/// 删除工作区（会话的 workspace_id 置 NULL，会话保留为普通对话）。
#[tauri::command]
pub async fn delete_workspace(
    state: tauri::State<'_, AppState>,
    workspace_id: String,
) -> AppResult<()> {
    state.db.delete_workspace(workspace_id).await
}

/// 设置会话级模型与思考配置（输入框切换模型 / 思考强度时调用）。
#[tauri::command]
pub async fn set_session_llm(
    state: tauri::State<'_, AppState>,
    session_id: String,
    model: String,
    thinking_mode: String,
    thinking_budget: i64,
) -> AppResult<()> {
    state
        .db
        .update_session_llm(session_id, model, thinking_mode, thinking_budget)
        .await
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
    state.db.delete_session(session_id).await
}

/// 置顶或取消置顶某个会话。
#[tauri::command]
pub async fn set_session_pin(
    state: tauri::State<'_, AppState>,
    session_id: String,
    pinned: bool,
) -> AppResult<()> {
    state.db.set_session_pin(session_id, pinned).await
}

/// 重命名某个会话。
#[tauri::command]
pub async fn rename_session(
    state: tauri::State<'_, AppState>,
    session_id: String,
    title: String,
) -> AppResult<()> {
    state.db.update_session_title(session_id, title.trim().to_string()).await
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
    let session_model = session_opt.as_ref().and_then(|s| s.model.clone()).unwrap_or_default();
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
        let m = session_opt.as_ref()
            .and_then(|s| s.thinking_mode.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| agent.thinking_mode.clone());
        if m.is_empty() { "off".to_string() } else { m }
    };
    let effective_thinking_budget = session_opt.as_ref()
        .and_then(|s| s.thinking_budget)
        .or(Some(agent.thinking_budget))
        .unwrap_or(0);

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
        state.db.get_setting(format!("provider:{}:api_key", pid)).await?
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
    let (user_md, memory_md) = load_explicit_memories_db_backed(&state.db, &agent.id).await?;

    // 读取会话历史与摘要（如有）
    let mut history_json = Vec::new();
    let mut summary = None;
    if let Some(ref sid) = session_id {
        if let Ok(Some(sess)) = state.db.get_session(sid.clone()).await {
            summary = sess.summary.clone();
        }
        if let Ok(history) = state.db.list_messages_with_parts(sid.clone()).await {
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

    let task_llm_configs = resolve_task_llm_configs(&state.db, &model_roles).await?;
    let snapshot = json!({
        "input": "",
        "context": {
            "agent": {
                "persona": agent.persona,
                "systemPrompt": agent.system_prompt,
                "model": agent_model_str,
                "toolPolicy": serde_json::from_str::<serde_json::Value>(&agent.tool_policy).unwrap_or(json!({}))
            },
            "llmConfig": {
                "provider": provider_kind,
                "apiBase": api_base,
                "apiKey": api_key,
                "model": model_name,
                "litellmModel": litellm_model,
                "thinking": {
                    "mode": effective_thinking_mode,
                    "budget": effective_thinking_budget
                }
            },
            "taskLlmConfigs": task_llm_configs,
            "settings": { "user_context_limit": serde_json::Value::Null },
            "recentMessages": history_json,
            "summary": summary,
            "explicitMemories": { "user_md": user_md, "memory_md": memory_md },
            "retrievedMemories": [],
            "projectContext": []
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
        state.agent.resolve_debug(&debug_id, serde_json::json!({ "error": e.to_string() }));
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
            return Err(AppError::Other("调试提示词拼装超时（Python sidecar 未响应）".into()));
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
    let discarded = payload
        .get("discarded_messages")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(DebugPromptDto {
        system_prompt,
        messages,
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
        let mut parts_dto = Vec::new();
        for p in am.parts {
            let mut tool_call_dto = None;
            if let Some(ref tc_id) = p.tool_call_id {
                if let Ok(Some(tc_row)) = state.db.get_tool_call(tc_id.clone()).await {
                    tool_call_dto = Some(ToolCallDto {
                        id: tc_row.id,
                        tool: tc_row.tool,
                        args: tc_row.params.unwrap_or_default(),
                        risk: tc_row.risk_level.unwrap_or_else(|| "Low".to_string()),
                        status: tc_row.status,
                        output: tc_row.stdout.or(tc_row.stderr),
                    });
                }
            }

            // 将数据库的 "reasoning" 种类翻译为前端的 "thought"
            let kind_mapped = if p.kind == "reasoning" {
                "thought".to_string()
            } else {
                p.kind
            };

            parts_dto.push(MessagePartDto {
                id: p.id,
                kind: kind_mapped,
                content: p.content,
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
        });
    }

    Ok(msgs_dto)
}

/// 切换某消息的版本（prev/next 同级）。同级共享 parent_id；切换即把父的
/// selected_child_id 指向目标同级，活动路径随之改走该同级的子树。
#[tauri::command]
pub async fn switch_version(
    state: tauri::State<'_, AppState>,
    message_id: String,
    direction: String, // "prev" | "next"
) -> AppResult<()> {
    let msg = state.db.get_message(message_id.clone()).await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    let parent_id = msg.parent_id.clone()
        .ok_or_else(|| AppError::Other("根消息无同级版本".into()))?;
    let all = state.db.list_messages_with_parts(msg.session_id.clone()).await?;
    // 同级：parent_id 相同，按 seq 升序（list 已排序）
    let siblings: Vec<String> = all.iter()
        .filter(|(m, _)| m.parent_id.as_deref() == Some(parent_id.as_str()))
        .map(|(m, _)| m.id.clone())
        .collect();
    let idx = siblings.iter().position(|id| id == &message_id)
        .ok_or_else(|| AppError::Other("找不到当前消息".into()))?;
    let new_idx = match direction.as_str() {
        "prev" => if idx == 0 { return Ok(()); } else { idx - 1 },
        "next" => if idx + 1 >= siblings.len() { return Ok(()); } else { idx + 1 },
        _ => return Ok(()),
    };
    let new_id = siblings[new_idx].clone();
    state.db.set_selected_child(parent_id, Some(new_id)).await
}

/// 创建分支：把该消息设为活动叶子（selected_child_id=NULL），其后代保留为
/// 可切回的同级；下次发消息即从该点长新枝。
#[tauri::command]
pub async fn create_branch(
    state: tauri::State<'_, AppState>,
    message_id: String,
) -> AppResult<()> {
    state.db.set_selected_child(message_id, None).await
}

/// 删除单条消息（仅叶子、非 pending/streaming）。若是父的 selected_child，
/// 把父的 selected_child 置 NULL，活动路径回退到父。
#[tauri::command]
pub async fn delete_message(
    state: tauri::State<'_, AppState>,
    message_id: String,
) -> AppResult<()> {
    let msg = state.db.get_message(message_id.clone()).await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.status == "pending" || msg.status == "streaming" {
        return Err(AppError::Other("消息正在生成，无法删除".into()));
    }
    let cnt = state.db.count_children(message_id.clone()).await?;
    if cnt > 0 {
        return Err(AppError::Other("仅可删除末梢消息（无后续消息）".into()));
    }
    if let Some(ref pid) = msg.parent_id {
        if let Some(p) = state.db.get_message(pid.clone()).await? {
            if p.selected_child_id.as_deref() == Some(message_id.as_str()) {
                state.db.set_selected_child(pid.clone(), None).await?;
            }
        }
    }
    state.db.delete_message(message_id).await
}

/// 编辑并重发：把编辑后的文本作为旧 user 消息的同级新版本插入（共享 parent），
/// 切换活动路径到新 user，造新 pending assistant，启动运行。旧 user 子树保留可切回。
#[tauri::command]
pub async fn edit_and_resend(
    state: tauri::State<'_, AppState>,
    message_id: String,
    text: String,
) -> AppResult<()> {
    let msg = state.db.get_message(message_id.clone()).await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.role != "user" {
        return Err(AppError::Other("仅可编辑用户消息".into()));
    }
    let cfg = resolve_llm(&state, &msg.session_id).await?;
    let parent_id = msg.parent_id.clone();
    let (_user_id, assistant_msg_id) = state.db
        .append_user_and_assistant(msg.session_id.clone(), parent_id, text, cfg.model_name.clone())
        .await?;
    start_agent_run(&state, &cfg, &msg.session_id, &assistant_msg_id).await
}

/// 单条重新生成：为 AI 消息造一个同级新版本（共享父 user 消息），切换活动路径
/// 到新 AI，启动运行。旧 AI 子树保留可切回。
#[tauri::command]
pub async fn regenerate_message(
    state: tauri::State<'_, AppState>,
    message_id: String,
) -> AppResult<()> {
    let msg = state.db.get_message(message_id.clone()).await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.role != "assistant" {
        return Err(AppError::Other("仅可重新生成 AI 消息".into()));
    }
    let parent_user_id = msg.parent_id.clone()
        .ok_or_else(|| AppError::Other("AI 消息无父用户消息".into()))?;
    let cfg = resolve_llm(&state, &msg.session_id).await?;
    let assistant_msg_id = state.db
        .append_assistant_sibling(msg.session_id.clone(), parent_user_id, cfg.model_name.clone())
        .await?;
    start_agent_run(&state, &cfg, &msg.session_id, &assistant_msg_id).await
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
    let msg = state.db.get_message(message_id.clone()).await?
        .ok_or_else(|| AppError::Other("消息不存在".into()))?;
    if msg.role != "assistant" {
        return Err(AppError::Other("仅可修改 AI 消息".into()));
    }
    if msg.status != "complete" {
        return Err(AppError::Other("仅可修改已完成的 AI 消息".into()));
    }
    let new_parts: Vec<NewMessagePart> = parts.into_iter().map(|p| NewMessagePart {
        id: uuid::Uuid::new_v4().to_string(),
        message_id: message_id.clone(),
        kind: p.kind,
        ordinal: 0, // actor handler 会按顺序重排
        mime_type: None,
        tool_call_id: p.tool_call_id,
        content: p.content,
        metadata: p.metadata,
    }).collect();
    state.db.replace_message_parts(message_id, new_parts).await
}

/// 取消某会话当前活跃的运行：取出 session→run_id 映射，向 Python 发 RUN_CANCEL。
/// 同时触发挂起审批的取消信号，解除 ws_server 在 approval oneshot 上的阻塞
/// （否则 ws 循环卡死，RUN_ERROR 与后续消息都无法处理）。Python 收到后取消对应任务。
#[tauri::command]
pub async fn cancel_run(
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> AppResult<()> {
    if let Some(run_id) = state.agent.remove_session_run(&session_id) {
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
fn normalize_memory_text(text: String) -> String {
    let t = text.trim();
    if t.is_empty()
        || t == "# USER.md"
        || t == "# MEMORY.md"
        || t.contains("在此输入您的基础个人画像")
        || t.contains("在此记录助手每次对话沉淀的事实")
    {
        return String::new();
    }
    text
}

/// 从 DB 中加载 explicit memories（如果不存在则从磁盘加载并写入 DB 作为 canonical 真相源）。
async fn load_explicit_memories_db_backed(
    db: &crate::db::DbActorHandle,
    agent_id: &str,
) -> AppResult<(String, String)> {
    let user_key = format!("agent:{}:user_md", agent_id);
    let memory_key = format!("agent:{}:memory_md", agent_id);

    let db_user = db.get_setting(user_key.clone()).await?;
    let db_memory = db.get_setting(memory_key.clone()).await?;

    if let (Some(u), Some(m)) = (db_user, db_memory) {
        // 自动同步写回磁盘 materialized view md 文件以防手改丢失；
        // 占位提示视为空，避免被当作记忆发送给 AI
        let u_norm = normalize_memory_text(u);
        let m_norm = normalize_memory_text(m);
        let _ = crate::memory::save_explicit_memories(agent_id, &u_norm, &m_norm);
        Ok((u_norm, m_norm))
    } else {
        // 磁盘作为 fallback 并生成默认值
        let (user_md, memory_md) = crate::memory::load_explicit_memories(agent_id)?;
        let user_md = normalize_memory_text(user_md);
        let memory_md = normalize_memory_text(memory_md);
        let _ = db.set_setting(user_key, user_md.clone()).await;
        let _ = db.set_setting(memory_key, memory_md.clone()).await;
        Ok((user_md, memory_md))
    }
}

/// 解析出的 LLM 运行配置（会话级优先，回退角色卡默认）。
struct ResolvedLlm {
    agent: AgentRow,
    session_context_limit: Option<i64>,
    session_summary: Option<String>,
    effective_model: String,
    model_name: String,
    provider_kind: String,
    api_base: Option<String>,
    api_key: Option<String>,
    litellm_model: String,
    thinking_mode: String,
    thinking_budget: i64,
    task_llm_configs: serde_json::Value,
}

/// Resolve a routed model reference into the same provider configuration used by the main model.
async fn resolve_routed_llm_config(
    db: &crate::db::DbActorHandle,
    model_ref: Option<&str>,
) -> AppResult<Option<serde_json::Value>> {
    let Some(model_ref) = model_ref.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some((provider_id, model_name)) = model_ref.split_once('/') else {
        return Ok(None);
    };
    let Some(provider) = db.get_model_provider(provider_id.to_string()).await? else {
        return Ok(None);
    };
    let api_key = db
        .get_setting(format!("provider:{}:api_key", provider.id))
        .await?;
    let litellm_model = match provider.kind.as_str() {
        "openai" | "anthropic" => model_name.to_string(),
        "ollama" => format!("ollama/{model_name}"),
        "openai_compatible" => format!("openai/{model_name}"),
        "google" => format!("gemini/{model_name}"),
        _ => model_name.to_string(),
    };
    Ok(Some(json!({
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
    db: &crate::db::DbActorHandle,
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
        if let Some(config) = resolve_routed_llm_config(db, selection).await? {
            configs.insert(key.to_string(), config);
        }
    }
    Ok(serde_json::Value::Object(configs))
}

/// 解析会话当前生效的 LLM 配置（模型/思考/provider/密钥）。
async fn resolve_llm(state: &AppState, session_id: &str) -> AppResult<ResolvedLlm> {
    let session = state.db.get_session(session_id.to_string()).await?
        .ok_or_else(|| AppError::Other(format!("会话 `{session_id}` 不存在")))?;
    let agents = state.db.list_agents().await?;
    let agent = agents.iter().find(|a| a.id == session.agent_id)
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
        (Some(effective_model[..idx].to_string()), effective_model[idx + 1..].to_string())
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
        state.db.get_setting(format!("provider:{}:api_key", pid)).await?
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
    let thinking_mode = {
        let m = session.thinking_mode.clone().filter(|s| !s.is_empty())
            .unwrap_or_else(|| agent.thinking_mode.clone());
        if m.is_empty() { "off".to_string() } else { m }
    };
    let thinking_budget = session.thinking_budget.or(Some(agent.thinking_budget)).unwrap_or(0);

    let task_llm_configs = resolve_task_llm_configs(&state.db, &model_roles).await?;
    Ok(ResolvedLlm {
        agent,
        session_context_limit: session.context_limit,
        session_summary: session.summary,
        effective_model,
        model_name,
        provider_kind,
        api_base,
        api_key,
        litellm_model,
        thinking_mode,
        thinking_budget,
        task_llm_configs,
    })
}

/// 用已解析配置启动一次 agent 运行：构建活动路径历史（跳过 pending assistant）→
/// 编译 snapshot（input="" 避免与 recentMessages 重复）→ 注册 run_id → 发 RUN_REQUEST。
async fn start_agent_run(state: &AppState, cfg: &ResolvedLlm, session_id: &str, assistant_msg_id: &str) -> AppResult<()> {
    let (user_md, memory_md) = load_explicit_memories_db_backed(&state.db, &cfg.agent.id).await?;

    let path = state.db.list_active_with_parts(session_id.to_string()).await?;
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

    let run_id = uuid::Uuid::new_v4().to_string();
    let context_snapshot = json!({
        "input": "",
        "context": {
            "agent": {
                "persona": cfg.agent.persona,
                "systemPrompt": cfg.agent.system_prompt,
                "model": cfg.effective_model,
                "toolPolicy": serde_json::from_str::<serde_json::Value>(&cfg.agent.tool_policy).unwrap_or(json!({}))
            },
            "llmConfig": {
                "provider": cfg.provider_kind,
                "apiBase": cfg.api_base,
                "apiKey": cfg.api_key,
                "model": cfg.model_name,
                "litellmModel": cfg.litellm_model,
                "thinking": {
                    "mode": cfg.thinking_mode,
                    "budget": cfg.thinking_budget
                }
            },
            "taskLlmConfigs": cfg.task_llm_configs,
            "settings": {
                "user_context_limit": cfg.session_context_limit
            },
            "recentMessages": history_json,
            "summary": cfg.session_summary,
            "explicitMemories": {
                "user_md": user_md,
                "memory_md": memory_md
            },
            "retrievedMemories": [],
            "projectContext": []
        }
    });

    // 显式注册 run_id → assistant_msg_id，供 ws_server 精确定位 pending 消息
    state.agent.register_run(run_id.clone(), assistant_msg_id.to_string());
    // 记录 session_id → run_id，供 cancel_run 按会话取消
    state.agent.set_session_run(session_id.to_string(), run_id.clone());

    let run_req = Envelope {
        protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        run_id,
        session_id: session_id.to_string(),
        msg_type: msg_type::RUN_REQUEST.to_string(),
        created_at: String::new(),
        payload: context_snapshot,
    };

    state.agent.send_to_agent(run_req)
}

/// 发送消息给 Agent，启动推理引擎运行（Tauri 主入口）。
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    session_id: String,
    text: String,
) -> AppResult<()> {
    let cfg = resolve_llm(&state, &session_id).await?;

    // 取当前活动叶子作为新 user 消息的 parent（首条消息时为 None）
    let path = state.db.list_active_with_parts(session_id.clone()).await?;
    let leaf_id = path.messages.last().map(|am| am.message.id.clone());

    // 原子插入 user + pending assistant 并链接版本树
    let (_user_id, assistant_msg_id) = state.db
        .append_user_and_assistant(session_id.clone(), leaf_id, text, cfg.model_name.clone())
        .await?;

    start_agent_run(&state, &cfg, &session_id, &assistant_msg_id).await
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
        Ok(())
    } else {
        Err(AppError::Other(format!("审批 ID `{tool_call_id}` 未找到或已失效")))
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
    let (user_md, memory_md) = load_explicit_memories_db_backed(&state.db, &agent_id).await?;
    Ok(ExplicitMemoriesDto { user_md, memory_md })
}

#[tauri::command]
pub async fn save_explicit_memories(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    user_md: String,
    memory_md: String,
) -> AppResult<()> {
    let user_key = format!("agent:{}:user_md", agent_id);
    let memory_key = format!("agent:{}:memory_md", agent_id);

    // 1. 写入 SQLite canonical 真相源
    state.db.set_setting(user_key, user_md.clone()).await?;
    state.db.set_setting(memory_key, memory_md.clone()).await?;

    // 2. 更新本地磁盘 materialized view md 视图
    crate::memory::save_explicit_memories(&agent_id, &user_md, &memory_md)
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
    Ok(rows.into_iter().map(|r| AuditLogDto {
        id: r.id,
        time: r.created_at,
        tool: r.tool,
        params: r.params.unwrap_or_default(),
        status: r.status,
        risk: r.risk_level.unwrap_or_else(|| "Low".to_string()),
    }).collect())
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

/// 列出所有已配置的模型提供商。
#[tauri::command]
pub async fn list_providers(
    state: tauri::State<'_, AppState>,
) -> AppResult<Vec<ModelProviderDto>> {
    let rows = state.db.list_model_providers().await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let models = parse_model_catalog(r.models_json.as_deref(), &r.kind);
        // 出于安全不返回明文 key，仅判断是否已配置
        let stored_key = state
            .db
            .get_setting(format!("provider:{}:api_key", r.id))
            .await
            .ok()
            .flatten();
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
    let id = provider.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // 存储 API Key 到 settings 表
    if let Some(ref key) = provider.api_key {
        state
            .db
            .set_setting(format!("provider:{}:api_key", id), key.clone())
            .await?;
    }

    let normalized_models = normalize_model_catalog(provider.models.unwrap_or_default(), &provider.kind);
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

    state.db.upsert_model_provider(new_row, set_default).await?;
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
    state.db.delete_model_provider(provider_id.clone()).await?;
    // 清理 settings 中的 API Key
    state
        .db
        .set_setting(format!("provider:{}:api_key", provider_id), String::new())
        .await?;
    let mut roles = load_model_roles(&state.db).await?;
    roles.clear_provider(&provider_id);
    save_model_roles(&state.db, &roles).await?;
    Ok(())
}

/// Return the global model role assignments.
#[tauri::command]
pub async fn get_model_roles(
    state: tauri::State<'_, AppState>,
) -> AppResult<ModelRoleAssignments> {
    load_model_roles(&state.db).await
}

/// Validate and persist the global model role assignments.
#[tauri::command]
pub async fn set_model_roles(
    state: tauri::State<'_, AppState>,
    roles: ModelRoleAssignments,
) -> AppResult<()> {
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
    save_model_roles(&state.db, &roles).await
}

/// 获取已保存的 API Key 明文（本地应用，仅供设置界面回显，默认以密码形式展示）。
#[tauri::command]
pub async fn get_provider_api_key(
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> AppResult<Option<String>> {
    Ok(state
        .db
        .get_setting(format!("provider:{}:api_key", provider_id))
        .await?
        .filter(|k| !k.is_empty()))
}

/// 测试模型提供商配置是否有效（V0.1 仅检查是否存在及 API Key 已配置）。
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
            // 对于 ollama 无需 API Key
            if p.kind == "ollama" {
                return Ok(TestProviderResult {
                    success: true,
                    message: "Ollama provider configured (no API key required)".into(),
                });
            }
            let api_key = state
                .db
                .get_setting(format!("provider:{}:api_key", provider_id))
                .await?;
            match api_key {
                Some(k) if !k.is_empty() => Ok(TestProviderResult {
                    success: true,
                    message: format!("Provider `{}` configured with API key", p.name),
                }),
                _ => Ok(TestProviderResult {
                    success: false,
                    message: format!("Provider `{}` has no API key configured", p.name),
                }),
            }
        }
    }
}

/// 从服务端自动获取可用模型列表
#[tauri::command]
pub async fn fetch_provider_models(
    kind: String,
    api_base: Option<String>,
    api_key: Option<String>,
) -> AppResult<Vec<ModelDescriptor>> {
    let client = reqwest::Client::new();
    let mut models = Vec::new();

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

        let resp = req.send().await.map_err(|e| AppError::Other(format!("请求失败: {}", e)))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| AppError::Other(format!("解析JSON失败: {}", e)))?;
        
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
        
        let resp = client.get(&url).send().await.map_err(|e| AppError::Other(format!("请求失败: {}", e)))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| AppError::Other(format!("解析JSON失败: {}", e)))?;
        
        if let Some(models_arr) = json.get("models").and_then(|m| m.as_array()) {
            for item in models_arr {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    let metadata = match client
                        .post(format!("{}/api/show", base.trim_end_matches('/')))
                        .json(&json!({ "model": name }))
                        .send()
                        .await
                    {
                        Ok(response) => response.json::<serde_json::Value>().await.unwrap_or_else(|_| item.clone()),
                        Err(_) => item.clone(),
                    };
                    models.push(descriptor_from_api(&kind, name, &metadata));
                }
            }
        } else {
            return Err(AppError::Other("未在响应中找到模型数据".into()));
        }
    } else {
        return Err(AppError::Other(format!("暂不支持自动获取 {} 的模型列表，请手动输入", kind)));
    }

    Ok(normalize_model_catalog(models, &kind))
}

/// 读取一个 settings 键值（供前端持久化 UI 状态，如上次选中的 agent/session）。
#[tauri::command]
pub async fn get_setting(
    state: tauri::State<'_, AppState>,
    key: String,
) -> AppResult<Option<String>> {
    state.db.get_setting(key).await
}

/// 写入一个 settings 键值。
#[tauri::command]
pub async fn set_setting(
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) -> AppResult<()> {
    state.db.set_setting(key, value).await
}
