export type AppFeatureId =
  | "chat"
  | "reading"
  | "memory"
  | "knowledge"
  | "drive"
  | "calendar"
  | "tasks";

export type ChatMode = "home" | "code";

export interface AppFeatureDefinition {
  id: AppFeatureId;
  label: string;
  enabled: boolean;
}

export const APP_FEATURES: readonly AppFeatureDefinition[] = [
  { id: "chat", label: "聊天", enabled: true },
  { id: "reading", label: "阅读", enabled: true },
  { id: "memory", label: "记忆", enabled: false },
  { id: "knowledge", label: "知识库", enabled: true },
  { id: "drive", label: "网盘", enabled: true },
  { id: "calendar", label: "日历", enabled: true },
  { id: "tasks", label: "待办", enabled: true },
];

export const ENABLED_APP_FEATURES = APP_FEATURES.filter((feature) => feature.enabled);
