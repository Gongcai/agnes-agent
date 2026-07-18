//! Local calendar and task persistence. External providers remain adapters above this layer.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, LocalResult, NaiveDate, SecondsFormat, TimeZone, Utc};
use chrono_tz::Tz as ChronoTz;
use rrule::{RRule, RRuleSet, Tz as RRuleTz, Unvalidated};
use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};

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
    pub occurrence_id: String,
    pub calendar_id: String,
    pub title: String,
    pub starts_at: String,
    pub ends_at: String,
    pub timezone: String,
    pub all_day: bool,
    pub recurrence_rule: Option<String>,
    pub original_occurrence: Option<String>,
    pub is_exception: bool,
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
    pub due_date: Option<String>,
    pub due_at: Option<String>,
    pub due_timezone: Option<String>,
    pub is_important: bool,
    pub my_day_date: Option<String>,
    pub completed_at: Option<String>,
    pub recurrence_rule: Option<String>,
    pub recurrence_anchor: Option<String>,
    pub recurrence_source_id: Option<String>,
    pub sort_order: f64,
}

#[derive(Serialize)]
struct CalendarSyncSource {
    id: String,
    name: String,
    color: Option<String>,
    timezone: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    origin_device_id: Option<String>,
}

#[derive(Serialize)]
struct CalendarEventSyncSource {
    id: String,
    calendar_id: String,
    title: String,
    description: Option<String>,
    location: Option<String>,
    starts_at: String,
    ends_at: String,
    timezone: String,
    all_day: i64,
    recurrence_rule: Option<String>,
    recurrence_id: Option<String>,
    status: String,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    origin_device_id: Option<String>,
}

#[derive(Serialize)]
struct EventExceptionSyncSource {
    id: String,
    event_id: String,
    original_occurrence: String,
    replacement_event_id: Option<String>,
    is_cancelled: i64,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    origin_device_id: Option<String>,
}

#[derive(Serialize)]
struct TaskListSyncSource {
    id: String,
    name: String,
    color: Option<String>,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    origin_device_id: Option<String>,
}

#[derive(Serialize)]
struct TaskSyncSource {
    id: String,
    task_list_id: String,
    parent_id: Option<String>,
    title: String,
    description: Option<String>,
    status: String,
    priority: i64,
    starts_at: Option<String>,
    due_date: Option<String>,
    due_at: Option<String>,
    due_timezone: Option<String>,
    is_important: i64,
    my_day_date: Option<String>,
    completed_at: Option<String>,
    recurrence_rule: Option<String>,
    recurrence_anchor: Option<String>,
    recurrence_source_id: Option<String>,
    sort_order: f64,
    created_at: String,
    updated_at: String,
    version: i64,
    deleted_at: Option<String>,
    origin_device_id: Option<String>,
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
pub struct OccurrenceUpdate {
    pub title: Option<String>,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub timezone: Option<String>,
    pub all_day: Option<bool>,
}

impl OccurrenceUpdate {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.starts_at.is_none()
            && self.ends_at.is_none()
            && self.timezone.is_none()
            && self.all_day.is_none()
    }
}

#[derive(Clone)]
struct StoredEvent {
    id: String,
    calendar_id: String,
    title: String,
    starts_at: String,
    ends_at: String,
    timezone: String,
    all_day: bool,
    recurrence_rule: Option<String>,
    recurrence_id: Option<String>,
    status: String,
}

#[derive(Clone)]
struct EventException {
    cancelled: bool,
    replacement: Option<StoredEvent>,
}

const MAX_OCCURRENCES_PER_SERIES: u16 = 4_096;

#[derive(Clone)]
pub struct TaskUpdate {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<i64>,
    pub due_date: Option<Option<String>>,
    pub due_at: Option<Option<String>>,
    pub due_timezone: Option<Option<String>>,
    pub is_important: Option<bool>,
    pub my_day_date: Option<Option<String>>,
    pub recurrence_rule: Option<Option<String>>,
    pub sort_order: Option<f64>,
}

impl TaskUpdate {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.priority.is_none()
            && self.due_date.is_none()
            && self.due_at.is_none()
            && self.due_timezone.is_none()
            && self.is_important.is_none()
            && self.my_day_date.is_none()
            && self.recurrence_rule.is_none()
            && self.sort_order.is_none()
    }
}

pub struct NewTask {
    pub id: String,
    pub task_list_id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: i64,
    pub due_date: Option<String>,
    pub due_at: Option<String>,
    pub due_timezone: Option<String>,
    pub is_important: bool,
    pub my_day_date: Option<String>,
    pub recurrence_rule: Option<String>,
    pub sort_order: f64,
}

fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

pub(crate) fn event_exception_entity_id(event_id: &str, original_occurrence: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(event_id.as_bytes());
    hasher.update([0]);
    hasher.update(original_occurrence.as_bytes());
    format!("event_exception:{:x}", hasher.finalize())
}

fn recurring_task_id(source_id: &str, due_date: Option<&str>, due_at: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_id.as_bytes());
    hasher.update([0]);
    hasher.update(due_date.or(due_at).unwrap_or_default().as_bytes());
    format!("recurring_task:{:x}", hasher.finalize())
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
fn parse_instant(value: &str, label: &str) -> AppResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| AppError::Other(format!("{label} must be an ISO 8601 instant")))
}

fn format_instant(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_timezone(value: &str) -> AppResult<ChronoTz> {
    value
        .trim()
        .parse::<ChronoTz>()
        .map_err(|_| AppError::Other("Timezone must be a valid IANA timezone".into()))
}

fn parse_local_date(value: &str, label: &str) -> AppResult<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::Other(format!("{label} must use YYYY-MM-DD")))
}

fn canonical_date(value: Option<String>, label: &str) -> AppResult<Option<String>> {
    nullable(value)
        .map(|value| {
            parse_local_date(&value, label).map(|date| date.format("%Y-%m-%d").to_string())
        })
        .transpose()
}

fn local_midnight(date: NaiveDate, timezone: ChronoTz) -> AppResult<DateTime<Utc>> {
    let local = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| AppError::Other("Task date is outside the supported range".into()))?;
    match timezone.from_local_datetime(&local) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Ok(first.with_timezone(&Utc)),
        LocalResult::None => Err(AppError::Other(
            "Task date does not map to a valid local time".into(),
        )),
    }
}

fn recurrence(
    value: Option<String>,
    starts_at: DateTime<Utc>,
    timezone: ChronoTz,
) -> AppResult<Option<String>> {
    let Some(value) = nullable(value) else {
        return Ok(None);
    };
    let body = value
        .strip_prefix("RRULE:")
        .filter(|body| !body.is_empty() && !body.contains(['\r', '\n']))
        .ok_or_else(|| {
            AppError::Other("Recurrence rules must contain one RRULE: prefixed rule".into())
        })?;
    let unvalidated = body
        .parse::<RRule<Unvalidated>>()
        .map_err(|error| AppError::Other(format!("Invalid recurrence rule: {error}")))?;
    let recurrence_timezone = RRuleTz::from(timezone);
    unvalidated
        .validate(starts_at.with_timezone(&recurrence_timezone))
        .map_err(|error| AppError::Other(format!("Invalid recurrence rule: {error}")))?;
    Ok(Some(value))
}

fn build_recurrence_set(
    recurrence_rule: &str,
    starts_at: DateTime<Utc>,
    timezone: ChronoTz,
) -> AppResult<RRuleSet> {
    let rule = recurrence(Some(recurrence_rule.to_string()), starts_at, timezone)?
        .ok_or_else(|| AppError::Other("Recurrence rule is required".into()))?;
    let body = rule
        .strip_prefix("RRULE:")
        .ok_or_else(|| AppError::Other("Stored recurrence rule is invalid".into()))?;
    let recurrence_timezone = RRuleTz::from(timezone);
    body.parse::<RRule<Unvalidated>>()
        .and_then(|rule| rule.build(starts_at.with_timezone(&recurrence_timezone)))
        .map_err(|error| AppError::Other(format!("Invalid recurrence rule: {error}")))
}

struct NormalizedTaskSchedule {
    due_date: Option<String>,
    due_at: Option<String>,
    due_timezone: Option<String>,
    recurrence_rule: Option<String>,
    recurrence_anchor: Option<String>,
}

fn normalize_task_schedule(
    due_date: Option<String>,
    due_at: Option<String>,
    due_timezone: Option<String>,
    recurrence_rule: Option<String>,
    recurrence_anchor: Option<String>,
) -> AppResult<NormalizedTaskSchedule> {
    let due_date = canonical_date(due_date, "Task due date")?;
    let due_at = nullable(due_at)
        .map(|value| parse_instant(&value, "Task due time").map(format_instant))
        .transpose()?;
    if due_date.is_some() && due_at.is_some() {
        return Err(AppError::Other(
            "Task cannot have both a due date and a due time".into(),
        ));
    }
    let has_due = due_date.is_some() || due_at.is_some();
    let timezone = if has_due {
        Some(parse_timezone(due_timezone.as_deref().unwrap_or("UTC"))?)
    } else {
        None
    };
    let recurrence_rule = nullable(recurrence_rule);
    if recurrence_rule.is_some() && !has_due {
        return Err(AppError::Other(
            "Recurring tasks require a due date or due time".into(),
        ));
    }
    let recurrence_rule = match (recurrence_rule, timezone) {
        (Some(rule), Some(timezone)) => {
            let starts_at = match (&due_date, &due_at) {
                (Some(date), None) => {
                    local_midnight(parse_local_date(date, "Task due date")?, timezone)?
                }
                (None, Some(instant)) => parse_instant(instant, "Task due time")?,
                _ => unreachable!("validated task due shape"),
            };
            recurrence(Some(rule), starts_at, timezone)?
        }
        (None, _) => None,
        _ => unreachable!("recurrence requires a due value"),
    };
    let recurrence_anchor = if recurrence_rule.is_some() {
        recurrence_anchor.or_else(|| due_date.clone().or_else(|| due_at.clone()))
    } else {
        None
    };
    Ok(NormalizedTaskSchedule {
        due_date,
        due_at,
        due_timezone: timezone.map(|timezone| timezone.name().to_string()),
        recurrence_rule,
        recurrence_anchor,
    })
}

fn stored_event_from_row(row: &Row<'_>) -> rusqlite::Result<StoredEvent> {
    Ok(StoredEvent {
        id: row.get(0)?,
        calendar_id: row.get(1)?,
        title: row.get(2)?,
        starts_at: row.get(3)?,
        ends_at: row.get(4)?,
        timezone: row.get(5)?,
        all_day: row.get::<_, i64>(6)? != 0,
        recurrence_rule: row.get(7)?,
        recurrence_id: row.get(8)?,
        status: row.get(9)?,
    })
}

fn get_event(conn: &Connection, id: &str) -> AppResult<StoredEvent> {
    conn.query_row(
        "SELECT id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,recurrence_id,status \
         FROM calendar_events WHERE id=?1 AND deleted_at IS NULL",
        [id],
        stored_event_from_row,
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => AppError::Other("Event does not exist".into()),
        error => error.into(),
    })
}

pub fn get_event_by_id(conn: &Connection, id: &str) -> AppResult<EventRow> {
    let event = get_event(conn, id)?;
    if event.recurrence_id.is_some() {
        return Err(AppError::Other(
            "Calendar event lookup requires a master event id".into(),
        ));
    }
    Ok(master_row(&event))
}

fn occurrence_id(event_id: &str, original_occurrence: &str) -> String {
    format!("{event_id}@{original_occurrence}")
}

fn master_row(event: &StoredEvent) -> EventRow {
    EventRow {
        id: event.id.clone(),
        occurrence_id: event.id.clone(),
        calendar_id: event.calendar_id.clone(),
        title: event.title.clone(),
        starts_at: event.starts_at.clone(),
        ends_at: event.ends_at.clone(),
        timezone: event.timezone.clone(),
        all_day: event.all_day,
        recurrence_rule: event.recurrence_rule.clone(),
        original_occurrence: None,
        is_exception: false,
        status: event.status.clone(),
    }
}

fn occurrence_row(
    master: &StoredEvent,
    original_occurrence: &str,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    replacement: Option<&StoredEvent>,
) -> AppResult<EventRow> {
    let (title, starts_at, ends_at, timezone, all_day, status, is_exception) =
        if let Some(replacement) = replacement {
            (
                replacement.title.clone(),
                format_instant(parse_instant(
                    &replacement.starts_at,
                    "Replacement event start",
                )?),
                format_instant(parse_instant(
                    &replacement.ends_at,
                    "Replacement event end",
                )?),
                replacement.timezone.clone(),
                replacement.all_day,
                replacement.status.clone(),
                true,
            )
        } else {
            (
                master.title.clone(),
                format_instant(starts_at),
                format_instant(ends_at),
                master.timezone.clone(),
                master.all_day,
                master.status.clone(),
                false,
            )
        };
    Ok(EventRow {
        id: master.id.clone(),
        occurrence_id: occurrence_id(&master.id, original_occurrence),
        calendar_id: master.calendar_id.clone(),
        title,
        starts_at,
        ends_at,
        timezone,
        all_day,
        recurrence_rule: master.recurrence_rule.clone(),
        original_occurrence: Some(original_occurrence.to_string()),
        is_exception,
        status,
    })
}

fn event_times(event: &StoredEvent) -> AppResult<(DateTime<Utc>, DateTime<Utc>, Duration)> {
    let starts_at = parse_instant(&event.starts_at, "Event start")?;
    let ends_at = parse_instant(&event.ends_at, "Event end")?;
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    Ok((starts_at, ends_at, ends_at - starts_at))
}

fn recurrence_set(event: &StoredEvent) -> AppResult<RRuleSet> {
    let (starts_at, _, _) = event_times(event)?;
    let timezone = parse_timezone(&event.timezone)?;
    let rule = event
        .recurrence_rule
        .as_deref()
        .ok_or_else(|| AppError::Other("Event is not recurring".into()))?;
    build_recurrence_set(rule, starts_at, timezone)
}

fn overlaps(
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    range_start: DateTime<Utc>,
    range_end: DateTime<Utc>,
) -> bool {
    starts_at < range_end && ends_at > range_start
}

fn load_exceptions(
    conn: &Connection,
    calendar_id: &str,
) -> AppResult<HashMap<String, HashMap<String, EventException>>> {
    let mut statement = conn.prepare(
        "SELECT ee.event_id,ee.original_occurrence,ee.is_cancelled, \
                replacement.id,replacement.calendar_id,replacement.title,replacement.starts_at, \
                replacement.ends_at,replacement.timezone,replacement.all_day, \
                replacement.recurrence_rule,replacement.recurrence_id,replacement.status \
         FROM event_exceptions ee \
         JOIN calendar_events master ON master.id=ee.event_id \
         LEFT JOIN calendar_events replacement \
           ON replacement.id=ee.replacement_event_id AND replacement.deleted_at IS NULL \
         WHERE master.calendar_id=?1 AND master.deleted_at IS NULL AND ee.deleted_at IS NULL",
    )?;
    let rows = statement.query_map([calendar_id], |row| {
        let replacement = match row.get::<_, Option<String>>(3)? {
            Some(id) => Some(StoredEvent {
                id,
                calendar_id: row.get(4)?,
                title: row.get(5)?,
                starts_at: row.get(6)?,
                ends_at: row.get(7)?,
                timezone: row.get(8)?,
                all_day: row.get::<_, i64>(9)? != 0,
                recurrence_rule: row.get(10)?,
                recurrence_id: row.get(11)?,
                status: row.get(12)?,
            }),
            None => None,
        };
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            EventException {
                cancelled: row.get::<_, i64>(2)? != 0,
                replacement,
            },
        ))
    })?;
    let mut grouped: HashMap<String, HashMap<String, EventException>> = HashMap::new();
    for row in rows {
        let (event_id, original_occurrence, exception) = row?;
        grouped
            .entry(event_id)
            .or_default()
            .insert(original_occurrence, exception);
    }
    Ok(grouped)
}

fn generated_occurrence(
    event: &StoredEvent,
    original_occurrence: &str,
) -> AppResult<(String, DateTime<Utc>, DateTime<Utc>)> {
    if event.recurrence_id.is_some() {
        return Err(AppError::Other(
            "Occurrence exceptions must target a recurring master event".into(),
        ));
    }
    let original = parse_instant(original_occurrence, "Original occurrence")?;
    let canonical = format_instant(original);
    let (_, _, duration) = event_times(event)?;
    let recurrence_timezone = RRuleTz::from(parse_timezone(&event.timezone)?);
    let result = recurrence_set(event)?
        .after((original - Duration::seconds(1)).with_timezone(&recurrence_timezone))
        .before((original + Duration::seconds(1)).with_timezone(&recurrence_timezone))
        .all(2);
    let matches = result
        .dates
        .into_iter()
        .any(|candidate| candidate.with_timezone(&Utc) == original);
    if !matches {
        return Err(AppError::Other(
            "Original occurrence does not belong to the recurring event".into(),
        ));
    }
    Ok((canonical, original, original + duration))
}

fn get_calendar_sync_source(conn: &Connection, id: &str) -> AppResult<Option<CalendarSyncSource>> {
    conn.query_row(
        "SELECT id,name,color,timezone,created_at,updated_at,version,deleted_at,origin_device_id \
         FROM calendars WHERE id=?1",
        [id],
        |row| {
            Ok(CalendarSyncSource {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                timezone: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                version: row.get(6)?,
                deleted_at: row.get(7)?,
                origin_device_id: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn get_event_sync_source(
    conn: &Connection,
    id: &str,
) -> AppResult<Option<CalendarEventSyncSource>> {
    conn.query_row(
        "SELECT id,calendar_id,title,description,location,starts_at,ends_at,timezone,all_day, \
                recurrence_rule,recurrence_id,status,created_at,updated_at,version,deleted_at,origin_device_id \
         FROM calendar_events WHERE id=?1",
        [id],
        |row| {
            Ok(CalendarEventSyncSource {
                id: row.get(0)?, calendar_id: row.get(1)?, title: row.get(2)?,
                description: row.get(3)?, location: row.get(4)?, starts_at: row.get(5)?,
                ends_at: row.get(6)?, timezone: row.get(7)?, all_day: row.get(8)?,
                recurrence_rule: row.get(9)?, recurrence_id: row.get(10)?, status: row.get(11)?,
                created_at: row.get(12)?, updated_at: row.get(13)?, version: row.get(14)?,
                deleted_at: row.get(15)?, origin_device_id: row.get(16)?,
            })
        },
    ).optional().map_err(Into::into)
}

fn get_exception_sync_source(
    conn: &Connection,
    event_id: &str,
    original_occurrence: &str,
) -> AppResult<Option<EventExceptionSyncSource>> {
    conn.query_row(
        "SELECT event_id,original_occurrence,replacement_event_id,is_cancelled,created_at,updated_at, \
                version,deleted_at,origin_device_id \
         FROM event_exceptions WHERE event_id=?1 AND original_occurrence=?2",
        params![event_id, original_occurrence],
        |row| {
            let event_id: String = row.get(0)?;
            let original_occurrence: String = row.get(1)?;
            Ok(EventExceptionSyncSource {
                id: event_exception_entity_id(&event_id, &original_occurrence),
                event_id,
                original_occurrence,
                replacement_event_id: row.get(2)?,
                is_cancelled: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
                version: row.get(6)?,
                deleted_at: row.get(7)?,
                origin_device_id: row.get(8)?,
            })
        },
    ).optional().map_err(Into::into)
}

fn get_task_list_sync_source(conn: &Connection, id: &str) -> AppResult<Option<TaskListSyncSource>> {
    conn.query_row(
        "SELECT id,name,color,created_at,updated_at,version,deleted_at,origin_device_id \
         FROM task_lists WHERE id=?1",
        [id],
        |row| {
            Ok(TaskListSyncSource {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                version: row.get(5)?,
                deleted_at: row.get(6)?,
                origin_device_id: row.get(7)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn get_task_sync_source(conn: &Connection, id: &str) -> AppResult<Option<TaskSyncSource>> {
    conn.query_row(
        "SELECT t.id,t.task_list_id,COALESCE(p.parent_id,t.parent_id),t.title,t.description,t.status, \
                t.priority,t.starts_at,t.due_date,t.due_at,t.due_timezone,t.is_important,t.my_day_date, \
                t.completed_at,t.recurrence_rule,t.recurrence_anchor,t.recurrence_source_id,t.sort_order, \
                t.created_at,t.updated_at,t.version,t.deleted_at,t.origin_device_id \
         FROM tasks t LEFT JOIN task_sync_parents p ON p.task_id=t.id WHERE t.id=?1",
        [id],
        |row| {
            Ok(TaskSyncSource {
                id: row.get(0)?, task_list_id: row.get(1)?, parent_id: row.get(2)?,
                title: row.get(3)?, description: row.get(4)?, status: row.get(5)?, priority: row.get(6)?,
                starts_at: row.get(7)?, due_date: row.get(8)?, due_at: row.get(9)?,
                due_timezone: row.get(10)?, is_important: row.get(11)?, my_day_date: row.get(12)?,
                completed_at: row.get(13)?, recurrence_rule: row.get(14)?,
                recurrence_anchor: row.get(15)?, recurrence_source_id: row.get(16)?,
                sort_order: row.get(17)?, created_at: row.get(18)?, updated_at: row.get(19)?,
                version: row.get(20)?, deleted_at: row.get(21)?, origin_device_id: row.get(22)?,
            })
        },
    ).optional().map_err(Into::into)
}

fn enqueue_calendar(conn: &Connection, id: &str) -> AppResult<()> {
    let source = get_calendar_sync_source(conn, id)?.ok_or_else(|| {
        AppError::Other(format!("calendar `{id}` disappeared during sync enqueue"))
    })?;
    let payload = serde_json::to_value(&source)?;
    super::sync::enqueue_projection(
        conn,
        crate::sync::payload::SyncEntityType::Calendar,
        &source.id,
        source.version,
        source.deleted_at.is_some(),
        &payload,
    )?;
    Ok(())
}

fn enqueue_event(conn: &Connection, id: &str) -> AppResult<()> {
    let source = get_event_sync_source(conn, id)?.ok_or_else(|| {
        AppError::Other(format!(
            "calendar event `{id}` disappeared during sync enqueue"
        ))
    })?;
    let payload = serde_json::to_value(&source)?;
    super::sync::enqueue_projection(
        conn,
        crate::sync::payload::SyncEntityType::CalendarEvent,
        &source.id,
        source.version,
        source.deleted_at.is_some(),
        &payload,
    )?;
    Ok(())
}

fn enqueue_exception(
    conn: &Connection,
    event_id: &str,
    original_occurrence: &str,
) -> AppResult<()> {
    let source = get_exception_sync_source(conn, event_id, original_occurrence)?.ok_or_else(|| {
        AppError::Other(format!("event exception `{event_id}` `{original_occurrence}` disappeared during sync enqueue"))
    })?;
    let payload = serde_json::to_value(&source)?;
    super::sync::enqueue_projection(
        conn,
        crate::sync::payload::SyncEntityType::EventException,
        &source.id,
        source.version,
        source.deleted_at.is_some(),
        &payload,
    )?;
    Ok(())
}

fn enqueue_task_list(conn: &Connection, id: &str) -> AppResult<()> {
    let source = get_task_list_sync_source(conn, id)?.ok_or_else(|| {
        AppError::Other(format!("task list `{id}` disappeared during sync enqueue"))
    })?;
    let payload = serde_json::to_value(&source)?;
    super::sync::enqueue_projection(
        conn,
        crate::sync::payload::SyncEntityType::TaskList,
        &source.id,
        source.version,
        source.deleted_at.is_some(),
        &payload,
    )?;
    Ok(())
}

fn enqueue_task(conn: &Connection, id: &str) -> AppResult<()> {
    let source = get_task_sync_source(conn, id)?
        .ok_or_else(|| AppError::Other(format!("task `{id}` disappeared during sync enqueue")))?;
    let payload = serde_json::to_value(&source)?;
    super::sync::enqueue_projection(
        conn,
        crate::sync::payload::SyncEntityType::Task,
        &source.id,
        source.version,
        source.deleted_at.is_some(),
        &payload,
    )?;
    Ok(())
}

fn set_task_sync_parent(
    conn: &Connection,
    task_id: &str,
    parent_id: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO task_sync_parents (task_id,parent_id) VALUES (?1,?2) \
         ON CONFLICT(task_id) DO UPDATE SET parent_id=excluded.parent_id",
        params![task_id, parent_id],
    )?;
    Ok(())
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
    conn: &mut Connection,
    id: &str,
    name: &str,
    color: Option<String>,
    timezone: &str,
) -> AppResult<()> {
    let timezone = parse_timezone(&required(timezone, "Timezone")?)?;
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    transaction.execute(
        "INSERT INTO calendars (id,name,color,timezone,created_at,updated_at,version,origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?5,1,?6)",
        params![
            id,
            required(name, "Calendar name")?,
            nullable(color),
            timezone.name(),
            timestamp,
            device_id,
        ],
    )?;
    enqueue_calendar(&transaction, id)?;
    transaction.commit()?;
    Ok(())
}
pub fn list_events(
    conn: &Connection,
    calendar_id: &str,
    range_start: &str,
    range_end: &str,
) -> AppResult<Vec<EventRow>> {
    let range_start = parse_instant(range_start, "Range start")?;
    let range_end = parse_instant(range_end, "Range end")?;
    if range_end <= range_start {
        return Err(AppError::Other("Range end must be after its start".into()));
    }
    let masters = {
        let mut statement = conn.prepare(
            "SELECT id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,recurrence_id,status \
             FROM calendar_events \
             WHERE calendar_id=?1 AND deleted_at IS NULL AND recurrence_id IS NULL \
             ORDER BY starts_at,id",
        )?;
        let rows = statement.query_map([calendar_id], stored_event_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let mut exceptions = load_exceptions(conn, calendar_id)?;
    let mut output = Vec::new();

    for master in masters {
        let (master_start, master_end, duration) = event_times(&master)?;
        let Some(_) = master.recurrence_rule else {
            if overlaps(master_start, master_end, range_start, range_end) {
                let mut row = master_row(&master);
                row.starts_at = format_instant(master_start);
                row.ends_at = format_instant(master_end);
                output.push(row);
            }
            continue;
        };

        let recurrence_timezone = RRuleTz::from(parse_timezone(&master.timezone)?);
        let result = recurrence_set(&master)?
            .after((range_start - duration).with_timezone(&recurrence_timezone))
            .before(range_end.with_timezone(&recurrence_timezone))
            .all(MAX_OCCURRENCES_PER_SERIES + 1);
        if result.limited || result.dates.len() > usize::from(MAX_OCCURRENCES_PER_SERIES) {
            return Err(AppError::Other(format!(
                "Recurring event {} produces too many occurrences for this range",
                master.id
            )));
        }

        let series_exceptions = exceptions.remove(&master.id).unwrap_or_default();
        let mut seen = HashSet::new();
        for occurrence in result.dates {
            let starts_at = occurrence.with_timezone(&Utc);
            let ends_at = starts_at + duration;
            let original_occurrence = format_instant(starts_at);
            seen.insert(original_occurrence.clone());
            match series_exceptions.get(&original_occurrence) {
                Some(exception) if exception.cancelled => {}
                Some(exception) => {
                    if let Some(replacement) = exception.replacement.as_ref() {
                        let (replacement_start, replacement_end, _) = event_times(replacement)?;
                        if overlaps(replacement_start, replacement_end, range_start, range_end) {
                            output.push(occurrence_row(
                                &master,
                                &original_occurrence,
                                starts_at,
                                ends_at,
                                Some(replacement),
                            )?);
                        }
                    }
                }
                None if overlaps(starts_at, ends_at, range_start, range_end) => {
                    output.push(occurrence_row(
                        &master,
                        &original_occurrence,
                        starts_at,
                        ends_at,
                        None,
                    )?);
                }
                None => {}
            }
        }

        for (original_occurrence, exception) in series_exceptions {
            if seen.contains(&original_occurrence) || exception.cancelled {
                continue;
            }
            let Some(replacement) = exception.replacement.as_ref() else {
                continue;
            };
            let (replacement_start, replacement_end, _) = event_times(replacement)?;
            if overlaps(replacement_start, replacement_end, range_start, range_end) {
                let (_, generated_start, generated_end) =
                    generated_occurrence(&master, &original_occurrence)?;
                output.push(occurrence_row(
                    &master,
                    &original_occurrence,
                    generated_start,
                    generated_end,
                    Some(replacement),
                )?);
            }
        }
    }

    output.sort_by(|left, right| {
        left.starts_at
            .cmp(&right.starts_at)
            .then_with(|| left.occurrence_id.cmp(&right.occurrence_id))
    });
    Ok(output)
}
pub fn create_event(
    conn: &mut Connection,
    id: &str,
    calendar_id: &str,
    title: &str,
    starts_at: &str,
    ends_at: &str,
    timezone: &str,
    all_day: bool,
    recurrence_rule: Option<String>,
) -> AppResult<()> {
    let starts_at = parse_instant(&required(starts_at, "Event start")?, "Event start")?;
    let ends_at = parse_instant(&required(ends_at, "Event end")?, "Event end")?;
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    let timezone = parse_timezone(&required(timezone, "Timezone")?)?;
    let recurrence_rule = recurrence(recurrence_rule, starts_at, timezone)?;
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    transaction.execute(
        "INSERT INTO calendar_events \
         (id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,created_at,updated_at,version,origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,1,?10)",
        params![
            id,
            calendar_id,
            required(title, "Event title")?,
            format_instant(starts_at),
            format_instant(ends_at),
            timezone.name(),
            all_day as i64,
            recurrence_rule,
            timestamp,
            device_id,
        ],
    )?;
    enqueue_event(&transaction, id)?;
    transaction.commit()?;
    Ok(())
}

pub fn update_event(conn: &mut Connection, id: &str, changes: EventUpdate) -> AppResult<EventRow> {
    if changes.is_empty() {
        return Err(AppError::Other(
            "Event update must change at least one field".into(),
        ));
    }
    let current = get_event(conn, id)?;
    if current.recurrence_id.is_some() {
        return Err(AppError::Other(
            "Use the occurrence exception API to update a replacement event".into(),
        ));
    }
    let title = changes
        .title
        .map(|value| required(&value, "Event title"))
        .transpose()?
        .unwrap_or_else(|| current.title.clone());
    let current_starts_at = parse_instant(&current.starts_at, "Event start")?;
    let starts_at = changes
        .starts_at
        .map(|value| parse_instant(&value, "Event start"))
        .transpose()?
        .unwrap_or(current_starts_at);
    let ends_at = changes
        .ends_at
        .map(|value| parse_instant(&value, "Event end"))
        .transpose()?
        .unwrap_or(parse_instant(&current.ends_at, "Event end")?);
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    let current_timezone = parse_timezone(&current.timezone)?;
    let timezone = changes
        .timezone
        .map(|value| parse_timezone(&value))
        .transpose()?
        .unwrap_or(current_timezone);
    let all_day = changes.all_day.unwrap_or(current.all_day);
    let recurrence_rule = match changes.recurrence_rule {
        Some(value) => recurrence(value, starts_at, timezone)?,
        None => recurrence(current.recurrence_rule.clone(), starts_at, timezone)?,
    };
    let schedule_changed = starts_at != current_starts_at
        || timezone != current_timezone
        || recurrence_rule != current.recurrence_rule;
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let cleared = if schedule_changed {
        let mut statement = transaction.prepare(
            "SELECT original_occurrence,replacement_event_id FROM event_exceptions \
             WHERE event_id=?1 AND deleted_at IS NULL",
        )?;
        let rows = statement
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    } else {
        Vec::new()
    };
    transaction.execute(
        "UPDATE calendar_events SET title=?1,starts_at=?2,ends_at=?3,timezone=?4,all_day=?5,recurrence_rule=?6,updated_at=?7,version=version+1,origin_device_id=?8 WHERE id=?9 AND deleted_at IS NULL",
        params![title, format_instant(starts_at), format_instant(ends_at), timezone.name(), all_day as i64, recurrence_rule, timestamp, device_id, id],
    )?;
    if schedule_changed {
        transaction.execute(
            "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
             WHERE id IN (SELECT replacement_event_id FROM event_exceptions \
                          WHERE event_id=?3 AND replacement_event_id IS NOT NULL) AND deleted_at IS NULL",
            params![timestamp, device_id, id],
        )?;
        transaction.execute(
            "UPDATE event_exceptions SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
             WHERE event_id=?3 AND deleted_at IS NULL",
            params![timestamp, device_id, id],
        )?;
        for (original_occurrence, replacement_id) in &cleared {
            if let Some(replacement_id) = replacement_id {
                enqueue_event(&transaction, replacement_id)?;
            }
            enqueue_exception(&transaction, id, original_occurrence)?;
        }
    }
    enqueue_event(&transaction, id)?;
    transaction.commit()?;
    Ok(master_row(&StoredEvent {
        id: id.to_string(),
        calendar_id: current.calendar_id,
        title,
        starts_at: format_instant(starts_at),
        ends_at: format_instant(ends_at),
        timezone: timezone.name().to_string(),
        all_day,
        recurrence_rule,
        recurrence_id: None,
        status: current.status,
    }))
}

pub fn update_occurrence(
    conn: &mut Connection,
    replacement_id: &str,
    event_id: &str,
    original_occurrence: &str,
    changes: OccurrenceUpdate,
) -> AppResult<EventRow> {
    if changes.is_empty() {
        return Err(AppError::Other(
            "Occurrence update must change at least one field".into(),
        ));
    }
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let master = get_event(&transaction, event_id)?;
    let (original_occurrence, generated_start, generated_end) =
        generated_occurrence(&master, original_occurrence)?;
    let existing_replacement_id = transaction
        .query_row(
            "SELECT replacement_event_id FROM event_exceptions \
             WHERE event_id=?1 AND original_occurrence=?2 AND deleted_at IS NULL",
            params![event_id, original_occurrence],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let existing = existing_replacement_id
        .as_deref()
        .map(|id| {
            transaction
                .query_row(
                    "SELECT id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,recurrence_id,status \
                     FROM calendar_events WHERE id=?1",
                    [id],
                    stored_event_from_row,
                )
                .optional()
        })
        .transpose()?
        .flatten();
    let replacement_id = existing_replacement_id.as_deref().unwrap_or(replacement_id);
    let title = changes
        .title
        .map(|value| required(&value, "Event title"))
        .transpose()?
        .unwrap_or_else(|| {
            existing
                .as_ref()
                .map(|event| event.title.clone())
                .unwrap_or_else(|| master.title.clone())
        });
    let default_start = match existing.as_ref() {
        Some(event) => parse_instant(&event.starts_at, "Event start")?,
        None => generated_start,
    };
    let starts_at = changes
        .starts_at
        .map(|value| parse_instant(&value, "Event start"))
        .transpose()?
        .unwrap_or(default_start);
    let default_end = match existing.as_ref() {
        Some(event) => parse_instant(&event.ends_at, "Event end")?,
        None => generated_end,
    };
    let ends_at = changes
        .ends_at
        .map(|value| parse_instant(&value, "Event end"))
        .transpose()?
        .unwrap_or(default_end);
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    let timezone = changes
        .timezone
        .map(|value| parse_timezone(&value))
        .transpose()?
        .unwrap_or(parse_timezone(
            existing
                .as_ref()
                .map(|event| event.timezone.as_str())
                .unwrap_or(master.timezone.as_str()),
        )?);
    let all_day = changes.all_day.unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|event| event.all_day)
            .unwrap_or(master.all_day)
    });
    let status = existing
        .as_ref()
        .map(|event| event.status.as_str())
        .unwrap_or(master.status.as_str());
    let timestamp = now();
    transaction.execute(
        "INSERT INTO calendar_events \
         (id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,recurrence_id,status,created_at,updated_at,deleted_at,version,origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8,?9,?10,?10,NULL,1,?11) \
         ON CONFLICT(id) DO UPDATE SET \
           calendar_id=excluded.calendar_id,title=excluded.title,starts_at=excluded.starts_at, \
           ends_at=excluded.ends_at,timezone=excluded.timezone,all_day=excluded.all_day, \
           recurrence_rule=NULL,recurrence_id=excluded.recurrence_id,status=excluded.status, \
           updated_at=excluded.updated_at,deleted_at=NULL,version=calendar_events.version+1,origin_device_id=excluded.origin_device_id",
        params![
            replacement_id,
            master.calendar_id,
            title,
            format_instant(starts_at),
            format_instant(ends_at),
            timezone.name(),
            all_day as i64,
            original_occurrence,
            status,
            timestamp,
            device_id,
        ],
    )?;
    transaction.execute(
        "INSERT INTO event_exceptions \
         (event_id,original_occurrence,replacement_event_id,is_cancelled,created_at,updated_at,version,deleted_at,origin_device_id) \
         VALUES (?1,?2,?3,0,?4,?4,1,NULL,?5) \
         ON CONFLICT(event_id,original_occurrence) DO UPDATE SET \
           replacement_event_id=excluded.replacement_event_id,is_cancelled=0,updated_at=excluded.updated_at, \
           deleted_at=NULL,version=event_exceptions.version+1,origin_device_id=excluded.origin_device_id",
        params![event_id, original_occurrence, replacement_id, timestamp, device_id],
    )?;
    enqueue_event(&transaction, replacement_id)?;
    enqueue_exception(&transaction, event_id, &original_occurrence)?;
    transaction.commit()?;
    occurrence_row(
        &master,
        &original_occurrence,
        generated_start,
        generated_end,
        Some(&StoredEvent {
            id: replacement_id.to_string(),
            calendar_id: master.calendar_id.clone(),
            title,
            starts_at: format_instant(starts_at),
            ends_at: format_instant(ends_at),
            timezone: timezone.name().to_string(),
            all_day,
            recurrence_rule: None,
            recurrence_id: Some(original_occurrence.clone()),
            status: status.to_string(),
        }),
    )
}

pub fn cancel_occurrence(
    conn: &mut Connection,
    event_id: &str,
    original_occurrence: &str,
) -> AppResult<()> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let master = get_event(&transaction, event_id)?;
    let (original_occurrence, _, _) = generated_occurrence(&master, original_occurrence)?;
    let replacement_id = transaction
        .query_row(
            "SELECT replacement_event_id FROM event_exceptions WHERE event_id=?1 AND original_occurrence=?2 AND deleted_at IS NULL",
            params![event_id, original_occurrence],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let timestamp = now();
    if let Some(replacement_id) = &replacement_id {
        transaction.execute(
            "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
             WHERE id=?3 AND deleted_at IS NULL",
            params![timestamp, device_id, replacement_id],
        )?;
        enqueue_event(&transaction, replacement_id)?;
    }
    transaction.execute(
        "INSERT INTO event_exceptions \
         (event_id,original_occurrence,replacement_event_id,is_cancelled,created_at,updated_at,version,deleted_at,origin_device_id) \
         VALUES (?1,?2,NULL,1,?3,?3,1,NULL,?4) \
         ON CONFLICT(event_id,original_occurrence) DO UPDATE SET \
           replacement_event_id=NULL,is_cancelled=1,updated_at=excluded.updated_at,deleted_at=NULL, \
           version=event_exceptions.version+1,origin_device_id=excluded.origin_device_id",
        params![event_id, original_occurrence, timestamp, device_id],
    )?;
    enqueue_exception(&transaction, event_id, &original_occurrence)?;
    transaction.commit()?;
    Ok(())
}

pub fn restore_occurrence(
    conn: &mut Connection,
    event_id: &str,
    original_occurrence: &str,
) -> AppResult<()> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let master = get_event(&transaction, event_id)?;
    let (original_occurrence, _, _) = generated_occurrence(&master, original_occurrence)?;
    let replacement_id = transaction
        .query_row(
            "SELECT replacement_event_id FROM event_exceptions \
             WHERE event_id=?1 AND original_occurrence=?2",
            params![event_id, original_occurrence],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten();
    let timestamp = now();
    if let Some(replacement_id) = replacement_id {
        transaction.execute(
            "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
             WHERE id=?3 AND deleted_at IS NULL",
            params![timestamp, device_id, replacement_id],
        )?;
        enqueue_event(&transaction, &replacement_id)?;
    }
    let restored = transaction.execute(
        "UPDATE event_exceptions SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
         WHERE event_id=?3 AND original_occurrence=?4 AND deleted_at IS NULL",
        params![timestamp, device_id, event_id, original_occurrence],
    )?;
    if restored > 0 {
        enqueue_exception(&transaction, event_id, &original_occurrence)?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn delete_event(conn: &mut Connection, id: &str) -> AppResult<()> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let event = get_event(&transaction, id)?;
    if event.recurrence_id.is_some() {
        return Err(AppError::Other(
            "Delete occurrence exceptions through their recurring master event".into(),
        ));
    }
    let timestamp = now();
    let exceptions = {
        let mut statement = transaction.prepare(
            "SELECT original_occurrence,replacement_event_id FROM event_exceptions \
             WHERE event_id=?1 AND deleted_at IS NULL",
        )?;
        let rows = statement
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    transaction.execute(
        "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
         WHERE id IN (SELECT replacement_event_id FROM event_exceptions \
                      WHERE event_id=?3 AND replacement_event_id IS NOT NULL) \
           AND deleted_at IS NULL",
        params![timestamp, device_id, id],
    )?;
    transaction.execute(
        "UPDATE event_exceptions SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
         WHERE event_id=?3 AND deleted_at IS NULL",
        params![timestamp, device_id, id],
    )?;
    transaction.execute(
        "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
         WHERE id=?3 AND deleted_at IS NULL",
        params![timestamp, device_id, id],
    )?;
    for (original_occurrence, replacement_id) in &exceptions {
        if let Some(replacement_id) = replacement_id {
            enqueue_event(&transaction, replacement_id)?;
        }
        enqueue_exception(&transaction, id, original_occurrence)?;
    }
    enqueue_event(&transaction, id)?;
    transaction.commit()?;
    Ok(())
}

pub fn list_task_lists(conn: &Connection) -> AppResult<Vec<TaskListRow>> {
    let mut statement = conn.prepare(
        "SELECT id,name,color FROM task_lists \
         WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE,id",
    )?;
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
    conn: &mut Connection,
    id: &str,
    name: &str,
    color: Option<String>,
) -> AppResult<()> {
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    transaction.execute(
        "INSERT INTO task_lists (id,name,color,created_at,updated_at,version,origin_device_id) \
         VALUES (?1,?2,?3,?4,?4,1,?5)",
        params![
            id,
            required(name, "Task list name")?,
            nullable(color),
            timestamp,
            device_id,
        ],
    )?;
    enqueue_task_list(&transaction, id)?;
    transaction.commit()?;
    Ok(())
}

fn task_from_row(row: &Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        task_list_id: row.get(1)?,
        parent_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        status: row.get(5)?,
        priority: row.get(6)?,
        starts_at: row.get(7)?,
        due_date: row.get(8)?,
        due_at: row.get(9)?,
        due_timezone: row.get(10)?,
        is_important: row.get::<_, i64>(11)? != 0,
        my_day_date: row.get(12)?,
        completed_at: row.get(13)?,
        recurrence_rule: row.get(14)?,
        recurrence_anchor: row.get(15)?,
        recurrence_source_id: row.get(16)?,
        sort_order: row.get(17)?,
    })
}

const TASK_SELECT: &str =
    "SELECT t.id,t.task_list_id,COALESCE(p.parent_id,t.parent_id),t.title,t.description,t.status,t.priority,t.starts_at, \
            t.due_date,t.due_at,t.due_timezone,t.is_important,t.my_day_date,t.completed_at, \
            t.recurrence_rule,t.recurrence_anchor,t.recurrence_source_id,t.sort_order \
     FROM tasks t LEFT JOIN task_sync_parents p ON p.task_id=t.id";

fn get_task(conn: &Connection, id: &str) -> AppResult<TaskRow> {
    conn.query_row(
        &format!("{TASK_SELECT} WHERE id=?1 AND deleted_at IS NULL"),
        [id],
        task_from_row,
    )
    .map_err(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => AppError::Other("Task does not exist".into()),
        error => error.into(),
    })
}

pub fn list_tasks(conn: &Connection, task_list_id: &str) -> AppResult<Vec<TaskRow>> {
    let mut statement = conn.prepare(&format!(
        "{TASK_SELECT} WHERE task_list_id=?1 AND deleted_at IS NULL \
         ORDER BY status='completed',sort_order,due_date,due_at,id"
    ))?;
    let rows = statement.query_map([task_list_id], task_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn list_all_tasks(conn: &Connection) -> AppResult<Vec<TaskRow>> {
    let mut statement = conn.prepare(&format!(
        "{TASK_SELECT} WHERE deleted_at IS NULL \
         ORDER BY status='completed',sort_order,due_date,due_at,id"
    ))?;
    let rows = statement.query_map([], task_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn create_task(conn: &mut Connection, task: NewTask) -> AppResult<()> {
    if !(0..=4).contains(&task.priority) {
        return Err(AppError::Other(
            "Task priority must be between 0 and 4".into(),
        ));
    }
    let my_day_date = canonical_date(task.my_day_date, "My Day date")?;
    let schedule = normalize_task_schedule(
        task.due_date,
        task.due_at,
        task.due_timezone,
        task.recurrence_rule,
        None,
    )?;
    let task_id = task.id.clone();
    let parent_id = nullable(task.parent_id.clone());
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    transaction.execute(
        "INSERT INTO tasks \
         (id,task_list_id,parent_id,title,description,priority,due_date,due_at,due_timezone, \
          is_important,my_day_date,recurrence_rule,recurrence_anchor,sort_order,created_at,updated_at,version,origin_device_id) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15,1,?16)",
        params![
            task_id,
            task.task_list_id,
            parent_id,
            required(&task.title, "Task title")?,
            nullable(task.description),
            task.priority,
            schedule.due_date,
            schedule.due_at,
            schedule.due_timezone,
            task.is_important as i64,
            my_day_date,
            schedule.recurrence_rule,
            schedule.recurrence_anchor,
            task.sort_order,
            timestamp,
            device_id,
        ],
    )?;
    set_task_sync_parent(&transaction, &task_id, parent_id.as_deref())?;
    enqueue_task(&transaction, &task_id)?;
    transaction.commit()?;
    Ok(())
}

fn next_task_schedule(task: &TaskRow) -> AppResult<Option<(Option<String>, Option<String>)>> {
    let Some(recurrence_rule) = task.recurrence_rule.as_deref() else {
        return Ok(None);
    };
    let timezone = parse_timezone(task.due_timezone.as_deref().unwrap_or("UTC"))?;
    let is_date_only = task.due_date.is_some();
    let current = match (&task.due_date, &task.due_at) {
        (Some(date), None) => local_midnight(parse_local_date(date, "Task due date")?, timezone)?,
        (None, Some(instant)) => parse_instant(instant, "Task due time")?,
        _ => {
            return Err(AppError::Other(
                "Recurring task has an invalid due value".into(),
            ))
        }
    };
    let anchor = task.recurrence_anchor.as_deref().unwrap_or_else(|| {
        task.due_date
            .as_deref()
            .or(task.due_at.as_deref())
            .expect("validated recurring task due value")
    });
    let anchor = if is_date_only {
        local_midnight(
            parse_local_date(anchor, "Task recurrence anchor")?,
            timezone,
        )?
    } else {
        parse_instant(anchor, "Task recurrence anchor")?
    };
    let recurrence_timezone = RRuleTz::from(timezone);
    let next = build_recurrence_set(recurrence_rule, anchor, timezone)?
        .after((current + Duration::nanoseconds(1)).with_timezone(&recurrence_timezone))
        .all(1)
        .dates
        .into_iter()
        .next();
    Ok(next.map(|next| {
        if is_date_only {
            (
                Some(
                    next.with_timezone(&timezone)
                        .date_naive()
                        .format("%Y-%m-%d")
                        .to_string(),
                ),
                None,
            )
        } else {
            (None, Some(format_instant(next.with_timezone(&Utc))))
        }
    }))
}

pub fn complete_task(
    conn: &mut Connection,
    id: &str,
    completed: bool,
    _next_task_id: &str,
) -> AppResult<()> {
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let task = get_task(&transaction, id)?;
    if (task.status == "completed") == completed {
        return Ok(());
    }
    let timestamp = now();
    let completed_at = completed.then_some(timestamp.as_str());
    let changed = transaction.execute(
        "UPDATE tasks SET status=?1,completed_at=?2,updated_at=?3,version=version+1,origin_device_id=?4 \
         WHERE id=?5 AND deleted_at IS NULL",
        params![
            if completed { "completed" } else { "open" },
            completed_at,
            timestamp,
            device_id,
            id
        ],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Task does not exist".into()));
    }
    if completed {
        if let Some((due_date, due_at)) = next_task_schedule(&task)? {
            let next_task_id = recurring_task_id(&task.id, due_date.as_deref(), due_at.as_deref());
            let inserted = transaction.execute(
                "INSERT INTO tasks \
                 (id,task_list_id,parent_id,title,description,status,priority,starts_at,due_date,due_at, \
                  due_timezone,is_important,my_day_date,completed_at,recurrence_rule,recurrence_anchor, \
                  recurrence_source_id,sort_order,created_at,updated_at,version,origin_device_id) \
                 VALUES (?1,?2,?3,?4,?5,'open',?6,?7,?8,?9,?10,?11,NULL,NULL,?12,?13,?14,?15,?16,?16,1,?17) \
                 ON CONFLICT(id) DO UPDATE SET task_list_id=excluded.task_list_id,parent_id=excluded.parent_id, \
                   title=excluded.title,description=excluded.description,status='open',priority=excluded.priority, \
                   starts_at=excluded.starts_at,due_date=excluded.due_date,due_at=excluded.due_at, \
                   due_timezone=excluded.due_timezone,is_important=excluded.is_important,my_day_date=NULL, \
                   completed_at=NULL,recurrence_rule=excluded.recurrence_rule, \
                   recurrence_anchor=excluded.recurrence_anchor,recurrence_source_id=excluded.recurrence_source_id, \
                   sort_order=excluded.sort_order,updated_at=excluded.updated_at,version=tasks.version+1, \
                   deleted_at=NULL,origin_device_id=excluded.origin_device_id",
                params![
                    next_task_id,
                    task.task_list_id,
                    task.parent_id,
                    task.title,
                    task.description,
                    task.priority,
                    task.starts_at,
                    due_date,
                    due_at,
                    task.due_timezone,
                    task.is_important as i64,
                    task.recurrence_rule,
                    task.recurrence_anchor,
                    task.id,
                    task.sort_order,
                    timestamp,
                    device_id,
                ],
            )?;
            if inserted > 0 {
                set_task_sync_parent(&transaction, &next_task_id, task.parent_id.as_deref())?;
                enqueue_task(&transaction, &next_task_id)?;
            }
        }
    } else {
        let generated_ids = {
            let mut statement = transaction.prepare(
                "SELECT id FROM tasks WHERE recurrence_source_id=?1 AND status='open' \
                 AND version=1 AND deleted_at IS NULL",
            )?;
            let rows = statement
                .query_map([id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        transaction.execute(
            "UPDATE tasks SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
             WHERE recurrence_source_id=?3 AND status='open' AND version=1 AND deleted_at IS NULL",
            params![timestamp, device_id, id],
        )?;
        for generated_id in generated_ids {
            enqueue_task(&transaction, &generated_id)?;
        }
    }
    enqueue_task(&transaction, id)?;
    transaction.commit()?;
    Ok(())
}

pub fn update_task(conn: &mut Connection, id: &str, changes: TaskUpdate) -> AppResult<TaskRow> {
    if changes.is_empty() {
        return Err(AppError::Other(
            "Task update must change at least one field".into(),
        ));
    }
    let current = get_task(conn, id)?;
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
    let schedule_changed = changes.due_date.is_some()
        || changes.due_at.is_some()
        || changes.due_timezone.is_some()
        || changes.recurrence_rule.is_some();
    let schedule = normalize_task_schedule(
        changes.due_date.unwrap_or(current.due_date),
        changes.due_at.unwrap_or(current.due_at),
        changes.due_timezone.unwrap_or(current.due_timezone),
        changes.recurrence_rule.unwrap_or(current.recurrence_rule),
        (!schedule_changed)
            .then_some(current.recurrence_anchor)
            .flatten(),
    )?;
    let is_important = changes.is_important.unwrap_or(current.is_important);
    let my_day_date = match changes.my_day_date {
        Some(value) => canonical_date(value, "My Day date")?,
        None => current.my_day_date,
    };
    let sort_order = changes.sort_order.unwrap_or(current.sort_order);
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    transaction.execute(
        "UPDATE tasks SET title=?1,description=?2,priority=?3,due_date=?4,due_at=?5, \
         due_timezone=?6,is_important=?7,my_day_date=?8,recurrence_rule=?9, \
         recurrence_anchor=?10,sort_order=?11,updated_at=?12,version=version+1,origin_device_id=?13 \
         WHERE id=?14 AND deleted_at IS NULL",
        params![
            title,
            description,
            priority,
            schedule.due_date,
            schedule.due_at,
            schedule.due_timezone,
            is_important as i64,
            my_day_date,
            schedule.recurrence_rule,
            schedule.recurrence_anchor,
            sort_order,
            timestamp,
            device_id,
            id
        ],
    )?;
    enqueue_task(&transaction, id)?;
    transaction.commit()?;
    Ok(TaskRow {
        id: id.to_string(),
        task_list_id: current.task_list_id,
        parent_id: current.parent_id,
        title,
        description,
        status: current.status,
        priority,
        starts_at: current.starts_at,
        due_date: schedule.due_date,
        due_at: schedule.due_at,
        due_timezone: schedule.due_timezone,
        is_important,
        my_day_date,
        completed_at: current.completed_at,
        recurrence_rule: schedule.recurrence_rule,
        recurrence_anchor: schedule.recurrence_anchor,
        recurrence_source_id: current.recurrence_source_id,
        sort_order,
    })
}

pub fn delete_task(conn: &mut Connection, id: &str) -> AppResult<()> {
    let timestamp = now();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let device_id = super::sync::device_id(&transaction)?;
    let task_ids = {
        let mut statement = transaction
            .prepare("SELECT id FROM tasks WHERE (id=?1 OR parent_id=?1) AND deleted_at IS NULL")?;
        let rows = statement
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    let changed = transaction.execute(
        "UPDATE tasks SET deleted_at=?1,updated_at=?1,version=version+1,origin_device_id=?2 \
         WHERE (id=?3 OR parent_id=?3) AND deleted_at IS NULL",
        params![timestamp, device_id, id],
    )?;
    if changed == 0 {
        return Err(AppError::Other("Task does not exist".into()));
    }
    for task_id in task_ids {
        enqueue_task(&transaction, &task_id)?;
    }
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_task(
        conn: &mut Connection,
        id: &str,
        task_list_id: &str,
        due_date: Option<&str>,
        due_at: Option<&str>,
        recurrence_rule: Option<&str>,
    ) -> AppResult<()> {
        create_task(
            conn,
            NewTask {
                id: id.into(),
                task_list_id: task_list_id.into(),
                parent_id: None,
                title: format!("Task {id}"),
                description: None,
                priority: 0,
                due_date: due_date.map(str::to_string),
                due_at: due_at.map(str::to_string),
                due_timezone: Some("Asia/Shanghai".into()),
                is_important: false,
                my_day_date: None,
                recurrence_rule: recurrence_rule.map(str::to_string),
                sort_order: 0.0,
            },
        )
    }

    #[test]
    fn updates_events_and_tasks_without_overwriting_omitted_fields() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_calendar(&mut conn, "calendar-1", "Personal", None, "Asia/Shanghai").unwrap();
        create_event(
            &mut conn,
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
            &mut conn,
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
            &mut conn,
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

        create_task_list(&mut conn, "task-list-1", "Work", None).unwrap();
        create_task(
            &mut conn,
            NewTask {
                id: "task-1".into(),
                task_list_id: "task-list-1".into(),
                parent_id: None,
                title: "Draft proposal".into(),
                description: Some("Initial outline".into()),
                priority: 1,
                due_date: None,
                due_at: Some("2026-07-18T09:00:00Z".into()),
                due_timezone: Some("Asia/Shanghai".into()),
                is_important: false,
                my_day_date: None,
                recurrence_rule: None,
                sort_order: 0.0,
            },
        )
        .unwrap();

        let task = update_task(
            &mut conn,
            "task-1",
            TaskUpdate {
                title: None,
                description: Some(None),
                priority: Some(3),
                due_date: None,
                due_at: Some(Some("2026-07-19T09:00:00Z".into())),
                due_timezone: None,
                is_important: None,
                my_day_date: None,
                recurrence_rule: None,
                sort_order: None,
            },
        )
        .unwrap();
        assert_eq!(task.title, "Draft proposal");
        assert_eq!(task.description, None);
        assert_eq!(task.priority, 3);
        assert_eq!(task.due_at.as_deref(), Some("2026-07-19T09:00:00Z"));
    }

    #[test]
    fn completes_reopens_and_completes_a_task_again() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_task_list(&mut conn, "list-1", "Work", None).unwrap();
        create_test_task(&mut conn, "task-1", "list-1", None, None, None).unwrap();

        complete_task(&mut conn, "task-1", true, "unused-next-1").unwrap();
        complete_task(&mut conn, "task-1", false, "unused-next-2").unwrap();
        complete_task(&mut conn, "task-1", true, "unused-next-3").unwrap();

        let task = get_task(&conn, "task-1").unwrap();
        assert_eq!(task.status, "completed");
        assert!(task.completed_at.is_some());
        let updated_at: Option<String> = conn
            .query_row(
                "SELECT updated_at FROM tasks WHERE id='task-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(updated_at.is_some());
    }

    #[test]
    fn validates_task_dates_times_and_recurrence_requirements() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_task_list(&mut conn, "list-1", "Work", None).unwrap();

        assert!(create_task(
            &mut conn,
            NewTask {
                id: "invalid-schedule".into(),
                task_list_id: "list-1".into(),
                parent_id: None,
                title: "Invalid schedule".into(),
                description: None,
                priority: 0,
                due_date: Some("2026-07-18".into()),
                due_at: Some("2026-07-18T09:00:00Z".into()),
                due_timezone: Some("UTC".into()),
                is_important: false,
                my_day_date: None,
                recurrence_rule: None,
                sort_order: 0.0,
            },
        )
        .is_err());
        assert!(create_task(
            &mut conn,
            NewTask {
                id: "invalid-my-day".into(),
                task_list_id: "list-1".into(),
                parent_id: None,
                title: "Invalid My Day".into(),
                description: None,
                priority: 0,
                due_date: None,
                due_at: None,
                due_timezone: None,
                is_important: false,
                my_day_date: Some("18/07/2026".into()),
                recurrence_rule: None,
                sort_order: 0.0,
            },
        )
        .is_err());
        assert!(create_test_task(
            &mut conn,
            "invalid-recurrence",
            "list-1",
            None,
            None,
            Some("RRULE:FREQ=DAILY"),
        )
        .is_err());
    }

    #[test]
    fn creates_and_reconciles_recurring_task_instances() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_task_list(&mut conn, "list-1", "Work", None).unwrap();
        create_test_task(
            &mut conn,
            "task-1",
            "list-1",
            Some("2026-07-17"),
            None,
            Some("RRULE:FREQ=DAILY;COUNT=3"),
        )
        .unwrap();

        complete_task(&mut conn, "task-1", true, "task-2").unwrap();
        let generated_id = recurring_task_id("task-1", Some("2026-07-18"), None);
        let generated = get_task(&conn, &generated_id).unwrap();
        assert_eq!(generated.due_date.as_deref(), Some("2026-07-18"));
        assert_eq!(generated.recurrence_source_id.as_deref(), Some("task-1"));

        complete_task(&mut conn, "task-1", false, "unused-next").unwrap();
        assert!(get_task(&conn, &generated_id).is_err());

        complete_task(&mut conn, "task-1", true, "task-2-edited").unwrap();
        update_task(
            &mut conn,
            &generated_id,
            TaskUpdate {
                title: Some("Edited generated task".into()),
                description: None,
                priority: None,
                due_date: None,
                due_at: None,
                due_timezone: None,
                is_important: None,
                my_day_date: None,
                recurrence_rule: None,
                sort_order: None,
            },
        )
        .unwrap();
        complete_task(&mut conn, "task-1", false, "unused-next").unwrap();
        assert_eq!(
            get_task(&conn, &generated_id).unwrap().title,
            "Edited generated task"
        );
    }

    #[test]
    fn stops_recurring_tasks_at_count_and_lists_then_deletes_them() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_task_list(&mut conn, "list-1", "Work", None).unwrap();
        create_task_list(&mut conn, "list-2", "Personal", None).unwrap();
        create_test_task(
            &mut conn,
            "task-1",
            "list-1",
            Some("2026-07-17"),
            None,
            Some("RRULE:FREQ=DAILY;COUNT=3"),
        )
        .unwrap();
        create_test_task(&mut conn, "other-task", "list-2", None, None, None).unwrap();

        complete_task(&mut conn, "task-1", true, "task-2").unwrap();
        let task_2 = recurring_task_id("task-1", Some("2026-07-18"), None);
        complete_task(&mut conn, &task_2, true, "task-3").unwrap();
        let task_3 = recurring_task_id(&task_2, Some("2026-07-19"), None);
        complete_task(&mut conn, &task_3, true, "task-after-count").unwrap();

        assert_eq!(list_all_tasks(&conn).unwrap().len(), 4);
        assert_eq!(list_all_tasks(&conn).unwrap().len(), 4);
        delete_task(&mut conn, "other-task").unwrap();
        assert_eq!(list_all_tasks(&conn).unwrap().len(), 3);
    }

    #[test]
    fn expands_recurring_events_in_their_iana_timezone() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_calendar(
            &mut conn,
            "calendar-1",
            "Personal",
            None,
            "America/New_York",
        )
        .unwrap();
        create_event(
            &mut conn,
            "event-1",
            "calendar-1",
            "Daily standup",
            "2026-03-07T14:00:00Z",
            "2026-03-07T14:30:00Z",
            "America/New_York",
            false,
            Some("RRULE:FREQ=DAILY;COUNT=3".into()),
        )
        .unwrap();

        let events = list_events(
            &conn,
            "calendar-1",
            "2026-03-07T00:00:00Z",
            "2026-03-10T00:00:00Z",
        )
        .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].starts_at, "2026-03-07T14:00:00Z");
        assert_eq!(events[1].starts_at, "2026-03-08T13:00:00Z");
        assert_eq!(events[2].starts_at, "2026-03-09T13:00:00Z");
        assert_eq!(events[1].occurrence_id, "event-1@2026-03-08T13:00:00Z");
        assert_eq!(
            events[1].original_occurrence.as_deref(),
            Some("2026-03-08T13:00:00Z")
        );
        assert!(!events[1].is_exception);
    }

    #[test]
    fn applies_moves_cancellations_and_restores_to_single_occurrences() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        assert!(create_calendar(
            &mut conn,
            "invalid-calendar",
            "Invalid",
            None,
            "Mars/Olympus_Mons",
        )
        .is_err());
        create_calendar(&mut conn, "calendar-1", "Work", None, "UTC").unwrap();
        create_event(
            &mut conn,
            "event-1",
            "calendar-1",
            "Focus time",
            "2026-07-17T09:00:00Z",
            "2026-07-17T10:00:00Z",
            "UTC",
            false,
            Some("RRULE:FREQ=DAILY;COUNT=4".into()),
        )
        .unwrap();

        let moved = update_occurrence(
            &mut conn,
            "replacement-1",
            "event-1",
            "2026-07-18T09:00:00Z",
            OccurrenceUpdate {
                title: Some("Moved focus time".into()),
                starts_at: Some("2026-08-01T12:00:00Z".into()),
                ends_at: Some("2026-08-01T13:00:00Z".into()),
                timezone: None,
                all_day: None,
            },
        )
        .unwrap();
        assert!(moved.is_exception);
        assert_eq!(moved.id, "event-1");
        assert_eq!(moved.starts_at, "2026-08-01T12:00:00Z");
        let renamed = update_occurrence(
            &mut conn,
            "unused-replacement-id",
            "event-1",
            "2026-07-18T09:00:00Z",
            OccurrenceUpdate {
                title: Some("Renamed focus time".into()),
                starts_at: None,
                ends_at: None,
                timezone: None,
                all_day: None,
            },
        )
        .unwrap();
        assert_eq!(renamed.title, "Renamed focus time");
        assert_eq!(renamed.starts_at, "2026-08-01T12:00:00Z");
        assert_eq!(renamed.ends_at, "2026-08-01T13:00:00Z");

        cancel_occurrence(&mut conn, "event-1", "2026-07-19T09:00:00Z").unwrap();
        let july = list_events(
            &conn,
            "calendar-1",
            "2026-07-17T00:00:00Z",
            "2026-07-22T00:00:00Z",
        )
        .unwrap();
        assert_eq!(
            july.iter()
                .map(|event| event.starts_at.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-07-17T09:00:00Z", "2026-07-20T09:00:00Z"]
        );

        let august = list_events(
            &conn,
            "calendar-1",
            "2026-08-01T00:00:00Z",
            "2026-08-02T00:00:00Z",
        )
        .unwrap();
        assert_eq!(august.len(), 1);
        assert_eq!(august[0].title, "Renamed focus time");
        assert_eq!(
            august[0].original_occurrence.as_deref(),
            Some("2026-07-18T09:00:00Z")
        );

        restore_occurrence(&mut conn, "event-1", "2026-07-18T09:00:00Z").unwrap();
        restore_occurrence(&mut conn, "event-1", "2026-07-19T09:00:00Z").unwrap();
        let restored = list_events(
            &conn,
            "calendar-1",
            "2026-07-17T00:00:00Z",
            "2026-07-22T00:00:00Z",
        )
        .unwrap();
        assert_eq!(restored.len(), 4);
        assert!(restored.iter().all(|event| !event.is_exception));
        let replacement_deleted: bool = conn
            .query_row(
                "SELECT deleted_at IS NOT NULL FROM calendar_events WHERE id='replacement-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(replacement_deleted);
    }

    #[test]
    fn rejects_invalid_rules_and_clears_stale_exceptions_when_schedule_changes() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_calendar(&mut conn, "calendar-1", "Work", None, "UTC").unwrap();
        assert!(create_event(
            &mut conn,
            "invalid-event",
            "calendar-1",
            "Invalid",
            "2026-07-17T09:00:00Z",
            "2026-07-17T10:00:00Z",
            "UTC",
            false,
            Some("RRULE:FREQ=NOT_REAL".into()),
        )
        .is_err());
        create_event(
            &mut conn,
            "event-1",
            "calendar-1",
            "Focus time",
            "2026-07-17T09:00:00Z",
            "2026-07-17T10:00:00Z",
            "UTC",
            false,
            Some("RRULE:FREQ=DAILY;COUNT=3".into()),
        )
        .unwrap();
        cancel_occurrence(&mut conn, "event-1", "2026-07-18T09:00:00Z").unwrap();

        update_event(
            &mut conn,
            "event-1",
            EventUpdate {
                title: Some("Renamed focus time".into()),
                starts_at: Some("2026-07-17T09:00:00Z".into()),
                ends_at: None,
                timezone: Some("UTC".into()),
                all_day: None,
                recurrence_rule: Some(Some("RRULE:FREQ=DAILY;COUNT=3".into())),
            },
        )
        .unwrap();
        let preserved_exception_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_exceptions WHERE event_id='event-1' AND deleted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_exception_count, 1);

        update_event(
            &mut conn,
            "event-1",
            EventUpdate {
                title: None,
                starts_at: Some("2026-07-17T10:00:00Z".into()),
                ends_at: Some("2026-07-17T11:00:00Z".into()),
                timezone: None,
                all_day: None,
                recurrence_rule: None,
            },
        )
        .unwrap();
        let exception_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM event_exceptions WHERE event_id='event-1' AND deleted_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exception_count, 0);
        assert!(list_events(
            &conn,
            "calendar-1",
            "2026-07-20T00:00:00Z",
            "2026-07-19T00:00:00Z",
        )
        .is_err());

        create_event(
            &mut conn,
            "dense-event",
            "calendar-1",
            "Heartbeat",
            "2026-07-17T09:00:00Z",
            "2026-07-17T09:00:01Z",
            "UTC",
            false,
            Some("RRULE:FREQ=SECONDLY".into()),
        )
        .unwrap();
        assert!(list_events(
            &conn,
            "calendar-1",
            "2026-07-17T09:00:00Z",
            "2026-07-17T11:00:00Z",
        )
        .is_err());

        delete_event(&mut conn, "event-1").unwrap();
        assert!(get_event(&conn, "event-1").is_err());
    }

    #[test]
    fn planner_writes_enqueue_versioned_entities_but_never_notifications() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_calendar(&mut conn, "calendar-1", "Work", None, "UTC").unwrap();
        create_event(
            &mut conn,
            "event-1",
            "calendar-1",
            "Planning",
            "2026-07-20T01:00:00Z",
            "2026-07-20T02:00:00Z",
            "UTC",
            false,
            Some("RRULE:FREQ=DAILY;COUNT=2".into()),
        )
        .unwrap();
        cancel_occurrence(&mut conn, "event-1", "2026-07-21T01:00:00Z").unwrap();
        create_task_list(&mut conn, "task-list-1", "Work", None).unwrap();
        create_test_task(
            &mut conn,
            "task-1",
            "task-list-1",
            Some("2026-07-20"),
            None,
            None,
        )
        .unwrap();
        complete_task(&mut conn, "task-1", true, "ignored").unwrap();

        let entities = {
            let mut statement = conn
                .prepare("SELECT DISTINCT entity_type FROM sync_outbox ORDER BY entity_type")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let notification_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            entities,
            vec![
                "calendar".to_string(),
                "calendar_event".to_string(),
                "event_exception".to_string(),
                "task".to_string(),
                "task_list".to_string(),
            ]
        );
        assert_eq!(notification_count, 0);
    }
}
