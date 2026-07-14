use serde::Serialize;
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
}

#[derive(Serialize)]
pub struct SessionDto {
    pub id: String,
    pub agent_id: String,
    pub title: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
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
        })
        .collect())
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

    // 2. 插入用户发送的 Message
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

    // 3. 插入一条 pending 状态的 Assistant 占位消息，供前台渲染 "Agnes 正在思考..."
    let assistant_msg_id = uuid::Uuid::new_v4().to_string();
    let new_assistant_msg = NewMessage {
        id: assistant_msg_id.clone(),
        session_id: session_id.clone(),
        role: "assistant".into(),
        seq: seq_user + 1,
        status: "pending".into(),
        model: Some(if agent.model.is_empty() { "gpt-4o".to_string() } else { agent.model.clone() }),
        token_count: None,
        metadata: None,
    };
    state.db.insert_message(new_assistant_msg, vec![]).await?;

    // 4. 从磁盘读取 USER.md 与 MEMORY.md 内存
    let (user_md, memory_md) = crate::memory::load_explicit_memories(&session.agent_id)?;

    // 5. 编译 ContextSnapshot
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
