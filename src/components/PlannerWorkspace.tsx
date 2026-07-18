import { CalendarDays, CheckSquare2 } from "lucide-react";
import { CalendarWorkspace } from "./planner/calendar/CalendarWorkspace";
import { TodoWorkspace } from "./planner/tasks/TodoWorkspace";
import type { PlannerMode } from "./planner/shared/types";

interface PlannerWorkspaceProps {
  mode: PlannerMode;
  requestedTaskId: string | null;
  requestedEventId: string | null;
  onOpenTask: (taskId: string) => void;
  onCloseRequestedTask: () => void;
  onCloseRequestedEvent: () => void;
}

export function PlannerWorkspace({
  mode,
  requestedTaskId,
  requestedEventId,
  onOpenTask,
  onCloseRequestedTask,
  onCloseRequestedEvent,
}: PlannerWorkspaceProps) {
  const isCalendar = mode === "calendar";
  const HeaderIcon = isCalendar ? CalendarDays : CheckSquare2;

  return (
    <main className="agnes-feature-workspace agnes-planner-workspace flex h-full min-w-0 flex-1 flex-col bg-[#FAF9F5]">
      <header className="flex h-14 shrink-0 items-center border-b border-stone-200 bg-white/55 px-5">
        <span className="flex items-center gap-2 text-sm font-semibold text-stone-800">
          <HeaderIcon className="h-4 w-4 text-[#4f7f68]" />
          {isCalendar ? "日历" : "待办"}
        </span>
      </header>

      {isCalendar ? (
        <CalendarWorkspace
          requestedEventId={requestedEventId}
          onCloseRequestedEvent={onCloseRequestedEvent}
          onOpenTask={onOpenTask}
        />
      ) : (
        <TodoWorkspace
          requestedTaskId={requestedTaskId}
          onCloseRequestedTask={onCloseRequestedTask}
        />
      )}
    </main>
  );
}
