//! Calendar and task tools backed by the local planner service.

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct CalendarListTool;
pub struct CalendarCreateTool;
pub struct TaskListTool;
pub struct TaskCreateTool;
pub struct TaskCompleteTool;

fn required(args: &Map<String, Value>, field: &str) -> AppResult<String> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| AppError::Other(format!("`{field}` must not be empty")))
}

fn optional(args: &Map<String, Value>, field: &str) -> AppResult<Option<String>> {
    match args.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            Ok((!value.trim().is_empty()).then(|| value.trim().to_string()))
        }
        _ => Err(AppError::Other(format!(
            "`{field}` must be a string or null"
        ))),
    }
}

async fn complete(ctx: &ToolCtx<'_>, summary: String) -> AppResult<()> {
    ctx.update_complete("done", Some("success"), Some(0), Some(summary), None)
        .await
}

async fn fail<T>(ctx: &ToolCtx<'_>, error: AppError) -> AppResult<T> {
    ctx.record_failure(&error.to_string()).await?;
    Err(error)
}

#[async_trait]
impl BuiltinTool for CalendarListTool {
    fn name(&self) -> &'static str {
        "calendar_list"
    }
    fn schema(&self) -> Value {
        json!({"type":"function","function":{"name":self.name(),"description":"List locally available calendars. Results include stable calendar IDs and timezones.","parameters":{"type":"object","properties":{},"additionalProperties":false}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        ctx.update_running("planner://calendars").await?;
        match ctx.db.list_calendars().await {
            Ok(calendars) => {
                complete(ctx, format!("Listed {} calendars", calendars.len())).await?;
                Ok(json!({"calendars":calendars}))
            }
            Err(error) => fail(ctx, error).await,
        }
    }
}

#[async_trait]
impl BuiltinTool for CalendarCreateTool {
    fn name(&self) -> &'static str {
        "calendar_create"
    }
    fn schema(&self) -> Value {
        json!({"type":"function","function":{"name":self.name(),"description":"Create a local calendar. This writes user data and always requires approval outside Full Access.","parameters":{"type":"object","properties":{"name":{"type":"string"},"timezone":{"type":"string","description":"IANA timezone, for example Asia/Shanghai."},"color":{"type":["string","null"]}},"required":["name","timezone"],"additionalProperties":false}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args = ctx
            .args
            .as_object()
            .ok_or_else(|| AppError::Other("Arguments must be an object".into()));
        let args = match args {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let name = match required(args, "name") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let timezone = match required(args, "timezone") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let color = match optional(args, "color") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let id = uuid::Uuid::new_v4().to_string();
        ctx.update_running("planner://calendars").await?;
        match ctx
            .db
            .create_calendar(id.clone(), name.clone(), color, timezone.clone())
            .await
        {
            Ok(()) => {
                complete(ctx, format!("Created calendar {id}")).await?;
                Ok(json!({"calendar":{"id":id,"name":name,"timezone":timezone}}))
            }
            Err(error) => fail(ctx, error).await,
        }
    }
}

#[async_trait]
impl BuiltinTool for TaskListTool {
    fn name(&self) -> &'static str {
        "task_list"
    }
    fn schema(&self) -> Value {
        json!({"type":"function","function":{"name":self.name(),"description":"List local task lists, or tasks in one list when task_list_id is provided.","parameters":{"type":"object","properties":{"task_list_id":{"type":"string"}},"additionalProperties":false}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let task_list_id = match ctx.args.get("task_list_id") {
            None => None,
            Some(Value::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            _ => {
                return fail(
                    ctx,
                    AppError::Other("`task_list_id` must be a non-empty string".into()),
                )
                .await
            }
        };
        ctx.update_running("planner://tasks").await?;
        match task_list_id {
            Some(id) => match ctx.db.list_tasks(id.clone()).await {
                Ok(tasks) => {
                    complete(ctx, format!("Listed {} tasks", tasks.len())).await?;
                    Ok(json!({"task_list_id":id,"tasks":tasks}))
                }
                Err(error) => fail(ctx, error).await,
            },
            None => match ctx.db.list_task_lists().await {
                Ok(task_lists) => {
                    complete(ctx, format!("Listed {} task lists", task_lists.len())).await?;
                    Ok(json!({"task_lists":task_lists}))
                }
                Err(error) => fail(ctx, error).await,
            },
        }
    }
}

#[async_trait]
impl BuiltinTool for TaskCreateTool {
    fn name(&self) -> &'static str {
        "task_create"
    }
    fn schema(&self) -> Value {
        json!({"type":"function","function":{"name":self.name(),"description":"Create a local task in an existing task list. This write always requires approval outside Full Access.","parameters":{"type":"object","properties":{"task_list_id":{"type":"string"},"title":{"type":"string"},"description":{"type":["string","null"]},"due_at":{"type":["string","null"],"description":"Optional ISO 8601 instant."},"priority":{"type":"integer","minimum":0,"maximum":4}},"required":["task_list_id","title"],"additionalProperties":false}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args = match ctx.args.as_object() {
            Some(value) => value,
            None => return fail(ctx, AppError::Other("Arguments must be an object".into())).await,
        };
        let task_list_id = match required(args, "task_list_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let title = match required(args, "title") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let description = match optional(args, "description") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let due_at = match optional(args, "due_at") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let priority = args.get("priority").and_then(Value::as_i64).unwrap_or(0);
        let id = uuid::Uuid::new_v4().to_string();
        ctx.update_running("planner://tasks").await?;
        match ctx
            .db
            .create_task(
                id.clone(),
                task_list_id.clone(),
                None,
                title.clone(),
                description,
                priority,
                due_at,
                0.0,
            )
            .await
        {
            Ok(()) => {
                complete(ctx, format!("Created task {id}")).await?;
                Ok(json!({"task":{"id":id,"task_list_id":task_list_id,"title":title}}))
            }
            Err(error) => fail(ctx, error).await,
        }
    }
}

#[async_trait]
impl BuiltinTool for TaskCompleteTool {
    fn name(&self) -> &'static str {
        "task_complete"
    }
    fn schema(&self) -> Value {
        json!({"type":"function","function":{"name":self.name(),"description":"Mark a local task completed or reopen it. This write always requires approval outside Full Access.","parameters":{"type":"object","properties":{"task_id":{"type":"string"},"completed":{"type":"boolean"}},"required":["task_id","completed"],"additionalProperties":false}}})
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args = match ctx.args.as_object() {
            Some(value) => value,
            None => return fail(ctx, AppError::Other("Arguments must be an object".into())).await,
        };
        let task_id = match required(args, "task_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let completed = match args.get("completed").and_then(Value::as_bool) {
            Some(value) => value,
            None => {
                return fail(ctx, AppError::Other("`completed` must be a boolean".into())).await
            }
        };
        ctx.update_running("planner://tasks").await?;
        match ctx.db.complete_task(task_id.clone(), completed).await {
            Ok(()) => {
                complete(ctx, format!("Updated task {task_id}")).await?;
                Ok(json!({"task_id":task_id,"completed":completed}))
            }
            Err(error) => fail(ctx, error).await,
        }
    }
}
