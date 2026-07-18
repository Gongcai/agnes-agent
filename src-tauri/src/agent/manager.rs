use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

use crate::agent::protocol::Envelope;
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::mcp::McpManager;

/// Agent 运行时：拥有 Python sidecar 子进程与 WS Server 端口/token。
struct AgentRuntime {
    _port: u16,
    _token: String,
    child: std::process::Child,
}

/// 管理 Python sidecar 生命周期并协调 WS 通信与工具审批。
pub struct AgentManager {
    inner: Mutex<Option<AgentRuntime>>,
    running: AtomicBool,
    // 当前活跃的 WebSocket 连接发送端通道
    active_sender: Mutex<Option<mpsc::UnboundedSender<Envelope>>>,
    // 等待审批的工具调用：tool_call_id -> oneshot 批准状态发送端
    pending_approvals: Mutex<std::collections::HashMap<String, oneshot::Sender<bool>>>,
    // 等待调试提示词拼装结果：请求 id -> oneshot 返回 payload 发送端
    pending_debug: Mutex<std::collections::HashMap<String, oneshot::Sender<serde_json::Value>>>,
    // Pending embedding batches: request id -> result payload sender.
    pending_embeddings:
        Mutex<std::collections::HashMap<String, oneshot::Sender<serde_json::Value>>>,
    // 显式注册的运行：run_id -> assistant_message_id，供 ws_server 精确定位 pending 消息
    pending_runs: Mutex<std::collections::HashMap<String, String>>,
    // 当前活跃运行：session_id -> run_id，供 cancel_run 按会话取消
    active_session_runs: Mutex<std::collections::HashMap<String, String>>,
    // 挂起审批的工具调用：run_id -> tool_call_id，cancel 时据此把审批 resolve 为 false 解除 ws 阻塞
    run_approval_tc: Mutex<std::collections::HashMap<String, String>>,
    // 挂起审批的取消信号：run_id -> oneshot::Sender，cancel_run 触发以解除 ws_server approval await
    run_cancel_signals: Mutex<std::collections::HashMap<String, oneshot::Sender<()>>>,
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            running: AtomicBool::new(false),
            active_sender: Mutex::new(None),
            pending_approvals: Mutex::new(std::collections::HashMap::new()),
            pending_debug: Mutex::new(std::collections::HashMap::new()),
            pending_embeddings: Mutex::new(std::collections::HashMap::new()),
            pending_runs: Mutex::new(std::collections::HashMap::new()),
            active_session_runs: Mutex::new(std::collections::HashMap::new()),
            run_approval_tc: Mutex::new(std::collections::HashMap::new()),
            run_cancel_signals: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 启动：绑定 127.0.0.1 随机端口，生成一次性 token，拉起 Python sidecar。
    /// 并将 WS Server 任务提交到 Tauri 的异步运行时。
    pub fn start(
        self: &Arc<Self>,
        db: DbActorHandle,
        app_handle: tauri::AppHandle,
        mcp: Arc<McpManager>,
    ) -> AppResult<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let token = generate_token();
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|e| AppError::Agent(format!("绑定 WS 端口失败：{e}")))?;
        let port = listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| AppError::Agent(format!("取端口失败：{e}")))?;

        let child = Command::new("uv")
            .args(["run", "python", "-m", "app.main"])
            .current_dir(resolve_agent_dir())
            .env("AGENT_WS_URL", format!("ws://127.0.0.1:{port}/agent"))
            .env("AGENT_PROTOCOL_TOKEN", &token)
            .spawn()
            .map_err(|e| {
                self.running.store(false, Ordering::SeqCst);
                AppError::Agent(format!(
                    "拉起 Python sidecar 失败：{e}（确认 uv 在 PATH 且 agent/ 存在）"
                ))
            })?;

        // 绑定 WS Server 运行到 Tauri 异步运行时
        let token_for_ws = token.clone();
        let manager_clone = self.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::agent::ws_server::run(
                listener,
                token_for_ws,
                db,
                app_handle,
                manager_clone,
                mcp,
            )
            .await
            {
                eprintln!("[agent][ws] 运行错误：{e}");
            }
        });

        *self.inner.lock().unwrap() = Some(AgentRuntime {
            _port: port,
            _token: token,
            child,
        });
        Ok(())
    }

    /// 注册当前活跃的 WS 发送端通道。
    pub fn register_active_sender(&self, tx: mpsc::UnboundedSender<Envelope>) {
        *self.active_sender.lock().unwrap() = Some(tx);
    }

    /// 注销当前活跃的 WS 发送端通道。
    pub fn clear_active_sender(&self) {
        *self.active_sender.lock().unwrap() = None;
    }

    /// 往 Python sidecar 发送一条协议信封消息。
    pub fn send_to_agent(&self, env: Envelope) -> AppResult<()> {
        let sender_opt = self.active_sender.lock().unwrap();
        if let Some(ref tx) = *sender_opt {
            tx.send(env)
                .map_err(|_| AppError::Ws("WS 连接通道已断开".into()))?;
            Ok(())
        } else {
            Err(AppError::Ws("Python sidecar 未连接或连接已断开".into()))
        }
    }

    /// 注册一个等待审批的工具调用。
    pub fn register_approval(&self, tool_call_id: String, tx: oneshot::Sender<bool>) {
        self.pending_approvals
            .lock()
            .unwrap()
            .insert(tool_call_id, tx);
    }

    /// 批准或拒绝一个挂起的工具调用。如果找到并处理则返回 true。
    pub fn resolve_approval(&self, tool_call_id: &str, approved: bool) -> bool {
        let mut map = self.pending_approvals.lock().unwrap();
        if let Some(tx) = map.remove(tool_call_id) {
            let _ = tx.send(approved);
            true
        } else {
            false
        }
    }

    /// 注册一个等待调试提示词结果的请求。
    pub fn register_debug(&self, id: String, tx: oneshot::Sender<serde_json::Value>) {
        self.pending_debug.lock().unwrap().insert(id, tx);
    }

    /// 解析一个调试提示词结果。如果找到并处理则返回 true。
    pub fn resolve_debug(&self, id: &str, payload: serde_json::Value) -> bool {
        let mut map = self.pending_debug.lock().unwrap();
        if let Some(tx) = map.remove(id) {
            let _ = tx.send(payload);
            true
        } else {
            false
        }
    }

    pub fn register_embedding(&self, id: String, tx: oneshot::Sender<serde_json::Value>) {
        self.pending_embeddings.lock().unwrap().insert(id, tx);
    }

    pub fn resolve_embedding(&self, id: &str, payload: serde_json::Value) -> bool {
        let mut map = self.pending_embeddings.lock().unwrap();
        if let Some(tx) = map.remove(id) {
            let _ = tx.send(payload);
            true
        } else {
            false
        }
    }

    /// 注册一次运行的 run_id → assistant_message_id 映射，供 ws_server 精确定位 pending 消息。
    pub fn register_run(&self, run_id: String, assistant_message_id: String) {
        self.pending_runs
            .lock()
            .unwrap()
            .insert(run_id, assistant_message_id);
    }

    /// 非消费地查看 run_id 对应的 assistant_message_id（ws_server ensure_run 用）。
    pub fn peek_run(&self, run_id: &str) -> Option<String> {
        self.pending_runs.lock().unwrap().get(run_id).cloned()
    }

    /// 清理 run_id → assistant_message_id 映射（RUN_FINISHED/RUN_ERROR 时调用）。
    pub fn remove_run(&self, run_id: &str) -> Option<String> {
        self.pending_runs.lock().unwrap().remove(run_id)
    }

    /// Drain runs that cannot complete after the sidecar connection closes.
    pub fn drain_runs(&self) -> std::collections::HashMap<String, String> {
        std::mem::take(&mut *self.pending_runs.lock().unwrap())
    }

    /// 记录某会话当前活跃运行的 run_id（cancel_run 按会话取消用）。
    pub fn set_session_run(&self, session_id: String, run_id: String) {
        self.active_session_runs
            .lock()
            .unwrap()
            .insert(session_id, run_id);
    }

    /// 移除并返回某会话当前活跃运行的 run_id。RUN_FINISHED/RUN_ERROR/cancel 时调用。
    pub fn remove_session_run(&self, session_id: &str) -> Option<String> {
        self.active_session_runs.lock().unwrap().remove(session_id)
    }

    /// Drain session-to-run associations when the only sidecar disconnects.
    pub fn drain_session_runs(&self) -> std::collections::HashMap<String, String> {
        std::mem::take(&mut *self.active_session_runs.lock().unwrap())
    }

    /// 记录某 run 当前挂起审批的 tool_call_id（cancel 时据此解除审批阻塞）。
    pub fn set_run_approval_tc(&self, run_id: String, tool_call_id: String) {
        self.run_approval_tc
            .lock()
            .unwrap()
            .insert(run_id, tool_call_id);
    }

    /// 取走某 run 挂起审批的 tool_call_id。
    pub fn take_run_approval_tc(&self, run_id: &str) -> Option<String> {
        self.run_approval_tc.lock().unwrap().remove(run_id)
    }

    /// 注册某 run 审批等待的取消信号（ws_server 在 approval await 时 select 它）。
    pub fn set_run_cancel_signal(&self, run_id: String, tx: oneshot::Sender<()>) {
        self.run_cancel_signals.lock().unwrap().insert(run_id, tx);
    }

    /// 取走并触发某 run 的取消信号（cancel_run 调用）。
    pub fn take_run_cancel_signal(&self, run_id: &str) -> Option<oneshot::Sender<()>> {
        self.run_cancel_signals.lock().unwrap().remove(run_id)
    }
}

fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| format!("{:02x}", rng.gen::<u8>()))
        .collect()
}

fn resolve_agent_dir() -> std::path::PathBuf {
    if std::path::Path::new("agent").exists() {
        std::path::PathBuf::from("agent")
    } else {
        std::path::PathBuf::from("../agent")
    }
}
