//! shell 工具：bash -c 执行，env_clear + 白名单 + 超时 + 输出截断。
use std::path::Path;
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
        const HIGH_PROGRAMS: &[&str] = &[
            "rm", "rmdir", "mv", "sudo", "dd", "mkfs", "shutdown", "reboot", "curl", "wget", "nc",
            "ncat", "ssh", "scp", "sftp",
        ];
        let tokens = shell_tokens(cmd).unwrap_or_default();
        let has_high_program = tokens.iter().any(|token| {
            Path::new(token)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| HIGH_PROGRAMS.contains(&name))
        });
        let chmod_world_writable = tokens.windows(2).any(|pair| {
            Path::new(&pair[0])
                .file_name()
                .and_then(|name| name.to_str())
                == Some("chmod")
                && pair[1] == "777"
        });
        if has_high_program
            || chmod_world_writable
            || tokens.iter().any(|token| token == ">" || token == ">>")
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
        if ctx.policy.shell.deny_write_outside_workspace {
            if let Err(error) =
                validate_shell_write_scope(command_str, &cwd_absolute, ctx.sandbox, 0)
            {
                ctx.record_failure(&error).await?;
                return Err(AppError::Other(error));
            }
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

fn validate_shell_write_scope(
    command: &str,
    cwd: &Path,
    sandbox: &dyn crate::tools::sandbox::SandboxGuard,
    depth: usize,
) -> Result<(), String> {
    if depth > 3 {
        return Err("Nested shell command exceeds the safety inspection limit".to_string());
    }
    let tokens = shell_tokens(command)?;
    for (index, token) in tokens.iter().enumerate() {
        if token == ">" || token == ">>" {
            let target = tokens
                .get(index + 1)
                .ok_or_else(|| "Output redirection is missing a target".to_string())?;
            validate_write_target(target, cwd, sandbox)?;
        }
    }

    for segment in shell_segments(&tokens) {
        let Some((command_index, program)) = segment_command(segment) else {
            continue;
        };
        let arguments = &segment[command_index + 1..];
        match program.as_str() {
            "tee" | "touch" | "mkdir" | "rmdir" | "rm" | "truncate" | "ln" | "mv" => {
                for target in arguments
                    .iter()
                    .filter(|argument| !argument.starts_with('-'))
                {
                    validate_write_target(target, cwd, sandbox)?;
                }
            }
            "cp" | "install" => {
                if let Some(target) = arguments
                    .iter()
                    .rev()
                    .find(|argument| !argument.starts_with('-'))
                {
                    validate_write_target(target, cwd, sandbox)?;
                }
            }
            "dd" => {
                for argument in arguments {
                    if let Some(target) = argument.strip_prefix("of=") {
                        validate_write_target(target, cwd, sandbox)?;
                    }
                }
            }
            "sed" if arguments.iter().any(|argument| argument.starts_with("-i")) => {
                for target in arguments
                    .iter()
                    .rev()
                    .take_while(|argument| !argument.starts_with('-'))
                {
                    validate_write_target(target, cwd, sandbox)?;
                }
            }
            "find" if arguments.iter().any(|argument| argument == "-delete") => {
                if let Some(target) = arguments.first() {
                    validate_write_target(target, cwd, sandbox)?;
                }
            }
            "bash" | "sh" | "zsh" => {
                if let Some(flag_index) = arguments.iter().position(|argument| argument == "-c") {
                    let nested = arguments.get(flag_index + 1).ok_or_else(|| {
                        "Nested shell `-c` invocation is missing its command".to_string()
                    })?;
                    validate_shell_write_scope(nested, cwd, sandbox, depth + 1)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn command_is_write(command: &str) -> bool {
    let Ok(tokens) = shell_tokens(command) else {
        return true;
    };
    if tokens.iter().any(|token| token == ">" || token == ">>") {
        return true;
    }
    shell_segments(&tokens).iter().any(|segment| {
        let Some((command_index, program)) = segment_command(segment) else {
            return false;
        };
        const WRITE_PROGRAMS: &[&str] = &[
            "tee", "touch", "mkdir", "rmdir", "rm", "truncate", "ln", "mv", "cp", "install", "dd",
            "chmod", "chown",
        ];
        if WRITE_PROGRAMS.contains(&program.as_str()) {
            return true;
        }
        let arguments = &segment[command_index + 1..];
        (program == "sed" && arguments.iter().any(|argument| argument.starts_with("-i")))
            || (program == "find" && arguments.iter().any(|argument| argument == "-delete"))
            || (program == "git"
                && arguments.iter().any(|argument| {
                    [
                        "add",
                        "commit",
                        "reset",
                        "clean",
                        "merge",
                        "rebase",
                        "checkout",
                        "switch",
                        "restore",
                        "cherry-pick",
                        "revert",
                        "stash",
                        "tag",
                        "push",
                        "pull",
                        "fetch",
                        "clone",
                        "rm",
                        "mv",
                        "init",
                    ]
                    .contains(&argument.as_str())
                }))
    })
}

fn validate_write_target(
    target: &str,
    cwd: &Path,
    sandbox: &dyn crate::tools::sandbox::SandboxGuard,
) -> Result<(), String> {
    if target.starts_with('&') || target == "/dev/null" {
        return Ok(());
    }
    if target.is_empty()
        || target
            .chars()
            .any(|character| matches!(character, '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}'))
    {
        return Err(format!(
            "Cannot safely resolve dynamic shell write target `{target}`"
        ));
    }
    let expanded = crate::tools::policy::expand_home(target);
    let path = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    sandbox.check_write(&crate::tools::builtin::normalize_path(&path))
}

fn segment_command(segment: &[String]) -> Option<(usize, String)> {
    let mut index = 0;
    while index < segment.len()
        && (segment[index].contains('=') && !segment[index].starts_with('='))
    {
        index += 1;
    }
    if segment.get(index).map(String::as_str) == Some("command") {
        index += 1;
    }
    if segment.get(index).map(String::as_str) == Some("env") {
        index += 1;
        while index < segment.len() && segment[index].contains('=') {
            index += 1;
        }
    }
    if segment.get(index).map(String::as_str) == Some("sudo") {
        index += 1;
        while index < segment.len() && segment[index].starts_with('-') {
            index += 1;
        }
    }
    let program = Path::new(segment.get(index)?)
        .file_name()
        .and_then(|name| name.to_str())?
        .to_string();
    Some((index, program))
}

fn shell_segments(tokens: &[String]) -> Vec<&[String]> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, token) in tokens.iter().enumerate() {
        if matches!(token.as_str(), ";" | "|" | "&&" | "||" | "\n") {
            if start < index {
                segments.push(&tokens[start..index]);
            }
            start = index + 1;
        }
    }
    if start < tokens.len() {
        segments.push(&tokens[start..]);
    }
    segments
}

fn shell_tokens(command: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else if character == '\\' && active_quote == '"' && index + 1 < chars.len() {
                index += 1;
                current.push(chars[index]);
            } else {
                current.push(character);
            }
            index += 1;
            continue;
        }

        match character {
            '\'' | '"' => quote = Some(character),
            '\\' if index + 1 < chars.len() => {
                index += 1;
                current.push(chars[index]);
            }
            ' ' | '\t' | '\r' => push_token(&mut tokens, &mut current),
            '\n' => {
                push_token(&mut tokens, &mut current);
                tokens.push("\n".to_string());
            }
            '>' => {
                push_token(&mut tokens, &mut current);
                if chars.get(index + 1) == Some(&'>') {
                    tokens.push(">>".to_string());
                    index += 1;
                } else {
                    tokens.push(">".to_string());
                }
                if chars.get(index + 1) == Some(&'|') {
                    index += 1;
                }
                if chars.get(index + 1) == Some(&'&') {
                    index += 1;
                    let mut descriptor = String::from("&");
                    while chars
                        .get(index + 1)
                        .is_some_and(|next| next.is_ascii_digit())
                    {
                        index += 1;
                        descriptor.push(chars[index]);
                    }
                    tokens.push(descriptor);
                }
            }
            '&' if chars.get(index + 1) == Some(&'>') => {
                push_token(&mut tokens, &mut current);
                tokens.push(">".to_string());
                index += 1;
            }
            '&' | '|' | ';' => {
                push_token(&mut tokens, &mut current);
                let mut operator = character.to_string();
                if chars.get(index + 1) == Some(&character) && character != ';' {
                    operator.push(character);
                    index += 1;
                }
                tokens.push(operator);
            }
            _ => current.push(character),
        }
        index += 1;
    }
    if let Some(quote) = quote {
        return Err(format!("Unclosed `{quote}` quote in shell command"));
    }
    push_token(&mut tokens, &mut current);
    Ok(tokens)
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        tokens.push(std::mem::take(current));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_redirections_and_nested_operators() {
        assert_eq!(
            shell_tokens("echo ok 2>/dev/null && echo done >| result.txt").unwrap(),
            [
                "echo",
                "ok",
                "2",
                ">",
                "/dev/null",
                "&&",
                "echo",
                "done",
                ">",
                "result.txt"
            ]
        );
    }

    #[test]
    fn detects_shell_write_operations() {
        assert!(!command_is_write("rg TODO src"));
        assert!(command_is_write("echo done\t>result.txt"));
        assert!(command_is_write("git commit -m test"));
        assert!(command_is_write("sed -i 's/a/b/' file.txt"));
    }
}
