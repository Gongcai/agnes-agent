export type AppFeatureId =
  | "chat"
  | "memory"
  | "knowledge"
  | "drive"
  | "calendar"
  | "tasks";

export interface AppFeatureDefinition {
  id: AppFeatureId;
  label: string;
  enabled: boolean;
}

export const APP_FEATURES: readonly AppFeatureDefinition[] = [
  { id: "chat", label: "聊天", enabled: true },
  { id: "memory", label: "记忆", enabled: false },
  { id: "knowledge", label: "知识库", enabled: false },
  { id: "drive", label: "网盘", enabled: false },
  { id: "calendar", label: "日历", enabled: false },
  { id: "tasks", label: "待办", enabled: false },
];

export const ENABLED_APP_FEATURES = APP_FEATURES.filter((feature) => feature.enabled);
