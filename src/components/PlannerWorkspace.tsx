import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CalendarDays,
  Ban,
  CheckSquare2,
  CirclePlus,
  LoaderCircle,
  Menu,
  Pencil,
  Repeat2,
  RotateCcw,
} from "lucide-react";

type PlannerMode = "calendar" | "tasks";

interface PlannerWorkspaceProps {
  mode: PlannerMode;
  isSidebarOpen: boolean;
  onToggleSidebar: () => void;
}

interface Calendar {
  id: string;
  name: string;
  timezone: string;
}

interface CalendarEvent {
  id: string;
  occurrence_id: string;
  title: string;
  starts_at: string;
  ends_at: string;
  all_day: boolean;
  recurrence_rule: string | null;
  original_occurrence: string | null;
  is_exception: boolean;
}

interface TaskList {
  id: string;
  name: string;
}

interface Task {
  id: string;
  title: string;
  status: string;
  due_at: string | null;
  priority: number;
}

const nowIso = () => new Date().toISOString();
const afterMonthIso = () => new Date(Date.now() + 31 * 86_400_000).toISOString();

export const PlannerWorkspace: React.FC<PlannerWorkspaceProps> = ({
  mode,
  isSidebarOpen,
  onToggleSidebar,
}) => {
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  const [taskLists, setTaskLists] = useState<TaskList[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isCalendar = mode === "calendar";
  const items = isCalendar ? calendars : taskLists;
  const HeaderIcon = isCalendar ? CalendarDays : CheckSquare2;
  const selectedName = items.find((item) => item.id === selectedId)?.name;

  const loadContainers = async () => {
    setError(null);
    if (isCalendar) {
      const rows = await invoke<Calendar[]>("list_calendars");
      setCalendars(rows);
      setSelectedId((current) =>
        current && rows.some((row) => row.id === current) ? current : rows[0]?.id ?? null,
      );
    } else {
      const rows = await invoke<TaskList[]>("list_task_lists");
      setTaskLists(rows);
      setSelectedId((current) =>
        current && rows.some((row) => row.id === current) ? current : rows[0]?.id ?? null,
      );
    }
  };

  const loadItems = async (containerId: string) => {
    if (isCalendar) {
      const rows = await invoke<CalendarEvent[]>("list_calendar_events", {
        calendarId: containerId,
        rangeStart: nowIso(),
        rangeEnd: afterMonthIso(),
      });
      setEvents(rows);
    } else {
      const rows = await invoke<Task[]>("list_tasks", { taskListId: containerId });
      setTasks(rows);
    }
  };

  useEffect(() => {
    loadContainers().catch((reason) => setError(String(reason)));
  }, [mode]);

  useEffect(() => {
    if (!selectedId) {
      setEvents([]);
      setTasks([]);
      return;
    }
    loadItems(selectedId).catch((reason) => setError(String(reason)));
  }, [isCalendar, selectedId]);

  const createContainer = async () => {
    const name = window.prompt(isCalendar ? "日历名称" : "任务列表名称");
    if (!name?.trim()) return;

    setBusy(true);
    try {
      const id = isCalendar
        ? await invoke<string>("create_calendar", {
            name: name.trim(),
            color: null,
            timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
          })
        : await invoke<string>("create_task_list", {
            name: name.trim(),
            color: null,
          });
      await loadContainers();
      setSelectedId(id);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const createItem = async () => {
    if (!selectedId) return;
    const title = window.prompt(isCalendar ? "事件标题" : "任务标题");
    if (!title?.trim()) return;

    setBusy(true);
    try {
      if (isCalendar) {
        const startsAt = window.prompt("开始时间（ISO 8601）", nowIso());
        const endsAt = window.prompt(
          "结束时间（ISO 8601）",
          new Date(Date.now() + 3_600_000).toISOString(),
        );
        if (!startsAt || !endsAt) return;
        const recurrenceRule = window.prompt("重复规则（RRULE，可留空）", "");
        if (recurrenceRule === null) return;

        await invoke("create_calendar_event", {
          calendarId: selectedId,
          title: title.trim(),
          startsAt,
          endsAt,
          timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
          allDay: false,
          recurrenceRule: recurrenceRule.trim() || null,
        });
      } else {
        await invoke("create_task", {
          taskListId: selectedId,
          parentId: null,
          title: title.trim(),
          description: null,
          priority: 0,
          dueAt: null,
          sortOrder: Date.now(),
        });
      }
      await loadItems(selectedId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const toggleTask = async (task: Task) => {
    if (!selectedId) return;
    try {
      await invoke("complete_task", {
        taskId: task.id,
        completed: task.status !== "completed",
      });
      await loadItems(selectedId);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const editEvent = async (event: CalendarEvent) => {
    if (!selectedId) return;
    const title = window.prompt("事件标题", event.title);
    if (!title?.trim()) return;
    const startsAt = window.prompt("开始时间（ISO 8601）", event.starts_at);
    if (!startsAt) return;
    const endsAt = window.prompt("结束时间（ISO 8601）", event.ends_at);
    if (!endsAt) return;

    setBusy(true);
    try {
      const changes = {
        title: title.trim(),
        startsAt,
        endsAt,
      };
      if (event.original_occurrence) {
        await invoke("update_calendar_occurrence", {
          eventId: event.id,
          originalOccurrence: event.original_occurrence,
          changes,
        });
      } else {
        await invoke("update_calendar_event", {
          eventId: event.id,
          changes,
        });
      }
      await loadItems(selectedId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const cancelOccurrence = async (event: CalendarEvent) => {
    if (!selectedId || !event.original_occurrence) return;
    if (!window.confirm(`取消“${event.title}”的本次日程？`)) return;

    setBusy(true);
    try {
      await invoke("cancel_calendar_occurrence", {
        eventId: event.id,
        originalOccurrence: event.original_occurrence,
      });
      await loadItems(selectedId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  const restoreOccurrence = async (event: CalendarEvent) => {
    if (!selectedId || !event.original_occurrence) return;

    setBusy(true);
    try {
      await invoke("restore_calendar_occurrence", {
        eventId: event.id,
        originalOccurrence: event.original_occurrence,
      });
      await loadItems(selectedId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="flex h-full min-w-0 flex-1 flex-col bg-[#FAF9F5]">
      <header className="flex h-14 shrink-0 items-center justify-between border-b border-stone-200 bg-white/40 px-6">
        <div className="flex items-center gap-3">
          <button
            onClick={onToggleSidebar}
            className="rounded-lg p-1.5 text-stone-500 hover:bg-stone-200/40"
            title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
          >
            <Menu className="h-4 w-4" />
          </button>
          <div className="h-4 w-px bg-stone-200" />
          <span className="flex items-center gap-2 text-sm font-semibold text-stone-800">
            <HeaderIcon className="h-4 w-4 text-[#8CA38A]" />
            {isCalendar ? "日历" : "待办"}
          </span>
        </div>
        {busy && <LoaderCircle className="h-4 w-4 animate-spin text-stone-400" />}
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="w-60 shrink-0 border-r border-stone-200 bg-white/30 p-3">
          <div className="mb-2 flex justify-between px-1 text-[10px] font-bold uppercase tracking-wider text-stone-400">
            <span>{isCalendar ? "Calendars" : "Task lists"}</span>
            <button onClick={createContainer} title="新建">
              <CirclePlus className="h-4 w-4 text-stone-500" />
            </button>
          </div>
          {items.map((item) => (
            <button
              key={item.id}
              onClick={() => setSelectedId(item.id)}
              className={`w-full rounded-lg px-2 py-2 text-left text-xs ${
                item.id === selectedId
                  ? "bg-white font-semibold text-emerald-700 shadow-sm"
                  : "text-stone-600 hover:bg-stone-200/40"
              }`}
            >
              {item.name}
            </button>
          ))}
        </aside>

        <section className="min-w-0 flex-1 p-6">
          <div className="mx-auto max-w-3xl">
            <div className="mb-5 flex justify-between">
              <div>
                <h1 className="text-lg font-semibold text-stone-900">
                  {selectedName ?? (isCalendar ? "本地日历" : "本地待办")}
                </h1>
                <p className="mt-1 text-xs text-stone-500">
                  {isCalendar ? "未来 31 天 · 本地时区" : "本地任务列表"}
                </p>
              </div>
              <button
                onClick={createItem}
                disabled={!selectedId || busy}
                className="rounded-xl bg-[#8CA38A] px-3 py-2 text-xs font-semibold text-white disabled:opacity-50"
              >
                新建{isCalendar ? "事件" : "任务"}
              </button>
            </div>

            {error && (
              <p className="mb-3 rounded-xl border border-rose-200 bg-rose-50 p-3 text-xs text-rose-700">
                {error}
              </p>
            )}

            {isCalendar ? (
              <div className="space-y-2">
                {events.map((event) => (
                  <article
                    key={event.occurrence_id}
                    className="flex items-start justify-between gap-4 rounded-lg border border-stone-200 bg-white p-4"
                  >
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <p className="truncate text-sm font-semibold text-stone-800">{event.title}</p>
                        {event.recurrence_rule && (
                          <Repeat2
                            className="h-3.5 w-3.5 shrink-0 text-stone-400"
                            aria-label="重复事件"
                          />
                        )}
                      </div>
                      <p className="mt-1 text-xs text-stone-500">
                        {event.all_day
                          ? event.starts_at.slice(0, 10)
                          : new Date(event.starts_at).toLocaleString()}
                      </p>
                      {event.is_exception && (
                        <p className="mt-1 text-[11px] text-amber-700">本次日程已修改</p>
                      )}
                    </div>
                    <div className="flex h-8 shrink-0 items-center gap-1">
                      <button
                        type="button"
                        onClick={() => editEvent(event)}
                        disabled={busy}
                        className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100 disabled:opacity-40"
                        title={event.original_occurrence ? "编辑本次" : "编辑事件"}
                      >
                        <Pencil className="h-3.5 w-3.5" />
                      </button>
                      {event.is_exception && (
                        <button
                          type="button"
                          onClick={() => restoreOccurrence(event)}
                          disabled={busy}
                          className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100 disabled:opacity-40"
                          title="恢复本次"
                        >
                          <RotateCcw className="h-3.5 w-3.5" />
                        </button>
                      )}
                      {event.original_occurrence && (
                        <button
                          type="button"
                          onClick={() => cancelOccurrence(event)}
                          disabled={busy}
                          className="grid h-8 w-8 place-items-center rounded-md text-rose-600 hover:bg-rose-50 disabled:opacity-40"
                          title="取消本次"
                        >
                          <Ban className="h-3.5 w-3.5" />
                        </button>
                      )}
                    </div>
                  </article>
                ))}
                {events.length === 0 && (
                  <p className="rounded-2xl border border-dashed border-stone-200 py-12 text-center text-sm text-stone-400">
                    新建日历后，可记录未来的本地事件。
                  </p>
                )}
              </div>
            ) : (
              <div className="space-y-2">
                {tasks.map((task) => (
                  <label
                    key={task.id}
                    className="flex items-center gap-3 rounded-xl border border-stone-200 bg-white p-3 text-sm text-stone-700"
                  >
                    <input
                      type="checkbox"
                      checked={task.status === "completed"}
                      onChange={() => toggleTask(task)}
                    />
                    <span className={task.status === "completed" ? "text-stone-400 line-through" : ""}>
                      {task.title}
                    </span>
                  </label>
                ))}
                {tasks.length === 0 && (
                  <p className="rounded-2xl border border-dashed border-stone-200 py-12 text-center text-sm text-stone-400">
                    新建任务列表后，可记录本地待办。
                  </p>
                )}
              </div>
            )}
          </div>
        </section>
      </div>
    </main>
  );
};
