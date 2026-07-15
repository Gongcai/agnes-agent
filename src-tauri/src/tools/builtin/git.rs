//! git 工具：在 workspace/指定 cwd 内执行 git 子命令。
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct GitTool;

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
        let args_val = ctx
            .args
            .get("args")
            .and_then(|x| x.as_array())
            .ok_or_else(|| AppError::Other("缺少 `args` 参数".into()))?;
        let args: Vec<String> = args_val
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        let cwd_absolute = resolve_cwd(ctx);

        if let Err(e) = ctx.policy.check_git() {
            ctx.record_failure(&e).await?;
            return Err(AppError::Other(e));
        }

        ctx.update_running(&cwd_absolute.to_string_lossy()).await?;

        let mut child = Command::new("git");
        child.args(&args);
        child.current_dir(&cwd_absolute);
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());

        let spawned = match child.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("无法生成 Git 子进程: {e}");
                ctx.record_failure(&err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        };

        // Git 默认超时 30 秒
        let timeout_duration = Duration::from_secs(30);
        let run_result = tokio::time::timeout(timeout_duration, spawned.wait_with_output()).await;

        match run_result {
            Ok(Ok(output)) => {
                let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code().unwrap_or(-1);
                let success = output.status.success();
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
                let err_msg = format!("执行 Git 出错: {e}");
                ctx.record_failure(&err_msg).await?;
                Err(AppError::Other(err_msg))
            }
            Err(_) => {
                let err_msg = "Git 执行超时 (限制 30 秒)";
                ctx.update_complete("cancelled", None, Some(-9), None, Some(err_msg.to_string()))
                    .await?;
                Err(AppError::Other(err_msg.to_string()))
            }
        }
    }
}
