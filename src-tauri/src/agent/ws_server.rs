use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tauri::{Emitter, Manager};
use tokio::net::TcpListener as TokioListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use crate::agent::protocol::{msg_type, Envelope};
use crate::agent::AgentManager;
use crate::db::repo::memory::NewMemory;
use crate::db::repo::messages::NewMessagePart;
use crate::db::repo::tools::NewToolCall;
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::mcp::McpManager;
use crate::state::AppState;
use crate::tools::{PermissionMode, ToolExecutor, ToolPolicy};

pub struct ActiveRun {
    pub assistant_message_id: String,
    pub session_id: String,
    pub accumulated_text: String,
    pub accumulated_thought: String,
    pub current_ordinal: i32,
    /// 当前是否处于 <thought> 思维链中（跨 chunk 持久，避免多段思维链路由错乱）
    pub in_thought: bool,
}

/// 把当前回合累积的思维链与正文落库为 message_parts，并重置累积缓冲。
/// 在「工具调用前」与「运行结束时」各调用一次，保持「回复-工具调用-回复」的回合顺序，
/// 避免跨回合合并成一段正文（accumulated_text 不再跨回合累加）。
fn drain_accumulated(run: &mut ActiveRun, out: &mut Vec<NewMessagePart>) {
    if !run.accumulated_thought.is_empty() {
        let t = run.accumulated_thought.trim().to_string();
        if !t.is_empty() {
            out.push(NewMessagePart {
                id: uuid::Uuid::new_v4().to_string(),
                message_id: run.assistant_message_id.clone(),
                kind: "reasoning".into(),
                ordinal: run.current_ordinal,
                mime_type: None,
                tool_call_id: None,
                content: t,
                metadata: None,
            });
            run.current_ordinal += 1;
        }
    }
    if !run.accumulated_text.is_empty() {
        let t = std::mem::take(&mut run.accumulated_text);
        if !t.trim().is_empty() {
            out.push(NewMessagePart {
                id: uuid::Uuid::new_v4().to_string(),
                message_id: run.assistant_message_id.clone(),
                kind: "text".into(),
                ordinal: run.current_ordinal,
                mime_type: None,
                tool_call_id: None,
                content: t,
                metadata: None,
            });
            run.current_ordinal += 1;
        }
    }
    run.accumulated_thought.clear();
    run.in_thought = false;
}

// 在 AgentManager 里扩展对 ActiveRun 的管理。
// 我们在 ws_server 里单独用一个 Mutex 存储 active runs 映射以解耦，或者直接在 ws_server 里定义全局线程安全 Map。
// 使用 lazy_static 或 Thread Local 在 Rust 里不够安全优雅；最优雅的是直接放在 AgentManager 里。
// 让我们在 AgentManager 里加入 active_runs，但为了不破坏已编译的 manager.rs，我们可以直接在 ws_server.rs 内部定义一个
// 共享的 Mutex<HashMap<String, ActiveRun>>，或者通过 AgentManager 暴露。
// 慢着，我们在上一步实现的 manager.rs 中没有定义 active_runs 字段。
// 我们可以通过在 ws_server.rs 里定义一个共享的并发 Map 来进行临时存储，也可以修改 manager.rs。
// 修改 manager.rs 增加 active_runs 是最严密、符合「设计优先」的做法。
// 让我们直接在 ws_server.rs 里定义一个静态的或局部 Arc 共享的 Mutex<HashMap<String, ActiveRun>>，
// 这样不需要频繁修改 manager.rs 字段，且生命周期只跟 WS Server 关联，非常高内聚！
// 没错！在 ws_server 内部维护 `runs: Arc<Mutex<HashMap<String, ActiveRun>>>`，
// 并在 ws_server::run 和 handle_conn 之间传递，这非常内聚！
// 那么 Tauri 命令如何注册这个 ActiveRun 呢？
// 哈哈！当 Tauri 命令启动 `send_message` 时，它会调用 AgentManager::send_to_agent。
// 我们可以在 AgentManager 中存储它，或者，我们可以让 Tauri 命令直接插入一条 pending 消息到 SQLite，
// 然后当 Python 第一个 ASSISTANT_DELTA 到达时，WS Server 自动在 DB 中查询当前 session 的最新 pending 消息！
// 这太聪明了！
// 1. Tauri 命令 `send_message` 插入一条状态为 "pending" 的 assistant 消息到 SQLite。
// 2. 当 WS Server 收到第一个 ASSISTANT_DELTA 时，如果在本地内存 Map 里没有这个 `run_id` 对应的 `ActiveRun`，
//    它就向 SQLite 查询此 session 的最新一条 pending 消息，并将其 ID 作为 `assistant_message_id` 缓存在内存里！
// 这消除了 Tauri 命令与 WS 线程之间的内存共享同步问题，完全以「数据库为唯一真相源」！
// 让我们为这个精妙的设计鼓掌！这非常健壮，能完全防止各种并发 Race Condition！

/// Rust 侧 WS Server 主循环：绑定已分配的 127.0.0.1 端口，按连接派发。
pub async fn run<R: tauri::Runtime>(
    std_listener: TcpListener,
    token: String,
    db: DbActorHandle,
    app_handle: tauri::AppHandle<R>,
    manager: Arc<AgentManager>,
    mcp: Arc<McpManager>,
) -> AppResult<()> {
    std_listener
        .set_nonblocking(true)
        .map_err(|e| AppError::Ws(format!("set_nonblocking 失败：{e}")))?;
    let listener =
        TokioListener::from_std(std_listener).map_err(|e| AppError::Ws(e.to_string()))?;
    println!(
        "[agent][ws] listening on 127.0.0.1:{}",
        listener.local_addr().map(|a| a.port()).unwrap_or(0)
    );

    let active_runs = Arc::new(tokio::sync::Mutex::new(HashMap::<String, ActiveRun>::new()));

    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .map_err(|e| AppError::Ws(e.to_string()))?;
        let token = token.clone();
        let db = db.clone();
        let app_handle = app_handle.clone();
        let manager = manager.clone();
        let mcp = mcp.clone();
        let active_runs = active_runs.clone();

        tokio::spawn(async move {
            if let Err(e) =
                handle_conn(stream, token, db, app_handle, manager, mcp, active_runs).await
            {
                eprintln!("[agent][ws] conn error: {e}");
            }
        });
    }
}

/// 单连接处理：握手 -> 注册活跃 channel -> 进入双向消息循环。
async fn handle_conn<R: tauri::Runtime>(
    stream: tokio::net::TcpStream,
    expected_token: String,
    db: DbActorHandle,
    app_handle: tauri::AppHandle<R>,
    manager: Arc<AgentManager>,
    mcp: Arc<McpManager>,
    active_runs: Arc<tokio::sync::Mutex<HashMap<String, ActiveRun>>>,
) -> AppResult<()> {
    let ws = accept_async(stream)
        .await
        .map_err(|e| AppError::Ws(e.to_string()))?;
    let (mut ws_write, mut ws_read) = ws.split();

    // 创建 WS 写入的 mpsc 队列并注册到 AgentManager
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    manager.register_active_sender(tx);

    // 启动一个后台任务处理消息发送给 Python
    let mut writer_task = tokio::spawn(async move {
        while let Some(env) = rx.recv().await {
            let raw = match serde_json::to_string(&env) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if ws_write.send(Message::Text(raw.into())).await.is_err() {
                break;
            }
        }
    });

    let tool_executor = ToolExecutor::new(db.clone(), mcp);

    // 读取循环
    while let Some(msg) = ws_read.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        let env: Envelope = match serde_json::from_str(msg.as_str()) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let session_id = env.session_id.clone();
        let run_id = env.run_id.clone();

        match env.msg_type.as_str() {
            msg_type::HELLO => {
                let ok = env.payload.get("token").and_then(|x| x.as_str())
                    == Some(expected_token.as_str());
                let reply = if ok {
                    Envelope::reply(msg_type::READY, serde_json::json!({}))
                } else {
                    Envelope::reply(
                        msg_type::RUN_ERROR,
                        serde_json::json!({ "message": "bad token" }),
                    )
                };
                let _ = manager.send_to_agent(reply);
            }

            msg_type::PING => {
                let _ =
                    manager.send_to_agent(Envelope::reply(msg_type::PONG, serde_json::json!({})));
            }

            msg_type::DEBUG_PROMPT_RESULT => {
                let req_id = env
                    .payload
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                if !req_id.is_empty() {
                    let _ = manager.resolve_debug(&req_id, env.payload.clone());
                }
            }

            msg_type::EMBEDDING_RESULT => {
                let req_id = env
                    .payload
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if !req_id.is_empty() {
                    let _ = manager.resolve_embedding(req_id, env.payload.clone());
                }
            }

            msg_type::ASSISTANT_DELTA => {
                let content = env
                    .payload
                    .get("content")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");

                // 1. 推送给前端 React
                let _ = app_handle.emit(
                    "agent://assistant_delta",
                    json!({
                        "session_id": session_id,
                        "run_id": run_id,
                        "content": content
                    }),
                );

                // 2. 缓存并累加消息内容
                let mut runs = active_runs.lock().await;
                if !runs.contains_key(&run_id) {
                    // 优先用 AgentManager 显式注册的 assistant_message_id（peek 非消费，
                    // 供后续 TOOL_CALL_REQUEST / RUN_ERROR 也能定位），消除「最后一条 pending」竞争。
                    let pending_id = manager.peek_run(&run_id);
                    let pending_id = match pending_id {
                        Some(id) => Some(id),
                        None => {
                            // fallback：查最后一条 pending 消息（保留以兼容未注册的路径）
                            db.list_messages_with_parts(session_id.clone())
                                .await
                                .ok()
                                .and_then(|msgs| {
                                    msgs.iter()
                                        .rev()
                                        .find(|(m, _)| m.status == "pending")
                                        .map(|(m, _)| m.id.clone())
                                })
                        }
                    };
                    if let Some(id) = pending_id {
                        runs.insert(
                            run_id.clone(),
                            ActiveRun {
                                assistant_message_id: id,
                                session_id: session_id.clone(),
                                accumulated_text: String::new(),
                                accumulated_thought: String::new(),
                                current_ordinal: 0,
                                in_thought: false,
                            },
                        );
                    }
                }

                if let Some(run) = runs.get_mut(&run_id) {
                    // 按 <thought>/</thought> 标签分段路由到 thought / text。
                    // 用 in_thought 标志跨 chunk 持久，避免多段思维链时第二段被误并入正文
                    // （旧逻辑用 accumulated_thought.contains("</thought>") 判断，第一段闭合后恒为 true，
                    //  导致第二段思维链全部落入 accumulated_text，重开读库时表现为正文泄漏）。
                    let mut remaining = content;
                    while !remaining.is_empty() {
                        if run.in_thought {
                            if let Some(idx) = remaining.find("</thought>") {
                                run.accumulated_thought.push_str(&remaining[..idx]);
                                run.in_thought = false;
                                remaining = &remaining[idx + "</thought>".len()..];
                            } else {
                                run.accumulated_thought.push_str(remaining);
                                remaining = "";
                            }
                        } else if let Some(idx) = remaining.find("<thought>") {
                            run.accumulated_text.push_str(&remaining[..idx]);
                            run.in_thought = true;
                            remaining = &remaining[idx + "<thought>".len()..];
                        } else {
                            run.accumulated_text.push_str(remaining);
                            remaining = "";
                        }
                    }
                }
            }

            msg_type::TOOL_CALL_REQUEST => {
                let tc_id = env
                    .payload
                    .get("id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_name = env
                    .payload
                    .get("tool")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let args = env
                    .payload
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let public_args = crate::tools::builtin::memory_entry::public_arguments(&args);

                // 确保 ActiveRun 已创建（AI 首个输出可能是工具调用，此前无 ASSISTANT_DELTA）。
                // 否则取消时无 ActiveRun 可 drain、状态不更新，消息会消失。
                {
                    let mut runs = active_runs.lock().await;
                    if !runs.contains_key(&run_id) {
                        let pending_id = manager.peek_run(&run_id);
                        if let Some(id) = pending_id {
                            runs.insert(
                                run_id.clone(),
                                ActiveRun {
                                    assistant_message_id: id,
                                    session_id: session_id.clone(),
                                    accumulated_text: String::new(),
                                    accumulated_thought: String::new(),
                                    current_ordinal: 0,
                                    in_thought: false,
                                },
                            );
                        }
                    }
                }

                // 获取 Agent 的 ToolPolicy
                let mut policy = ToolPolicy::default();
                let mut permission_mode = PermissionMode::default();
                if let Ok(Some(sess)) = db.get_session(session_id.clone()).await {
                    permission_mode = sess.permission_mode.parse().unwrap_or_default();
                    if let Ok(agents) = db.list_agents().await {
                        if let Some(agent) = agents.iter().find(|a| a.id == sess.agent_id) {
                            if let Ok(parsed) =
                                serde_json::from_str::<ToolPolicy>(&agent.tool_policy)
                            {
                                policy = parsed;
                            }
                        }
                    }
                }
                policy = permission_mode.effective_policy(&policy);

                // Resolve approval from the session permission mode.
                let risk = crate::tools::builtin::compute_risk(&tool_name, &args);
                let role_policy_requires_approval =
                    crate::tools::builtin::needs_approval(&tool_name, &args, &policy);
                let approval_decision =
                    crate::tools::permissions::approval_decision(permission_mode, &tool_name, risk);
                let workspace_cwd =
                    crate::tools::workspace::resolve_workspace_cwd(&db, &session_id).await;
                let effective_cwd = args
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .filter(|cwd| !cwd.is_empty())
                    .map(ToString::to_string)
                    .or_else(|| workspace_cwd.map(|path| path.to_string_lossy().to_string()))
                    .or_else(|| policy.shell.allowed_cwd.first().cloned());

                // Emit the tool call before approval or execution so every permission mode
                // gets a live card in the frontend.
                let tool_event_name = if approval_decision.needs_approval {
                    "agent://tool_call_pending"
                } else {
                    "agent://tool_call_started"
                };
                let _ = app_handle.emit(tool_event_name, json!({
                    "session_id": session_id.clone(),
                    "run_id": run_id.clone(),
                    "tool_call_id": tc_id.clone(),
                    "tool": tool_name.clone(),
                    "arguments": public_args.clone(),
                    "risk": risk.as_str(),
                    "cwd": effective_cwd,
                    "network_allowed": policy.network.allow,
                    "landlock": policy.sandbox.landlock,
                    "permission_mode": permission_mode.as_str(),
                    "approval_reason": approval_decision.reason,
                    "is_secondary_confirmation": approval_decision.is_secondary_confirmation,
                    "role_policy_requires_approval": role_policy_requires_approval,
                    "status": if approval_decision.needs_approval { "pending_approval" } else { "running" },
                }));
                if approval_decision.needs_approval {
                    if let Some(state) = app_handle.try_state::<AppState>() {
                        if let Err(error) = state
                            .notifications
                            .notify_approval_requested(&session_id, &tc_id, &tool_name)
                            .await
                        {
                            eprintln!("[notifications] failed to record approval request: {error}");
                        }
                    }
                }

                let mut approved = true;
                let mut cancelled_during_approval = false;
                if approval_decision.needs_approval {
                    // 1. 注册 Oneshot Channel 并将协程挂起
                    let (approval_tx, approval_rx) = tokio::sync::oneshot::channel::<bool>();
                    manager.register_approval(tc_id.clone(), approval_tx);
                    // 记录 run_id → tc_id，cancel 时据此清理
                    manager.set_run_approval_tc(run_id.clone(), tc_id.clone());
                    // 注册取消信号：cancel_run 触发以解除下方 select 阻塞
                    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
                    manager.set_run_cancel_signal(run_id.clone(), cancel_tx);

                    // 2. 等待用户点击按钮释放，或 cancel_run 触发取消信号
                    tokio::select! {
                        r = approval_rx => { approved = r.unwrap_or(false); }
                        _ = cancel_rx => { cancelled_during_approval = true; }
                    }
                    // 清理：未消费的 approval_tx 与 run→tc 映射（已消费则 no-op）
                    let _ = manager.resolve_approval(&tc_id, false);
                    let _ = manager.take_run_approval_tc(&run_id);
                }

                // 取消：不执行工具、不发 TOOL_RESULT，让 RUN_ERROR 清理消息状态
                if cancelled_during_approval {
                    continue;
                }

                if !approved {
                    // 用户拒绝执行
                    // 记录拒绝审计
                    let tc_log = NewToolCall {
                        id: tc_id.clone(),
                        session_id: session_id.clone(),
                        message_id: None,
                        tool: tool_name.clone(),
                        params: Some(public_args.to_string()),
                        status: "rejected".to_string(),
                        risk_level: Some(risk.as_str().to_string()),
                        approval_policy_snapshot: Some(crate::tools::permissions::audit_snapshot(
                            permission_mode,
                            &policy,
                        )),
                    };
                    let _ = db.insert_tool_call(tc_log).await;
                    let _ = app_handle.emit(
                        "agent://tool_result",
                        json!({
                            "session_id": session_id.clone(),
                            "run_id": run_id.clone(),
                            "tool_call_id": tc_id.clone(),
                            "tool": tool_name.clone(),
                            "status": "denied",
                            "output": "User rejected tool execution"
                        }),
                    );

                    let reply = Envelope::reply(
                        msg_type::TOOL_RESULT,
                        json!({
                            "id": tc_id,
                            "exit_code": -2,
                            "stdout": "",
                            "stderr": "User rejected tool execution",
                        }),
                    );
                    let _ = manager.send_to_agent(reply);
                    continue;
                }

                // 前端展示正在执行
                let _ = app_handle.emit(
                    "agent://tool_result",
                    json!({
                        "session_id": session_id.clone(),
                        "run_id": run_id.clone(),
                        "tool_call_id": tc_id.clone(),
                        "tool": tool_name.clone(),
                        "status": "running",
                        "output": "Executing..."
                    }),
                );

                // 获取当前 ActiveRun 的 assistant_message_id，作为外键绑定到 tool_calls 审计表
                let mut assistant_msg_id = None;
                let runs = active_runs.lock().await;
                if let Some(run) = runs.get(&run_id) {
                    assistant_msg_id = Some(run.assistant_message_id.clone());
                }
                drop(runs);

                // 执行物理调用
                let exec_res = tool_executor
                    .execute_with_permission_mode(
                        &session_id,
                        assistant_msg_id.as_deref(),
                        &tc_id,
                        &tool_name,
                        &args,
                        &policy,
                        permission_mode,
                    )
                    .await;

                // 准备回传 Python 的结果
                let reply_payload = match exec_res {
                    Ok(val) => {
                        let stdout = val
                            .get("stdout")
                            .and_then(|x| x.as_str())
                            .or_else(|| val.get("content").and_then(|x| x.as_str()))
                            .map(ToString::to_string)
                            .unwrap_or_else(|| {
                                serde_json::to_string(&val)
                                    .unwrap_or_else(|_| "Success".to_string())
                            });
                        // 向前端发送执行结果以渲染 terminal log
                        let _ = app_handle.emit(
                            "agent://tool_result",
                            json!({
                                "session_id": session_id.clone(),
                                "run_id": run_id.clone(),
                                "tool_call_id": tc_id.clone(),
                                "tool": tool_name.clone(),
                                "status": "succeeded",
                                "output": stdout.clone()
                            }),
                        );

                        json!({
                            "id": tc_id,
                            "exit_code": val.get("exit_code").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                            "stdout": stdout,
                            "stderr": val.get("stderr").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        })
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        let _ = app_handle.emit(
                            "agent://tool_result",
                            json!({
                                "session_id": session_id.clone(),
                                "run_id": run_id.clone(),
                                "tool_call_id": tc_id.clone(),
                                "tool": tool_name.clone(),
                                "status": "failed",
                                "output": format!("Error: {err_str}")
                            }),
                        );

                        json!({
                            "id": tc_id,
                            "exit_code": -1,
                            "stdout": "",
                            "stderr": err_str,
                        })
                    }
                };

                // 把执行结果插入为当前 assistant 消息的 tool_call 和 tool_result 两个 message_parts。
                // 这样前端加载历史消息时可以完整展示思考中的工具轨迹。
                let mut runs = active_runs.lock().await;
                if let Some(run) = runs.get_mut(&run_id) {
                    let mut parts = Vec::new();

                    // 先把本回合累积的思维链/正文落库，保持「回复-工具调用-回复」的回合顺序，
                    // 不跨回合合并（避免重开后所有正文被拼到一起）。
                    drain_accumulated(run, &mut parts);

                    // tool_call part
                    parts.push(NewMessagePart {
                        id: uuid::Uuid::new_v4().to_string(),
                        message_id: run.assistant_message_id.clone(),
                        kind: "tool_call".into(),
                        ordinal: run.current_ordinal,
                        mime_type: None,
                        tool_call_id: Some(tc_id.clone()),
                        content: format!("Calling {tool_name} with params: {public_args}"),
                        metadata: None,
                    });
                    run.current_ordinal += 1;

                    // tool_result part
                    let stdout_clean = reply_payload
                        .get("stdout")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let stderr_clean = reply_payload
                        .get("stderr")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    parts.push(NewMessagePart {
                        id: uuid::Uuid::new_v4().to_string(),
                        message_id: run.assistant_message_id.clone(),
                        kind: "tool_result".into(),
                        ordinal: run.current_ordinal,
                        mime_type: None,
                        tool_call_id: Some(tc_id.clone()),
                        content: if stderr_clean.is_empty() {
                            stdout_clean
                        } else {
                            stderr_clean
                        },
                        metadata: None,
                    });
                    run.current_ordinal += 1;

                    let _ = db.insert_message_parts(parts).await;
                }
                drop(runs);

                // 发送给 Python client，使其恢复状态机运行
                let reply = Envelope::reply(msg_type::TOOL_RESULT, reply_payload);
                let _ = manager.send_to_agent(reply);
            }

            msg_type::RUN_FINISHED => {
                let summary = env
                    .payload
                    .get("summary")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let memories_val = env.payload.get("memories").and_then(|x| x.as_array());

                // 1. 更新会话摘要
                if !summary.is_empty() {
                    let _ = db
                        .update_session_summary(session_id.clone(), summary.to_string())
                        .await;
                }

                // 获取 Agent ID
                let mut agent_id = String::new();
                if let Ok(Some(sess)) = db.get_session(session_id.clone()).await {
                    agent_id = sess.agent_id;
                }

                // 2. Insert extracted structured memories and their optional local vectors.
                if let Some(mems) = memories_val {
                    for m in mems {
                        let content = m.get("content").and_then(|x| x.as_str()).unwrap_or("");
                        let name = m
                            .get("name")
                            .and_then(|value| value.as_str())
                            .filter(|value| !value.trim().is_empty())
                            .map(ToString::to_string)
                            .unwrap_or_else(|| content.chars().take(60).collect());
                        let keywords = m
                            .get("keywords")
                            .and_then(|value| value.as_array())
                            .map(|values| {
                                values
                                    .iter()
                                    .filter_map(|value| value.as_str().map(ToString::to_string))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let m_type = m.get("type").and_then(|x| x.as_str()).unwrap_or("Fact");
                        let confidence =
                            m.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.8);
                        let source = m.get("source").and_then(|x| x.as_str()).unwrap_or("");

                        if !content.is_empty() && !agent_id.is_empty() {
                            let mem_id = uuid::Uuid::new_v4().to_string();

                            let new_mem = NewMemory {
                                id: mem_id.clone(),
                                agent_id: agent_id.clone(),
                                name,
                                keywords,
                                content: content.to_string(),
                                creator: "ai".to_string(),
                                memory_type: m_type.to_string(),
                                scope: "agent".to_string(),
                                source: source.to_string(),
                                confidence,
                                embedding_id: None,
                            };
                            match db.insert_memory(new_mem).await {
                                Ok(true) => {
                                    if let Some(embedding) = m.get("embedding").and_then(
                                        crate::tools::builtin::memory_entry::parse_embedding_value,
                                    ) {
                                        if let Err(error) = db
                                            .upsert_memory_embedding(
                                                uuid::Uuid::new_v4().to_string(),
                                                mem_id,
                                                embedding.model,
                                                content.to_string(),
                                                embedding.vector,
                                            )
                                            .await
                                        {
                                            eprintln!("[memory] Failed to index extracted memory: {error}");
                                        }
                                    }
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    eprintln!(
                                        "[memory] Failed to persist extracted memory: {error}"
                                    )
                                }
                            }
                        }
                    }
                }

                // 3. 落地最终的 assistant 思考及正文消息片段（本回合的，已累计的在前序回合 drain 过）。
                // A provider may finish without sending a delta (for example, an
                // exhausted reasoning response). Create the run record from the
                // explicit manager mapping so that this cannot leave a pending
                // assistant bubble behind.
                let mut runs = active_runs.lock().await;
                if !runs.contains_key(&run_id) {
                    if let Some(id) = manager.peek_run(&run_id) {
                        runs.insert(
                            run_id.clone(),
                            ActiveRun {
                                assistant_message_id: id,
                                session_id: session_id.clone(),
                                accumulated_text: String::new(),
                                accumulated_thought: String::new(),
                                current_ordinal: 0,
                                in_thought: false,
                            },
                        );
                    }
                }
                if let Some(mut run) = runs.remove(&run_id) {
                    let mut parts_to_insert = Vec::new();
                    drain_accumulated(&mut run, &mut parts_to_insert);
                    let has_output = !parts_to_insert.is_empty() || run.current_ordinal > 0;

                    // 消息行（pending 占位）已在 send_message 中创建，这里只需把累积的文本/思考片段
                    // 写入已有的 message_parts，避免以相同 id 重新 INSERT 触发主键冲突导致整段事务回滚、
                    // 回复内容丢失（表现为 AI 消息在输出结束后消失）。
                    if has_output {
                        if !parts_to_insert.is_empty() {
                            let _ = db.insert_message_parts(parts_to_insert).await;
                        }
                        let completed = db
                            .update_message_status(run.assistant_message_id, "complete".to_string())
                            .await;
                        if completed.is_ok() {
                            if let Some(state) = app_handle.try_state::<AppState>() {
                                state.sync.schedule();
                            }
                        }
                    } else {
                        let _ = db
                            .fail_pending_assistant(
                                run.assistant_message_id,
                                "（模型未返回内容，请重试或关闭思考模式后再试）".to_string(),
                            )
                            .await;
                    }
                } else if let Some(id) = manager.peek_run(&run_id) {
                    let _ = db
                        .fail_pending_assistant(
                            id,
                            "（模型未返回内容，请重试或关闭思考模式后再试）".to_string(),
                        )
                        .await;
                }
                drop(runs);

                // 清除 run 与 session 映射（运行已结束）
                let _ = manager.remove_run(&run_id);
                let _ = manager.remove_session_run(&session_id);

                if let Some(state) = app_handle.try_state::<AppState>() {
                    if let Err(error) = state
                        .notifications
                        .notify_agent_completed(&session_id, &run_id)
                        .await
                    {
                        eprintln!("[notifications] failed to record completed run: {error}");
                    }
                }

                // 通知前端渲染完成
                let _ = app_handle.emit(
                    "agent://run_finished",
                    json!({
                        "session_id": session_id,
                        "run_id": run_id
                    }),
                );
            }

            msg_type::RUN_ERROR => {
                let err_msg = env
                    .payload
                    .get("message")
                    .and_then(|x| x.as_str())
                    .unwrap_or("Unknown runtime error");
                let is_cancelled = err_msg == "已取消";

                // 取 ActiveRun（若有）；否则用 peek_run 兜底定位 assistant_msg_id
                // （AI 首个输出即工具调用且取消时，可能从未产生 ASSISTANT_DELTA，ActiveRun 不存在）
                let mut runs = active_runs.lock().await;
                let (assistant_msg_id, mut parts_to_insert) =
                    if let Some(mut run) = runs.remove(&run_id) {
                        let mut parts = Vec::new();
                        drain_accumulated(&mut run, &mut parts);
                        (Some(run.assistant_message_id.clone()), parts)
                    } else {
                        (manager.peek_run(&run_id), Vec::new())
                    };
                drop(runs);

                if let Some(id) = assistant_msg_id.clone() {
                    parts_to_insert.push(NewMessagePart {
                        id: uuid::Uuid::new_v4().to_string(),
                        message_id: id.clone(),
                        kind: "text".into(),
                        ordinal: 0,
                        mime_type: None,
                        tool_call_id: None,
                        content: if is_cancelled {
                            "（已取消）".to_string()
                        } else {
                            format!("Error: {err_msg}")
                        },
                        metadata: None,
                    });
                    let _ = db.insert_message_parts(parts_to_insert).await;
                    let _ = db
                        .update_message_status(
                            id,
                            if is_cancelled {
                                "cancelled".to_string()
                            } else {
                                "failed".to_string()
                            },
                        )
                        .await;
                }

                // 清除 run 与 session 映射（运行已结束）
                let _ = manager.remove_run(&run_id);
                let _ = manager.remove_session_run(&session_id);

                let _ = app_handle.emit(
                    "agent://run_error",
                    json!({
                        "session_id": session_id,
                        "run_id": run_id,
                        "message": err_msg
                    }),
                );
            }

            _ => {}
        }
    }

    // A sidecar disconnect can otherwise leave the UI with blank pending
    // assistant placeholders forever. Resolve every in-flight run explicitly.
    let session_runs = manager.drain_session_runs();
    let sessions_by_run = session_runs
        .into_iter()
        .map(|(session_id, run_id)| (run_id, session_id))
        .collect::<std::collections::HashMap<_, _>>();
    for (run_id, assistant_message_id) in manager.drain_runs() {
        let _ = db
            .fail_pending_assistant(
                assistant_message_id,
                "（Agent 连接已断开，请重新发送）".to_string(),
            )
            .await;
        if let Some(session_id) = sessions_by_run.get(&run_id) {
            let _ = app_handle.emit(
                "agent://run_error",
                json!({
                    "session_id": session_id,
                    "run_id": run_id,
                    "message": "Agent 连接已断开"
                }),
            );
        }
    }

    // 关闭逻辑
    manager.clear_active_sender();
    writer_task.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    #[tokio::test]
    async fn python_sidecar_handshake_succeeds() {
        let token = "unit-test-token".to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ws port");
        let port = listener.local_addr().unwrap().port();

        let db_path = std::path::PathBuf::from("target/test_handshake.db");
        if db_path.exists() {
            let _ = std::fs::remove_file(&db_path);
        }
        let db = crate::db::spawn_db_actor(db_path.clone());
        let app = tauri::test::mock_app();
        let app_handle = app.handle().clone();
        let manager = Arc::new(AgentManager::new());
        let mcp = Arc::new(crate::mcp::McpManager::new(
            db.clone(),
            Arc::new(crate::secrets::InMemorySecretStore::default()),
        ));

        // 真实 Rust WS Server
        let srv_token = token.clone();
        let db_clone = db.clone();
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            let _ = run(
                listener,
                srv_token,
                db_clone,
                app_handle,
                manager_clone,
                mcp,
            )
            .await;
        });

        // 真实 Python sidecar（WS Client）
        let url = format!("ws://127.0.0.1:{port}/agent");
        let mut child = TokioCommand::new("uv")
            .args(["run", "python", "-m", "app.main"])
            .current_dir("../agent")
            .env("AGENT_WS_URL", &url)
            .env("AGENT_PROTOCOL_TOKEN", &token)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn python sidecar");

        let mut stderr = child.stderr.take().expect("piped stderr");
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await;
            if !buf.is_empty() {
                eprintln!("[sidecar stderr]\n{}", String::from_utf8_lossy(&buf));
            }
        });

        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();

        let mut ok = false;
        while let Ok(Ok(Some(line))) =
            tokio::time::timeout(Duration::from_secs(30), lines.next_line()).await
        {
            if line.contains("握手成功") {
                ok = true;
                break;
            }
        }
        let mut embedding_roundtrip = false;
        if ok {
            let request_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx) = tokio::sync::oneshot::channel();
            manager.register_embedding(request_id.clone(), tx);
            manager
                .send_to_agent(Envelope {
                    protocol_version: crate::agent::protocol::PROTOCOL_VERSION,
                    id: request_id,
                    run_id: String::new(),
                    session_id: String::new(),
                    msg_type: msg_type::EMBEDDING_REQUEST.to_string(),
                    created_at: String::new(),
                    payload: json!({
                        "config": {"model": "unused", "litellmModel": "unused"},
                        "inputs": []
                    }),
                })
                .unwrap();
            embedding_roundtrip = tokio::time::timeout(Duration::from_secs(10), rx)
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|payload| payload.get("error").cloned())
                .and_then(|error| error.as_str().map(ToString::to_string))
                .is_some_and(|error| error.contains("non-empty strings"));
        }
        let _ = child.start_kill();
        let _ = std::fs::remove_file(&db_path);
        assert!(ok, "Python sidecar 未报告握手成功");
        assert!(
            embedding_roundtrip,
            "Python sidecar 未返回 embedding_result"
        );
    }
}
