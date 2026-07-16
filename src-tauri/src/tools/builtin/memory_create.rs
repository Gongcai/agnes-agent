use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::db::repo::memory::NewMemory;
use crate::error::{AppError, AppResult};
use crate::tools::builtin::memory_entry::{object_with_allowed_fields, visible_memory};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct MemoryCreateTool;

#[async_trait]
impl BuiltinTool for MemoryCreateTool {
    fn name(&self) -> &'static str {
        "memory_create"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Create one structured long-term memory for the current agent. The system assigns its ID, agent, creator, and timestamps.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Short, recognizable memory name."},
                        "keywords": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Optional keywords used for simple string matching."
                        },
                        "content": {"type": "string", "description": "Complete memory content."}
                    },
                    "required": ["name", "content"],
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
        let args = match object_with_allowed_fields(ctx.args, &["name", "keywords", "content"]) {
            Ok(args) => args,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let name = match required_string(args, "name") {
            Ok(value) => value,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let content = match required_string(args, "content") {
            Ok(value) => value,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let keywords = match optional_keywords(args) {
            Ok(value) => value,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let agent_id = match crate::memory::agent_id_for_session(ctx.db, ctx.session_id).await {
            Ok(agent_id) => agent_id,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        ctx.update_running(&format!("memory://{agent_id}/store"))
            .await?;

        let memory_id = uuid::Uuid::new_v4().to_string();
        let inserted = match ctx
            .db
            .insert_memory(NewMemory {
                id: memory_id.clone(),
                agent_id: agent_id.clone(),
                name,
                keywords,
                content,
                creator: "ai".into(),
                memory_type: "Note".into(),
                scope: "agent".into(),
                source: "memory_create".into(),
                confidence: 1.0,
                embedding_id: None,
            })
            .await
        {
            Ok(inserted) => inserted,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        if !inserted {
            return fail(
                ctx,
                "An active memory with the same name and content already exists",
            )
            .await;
        }
        let memory = match ctx.db.get_memory(memory_id, agent_id).await {
            Ok(Some(memory)) => memory,
            Ok(None) => return fail(ctx, "Created memory could not be read back").await,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Created structured memory {}", memory.id)),
            None,
        )
        .await?;
        Ok(json!({"memory": visible_memory(&memory)}))
    }
}

pub(super) fn required_string(args: &Map<String, Value>, field: &str) -> AppResult<String> {
    args.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .ok_or_else(|| AppError::Other(format!("`{field}` must not be empty")))
}

pub(super) fn optional_keywords(args: &Map<String, Value>) -> AppResult<Vec<String>> {
    let Some(value) = args.get("keywords") else {
        return Ok(Vec::new());
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
        .collect()
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_system_fields_and_invalid_keywords() {
        let with_creator = json!({"name": "Test", "content": "Content", "creator": "user"});
        assert!(
            object_with_allowed_fields(&with_creator, &["name", "keywords", "content"])
                .unwrap_err()
                .to_string()
                .contains("creator")
        );

        let invalid_keywords = json!({"keywords": ["valid", 1]});
        assert!(optional_keywords(invalid_keywords.as_object().unwrap()).is_err());
    }
}
