//! shell 工具：bash -c 执行，env_clear + 白名单 + 超时 + 输出截断。
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct ShellTool;

fn resolve_cwd(ctx: &ToolCtx<'_>) -> std::path::PathBuf {
    let cwd_str = ctx.args.get("cwd").and_then(|x| x.as_str());
    let raw = match cwd_str {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => ctx
            .workspace_cwd
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string()),
    };
    let expanded = crate::tools::policy::expand_home(&raw);
    expanded.canonicalize().unwrap_or(expanded)
}

#[async_trait]
impl BuiltinTool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Execute a command in bash inside the current workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "Command to execute."},
                        "cwd": {"type": "string", "description": "Optional working directory; defaults to the workspace."}
                    },
                    "required": ["command"]
                }
            }
        })
    }

    fn risk(&self, args: &Value) -> Risk {
        let cmd = args.get("command").and_then(|x| x.as_str()).unwrap_or("");
        const HIGH_PATTERNS: &[&str] = &[
            "rm ",
            "rmdir",
            "sudo",
            "chmod 777",
            "dd ",
            "mkfs",
            "shutdown",
            "reboot",
            "curl ",
            "wget ",
            "nc ",
            "ssh ",
            "scp ",
        ];
        if HIGH_PATTERNS.iter().any(|p| cmd.contains(p)) || cmd.contains(">") || cmd.contains(">>")
        {
            Risk::High
        } else {
            Risk::Medium
        }
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let command_str = ctx
            .args
            .get("command")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `command` 参数".into()))?;

        let cwd_absolute = resolve_cwd(ctx);

        if let Err(e) = ctx.policy.check_shell(&cwd_absolute.to_string_lossy()) {
            ctx.record_failure(&e).await?;
            return Err(AppError::Other(e));
        }

        ctx.update_running(&cwd_absolute.to_string_lossy()).await?;

        let mut child = Command::new("bash");
        child.arg("-c").arg(command_str);
        child.current_dir(&cwd_absolute);
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        child.env_clear();
        for env_name in &ctx.policy.shell.env_allowlist {
            if let Some(val) = std::env::var_os(env_name) {
                child.env(env_name, val);
            }
        }

        let mut spawned = match child.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("无法生成 Shell 子进程: {e}");
                ctx.record_failure(&err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        };

        let timeout_duration = Duration::from_secs(ctx.policy.shell.timeout_sec as u64);
        let run_result = tokio::time::timeout(timeout_duration, spawned.wait()).await;

        match run_result {
            Ok(Ok(status)) => {
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                if let Some(mut stdout) = spawned.stdout.take() {
                    let _ = stdout
                        .take(ctx.policy.shell.max_output_bytes as u64)
                        .read_to_end(&mut stdout_buf)
                        .await;
                }
                if let Some(mut stderr) = spawned.stderr.take() {
                    let _ = stderr
                        .take(ctx.policy.shell.max_output_bytes as u64)
                        .read_to_end(&mut stderr_buf)
                        .await;
                }
                let stdout_str = String::from_utf8_lossy(&stdout_buf).to_string();
                let stderr_str = String::from_utf8_lossy(&stderr_buf).to_string();
                let exit_code = status.code().unwrap_or(-1);
                let success = status.success();
                let status_name = if success { "done" } else { "failed" };

                ctx.update_complete(
                    status_name,
                    Some(if success { "success" } else { "error" }),
                    Some(exit_code),
                    Some(stdout_str.clone()),
                    Some(stderr_str.clone()),
                )
                .await?;

                Ok(json!({ "exit_code": exit_code, "stdout": stdout_str, "stderr": stderr_str }))
            }
            Ok(Err(e)) => {
                let err_msg = format!("执行出错: {e}");
                ctx.record_failure(&err_msg).await?;
                Err(AppError::Other(err_msg))
            }
            Err(_) => {
                let _ = spawned.kill().await;
                let err_msg = format!("执行超时 (限制 {} 秒)", ctx.policy.shell.timeout_sec);
                ctx.update_complete("cancelled", None, Some(-9), None, Some(err_msg.clone()))
                    .await?;
                Err(AppError::Other(err_msg))
            }
        }
    }
}
