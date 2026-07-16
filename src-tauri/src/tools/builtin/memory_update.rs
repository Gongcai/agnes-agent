use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::db::repo::memory::MemoryUpdate;
use crate::error::{AppError, AppResult};
use crate::tools::builtin::memory_entry::{object_with_allowed_fields, visible_memory};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct MemoryUpdateTool;

#[async_trait]
impl BuiltinTool for MemoryUpdateTool {
    fn name(&self) -> &'static str {
        "memory_update"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Update selected fields of one structured long-term memory belonging to the current agent. The creator and creation time are preserved.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "memory_id": {"type": "string", "description": "Stable ID returned by memory_search or memory_create."},
                        "name": {"type": "string", "description": "Optional replacement memory name."},
                        "keywords": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Optional replacement keyword list. Use an empty array to clear it."
                        },
                        "content": {"type": "string", "description": "Optional replacement memory content."}
                    },
                    "required": ["memory_id"],
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
        let args = match object_with_allowed_fields(
            ctx.args,
            &["memory_id", "name", "keywords", "content"],
        ) {
            Ok(args) => args,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let memory_id = match non_empty_string(args, "memory_id", true) {
            Ok(Some(value)) => value,
            Ok(None) => unreachable!(),
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        if !["name", "keywords", "content"]
            .iter()
            .any(|field| args.contains_key(*field))
        {
            return fail(
                ctx,
                "At least one of `name`, `keywords`, or `content` is required",
            )
            .await;
        }
        let agent_id = match crate::memory::agent_id_for_session(ctx.db, ctx.session_id).await {
            Ok(agent_id) => agent_id,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        ctx.update_running(&format!("memory://{agent_id}/store/{memory_id}"))
            .await?;
        let current = match ctx.db.get_memory(memory_id.clone(), agent_id.clone()).await {
            Ok(Some(memory)) => memory,
            Ok(None) => return fail(ctx, "Memory was not found for this agent").await,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };

        let name = match non_empty_string(args, "name", false) {
            Ok(Some(value)) => value,
            Ok(None) => current.name.clone(),
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let content = match non_empty_string(args, "content", false) {
            Ok(Some(value)) => value,
            Ok(None) => current.content.clone(),
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let keywords = match replacement_keywords(args) {
            Ok(Some(values)) => values,
            Ok(None) => current.keywords.clone(),
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        if let Err(error) = ctx
            .db
            .update_memory(
                memory_id.clone(),
                agent_id.clone(),
                MemoryUpdate {
                    name,
                    keywords,
                    content,
                },
            )
            .await
        {
            return fail(ctx, &error.to_string()).await;
        }
        let updated = match ctx.db.get_memory(memory_id, agent_id).await {
            Ok(Some(memory)) => memory,
            Ok(None) => return fail(ctx, "Updated memory could not be read back").await,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Updated structured memory {}", updated.id)),
            None,
        )
        .await?;
        Ok(json!({"memory": visible_memory(&updated)}))
    }
}

fn non_empty_string(
    args: &Map<String, Value>,
    field: &str,
    required: bool,
) -> AppResult<Option<String>> {
    let Some(value) = args.get(field) else {
        return if required {
            Err(AppError::Other(format!("Missing `{field}` argument")))
        } else {
            Ok(None)
        };
    };
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(|value| Some(value.trim().to_string()))
        .ok_or_else(|| AppError::Other(format!("`{field}` must not be empty")))
}

fn replacement_keywords(args: &Map<String, Value>) -> AppResult<Option<Vec<String>>> {
    let Some(value) = args.get("keywords") else {
        return Ok(None);
    };
    let values = value
        .as_array()
        .ok_or_else(|| AppError::Other("`keywords` must be an array of strings".into()))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .ok_or_else(|| AppError::Other("`keywords` must contain only strings".into()))
        })
        .collect::<AppResult<Vec<_>>>()
        .map(Some)
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_omitted_and_empty_replacement_keywords() {
        assert_eq!(
            replacement_keywords(json!({}).as_object().unwrap()).unwrap(),
            None
        );
        assert_eq!(
            replacement_keywords(json!({"keywords": []}).as_object().unwrap()).unwrap(),
            Some(vec![])
        );
    }
}
