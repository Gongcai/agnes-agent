use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct MemoryMdEditTool;

#[async_trait]
impl BuiltinTool for MemoryMdEditTool {
    fn name(&self) -> &'static str {
        "memory_md_edit"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Update only the current agent's MEMORY.md. Append new Markdown or replace one uniquely matching exact block. Call memory_md_view to verify the final document when needed.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["append", "replace"]},
                        "content": {"type": "string", "description": "Non-empty Markdown to append when action is append."},
                        "old_text": {"type": "string", "description": "Unique exact text to replace when action is replace."},
                        "new_text": {"type": "string", "description": "Replacement text when action is replace."}
                    },
                    "required": ["action"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        if !ctx.policy.memory.enabled {
            return fail(ctx, "Memory tools are disabled for this agent").await;
        }
        let (agent_id, current) =
            match crate::memory::load_memory_md_for_session(ctx.db, ctx.session_id).await {
                Ok(result) => result,
                Err(error) => return fail(ctx, &error.to_string()).await,
            };
        ctx.update_running(&format!("memory://{agent_id}/MEMORY.md"))
            .await?;

        let (updated, summary) = match apply_edit(&current, ctx.args) {
            Ok(result) => result,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };

        if let Err(error) = crate::memory::save_memory_md(ctx.db, &agent_id, &updated).await {
            return fail(ctx, &error.to_string()).await;
        }
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(summary.clone()),
            None,
        )
        .await?;
        Ok(json!({"success": true, "stdout": summary}))
    }
}

fn apply_edit(current: &str, args: &Value) -> AppResult<(String, String)> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Other("Missing `action` argument".into()))?;
    match action {
        "append" => {
            let addition = args
                .get("content")
                .and_then(Value::as_str)
                .filter(|content| !content.trim().is_empty())
                .map(str::trim)
                .ok_or_else(|| AppError::Other("`content` must not be empty for append".into()))?;
            let updated = if current.trim().is_empty() {
                addition.to_string()
            } else {
                format!("{}\n\n{}", current.trim_end(), addition)
            };
            Ok((
                updated,
                format!("Appended {} bytes to MEMORY.md", addition.len()),
            ))
        }
        "replace" => {
            let old_text = args
                .get("old_text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .ok_or_else(|| {
                    AppError::Other("`old_text` must not be empty for replace".into())
                })?;
            let new_text = args
                .get("new_text")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Other("Missing `new_text` for replace".into()))?;
            let occurrences = current.match_indices(old_text).count();
            if occurrences == 0 {
                return Err(AppError::Other(
                    "`old_text` was not found in MEMORY.md".into(),
                ));
            }
            if occurrences != 1 {
                return Err(AppError::Other(format!(
                    "`old_text` occurs {occurrences} times; provide a unique block"
                )));
            }
            Ok((
                current.replacen(old_text, new_text, 1),
                "Replaced one exact block in MEMORY.md".to_string(),
            ))
        }
        _ => Err(AppError::Other("`action` must be append or replace".into())),
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_with_stable_spacing() {
        let (updated, _) = apply_edit(
            "# Facts\n\nExisting",
            &json!({
                "action": "append",
                "content": "  New fact  "
            }),
        )
        .unwrap();
        assert_eq!(updated, "# Facts\n\nExisting\n\nNew fact");
    }

    #[test]
    fn exact_replace_requires_one_match() {
        let (updated, _) = apply_edit(
            "Alpha\nBeta",
            &json!({
                "action": "replace",
                "old_text": "Beta",
                "new_text": "Gamma"
            }),
        )
        .unwrap();
        assert_eq!(updated, "Alpha\nGamma");

        let duplicate = apply_edit(
            "Same\nSame",
            &json!({
                "action": "replace",
                "old_text": "Same",
                "new_text": "Different"
            }),
        );
        assert!(duplicate.is_err());
    }
}
