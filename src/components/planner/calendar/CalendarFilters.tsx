import { useState } from "react";
import { CalendarDays, CheckSquare2, CirclePlus, X } from "lucide-react";
import type { PlannerCalendar } from "../shared/types";

const colors = ["#4f8a6f", "#3b82a0", "#b7791f", "#9b5f86", "#c45d55", "#697386"];

interface CalendarFiltersProps {
  calendars: PlannerCalendar[];
  visibleIds: Set<string>;
  taskLayerVisible: boolean;
  busy: boolean;
  onToggleCalendar: (calendarId: string) => void;
  onToggleTaskLayer: () => void;
  onCreateCalendar: (name: string, color: string) => Promise<void>;
}

export function CalendarFilters({
  calendars,
  visibleIds,
  taskLayerVisible,
  busy,
  onToggleCalendar,
  onToggleTaskLayer,
  onCreateCalendar,
}: CalendarFiltersProps) {
  const [isCreating, setIsCreating] = useState(false);
  const [name, setName] = useState("");
  const [color, setColor] = useState(colors[0]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!name.trim()) return;
    try {
      await onCreateCalendar(name.trim(), color);
      setName("");
      setIsCreating(false);
    } catch {
      // The workspace owns the visible error state and keeps this draft open.
    }
  };

  return (
    <>
      <aside className="flex w-56 shrink-0 flex-col border-r border-stone-200 bg-white/45 px-3 py-4">
        <div className="mb-2 flex h-8 items-center justify-between px-2">
          <span className="text-[11px] font-semibold text-stone-500">我的日历</span>
          <button
            type="button"
            onClick={() => setIsCreating(true)}
            className="grid h-7 w-7 place-items-center rounded-md text-stone-500 hover:bg-stone-100"
            title="新建日历"
          >
            <CirclePlus className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-0.5">
          {calendars.map((calendar, index) => {
            const calendarColor = calendar.color || colors[index % colors.length];
            return (
              <label
                key={calendar.id}
                className="flex h-9 cursor-pointer items-center gap-2 rounded-md px-2 text-xs text-stone-700 hover:bg-stone-100"
              >
                <input
                  type="checkbox"
                  checked={visibleIds.has(calendar.id)}
                  onChange={() => onToggleCalendar(calendar.id)}
                  className="sr-only"
                />
                <span
                  className={`grid h-4 w-4 shrink-0 place-items-center rounded border ${
                    visibleIds.has(calendar.id) ? "border-transparent text-white" : "border-stone-300 bg-white"
                  }`}
                  style={visibleIds.has(calendar.id) ? { backgroundColor: calendarColor } : undefined}
                >
                  {visibleIds.has(calendar.id) && <span className="text-[10px] leading-none">✓</span>}
                </span>
                <CalendarDays className="h-3.5 w-3.5 shrink-0 text-stone-400" />
                <span className="truncate">{calendar.name}</span>
              </label>
            );
          })}
          {calendars.length === 0 && (
            <p className="px-2 py-3 text-xs leading-5 text-stone-400">新建一个日历后即可添加事件。</p>
          )}
        </div>

        <div className="my-3 border-t border-stone-200" />
        <label className="flex h-9 cursor-pointer items-center gap-2 rounded-md px-2 text-xs text-stone-700 hover:bg-stone-100">
          <input
            type="checkbox"
            checked={taskLayerVisible}
            onChange={onToggleTaskLayer}
            className="h-4 w-4 accent-stone-600"
          />
          <CheckSquare2 className="h-3.5 w-3.5 text-stone-400" />
          <span>待办</span>
        </label>
      </aside>

      {isCreating && (
        <div className="fixed inset-0 z-50 grid place-items-center bg-stone-950/25 p-4" role="presentation">
          <form
            onSubmit={submit}
            className="w-full max-w-sm rounded-lg border border-stone-200 bg-white p-5 shadow-xl"
            role="dialog"
            aria-modal="true"
            aria-label="新建日历"
          >
            <div className="mb-5 flex items-center justify-between">
              <h2 className="text-base font-semibold text-stone-900">新建日历</h2>
              <button
                type="button"
                onClick={() => setIsCreating(false)}
                className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100"
                title="关闭"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <label className="block text-xs font-medium text-stone-600">
              名称
              <input
                autoFocus
                value={name}
                onChange={(event) => setName(event.target.value)}
                className="mt-2 h-10 w-full rounded-md border border-stone-300 px-3 text-sm outline-none focus:border-emerald-600 focus:ring-2 focus:ring-emerald-100"
                placeholder="例如：工作"
              />
            </label>
            <fieldset className="mt-4">
              <legend className="text-xs font-medium text-stone-600">颜色</legend>
              <div className="mt-2 flex gap-2">
                {colors.map((option) => (
                  <button
                    key={option}
                    type="button"
                    onClick={() => setColor(option)}
                    className={`h-7 w-7 rounded-full border-2 ${
                      color === option ? "border-stone-700" : "border-transparent"
                    }`}
                    style={{ backgroundColor: option }}
                    title={option}
                  />
                ))}
              </div>
            </fieldset>
            <div className="mt-6 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setIsCreating(false)}
                className="h-9 rounded-md px-3 text-sm text-stone-600 hover:bg-stone-100"
              >
                取消
              </button>
              <button
                type="submit"
                disabled={busy || !name.trim()}
                className="h-9 rounded-md bg-[#4f7f68] px-4 text-sm font-medium text-white disabled:opacity-50"
              >
                创建
              </button>
            </div>
          </form>
        </div>
      )}
    </>
  );
}
