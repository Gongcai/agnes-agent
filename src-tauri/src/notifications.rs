//! Shared, device-local notification service.
//!
//! Sources only describe an event; this module owns persistence, deduplication,
//! scheduling and renderer delivery. Native operating-system delivery can be
//! added later as another output adapter without changing feature modules.

use std::sync::Arc;

use chrono::{DateTime, Duration, LocalResult, NaiveDate, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz as ChronoTz;
use tauri::{AppHandle, Emitter};

use crate::db::repo::notifications::{NewNotification, NotificationRow};
use crate::db::repo::planner::{EventRow, TaskRow};
use crate::db::DbActorHandle;
use crate::error::AppResult;

const SCAN_INTERVAL_SECONDS: u64 = 30;
const MISSED_WINDOW_MINUTES: i64 = 10;

#[derive(Clone)]
pub struct NotificationService {
    db: DbActorHandle,
    app_handle: AppHandle,
}

impl NotificationService {
    pub fn new(db: DbActorHandle, app_handle: AppHandle) -> Self {
        Self { db, app_handle }
    }

    /// Start a lightweight local scheduler. The first scan happens immediately
    /// so reminders survive a short application restart without duplication.
    pub fn start_background(self: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(SCAN_INTERVAL_SECONDS));
            loop {
                interval.tick().await;
                if let Err(error) = self.scan_due().await {
                    eprintln!("[notifications] reminder scan failed: {error}");
                }
            }
        });
    }

    async fn persist(&self, notification: NewNotification) -> AppResult<Option<NotificationRow>> {
        let created = self.db.create_notification(notification).await?;
        if let Some(ref row) = created {
            let _ = self.app_handle.emit("notification://created", row);
        }
        Ok(created)
    }

    pub async fn notify_agent_completed(&self, session_id: &str, run_id: &str) -> AppResult<()> {
        let session_title = self
            .db
            .get_session(session_id.to_string())
            .await?
            .map(|session| session.title)
            .unwrap_or_else(|| "对话".to_string());
        self.persist(NewNotification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "agent_completed".to_string(),
            title: "AI 已完成回复".to_string(),
            body: Some(format!("“{session_title}”已有新的回复。")),
            target_kind: "chat".to_string(),
            target_id: Some(session_id.to_string()),
            source_kind: "agent_run".to_string(),
            source_id: run_id.to_string(),
            dedupe_key: format!("agent-run:{run_id}:completed"),
            scheduled_at: None,
        })
        .await?;
        Ok(())
    }

    pub async fn notify_approval_requested(
        &self,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
    ) -> AppResult<()> {
        self.persist(NewNotification {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "approval_requested".to_string(),
            title: "AI 请求授权".to_string(),
            body: Some(format!("工具 “{tool_name}” 正在等待你的确认。")),
            target_kind: "chat".to_string(),
            target_id: Some(session_id.to_string()),
            source_kind: "approval".to_string(),
            source_id: tool_call_id.to_string(),
            dedupe_key: format!("approval:{tool_call_id}"),
            scheduled_at: None,
        })
        .await?;
        Ok(())
    }

    pub async fn resolve_approval(&self, tool_call_id: &str) -> AppResult<()> {
        self.db
            .mark_notification_source_read("approval".to_string(), tool_call_id.to_string())
            .await?;
        self.emit_changed();
        Ok(())
    }

    pub async fn mark_read(&self, notification_id: &str) -> AppResult<()> {
        self.db
            .mark_notification_read(notification_id.to_string())
            .await?;
        self.emit_changed();
        Ok(())
    }

    pub async fn mark_all_read(&self) -> AppResult<()> {
        self.db.mark_all_notifications_read().await?;
        self.emit_changed();
        Ok(())
    }

    fn emit_changed(&self) {
        let _ = self.app_handle.emit("notification://changed", ());
    }

    pub async fn scan_due(&self) -> AppResult<usize> {
        let now = Utc::now();
        let since = now - Duration::minutes(MISSED_WINDOW_MINUTES);
        let mut created = 0;

        for task in self.db.list_all_tasks().await? {
            if task.status != "open" {
                continue;
            }
            let Some(remind_at) = notification_time_for_task(&task) else {
                continue;
            };
            if !inside_window(remind_at, since, now) {
                continue;
            }
            if self
                .persist(NewNotification {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind: "task_due".to_string(),
                    title: "任务到期".to_string(),
                    body: Some(format!("“{}”现在需要处理。", task.title)),
                    target_kind: "task".to_string(),
                    target_id: Some(task.id.clone()),
                    source_kind: "task".to_string(),
                    source_id: task.id.clone(),
                    dedupe_key: format!("task:{}:{}", task.id, format_instant(remind_at)),
                    scheduled_at: Some(format_instant(remind_at)),
                })
                .await?
                .is_some()
            {
                created += 1;
            }
        }

        // A full-day lookback admits 09:00 reminders for all-day events in every
        // supported timezone. `notification_time_for_event` still rejects older
        // events, so multi-day items do not produce repeated notifications.
        let calendar_start = (since - Duration::days(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
        let calendar_end = (now + Duration::seconds(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
        for calendar in self.db.list_calendars().await? {
            for event in self
                .db
                .list_calendar_events(calendar.id, calendar_start.clone(), calendar_end.clone())
                .await?
            {
                if event.status == "cancelled" {
                    continue;
                }
                let Some(remind_at) = notification_time_for_event(&event) else {
                    continue;
                };
                if !inside_window(remind_at, since, now) {
                    continue;
                }
                let event_label = if event.all_day {
                    "全天事件开始"
                } else {
                    "事件开始"
                };
                if self
                    .persist(NewNotification {
                        id: uuid::Uuid::new_v4().to_string(),
                        kind: "event_start".to_string(),
                        title: event_label.to_string(),
                        body: Some(format!("“{}”现在开始。", event.title)),
                        target_kind: "calendar".to_string(),
                        target_id: Some(event.id.clone()),
                        source_kind: "calendar_event".to_string(),
                        source_id: event.occurrence_id.clone(),
                        dedupe_key: format!(
                            "calendar-event:{}:{}",
                            event.occurrence_id,
                            format_instant(remind_at)
                        ),
                        scheduled_at: Some(format_instant(remind_at)),
                    })
                    .await?
                    .is_some()
                {
                    created += 1;
                }
            }
        }
        Ok(created)
    }
}

fn format_instant(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_instant(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn local_nine_am(date: NaiveDate, timezone: &str) -> Option<DateTime<Utc>> {
    let timezone = timezone.parse::<ChronoTz>().ok()?;
    let local = date.and_hms_opt(9, 0, 0)?;
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => {
            Some(value.with_timezone(&Utc))
        }
        LocalResult::None => None,
    }
}

/// Timed items notify at their exact instant. Date-only tasks and all-day events
/// notify at 09:00 in their own IANA timezone, which avoids an unexpected
/// midnight alert while remaining deterministic across daylight-saving changes.
fn notification_time_for_task(task: &TaskRow) -> Option<DateTime<Utc>> {
    if let Some(due_at) = task.due_at.as_deref() {
        return parse_instant(due_at);
    }
    let due_date = NaiveDate::parse_from_str(task.due_date.as_deref()?, "%Y-%m-%d").ok()?;
    local_nine_am(due_date, task.due_timezone.as_deref().unwrap_or("UTC"))
}

fn notification_time_for_event(event: &EventRow) -> Option<DateTime<Utc>> {
    let starts_at = parse_instant(&event.starts_at)?;
    if !event.all_day {
        return Some(starts_at);
    }
    let timezone = event.timezone.parse::<ChronoTz>().ok()?;
    local_nine_am(
        starts_at.with_timezone(&timezone).date_naive(),
        &event.timezone,
    )
}

fn inside_window(value: DateTime<Utc>, since: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    value >= since && value <= now
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(due_date: Option<&str>, due_at: Option<&str>) -> TaskRow {
        TaskRow {
            id: "task-1".to_string(),
            task_list_id: "list-1".to_string(),
            parent_id: None,
            title: "Report".to_string(),
            description: None,
            status: "open".to_string(),
            priority: 0,
            starts_at: None,
            due_date: due_date.map(str::to_string),
            due_at: due_at.map(str::to_string),
            due_timezone: Some("Asia/Shanghai".to_string()),
            is_important: false,
            my_day_date: None,
            completed_at: None,
            recurrence_rule: None,
            recurrence_anchor: None,
            recurrence_source_id: None,
            sort_order: 0.0,
        }
    }

    #[test]
    fn date_only_tasks_use_nine_am_in_the_task_timezone() {
        let reminder = notification_time_for_task(&task(Some("2026-07-17"), None)).unwrap();
        assert_eq!(format_instant(reminder), "2026-07-17T01:00:00Z");
    }

    #[test]
    fn timed_tasks_keep_their_absolute_due_instant() {
        let reminder =
            notification_time_for_task(&task(None, Some("2026-07-17T01:00:00Z"))).unwrap();
        assert_eq!(format_instant(reminder), "2026-07-17T01:00:00Z");
    }

    #[test]
    fn all_day_events_use_their_own_timezone() {
        let event = EventRow {
            id: "event-1".to_string(),
            occurrence_id: "event-1".to_string(),
            calendar_id: "calendar-1".to_string(),
            title: "Holiday".to_string(),
            starts_at: "2026-07-16T16:00:00Z".to_string(),
            ends_at: "2026-07-17T16:00:00Z".to_string(),
            timezone: "Asia/Shanghai".to_string(),
            all_day: true,
            recurrence_rule: None,
            original_occurrence: None,
            is_exception: false,
            status: "confirmed".to_string(),
        };
        assert_eq!(
            format_instant(notification_time_for_event(&event).unwrap()),
            "2026-07-17T01:00:00Z"
        );
    }
}
