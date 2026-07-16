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

#[derive(Clone)]
pub struct EventUpdate {
    pub title: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub timezone: Option<String>,
    pub all_day: Option<bool>,
    pub recurrence_rule: Option<Option<String>>,
}

impl EventUpdate {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.starts_at.is_none()
            && self.ends_at.is_none()
            && self.timezone.is_none()
            && self.all_day.is_none()
            && self.recurrence_rule.is_none()
    }
}

#[derive(Clone)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<i64>,
    pub due_at: Option<Option<String>>,
}

impl TaskUpdate {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.priority.is_none()
            && self.due_at.is_none()
    }
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

pub fn update_event(conn: &Connection, id: &str, changes: EventUpdate) -> AppResult<EventRow> {
    if changes.is_empty() {
        return Err(AppError::Other(
            "Event update must change at least one field".into(),
        ));
    }
    let current = conn
        .query_row(
            "SELECT calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,status \
             FROM calendar_events WHERE id=?1 AND deleted_at IS NULL",
            [id],
            |row| {
                Ok(EventRow {
                    id: id.to_string(),
                    calendar_id: row.get(0)?,
                    title: row.get(1)?,
                    starts_at: row.get(2)?,
                    ends_at: row.get(3)?,
                    timezone: row.get(4)?,
                    all_day: row.get::<_, i64>(5)? != 0,
                    recurrence_rule: row.get(6)?,
                    status: row.get(7)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => AppError::Other("Event does not exist".into()),
            error => error.into(),
        })?;
    let title = changes
        .title
        .map(|value| required(&value, "Event title"))
        .transpose()?
        .unwrap_or(current.title);
    let starts_at = changes
        .starts_at
        .map(|value| required(&value, "Event start"))
        .transpose()?
        .unwrap_or(current.starts_at);
    let ends_at = changes
        .ends_at
        .map(|value| required(&value, "Event end"))
        .transpose()?
        .unwrap_or(current.ends_at);
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    let timezone = changes
        .timezone
        .map(|value| required(&value, "Timezone"))
        .transpose()?
        .unwrap_or(current.timezone);
    let all_day = changes.all_day.unwrap_or(current.all_day);
    let recurrence_rule = match changes.recurrence_rule {
        Some(value) => recurrence(value)?,
        None => current.recurrence_rule,
    };
    let timestamp = now();
    conn.execute(
        "UPDATE calendar_events SET title=?1,starts_at=?2,ends_at=?3,timezone=?4,all_day=?5,recurrence_rule=?6,updated_at=?7,version=version+1 WHERE id=?8 AND deleted_at IS NULL",
        params![title, starts_at, ends_at, timezone, all_day as i64, recurrence_rule, timestamp, id],
    )?;
    Ok(EventRow {
        id: id.to_string(),
        calendar_id: current.calendar_id,
        title,
        starts_at,
        ends_at,
        timezone,
        all_day,
        recurrence_rule,
        status: current.status,
    })
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

pub fn update_task(conn: &Connection, id: &str, changes: TaskUpdate) -> AppResult<TaskRow> {
    if changes.is_empty() {
        return Err(AppError::Other(
            "Task update must change at least one field".into(),
        ));
    }
    let current = conn
        .query_row(
            "SELECT task_list_id,parent_id,title,description,status,priority,starts_at,due_at,completed_at,recurrence_rule,sort_order \
             FROM tasks WHERE id=?1 AND deleted_at IS NULL",
            [id],
            |row| {
                Ok(TaskRow {
                    id: id.to_string(),
                    task_list_id: row.get(0)?,
                    parent_id: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    status: row.get(4)?,
                    priority: row.get(5)?,
                    starts_at: row.get(6)?,
                    due_at: row.get(7)?,
                    completed_at: row.get(8)?,
                    recurrence_rule: row.get(9)?,
                    sort_order: row.get(10)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => AppError::Other("Task does not exist".into()),
            error => error.into(),
        })?;
    let title = changes
        .title
        .map(|value| required(&value, "Task title"))
        .transpose()?
        .unwrap_or(current.title);
    let description = changes.description.unwrap_or(current.description);
    let priority = changes.priority.unwrap_or(current.priority);
    if !(0..=4).contains(&priority) {
        return Err(AppError::Other(
            "Task priority must be between 0 and 4".into(),
        ));
    }
    let due_at = changes.due_at.unwrap_or(current.due_at);
    let timestamp = now();
    conn.execute(
        "UPDATE tasks SET title=?1,description=?2,priority=?3,due_at=?4,updated_at=?5,version=version+1 WHERE id=?6 AND deleted_at IS NULL",
        params![title, description, priority, due_at, timestamp, id],
    )?;
    Ok(TaskRow {
        id: id.to_string(),
        task_list_id: current.task_list_id,
        parent_id: current.parent_id,
        title,
        description,
        status: current.status,
        priority,
        starts_at: current.starts_at,
        due_at,
        completed_at: current.completed_at,
        recurrence_rule: current.recurrence_rule,
        sort_order: current.sort_order,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_events_and_tasks_without_overwriting_omitted_fields() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_calendar(&conn, "calendar-1", "Personal", None, "Asia/Shanghai").unwrap();
        create_event(
            &conn,
            "event-1",
            "calendar-1",
            "Planning",
            "2026-07-17T09:00:00Z",
            "2026-07-17T10:00:00Z",
            "Asia/Shanghai",
            false,
            None,
        )
        .unwrap();

        let event = update_event(
            &conn,
            "event-1",
            EventUpdate {
                title: Some("Updated planning".into()),
                starts_at: None,
                ends_at: Some("2026-07-17T11:00:00Z".into()),
                timezone: None,
                all_day: Some(true),
                recurrence_rule: Some(Some("RRULE:FREQ=WEEKLY".into())),
            },
        )
        .unwrap();
        assert_eq!(event.title, "Updated planning");
        assert_eq!(event.starts_at, "2026-07-17T09:00:00Z");
        assert_eq!(event.ends_at, "2026-07-17T11:00:00Z");
        assert!(event.all_day);
        assert_eq!(event.recurrence_rule.as_deref(), Some("RRULE:FREQ=WEEKLY"));
        assert!(update_event(
            &conn,
            "event-1",
            EventUpdate {
                title: None,
                starts_at: None,
                ends_at: Some("2026-07-17T08:00:00Z".into()),
                timezone: None,
                all_day: None,
                recurrence_rule: None,
            },
        )
        .is_err());

        create_task_list(&conn, "task-list-1", "Work", None).unwrap();
        create_task(
            &conn,
            "task-1",
            "task-list-1",
            None,
            "Draft proposal",
            Some("Initial outline".into()),
            1,
            Some("2026-07-18T09:00:00Z".into()),
            0.0,
        )
        .unwrap();

        let task = update_task(
            &conn,
            "task-1",
            TaskUpdate {
                title: None,
                description: Some(None),
                priority: Some(3),
                due_at: Some(Some("2026-07-19T09:00:00Z".into())),
            },
        )
        .unwrap();
        assert_eq!(task.title, "Draft proposal");
        assert_eq!(task.description, None);
        assert_eq!(task.priority, 3);
        assert_eq!(task.due_at.as_deref(), Some("2026-07-19T09:00:00Z"));
    }
}
