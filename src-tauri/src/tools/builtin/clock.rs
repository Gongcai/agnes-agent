use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::AppResult;
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

/// Read-only clock exposed so the model can fetch the current local time on demand
/// instead of receiving a volatile timestamp baked into the (cache-stable) system prompt.
pub struct GetCurrentTimeTool;

#[async_trait]
impl BuiltinTool for GetCurrentTimeTool {
    fn name(&self) -> &'static str {
        "get_current_time"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Get the current local date and time. Call this whenever the request depends on the current date or time — \"today\", \"now\", relative dates, scheduling, greetings, or any time-sensitive reasoning — and use the returned instant instead of guessing. Returns an RFC 3339 / ISO 8601 instant with the local UTC offset.",
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
        let now = chrono::Local::now();
        let datetime = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let weekday = now.format("%A").to_string();
        let human = format!("{datetime} ({weekday})");
        ctx.update_complete("done", Some("success"), Some(0), Some(human.clone()), None)
            .await?;
        Ok(json!({
            "datetime": datetime,
            "weekday": weekday,
            "stdout": human,
        }))
    }
}
