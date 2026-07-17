//! Local calendar and task persistence. External providers remain adapters above this layer.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use chrono_tz::Tz as ChronoTz;
use rrule::{RRule, RRuleSet, Tz as RRuleTz, Unvalidated};
use rusqlite::{params, Connection, OptionalExtension, Row};
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
    let rule = recurrence(event.recurrence_rule.clone(), starts_at, timezone)?
        .ok_or_else(|| AppError::Other("Event is not recurring".into()))?;
    let body = rule
        .strip_prefix("RRULE:")
        .ok_or_else(|| AppError::Other("Stored recurrence rule is invalid".into()))?;
    let recurrence_timezone = RRuleTz::from(timezone);
    body.parse::<RRule<Unvalidated>>()
        .and_then(|rule| rule.build(starts_at.with_timezone(&recurrence_timezone)))
        .map_err(|error| AppError::Other(format!("Invalid recurrence rule: {error}")))
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
         WHERE master.calendar_id=?1 AND master.deleted_at IS NULL",
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
    let timezone = parse_timezone(&required(timezone, "Timezone")?)?;
    let timestamp = now();
    conn.execute(
        "INSERT INTO calendars (id,name,color,timezone,created_at,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?5)",
        params![
            id,
            required(name, "Calendar name")?,
            nullable(color),
            timezone.name(),
            timestamp
        ],
    )?;
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
    let starts_at = parse_instant(&required(starts_at, "Event start")?, "Event start")?;
    let ends_at = parse_instant(&required(ends_at, "Event end")?, "Event end")?;
    if ends_at <= starts_at {
        return Err(AppError::Other("Event end must be after its start".into()));
    }
    let timezone = parse_timezone(&required(timezone, "Timezone")?)?;
    let recurrence_rule = recurrence(recurrence_rule, starts_at, timezone)?;
    let timestamp = now();
    conn.execute(
        "INSERT INTO calendar_events \
         (id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,created_at,updated_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)",
        params![
            id,
            calendar_id,
            required(title, "Event title")?,
            format_instant(starts_at),
            format_instant(ends_at),
            timezone.name(),
            all_day as i64,
            recurrence_rule,
            timestamp
        ],
    )?;
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
    let transaction = conn.transaction()?;
    transaction.execute(
        "UPDATE calendar_events SET title=?1,starts_at=?2,ends_at=?3,timezone=?4,all_day=?5,recurrence_rule=?6,updated_at=?7,version=version+1 WHERE id=?8 AND deleted_at IS NULL",
        params![title, format_instant(starts_at), format_instant(ends_at), timezone.name(), all_day as i64, recurrence_rule, timestamp, id],
    )?;
    if schedule_changed {
        transaction.execute(
            "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1 \
             WHERE id IN (SELECT replacement_event_id FROM event_exceptions \
                          WHERE event_id=?2 AND replacement_event_id IS NOT NULL)",
            params![timestamp, id],
        )?;
        transaction.execute("DELETE FROM event_exceptions WHERE event_id=?1", [id])?;
    }
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
    let transaction = conn.transaction()?;
    let master = get_event(&transaction, event_id)?;
    let (original_occurrence, generated_start, generated_end) =
        generated_occurrence(&master, original_occurrence)?;
    let existing_replacement_id = transaction
        .query_row(
            "SELECT replacement_event_id FROM event_exceptions \
             WHERE event_id=?1 AND original_occurrence=?2",
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
         (id,calendar_id,title,starts_at,ends_at,timezone,all_day,recurrence_rule,recurrence_id,status,created_at,updated_at,deleted_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8,?9,?10,?10,NULL) \
         ON CONFLICT(id) DO UPDATE SET \
           calendar_id=excluded.calendar_id,title=excluded.title,starts_at=excluded.starts_at, \
           ends_at=excluded.ends_at,timezone=excluded.timezone,all_day=excluded.all_day, \
           recurrence_rule=NULL,recurrence_id=excluded.recurrence_id,status=excluded.status, \
           updated_at=excluded.updated_at,deleted_at=NULL,version=calendar_events.version+1",
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
            timestamp
        ],
    )?;
    transaction.execute(
        "INSERT INTO event_exceptions \
         (event_id,original_occurrence,replacement_event_id,is_cancelled,created_at,updated_at) \
         VALUES (?1,?2,?3,0,?4,?4) \
         ON CONFLICT(event_id,original_occurrence) DO UPDATE SET \
           replacement_event_id=excluded.replacement_event_id,is_cancelled=0,updated_at=excluded.updated_at",
        params![event_id, original_occurrence, replacement_id, timestamp],
    )?;
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
    let transaction = conn.transaction()?;
    let master = get_event(&transaction, event_id)?;
    let (original_occurrence, _, _) = generated_occurrence(&master, original_occurrence)?;
    let timestamp = now();
    transaction.execute(
        "INSERT INTO event_exceptions \
         (event_id,original_occurrence,replacement_event_id,is_cancelled,created_at,updated_at) \
         VALUES (?1,?2,NULL,1,?3,?3) \
         ON CONFLICT(event_id,original_occurrence) DO UPDATE SET \
           is_cancelled=1,updated_at=excluded.updated_at",
        params![event_id, original_occurrence, timestamp],
    )?;
    transaction.commit()?;
    Ok(())
}

pub fn restore_occurrence(
    conn: &mut Connection,
    event_id: &str,
    original_occurrence: &str,
) -> AppResult<()> {
    let transaction = conn.transaction()?;
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
            "UPDATE calendar_events SET deleted_at=?1,updated_at=?1,version=version+1 \
             WHERE id=?2 AND deleted_at IS NULL",
            params![timestamp, replacement_id],
        )?;
    }
    transaction.execute(
        "DELETE FROM event_exceptions WHERE event_id=?1 AND original_occurrence=?2",
        params![event_id, original_occurrence],
    )?;
    transaction.commit()?;
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

    #[test]
    fn expands_recurring_events_in_their_iana_timezone() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        create_calendar(&conn, "calendar-1", "Personal", None, "America/New_York").unwrap();
        create_event(
            &conn,
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
            &conn,
            "invalid-calendar",
            "Invalid",
            None,
            "Mars/Olympus_Mons",
        )
        .is_err());
        create_calendar(&conn, "calendar-1", "Work", None, "UTC").unwrap();
        create_event(
            &conn,
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
        create_calendar(&conn, "calendar-1", "Work", None, "UTC").unwrap();
        assert!(create_event(
            &conn,
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
            &conn,
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
                "SELECT COUNT(*) FROM event_exceptions WHERE event_id='event-1'",
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
                "SELECT COUNT(*) FROM event_exceptions WHERE event_id='event-1'",
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
            &conn,
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
    }
}
