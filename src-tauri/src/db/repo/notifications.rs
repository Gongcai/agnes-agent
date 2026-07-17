//! Device-local notification inbox persistence.

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use crate::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize)]
pub struct NotificationRow {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub target_kind: String,
    pub target_id: Option<String>,
    pub source_kind: String,
    pub source_id: String,
    pub scheduled_at: Option<String>,
    pub delivered_at: String,
    pub read_at: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct NewNotification {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub target_kind: String,
    pub target_id: Option<String>,
    pub source_kind: String,
    pub source_id: String,
    pub dedupe_key: String,
    pub scheduled_at: Option<String>,
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn trim_required(value: String, label: &str) -> AppResult<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(AppError::Other(format!(
            "Notification {label} cannot be empty"
        )))
    } else {
        Ok(value)
    }
}

fn optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn row_from(row: &Row<'_>) -> rusqlite::Result<NotificationRow> {
    Ok(NotificationRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        target_kind: row.get(4)?,
        target_id: row.get(5)?,
        source_kind: row.get(6)?,
        source_id: row.get(7)?,
        scheduled_at: row.get(8)?,
        delivered_at: row.get(9)?,
        read_at: row.get(10)?,
        created_at: row.get(11)?,
    })
}

const SELECT: &str = "SELECT id,kind,title,body,target_kind,target_id,source_kind,source_id, \
                      scheduled_at,delivered_at,read_at,created_at FROM notifications";

/// Insert one notification exactly once. A duplicate key is a normal no-op.
pub fn create_if_absent(
    conn: &Connection,
    notification: NewNotification,
) -> AppResult<Option<NotificationRow>> {
    let delivered_at = now();
    let title = trim_required(notification.title, "title")?;
    let kind = trim_required(notification.kind, "kind")?;
    let target_kind = trim_required(notification.target_kind, "target kind")?;
    let source_kind = trim_required(notification.source_kind, "source kind")?;
    let source_id = trim_required(notification.source_id, "source id")?;
    let dedupe_key = trim_required(notification.dedupe_key, "dedupe key")?;
    let changed = conn.execute(
        "INSERT INTO notifications \
         (id,kind,title,body,target_kind,target_id,source_kind,source_id,dedupe_key, \
          scheduled_at,delivered_at,created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11) \
         ON CONFLICT(dedupe_key) DO NOTHING",
        params![
            notification.id,
            kind,
            title,
            optional_text(notification.body),
            target_kind,
            optional_text(notification.target_id),
            source_kind,
            source_id,
            dedupe_key,
            optional_text(notification.scheduled_at),
            delivered_at,
        ],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    let id = conn.last_insert_rowid();
    Ok(conn
        .query_row(&format!("{SELECT} WHERE rowid=?1"), [id], row_from)
        .optional()?)
}

pub fn list(conn: &Connection, limit: usize) -> AppResult<Vec<NotificationRow>> {
    let limit = limit.clamp(1, 200) as i64;
    let mut statement = conn.prepare(&format!(
        "{SELECT} ORDER BY read_at IS NOT NULL, delivered_at DESC, id DESC LIMIT ?1"
    ))?;
    let rows = statement.query_map([limit], row_from)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn mark_read(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE notifications SET read_at=COALESCE(read_at, ?1) WHERE id=?2",
        params![now(), id],
    )?;
    Ok(())
}

pub fn mark_all_read(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "UPDATE notifications SET read_at=?1 WHERE read_at IS NULL",
        [now()],
    )?;
    Ok(())
}

pub fn mark_source_read(conn: &Connection, source_kind: &str, source_id: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE notifications SET read_at=COALESCE(read_at, ?1) \
         WHERE source_kind=?2 AND source_id=?3",
        params![now(), source_kind, source_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(id: &str, dedupe_key: &str) -> NewNotification {
        NewNotification {
            id: id.to_string(),
            kind: "task_due".to_string(),
            title: "提交报告".to_string(),
            body: Some("任务已到期".to_string()),
            target_kind: "task".to_string(),
            target_id: Some("task-1".to_string()),
            source_kind: "task".to_string(),
            source_id: "task-1".to_string(),
            dedupe_key: dedupe_key.to_string(),
            scheduled_at: Some("2026-07-17T01:00:00Z".to_string()),
        }
    }

    #[test]
    fn deduplicates_and_orders_the_local_inbox() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notifications (\
               id TEXT PRIMARY KEY,kind TEXT NOT NULL,title TEXT NOT NULL,body TEXT,\
               target_kind TEXT NOT NULL,target_id TEXT,source_kind TEXT NOT NULL,\
               source_id TEXT NOT NULL,dedupe_key TEXT NOT NULL UNIQUE,scheduled_at TEXT,\
               delivered_at TEXT NOT NULL,read_at TEXT,created_at TEXT NOT NULL\
             );",
        )
        .unwrap();

        assert!(create_if_absent(&conn, notification("notice-1", "task:1"))
            .unwrap()
            .is_some());
        assert!(create_if_absent(&conn, notification("notice-2", "task:1"))
            .unwrap()
            .is_none());
        assert_eq!(list(&conn, 20).unwrap().len(), 1);
        mark_read(&conn, "notice-1").unwrap();
        assert!(list(&conn, 20).unwrap()[0].read_at.is_some());
        mark_source_read(&conn, "task", "task-1").unwrap();
        mark_all_read(&conn).unwrap();
    }
}
