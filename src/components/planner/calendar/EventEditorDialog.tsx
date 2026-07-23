import { useMemo, useState } from "react";
import { CalendarDays, Clock3, Repeat2, RotateCcw, Trash2, X } from "lucide-react";
import { useConfirmDialog } from "../../ConfirmDialog";
import type {
  CalendarEvent,
  EventEditorValue,
  PlannerCalendar,
  RepeatOption,
} from "../shared/types";
import {
  allDayDateToIso,
  commonTimezones,
  dateTimeInputToIso,
  exclusiveEndIsoToDateKey,
  inclusiveEndDateToIso,
  isoToDateKey,
  isoToDateTimeInput,
  localTimezone,
  repeatOptionFromRule,
  repeatRuleFromOption,
} from "../shared/time";

interface EventEditorDialogProps {
  event: CalendarEvent | null;
  seriesEvent: CalendarEvent | null;
  calendars: PlannerCalendar[];
  defaultCalendarId: string;
  initialStart: string;
  initialEnd: string;
  initialAllDay: boolean;
  busy: boolean;
  onClose: () => void;
  onSave: (value: EventEditorValue) => Promise<void>;
  onDelete: (scope: "occurrence" | "series") => Promise<void>;
  onRestore: () => Promise<void>;
}

function sourceValues(source: CalendarEvent) {
  const timezone = source.timezone || localTimezone;
  return {
    title: source.title,
    timezone,
    allDay: source.all_day,
    start: source.all_day
      ? isoToDateKey(source.starts_at, timezone)
      : isoToDateTimeInput(source.starts_at, timezone),
    end: source.all_day
      ? exclusiveEndIsoToDateKey(source.ends_at, timezone)
      : isoToDateTimeInput(source.ends_at, timezone),
    repeat: repeatOptionFromRule(source.recurrence_rule),
  };
}

export function EventEditorDialog({
  event,
  seriesEvent,
  calendars,
  defaultCalendarId,
  initialStart,
  initialEnd,
  initialAllDay,
  busy,
  onClose,
  onSave,
  onDelete,
  onRestore,
}: EventEditorDialogProps) {
  const confirmDelete = useConfirmDialog();
  const initialSource = event ? sourceValues(event) : null;
  const initialTimezone =
    initialSource?.timezone ||
    calendars.find((calendar) => calendar.id === defaultCalendarId)?.timezone ||
    localTimezone;
  const [title, setTitle] = useState(initialSource?.title ?? "");
  const [calendarId, setCalendarId] = useState(event?.calendar_id ?? defaultCalendarId);
  const [timezone, setTimezone] = useState(initialTimezone);
  const [allDay, setAllDay] = useState(initialSource?.allDay ?? initialAllDay);
  const [start, setStart] = useState(initialSource?.start ?? initialStart);
  const [end, setEnd] = useState(initialSource?.end ?? initialEnd);
  const [repeat, setRepeat] = useState<RepeatOption>(initialSource?.repeat ?? "none");
  const [scope, setScope] = useState<"occurrence" | "series">(
    event?.original_occurrence ? "occurrence" : "series",
  );
  const [error, setError] = useState<string | null>(null);
  const currentRule = scope === "series" ? seriesEvent?.recurrence_rule : event?.recurrence_rule;
  const timezoneOptions = useMemo(
    () => Array.from(new Set([timezone, ...commonTimezones])),
    [timezone],
  );

  const changeScope = (nextScope: "occurrence" | "series") => {
    setScope(nextScope);
    const source = nextScope === "series" ? seriesEvent : event;
    if (!source) return;
    const values = sourceValues(source);
    setTitle(values.title);
    setTimezone(values.timezone);
    setAllDay(values.allDay);
    setStart(values.start);
    setEnd(values.end);
    setRepeat(values.repeat);
  };

  const toggleAllDay = (checked: boolean) => {
    setAllDay(checked);
    if (checked) {
      setStart(start.slice(0, 10));
      setEnd(end.slice(0, 10));
    } else {
      setStart(`${start.slice(0, 10)}T09:00`);
      setEnd(`${end.slice(0, 10)}T10:00`);
    }
  };

  const submit = async (formEvent: React.FormEvent) => {
    formEvent.preventDefault();
    if (!title.trim()) {
      setError("请输入事件标题");
      return;
    }
    try {
      const startsAt = allDay ? allDayDateToIso(start, timezone) : dateTimeInputToIso(start, timezone);
      const endsAt = allDay
        ? inclusiveEndDateToIso(end, timezone)
        : dateTimeInputToIso(end, timezone);
      if (new Date(endsAt).getTime() <= new Date(startsAt).getTime()) {
        throw new Error("结束时间必须晚于开始时间");
      }
      setError(null);
      await onSave({
        calendarId,
        title: title.trim(),
        startsAt,
        endsAt,
        timezone,
        allDay,
        recurrenceRule:
          scope === "occurrence"
            ? event?.recurrence_rule ?? null
            : repeatRuleFromOption(repeat, currentRule ?? null),
        scope,
      });
    } catch (reason) {
      setError(String(reason instanceof Error ? reason.message : reason));
    }
  };

  const isOccurrence = Boolean(event?.original_occurrence);

  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-stone-950/25 p-4" role="presentation">
      <form
        onSubmit={submit}
        className="max-h-[92vh] w-full max-w-lg overflow-y-auto rounded-lg border border-stone-200 bg-white shadow-xl"
        role="dialog"
        aria-modal="true"
        aria-label={event ? "编辑事件" : "新建事件"}
      >
        <header className="sticky top-0 z-10 flex h-14 items-center justify-between border-b border-stone-200 bg-white px-5">
          <h2 className="text-base font-semibold text-stone-900">{event ? "编辑事件" : "新建事件"}</h2>
          <button
            type="button"
            onClick={onClose}
            className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100"
            title="关闭"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="space-y-5 p-5">
          {isOccurrence && (
            <div className="grid grid-cols-2 rounded-md bg-stone-100 p-1">
              <button
                type="button"
                onClick={() => changeScope("occurrence")}
                className={`h-8 rounded px-3 text-xs font-medium ${
                  scope === "occurrence" ? "bg-white text-stone-900 shadow-sm" : "text-stone-500"
                }`}
              >
                仅本次
              </button>
              <button
                type="button"
                onClick={() => changeScope("series")}
                className={`h-8 rounded px-3 text-xs font-medium ${
                  scope === "series" ? "bg-white text-stone-900 shadow-sm" : "text-stone-500"
                }`}
              >
                整个系列
              </button>
            </div>
          )}

          <label className="block">
            <span className="text-xs font-medium text-stone-600">标题</span>
            <input
              autoFocus
              value={title}
              onChange={(inputEvent) => setTitle(inputEvent.target.value)}
              className="mt-2 h-11 w-full rounded-md border border-stone-300 px-3 text-sm outline-none focus:border-emerald-600 focus:ring-2 focus:ring-emerald-100"
              placeholder="添加标题"
            />
          </label>

          <div className="grid grid-cols-[20px_1fr] gap-3">
            <CalendarDays className="mt-2 h-4 w-4 text-stone-400" />
            <div className="grid grid-cols-2 gap-3">
              <label className="text-xs font-medium text-stone-600">
                日历
                <select
                  value={calendarId}
                  onChange={(selectEvent) => {
                    const nextId = selectEvent.target.value;
                    setCalendarId(nextId);
                    const nextTimezone = calendars.find((calendar) => calendar.id === nextId)?.timezone;
                    if (nextTimezone) setTimezone(nextTimezone);
                  }}
                  disabled={Boolean(event)}
                  className="mt-2 h-10 w-full rounded-md border border-stone-300 bg-white px-2 text-sm disabled:bg-stone-50"
                >
                  {calendars.map((calendar) => (
                    <option key={calendar.id} value={calendar.id}>
                      {calendar.name}
                    </option>
                  ))}
                </select>
              </label>
              <label className="text-xs font-medium text-stone-600">
                时区
                <select
                  value={timezone}
                  onChange={(selectEvent) => setTimezone(selectEvent.target.value)}
                  className="mt-2 h-10 w-full rounded-md border border-stone-300 bg-white px-2 text-sm"
                >
                  {timezoneOptions.map((option) => (
                    <option key={option} value={option}>
                      {option}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          </div>

          <div className="grid grid-cols-[20px_1fr] gap-3">
            <Clock3 className="mt-2 h-4 w-4 text-stone-400" />
            <div>
              <label className="mb-3 flex h-7 items-center gap-2 text-xs text-stone-600">
                <input
                  type="checkbox"
                  checked={allDay}
                  onChange={(inputEvent) => toggleAllDay(inputEvent.target.checked)}
                  className="h-4 w-4 accent-emerald-700"
                />
                全天
              </label>
              <div className="grid grid-cols-2 gap-3">
                <label className="text-xs font-medium text-stone-600">
                  开始
                  <input
                    type={allDay ? "date" : "datetime-local"}
                    value={start}
                    onChange={(inputEvent) => setStart(inputEvent.target.value)}
                    className="mt-2 h-10 w-full rounded-md border border-stone-300 px-2 text-sm"
                  />
                </label>
                <label className="text-xs font-medium text-stone-600">
                  结束
                  <input
                    type={allDay ? "date" : "datetime-local"}
                    value={end}
                    min={allDay ? start : undefined}
                    onChange={(inputEvent) => setEnd(inputEvent.target.value)}
                    className="mt-2 h-10 w-full rounded-md border border-stone-300 px-2 text-sm"
                  />
                </label>
              </div>
            </div>
          </div>

          {scope === "series" && (
            <div className="grid grid-cols-[20px_1fr] gap-3">
              <Repeat2 className="mt-2 h-4 w-4 text-stone-400" />
              <label className="text-xs font-medium text-stone-600">
                重复
                <select
                  value={repeat}
                  onChange={(selectEvent) => setRepeat(selectEvent.target.value as RepeatOption)}
                  className="mt-2 h-10 w-full rounded-md border border-stone-300 bg-white px-2 text-sm"
                >
                  <option value="none">不重复</option>
                  <option value="daily">每天</option>
                  <option value="weekdays">每个工作日</option>
                  <option value="weekly">每周</option>
                  <option value="monthly">每月</option>
                  <option value="yearly">每年</option>
                  {repeat === "custom" && <option value="custom">保留当前自定义规则</option>}
                </select>
              </label>
            </div>
          )}

          {error && <p className="rounded-md bg-rose-50 px-3 py-2 text-xs text-rose-700">{error}</p>}
        </div>

        <footer className="flex min-h-16 items-center justify-between border-t border-stone-200 bg-stone-50/70 px-5 py-3">
          <div className="flex items-center gap-1">
            {event && (
              <button
                type="button"
                onClick={async () => {
                  const occurrence = scope === "occurrence";
                  if (!await confirmDelete({
                    title: occurrence ? "取消本次日程？" : "删除整个事件？",
                    description: occurrence
                      ? "本次日程将从重复事件中移除。"
                      : "该事件及其所有重复日程将被删除，且无法恢复。",
                    confirmLabel: occurrence ? "取消本次" : "删除事件",
                  })) return;
                  await onDelete(scope);
                }}
                disabled={busy}
                className="grid h-9 w-9 place-items-center rounded-md text-rose-600 hover:bg-rose-50"
                title={scope === "occurrence" ? "取消本次日程" : "删除事件"}
              >
                <Trash2 className="h-4 w-4" />
              </button>
            )}
            {event?.is_exception && scope === "occurrence" && (
              <button
                type="button"
                onClick={onRestore}
                disabled={busy}
                className="grid h-9 w-9 place-items-center rounded-md text-stone-600 hover:bg-stone-100 disabled:opacity-50"
                title="恢复本次日程"
              >
                <RotateCcw className="h-4 w-4" />
              </button>
            )}
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={onClose}
              className="h-9 rounded-md px-3 text-sm text-stone-600 hover:bg-stone-100"
            >
              取消
            </button>
            <button
              type="submit"
              disabled={busy || !calendarId}
              className="h-9 rounded-md bg-[#4f7f68] px-4 text-sm font-medium text-white disabled:opacity-50"
            >
              保存
            </button>
          </div>
        </footer>
      </form>
    </div>
  );
}
