//! 内置工具统一 trait 与执行上下文。
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;

use crate::db::DbActorHandle;
use crate::error::AppResult;
use crate::tools::policy::ToolPolicy;

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
