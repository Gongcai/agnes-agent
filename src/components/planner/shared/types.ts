export type PlannerMode = "calendar" | "tasks";

export interface PlannerCalendar {
  id: string;
  name: string;
  color: string | null;
  timezone: string;
}

export interface CalendarEvent {
  id: string;
  occurrence_id: string;
  calendar_id: string;
  title: string;
  starts_at: string;
  ends_at: string;
  timezone: string;
  all_day: boolean;
  recurrence_rule: string | null;
  original_occurrence: string | null;
  is_exception: boolean;
  status: string;
}

export interface TaskList {
  id: string;
  name: string;
  color: string | null;
}

export interface Task {
  id: string;
  task_list_id: string;
  parent_id: string | null;
  title: string;
  description: string | null;
  status: "open" | "completed" | "cancelled";
  priority: number;
  starts_at: string | null;
  due_date: string | null;
  due_at: string | null;
  due_timezone: string | null;
  is_important: boolean;
  my_day_date: string | null;
  completed_at: string | null;
  recurrence_rule: string | null;
  recurrence_anchor: string | null;
  recurrence_source_id: string | null;
  sort_order: number;
}

export type SmartTaskView = "my-day" | "important" | "planned" | "all" | "completed";

export type TaskView =
  | { kind: "smart"; id: SmartTaskView }
  | { kind: "list"; id: string };

export type RepeatOption = "none" | "daily" | "weekdays" | "weekly" | "monthly" | "yearly" | "custom";

export interface TaskUpdateChanges {
  title?: string;
  description?: string | null;
  priority?: number;
  dueDate?: string | null;
  dueAt?: string | null;
  dueTimezone?: string | null;
  isImportant?: boolean;
  myDayDate?: string | null;
  recurrenceRule?: string | null;
  sortOrder?: number;
}

export interface EventEditorValue {
  calendarId: string;
  title: string;
  startsAt: string;
  endsAt: string;
  timezone: string;
  allDay: boolean;
  recurrenceRule: string | null;
  scope: "occurrence" | "series";
}
