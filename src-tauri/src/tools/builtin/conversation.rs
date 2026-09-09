use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

const MAX_QUERY_CHARS: usize = 200;
const MAX_SEARCH_RESULTS: usize = 20;
const MAX_CONTEXT_MESSAGES: usize = 30;
const MAX_CONTEXT_SIDE: usize = 10;

pub struct ConversationSearchTool;
pub struct ConversationReadTool;

fn required_string(args: &Map<String, Value>, field: &str) -> AppResult<String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| AppError::Other(format!("{field} must be a non-empty string")))
}

fn optional_string(args: &Map<String, Value>, field: &str) -> AppResult<Option<String>> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            Ok((!value.is_empty()).then(|| value.to_string()))
        }
        _ => Err(AppError::Other(format!("{field} must be a string or null"))),
    }
}

fn bounded_integer(
    args: &Map<String, Value>,
    field: &str,
    default: usize,
    minimum: usize,
    maximum: usize,
) -> AppResult<usize> {
    let Some(value) = args.get(field) else {
        return Ok(default);
    };
    let value = value
        .as_u64()
        .ok_or_else(|| AppError::Other(format!("{field} must be an integer")))?;
    let value =
        usize::try_from(value).map_err(|_| AppError::Other(format!("{field} is too large")))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(AppError::Other(format!(
            "{field} must be between {minimum} and {maximum}"
        )));
    }
    Ok(value)
}

async fn agent_id(ctx: &ToolCtx<'_>) -> AppResult<String> {
    crate::memory::agent_id_for_session(ctx.db, ctx.session_id).await
}

async fn validate_session(ctx: &ToolCtx<'_>, agent_id: &str, session_id: &str) -> AppResult<()> {
    let session = ctx
        .db
        .get_session(session_id.to_string())
        .await?
        .ok_or_else(|| AppError::Other("Conversation session was not found".into()))?;
    if session.agent_id != agent_id {
        return Err(AppError::Other(
            "Conversation history is limited to the current agent".into(),
        ));
    }
    Ok(())
}

async fn begin(ctx: &ToolCtx<'_>, location: &str) -> AppResult<()> {
    if let Err(error) = ctx.policy.check_conversation() {
        return fail(ctx, error).await;
    }
    ctx.update_running(location).await
}

async fn finish(ctx: &ToolCtx<'_>, summary: String) -> AppResult<()> {
    ctx.update_complete("done", Some("success"), Some(0), Some(summary), None)
        .await
}

async fn fail<T>(ctx: &ToolCtx<'_>, error: impl Into<String>) -> AppResult<T> {
    let error = error.into();
    ctx.record_failure(&error).await?;
    Err(AppError::Other(error))
}

#[async_trait]
impl BuiltinTool for ConversationSearchTool {
    fn name(&self) -> &'static str {
        "conversation_search"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Search completed visible text in past conversations belonging to the current agent. Use the returned session_id and message_id with conversation_read to retrieve surrounding messages. Reasoning, tool calls, tool results, deleted messages, and other agents' sessions are never returned.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 200,
                            "description": "A keyword or exact phrase to find in visible user/assistant messages."
                        },
                        "session_id": {
                            "type": ["string", "null"],
                            "description": "Optional session ID from a previous result to limit the search."
                        },
                        "role": {
                            "type": "string",
                            "enum": ["any", "user", "assistant"],
                            "description": "Optional message role filter; defaults to any."
                        },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 20,
                            "description": "Maximum number of matching messages; defaults to 8."
                        }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Low
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args = match ctx.args.as_object() {
            Some(args) => args,
            None => return fail(ctx, "Arguments must be an object").await,
        };
        let query = match required_string(args, "query") {
            Ok(query) if query.chars().count() <= MAX_QUERY_CHARS && !query.contains('\0') => query,
            Ok(_) => {
                return fail(
                    ctx,
                    format!("query must contain between 1 and {MAX_QUERY_CHARS} characters"),
                )
                .await
            }
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let session_id = match optional_string(args, "session_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let role = match optional_string(args, "role") {
            Ok(None) => None,
            Ok(Some(value)) if value == "any" => None,
            Ok(Some(value)) if matches!(value.as_str(), "user" | "assistant") => Some(value),
            Ok(Some(_)) => return fail(ctx, "role must be any, user, or assistant").await,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let limit = match bounded_integer(args, "limit", 8, 1, MAX_SEARCH_RESULTS) {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let agent_id = match agent_id(ctx).await {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        if let Some(session_id) = session_id.as_deref() {
            if let Err(error) = validate_session(ctx, &agent_id, session_id).await {
                return fail(ctx, error.to_string()).await;
            }
        }
        begin(ctx, "conversation://search").await?;

        let results = match ctx
            .db
            .search_conversations(agent_id, session_id.clone(), query.clone(), role, limit)
            .await
        {
            Ok(results) => results,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        finish(ctx, format!("Found {} matching message(s)", results.len())).await?;
        Ok(json!({
            "query": query,
            "session_id": session_id,
            "results": results,
        }))
    }
}

#[async_trait]
impl BuiltinTool for ConversationReadTool {
    fn name(&self) -> &'static str {
        "conversation_read"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Read visible user/assistant text from one historical conversation. Pass a session_id from conversation_search and optionally a message_id to read that match with nearby context. Without message_id, returns the latest bounded messages in the session. Reasoning and tool activity are excluded.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session ID returned by conversation_search."
                        },
                        "message_id": {
                            "type": ["string", "null"],
                            "description": "Optional message ID returned by conversation_search."
                        },
                        "before": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 10,
                            "description": "Messages before message_id to include; defaults to 2."
                        },
                        "after": {
                            "type": "integer",
                            "minimum": 0,
                            "maximum": 10,
                            "description": "Messages after message_id to include; defaults to 2."
                        },
                        "max_messages": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 30,
                            "description": "Hard cap on returned messages; defaults to 12."
                        }
                    },
                    "required": ["session_id"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Low
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args = match ctx.args.as_object() {
            Some(args) => args,
            None => return fail(ctx, "Arguments must be an object").await,
        };
        let session_id = match required_string(args, "session_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let message_id = match optional_string(args, "message_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let before = match bounded_integer(args, "before", 2, 0, MAX_CONTEXT_SIDE) {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let after = match bounded_integer(args, "after", 2, 0, MAX_CONTEXT_SIDE) {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let max_messages = match bounded_integer(args, "max_messages", 12, 1, MAX_CONTEXT_MESSAGES)
        {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        let agent_id = match agent_id(ctx).await {
            Ok(value) => value,
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        if let Err(error) = validate_session(ctx, &agent_id, &session_id).await {
            return fail(ctx, error.to_string()).await;
        }
        begin(ctx, "conversation://read").await?;

        let result = match ctx
            .db
            .read_conversation(
                agent_id,
                session_id,
                message_id,
                before,
                after,
                max_messages,
            )
            .await
        {
            Ok(Some(result)) => result,
            Ok(None) => {
                return fail(
                    ctx,
                    "The requested conversation or message is not available",
                )
                .await
            }
            Err(error) => return fail(ctx, error.to_string()).await,
        };
        finish(
            ctx,
            format!("Read {} visible message(s)", result.messages.len()),
        )
        .await?;
        Ok(serde_json::to_value(result)?)
    }
}
