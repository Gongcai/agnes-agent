import { CalendarDays, CheckSquare2, Clock3, Repeat2 } from "lucide-react";
import type { CalendarEvent, PlannerCalendar, Task } from "../shared/types";
import {
  eventOccursOnDate,
  formatDateKey,
  formatDateTime,
  isoToDateKey,
  localTimezone,
} from "../shared/time";

interface DayAgendaProps {
  date: string;
  events: CalendarEvent[];
  tasks: Task[];
  calendars: PlannerCalendar[];
  visibleCalendarIds: Set<string>;
  taskLayerVisible: boolean;
  onOpenEvent: (event: CalendarEvent) => void;
  onOpenTask: (taskId: string) => void;
}

export function DayAgenda({
  date,
  events,
  tasks,
  calendars,
  visibleCalendarIds,
  taskLayerVisible,
  onOpenEvent,
  onOpenTask,
}: DayAgendaProps) {
  const calendarById = new Map(calendars.map((calendar) => [calendar.id, calendar]));
  const dayEvents = events
    .filter(
      (event) =>
        visibleCalendarIds.has(event.calendar_id) &&
        eventOccursOnDate(event.starts_at, event.ends_at, event.all_day, date, localTimezone),
    )
    .sort((left, right) => Number(right.all_day) - Number(left.all_day) || left.starts_at.localeCompare(right.starts_at));
  const dayTasks = taskLayerVisible
    ? tasks
        .filter(
          (task) =>
            task.status !== "cancelled" &&
            (task.due_date === date ||
              (task.due_at && isoToDateKey(task.due_at, task.due_timezone || localTimezone) === date)),
        )
        .sort((left, right) => (left.due_at || left.due_date || "").localeCompare(right.due_at || right.due_date || ""))
    : [];

  return (
    <section className="border-t border-stone-200 bg-white/55 px-5 py-4">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="text-sm font-semibold text-stone-900">{formatDateKey(date)}</h2>
        <span className="text-[11px] text-stone-400">{dayEvents.length + dayTasks.length} 项安排</span>
      </div>
      <div className="divide-y divide-stone-100">
        {dayEvents.map((event) => {
          const calendar = calendarById.get(event.calendar_id);
          return (
            <button
              key={event.occurrence_id}
              type="button"
              onClick={() => onOpenEvent(event)}
              className="grid min-h-12 w-full grid-cols-[82px_12px_1fr_auto] items-center gap-3 py-2 text-left hover:bg-stone-50"
            >
              <span className="flex items-center gap-1 text-xs tabular-nums text-stone-500">
                {event.all_day ? (
                  <>
                    <CalendarDays className="h-3.5 w-3.5" /> 全天
                  </>
                ) : (
                  <>
                    <Clock3 className="h-3.5 w-3.5" />
                    {new Date(event.starts_at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
                  </>
                )}
              </span>
              <span
                className="h-2.5 w-2.5 rounded-full"
                style={{ backgroundColor: calendar?.color || "#4f8a6f" }}
              />
              <span className="min-w-0">
                <span className="flex items-center gap-2">
                  <span className="truncate text-sm font-medium text-stone-800">{event.title}</span>
                  {event.recurrence_rule && <Repeat2 className="h-3.5 w-3.5 shrink-0 text-stone-400" />}
                </span>
                <span className="mt-0.5 block text-[11px] text-stone-400">{calendar?.name ?? "日历"}</span>
              </span>
              {event.is_exception && <span className="text-[11px] text-amber-700">已调整</span>}
            </button>
          );
        })}
        {dayTasks.map((task) => (
          <button
            key={task.id}
            type="button"
            onClick={() => onOpenTask(task.id)}
            className="grid min-h-12 w-full grid-cols-[82px_12px_1fr_auto] items-center gap-3 py-2 text-left hover:bg-stone-50"
          >
            <span className="flex items-center gap-1 text-xs tabular-nums text-stone-500">
              {task.due_at ? (
                <>{formatDateTime(task.due_at, task.due_timezone || localTimezone).split(" ").at(-1)}</>
              ) : (
                "全天"
              )}
            </span>
            <CheckSquare2 className="h-3.5 w-3.5 text-stone-400" />
            <span
              className={`truncate text-sm font-medium ${
                task.status === "completed" ? "text-stone-400 line-through" : "text-stone-800"
              }`}
            >
              {task.title}
            </span>
            {task.recurrence_rule && <Repeat2 className="h-3.5 w-3.5 text-stone-400" />}
          </button>
        ))}
        {dayEvents.length === 0 && dayTasks.length === 0 && (
          <p className="py-7 text-center text-xs text-stone-400">这一天还没有安排</p>
        )}
      </div>
    </section>
  );
}
