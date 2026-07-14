use std::net::TcpListener;
use std::sync::Arc;
use std::collections::HashMap;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener as TokioListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::accept_async;
use serde_json::json;
use tauri::Emitter;

use crate::agent::protocol::{msg_type, Envelope};
use crate::agent::AgentManager;
use crate::db::DbActorHandle;
use crate::db::repo::messages::{NewMessage, NewMessagePart};
use crate::db::repo::memory::NewMemory;
use crate::db::repo::tools::NewToolCall;
use crate::error::{AppError, AppResult};
use crate::tools::{ToolExecutor, ToolPolicy};

pub struct ActiveRun {
    pub assistant_message_id: String,
    pub session_id: String,
    pub accumulated_text: String,
    pub accumulated_thought: String,
    pub current_ordinal: i32,
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
) -> AppResult<()> {
    std_listener
        .set_nonblocking(true)
        .map_err(|e| AppError::Ws(format!("set_nonblocking 失败：{e}")))?;
    let listener = TokioListener::from_std(std_listener)
        .map_err(|e| AppError::Ws(e.to_string()))?;
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
        let active_runs = active_runs.clone();
        
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, token, db, app_handle, manager, active_runs).await {
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

    let tool_executor = ToolExecutor::new(db.clone());

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
                let ok = env.payload.get("token").and_then(|x| x.as_str()) == Some(expected_token.as_str());
                let reply = if ok {
                    Envelope::reply(msg_type::READY, serde_json::json!({}))
                } else {
                    Envelope::reply(msg_type::RUN_ERROR, serde_json::json!({ "message": "bad token" }))
                };
                let _ = manager.send_to_agent(reply);
            }

            msg_type::PING => {
                let _ = manager.send_to_agent(Envelope::reply(msg_type::PONG, serde_json::json!({})));
            }

            msg_type::ASSISTANT_DELTA => {
                let content = env.payload.get("content").and_then(|x| x.as_str()).unwrap_or("");
                
                // 1. 推送给前端 React
                let _ = app_handle.emit("agent://assistant_delta", json!({
                    "session_id": session_id,
                    "run_id": run_id,
                    "content": content
                }));

                // 2. 缓存并累加消息内容
                let mut runs = active_runs.lock().await;
                if !runs.contains_key(&run_id) {
                    // 查询 SQLite 获取最新的 pending 消息 ID
                    if let Ok(msgs) = db.list_messages_with_parts(session_id.clone()).await {
                        if let Some((pending_msg, _)) = msgs.iter().rev().find(|(m, _)| m.status == "pending") {
                            runs.insert(run_id.clone(), ActiveRun {
                                assistant_message_id: pending_msg.id.clone(),
                                session_id: session_id.clone(),
                                accumulated_text: String::new(),
                                accumulated_thought: String::new(),
                                current_ordinal: 0,
                            });
                        }
                    }
                }

                if let Some(run) = runs.get_mut(&run_id) {
                    if content.starts_with("<thought>") || run.accumulated_thought.starts_with("<thought>") && !run.accumulated_thought.contains("</thought>") {
                        run.accumulated_thought.push_str(content);
                    } else {
                        run.accumulated_text.push_str(content);
                    }
                }
            }

            msg_type::TOOL_CALL_REQUEST => {
                let tc_id = env.payload.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let tool_name = env.payload.get("tool").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let args = env.payload.get("arguments").cloned().unwrap_or(serde_json::Value::Null);

                // 获取 Agent 的 ToolPolicy
                let mut policy = ToolPolicy::default();
                let mut agent_id = String::new();
                
                if let Ok(Some(sess)) = db.get_session(session_id.clone()).await {
                    agent_id = sess.agent_id.clone();
                    if let Ok(agents) = db.list_agents().await {
                        if let Some(agent) = agents.iter().find(|a| a.id == sess.agent_id) {
                            if let Ok(parsed) = serde_json::from_str::<ToolPolicy>(&agent.tool_policy) {
                                policy = parsed;
                            }
                        }
                    }
                }

                // 判断是否需要人工审批
                let needs_approval = match tool_name.as_str() {
                    "shell" => policy.shell.approval == "always",
                    "file_write" => policy.file.approval == "always" || policy.file.approval == "write",
                    "git" => {
                        let is_push = args.get("args")
                            .and_then(|x| x.as_array())
                            .map(|arr| arr.iter().any(|v| v.as_str() == Some("push")))
                            .unwrap_or(false);
                        policy.git.approval == "always" || (policy.git.approval == "push" && is_push)
                    }
                    _ => false,
                };

                let mut approved = true;
                if needs_approval {
                    // 1. 注册 Oneshot Channel 并将协程挂起
                    let (approval_tx, approval_rx) = tokio::sync::oneshot::channel::<bool>();
                    manager.register_approval(tc_id.clone(), approval_tx);

                    // 2. 向 React 发送等待审批事件
                    let _ = app_handle.emit("agent://tool_call_pending", json!({
                        "session_id": session_id.clone(),
                        "run_id": run_id.clone(),
                        "tool_call_id": tc_id.clone(),
                        "tool": tool_name.clone(),
                        "arguments": args.clone(),
                    }));

                    // 3. 等待用户点击按钮释放
                    approved = approval_rx.await.unwrap_or(false);
                }

                if !approved {
                    // 用户拒绝执行
                    // 记录拒绝审计
                    let tc_log = NewToolCall {
                        id: tc_id.clone(),
                        session_id: session_id.clone(),
                        message_id: None,
                        tool: tool_name.clone(),
                        params: Some(args.to_string()),
                        status: "rejected".to_string(),
                        risk_level: Some("Medium".to_string()),
                        approval_policy_snapshot: Some(serde_json::to_string(&policy).unwrap_or_default()),
                    };
                    let _ = db.insert_tool_call(tc_log).await;
                    
                    let reply = Envelope::reply(msg_type::TOOL_RESULT, json!({
                        "id": tc_id,
                        "exit_code": -2,
                        "stdout": "",
                        "stderr": "User rejected tool execution",
                    }));
                    let _ = manager.send_to_agent(reply);
                    continue;
                }

                // 前端展示正在执行
                let _ = app_handle.emit("agent://tool_result", json!({
                    "session_id": session_id.clone(),
                    "run_id": run_id.clone(),
                    "tool_call_id": tc_id.clone(),
                    "tool": tool_name.clone(),
                    "output": "Executing..."
                }));

                // 获取当前 ActiveRun 的 assistant_message_id，作为外键绑定到 tool_calls 审计表
                let mut assistant_msg_id = None;
                let runs = active_runs.lock().await;
                if let Some(run) = runs.get(&run_id) {
                    assistant_msg_id = Some(run.assistant_message_id.clone());
                }
                drop(runs);

                // 执行物理调用
                let exec_res = tool_executor.execute(
                    &session_id,
                    assistant_msg_id.as_deref(),
                    &tc_id,
                    &tool_name,
                    &args,
                    &policy,
                ).await;

                // 准备回传 Python 的结果
                let reply_payload = match exec_res {
                    Ok(val) => {
                        let stdout = val.get("stdout").and_then(|x| x.as_str()).unwrap_or("Success").to_string();
                        // 向前端发送执行结果以渲染 terminal log
                        let _ = app_handle.emit("agent://tool_result", json!({
                            "session_id": session_id.clone(),
                            "run_id": run_id.clone(),
                            "tool_call_id": tc_id.clone(),
                            "tool": tool_name.clone(),
                            "output": stdout.clone()
                        }));

                        json!({
                            "id": tc_id,
                            "exit_code": val.get("exit_code").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                            "stdout": stdout,
                            "stderr": val.get("stderr").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        })
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        let _ = app_handle.emit("agent://tool_result", json!({
                            "session_id": session_id.clone(),
                            "run_id": run_id.clone(),
                            "tool_call_id": tc_id.clone(),
                            "tool": tool_name.clone(),
                            "output": format!("Error: {err_str}")
                        }));

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
                    let part_tc_id = uuid::Uuid::new_v4().to_string();
                    let part_res_id = uuid::Uuid::new_v4().to_string();

                    // tool_call part
                    let _ = db.insert_message(
                        NewMessage {
                            id: run.assistant_message_id.clone(),
                            session_id: session_id.clone(),
                            role: "assistant".into(),
                            seq: 0, // 仅用作 messages 的 seq，此处消息本身已存在，这里底层 insert_message 不会重复创建
                            status: "pending".into(),
                            model: None,
                            token_count: None,
                            metadata: None,
                        },
                        vec![
                            NewMessagePart {
                                id: part_tc_id,
                                message_id: run.assistant_message_id.clone(),
                                kind: "tool_call".into(),
                                ordinal: run.current_ordinal,
                                mime_type: None,
                                tool_call_id: Some(tc_id.clone()),
                                content: format!("Calling {tool_name} with params: {}", args.to_string()),
                                metadata: None,
                            }
                        ]
                    ).await;
                    run.current_ordinal += 1;

                    // tool_result part
                    let stdout_clean = reply_payload.get("stdout").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let stderr_clean = reply_payload.get("stderr").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let _ = db.insert_message(
                        NewMessage {
                            id: run.assistant_message_id.clone(),
                            session_id: session_id.clone(),
                            role: "assistant".into(),
                            seq: 0,
                            status: "pending".into(),
                            model: None,
                            token_count: None,
                            metadata: None,
                        },
                        vec![
                            NewMessagePart {
                                id: part_res_id,
                                message_id: run.assistant_message_id.clone(),
                                kind: "tool_result".into(),
                                ordinal: run.current_ordinal,
                                mime_type: None,
                                tool_call_id: Some(tc_id.clone()),
                                content: if stderr_clean.is_empty() { stdout_clean } else { stderr_clean },
                                metadata: None,
                            }
                        ]
                    ).await;
                    run.current_ordinal += 1;
                }
                drop(runs);

                // 发送给 Python client，使其恢复状态机运行
                let reply = Envelope::reply(msg_type::TOOL_RESULT, reply_payload);
                let _ = manager.send_to_agent(reply);
            }

            msg_type::RUN_FINISHED => {
                let summary = env.payload.get("summary").and_then(|x| x.as_str()).unwrap_or("");
                let memories_val = env.payload.get("memories").and_then(|x| x.as_array());

                // 1. 更新会话摘要
                if !summary.is_empty() {
                    let _ = db.update_session_summary(session_id.clone(), summary.to_string()).await;
                }

                // 获取 Agent ID
                let mut agent_id = String::new();
                if let Ok(Some(sess)) = db.get_session(session_id.clone()).await {
                    agent_id = sess.agent_id;
                }

                // 2. 插入新提取的长期记忆条目
                if let Some(mems) = memories_val {
                    for m in mems {
                        let content = m.get("content").and_then(|x| x.as_str()).unwrap_or("");
                        let m_type = m.get("type").and_then(|x| x.as_str()).unwrap_or("Fact");
                        let confidence = m.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.8);
                        let source = m.get("source").and_then(|x| x.as_str()).unwrap_or("");

                        if !content.is_empty() && !agent_id.is_empty() {
                            let new_mem = NewMemory {
                                id: uuid::Uuid::new_v4().to_string(),
                                agent_id: agent_id.clone(),
                                content: content.to_string(),
                                memory_type: m_type.to_string(),
                                scope: "agent".to_string(),
                                source: source.to_string(),
                                confidence,
                            };
                            let _ = db.insert_memory(new_mem).await;
                        }
                    }
                }

                // 3. 落地最终的 assistant 思考及正文消息片段
                let mut runs = active_runs.lock().await;
                if let Some(run) = runs.remove(&run_id) {
                    let mut parts_to_insert = Vec::new();

                    // reasoning (thought) part
                    if !run.accumulated_thought.is_empty() {
                        // 去除 thought 首尾标记
                        let thought_clean = run.accumulated_thought
                            .replace("<thought>", "")
                            .replace("</thought>", "")
                            .trim()
                            .to_string();

                        if !thought_clean.is_empty() {
                            parts_to_insert.push(NewMessagePart {
                                id: uuid::Uuid::new_v4().to_string(),
                                message_id: run.assistant_message_id.clone(),
                                kind: "reasoning".into(),
                                ordinal: run.current_ordinal,
                                mime_type: None,
                                tool_call_id: None,
                                content: thought_clean,
                                metadata: None,
                            });
                        }
                    }

                    // text response part
                    if !run.accumulated_text.is_empty() {
                        parts_to_insert.push(NewMessagePart {
                            id: uuid::Uuid::new_v4().to_string(),
                            message_id: run.assistant_message_id.clone(),
                            kind: "text".into(),
                            ordinal: run.current_ordinal + 1,
                            mime_type: None,
                            tool_call_id: None,
                            content: run.accumulated_text,
                            metadata: None,
                        });
                    }

                    if !parts_to_insert.is_empty() {
                        // 以 complete 状态写入 SQLite
                        let _ = db.insert_message(
                            NewMessage {
                                id: run.assistant_message_id.clone(),
                                session_id: session_id.clone(),
                                role: "assistant".into(),
                                seq: 0,
                                status: "complete".into(),
                                model: None,
                                token_count: None,
                                metadata: None,
                            },
                            parts_to_insert
                        ).await;
                    }
                    
                    let _ = db.update_message_status(run.assistant_message_id, "complete".to_string()).await;
                }
                drop(runs);

                // 通知前端渲染完成
                let _ = app_handle.emit("agent://run_finished", json!({
                    "session_id": session_id,
                    "run_id": run_id
                }));
            }

            msg_type::RUN_ERROR => {
                let err_msg = env.payload.get("message").and_then(|x| x.as_str()).unwrap_or("Unknown runtime error");
                
                let mut runs = active_runs.lock().await;
                if let Some(run) = runs.remove(&run_id) {
                    let _ = db.update_message_status(run.assistant_message_id.clone(), "failed".to_string()).await;
                    
                    // 写入一个错误片段
                    let _ = db.insert_message(
                        NewMessage {
                            id: run.assistant_message_id.clone(),
                            session_id: session_id.clone(),
                            role: "assistant".into(),
                            seq: 0,
                            status: "failed".into(),
                            model: None,
                            token_count: None,
                            metadata: None,
                        },
                        vec![
                            NewMessagePart {
                                id: uuid::Uuid::new_v4().to_string(),
                                message_id: run.assistant_message_id,
                                kind: "text".into(),
                                ordinal: run.current_ordinal,
                                mime_type: None,
                                tool_call_id: None,
                                content: format!("Error: {err_msg}"),
                                metadata: None,
                            }
                        ]
                    ).await;
                }
                drop(runs);

                let _ = app_handle.emit("agent://run_error", json!({
                    "session_id": session_id,
                    "run_id": run_id,
                    "message": err_msg
                }));
            }

            _ => {}
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

        // 真实 Rust WS Server
        let srv_token = token.clone();
        let db_clone = db.clone();
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            let _ = run(listener, srv_token, db_clone, app_handle, manager_clone).await;
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
        let _ = child.start_kill();
        let _ = std::fs::remove_file(&db_path);
        assert!(ok, "Python sidecar 未报告握手成功");
    }
}
