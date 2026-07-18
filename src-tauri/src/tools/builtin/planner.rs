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
                "description": "List local calendars first to obtain calendar IDs. Then call again with calendar_id plus range_start and range_end to read event occurrences; do not conclude that a calendar has no events from the first call alone. Range values must be RFC 3339 / ISO 8601 instants including Z or a numeric UTC offset, for example 2026-07-18T00:00:00+08:00. In each recurring result, id is the event_id used for updates; occurrence_id and original_occurrence identify that occurrence.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "calendar_id": {"type": "string", "description": "Calendar ID returned by calendar_list."},
                        "range_start": {"type": "string", "description": "Required with calendar_id. RFC 3339 / ISO 8601 range start with Z or an explicit offset, for example 2026-07-18T00:00:00+08:00; never omit the timezone."},
                        "range_end": {"type": "string", "description": "Required with calendar_id. RFC 3339 / ISO 8601 exclusive range end with Z or an explicit offset, for example 2026-07-19T00:00:00+08:00; never omit the timezone."}
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
                "description": "Create an event in an existing local calendar. starts_at and ends_at must be RFC 3339 / ISO 8601 instants with Z or an explicit UTC offset, never a timezone-less datetime. For an all-day event, use local midnight for starts_at and the next local midnight for ends_at in the supplied timezone. This writes user data and always requires approval outside Full Access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "calendar_id": {"type": "string", "description": "Calendar ID returned by calendar_list."},
                        "title": {"type": "string", "description": "Event title."},
                        "starts_at": {"type": "string", "description": "RFC 3339 / ISO 8601 start instant with timezone: use Z or an explicit UTC offset, for example 2026-07-18T09:00:00+08:00."},
                        "ends_at": {"type": "string", "description": "RFC 3339 / ISO 8601 end instant after starts_at with timezone: use Z or an explicit UTC offset, for example 2026-07-18T10:00:00+08:00."},
                        "timezone": {"type": "string", "description": "IANA timezone used for wall-clock and recurrence semantics, for example Asia/Shanghai."},
                        "all_day": {"type": "boolean", "description": "True for an all-day range whose instants are local midnights."},
                        "recurrence_rule": {"type": ["string", "null"], "description": "Optional RFC 5545 rule prefixed with RRULE:, for example RRULE:FREQ=WEEKLY."}
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
                "description": "Update a recurring series or one occurrence. starts_at and ends_at, when supplied, must be RFC 3339 / ISO 8601 instants with Z or an explicit UTC offset. Omit original_occurrence to update the series; include it to update one occurrence. With original_occurrence, cancelled=true cancels that occurrence and cancelled=false restores the original occurrence. Do not combine cancelled with edit fields. This write always requires approval outside Full Access.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "event_id": {"type": "string", "description": "Stable event ID returned by calendar_list."},
                        "title": {"type": "string", "description": "Replacement title."},
                        "starts_at": {"type": "string", "description": "RFC 3339 / ISO 8601 start instant with timezone: use Z or an explicit UTC offset, for example 2026-07-18T09:00:00+08:00."},
                        "ends_at": {"type": "string", "description": "RFC 3339 / ISO 8601 end instant after starts_at with timezone: use Z or an explicit UTC offset."},
                        "timezone": {"type": "string", "description": "IANA timezone, for example Asia/Shanghai."},
                        "all_day": {"type": "boolean", "description": "Whether the resulting event is all-day."},
                        "recurrence_rule": {"type": ["string", "null"], "description": "RFC 5545 rule prefixed with RRULE:, or null to clear series recurrence."},
                        "original_occurrence": {"type": "string", "description": "Exact ISO 8601 instant returned by calendar_list. When present, edit only that occurrence."},
                        "cancelled": {"type": "boolean", "description": "With original_occurrence only: true cancels and false restores the occurrence."}
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
        let title = match optional_required(args, "title") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let starts_at = match optional_required(args, "starts_at") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let ends_at = match optional_required(args, "ends_at") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let timezone = match optional_required(args, "timezone") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let all_day = match optional_bool(args, "all_day") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let recurrence_rule = match optional_update(args, "recurrence_rule") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let original_occurrence = match optional_required(args, "original_occurrence") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let cancelled = match optional_bool(args, "cancelled") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_running("planner://calendar-events").await?;
        match original_occurrence {
            Some(original_occurrence) => {
                if recurrence_rule.is_some() {
                    return fail(
                        ctx,
                        AppError::Other(
                            "`recurrence_rule` can only be changed on the recurring series".into(),
                        ),
                    )
                    .await;
                }
                let occurrence_changes = crate::db::repo::planner::OccurrenceUpdate {
                    title,
                    starts_at,
                    ends_at,
                    timezone,
                    all_day,
                };
                if let Some(cancelled) = cancelled {
                    if !occurrence_changes.is_empty() {
                        return fail(
                            ctx,
                            AppError::Other(
                                "`cancelled` cannot be combined with occurrence edit fields".into(),
                            ),
                        )
                        .await;
                    }
                    let result = if cancelled {
                        ctx.db
                            .cancel_calendar_occurrence(
                                event_id.clone(),
                                original_occurrence.clone(),
                            )
                            .await
                    } else {
                        ctx.db
                            .restore_calendar_occurrence(
                                event_id.clone(),
                                original_occurrence.clone(),
                            )
                            .await
                    };
                    match result {
                        Ok(()) => {
                            complete(
                                ctx,
                                format!(
                                    "{} calendar occurrence {}",
                                    if cancelled { "Cancelled" } else { "Restored" },
                                    original_occurrence
                                ),
                            )
                            .await?;
                            Ok(json!({
                                "event_id": event_id,
                                "original_occurrence": original_occurrence,
                                "cancelled": cancelled,
                            }))
                        }
                        Err(error) => fail(ctx, error).await,
                    }
                } else {
                    match ctx
                        .db
                        .update_calendar_occurrence(
                            uuid::Uuid::new_v4().to_string(),
                            event_id,
                            original_occurrence,
                            occurrence_changes,
                        )
                        .await
                    {
                        Ok(event) => {
                            complete(
                                ctx,
                                format!("Updated calendar occurrence {}", event.occurrence_id),
                            )
                            .await?;
                            Ok(json!({"event": event}))
                        }
                        Err(error) => fail(ctx, error).await,
                    }
                }
            }
            None => {
                if cancelled.is_some() {
                    return fail(
                        ctx,
                        AppError::Other("`cancelled` requires `original_occurrence`".into()),
                    )
                    .await;
                }
                let changes = crate::db::repo::planner::EventUpdate {
                    title,
                    starts_at,
                    ends_at,
                    timezone,
                    all_day,
                    recurrence_rule,
                };
                match ctx.db.update_calendar_event(event_id, changes).await {
                    Ok(event) => {
                        complete(ctx, format!("Updated calendar event {}", event.id)).await?;
                        Ok(json!({"event": event}))
                    }
                    Err(error) => fail(ctx, error).await,
                }
            }
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
        json!({"type":"function","function":{"name":self.name(),"description":"Create a local task in an existing task list. Use due_date for a local YYYY-MM-DD deadline or due_at for an exact ISO 8601 instant, never both. This write always requires approval outside Full Access.","parameters":{"type":"object","properties":{"task_list_id":{"type":"string"},"title":{"type":"string"},"description":{"type":["string","null"]},"due_date":{"type":["string","null"]},"due_at":{"type":["string","null"]},"due_timezone":{"type":["string","null"]},"is_important":{"type":"boolean"},"my_day_date":{"type":["string","null"]},"recurrence_rule":{"type":["string","null"]},"priority":{"type":"integer","minimum":0,"maximum":4}},"required":["task_list_id","title"],"additionalProperties":false}}})
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
        let due_date = match optional(args, "due_date") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let due_timezone = match optional(args, "due_timezone") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let my_day_date = match optional(args, "my_day_date") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let recurrence_rule = match optional(args, "recurrence_rule") {
            Ok(value) => value,
            Err(error) => return fail(ctx, error).await,
        };
        let is_important = match optional_bool(args, "is_important") {
            Ok(value) => value.unwrap_or(false),
            Err(error) => return fail(ctx, error).await,
        };
        let priority = args.get("priority").and_then(Value::as_i64).unwrap_or(0);
        let id = uuid::Uuid::new_v4().to_string();
        ctx.update_running("planner://tasks").await?;
        match ctx
            .db
            .create_task(crate::db::repo::planner::NewTask {
                id: id.clone(),
                task_list_id: task_list_id.clone(),
                parent_id: None,
                title: title.clone(),
                description,
                priority,
                due_date,
                due_at,
                due_timezone,
                is_important,
                my_day_date,
                recurrence_rule,
                sort_order: 0.0,
            })
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
                        "due_date": {"type": ["string", "null"], "description": "Local YYYY-MM-DD deadline, or null to clear it."},
                        "due_at": {"type": ["string", "null"], "description": "ISO 8601 instant, or null to clear it."},
                        "due_timezone": {"type": ["string", "null"], "description": "IANA timezone for the deadline."},
                        "is_important": {"type": "boolean"},
                        "my_day_date": {"type": ["string", "null"], "description": "YYYY-MM-DD membership in My Day, or null to remove."},
                        "recurrence_rule": {"type": ["string", "null"], "description": "RRULE recurrence, or null to clear."}
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
            due_date: match optional_update(args, "due_date") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            due_at: match optional_update(args, "due_at") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            due_timezone: match optional_update(args, "due_timezone") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            is_important: match optional_bool(args, "is_important") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            my_day_date: match optional_update(args, "my_day_date") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            recurrence_rule: match optional_update(args, "recurrence_rule") {
                Ok(value) => value,
                Err(error) => return fail(ctx, error).await,
            },
            sort_order: None,
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
