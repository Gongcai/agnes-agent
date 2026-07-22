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
        let Some(args) = args.get("args").and_then(Value::as_array).and_then(|args| {
            args.iter()
                .map(|arg| arg.as_str().map(ToString::to_string))
                .collect::<Option<Vec<_>>>()
        }) else {
            return Risk::High;
        };
        git_risk(&args)
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
        if let Err(error) = validate_git_args(&args) {
            ctx.record_failure(&error).await?;
            return Err(AppError::Other(error));
        }
        if let Err(error) = ctx.sandbox.check_cwd(&cwd_absolute) {
            ctx.record_failure(&error).await?;
            return Err(AppError::Other(error));
        }

        ctx.update_running(&cwd_absolute.to_string_lossy()).await?;

        let mut execution_args = vec![
            "--no-pager".to_string(),
            "-c".to_string(),
            "core.hooksPath=/dev/null".to_string(),
            "-c".to_string(),
            "commit.gpgSign=false".to_string(),
            "-c".to_string(),
            "tag.gpgSign=false".to_string(),
        ];
        execution_args.extend(args.iter().cloned());
        let mut child =
            match ctx
                .sandbox
                .git_command("git", &execution_args, &ctx.policy.shell.env_allowlist)
            {
                Ok(command) => command,
                Err(error) => {
                    ctx.record_failure(&error.to_string()).await?;
                    return Err(error);
                }
            };
        child.current_dir(&cwd_absolute);
        child.env("GIT_TERMINAL_PROMPT", "0");
        child.env("GIT_EDITOR", "true");
        child.env("GIT_SEQUENCE_EDITOR", "true");
        child.env("GIT_PAGER", "cat");

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

        let timeout_duration = Duration::from_secs(ctx.policy.git.timeout_sec as u64);
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
                let err_msg = format!("Git 执行超时 (限制 {} 秒)", ctx.policy.git.timeout_sec);
                ctx.update_complete("cancelled", None, Some(-9), None, Some(err_msg.clone()))
                    .await?;
                Err(AppError::Other(err_msg))
            }
        }
    }
}

fn git_risk(args: &[String]) -> Risk {
    let Some((operation_index, operation)) = git_operation(args) else {
        return Risk::High;
    };
    let operation_args = &args[operation_index + 1..];
    let high = operation == "push"
        || operation == "filter-branch"
        || (operation == "reset" && operation_args.iter().any(|arg| arg == "--hard"))
        || (operation == "clean" && operation_args.iter().any(|arg| clean_forces(arg)))
        || (operation == "config"
            && operation_args
                .iter()
                .any(|arg| arg == "--global" || arg == "--system"));
    if high {
        Risk::High
    } else {
        Risk::Low
    }
}

fn git_operation(args: &[String]) -> Option<(usize, &str)> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "-c" || arg == "-C" {
            index += 2;
            continue;
        }
        if arg.starts_with('-') {
            index += 1;
            continue;
        }
        return Some((index, arg));
    }
    None
}

fn clean_forces(arg: &str) -> bool {
    arg == "--force"
        || (arg.starts_with('-')
            && !arg.starts_with("--")
            && arg.chars().skip(1).any(|flag| flag == 'f'))
}

fn validate_git_args(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Git requires a subcommand".to_string());
    }
    for (index, arg) in args.iter().enumerate() {
        if arg == "-C"
            || arg.starts_with("--git-dir")
            || arg.starts_with("--work-tree")
            || arg.starts_with("--exec-path")
            || arg.starts_with("--namespace")
            || arg.starts_with("--super-prefix")
            || arg.starts_with("--config-env")
        {
            return Err(format!(
                "Git argument `{arg}` is not allowed; use the tool's `cwd` argument instead"
            ));
        }
        if arg == "-c" {
            let Some(value) = args.get(index + 1) else {
                return Err("Git `-c` requires a key=value argument".to_string());
            };
            validate_config_override(value)?;
        }
    }

    let Some((operation_index, operation)) = git_operation(args) else {
        return Err("Git requires a subcommand".to_string());
    };
    const ALLOWED: &[&str] = &[
        "add",
        "archive",
        "bisect",
        "blame",
        "branch",
        "cat-file",
        "check-attr",
        "check-ignore",
        "checkout",
        "cherry-pick",
        "clean",
        "clone",
        "commit",
        "config",
        "describe",
        "diff",
        "fetch",
        "for-each-ref",
        "grep",
        "hash-object",
        "init",
        "log",
        "ls-files",
        "merge",
        "merge-base",
        "mv",
        "name-rev",
        "notes",
        "pull",
        "push",
        "rebase",
        "reflog",
        "remote",
        "reset",
        "restore",
        "revert",
        "rev-parse",
        "rm",
        "shortlog",
        "show",
        "show-ref",
        "stash",
        "status",
        "switch",
        "symbolic-ref",
        "tag",
        "update-index",
        "update-ref",
    ];
    if !ALLOWED.contains(&operation) {
        return Err(format!("Git subcommand `{operation}` is not allowed"));
    }

    let operation_args = &args[operation_index + 1..];
    if operation == "config" {
        if operation_args.iter().any(|arg| {
            arg == "--global"
                || arg == "--system"
                || arg == "--file"
                || arg.starts_with("--file=")
                || arg == "--blob"
                || arg.starts_with("--blob=")
        }) {
            return Err("Git config is restricted to the current repository".to_string());
        }
        for arg in operation_args {
            validate_config_key(arg)?;
        }
    }
    if (operation == "diff" || operation == "log" || operation == "show")
        && operation_args
            .iter()
            .any(|arg| arg == "--ext-diff" || arg == "--textconv")
    {
        return Err("External git diff commands are not allowed".to_string());
    }
    if operation == "grep"
        && operation_args
            .iter()
            .any(|arg| arg.starts_with("--open-files-in-pager"))
    {
        return Err("Git grep cannot launch an external pager".to_string());
    }
    if operation == "rebase"
        && operation_args
            .iter()
            .any(|arg| arg == "--exec" || arg == "-x" || arg.starts_with("--exec="))
    {
        return Err("Git rebase cannot execute arbitrary commands".to_string());
    }
    if operation == "bisect" && operation_args.first().map(String::as_str) == Some("run") {
        return Err("Git bisect run is not allowed".to_string());
    }
    if operation == "clone"
        && operation_args
            .iter()
            .any(|arg| arg == "-u" || arg == "--upload-pack" || arg.starts_with("--upload-pack="))
    {
        return Err("Git clone cannot override the upload-pack command".to_string());
    }
    Ok(())
}

fn validate_config_override(value: &str) -> Result<(), String> {
    let key = value.split('=').next().unwrap_or(value);
    validate_config_key(key)
}

fn validate_config_key(value: &str) -> Result<(), String> {
    let normalized = value.to_ascii_lowercase();
    const BLOCKED_PREFIXES: &[&str] = &[
        "alias.",
        "credential.helper",
        "core.editor",
        "core.hookspath",
        "core.pager",
        "core.fsmonitor",
        "core.sshcommand",
        "diff.",
        "filter.",
        "merge.",
        "protocol.",
        "sequence.editor",
    ];
    if BLOCKED_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        Err(format!("Git config key `{value}` is not allowed"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn classifies_destructive_git_operations() {
        assert_eq!(git_risk(&args(&["status", "--short"])), Risk::Low);
        assert_eq!(git_risk(&args(&["reset", "HEAD~1"])), Risk::Low);
        assert_eq!(git_risk(&args(&["reset", "--hard"])), Risk::High);
        assert_eq!(git_risk(&args(&["clean", "-ndx"])), Risk::Low);
        assert_eq!(git_risk(&args(&["clean", "-fdx"])), Risk::High);
        assert_eq!(git_risk(&args(&["push", "origin", "main"])), Risk::High);
    }

    #[test]
    fn rejects_git_escape_hatches() {
        assert!(validate_git_args(&args(&["config", "--global", "user.name", "x"])).is_err());
        assert!(validate_git_args(&args(&["-C", "/tmp", "status"])).is_err());
        assert!(validate_git_args(&args(&["filter-branch"])).is_err());
        assert!(validate_git_args(&args(&["-c", "alias.pwn=!sh", "pwn"])).is_err());
        assert!(validate_git_args(&args(&["rebase", "-x", "touch outside"])).is_err());
        assert!(validate_git_args(&args(&["bisect", "run", "sh", "test.sh"])).is_err());
        assert!(validate_git_args(&args(&["status", "--short"])).is_ok());
    }
}
