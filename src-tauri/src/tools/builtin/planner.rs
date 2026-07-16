//! Calendar and task tools backed by the local planner service.

use async_trait::async_trait;
use serde_json::{json, Map, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct CalendarListTool;
pub struct CalendarCreateTool;
pub struct CalendarEventCreateTool;
pub struct CalendarUpdateTool;
pub struct TaskListTool;
pub struct TaskCreateTool;
pub struct TaskCompleteTool;
pub struct TaskUpdateTool;

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

fn optional_update(args: &Map<String, Value>, field: &str) -> AppResult<Option<Option<String>>> {
    match args.get(field) {
        None => Ok(None),
        Some(_) => optional(args, field).map(Some),
    }
}

fn optional_required(args: &Map<String, Value>, field: &str) -> AppResult<Option<String>> {
    match args.get(field) {
        None => Ok(None),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                Err(AppError::Other(format!("`{field}` must not be empty")))
            } else {
                Ok(Some(value.to_string()))
            }
        }
        _ => Err(AppError::Other(format!("`{field}` must be a string"))),
    }
}

fn optional_bool(args: &Map<String, Value>, field: &str) -> AppResult<Option<bool>> {
    match args.get(field) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(AppError::Other(format!("`{field}` must be a boolean"))),
    }
}

fn optional_priority(args: &Map<String, Value>) -> AppResult<Option<i64>> {
    match args.get("priority") {
        None => Ok(None),
        Some(value) => {
            let value = value.as_i64().ok_or_else(|| {
                AppError::Other("`priority` must be an integer between 0 and 4".into())
            })?;
            if !(0..=4).contains(&value) {
                return Err(AppError::Other(
                    "`priority` must be an integer between 0 and 4".into(),
                ));
            }
            Ok(Some(value))
        }
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

async fn require_planner(ctx: &ToolCtx<'_>) -> AppResult<()> {
    if let Err(error) = ctx.policy.check_planner() {
        return fail(ctx, AppError::Other(error)).await;
    }
    Ok(())
}

#[async_trait]
impl BuiltinTool for CalendarListTool {
    fn name(&self) -> &'static str {
        "calendar_list"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "List local calendars, or events in one calendar when calendar_id and an ISO 8601 range are provided.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "calendar_id": {"type": "string"},
                        "range_start": {"type": "string", "description": "ISO 8601 range start, required with calendar_id."},
                        "range_end": {"type": "string", "description": "ISO 8601 range end, required with calendar_id."}
                    },
                    "additionalProperties": false
                }
            }
        })
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        require_planner(ctx).await?;
        let args = match ctx.args.as_object() {
            Some(value) => value,
            None => return fail(ctx, AppError::Other("Arguments must be an object".into())).await,
        };
        let calendar_id = match args.get("calendar_id") {
            None => None,
            Some(Value::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            _ => {
                return fail(
                    ctx,
                    AppError::Other("`calendar_id` must be a non-empty string".into()),
                )
                .await
            }
        };
        ctx.update_running("planner://calendars").await?;
        match calendar_id {
            Some(calendar_id) => {
                let range_start = match required(args, "range_start") {
                    Ok(value) => value,
                    Err(error) => return fail(ctx, error).await,
                };
                let range_end = match required(args, "range_end") {
                    Ok(value) => value,
                    Err(error) => return fail(ctx, error).await,
                };
                match ctx
                    .db
                    .list_calendar_events(calendar_id.clone(), range_start, range_end)
                    .await
                {
                    Ok(events) => {
                        complete(ctx, format!("Listed {} events", events.len())).await?;
                        Ok(json!({"calendar_id": calendar_id, "events": events}))
                    }
                    Err(error) => fail(ctx, error).await,
                }
            }
            None => match ctx.db.list_calendars().await {
                Ok(calendars) => {
                    complete(ctx, format!("Listed {} calendars", calendars.len())).await?;
                    Ok(json!({"calendars":calendars}))
                }
                Err(error) => fail(ctx, error).await,
            },
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
        require_planner(ctx).await?;
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
impl BuiltinTool for CalendarEventCreateTool {
    fn name(&self) -> &'static str {
        "calendar_event_create"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Create an event in an existing local calendar. This writes user data and always requires approval outside Full Access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "calendar_id": {"type": "string"},
                        "title": {"type": "string"},
                        "starts_at": {"type": "string", "description": "ISO 8601 instant."},
                        "ends_at": {"type": "string", "description": "ISO 8601 instant after starts_at."},
                        "timezone": {"type": "string", "description": "IANA timezone, for example Asia/Shanghai."},
                        "all_day": {"type": "boolean"},
                        "recurrence_rule": {"type": ["string", "null"], "description": "Optional RRULE: prefixed recurrence rule."}
                    },
                    "required": ["calendar_id", "title", "starts_at", "ends_at", "timezone"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        require_planner(ctx).await?;
        let args = match ctx.args.as_object() {
            Some(value) => value,
            None => return fail(ctx, AppError::Other("Arguments must be an object".into())).await,
        };
        let calendar_id = match required(args, "calendar_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let title = match required(args, "title") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let starts_at = match required(args, "starts_at") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let ends_at = match required(args, "ends_at") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let timezone = match required(args, "timezone") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let all_day = match optional_bool(args, "all_day") {
            Ok(value) => value.unwrap_or(false),
            Err(error) => return fail(ctx, error).await,
        };
        let recurrence_rule = match optional(args, "recurrence_rule") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let id = uuid::Uuid::new_v4().to_string();
        ctx.update_running("planner://calendar-events").await?;
        match ctx
            .db
            .create_calendar_event(
                id.clone(),
                calendar_id.clone(),
                title.clone(),
                starts_at.clone(),
                ends_at.clone(),
                timezone.clone(),
                all_day,
                recurrence_rule.clone(),
            )
            .await
        {
            Ok(()) => {
                complete(ctx, format!("Created calendar event {id}")).await?;
                Ok(json!({
                    "event": {
                        "id": id,
                        "calendar_id": calendar_id,
                        "title": title,
                        "starts_at": starts_at,
                        "ends_at": ends_at,
                        "timezone": timezone,
                        "all_day": all_day,
                        "recurrence_rule": recurrence_rule,
                    }
                }))
            }
            Err(error) => fail(ctx, error).await,
        }
    }
}

#[async_trait]
impl BuiltinTool for CalendarUpdateTool {
    fn name(&self) -> &'static str {
        "calendar_update"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Update an existing local calendar event. Include only the fields that should change. This write always requires approval outside Full Access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_id": {"type": "string"},
                        "title": {"type": "string"},
                        "starts_at": {"type": "string", "description": "ISO 8601 instant."},
                        "ends_at": {"type": "string", "description": "ISO 8601 instant after starts_at."},
                        "timezone": {"type": "string", "description": "IANA timezone."},
                        "all_day": {"type": "boolean"},
                        "recurrence_rule": {"type": ["string", "null"], "description": "Use null to clear recurrence."}
                    },
                    "required": ["event_id"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        require_planner(ctx).await?;
        let args = match ctx.args.as_object() {
            Some(value) => value,
            None => return fail(ctx, AppError::Other("Arguments must be an object".into())).await,
        };
        let event_id = match required(args, "event_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let changes = crate::db::repo::planner::EventUpdate {
            title: match optional_required(args, "title") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            starts_at: match optional_required(args, "starts_at") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            ends_at: match optional_required(args, "ends_at") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            timezone: match optional_required(args, "timezone") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            all_day: match optional_bool(args, "all_day") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            recurrence_rule: match optional_update(args, "recurrence_rule") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
        };
        ctx.update_running("planner://calendar-events").await?;
        match ctx.db.update_calendar_event(event_id, changes).await {
            Ok(event) => {
                complete(ctx, format!("Updated calendar event {}", event.id)).await?;
                Ok(json!({"event": event}))
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
        require_planner(ctx).await?;
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
        require_planner(ctx).await?;
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
        require_planner(ctx).await?;
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

#[async_trait]
impl BuiltinTool for TaskUpdateTool {
    fn name(&self) -> &'static str {
        "task_update"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Update an existing local task. Include only the fields that should change. This write always requires approval outside Full Access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string"},
                        "title": {"type": "string"},
                        "description": {"type": ["string", "null"], "description": "Use null to clear the description."},
                        "priority": {"type": "integer", "minimum": 0, "maximum": 4},
                        "due_at": {"type": ["string", "null"], "description": "ISO 8601 instant, or null to clear the due time."}
                    },
                    "required": ["task_id"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        require_planner(ctx).await?;
        let args = match ctx.args.as_object() {
            Some(value) => value,
            None => return fail(ctx, AppError::Other("Arguments must be an object".into())).await,
        };
        let task_id = match required(args, "task_id") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let changes = crate::db::repo::planner::TaskUpdate {
            title: match optional_required(args, "title") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            description: match optional_update(args, "description") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            priority: match optional_priority(args) {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            due_at: match optional_update(args, "due_at") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
        };
        ctx.update_running("planner://tasks").await?;
        match ctx.db.update_task(task_id, changes).await {
            Ok(task) => {
                complete(ctx, format!("Updated task {}", task.id)).await?;
                Ok(json!({"task": task}))
            }
            Err(error) => fail(ctx, error).await,
        }
    }
}
