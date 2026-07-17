import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import FullCalendar from "@fullcalendar/react";
import type { DateClickArg } from "@fullcalendar/interaction";
import type { DateSelectArg, DatesSetArg, EventClickArg, EventInput } from "@fullcalendar/core";
import zhCnLocale from "@fullcalendar/core/locales/zh-cn";
import dayGridPlugin from "@fullcalendar/daygrid";
import timeGridPlugin from "@fullcalendar/timegrid";
import listPlugin from "@fullcalendar/list";
import interactionPlugin from "@fullcalendar/interaction";
import { invoke } from "@tauri-apps/api/core";
import { DateTime } from "luxon";
import {
  CalendarPlus2,
  CheckSquare2,
  ChevronLeft,
  ChevronRight,
  LoaderCircle,
} from "lucide-react";
import { CalendarFilters } from "./CalendarFilters";
import { DayAgenda } from "./DayAgenda";
import { EventEditorDialog } from "./EventEditorDialog";
import type {
  CalendarEvent,
  EventEditorValue,
  PlannerCalendar,
  Task,
} from "../shared/types";
import {
  exclusiveEndIsoToDateKey,
  isoToDateKey,
  isoToDateTimeInput,
  localTimezone,
  todayKey,
} from "../shared/time";

const fallbackColors = ["#4f8a6f", "#3b82a0", "#b7791f", "#9b5f86", "#c45d55", "#697386"];

interface CalendarWorkspaceProps {
  requestedEventId: string | null;
  onCloseRequestedEvent: () => void;
  onOpenTask: (taskId: string) => void;
}

interface EditorState {
  event: CalendarEvent | null;
  seriesEvent: CalendarEvent | null;
  defaultCalendarId: string;
  initialStart: string;
  initialEnd: string;
  initialAllDay: boolean;
}

const viewOptions = [
  { id: "dayGridMonth", label: "月" },
  { id: "timeGridWeek", label: "周" },
  { id: "timeGridDay", label: "日" },
  { id: "listWeek", label: "议程" },
] as const;

export function CalendarWorkspace({
  requestedEventId,
  onCloseRequestedEvent,
  onOpenTask,
}: CalendarWorkspaceProps) {
  const calendarRef = useRef<FullCalendar | null>(null);
  const initializedVisibility = useRef(false);
  const [calendars, setCalendars] = useState<PlannerCalendar[]>([]);
  const [calendarEvents, setCalendarEvents] = useState<CalendarEvent[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [visibleCalendarIds, setVisibleCalendarIds] = useState<Set<string>>(new Set());
  const [taskLayerVisible, setTaskLayerVisible] = useState(true);
  const [selectedDate, setSelectedDate] = useState(todayKey());
  const [range, setRange] = useState(() => ({
    start: DateTime.local().startOf("month").minus({ days: 7 }).toUTC().toISO() ?? "",
    end: DateTime.local().endOf("month").plus({ days: 7 }).toUTC().toISO() ?? "",
  }));
  const [viewTitle, setViewTitle] = useState("");
  const [viewType, setViewType] = useState("dayGridMonth");
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadContainers = useCallback(async () => {
    const [calendarRows, taskRows] = await Promise.all([
      invoke<PlannerCalendar[]>("list_calendars"),
      invoke<Task[]>("list_all_tasks"),
    ]);
    setCalendars(calendarRows);
    setTasks(taskRows);
    setVisibleCalendarIds((current) => {
      if (!initializedVisibility.current) {
        initializedVisibility.current = true;
        return new Set(calendarRows.map((calendar) => calendar.id));
      }
      return new Set([...current].filter((id) => calendarRows.some((calendar) => calendar.id === id)));
    });
  }, []);

  const loadEvents = useCallback(async () => {
    if (calendars.length === 0 || !range.start || !range.end) {
      setCalendarEvents([]);
      return;
    }
    const rows = await Promise.all(
      calendars.map((calendar) =>
        invoke<CalendarEvent[]>("list_calendar_events", {
          calendarId: calendar.id,
          rangeStart: range.start,
          rangeEnd: range.end,
        }),
      ),
    );
    setCalendarEvents(rows.flat());
  }, [calendars, range.end, range.start]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    loadContainers()
      .catch((reason) => active && setError(String(reason)))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [loadContainers]);

  useEffect(() => {
    let active = true;
    loadEvents().catch((reason) => active && setError(String(reason)));
    return () => {
      active = false;
    };
  }, [loadEvents]);

  const calendarColors = useMemo(
    () =>
      new Map(
        calendars.map((calendar, index) => [
          calendar.id,
          calendar.color || fallbackColors[index % fallbackColors.length],
        ]),
      ),
    [calendars],
  );

  const fullCalendarEvents = useMemo<EventInput[]>(() => {
    const events: EventInput[] = calendarEvents
      .filter((event) => visibleCalendarIds.has(event.calendar_id))
      .map((event) => ({
        id: `event:${event.occurrence_id}`,
        title: event.title,
        start: event.all_day ? isoToDateKey(event.starts_at, event.timezone) : event.starts_at,
        end: event.all_day ? isoToDateKey(event.ends_at, event.timezone) : event.ends_at,
        allDay: event.all_day,
        backgroundColor: calendarColors.get(event.calendar_id),
        borderColor: calendarColors.get(event.calendar_id),
        extendedProps: { kind: "event", source: event },
      }));
    if (taskLayerVisible) {
      events.push(
        ...tasks
          .filter((task) => task.status !== "cancelled" && (task.due_date || task.due_at))
          .map((task) => ({
            id: `task:${task.id}`,
            title: task.title,
            start: task.due_date || task.due_at || undefined,
            allDay: Boolean(task.due_date),
            backgroundColor: task.status === "completed" ? "#a8a29e" : "#5f6b76",
            borderColor: task.status === "completed" ? "#a8a29e" : "#5f6b76",
            classNames: task.status === "completed" ? ["planner-task-completed"] : ["planner-task-event"],
            extendedProps: { kind: "task", taskId: task.id },
          })),
      );
    }
    return events;
  }, [calendarColors, calendarEvents, taskLayerVisible, tasks, visibleCalendarIds]);

  const datesSet = (info: DatesSetArg) => {
    setViewTitle(info.view.title);
    setViewType(info.view.type);
    const start = info.start.toISOString();
    const end = info.end.toISOString();
    setRange((current) => (current.start === start && current.end === end ? current : { start, end }));
  };

  const openNewEvent = (start = selectedDate, end = selectedDate, allDay = true) => {
    const defaultCalendarId =
      calendars.find((calendar) => visibleCalendarIds.has(calendar.id))?.id || calendars[0]?.id;
    if (!defaultCalendarId) return;
    setEditor({
      event: null,
      seriesEvent: null,
      defaultCalendarId,
      initialStart: start,
      initialEnd: end,
      initialAllDay: allDay,
    });
  };

  const selectRange = (info: DateSelectArg) => {
    const selected = DateTime.fromJSDate(info.start).toISODate() || selectedDate;
    setSelectedDate(selected);
    if (info.allDay) {
      const inclusiveEnd = DateTime.fromJSDate(info.end).minus({ days: 1 }).toISODate() || selected;
      openNewEvent(selected, inclusiveEnd, true);
    } else {
      openNewEvent(
        isoToDateTimeInput(info.start.toISOString(), localTimezone),
        isoToDateTimeInput(info.end.toISOString(), localTimezone),
        false,
      );
    }
  };

  const clickDate = (info: DateClickArg) => {
    setSelectedDate(info.dateStr.slice(0, 10));
  };

  const openExistingEvent = async (event: CalendarEvent) => {
    try {
      setError(null);
      const seriesEvent = event.recurrence_rule
        ? await invoke<CalendarEvent>("get_calendar_event", { eventId: event.id })
        : event;
      setEditor({
        event,
        seriesEvent,
        defaultCalendarId: event.calendar_id,
        initialStart: event.all_day
          ? isoToDateKey(event.starts_at, event.timezone)
          : isoToDateTimeInput(event.starts_at, event.timezone),
        initialEnd: event.all_day
          ? exclusiveEndIsoToDateKey(event.ends_at, event.timezone)
          : isoToDateTimeInput(event.ends_at, event.timezone),
        initialAllDay: event.all_day,
      });
    } catch (reason) {
      setError(String(reason));
    }
  };

  useEffect(() => {
    if (!requestedEventId) return;
    let active = true;
    invoke<CalendarEvent>("get_calendar_event", { eventId: requestedEventId })
      .then(async (event) => {
        if (!active) return;
        calendarRef.current?.getApi().gotoDate(event.starts_at);
        setSelectedDate(isoToDateKey(event.starts_at, event.timezone));
        await openExistingEvent(event);
      })
      .catch((reason) => active && setError(String(reason)))
      .finally(() => active && onCloseRequestedEvent());
    return () => {
      active = false;
    };
  }, [requestedEventId, onCloseRequestedEvent]);

  const clickEvent = (info: EventClickArg) => {
    const props = info.event.extendedProps as {
      kind: "event" | "task";
      source?: CalendarEvent;
      taskId?: string;
    };
    if (props.kind === "task" && props.taskId) {
      onOpenTask(props.taskId);
      return;
    }
    if (props.source) void openExistingEvent(props.source);
  };

  const saveEvent = async (value: EventEditorValue) => {
    if (!editor) return;
    setBusy(true);
    setError(null);
    try {
      if (!editor.event) {
        await invoke("create_calendar_event", {
          calendarId: value.calendarId,
          title: value.title,
          startsAt: value.startsAt,
          endsAt: value.endsAt,
          timezone: value.timezone,
          allDay: value.allDay,
          recurrenceRule: value.recurrenceRule,
        });
      } else if (value.scope === "occurrence" && editor.event.original_occurrence) {
        await invoke("update_calendar_occurrence", {
          eventId: editor.event.id,
          originalOccurrence: editor.event.original_occurrence,
          changes: {
            title: value.title,
            startsAt: value.startsAt,
            endsAt: value.endsAt,
            timezone: value.timezone,
            allDay: value.allDay,
          },
        });
      } else {
        await invoke("update_calendar_event", {
          eventId: editor.event.id,
          changes: {
            title: value.title,
            startsAt: value.startsAt,
            endsAt: value.endsAt,
            timezone: value.timezone,
            allDay: value.allDay,
            recurrenceRule: value.recurrenceRule,
          },
        });
      }
      setEditor(null);
      await loadEvents();
    } catch (reason) {
      setError(String(reason));
      throw reason;
    } finally {
      setBusy(false);
    }
  };

  const deleteEvent = async (scope: "occurrence" | "series") => {
    if (!editor?.event) return;
    setBusy(true);
    try {
      if (scope === "occurrence" && editor.event.original_occurrence) {
        await invoke("cancel_calendar_occurrence", {
          eventId: editor.event.id,
          originalOccurrence: editor.event.original_occurrence,
        });
      } else {
        await invoke("delete_calendar_event", { eventId: editor.event.id });
      }
      setEditor(null);
      await loadEvents();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const restoreEvent = async () => {
    if (!editor?.event?.original_occurrence) return;
    setBusy(true);
    try {
      await invoke("restore_calendar_occurrence", {
        eventId: editor.event.id,
        originalOccurrence: editor.event.original_occurrence,
      });
      setEditor(null);
      await loadEvents();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const createCalendar = async (name: string, color: string) => {
    setBusy(true);
    try {
      const id = await invoke<string>("create_calendar", {
        name,
        color,
        timezone: localTimezone,
      });
      const rows = await invoke<PlannerCalendar[]>("list_calendars");
      setCalendars(rows);
      setVisibleCalendarIds((current) => new Set([...current, id]));
    } catch (reason) {
      setError(String(reason));
      throw reason;
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-h-0 flex-1">
      <CalendarFilters
        calendars={calendars}
        visibleIds={visibleCalendarIds}
        taskLayerVisible={taskLayerVisible}
        busy={busy}
        onToggleCalendar={(calendarId) =>
          setVisibleCalendarIds((current) => {
            const next = new Set(current);
            if (next.has(calendarId)) next.delete(calendarId);
            else next.add(calendarId);
            return next;
          })
        }
        onToggleTaskLayer={() => setTaskLayerVisible((visible) => !visible)}
        onCreateCalendar={createCalendar}
      />

      <section className="min-w-0 flex-1 overflow-y-auto bg-[#FAF9F5]">
        <div className="sticky top-0 z-20 flex min-h-14 items-center justify-between border-b border-stone-200 bg-[#FAF9F5]/95 px-5 backdrop-blur">
          <div className="flex min-w-0 items-center gap-2">
            <button
              type="button"
              onClick={() => calendarRef.current?.getApi().prev()}
              className="grid h-8 w-8 place-items-center rounded-md text-stone-600 hover:bg-stone-100"
              title="上一段时间"
            >
              <ChevronLeft className="h-4 w-4" />
            </button>
            <button
              type="button"
              onClick={() => calendarRef.current?.getApi().next()}
              className="grid h-8 w-8 place-items-center rounded-md text-stone-600 hover:bg-stone-100"
              title="下一段时间"
            >
              <ChevronRight className="h-4 w-4" />
            </button>
            <button
              type="button"
              onClick={() => calendarRef.current?.getApi().today()}
              className="h-8 rounded-md border border-stone-300 bg-white px-3 text-xs font-medium text-stone-700 hover:bg-stone-50"
            >
              今天
            </button>
            <h1 className="ml-2 truncate text-base font-semibold text-stone-900">{viewTitle}</h1>
          </div>
          <div className="flex items-center gap-3">
            <div className="flex rounded-md bg-stone-200/70 p-1">
              {viewOptions.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  onClick={() => calendarRef.current?.getApi().changeView(option.id)}
                  className={`h-7 min-w-10 rounded px-2 text-xs font-medium ${
                    viewType === option.id ? "bg-white text-stone-900 shadow-sm" : "text-stone-500"
                  }`}
                >
                  {option.label}
                </button>
              ))}
            </div>
            <button
              type="button"
              onClick={() => openNewEvent()}
              disabled={calendars.length === 0 || busy}
              className="flex h-9 items-center gap-2 rounded-md bg-[#4f7f68] px-3 text-xs font-medium text-white disabled:opacity-50"
            >
              <CalendarPlus2 className="h-4 w-4" />
              新建事件
            </button>
          </div>
        </div>

        {error && (
          <div className="mx-5 mt-4 flex items-center justify-between rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700">
            <span className="min-w-0 truncate">{error}</span>
            <button type="button" onClick={() => setError(null)} className="ml-3 font-medium">
              关闭
            </button>
          </div>
        )}

        <div className="relative px-5 py-4">
          {loading && (
            <div className="absolute inset-0 z-10 grid place-items-center bg-[#FAF9F5]/70">
              <LoaderCircle className="h-5 w-5 animate-spin text-stone-400" />
            </div>
          )}
          <FullCalendar
            ref={calendarRef}
            plugins={[dayGridPlugin, timeGridPlugin, listPlugin, interactionPlugin]}
            initialView="dayGridMonth"
            locale={zhCnLocale}
            headerToolbar={false}
            firstDay={1}
            height="auto"
            nowIndicator
            selectable
            selectMirror
            dayMaxEvents={4}
            eventDisplay="block"
            allDayText="全天"
            slotMinTime="06:00:00"
            slotMaxTime="23:00:00"
            events={fullCalendarEvents}
            datesSet={datesSet}
            dateClick={clickDate}
            select={selectRange}
            eventClick={clickEvent}
            dayCellClassNames={(argument) =>
              DateTime.fromJSDate(argument.date).toISODate() === selectedDate ? ["planner-selected-day"] : []
            }
            eventContent={(argument) => (
              <span className="flex min-w-0 items-center gap-1 px-0.5">
                {argument.event.extendedProps.kind === "task" && <CheckSquare2 className="h-3 w-3 shrink-0" />}
                <span className="truncate">{argument.timeText && `${argument.timeText} `}{argument.event.title}</span>
              </span>
            )}
          />
        </div>

        <DayAgenda
          date={selectedDate}
          events={calendarEvents}
          tasks={tasks}
          calendars={calendars}
          visibleCalendarIds={visibleCalendarIds}
          taskLayerVisible={taskLayerVisible}
          onOpenEvent={(event) => void openExistingEvent(event)}
          onOpenTask={onOpenTask}
        />
      </section>

      {editor && (
        <EventEditorDialog
          event={editor.event}
          seriesEvent={editor.seriesEvent}
          calendars={calendars}
          defaultCalendarId={editor.defaultCalendarId}
          initialStart={editor.initialStart}
          initialEnd={editor.initialEnd}
          initialAllDay={editor.initialAllDay}
          busy={busy}
          onClose={() => setEditor(null)}
          onSave={saveEvent}
          onDelete={deleteEvent}
          onRestore={restoreEvent}
        />
      )}
    </div>
  );
}
