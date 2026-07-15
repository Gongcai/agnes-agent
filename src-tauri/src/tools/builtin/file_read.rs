//! file_read 工具：读取文件内容，受 policy 路径白名单约束。
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct FileReadTool;

#[async_trait]
impl BuiltinTool for FileReadTool {
    fn name(&self) -> &'static str {
        "file_read"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Read a UTF-8 text file from the current workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path or a path relative to the workspace."}
                    },
                    "required": ["path"]
                }
            }
        })
    }

    fn risk(&self, args: &Value) -> Risk {
        let path = args.get("path").and_then(|x| x.as_str()).unwrap_or("");
        const SENSITIVE: &[&str] = &[".ssh", "/etc", "id_rsa", ".env", ".aws", ".gnupg"];
        if SENSITIVE.iter().any(|p| path.contains(p)) {
            Risk::Medium
        } else {
            Risk::Low
        }
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let path_str = ctx
            .args
            .get("path")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `path` 参数".into()))?;

        let expanded_path = crate::tools::builtin::normalize_path(
            &crate::tools::builtin::resolve_path(ctx, path_str),
        );

        if let Err(e) = ctx.policy.check_file_read(&expanded_path.to_string_lossy()) {
            ctx.record_failure(&e).await?;
            return Err(AppError::Other(e));
        }

        match tokio::fs::read_to_string(&expanded_path).await {
            Ok(content) => {
                ctx.update_complete(
                    "done",
                    Some("success"),
                    Some(0),
                    Some(format!("已读取 {} 字节", content.len())),
                    None,
                )
                .await?;
                Ok(json!({ "content": content, "stdout": content }))
            }
            Err(e) => {
                let err_msg = format!("无法读取文件: {e}");
                ctx.record_failure(&err_msg).await?;
                Err(AppError::Other(err_msg))
            }
        }
    }
}
