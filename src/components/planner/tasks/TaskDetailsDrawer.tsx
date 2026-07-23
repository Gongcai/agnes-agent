import { useEffect, useMemo, useState } from "react";
import {
  CalendarClock,
  Check,
  Circle,
  CirclePlus,
  Repeat2,
  Star,
  Sun,
  Trash2,
  X,
} from "lucide-react";
import { useConfirmDialog } from "../../ConfirmDialog";
import type { RepeatOption, Task, TaskList, TaskUpdateChanges } from "../shared/types";
import {
  commonTimezones,
  dateTimeInputToIso,
  isoToDateTimeInput,
  localTimezone,
  repeatOptionFromRule,
  repeatRuleFromOption,
  todayKey,
  tomorrowKey,
} from "../shared/time";

interface TaskDetailsDrawerProps {
  task: Task;
  list: TaskList | undefined;
  subtasks: Task[];
  busy: boolean;
  onClose: () => void;
  onSave: (changes: TaskUpdateChanges) => Promise<void>;
  onToggle: (task: Task) => Promise<void>;
  onCreateSubtask: (title: string) => Promise<void>;
  onDelete: () => Promise<void>;
}

type DueMode = "none" | "date" | "time";

export function TaskDetailsDrawer({
  task,
  list,
  subtasks,
  busy,
  onClose,
  onSave,
  onToggle,
  onCreateSubtask,
  onDelete,
}: TaskDetailsDrawerProps) {
  const confirmDelete = useConfirmDialog();
  const taskTimezone = task.due_timezone || localTimezone;
  const [title, setTitle] = useState(task.title);
  const [description, setDescription] = useState(task.description || "");
  const [important, setImportant] = useState(task.is_important);
  const [myDay, setMyDay] = useState(task.my_day_date === todayKey());
  const [dueMode, setDueMode] = useState<DueMode>(task.due_date ? "date" : task.due_at ? "time" : "none");
  const [dueDate, setDueDate] = useState(task.due_date || tomorrowKey());
  const [dueTime, setDueTime] = useState(
    task.due_at ? isoToDateTimeInput(task.due_at, taskTimezone) : `${tomorrowKey()}T09:00`,
  );
  const [timezone, setTimezone] = useState(taskTimezone);
  const [repeat, setRepeat] = useState<RepeatOption>(repeatOptionFromRule(task.recurrence_rule));
  const [subtaskTitle, setSubtaskTitle] = useState("");
  const [error, setError] = useState<string | null>(null);
  const timezoneOptions = useMemo(
    () => Array.from(new Set([timezone, ...commonTimezones])),
    [timezone],
  );

  useEffect(() => {
    const nextTimezone = task.due_timezone || localTimezone;
    setTitle(task.title);
    setDescription(task.description || "");
    setImportant(task.is_important);
    setMyDay(task.my_day_date === todayKey());
    setDueMode(task.due_date ? "date" : task.due_at ? "time" : "none");
    setDueDate(task.due_date || tomorrowKey());
    setDueTime(task.due_at ? isoToDateTimeInput(task.due_at, nextTimezone) : `${tomorrowKey()}T09:00`);
    setTimezone(nextTimezone);
    setRepeat(repeatOptionFromRule(task.recurrence_rule));
    setError(null);
  }, [task]);

  const save = async () => {
    if (!title.trim()) {
      setError("任务标题不能为空");
      return;
    }
    if (repeat !== "none" && dueMode === "none") {
      setError("重复任务需要设置截止日期或时间");
      return;
    }
    try {
      setError(null);
      await onSave({
        title: title.trim(),
        description: description.trim() || null,
        isImportant: important,
        myDayDate: myDay ? todayKey() : null,
        dueDate: dueMode === "date" ? dueDate : null,
        dueAt: dueMode === "time" ? dateTimeInputToIso(dueTime, timezone) : null,
        dueTimezone: dueMode === "none" ? null : timezone,
        recurrenceRule:
          dueMode === "none" ? null : repeatRuleFromOption(repeat, task.recurrence_rule),
      });
    } catch (reason) {
      setError(String(reason instanceof Error ? reason.message : reason));
    }
  };

  const addSubtask = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!subtaskTitle.trim()) return;
    try {
      await onCreateSubtask(subtaskTitle.trim());
      setSubtaskTitle("");
    } catch (reason) {
      setError(String(reason));
    }
  };

  return (
    <div className="fixed inset-0 z-40 flex justify-end bg-stone-950/20" role="presentation">
      <aside
        className="flex h-full w-full max-w-md flex-col border-l border-stone-200 bg-[#FAF9F5] shadow-2xl"
        role="dialog"
        aria-modal="true"
        aria-label="任务详情"
      >
        <header className="flex h-14 shrink-0 items-center justify-between border-b border-stone-200 bg-white/70 px-4">
          <span className="text-xs font-medium text-stone-500">{list?.name ?? "任务"}</span>
          <button
            type="button"
            onClick={onClose}
            className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100"
            title="关闭"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          <div className="border-b border-stone-200 bg-white px-5 py-5">
            <div className="flex items-start gap-3">
              <button
                type="button"
                onClick={() => onToggle(task)}
                disabled={busy}
                className={`mt-1 grid h-6 w-6 shrink-0 place-items-center rounded-full border ${
                  task.status === "completed"
                    ? "border-[#4f7f68] bg-[#4f7f68] text-white"
                    : "border-stone-400 text-transparent hover:border-[#4f7f68]"
                }`}
                title={task.status === "completed" ? "重新打开任务" : "完成任务"}
              >
                <Check className="h-3.5 w-3.5" />
              </button>
              <textarea
                value={title}
                onChange={(event) => setTitle(event.target.value)}
                rows={2}
                className="min-w-0 flex-1 resize-none bg-transparent text-base font-semibold leading-6 text-stone-900 outline-none"
              />
              <button
                type="button"
                onClick={() => setImportant((value) => !value)}
                className={`grid h-8 w-8 shrink-0 place-items-center rounded-md ${
                  important ? "text-amber-500" : "text-stone-400 hover:bg-stone-100"
                }`}
                title={important ? "取消重要" : "标记为重要"}
              >
                <Star className={`h-5 w-5 ${important ? "fill-current" : ""}`} />
              </button>
            </div>
          </div>

          <div className="space-y-3 px-5 py-4">
            <button
              type="button"
              onClick={() => setMyDay((value) => !value)}
              className={`flex h-11 w-full items-center gap-3 rounded-md border px-3 text-left text-sm ${
                myDay
                  ? "border-sky-200 bg-sky-50 text-sky-800"
                  : "border-stone-200 bg-white text-stone-700 hover:bg-stone-50"
              }`}
            >
              <Sun className="h-4 w-4" />
              {myDay ? "已加入我的一天" : "加入我的一天"}
            </button>

            <section className="rounded-md border border-stone-200 bg-white p-3">
              <div className="mb-3 flex items-center gap-2 text-xs font-medium text-stone-600">
                <CalendarClock className="h-4 w-4" /> 截止时间
              </div>
              <div className="grid grid-cols-3 rounded-md bg-stone-100 p-1">
                {([
                  ["none", "无"],
                  ["date", "日期"],
                  ["time", "具体时间"],
                ] as const).map(([value, label]) => (
                  <button
                    key={value}
                    type="button"
                    onClick={() => setDueMode(value)}
                    className={`h-8 rounded text-xs ${
                      dueMode === value ? "bg-white font-medium text-stone-900 shadow-sm" : "text-stone-500"
                    }`}
                  >
                    {label}
                  </button>
                ))}
              </div>
              {dueMode === "date" && (
                <input
                  type="date"
                  value={dueDate}
                  onChange={(event) => setDueDate(event.target.value)}
                  className="mt-3 h-10 w-full rounded-md border border-stone-300 px-2 text-sm"
                />
              )}
              {dueMode === "time" && (
                <input
                  type="datetime-local"
                  value={dueTime}
                  onChange={(event) => setDueTime(event.target.value)}
                  className="mt-3 h-10 w-full rounded-md border border-stone-300 px-2 text-sm"
                />
              )}
              {dueMode !== "none" && (
                <select
                  value={timezone}
                  onChange={(event) => setTimezone(event.target.value)}
                  className="mt-2 h-9 w-full rounded-md border border-stone-300 bg-white px-2 text-xs text-stone-600"
                  aria-label="时区"
                >
                  {timezoneOptions.map((option) => (
                    <option key={option} value={option}>
                      {option}
                    </option>
                  ))}
                </select>
              )}
            </section>

            <label className="block rounded-md border border-stone-200 bg-white p-3 text-xs font-medium text-stone-600">
              <span className="flex items-center gap-2">
                <Repeat2 className="h-4 w-4" /> 重复
              </span>
              <select
                value={repeat}
                onChange={(event) => setRepeat(event.target.value as RepeatOption)}
                disabled={dueMode === "none"}
                className="mt-3 h-10 w-full rounded-md border border-stone-300 bg-white px-2 text-sm disabled:bg-stone-50 disabled:text-stone-400"
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

            <section className="rounded-md border border-stone-200 bg-white p-3">
              <div className="mb-2 flex items-center justify-between">
                <span className="text-xs font-medium text-stone-600">步骤</span>
                <span className="text-[11px] text-stone-400">{subtasks.length}</span>
              </div>
              <div className="divide-y divide-stone-100">
                {subtasks.map((subtask) => (
                  <div key={subtask.id} className="flex min-h-9 items-center gap-2 py-1">
                    <button
                      type="button"
                      onClick={() => onToggle(subtask)}
                      className="grid h-7 w-7 shrink-0 place-items-center text-stone-500"
                      title={subtask.status === "completed" ? "重新打开步骤" : "完成步骤"}
                    >
                      {subtask.status === "completed" ? (
                        <Check className="h-4 w-4 text-[#4f7f68]" />
                      ) : (
                        <Circle className="h-4 w-4" />
                      )}
                    </button>
                    <span
                      className={`min-w-0 flex-1 truncate text-sm ${
                        subtask.status === "completed" ? "text-stone-400 line-through" : "text-stone-700"
                      }`}
                    >
                      {subtask.title}
                    </span>
                  </div>
                ))}
              </div>
              <form onSubmit={addSubtask} className="mt-2 flex items-center gap-2 border-t border-stone-100 pt-2">
                <CirclePlus className="h-4 w-4 shrink-0 text-stone-400" />
                <input
                  value={subtaskTitle}
                  onChange={(event) => setSubtaskTitle(event.target.value)}
                  className="h-8 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-stone-400"
                  placeholder="添加步骤"
                />
                <button
                  type="submit"
                  disabled={!subtaskTitle.trim() || busy}
                  className="h-8 rounded-md px-2 text-xs font-medium text-[#4f7f68] disabled:opacity-40"
                >
                  添加
                </button>
              </form>
            </section>

            <label className="block rounded-md border border-stone-200 bg-white p-3 text-xs font-medium text-stone-600">
              备注
              <textarea
                value={description}
                onChange={(event) => setDescription(event.target.value)}
                rows={5}
                className="mt-2 w-full resize-y bg-transparent text-sm font-normal leading-6 text-stone-700 outline-none placeholder:text-stone-400"
                placeholder="添加说明或相关信息"
              />
            </label>

            {error && <p className="rounded-md bg-rose-50 px-3 py-2 text-xs text-rose-700">{error}</p>}
          </div>
        </div>

        <footer className="flex min-h-16 shrink-0 items-center justify-between border-t border-stone-200 bg-white px-4 py-3">
          <button
            type="button"
            onClick={async () => {
              if (!await confirmDelete({
                title: `删除任务「${task.title}」？`,
                description: "任务及其所有步骤将一并删除，且无法恢复。",
                confirmLabel: "删除任务",
              })) return;
              await onDelete();
            }}
            disabled={busy}
            className="grid h-9 w-9 place-items-center rounded-md text-rose-600 hover:bg-rose-50 disabled:opacity-50"
            title="删除任务"
          >
            <Trash2 className="h-4 w-4" />
          </button>
          <button
            type="button"
            onClick={save}
            disabled={busy}
            className="h-9 rounded-md bg-[#4f7f68] px-4 text-sm font-medium text-white disabled:opacity-50"
          >
            保存
          </button>
        </footer>
      </aside>
    </div>
  );
}
