//! PTY-backed shell tools with incremental polling and process-group cleanup.
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct ShellTool;
pub struct WriteStdinTool;
pub struct StopTerminalTool;

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
                        "cwd": {"type": "string", "description": "Optional working directory; defaults to the workspace."},
                        "yield_time_ms": {"type": "integer", "minimum": 0, "maximum": 30000, "description": "How long to wait before returning a running terminal session."},
                        "timeout_sec": {"type": "integer", "minimum": 0, "maximum": 86400, "description": "Optional total command lifetime; zero means no hard deadline."}
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
        let command =
            match ctx
                .sandbox
                .command_spec("bash", &command_args, &ctx.policy.shell.env_allowlist)
            {
                Ok(command) => command,
                Err(error) => {
                    ctx.record_failure(&error.to_string()).await?;
                    return Err(error);
                }
            };
        let timeout_sec = ctx
            .args
            .get("timeout_sec")
            .and_then(Value::as_u64)
            .map(|value| value.min(86_400) as u32)
            .unwrap_or(ctx.policy.shell.timeout_sec);
        let run_id = ctx.run_id.unwrap_or(ctx.tool_call_id);
        let terminal_id = match ctx.terminals.spawn(
            command,
            &cwd_absolute,
            ctx.session_id,
            run_id,
            ctx.policy.shell.max_output_bytes as usize,
            timeout_sec,
        ) {
            Ok(id) => id,
            Err(error) => {
                ctx.record_failure(&error.to_string()).await?;
                return Err(error);
            }
        };
        let poll = ctx
            .terminals
            .poll(
                &terminal_id,
                ctx.session_id,
                yield_duration(ctx.args, 10_000),
            )
            .await?;
        complete_tool_call(ctx, &poll).await
    }
}

#[async_trait]
impl BuiltinTool for WriteStdinTool {
    fn name(&self) -> &'static str {
        "write_stdin"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Send input to or poll a running terminal session.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "session_id": {"type": "string"},
                        "chars": {"type": "string", "description": "Characters to send; omit or use an empty string to poll only."},
                        "yield_time_ms": {"type": "integer", "minimum": 0, "maximum": 30000}
                    },
                    "required": ["session_id"]
                }
            }
        })
    }

    fn risk(&self, args: &Value) -> Risk {
        if args
            .get("chars")
            .and_then(Value::as_str)
            .is_some_and(|chars| !chars.is_empty())
        {
            Risk::High
        } else {
            Risk::Low
        }
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        if !ctx.policy.shell.enabled {
            return fail(ctx, "Shell execution tools are disabled").await;
        }
        let terminal_id = required_string(ctx, "session_id").await?;
        let chars = ctx.args.get("chars").and_then(Value::as_str).unwrap_or("");
        if chars.len() > 64 * 1024 {
            return fail(ctx, "Terminal input exceeds 64 KiB").await;
        }
        ctx.update_running(&terminal_id).await?;
        let poll = match ctx
            .terminals
            .write_and_poll(
                &terminal_id,
                ctx.session_id,
                chars,
                yield_duration(ctx.args, 1_000),
            )
            .await
        {
            Ok(poll) => poll,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        complete_tool_call(ctx, &poll).await
    }
}

#[async_trait]
impl BuiltinTool for StopTerminalTool {
    fn name(&self) -> &'static str {
        "stop_terminal"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Stop a running terminal session and all of its descendant processes.",
                "parameters": {
                    "type": "object",
                    "properties": {"session_id": {"type": "string"}},
                    "required": ["session_id"]
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        if !ctx.policy.shell.enabled {
            return fail(ctx, "Shell execution tools are disabled").await;
        }
        let terminal_id = required_string(ctx, "session_id").await?;
        ctx.update_running(&terminal_id).await?;
        if let Err(error) = ctx.terminals.stop(&terminal_id, ctx.session_id) {
            return fail(ctx, &error.to_string()).await;
        }
        let poll = ctx
            .terminals
            .poll(&terminal_id, ctx.session_id, Duration::from_secs(2))
            .await?;
        complete_tool_call(ctx, &poll).await
    }
}

fn yield_duration(args: &Value, default_ms: u64) -> Duration {
    Duration::from_millis(
        args.get("yield_time_ms")
            .and_then(Value::as_u64)
            .unwrap_or(default_ms)
            .min(30_000),
    )
}

async fn complete_tool_call(
    ctx: &ToolCtx<'_>,
    poll: &crate::tools::terminal::TerminalPoll,
) -> AppResult<Value> {
    let stdout = render_terminal_output(poll);
    let exit_code = poll.exit_code.unwrap_or(0);
    ctx.update_complete(
        "done",
        Some(if poll.running { "running" } else { "exit" }),
        (!poll.running).then_some(exit_code),
        Some(stdout.clone()),
        None,
    )
    .await?;
    Ok(json!({
        "session_id": poll.session_id,
        "status": if poll.running { "running" } else { "exited" },
        "exit_code": exit_code,
        "stdout": stdout,
        "stderr": "",
        "truncated": poll.truncated,
        "signal": poll.signal,
    }))
}

fn render_terminal_output(poll: &crate::tools::terminal::TerminalPoll) -> String {
    let mut output = poll.output.clone();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    if poll.running {
        output.push_str(&format!(
            "[terminal session_id={} status=running; use write_stdin to poll or send input, or stop_terminal to stop it]",
            poll.session_id
        ));
    } else {
        output.push_str(&format!(
            "[terminal session_id={} status=exited exit_code={}]",
            poll.session_id,
            poll.exit_code.unwrap_or(-1)
        ));
    }
    output
}

async fn required_string(ctx: &ToolCtx<'_>, key: &str) -> AppResult<String> {
    match ctx
        .args
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(value) => Ok(value.to_string()),
        None => fail(ctx, &format!("Missing or empty `{key}` argument")).await,
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
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
