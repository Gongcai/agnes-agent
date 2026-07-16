use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct MemoryMdViewTool;

#[async_trait]
impl BuiltinTool for MemoryMdViewTool {
    fn name(&self) -> &'static str {
        "memory_md_view"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Read the complete MEMORY.md for the current agent. Use this after editing when the final content must be verified.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
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
        let (agent_id, content) =
            match crate::memory::load_memory_md_for_session(ctx.db, ctx.session_id).await {
                Ok(result) => result,
                Err(error) => return fail(ctx, &error.to_string()).await,
            };
        ctx.update_running(&format!("memory://{agent_id}/MEMORY.md"))
            .await?;
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Read {} bytes from MEMORY.md", content.len())),
            None,
        )
        .await?;
        Ok(json!({"content": content, "stdout": content}))
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}
