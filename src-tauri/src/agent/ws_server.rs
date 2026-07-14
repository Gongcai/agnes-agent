use std::net::TcpListener;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener as TokioListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::accept_async;

use crate::agent::protocol::{msg_type, Envelope};
use crate::error::{AppError, AppResult};

/// Rust 侧 WS Server 主循环：绑定已分配的 127.0.0.1 端口，按连接派发。
pub async fn run(std_listener: TcpListener, token: String) -> AppResult<()> {
    // 必须将 std listener 置为非阻塞，否则 Tokio runtime 注册 fd 时会 panic。
    std_listener
        .set_nonblocking(true)
        .map_err(|e| AppError::Ws(format!("set_nonblocking 失败：{e}")))?;
    let listener = TokioListener::from_std(std_listener)
        .map_err(|e| AppError::Ws(e.to_string()))?;
    println!(
        "[agent][ws] listening on 127.0.0.1:{}",
        listener.local_addr().map(|a| a.port()).unwrap_or(0)
    );

    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .map_err(|e| AppError::Ws(e.to_string()))?;
        let token = token.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, token).await {
                eprintln!("[agent][ws] conn error: {e}");
            }
        });
    }
}

/// 单连接处理：hello（校验 payload.token）→ ready 握手，其余消息暂透传。
async fn handle_conn(stream: tokio::net::TcpStream, expected_token: String) -> AppResult<()> {
    let ws = accept_async(stream)
        .await
        .map_err(|e| AppError::Ws(e.to_string()))?;
    let (mut write, mut read) = ws.split();

    while let Some(msg) = read.next().await {
        let msg = msg.map_err(|e| AppError::Ws(e.to_string()))?;
        if let Message::Text(text) = msg {
            let env: Envelope = match serde_json::from_str(text.as_str()) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if env.msg_type == msg_type::HELLO {
                let ok = env
                    .payload
                    .get("token")
                    .and_then(|x| x.as_str())
                    == Some(expected_token.as_str());
                let reply = if ok {
                    Envelope::reply(msg_type::READY, serde_json::json!({}))
                } else {
                    Envelope::reply(
                        msg_type::RUN_ERROR,
                        serde_json::json!({ "message": "bad token" }),
                    )
                };
                let raw = serde_json::to_string(&reply).map_err(|e| AppError::Ws(e.to_string()))?;
                write
                    .send(Message::Text(raw.into()))
                    .await
                    .map_err(|e| AppError::Ws(e.to_string()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    /// 端到端握手：真实 Rust WS Server（run）对接真实 Python sidecar（app.main）。
    /// 验证协议契约：Python 在 payload.token 携带一次性 token，Rust 校验后回 ready。
    /// 需要环境里 `uv` 在 PATH 且 agent/ 已 uv sync。
    #[tokio::test]
    async fn python_sidecar_handshake_succeeds() {
        let token = "unit-test-token".to_string();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ws port");
        let port = listener.local_addr().unwrap().port();

        // 真实 Rust WS Server
        let srv_token = token.clone();
        tokio::spawn(async move {
            let _ = run(listener, srv_token).await;
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
            .expect("spawn python sidecar（确认 uv 在 PATH 且 agent/ 已 uv sync）");

        // 转发 sidecar stderr 便于诊断
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
        assert!(ok, "Python sidecar 未报告握手成功（未收到 ready 或 token 校验失败）");
    }
}
