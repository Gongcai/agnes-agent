use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::error::{AppError, AppResult};

/// Agent 运行时：拥有 Python sidecar 子进程与 WS Server 端口/token。
struct AgentRuntime {
    _port: u16,
    _token: String,
    child: std::process::Child,
}

/// 管理 Python sidecar 生命周期（dev 用 uv，发布用 externalBin）。
/// Rust 是主进程：绑定 127.0.0.1 WS、生成一次性 token、spawn Python、管权限与重启。
pub struct AgentManager {
    inner: Mutex<Option<AgentRuntime>>,
    running: AtomicBool,
}

impl AgentManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
            running: AtomicBool::new(false),
        }
    }

    /// 启动：绑定 127.0.0.1 随机端口，生成一次性 token，拉起 Python sidecar。
    /// 非致命：失败仅返回 Err，由调用方决定是否阻断（main 中选择不阻断）。
    pub fn start(&self) -> AppResult<()> {
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

        // WS Server 跑在独立线程（自带 tokio runtime），已绑定的 listener 移交过去。
        // token 先 clone 一份给线程，原 token 留给 AgentRuntime 持有。
        let token_for_ws = token.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build();
            if let Ok(rt) = rt {
                rt.block_on(async move {
                    if let Err(e) = crate::agent::ws_server::run(listener, token_for_ws).await {
                        eprintln!("[agent][ws] 运行错误：{e}");
                    }
                });
            }
        });

        *self.inner.lock().unwrap() = Some(AgentRuntime {
            _port: port,
            _token: token,
            child,
        });
        Ok(())
    }
}

fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..32).map(|_| format!("{:02x}", rng.gen::<u8>())).collect()
}

fn resolve_agent_dir() -> std::path::PathBuf {
    // dev 时 cwd 通常为仓库根
    if std::path::Path::new("agent").exists() {
        std::path::PathBuf::from("agent")
    } else {
        std::path::PathBuf::from("../agent")
    }
}
