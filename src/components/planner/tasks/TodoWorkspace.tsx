import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CalendarClock,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CirclePlus,
  Inbox,
  ListChecks,
  ListTodo,
  LoaderCircle,
  Search,
  Star,
  Sun,
  X,
} from "lucide-react";
import type {
  SmartTaskView,
  Task,
  TaskList,
  TaskUpdateChanges,
  TaskView,
} from "../shared/types";
import {
  formatDateKey,
  formatDateTime,
  localTimezone,
  repeatLabel,
  todayKey,
} from "../shared/time";
import { QuickAdd } from "./QuickAdd";
import { TaskDetailsDrawer } from "./TaskDetailsDrawer";

interface TodoWorkspaceProps {
  requestedTaskId: string | null;
  onCloseRequestedTask: () => void;
}

const smartViews: Array<{
  id: SmartTaskView;
  label: string;
  icon: typeof Sun;
  tone: string;
}> = [
  { id: "my-day", label: "我的一天", icon: Sun, tone: "text-sky-600" },
  { id: "important", label: "重要", icon: Star, tone: "text-amber-500" },
  { id: "planned", label: "已计划", icon: CalendarClock, tone: "text-emerald-600" },
  { id: "all", label: "全部", icon: Inbox, tone: "text-stone-500" },
  { id: "completed", label: "已完成", icon: CheckCircle2, tone: "text-violet-600" },
];

const listColors = ["#4f8a6f", "#3b82a0", "#b7791f", "#9b5f86", "#c45d55", "#697386"];

function viewKey(view: TaskView): string {
  return `${view.kind}:${view.id}`;
}

function taskMatchesView(task: Task, view: TaskView, today: string): boolean {
  if (task.parent_id) return false;
  if (view.kind === "list") return task.task_list_id === view.id;
  if (view.id === "my-day") return task.my_day_date === today;
  if (view.id === "important") return task.is_important;
  if (view.id === "planned") return Boolean(task.due_date || task.due_at);
  if (view.id === "completed") return task.status === "completed";
  return true;
}

function sortTasks(left: Task, right: Task): number {
  return (
    Number(right.is_important) - Number(left.is_important) ||
    (left.due_date || left.due_at || "9999").localeCompare(right.due_date || right.due_at || "9999") ||
    left.sort_order - right.sort_order ||
    left.title.localeCompare(right.title, "zh-CN")
  );
}

export function TodoWorkspace({ requestedTaskId, onCloseRequestedTask }: TodoWorkspaceProps) {
  const [lists, setLists] = useState<TaskList[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [view, setView] = useState<TaskView>({ kind: "smart", id: "my-day" });
  const [openTaskId, setOpenTaskId] = useState<string | null>(requestedTaskId);
  const [query, setQuery] = useState("");
  const [showCompleted, setShowCompleted] = useState(true);
  const [busyIds, setBusyIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [createListOpen, setCreateListOpen] = useState(false);
  const [newListName, setNewListName] = useState("");
  const today = todayKey();

  const loadData = useCallback(async () => {
    const [listRows, taskRows] = await Promise.all([
      invoke<TaskList[]>("list_task_lists"),
      invoke<Task[]>("list_all_tasks"),
    ]);
    setLists(listRows);
    setTasks(taskRows);
  }, []);

  useEffect(() => {
    let active = true;
    setLoading(true);
    loadData()
      .catch((reason) => active && setError(String(reason)))
      .finally(() => active && setLoading(false));
    return () => {
      active = false;
    };
  }, [loadData]);

  useEffect(() => {
    if (requestedTaskId) setOpenTaskId(requestedTaskId);
  }, [requestedTaskId]);

  const currentView = useMemo(() => {
    if (view.kind === "list") {
      const list = lists.find((candidate) => candidate.id === view.id);
      return { title: list?.name ?? "任务列表", subtitle: "自定义列表", icon: ListChecks };
    }
    const definitions: Record<SmartTaskView, { title: string; subtitle: string; icon: typeof Sun }> = {
      "my-day": { title: "我的一天", subtitle: formatDateKey(today), icon: Sun },
      important: { title: "重要", subtitle: "标记为重要的任务", icon: Star },
      planned: { title: "已计划", subtitle: "有截止日期或时间的任务", icon: CalendarClock },
      all: { title: "全部", subtitle: "所有任务列表", icon: Inbox },
      completed: { title: "已完成", subtitle: "已完成的任务实例", icon: CheckCircle2 },
    };
    return definitions[view.id];
  }, [lists, today, view]);

  const visibleTasks = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase("zh-CN");
    return tasks
      .filter((task) => taskMatchesView(task, view, today))
      .filter(
        (task) =>
          !normalizedQuery ||
          task.title.toLocaleLowerCase("zh-CN").includes(normalizedQuery) ||
          task.description?.toLocaleLowerCase("zh-CN").includes(normalizedQuery),
      )
      .sort(sortTasks);
  }, [query, tasks, today, view]);

  const openTasks = visibleTasks.filter((task) => task.status !== "completed");
  const completedTasks = visibleTasks.filter((task) => task.status === "completed");
  const selectedTask = tasks.find((task) => task.id === openTaskId);
  const selectedTaskList = selectedTask
    ? lists.find((list) => list.id === selectedTask.task_list_id)
    : undefined;
  const selectedSubtasks = selectedTask
    ? tasks.filter((task) => task.parent_id === selectedTask.id).sort(sortTasks)
    : [];
  const defaultListId = view.kind === "list" ? view.id : lists[0]?.id ?? null;

  const setBusy = (id: string, value: boolean) => {
    setBusyIds((current) => {
      const next = new Set(current);
      if (value) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  const toggleTask = async (task: Task) => {
    if (busyIds.has(task.id)) return;
    const completed = task.status !== "completed";
    setBusy(task.id, true);
    setTasks((current) =>
      current.map((candidate) =>
        candidate.id === task.id
          ? {
              ...candidate,
              status: completed ? "completed" : "open",
              completed_at: completed ? new Date().toISOString() : null,
            }
          : candidate,
      ),
    );
    try {
      await invoke("complete_task", { taskId: task.id, completed });
      await loadData();
    } catch (reason) {
      setTasks((current) =>
        current.map((candidate) => (candidate.id === task.id ? task : candidate)),
      );
      setError(String(reason));
    } finally {
      setBusy(task.id, false);
    }
  };

  const updateTask = async (taskId: string, changes: TaskUpdateChanges) => {
    setBusy(taskId, true);
    try {
      await invoke("update_task", { taskId, changes });
      await loadData();
    } catch (reason) {
      setError(String(reason));
      throw reason;
    } finally {
      setBusy(taskId, false);
    }
  };

  const createTask = async (value: {
    title: string;
    taskListId: string;
    dueDate: string | null;
    addToMyDay: boolean;
    isImportant: boolean;
  }) => {
    const operationId = "create-task";
    setBusy(operationId, true);
    try {
      await invoke("create_task", {
        taskListId: value.taskListId,
        parentId: null,
        title: value.title,
        description: null,
        priority: 0,
        dueDate: value.dueDate,
        dueAt: null,
        dueTimezone: value.dueDate ? localTimezone : null,
        isImportant: value.isImportant,
        myDayDate: value.addToMyDay ? today : null,
        recurrenceRule: null,
        sortOrder: Date.now(),
      });
      await loadData();
    } catch (reason) {
      setError(String(reason));
      throw reason;
    } finally {
      setBusy(operationId, false);
    }
  };

  const createSubtask = async (title: string) => {
    if (!selectedTask) return;
    setBusy(selectedTask.id, true);
    try {
      await invoke("create_task", {
        taskListId: selectedTask.task_list_id,
        parentId: selectedTask.id,
        title,
        description: null,
        priority: 0,
        dueDate: null,
        dueAt: null,
        dueTimezone: null,
        isImportant: false,
        myDayDate: null,
        recurrenceRule: null,
        sortOrder: Date.now(),
      });
      await loadData();
    } catch (reason) {
      setError(String(reason));
      throw reason;
    } finally {
      setBusy(selectedTask.id, false);
    }
  };

  const deleteSelectedTask = async () => {
    if (!selectedTask) return;
    setBusy(selectedTask.id, true);
    try {
      await invoke("delete_task", { taskId: selectedTask.id });
      setOpenTaskId(null);
      onCloseRequestedTask();
      await loadData();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(selectedTask.id, false);
    }
  };

  const createList = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!newListName.trim()) return;
    setBusy("create-list", true);
    try {
      const color = listColors[lists.length % listColors.length];
      const id = await invoke<string>("create_task_list", { name: newListName.trim(), color });
      setNewListName("");
      setCreateListOpen(false);
      await loadData();
      setView({ kind: "list", id });
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy("create-list", false);
    }
  };

  const renderTask = (task: Task) => {
    const dueText = task.due_date
      ? formatDateKey(task.due_date)
      : task.due_at
        ? formatDateTime(task.due_at, task.due_timezone || localTimezone)
        : null;
    const recurrence = repeatLabel(task.recurrence_rule);
    const listName = lists.find((list) => list.id === task.task_list_id)?.name;
    return (
      <div
        key={task.id}
        className="group flex min-h-14 items-center gap-3 border-b border-stone-100 bg-white px-4 hover:bg-stone-50/80"
      >
        <button
          type="button"
          onClick={() => void toggleTask(task)}
          disabled={busyIds.has(task.id)}
          className={`grid h-6 w-6 shrink-0 place-items-center rounded-full border ${
            task.status === "completed"
              ? "border-[#4f7f68] bg-[#4f7f68] text-white"
              : "border-stone-400 text-transparent hover:border-[#4f7f68] hover:text-[#4f7f68]"
          }`}
          title={task.status === "completed" ? "重新打开任务" : "完成任务"}
        >
          {busyIds.has(task.id) ? (
            <LoaderCircle className="h-3.5 w-3.5 animate-spin text-current" />
          ) : (
            <Check className="h-3.5 w-3.5" />
          )}
        </button>
        <button
          type="button"
          onClick={() => setOpenTaskId(task.id)}
          className="min-w-0 flex-1 py-2 text-left"
        >
          <span
            className={`block truncate text-sm ${
              task.status === "completed" ? "text-stone-400 line-through" : "font-medium text-stone-800"
            }`}
          >
            {task.title}
          </span>
          {(dueText || recurrence || view.kind === "smart") && (
            <span className="mt-1 flex min-w-0 items-center gap-2 text-[11px] text-stone-400">
              {dueText && <span className="truncate text-amber-700">{dueText}</span>}
              {recurrence && <span className="truncate">{recurrence}</span>}
              {view.kind === "smart" && listName && <span className="truncate">{listName}</span>}
            </span>
          )}
        </button>
        <button
          type="button"
          onClick={() => {
            void updateTask(task.id, { isImportant: !task.is_important }).catch(() => {});
          }}
          disabled={busyIds.has(task.id)}
          className={`grid h-8 w-8 shrink-0 place-items-center rounded-md ${
            task.is_important ? "text-amber-500" : "text-stone-300 hover:bg-stone-100 hover:text-stone-500"
          }`}
          title={task.is_important ? "取消重要" : "标记为重要"}
        >
          <Star className={`h-4 w-4 ${task.is_important ? "fill-current" : ""}`} />
        </button>
      </div>
    );
  };

  return (
    <div className="flex min-h-0 flex-1">
      <aside className="flex w-56 shrink-0 flex-col border-r border-stone-200 bg-white/45 px-3 py-4">
        <nav className="space-y-0.5">
          {smartViews.map((item) => {
            const Icon = item.icon;
            const count = tasks.filter((task) => taskMatchesView(task, { kind: "smart", id: item.id }, today)).length;
            const active = view.kind === "smart" && view.id === item.id;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setView({ kind: "smart", id: item.id })}
                className={`flex h-9 w-full items-center gap-2 rounded-md px-2 text-xs ${
                  active ? "bg-white font-semibold text-stone-900 shadow-sm" : "text-stone-600 hover:bg-stone-100"
                }`}
              >
                <Icon className={`h-4 w-4 ${item.tone}`} />
                <span className="flex-1 text-left">{item.label}</span>
                {count > 0 && <span className="text-[10px] tabular-nums text-stone-400">{count}</span>}
              </button>
            );
          })}
        </nav>

        <div className="my-3 border-t border-stone-200" />
        <div className="mb-1 flex h-8 items-center justify-between px-2">
          <span className="text-[11px] font-semibold text-stone-500">列表</span>
          <button
            type="button"
            onClick={() => setCreateListOpen(true)}
            className="grid h-7 w-7 place-items-center rounded-md text-stone-500 hover:bg-stone-100"
            title="新建列表"
          >
            <CirclePlus className="h-4 w-4" />
          </button>
        </div>
        <nav className="min-h-0 flex-1 space-y-0.5 overflow-y-auto">
          {lists.map((list, index) => {
            const active = view.kind === "list" && view.id === list.id;
            const count = tasks.filter(
              (task) => !task.parent_id && task.task_list_id === list.id && task.status !== "completed",
            ).length;
            return (
              <button
                key={list.id}
                type="button"
                onClick={() => setView({ kind: "list", id: list.id })}
                className={`flex h-9 w-full items-center gap-2 rounded-md px-2 text-xs ${
                  active ? "bg-white font-semibold text-stone-900 shadow-sm" : "text-stone-600 hover:bg-stone-100"
                }`}
              >
                <span
                  className="h-2.5 w-2.5 rounded-full"
                  style={{ backgroundColor: list.color || listColors[index % listColors.length] }}
                />
                <span className="min-w-0 flex-1 truncate text-left">{list.name}</span>
                {count > 0 && <span className="text-[10px] tabular-nums text-stone-400">{count}</span>}
              </button>
            );
          })}
        </nav>
      </aside>

      <section className="flex min-w-0 flex-1 flex-col bg-[#FAF9F5]">
        <header className="flex min-h-20 shrink-0 items-center justify-between px-5 py-3">
          <div className="flex min-w-0 items-center gap-3">
            <currentView.icon className="h-5 w-5 shrink-0 text-[#4f7f68]" />
            <div className="min-w-0">
              <h1 className="truncate text-lg font-semibold text-stone-900">{currentView.title}</h1>
              <p className="mt-0.5 truncate text-xs text-stone-500">{currentView.subtitle}</p>
            </div>
          </div>
          <label className="flex h-9 w-52 items-center gap-2 rounded-md border border-stone-200 bg-white px-3 text-stone-400 focus-within:border-stone-400">
            <Search className="h-4 w-4 shrink-0" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              className="min-w-0 flex-1 bg-transparent text-xs text-stone-700 outline-none"
              placeholder="搜索任务"
            />
          </label>
        </header>

        {!(view.kind === "smart" && view.id === "completed") && (
          <QuickAdd
            key={viewKey(view)}
            lists={lists}
            defaultListId={defaultListId}
            defaultDueDate={view.kind === "smart" && view.id === "planned" ? today : undefined}
            defaultMyDay={view.kind === "smart" && view.id === "my-day"}
            defaultImportant={view.kind === "smart" && view.id === "important"}
            busy={busyIds.has("create-task")}
            onCreate={createTask}
          />
        )}

        {error && (
          <div className="mx-4 mt-3 flex items-center justify-between rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700">
            <span className="min-w-0 truncate">{error}</span>
            <button type="button" onClick={() => setError(null)} className="ml-3 font-medium">
              关闭
            </button>
          </div>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-4">
          {loading ? (
            <div className="grid h-40 place-items-center">
              <LoaderCircle className="h-5 w-5 animate-spin text-stone-400" />
            </div>
          ) : (
            <div className="overflow-hidden rounded-md border border-stone-200 bg-white">
              {openTasks.map(renderTask)}
              {completedTasks.length > 0 && view.kind !== "smart" ||
              (completedTasks.length > 0 && view.id !== "completed") ? (
                <>
                  <button
                    type="button"
                    onClick={() => setShowCompleted((value) => !value)}
                    className="flex h-11 w-full items-center gap-2 border-b border-stone-100 bg-stone-50 px-4 text-xs font-medium text-stone-600"
                  >
                    {showCompleted ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
                    已完成
                    <span className="text-stone-400">{completedTasks.length}</span>
                  </button>
                  {showCompleted && completedTasks.map(renderTask)}
                </>
              ) : (
                view.kind === "smart" && view.id === "completed" && completedTasks.map(renderTask)
              )}
              {visibleTasks.length === 0 && (
                <div className="grid min-h-52 place-items-center px-6 text-center">
                  <div>
                    <ListTodo className="mx-auto h-8 w-8 text-stone-300" />
                    <p className="mt-3 text-sm font-medium text-stone-500">这里还没有任务</p>
                    <p className="mt-1 text-xs text-stone-400">可从上方快速添加，或切换到其他列表。</p>
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </section>

      {selectedTask && (
        <TaskDetailsDrawer
          task={selectedTask}
          list={selectedTaskList}
          subtasks={selectedSubtasks}
          busy={busyIds.has(selectedTask.id)}
          onClose={() => {
            setOpenTaskId(null);
            onCloseRequestedTask();
          }}
          onSave={(changes) => updateTask(selectedTask.id, changes)}
          onToggle={toggleTask}
          onCreateSubtask={createSubtask}
          onDelete={deleteSelectedTask}
        />
      )}

      {createListOpen && (
        <div className="fixed inset-0 z-50 grid place-items-center bg-stone-950/25 p-4" role="presentation">
          <form
            onSubmit={createList}
            className="w-full max-w-sm rounded-lg border border-stone-200 bg-white p-5 shadow-xl"
            role="dialog"
            aria-modal="true"
            aria-label="新建任务列表"
          >
            <div className="mb-5 flex items-center justify-between">
              <h2 className="text-base font-semibold text-stone-900">新建列表</h2>
              <button
                type="button"
                onClick={() => setCreateListOpen(false)}
                className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100"
                title="关闭"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <input
              autoFocus
              value={newListName}
              onChange={(event) => setNewListName(event.target.value)}
              className="h-10 w-full rounded-md border border-stone-300 px-3 text-sm outline-none focus:border-emerald-600 focus:ring-2 focus:ring-emerald-100"
              placeholder="列表名称"
            />
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setCreateListOpen(false)}
                className="h-9 rounded-md px-3 text-sm text-stone-600 hover:bg-stone-100"
              >
                取消
              </button>
              <button
                type="submit"
                disabled={!newListName.trim() || busyIds.has("create-list")}
                className="h-9 rounded-md bg-[#4f7f68] px-4 text-sm font-medium text-white disabled:opacity-50"
              >
                创建
              </button>
            </div>
          </form>
        </div>
      )}
    </div>
  );
}
