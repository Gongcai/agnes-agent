use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::agent::protocol::{msg_type, Envelope};
use crate::db::repo::messages::{NewMessage, NewMessagePart};
use crate::db::repo::sessions::NewSession;

#[derive(Serialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
    pub persona: String,
    pub system_prompt: String,
    pub model: String,
    pub tool_policy: String,
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
            system_prompt: r.system_prompt,
            model: r.model,
            tool_policy: r.tool_policy,
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

/// 创建新会话。
#[tauri::command]
pub async fn create_session(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    title: String,
) -> AppResult<String> {
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
        })
        .collect())
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

    // 复用 send_message 的 LLM 配置解析逻辑
    let agent_model_str = if agent.model.is_empty() {
        "gpt-4o".to_string()
    } else {
        agent.model.clone()
    };
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

    let snapshot = json!({
        "input": "",
        "context": {
            "agent": {
                "persona": agent.persona,
                "systemPrompt": agent.system_prompt,
                "model": agent.model,
                "toolPolicy": serde_json::from_str::<serde_json::Value>(&agent.tool_policy).unwrap_or(json!({}))
            },
            "llmConfig": {
                "provider": provider_kind,
                "apiBase": api_base,
                "apiKey": api_key,
                "model": model_name,
                "litellmModel": litellm_model
            },
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
    let raw_msgs = state.db.list_messages_with_parts(session_id).await?;
    let mut msgs_dto = Vec::new();

    for (msg, parts) in raw_msgs {
        let mut parts_dto = Vec::new();
        for p in parts {
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
        });
    }

    Ok(msgs_dto)
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

/// 发送消息给 Agent，启动推理引擎运行（Tauri 主入口）。
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, AppState>,
    session_id: String,
    text: String,
) -> AppResult<()> {
    // 1. 获取会话与 Agent 详情
    let session = state.db.get_session(session_id.clone()).await?
        .ok_or_else(|| AppError::Other(format!("会话 `{session_id}` 不存在")))?;
        
    let agents = state.db.list_agents().await?;
    let agent = agents.iter().find(|a| a.id == session.agent_id)
        .ok_or_else(|| AppError::Other(format!("关联的智能体 `{}` 不存在", session.agent_id)))?;

    // 2. 解析 LLM 配置：provider_id/model_name 或仅 model_name（使用默认 provider）
    let agent_model_str = if agent.model.is_empty() { "gpt-4o".to_string() } else { agent.model.clone() };
    let (provider_id_opt, model_name) = if let Some(idx) = agent_model_str.find('/') {
        (Some(agent_model_str[..idx].to_string()), agent_model_str[idx + 1..].to_string())
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

    // 3. 插入用户发送的 Message
    let user_msg_id = uuid::Uuid::new_v4().to_string();
    let current_history = state.db.list_messages_with_parts(session_id.clone()).await?;
    let seq_user = current_history.len() as i32;
    
    let new_user_msg = NewMessage {
        id: user_msg_id.clone(),
        session_id: session_id.clone(),
        role: "user".into(),
        seq: seq_user,
        status: "complete".into(),
        model: None,
        token_count: None,
        metadata: None,
    };
    let new_user_part = NewMessagePart {
        id: uuid::Uuid::new_v4().to_string(),
        message_id: user_msg_id,
        kind: "text".into(),
        ordinal: 0,
        mime_type: None,
        tool_call_id: None,
        content: text.clone(),
        metadata: None,
    };
    state.db.insert_message(new_user_msg, vec![new_user_part]).await?;

    // 4. 插入一条 pending 状态的 Assistant 占位消息，供前台渲染 "Agnes 正在思考..."
    let assistant_msg_id = uuid::Uuid::new_v4().to_string();
    let new_assistant_msg = NewMessage {
        id: assistant_msg_id.clone(),
        session_id: session_id.clone(),
        role: "assistant".into(),
        seq: seq_user + 1,
        status: "pending".into(),
        model: Some(model_name.clone()),
        token_count: None,
        metadata: None,
    };
    state.db.insert_message(new_assistant_msg, vec![]).await?;

    // 5. 读取 USER.md 与 MEMORY.md 内存 (SQLite canonical)
    let (user_md, memory_md) = load_explicit_memories_db_backed(&state.db, &session.agent_id).await?;

    // 6. 编译 ContextSnapshot
    // 我们再次拉取完整的历史记录（此时已含最新的 user 消息，不含 pending assistant）
    let updated_history = state.db.list_messages_with_parts(session_id.clone()).await?;
    let mut history_json = Vec::new();
    for (m, parts) in updated_history {
        // 跳过我们刚才插入的那个 pending assistant 占位消息，只把完整消息发给 Python
        if m.id == assistant_msg_id {
            continue;
        }
        
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

    let run_id = uuid::Uuid::new_v4().to_string();

    let context_snapshot = json!({
        "input": text,
        "context": {
            "agent": {
                "persona": agent.persona,
                "systemPrompt": agent.system_prompt,
                "model": agent.model,
                "toolPolicy": serde_json::from_str::<serde_json::Value>(&agent.tool_policy).unwrap_or(json!({}))
            },
            "llmConfig": {
                "provider": provider_kind,
                "apiBase": api_base,
                "apiKey": api_key,
                "model": model_name,
                "litellmModel": litellm_model
            },
            "settings": {
                "user_context_limit": session.context_limit
            },
            "recentMessages": history_json,
            "summary": session.summary,
            "explicitMemories": {
                "user_md": user_md,
                "memory_md": memory_md
            },
            "retrievedMemories": [],
            "projectContext": []
        }
    });

    // 6. 构造 RUN_REQUEST 协议信封并发送
    let run_req = Envelope {
        protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        run_id,
        session_id,
        msg_type: msg_type::RUN_REQUEST.to_string(),
        created_at: String::new(),
        payload: context_snapshot,
    };

    // 路由分发
    state.agent.send_to_agent(run_req)
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
    pub models: Vec<String>,
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
    pub models: Option<Vec<String>>,
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
        let models: Vec<String> = r
            .models_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
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

    let models_json = provider
        .models
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "[]".to_string()));

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
    Ok(())
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
) -> AppResult<Vec<String>> {
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
                    models.push(id.to_string());
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
                    models.push(name.to_string());
                }
            }
        } else {
            return Err(AppError::Other("未在响应中找到模型数据".into()));
        }
    } else {
        return Err(AppError::Other(format!("暂不支持自动获取 {} 的模型列表，请手动输入", kind)));
    }

    Ok(models)
}
