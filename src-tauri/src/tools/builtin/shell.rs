//! shell 工具：bash -c 执行，env_clear + 白名单 + 超时 + 输出截断。
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct ShellTool;

fn resolve_cwd(ctx: &ToolCtx<'_>) -> std::path::PathBuf {
    let raw = ctx
        .args
        .get("cwd")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(".");
    crate::tools::builtin::normalize_path(&crate::tools::builtin::resolve_path(ctx, raw))
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
        let command_str = match ctx.args.get("command").and_then(|value| value.as_str()) {
            Some(command) => command,
            None => {
                let error = "Missing `command` argument";
                ctx.record_failure(error).await?;
                return Err(AppError::Other(error.to_string()));
            }
        };

        let cwd_absolute = resolve_cwd(ctx);

        if let Err(e) = ctx.policy.check_shell(&cwd_absolute.to_string_lossy()) {
            ctx.record_failure(&e).await?;
            return Err(AppError::Other(e));
        }
        if let Err(error) = ctx.sandbox.check_cwd(&cwd_absolute) {
            ctx.record_failure(&error).await?;
            return Err(AppError::Other(error));
        }

        ctx.update_running(&cwd_absolute.to_string_lossy()).await?;

        let command_args = vec!["-c".to_string(), command_str.to_string()];
        let mut child =
            match ctx
                .sandbox
                .command("bash", &command_args, &ctx.policy.shell.env_allowlist)
            {
                Ok(command) => command,
                Err(error) => {
                    ctx.record_failure(&error.to_string()).await?;
                    return Err(error);
                }
            };
        child.current_dir(&cwd_absolute);

        let mut spawned = match child.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("无法生成 Shell 子进程: {e}");
                ctx.record_failure(&err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        };

        let max_output = ctx.policy.shell.max_output_bytes as usize;
        let stdout_task = spawned.stdout.take().map(|stdout| {
            tokio::spawn(crate::tools::sandbox::read_stream_capped(
                stdout, max_output,
            ))
        });
        let stderr_task = spawned.stderr.take().map(|stderr| {
            tokio::spawn(crate::tools::sandbox::read_stream_capped(
                stderr, max_output,
            ))
        });

        let timeout_duration = Duration::from_secs(ctx.policy.shell.timeout_sec as u64);
        let run_result = tokio::time::timeout(timeout_duration, spawned.wait()).await;

        match run_result {
            Ok(Ok(status)) => {
                let (stdout_buf, stdout_truncated) =
                    crate::tools::sandbox::join_capture(stdout_task).await;
                let (stderr_buf, stderr_truncated) =
                    crate::tools::sandbox::join_capture(stderr_task).await;
                let stdout_str =
                    crate::tools::sandbox::render_captured_output(stdout_buf, stdout_truncated);
                let stderr_str =
                    crate::tools::sandbox::render_captured_output(stderr_buf, stderr_truncated);
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
                let _ = crate::tools::sandbox::join_capture(stdout_task).await;
                let _ = crate::tools::sandbox::join_capture(stderr_task).await;
                let err_msg = format!("执行出错: {e}");
                ctx.record_failure(&err_msg).await?;
                Err(AppError::Other(err_msg))
            }
            Err(_) => {
                let _ = spawned.kill().await;
                let _ = spawned.wait().await;
                let _ = crate::tools::sandbox::join_capture(stdout_task).await;
                let _ = crate::tools::sandbox::join_capture(stderr_task).await;
                let err_msg = format!("执行超时 (限制 {} 秒)", ctx.policy.shell.timeout_sec);
                ctx.update_complete("cancelled", None, Some(-9), None, Some(err_msg.clone()))
                    .await?;
                Err(AppError::Other(err_msg))
            }
        }
    }
}
