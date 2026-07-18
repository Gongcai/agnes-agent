//! 内置工具统一 trait 与执行上下文。
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::db::DbActorHandle;
use crate::error::AppResult;
use crate::tools::policy::{ApprovalTier, Risk, ToolPolicy};
use crate::tools::sandbox::SandboxGuard;

pub mod apply_patch;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod git;
pub mod grep;
pub mod list_files;
pub mod memory_create;
pub(crate) mod memory_entry;
pub mod memory_md_edit;
pub mod memory_md_view;
pub mod memory_search;
pub mod memory_update;
pub mod planner;
pub mod shell;
pub mod web;

/// 工具执行上下文：包含审计所需的 DB 句柄、参数、policy 与 workspace cwd。
pub struct ToolCtx<'a> {
    pub db: &'a DbActorHandle,
    pub session_id: &'a str,
    pub tool_call_id: &'a str,
    pub args: &'a Value,
    /// 已合并 workspace cwd 的有效 policy（workspace 自动加入 allowed_cwd/allowed_roots）
    pub policy: &'a ToolPolicy,
    pub workspace_cwd: Option<PathBuf>,
    pub sandbox: &'a dyn SandboxGuard,
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
    /// Stable name used in the LLM protocol.
    fn name(&self) -> &'static str;
    /// JSON schema exposed to model providers.
    fn schema(&self) -> Value;
    /// Risk for the supplied arguments.
    fn risk(&self, args: &Value) -> Risk;
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value>;
}

/// Return all registered built-in tools.
pub fn builtin_tools() -> Vec<Box<dyn BuiltinTool>> {
    vec![
        Box::new(shell::ShellTool),
        Box::new(file_read::FileReadTool),
        Box::new(file_write::FileWriteTool),
        Box::new(file_edit::FileEditTool),
        Box::new(list_files::ListFilesTool),
        Box::new(grep::GrepTool),
        Box::new(apply_patch::ApplyPatchTool),
        Box::new(git::GitTool),
        Box::new(memory_search::MemorySearchTool),
        Box::new(memory_create::MemoryCreateTool),
        Box::new(memory_update::MemoryUpdateTool),
        Box::new(memory_md_view::MemoryMdViewTool),
        Box::new(memory_md_edit::MemoryMdEditTool),
        Box::new(web::WebSearchTool),
        Box::new(web::WebFetchTool),
        Box::new(planner::CalendarListTool),
        Box::new(planner::CalendarCreateTool),
        Box::new(planner::CalendarEventCreateTool),
        Box::new(planner::CalendarUpdateTool),
        Box::new(planner::TaskListTool),
        Box::new(planner::TaskCreateTool),
        Box::new(planner::TaskCompleteTool),
        Box::new(planner::TaskUpdateTool),
    ]
}

/// Resolve a file argument relative to the current workspace.
pub fn resolve_path(ctx: &ToolCtx<'_>, path: &str) -> PathBuf {
    let expanded = crate::tools::policy::expand_home(path);
    if expanded.is_absolute() {
        return expanded;
    }

    let base = ctx
        .workspace_cwd
        .clone()
        .or_else(|| {
            ctx.policy
                .shell
                .allowed_cwd
                .first()
                .map(|path| crate::tools::policy::expand_home(path))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(expanded)
}

/// Resolve a path as far as possible without requiring the leaf to exist.
pub fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            return path.to_path_buf();
        };
        if parent == existing {
            return path.to_path_buf();
        }
        existing = parent;
    }
    if let Ok(mut canonical) = existing.canonicalize() {
        for component in missing.into_iter().rev() {
            canonical.push(component);
        }
        canonical
    } else {
        path.to_path_buf()
    }
}

/// Compute the risk of a registered tool call.
pub fn compute_risk(tool: &str, args: &Value) -> Risk {
    for impl_ in builtin_tools() {
        if impl_.name() == tool {
            return impl_.risk(args);
        }
    }
    Risk::High
}

/// 判断操作是否为写操作（用于 OnWrite tier）。
pub fn is_write_op(tool: &str, args: &Value) -> bool {
    match tool {
        "file_write"
        | "file_edit"
        | "apply_patch"
        | "memory_create"
        | "memory_update"
        | "memory_md_edit"
        | "calendar_create"
        | "calendar_event_create"
        | "calendar_update"
        | "task_create"
        | "task_complete"
        | "task_update" => true,
        "file_read" | "list_files" | "grep" | "memory_search" | "memory_md_view" | "web_search"
        | "web_fetch" | "calendar_list" | "task_list" => false,
        "shell" => {
            let cmd = args.get("command").and_then(|x| x.as_str()).unwrap_or("");
            shell::command_is_write(cmd)
        }
        "git" => {
            let arr = args.get("args").and_then(|x| x.as_array());
            const WRITE_CMDS: &[&str] = &[
                "push",
                "commit",
                "reset",
                "clean",
                "merge",
                "rebase",
                "checkout",
                "cherry-pick",
                "stash",
                "tag",
                "add",
                "archive",
                "bisect",
                "config",
                "fetch",
                "hash-object",
                "notes",
                "rm",
                "mv",
                "init",
                "pull",
                "restore",
                "revert",
                "switch",
                "update-index",
                "update-ref",
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

    #[test]
    fn registry_contains_every_declared_builtin() {
        let names: Vec<_> = builtin_tools()
            .into_iter()
            .map(|tool| tool.name())
            .collect();
        assert_eq!(
            names,
            vec![
                "shell",
                "file_read",
                "file_write",
                "file_edit",
                "list_files",
                "grep",
                "apply_patch",
                "git",
                "memory_search",
                "memory_create",
                "memory_update",
                "memory_md_view",
                "memory_md_edit",
                "web_search",
                "web_fetch",
                "calendar_list",
                "calendar_create",
                "calendar_event_create",
                "calendar_update",
                "task_list",
                "task_create",
                "task_complete",
                "task_update"
            ]
        );
        assert!(builtin_tools()
            .into_iter()
            .all(|tool| tool.schema()["function"]["name"] == tool.name()));
        for tool in ["memory_create", "memory_update"] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::Medium);
            assert!(is_write_op(tool, &json!({})));
        }
        for tool in [
            "calendar_create",
            "calendar_event_create",
            "calendar_update",
            "task_create",
            "task_complete",
            "task_update",
        ] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::High);
            assert!(is_write_op(tool, &json!({})));
        }
        for tool in ["calendar_list", "task_list"] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::Low);
            assert!(!is_write_op(tool, &json!({})));
        }
        for tool in ["web_search", "web_fetch"] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::Low);
            assert!(!is_write_op(tool, &json!({})));
        }
    }
}
