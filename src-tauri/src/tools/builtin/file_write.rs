//! file_write 工具：写入文件，自动创建父目录，受 policy 路径白名单约束。
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct FileWriteTool;

#[async_trait]
impl BuiltinTool for FileWriteTool {
    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let path_str = ctx
            .args
            .get("path")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `path` 参数".into()))?;
        let content = ctx
            .args
            .get("content")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `content` 参数".into()))?;

        let expanded_path = crate::tools::policy::expand_home(path_str);

        if let Err(e) = ctx.policy.check_file_write(&expanded_path.to_string_lossy()) {
            ctx.record_failure(&e).await?;
            return Err(AppError::Other(e));
        }

        if let Some(parent) = expanded_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                let err_msg = format!("无法创建父目录: {e}");
                ctx.record_failure(&err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        }

        match tokio::fs::write(&expanded_path, content).await {
            Ok(_) => {
                ctx.update_complete(
                    "done",
                    Some("success"),
                    Some(0),
                    Some(format!("已写入 {} 字节", content.len())),
                    None,
                )
                .await?;
                Ok(json!({ "success": true }))
            }
            Err(e) => {
                let err_msg = format!("无法写入文件: {e}");
                ctx.record_failure(&err_msg).await?;
                Err(AppError::Other(err_msg))
            }
        }
    }
}
