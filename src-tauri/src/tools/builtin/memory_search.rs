use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct MemorySearchTool;

#[async_trait]
impl BuiltinTool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Search the current agent's structured long-term memories by name, keywords, and content.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Text to match against memory names, keywords, and content."},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum results; defaults to 10."}
                    },
                    "required": ["query"]
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Low
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        if !ctx.policy.memory.enabled {
            return fail(ctx, "Memory tools are disabled for this agent").await;
        }
        let query = match ctx.args.get("query").and_then(Value::as_str) {
            Some(query) if !query.trim().is_empty() => query.trim(),
            _ => return fail(ctx, "`query` must not be empty").await,
        };
        let limit = ctx
            .args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(10)
            .clamp(1, 50) as usize;
        let agent_id = match crate::memory::agent_id_for_session(ctx.db, ctx.session_id).await {
            Ok(agent_id) => agent_id,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        ctx.update_running(&format!("memory://{agent_id}/store"))
            .await?;
        let memories = match ctx
            .db
            .search_memories(query.to_string(), agent_id, limit)
            .await
        {
            Ok(memories) => memories,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let summary = format!("Found {} structured memories", memories.len());
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(summary.clone()),
            None,
        )
        .await?;
        let visible_memories = memories
            .into_iter()
            .map(|memory| {
                json!({
                    "name": memory.name,
                    "keywords": memory.keywords,
                    "created_at": memory.created_at,
                    "content": memory.content,
                    "creator": memory.creator,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({"memories": visible_memories}))
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}
