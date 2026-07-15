//! Exact text replacement for UTF-8 files.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct FileEditTool;

#[async_trait]
impl BuiltinTool for FileEditTool {
    fn name(&self) -> &'static str {
        "file_edit"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Replace an exact string in a UTF-8 file. By default the old string must occur exactly once.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path or a path relative to the workspace."},
                        "old_string": {"type": "string", "description": "Exact text to replace."},
                        "new_string": {"type": "string", "description": "Replacement text."},
                        "replace_all": {"type": "boolean", "description": "Replace every occurrence instead of requiring a unique match."}
                    },
                    "required": ["path", "old_string", "new_string"]
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let path_str = required_string(ctx, "path").await?;
        let old_string = required_string(ctx, "old_string").await?;
        let new_string = required_string(ctx, "new_string").await?;
        if old_string.is_empty() {
            return fail(ctx, "`old_string` must not be empty").await;
        }
        let replace_all = ctx
            .args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let path = crate::tools::builtin::normalize_path(&crate::tools::builtin::resolve_path(
            ctx, &path_str,
        ));
        if let Err(error) = ctx.policy.check_file_write(&path.to_string_lossy()) {
            return fail(ctx, &error).await;
        }
        ctx.update_running(&path.to_string_lossy()).await?;

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) => return fail(ctx, &format!("Unable to read file: {error}")).await,
        };
        let occurrences = content.match_indices(&old_string).count();
        if occurrences == 0 {
            return fail(ctx, "`old_string` was not found in the file").await;
        }
        if !replace_all && occurrences != 1 {
            return fail(
                ctx,
                &format!(
                    "`old_string` occurs {occurrences} times; provide more context or set `replace_all`"
                ),
            )
            .await;
        }

        let updated = if replace_all {
            content.replace(&old_string, &new_string)
        } else {
            content.replacen(&old_string, &new_string, 1)
        };
        if let Err(error) = tokio::fs::write(&path, updated).await {
            return fail(ctx, &format!("Unable to write file: {error}")).await;
        }

        let replaced = if replace_all { occurrences } else { 1 };
        let summary = format!("Replaced {replaced} occurrence(s) in {}", path.display());
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(summary.clone()),
            None,
        )
        .await?;
        Ok(json!({"success": true, "replacements": replaced, "stdout": summary}))
    }
}

async fn required_string(ctx: &ToolCtx<'_>, key: &str) -> AppResult<String> {
    match ctx.args.get(key).and_then(Value::as_str) {
        Some(value) => Ok(value.to_string()),
        None => fail(ctx, &format!("Missing `{key}` argument")).await,
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}
