import { useState } from "react";
import { CalendarClock, Plus, Star, Sun } from "lucide-react";
import type { TaskList } from "../shared/types";

interface QuickAddValue {
  title: string;
  taskListId: string;
  dueDate: string | null;
  addToMyDay: boolean;
  isImportant: boolean;
}

interface QuickAddProps {
  lists: TaskList[];
  defaultListId: string | null;
  defaultDueDate?: string;
  defaultMyDay?: boolean;
  defaultImportant?: boolean;
  busy: boolean;
  onCreate: (value: QuickAddValue) => Promise<void>;
}

export function QuickAdd({
  lists,
  defaultListId,
  defaultDueDate = "",
  defaultMyDay = false,
  defaultImportant = false,
  busy,
  onCreate,
}: QuickAddProps) {
  const [title, setTitle] = useState("");
  const [listId, setListId] = useState(defaultListId ?? "");
  const [dueDate, setDueDate] = useState(defaultDueDate);
  const [addToMyDay, setAddToMyDay] = useState(defaultMyDay);
  const [isImportant, setIsImportant] = useState(defaultImportant);

  const effectiveListId = lists.some((list) => list.id === listId)
    ? listId
    : defaultListId || lists[0]?.id || "";

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!title.trim() || !effectiveListId) return;
    try {
      await onCreate({
        title: title.trim(),
        taskListId: effectiveListId,
        dueDate: dueDate || null,
        addToMyDay,
        isImportant,
      });
      setTitle("");
      setDueDate(defaultDueDate);
      setAddToMyDay(defaultMyDay);
      setIsImportant(defaultImportant);
    } catch {
      // The workspace owns the visible error state and keeps this draft intact.
    }
  };

  return (
    <form
      onSubmit={submit}
      className="border-y border-stone-200 bg-white px-4 py-3 shadow-[0_1px_2px_rgba(0,0,0,0.02)]"
    >
      <div className="flex min-h-10 items-center gap-3">
        <Plus className="h-5 w-5 shrink-0 text-[#4f7f68]" />
        <input
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          className="min-w-0 flex-1 bg-transparent text-sm text-stone-800 outline-none placeholder:text-stone-400"
          placeholder={lists.length ? "添加任务" : "先新建一个任务列表"}
          disabled={!lists.length}
        />
        <button
          type="button"
          onClick={() => setAddToMyDay((value) => !value)}
          className={`grid h-8 w-8 place-items-center rounded-md ${
            addToMyDay ? "bg-sky-50 text-sky-700" : "text-stone-400 hover:bg-stone-100"
          }`}
          title={addToMyDay ? "从我的一天移除" : "加入我的一天"}
        >
          <Sun className="h-4 w-4" />
        </button>
        <button
          type="button"
          onClick={() => setIsImportant((value) => !value)}
          className={`grid h-8 w-8 place-items-center rounded-md ${
            isImportant ? "text-amber-500" : "text-stone-400 hover:bg-stone-100"
          }`}
          title={isImportant ? "取消重要" : "标记为重要"}
        >
          <Star className={`h-4 w-4 ${isImportant ? "fill-current" : ""}`} />
        </button>
        <label className="relative grid h-8 w-8 cursor-pointer place-items-center rounded-md text-stone-400 hover:bg-stone-100" title="设置截止日期">
          <CalendarClock className={`h-4 w-4 ${dueDate ? "text-amber-700" : ""}`} />
          <input
            type="date"
            value={dueDate}
            onChange={(event) => setDueDate(event.target.value)}
            className="absolute inset-0 cursor-pointer opacity-0"
          />
        </label>
        <select
          value={effectiveListId}
          onChange={(event) => setListId(event.target.value)}
          disabled={!lists.length}
          className="h-8 max-w-36 rounded-md border border-stone-200 bg-stone-50 px-2 text-xs text-stone-600"
          aria-label="任务列表"
        >
          {lists.map((list) => (
            <option key={list.id} value={list.id}>
              {list.name}
            </option>
          ))}
        </select>
        <button
          type="submit"
          disabled={busy || !title.trim() || !effectiveListId}
          className="h-8 rounded-md bg-[#4f7f68] px-3 text-xs font-medium text-white disabled:opacity-40"
        >
          添加
        </button>
      </div>
    </form>
  );
}
