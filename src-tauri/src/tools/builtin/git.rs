//! git 工具：在 workspace/指定 cwd 内执行 git 子命令。
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct GitTool;

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
impl BuiltinTool for GitTool {
    fn name(&self) -> &'static str {
        "git"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Run a git operation inside the current workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Arguments passed directly to git, for example ['status', '--short']."
                        },
                        "cwd": {"type": "string", "description": "Optional working directory; defaults to the workspace."}
                    },
                    "required": ["args"]
                }
            }
        })
    }

    fn risk(&self, args: &Value) -> Risk {
        let arr = args.get("args").and_then(|x| x.as_array());
        const HIGH_CMDS: &[&str] = &["push", "reset", "clean", "filter-branch"];
        let is_high = |a: &Value| a.as_str().map(|s| HIGH_CMDS.contains(&s)).unwrap_or(false);
        if arr.map(|a| a.iter().any(is_high)).unwrap_or(false) {
            Risk::High
        } else {
            Risk::Low
        }
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args_val = match ctx.args.get("args").and_then(|value| value.as_array()) {
            Some(args) => args,
            None => {
                let error = "Missing `args` argument";
                ctx.record_failure(error).await?;
                return Err(AppError::Other(error.to_string()));
            }
        };
        let args = match args_val
            .iter()
            .map(|value| value.as_str().map(ToString::to_string))
            .collect::<Option<Vec<_>>>()
        {
            Some(args) => args,
            None => {
                let error = "Every git argument must be a string";
                ctx.record_failure(error).await?;
                return Err(AppError::Other(error.to_string()));
            }
        };

        let cwd_absolute = resolve_cwd(ctx);

        if let Err(e) = ctx.policy.check_git() {
            ctx.record_failure(&e).await?;
            return Err(AppError::Other(e));
        }
        if let Err(error) = ctx.sandbox.check_cwd(&cwd_absolute) {
            ctx.record_failure(&error).await?;
            return Err(AppError::Other(error));
        }

        ctx.update_running(&cwd_absolute.to_string_lossy()).await?;

        let mut child = match ctx
            .sandbox
            .command("git", &args, &ctx.policy.shell.env_allowlist)
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
                let err_msg = format!("无法生成 Git 子进程: {e}");
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

        // Git 默认超时 30 秒
        let timeout_duration = Duration::from_secs(30);
        let run_result = tokio::time::timeout(timeout_duration, spawned.wait()).await;

        match run_result {
            Ok(Ok(status)) => {
                let (stdout, stdout_truncated) =
                    crate::tools::sandbox::join_capture(stdout_task).await;
                let (stderr, stderr_truncated) =
                    crate::tools::sandbox::join_capture(stderr_task).await;
                let stdout_str =
                    crate::tools::sandbox::render_captured_output(stdout, stdout_truncated);
                let stderr_str =
                    crate::tools::sandbox::render_captured_output(stderr, stderr_truncated);
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
                let err_msg = format!("执行 Git 出错: {e}");
                ctx.record_failure(&err_msg).await?;
                Err(AppError::Other(err_msg))
            }
            Err(_) => {
                let _ = spawned.kill().await;
                let _ = spawned.wait().await;
                let _ = crate::tools::sandbox::join_capture(stdout_task).await;
                let _ = crate::tools::sandbox::join_capture(stderr_task).await;
                let err_msg = "Git 执行超时 (限制 30 秒)";
                ctx.update_complete("cancelled", None, Some(-9), None, Some(err_msg.to_string()))
                    .await?;
                Err(AppError::Other(err_msg.to_string()))
            }
        }
    }
}
