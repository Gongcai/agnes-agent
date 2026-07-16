//! Local calendar and task persistence. External providers remain adapters above this layer.

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Clone, Serialize)]
pub struct CalendarRow {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub timezone: String,
}
#[derive(Clone, Serialize)]
pub struct EventRow {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub starts_at: String,
    pub ends_at: String,
    pub timezone: String,
    pub all_day: bool,
    pub recurrence_rule: Option<String>,
    pub status: String,
}
#[derive(Clone, Serialize)]
pub struct TaskListRow {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct TaskRow {
    pub id: String,
    pub task_list_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: i64,
    pub starts_at: Option<String>,
    pub due_at: Option<String>,
    pub completed_at: Option<String>,
    pub recurrence_rule: Option<String>,
    pub sort_order: f64,
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}
fn required(value: &str, label: &str) -> AppResult<String> {
    let value = value.trim();
    if value.is_empty() {
        Err(AppError::Other(format!("{label} cannot be empty")))
    } else {
        Ok(value.into())
    }
}
fn nullable(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}
fn recurrence(value: Option<String>) -> AppResult<Option<String>> {
    let value = nullable(value);
    if let Some(value) = &value {
        if !value.starts_with("RRULE:") {
            return Err(AppError::Other(
                "Recurrence rules must use the RRULE: prefix".into(),
            ));
        }
    }
    Ok(value)
}

pub fn list_calendars(conn: &Connection) -> AppResult<Vec<CalendarRow>> {
    let mut statement = conn.prepare("SELECT id, name, color, timezone FROM calendars WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE, id")?;
    let rows = statement.query_map([], |r| {
        Ok(CalendarRow {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
            timezone: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
pub fn create_calendar(
    conn: &Connection,
    id: &str,
    name: &str,
    color: Option<String>,
    timezone: &str,
) -> AppResult<()> {
    let timestamp = now();
    conn.execute("INSERT INTO calendars (id,name,color,timezone,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?5)", params![id,required(name,"Calendar name")?,nullable(color),required(timezone,"Timezone")?,timestamp])?;
    Ok(())
}
pub fn list_events(
    conn: &Connection,
    calendar_id: &str,
    range_start: &str,
    range_end: &str,
) -> AppResult<Vec<EventRow>> {
    let mut statement=conn.prepare("SELECT id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,status FROM calendar_events WHERE calendar_id=?1 AND deleted_at IS NULL AND starts_at < ?3 AND ends_at > ?2 ORDER BY starts_at,id")?;
    let rows = statement.query_map(params![calendar_id, range_start, range_end], |r| {
        Ok(EventRow {
            id: r.get(0)?,
            calendar_id: r.get(1)?,
            title: r.get(2)?,
            starts_at: r.get(3)?,
            ends_at: r.get(4)?,
            timezone: r.get(5)?,
            all_day: r.get::<_, i64>(6)? != 0,
            recurrence_rule: r.get(7)?,
            status: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
pub fn create_event(
    conn: &Connection,
    id: &str,
    calendar_id: &str,
    title: &str,
    starts_at: &str,
    ends_at: &str,
    timezone: &str,
    all_day: bool,
    recurrence_rule: Option<String>,
) -> AppResult<()> {
    let starts_at = required(starts_at, "Event start")?;
    let ends_at = required(ends_at, "Event end")?;
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    let timestamp = now();
    conn.execute("INSERT INTO calendar_events (id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",params![id,calendar_id,required(title,"Event title")?,starts_at,ends_at,required(timezone,"Timezone")?,all_day as i64,recurrence(recurrence_rule)?,timestamp])?;
    Ok(())
}
pub fn list_task_lists(conn: &Connection) -> AppResult<Vec<TaskListRow>> {
    let mut statement=conn.prepare("SELECT id,name,color FROM task_lists WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE,id")?;
    let rows = statement.query_map([], |r| {
        Ok(TaskListRow {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
pub fn create_task_list(
    conn: &Connection,
    id: &str,
    name: &str,
    color: Option<String>,
) -> AppResult<()> {
    let timestamp = now();
    conn.execute(
        "INSERT INTO task_lists (id,name,color,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",
        params![
            id,
            required(name, "Task list name")?,
            nullable(color),
            timestamp
        ],
    )?;
    Ok(())
}
pub fn list_tasks(conn: &Connection, task_list_id: &str) -> AppResult<Vec<TaskRow>> {
    let mut statement=conn.prepare("SELECT id,task_list_id,parent_id,title,description,status,priority,starts_at,due_at,completed_at,recurrence_rule,sort_order FROM tasks WHERE task_list_id=?1 AND deleted_at IS NULL ORDER BY status='completed',sort_order,due_at,id")?;
    let rows = statement.query_map([task_list_id], |r| {
        Ok(TaskRow {
            id: r.get(0)?,
            task_list_id: r.get(1)?,
            parent_id: r.get(2)?,
            title: r.get(3)?,
            description: r.get(4)?,
            status: r.get(5)?,
            priority: r.get(6)?,
            starts_at: r.get(7)?,
            due_at: r.get(8)?,
            completed_at: r.get(9)?,
            recurrence_rule: r.get(10)?,
            sort_order: r.get(11)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}
pub fn create_task(
    conn: &Connection,
    id: &str,
    task_list_id: &str,
    parent_id: Option<String>,
    title: &str,
    description: Option<String>,
    priority: i64,
    due_at: Option<String>,
    sort_order: f64,
) -> AppResult<()> {
    if !(0..=4).contains(&priority) {
        return Err(AppError::Other(
            "Task priority must be between 0 and 4".into(),
        ));
    }
    let timestamp = now();
    conn.execute("INSERT INTO tasks (id,task_list_id,parent_id,title,description,priority,due_at,sort_order,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",params![id,task_list_id,nullable(parent_id),required(title,"Task title")?,nullable(description),priority,nullable(due_at),sort_order,timestamp])?;
    Ok(())
}
pub fn complete_task(conn: &Connection, id: &str, completed: bool) -> AppResult<()> {
    let timestamp = now();
    let changed=conn.execute("UPDATE tasks SET status=?1,completed_at=?2,updated_at=?2,version=version+1 WHERE id=?3 AND deleted_at IS NULL",params![if completed{"completed"}else{"open"},if completed{Some(timestamp.as_str())}else{None},id])?;
    if changed == 0 {
        return Err(AppError::Other("Task does not exist".into()));
    }
    Ok(())
}
