//! 内置工具统一 trait 与执行上下文。
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::db::DbActorHandle;
use crate::error::AppResult;
use crate::tools::policy::{ApprovalTier, Risk, ToolPolicy};

pub mod shell;
pub mod file_read;
pub mod file_write;
pub mod git;

/// 工具执行上下文：包含审计所需的 DB 句柄、参数、policy 与 workspace cwd。
pub struct ToolCtx<'a> {
    pub db: &'a DbActorHandle,
    pub session_id: &'a str,
    pub tool_call_id: &'a str,
    pub args: &'a Value,
    /// 已合并 workspace cwd 的有效 policy（workspace 自动加入 allowed_cwd/allowed_roots）
    pub policy: &'a ToolPolicy,
    pub workspace_cwd: Option<PathBuf>,
}

impl<'a> ToolCtx<'a> {
    /// 记录运行中状态（含 cwd）。
    pub async fn update_running(&self, cwd: &str) -> AppResult<()> {
        self.db
            .update_tool_call_running(self.tool_call_id.to_string(), cwd.to_string())
            .await
    }

    /// 记录执行完成（成功/失败）。
    pub async fn update_complete(
        &self,
        status: &str,
        result_kind: Option<&str>,
        exit_code: Option<i32>,
        stdout: Option<String>,
        stderr: Option<String>,
    ) -> AppResult<()> {
        self.db
            .update_tool_call_complete(
                self.tool_call_id.to_string(),
                status.to_string(),
                result_kind.map(|s| s.to_string()),
                exit_code,
                stdout,
                stderr,
            )
            .await
    }

    /// 统一失败落库。
    pub async fn record_failure(&self, err_msg: &str) -> AppResult<()> {
        self.update_complete("failed", None, Some(-1), None, Some(err_msg.to_string()))
            .await
    }
}

/// 内置工具统一接口。
#[async_trait]
pub trait BuiltinTool: Send + Sync {
    /// 该工具在给定参数下的风险等级。
    fn risk(&self, args: &Value) -> Risk;
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value>;
}

/// 注册表：返回所有内置工具实例（按 name 派发）。
pub fn builtin_tools() -> Vec<(&'static str, Box<dyn BuiltinTool>)> {
    vec![
        ("shell", Box::new(shell::ShellTool)),
        ("file_read", Box::new(file_read::FileReadTool)),
        ("file_write", Box::new(file_write::FileWriteTool)),
        ("git", Box::new(git::GitTool)),
    ]
}

/// 计算某工具调用在给定参数下的风险等级。
pub fn compute_risk(tool: &str, args: &Value) -> Risk {
    for (name, impl_) in builtin_tools() {
        if name == tool {
            return impl_.risk(args);
        }
    }
    Risk::Low
}

/// 判断操作是否为写操作（用于 OnWrite tier）。
pub fn is_write_op(tool: &str, args: &Value) -> bool {
    match tool {
        "file_write" | "file_edit" | "apply_patch" => true,
        "file_read" | "list_files" | "grep" => false,
        "shell" => {
            let cmd = args.get("command").and_then(|x| x.as_str()).unwrap_or("");
            const WRITE_PATTERNS: &[&str] = &[
                " > ", " >> ", ">", ">>", "tee", "rm ", "rmdir", "mv ", "cp ", "mkdir",
                "touch", "dd ", "chmod", "chown", "sed -i", "install",
            ];
            cmd.contains('>') || WRITE_PATTERNS.iter().any(|p| cmd.contains(p))
        }
        "git" => {
            let arr = args.get("args").and_then(|x| x.as_array());
            const WRITE_CMDS: &[&str] = &[
                "push", "commit", "reset", "clean", "merge", "rebase", "checkout",
                "cherry-pick", "stash", "tag", "add", "rm", "mv", "init",
            ];
            arr.map(|a| {
                a.iter()
                    .any(|v| v.as_str().map(|s| WRITE_CMDS.contains(&s)).unwrap_or(false))
            })
            .unwrap_or(false)
        }
        _ => false,
    }
}

/// 统一审批判定：tier + risk + 是否写操作。
pub fn needs_approval(tool: &str, args: &Value, policy: &ToolPolicy) -> bool {
    let tier = policy.approval_for(tool);
    let risk = compute_risk(tool, args);
    match tier {
        ApprovalTier::Never => false,
        ApprovalTier::OnWrite => is_write_op(tool, args),
        ApprovalTier::OnRisk => risk == Risk::High,
        ApprovalTier::Always => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn risk_drives_on_risk_approval() {
        let policy = ToolPolicy::default();
        assert!(!needs_approval(
            "shell",
            &json!({"command": "cargo test"}),
            &policy
        ));
        assert!(needs_approval(
            "shell",
            &json!({"command": "rm -rf target"}),
            &policy
        ));
        assert!(needs_approval(
            "git",
            &json!({"args": ["push", "origin", "main"]}),
            &policy
        ));
        assert!(!needs_approval(
            "git",
            &json!({"args": ["status", "--short"]}),
            &policy
        ));
    }

    #[test]
    fn on_write_distinguishes_read_and_write_operations() {
        let mut policy = ToolPolicy::default();
        policy.shell.approval = ApprovalTier::OnWrite;
        assert!(!needs_approval(
            "shell",
            &json!({"command": "rg TODO src"}),
            &policy
        ));
        assert!(needs_approval(
            "shell",
            &json!({"command": "echo done > result.txt"}),
            &policy
        ));
        assert!(needs_approval(
            "file_write",
            &json!({"path": "result.txt", "content": "done"}),
            &policy
        ));
        assert!(!needs_approval(
            "file_read",
            &json!({"path": "result.txt"}),
            &policy
        ));
    }
}
