import React, { useState, useEffect } from "react";
import { X, User, Database, Sliders, ShieldCheck, ShieldOff, Key, Plus, Trash2, Pencil, Check, Zap, Server, Download, Eye, EyeOff, Terminal, Settings, Search, RefreshCw, GitCompareArrows, Laptop, Cloud, LockKeyhole, Copy, FileKey2, ArrowUp, ArrowDown, Globe2, BarChart3, Brain, Moon, Sun, HardDrive, Eraser, Gauge } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "../store/useAgentStore";
import type {
  AgentSummary,
  ModelCapabilities,
  ModelDescriptor,
  ModelModality,
  ModelProvider,
  ModelRoleAssignments,
} from "../store/useAgentStore";
import { AgentAvatar } from "./AgentAvatar";
import {
  embeddingModelName,
  formatMemoryTime,
  memoryEmbeddingProgress,
  memoryMatchesQuery,
  parseMemoryKeywords,
} from "../lib/memory";
import {
  beginSyncE2eeSetup,
  beginSyncE2eeRotation,
  approveSyncPairing,
  confirmSyncE2eeSetup,
  discardSyncE2eeSetup,
  finishSyncPairing,
  getSyncStatus,
  getSyncPairingRequest,
  joinSyncPairing,
  listSyncConflicts,
  listSyncDevices,
  resolveSyncConflict,
  revokeSyncDevice,
  restoreSyncE2ee,
  setSyncCredential,
  startSyncPairing,
  syncNow,
  type SyncConflict,
  type SyncDevice,
  type SyncRecoveryMaterial,
  type SyncPairingDevice,
  type SyncPairingInvite,
  type SyncPairingJoinStarted,
  type SyncStatus,
} from "../lib/ipc";
import {
  announceUIPreferenceChange,
  applyColorScheme,
  getCachedAutoFollowStreaming,
  getCachedAutoExpandThoughts,
  getCachedColorScheme,
  DEFAULT_MAX_OUTPUT_TOKENS,
  MAX_MAX_OUTPUT_TOKENS,
  MIN_MAX_OUTPUT_TOKENS,
  normalizeBooleanPreference,
  normalizeColorScheme,
  normalizeMaxOutputTokens,
  setAutoFollowStreaming,
  setAutoExpandThoughts,
  UI_AUTO_FOLLOW_STREAMING_KEY,
  UI_AUTO_EXPAND_THOUGHTS_KEY,
  UI_COLOR_SCHEME_KEY,
  UI_DEFAULT_MAX_OUTPUT_TOKENS_KEY,
  type ColorScheme,
} from "../lib/uiPreferences";

type SettingsTab = "general" | "agents" | "memory" | "storage" | "llm" | "tokens" | "web" | "mcp" | "audit" | "debug";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  initialTab?: SettingsTab;
}

interface AuditLog {
  id: string;
  time: string;
  tool: string;
  params: string;
  status: string;
  risk: string;
}

interface TokenUsageStats {
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  total_tokens: number;
}

interface StructuredMemory {
  id: string;
  agent_id: string;
  name: string;
  keywords: string[];
  content: string;
  creator: "user" | "ai";
  created_at: string;
  updated_at: string;
}

interface MemoryEmbeddingStatus {
  total: number;
  indexed: number;
  pending: number;
  model_ref: string | null;
}

interface MemoryVectorizationResult {
  indexed_now: number;
  status: MemoryEmbeddingStatus;
}

interface ArtifactStorageStatus {
  quotaBytes: number;
  usedBytes: number;
  outboxBytes: number;
  installedBytes: number;
  temporaryBytes: number;
  reclaimableBytes: number;
  localArtifactCount: number;
  overQuota: boolean;
}

interface ArtifactGcResult {
  reclaimedBytes: number;
  removedPaths: number;
  reconciledRecords: number;
  failedPaths: number;
  status: ArtifactStorageStatus;
}

interface MemoryFormValues {
  name: string;
  keywords: string;
  content: string;
}

const EMPTY_MEMORY_FORM: MemoryFormValues = {
  name: "",
  keywords: "",
  content: "",
};

const SYNC_ENTITY_LABELS: Record<string, string> = {
  agent: "角色卡",
  workspace: "工作区",
  session: "会话",
  message: "消息",
  explicit_memory: "必注入记忆",
  memory: "结构化记忆",
  calendar: "日历",
  calendar_event: "日历事件",
  event_exception: "事件例外",
  task_list: "待办列表",
  task: "待办",
};

function conflictPayloadPreview(
  payload: Record<string, unknown> | null,
  fields: string[],
  deleted: boolean,
): string {
  if (deleted) return "已删除";
  if (!payload) return "等待同步";
  const selected = Object.fromEntries(
    (fields.length > 0 ? fields : ["name", "title", "content"])
      .filter((field) => field in payload)
      .map((field) => [field, payload[field]]),
  );
  return JSON.stringify(Object.keys(selected).length > 0 ? selected : payload, null, 2);
}

function formatSyncDeviceTime(value: number | null): string {
  if (value == null) return "从未连接";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(value));
}

type ProviderKind = "openai" | "anthropic" | "ollama" | "openai_compatible" | "google";

const KIND_OPTIONS: { value: ProviderKind; label: string }[] = [
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Anthropic" },
  { value: "ollama", label: "Ollama" },
  { value: "openai_compatible", label: "OpenAI 兼容" },
  { value: "google", label: "Google" },
];

const KIND_BADGE_COLORS: Record<string, { bg: string; text: string; border: string }> = {
  openai: { bg: "bg-emerald-50", text: "text-emerald-700", border: "border-emerald-200/60" },
  anthropic: { bg: "bg-violet-50", text: "text-violet-700", border: "border-violet-200/60" },
  ollama: { bg: "bg-amber-50", text: "text-amber-700", border: "border-amber-200/60" },
  openai_compatible: { bg: "bg-blue-50", text: "text-blue-700", border: "border-blue-200/60" },
  google: { bg: "bg-red-50", text: "text-red-600", border: "border-red-200/60" },
};

const KIND_LABELS: Record<string, string> = {
  openai: "OpenAI",
  anthropic: "Anthropic",
  ollama: "Ollama",
  openai_compatible: "OpenAI 兼容",
  google: "Google",
};

const KIND_PLACEHOLDER_URL: Record<string, string> = {
  openai: "https://api.openai.com/v1 (默认)",
  anthropic: "https://api.anthropic.com (默认)",
  ollama: "http://localhost:11434",
  openai_compatible: "输入自定义 API 地址...",
  google: "https://generativelanguage.googleapis.com (默认)",
};

const KIND_EXAMPLE_MODELS: Record<string, string> = {
  openai: "gpt-4o, gpt-4o-mini, o3-mini",
  anthropic: "claude-sonnet-4-20250514, claude-3-5-haiku-20241022",
  ollama: "llama3, mistral, codellama",
  openai_compatible: "模型名称取决于服务商",
  google: "gemini-2.5-pro, gemini-2.5-flash",
};

interface ProviderFormValues {
  name: string;
  kind: ProviderKind;
  api_base: string;
  api_key: string;
  models: ModelDescriptor[];
  modelDraft: string;
  is_default: boolean;
}

interface SecretStoreStatus {
  available: boolean;
  backend: string;
  error: string | null;
}

type SearchProviderId = "duckduckgo" | "bing" | "searxng" | "brave";

interface SearchProviderSettings {
  fallback_order: SearchProviderId[];
  searxng_base_url: string | null;
  has_brave_api_key: boolean;
}

interface SearchProviderForm {
  fallbackOrder: SearchProviderId[];
  searxngBaseUrl: string;
  braveApiKey: string;
  hasBraveApiKey: boolean;
  clearBraveApiKey: boolean;
}

interface SearchProviderTestResult {
  success: boolean;
  provider: SearchProviderId;
  category: string | null;
  message: string;
  result_count: number;
  latency_ms: number;
}

const SEARCH_PROVIDER_LABELS: Record<SearchProviderId, string> = {
  duckduckgo: "DuckDuckGo HTML",
  bing: "Bing HTML",
  searxng: "SearXNG",
  brave: "Brave Search API",
};

const SEARCH_PROVIDER_IDS: SearchProviderId[] = ["duckduckgo", "bing", "searxng", "brave"];

const EMPTY_SEARCH_PROVIDER_FORM: SearchProviderForm = {
  fallbackOrder: ["duckduckgo", "bing"],
  searxngBaseUrl: "",
  braveApiKey: "",
  hasBraveApiKey: false,
  clearBraveApiKey: false,
};

interface McpEnvConfig {
  name: string;
  hasValue: boolean;
}

type McpServerTransport =
  | { type: "stdio"; command: string; args: string[]; env: McpEnvConfig[] }
  | { type: "streamable_http"; url: string; has_bearer_token: boolean };

interface McpServer {
  id: string;
  name: string;
  enabled: boolean;
  transport: McpServerTransport;
}

interface McpTestResult {
  serverName: string;
  toolCount: number;
  tools: string[];
}

interface McpFormValues {
  id: string | null;
  name: string;
  enabled: boolean;
  transportType: "stdio" | "streamable_http";
  command: string;
  argsText: string;
  env: Array<{ name: string; value: string; hasValue: boolean }>;
  url: string;
  bearerToken: string;
  hasBearerToken: boolean;
  clearBearerToken: boolean;
}

const EMPTY_MCP_FORM: McpFormValues = {
  id: null,
  name: "",
  enabled: true,
  transportType: "stdio",
  command: "",
  argsText: "",
  env: [],
  url: "",
  bearerToken: "",
  hasBearerToken: false,
  clearBearerToken: false,
};

const EMPTY_FORM: ProviderFormValues = {
  name: "",
  kind: "openai",
  api_base: "",
  api_key: "",
  models: [],
  modelDraft: "",
  is_default: false,
};

type ModelRoleField = Exclude<keyof ModelRoleAssignments, "fallback_models">;

const MODEL_ROLE_OPTIONS: {
  key: ModelRoleField;
  label: string;
  desc: string;
}[] = [
  { key: "main_model", label: "主模型", desc: "会话与角色均未指定模型时使用" },
  { key: "image_model", label: "图片处理模型", desc: "图片转自然语言、视觉理解和 OCR" },
  { key: "summary_model", label: "对话总结模型", desc: "滚动压缩历史对话，建议选择便宜模型" },
  { key: "memory_model", label: "记忆更新模型", desc: "抽取需要长期保存的事实和偏好" },
  { key: "speech_model", label: "语音理解模型", desc: "预留语音转文本调用入口" },
  { key: "quick_model", label: "快速模型", desc: "生成会话标题；预留划线翻译、搜索和名词解释" },
  { key: "embedding_model", label: "嵌入模型", desc: "生成本地记忆向量，维度由模型响应自动确定" },
];

function modelSupportsRole(model: ModelDescriptor, role: ModelRoleField): boolean {
  const { input_modalities: inputs, output_modalities: outputs, embedding } = model.capabilities;
  if (role === "embedding_model") return embedding;
  if (role === "image_model") return inputs.includes("image") && outputs.includes("text");
  if (role === "speech_model") return outputs.includes("text");
  return inputs.includes("text") && outputs.includes("text");
}

function capabilityLabels(capabilities: ModelCapabilities): string[] {
  const labels = [
    ...capabilities.input_modalities.map((modality) => `输入·${modality === "text" ? "文本" : "图像"}`),
    ...capabilities.output_modalities.map((modality) => `输出·${modality === "text" ? "文本" : "图像"}`),
  ];
  if (capabilities.embedding) labels.push("嵌入");
  return labels;
}

type ApprovalTier = "never" | "on_write" | "on_risk" | "always";
type WebSearchProvider = "auto" | SearchProviderId;

interface AgentToolToggle {
  enabled: boolean;
  approval: ApprovalTier;
  [key: string]: unknown;
}

interface AgentFormValues {
  id: string | null;
  name: string;
  persona: string;
  scenario: string;
  system_prompt: string;
  greeting: string;
  example_dialogue: string;
  model: string;
  tags: string;
  avatar: string;
  thinkingMode: string;
  thinkingBudget: number;
  toolPolicy: {
    shell: AgentToolToggle;
    file: AgentToolToggle;
    git: AgentToolToggle;
    memory: AgentToolToggle;
    planner: AgentToolToggle;
    web: AgentToolToggle & { search_provider: WebSearchProvider; timeout_sec?: number };
    mcp: AgentToolToggle & { server_ids: string[] };
    network: { allow: boolean; [key: string]: unknown };
    sandbox: {
      landlock: boolean;
      bwrap: "auto" | "disabled" | "required";
      rlimits: boolean;
      [key: string]: unknown;
    };
    [key: string]: unknown;
  };
}

interface DebugPromptMessage extends Record<string, unknown> {
  role?: string;
  content?: unknown;
  tool_calls?: unknown;
}

interface DebugPromptPreview {
  system_prompt: string;
  messages: DebugPromptMessage[];
  tools: Array<Record<string, unknown>>;
  discarded_count: number;
}

/// 思考模式/强度选项：off=关闭，auto=自动，low/medium/high=思考强度等级。
const THINKING_MODE_OPTIONS: { value: string; label: string; desc: string }[] = [
  { value: "off", label: "关闭", desc: "不启用思考" },
  { value: "auto", label: "自动", desc: "由模型决定思考深度" },
  { value: "low", label: "轻度", desc: "浅层思考，响应更快" },
  { value: "medium", label: "中等", desc: "常规思考深度" },
  { value: "high", label: "深度", desc: "深入推理，消耗更多 token" },
];

const DEFAULT_TOOL_POLICY: AgentFormValues["toolPolicy"] = {
  shell: { enabled: true, approval: "on_risk" },
  file: { enabled: true, approval: "on_write" },
  git: { enabled: true, approval: "on_risk" },
  memory: { enabled: true, approval: "on_write" },
  planner: { enabled: true, approval: "always" },
  web: { enabled: true, approval: "never", search_provider: "auto", timeout_sec: 15 },
  mcp: { enabled: false, approval: "always", server_ids: [] },
  network: { allow: true },
  sandbox: { landlock: true, bwrap: "auto", rlimits: true },
};

const APPROVAL_OPTIONS: { value: ApprovalTier; label: string }[] = [
  { value: "never", label: "自动执行" },
  { value: "on_write", label: "写入时审批" },
  { value: "on_risk", label: "高风险时审批" },
  { value: "always", label: "始终审批" },
];

const AGENT_EMOJIS: string[] = [
  "🤖", "🧑‍💻", "👩‍💻", "🦊", "🐱", "🐶", "🦁", "🐯",
  "🐼", "🐨", "🐧", "🦉", "🦄", "🐉", "🐲", "🦖",
  "👽", "🤡", "💡", "🔮", "⚡", "🔥", "🌟", "🌈",
  "📚", "✍️", "🎯", "🧠", "💬", "🗣️", "🫶", "😺",
  "🥷", "🦸", "🧙", "🧚", "👻", "🎭", "🛡️", "⚙️",
  "🌸", "🌿", "☕", "🍎", "🪐", "🌍", "🔭", "🧪",
];

const EMPTY_AGENT_FORM: AgentFormValues = {
  id: null,
  name: "",
  persona: "",
  scenario: "",
  system_prompt: "",
  greeting: "",
  example_dialogue: "",
  model: "",
  tags: "",
  avatar: "",
  thinkingMode: "off",
  thinkingBudget: 0,
  toolPolicy: DEFAULT_TOOL_POLICY,
};

/// 从 DB 中的 tool_policy JSON 安全解析为结构化开关（缺省字段回退默认）。
function parseToolPolicy(json?: string): AgentFormValues["toolPolicy"] {
  const base: AgentFormValues["toolPolicy"] = JSON.parse(JSON.stringify(DEFAULT_TOOL_POLICY));
  if (!json) return base;
  try {
    const obj = JSON.parse(json);
    if (obj && typeof obj === "object") {
      const knownKeys = new Set(["shell", "file", "git", "memory", "planner", "web", "mcp", "network", "sandbox"]);
      Object.entries(obj).forEach(([key, value]) => {
        if (!knownKeys.has(key)) base[key] = value;
      });
    }
    (["shell", "file", "git", "memory", "planner", "web", "mcp"] as const).forEach((k) => {
      const t = obj?.[k];
      if (t && typeof t === "object") {
        const legacyDefaults: Record<typeof k, ApprovalTier> = {
          shell: "on_risk",
          file: "on_write",
          git: "on_risk",
          memory: "on_write",
          planner: "always",
          web: "never",
          mcp: "always",
        };
        const rawApproval = t.approval;
        let approval = legacyDefaults[k];
        if (rawApproval === true || rawApproval === "always") approval = "always";
        else if (rawApproval === false || rawApproval === "never") approval = "never";
        else if (rawApproval === "write" || rawApproval === "on_write" || rawApproval === "on-write") approval = "on_write";
        else if (rawApproval === "push" || rawApproval === "on_risk" || rawApproval === "on-risk") approval = "on_risk";
        base[k] = { ...t, enabled: t.enabled !== false, approval };
      }
    });
    const searchProvider = ["auto", "duckduckgo", "bing", "searxng", "brave"].includes(obj?.web?.search_provider)
      ? obj.web.search_provider as WebSearchProvider
      : "auto";
    base.web = {
      ...base.web,
      search_provider: searchProvider,
      timeout_sec: typeof obj?.web?.timeout_sec === "number" ? obj.web.timeout_sec : 15,
    };
    base.mcp = {
      ...base.mcp,
      server_ids: Array.isArray(obj?.mcp?.server_ids)
        ? obj.mcp.server_ids.filter((id: unknown): id is string => typeof id === "string")
        : [],
    };
    const network = obj?.network;
    base.network = {
      ...(network && typeof network === "object" ? network : {}),
      allow: network?.allow !== false,
    };
    const sandbox = obj?.sandbox;
    const bwrap = ["auto", "disabled", "required"].includes(sandbox?.bwrap)
      ? sandbox.bwrap
      : "auto";
    base.sandbox = {
      ...DEFAULT_TOOL_POLICY.sandbox,
      ...(sandbox && typeof sandbox === "object" ? sandbox : {}),
      landlock: sandbox?.landlock !== false,
      rlimits: sandbox?.rlimits !== false,
      bwrap,
    };
  } catch {
    // 解析失败则使用默认策略
  }
  return base;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  onClose,
  initialTab = "agents",
}) => {
  const {
    agents,
    activeAgentId,
    activeSessionId,
    providers,
    modelRoles,
    loadProviders,
    loadModelRoles,
    setModelRoles,
    upsertProvider,
    deleteProvider,
    updateAgentModel,
    setActiveAgentId,
    upsertAgent,
    deleteAgent,
  } = useAgentStore();
  const [activeTab, setActiveTab] = useState<SettingsTab>(initialTab);
  
  // Memory MD state
  const [userMdText, setUserMdText] = useState("");
  const [memoryMdText, setMemoryMdText] = useState("");
  const [isEditingUserMd, setIsEditingUserMd] = useState(false);
  const [isEditingMemoryMd, setIsEditingMemoryMd] = useState(false);
  const [structuredMemories, setStructuredMemories] = useState<StructuredMemory[]>([]);
  const [memorySearch, setMemorySearch] = useState("");
  const [editingMemoryId, setEditingMemoryId] = useState<string | null>(null);
  const [memoryForm, setMemoryForm] = useState<MemoryFormValues>(EMPTY_MEMORY_FORM);
  const [isSavingMemory, setIsSavingMemory] = useState(false);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [memoryEmbeddingStatus, setMemoryEmbeddingStatus] = useState<MemoryEmbeddingStatus | null>(null);
  const [isVectorizingMemories, setIsVectorizingMemories] = useState(false);
  const [memoryVectorMessage, setMemoryVectorMessage] = useState<{ success: boolean; text: string } | null>(null);

  // Audit state
  const [auditLogs, setAuditLogs] = useState<AuditLog[]>([]);
  const [tokenUsageStats, setTokenUsageStats] = useState<TokenUsageStats | null>(null);
  const [tokenUsageScope, setTokenUsageScope] = useState<"all" | "agent">("all");
  const [tokenUsageLoading, setTokenUsageLoading] = useState(false);
  const [tokenUsageError, setTokenUsageError] = useState<string | null>(null);

  // Provider editor state
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null); // null = closed, "new" = adding, uuid = editing
  const [formValues, setFormValues] = useState<ProviderFormValues>(EMPTY_FORM);
  const [isSaving, setIsSaving] = useState(false);
  const [testResult, setTestResult] = useState<{ success: boolean; message: string } | null>(null);
  const [isTesting, setIsTesting] = useState(false);
  const [isFetchingModels, setIsFetchingModels] = useState(false);
  const [showApiKey, setShowApiKey] = useState(false);
  const [modelRoleForm, setModelRoleForm] = useState<ModelRoleAssignments>(modelRoles);
  const [isSavingModelRoles, setIsSavingModelRoles] = useState(false);
  const [modelRoleMessage, setModelRoleMessage] = useState<{ success: boolean; text: string } | null>(null);
  const [secretStoreStatus, setSecretStoreStatus] = useState<SecretStoreStatus | null>(null);
  const [mcpServers, setMcpServers] = useState<McpServer[]>([]);
  const [mcpForm, setMcpForm] = useState<McpFormValues>(EMPTY_MCP_FORM);
  const [editingMcpId, setEditingMcpId] = useState<string | null>(null);
  const [isSavingMcp, setIsSavingMcp] = useState(false);
  const [testingMcpId, setTestingMcpId] = useState<string | null>(null);
  const [mcpMessage, setMcpMessage] = useState<{ success: boolean; text: string } | null>(null);
  const [savedSearchProviderSettings, setSavedSearchProviderSettings] = useState<SearchProviderSettings | null>(null);
  const [searchProviderForm, setSearchProviderForm] = useState<SearchProviderForm>(EMPTY_SEARCH_PROVIDER_FORM);
  const [isSavingSearchProviders, setIsSavingSearchProviders] = useState(false);
  const [testingSearchProvider, setTestingSearchProvider] = useState<SearchProviderId | null>(null);
  const [searchProviderTests, setSearchProviderTests] = useState<Partial<Record<SearchProviderId, SearchProviderTestResult>>>({});
  const [searchProviderMessage, setSearchProviderMessage] = useState<{ success: boolean; text: string } | null>(null);
  const [showBraveSearchKey, setShowBraveSearchKey] = useState(false);
  const [syncStatus, setSyncStatus] = useState<SyncStatus | null>(null);
  const [syncStatusError, setSyncStatusError] = useState<string | null>(null);
  const [isSyncingNow, setIsSyncingNow] = useState(false);
  const [syncToken, setSyncToken] = useState("");
  const [showSyncToken, setShowSyncToken] = useState(false);
  const [isSavingSyncCredential, setIsSavingSyncCredential] = useState(false);
  const [syncConflicts, setSyncConflicts] = useState<SyncConflict[]>([]);
  const [resolvingConflictId, setResolvingConflictId] = useState<string | null>(null);
  const [syncDevices, setSyncDevices] = useState<SyncDevice[]>([]);
  const [revokingDeviceId, setRevokingDeviceId] = useState<string | null>(null);
  const [syncRecoveryMaterial, setSyncRecoveryMaterial] = useState<SyncRecoveryMaterial | null>(null);
  const [syncRecoveryAcknowledged, setSyncRecoveryAcknowledged] = useState(false);
  const [showSyncRestore, setShowSyncRestore] = useState(false);
  const [syncRecoveryKeyInput, setSyncRecoveryKeyInput] = useState("");
  const [syncRecoveryBundleInput, setSyncRecoveryBundleInput] = useState("");
  const [isConfiguringSyncE2ee, setIsConfiguringSyncE2ee] = useState(false);
  const [syncPairingInvite, setSyncPairingInvite] = useState<SyncPairingInvite | null>(null);
  const [syncPairingDevice, setSyncPairingDevice] = useState<SyncPairingDevice | null>(null);
  const [syncPairingCodeInput, setSyncPairingCodeInput] = useState("");
  const [syncPairingDeviceName, setSyncPairingDeviceName] = useState("新设备");
  const [syncPairingJoin, setSyncPairingJoin] = useState<SyncPairingJoinStarted | null>(null);
  const [isPairingSyncDevice, setIsPairingSyncDevice] = useState(false);

  // Debug prompt panel state
  const [debugPrompt, setDebugPrompt] = useState<DebugPromptPreview | null>(null);
  const [debugLoading, setDebugLoading] = useState(false);
  const [debugError, setDebugError] = useState<string | null>(null);

  // 角色卡编辑器状态：null = 关闭；"new" = 新建；uuid = 编辑对应角色卡
  const [editingAgentId, setEditingAgentId] = useState<string | null>(null);
  const [agentForm, setAgentForm] = useState<AgentFormValues>(EMPTY_AGENT_FORM);
  const [isSavingAgent, setIsSavingAgent] = useState(false);

  const openNewAgent = () => {
    setAgentForm(EMPTY_AGENT_FORM);
    setEditingAgentId("new");
  };

  const openEditAgent = (agent: AgentSummary) => {
    setAgentForm({
      id: agent.id,
      name: agent.name,
      persona: agent.persona || "",
      scenario: agent.scenario || "",
      system_prompt: agent.system_prompt || "",
      greeting: agent.greeting || "",
      example_dialogue: agent.example_dialogue || "",
      model: agent.model || "",
      tags: agent.tags || "",
      avatar: agent.avatar || "",
      thinkingMode: agent.thinking_mode || "off",
      thinkingBudget: agent.thinking_budget || 0,
      toolPolicy: parseToolPolicy(agent.tool_policy),
    });
    setEditingAgentId(agent.id);
  };

  const closeAgentEditor = () => {
    setEditingAgentId(null);
    setAgentForm(EMPTY_AGENT_FORM);
  };

  const saveAgent = async () => {
    setIsSavingAgent(true);
    try {
      const id = await upsertAgent({
        id: agentForm.id ?? undefined,
        name: agentForm.name.trim() || "未命名角色",
        persona: agentForm.persona,
        scenario: agentForm.scenario,
        system_prompt: agentForm.system_prompt,
        greeting: agentForm.greeting,
        example_dialogue: agentForm.example_dialogue,
        model: agentForm.model,
        tool_policy: JSON.stringify(agentForm.toolPolicy),
        avatar: agentForm.avatar,
        tags: agentForm.tags,
        thinking_mode: agentForm.thinkingMode,
        thinking_budget: agentForm.thinkingBudget,
      });
      closeAgentEditor();
      if (agentForm.id === null) {
        await setActiveAgentId(id);
      }
    } catch (e) {
      console.error("保存角色卡失败", e);
    } finally {
      setIsSavingAgent(false);
    }
  };

  const handleDeleteAgent = async (agentId: string, name: string) => {
    if (!window.confirm(`确定删除角色卡「${name}」吗？其所有会话与消息也会一并删除。`)) return;
    try {
      await deleteAgent(agentId);
      closeAgentEditor();
    } catch (e) {
      console.error("删除角色卡失败", e);
    }
  };

  // Render capability and approval controls for one tool group.
  const renderToolToggle = (key: "shell" | "file" | "git" | "memory" | "planner" | "web" | "mcp", label: string) => (
    <div className="flex items-center justify-between py-1.5 border-b border-stone-100 last:border-0">
      <span className="text-xs text-stone-700">{label}</span>
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() =>
            setAgentForm((f) => ({
              ...f,
              toolPolicy: {
                ...f.toolPolicy,
                [key]: { ...f.toolPolicy[key], enabled: !f.toolPolicy[key].enabled },
              },
            }))
          }
          className={`px-2 py-1 rounded-md text-[10px] font-semibold transition-colors ${
            agentForm.toolPolicy[key].enabled
              ? "bg-emerald-50 text-emerald-700 border border-emerald-200"
              : "bg-stone-100 text-stone-400 border border-stone-200"
          }`}
        >
          {agentForm.toolPolicy[key].enabled ? "已启用" : "已禁用"}
        </button>
        <select
          value={agentForm.toolPolicy[key].approval}
          onChange={(event) =>
            setAgentForm((f) => ({
              ...f,
              toolPolicy: {
                ...f.toolPolicy,
                [key]: { ...f.toolPolicy[key], approval: event.target.value as ApprovalTier },
              },
            }))
          }
          className="px-2 py-1 rounded-md text-[10px] font-semibold bg-amber-50 text-amber-700 border border-amber-200 outline-none"
        >
          {APPROVAL_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      </div>
    </div>
  );

  // ===== 角色卡头像：emoji 选择 + 图片上传 + 圆形裁剪 =====
  const [showEmojiPicker, setShowEmojiPicker] = useState(false);
  const [cropSrc, setCropSrc] = useState<string | null>(null);
  const [cropScale, setCropScale] = useState(1);
  const [cropX, setCropX] = useState(0);
  const [cropY, setCropY] = useState(0);
  const cropCanvasRef = React.useRef<HTMLCanvasElement | null>(null);
  const cropImgRef = React.useRef<HTMLImageElement | null>(null);
  const cropDragRef = React.useRef<{ x: number; y: number; ox: number; oy: number } | null>(null);
  const fileInputRef = React.useRef<HTMLInputElement | null>(null);

  const CROP_SIZE = 256;

  const drawCrop = React.useCallback(() => {
    const canvas = cropCanvasRef.current;
    const img = cropImgRef.current;
    if (!canvas || !img) return;
    const S = CROP_SIZE;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, S, S);
    ctx.save();
    ctx.beginPath();
    ctx.arc(S / 2, S / 2, S / 2, 0, Math.PI * 2);
    ctx.clip();
    const baseScale = Math.max(S / img.naturalWidth, S / img.naturalHeight);
    const total = baseScale * cropScale;
    const drawW = img.naturalWidth * total;
    const drawH = img.naturalHeight * total;
    const lowerX = (S - drawW) / 2;
    const lowerY = (S - drawH) / 2;
    const x = lowerX + cropX;
    const y = lowerY + cropY;
    ctx.drawImage(img, x, y, drawW, drawH);
    ctx.restore();
  }, [cropScale, cropX, cropY]);

  // 上传文件 → 进入裁剪
  const onAvatarFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = reader.result as string;
      setCropSrc(dataUrl);
      setCropScale(1);
      setCropX(0);
      setCropY(0);
    };
    reader.readAsDataURL(file);
    e.target.value = "";
  };

  // 图片加载后缓存并首次绘制
  React.useEffect(() => {
    if (!cropSrc) return;
    const img = new Image();
    img.onload = () => {
      cropImgRef.current = img;
      drawCrop();
    };
    img.src = cropSrc;
  }, [cropSrc, drawCrop]);

  // 缩放/拖拽变化时重绘
  React.useEffect(() => {
    drawCrop();
  }, [cropScale, cropX, cropY, drawCrop]);

  const onCropPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    cropDragRef.current = { x: e.clientX, y: e.clientY, ox: cropX, oy: cropY };
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
  };

  const onCropPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const drag = cropDragRef.current;
    const canvas = cropCanvasRef.current;
    const img = cropImgRef.current;
    if (!drag || !canvas || !img) return;
    const S = CROP_SIZE;
    const baseScale = Math.max(S / img.naturalWidth, S / img.naturalHeight);
    const total = baseScale * cropScale;
    const drawW = img.naturalWidth * total;
    const drawH = img.naturalHeight * total;
    const lowerX = (S - drawW) / 2;
    const lowerY = (S - drawH) / 2;
    const upperX = (drawW - S) / 2;
    const upperY = (drawH - S) / 2;
    const nx = Math.min(upperX, Math.max(lowerX, drag.ox + (e.clientX - drag.x)));
    const ny = Math.min(upperY, Math.max(lowerY, drag.oy + (e.clientY - drag.y)));
    setCropX(nx);
    setCropY(ny);
  };

  const onCropPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    cropDragRef.current = null;
    (e.target as HTMLElement).releasePointerCapture?.(e.pointerId);
  };

  const confirmCrop = () => {
    const canvas = cropCanvasRef.current;
    if (!canvas) return;
    const dataUrl = canvas.toDataURL("image/png");
    setAgentForm((f) => ({ ...f, avatar: dataUrl }));
    setCropSrc(null);
  };

  const clearAvatar = () => {
    setAgentForm((f) => ({ ...f, avatar: "" }));
    setShowEmojiPicker(false);
  };

  const loadDebugPrompt = () => {
    if (!activeAgentId) return;
    setDebugLoading(true);
    setDebugError(null);
    invoke<DebugPromptPreview>("get_debug_prompt", {
      agentId: activeAgentId,
      sessionId: activeSessionId ?? null,
    })
      .then((res) => setDebugPrompt(res))
      .catch((e) => {
        setDebugError(e?.toString() || "获取提示词失败");
        setDebugPrompt(null);
      })
      .finally(() => setDebugLoading(false));
  };
  
  // Model selection modal state
  const [isModelSelectOpen, setIsModelSelectOpen] = useState(false);
  const [availableModels, setAvailableModels] = useState<ModelDescriptor[]>([]);
  const [selectedModels, setSelectedModels] = useState<Set<string>>(new Set());

  // Sync memory text when activeAgentId changes
  useEffect(() => {
    if (activeAgentId && activeTab === "memory") {
      setMemoryEmbeddingStatus(null);
      setMemoryVectorMessage(null);
      Promise.all([
        invoke<{ user_md: string; memory_md: string }>("get_explicit_memories", {
          agentId: activeAgentId,
        }),
        invoke<StructuredMemory[]>("list_memories", { agentId: activeAgentId }),
        invoke<MemoryEmbeddingStatus>("get_memory_embedding_status", { agentId: activeAgentId }),
      ])
        .then(([explicit, memories, embeddingStatus]) => {
          setUserMdText(explicit.user_md);
          setMemoryMdText(explicit.memory_md);
          setStructuredMemories(memories);
          setMemoryEmbeddingStatus(embeddingStatus);
          setIsEditingUserMd(false);
          setIsEditingMemoryMd(false);
          setEditingMemoryId(null);
          setMemoryError(null);
        })
        .catch((error) => {
          console.error(error);
          setMemoryVectorMessage({ success: false, text: String(error) });
        });
    }
  }, [activeAgentId, activeTab]);

  // Load audit logs when tab is switched to audit
  useEffect(() => {
    if (activeSessionId && activeTab === "audit") {
      invoke<AuditLog[]>("list_audit_logs", { sessionId: activeSessionId })
        .then((res) => {
          setAuditLogs(res);
        })
        .catch(console.error);
    }
  }, [activeSessionId, activeTab]);

  const loadTokenUsageStats = React.useCallback(() => {
    if (tokenUsageScope === "agent" && !activeAgentId) return Promise.resolve();
    setTokenUsageLoading(true);
    setTokenUsageError(null);
    return invoke<TokenUsageStats>("get_token_usage_stats", {
      agentId: tokenUsageScope === "agent" ? activeAgentId : null,
    })
      .then(setTokenUsageStats)
      .catch((error) => {
        setTokenUsageStats(null);
        setTokenUsageError(String(error));
      })
      .finally(() => setTokenUsageLoading(false));
  }, [activeAgentId, tokenUsageScope]);

  useEffect(() => {
    if (!isOpen || activeTab !== "tokens") return;
    void loadTokenUsageStats();
  }, [activeTab, isOpen, loadTokenUsageStats]);

  // Load providers when LLM tab is activated
  useEffect(() => {
    if (activeTab === "llm") {
      loadProviders();
      loadModelRoles();
      invoke<SecretStoreStatus>("get_secret_store_status")
        .then(setSecretStoreStatus)
        .catch((error) => {
          setSecretStoreStatus({ available: false, backend: "OS Keyring", error: String(error) });
        });
    }
  }, [activeTab, loadProviders, loadModelRoles]);

  const loadMcpServers = React.useCallback(() => {
    return invoke<McpServer[]>("list_mcp_servers")
      .then(setMcpServers)
      .catch((error) => {
        setMcpMessage({ success: false, text: String(error) });
      });
  }, []);

  const loadSearchProviderSettings = React.useCallback(() => {
    return invoke<SearchProviderSettings>("get_search_provider_settings")
      .then((settings) => {
        setSavedSearchProviderSettings(settings);
        setSearchProviderForm({
          fallbackOrder: settings.fallback_order,
          searxngBaseUrl: settings.searxng_base_url || "",
          braveApiKey: "",
          hasBraveApiKey: settings.has_brave_api_key,
          clearBraveApiKey: false,
        });
      })
      .catch((error) => {
        setSearchProviderMessage({ success: false, text: String(error) });
      });
  }, []);

  useEffect(() => {
    if (!isOpen || (activeTab !== "mcp" && activeTab !== "agents")) return;
    void loadMcpServers();
    if (activeTab === "mcp") {
      invoke<SecretStoreStatus>("get_secret_store_status")
        .then(setSecretStoreStatus)
        .catch((error) => {
          setSecretStoreStatus({ available: false, backend: "OS Keyring", error: String(error) });
        });
    }
  }, [activeTab, isOpen, loadMcpServers]);

  useEffect(() => {
    if (!isOpen || (activeTab !== "web" && activeTab !== "agents")) return;
    void loadSearchProviderSettings();
    if (activeTab === "web") {
      invoke<SecretStoreStatus>("get_secret_store_status")
        .then(setSecretStoreStatus)
        .catch((error) => {
          setSecretStoreStatus({ available: false, backend: "OS Keyring", error: String(error) });
        });
    }
  }, [activeTab, isOpen, loadSearchProviderSettings]);

  const saveSearchProviders = async () => {
    setIsSavingSearchProviders(true);
    setSearchProviderMessage(null);
    try {
      await invoke("set_search_provider_settings", {
        input: {
          fallback_order: searchProviderForm.fallbackOrder,
          searxng_base_url: searchProviderForm.searxngBaseUrl.trim() || null,
          brave_api_key: searchProviderForm.braveApiKey.trim() || null,
          clear_brave_api_key: searchProviderForm.clearBraveApiKey,
        },
      });
      await loadSearchProviderSettings();
      setSearchProviderTests({});
      setSearchProviderMessage({ success: true, text: "搜索 Provider 配置已保存" });
    } catch (error) {
      setSearchProviderMessage({ success: false, text: String(error) });
    } finally {
      setIsSavingSearchProviders(false);
    }
  };

  const testSearchProvider = async (provider: SearchProviderId) => {
    setTestingSearchProvider(provider);
    try {
      const result = await invoke<SearchProviderTestResult>("test_search_provider", { providerId: provider });
      setSearchProviderTests((current) => ({ ...current, [provider]: result }));
    } catch (error) {
      setSearchProviderTests((current) => ({
        ...current,
        [provider]: {
          success: false,
          provider,
          category: "command_error",
          message: String(error),
          result_count: 0,
          latency_ms: 0,
        },
      }));
    } finally {
      setTestingSearchProvider(null);
    }
  };

  const openNewMcpServer = () => {
    setMcpForm(EMPTY_MCP_FORM);
    setEditingMcpId("new");
    setMcpMessage(null);
  };

  const openEditMcpServer = (server: McpServer) => {
    const transport = server.transport;
    setMcpForm({
      ...EMPTY_MCP_FORM,
      id: server.id,
      name: server.name,
      enabled: server.enabled,
      transportType: transport.type,
      command: transport.type === "stdio" ? transport.command : "",
      argsText: transport.type === "stdio" ? transport.args.join("\n") : "",
      env: transport.type === "stdio"
        ? transport.env.map((item) => ({ ...item, value: "" }))
        : [],
      url: transport.type === "streamable_http" ? transport.url : "",
      hasBearerToken: transport.type === "streamable_http" && transport.has_bearer_token,
      bearerToken: "",
      clearBearerToken: false,
    });
    setEditingMcpId(server.id);
    setMcpMessage(null);
  };

  const mcpInputFromForm = (form: McpFormValues) => ({
    id: form.id,
    name: form.name.trim(),
    enabled: form.enabled,
    transport: form.transportType === "stdio"
      ? {
          type: "stdio" as const,
          command: form.command.trim(),
          args: form.argsText.split("\n").map((arg) => arg.trim()).filter(Boolean),
          env: form.env
            .map((item) => ({
              name: item.name.trim(),
              value: item.value.length > 0 ? item.value : undefined,
            }))
            .filter((item) => item.name.length > 0),
        }
      : {
          type: "streamable_http" as const,
          url: form.url.trim(),
          bearer_token: form.clearBearerToken
            ? ""
            : form.bearerToken.length > 0
              ? form.bearerToken
              : undefined,
        },
  });

  const saveMcpServer = async () => {
    setIsSavingMcp(true);
    setMcpMessage(null);
    try {
      await invoke<string>("upsert_mcp_server", { server: mcpInputFromForm(mcpForm) });
      setEditingMcpId(null);
      setMcpForm(EMPTY_MCP_FORM);
      await loadMcpServers();
      setMcpMessage({ success: true, text: "MCP Server 已保存" });
    } catch (error) {
      setMcpMessage({ success: false, text: String(error) });
    } finally {
      setIsSavingMcp(false);
    }
  };

  const deleteMcpServer = async (server: McpServer) => {
    if (!window.confirm(`确定删除 MCP Server「${server.name}」及其本机凭证吗？`)) return;
    try {
      await invoke("delete_mcp_server", { serverId: server.id });
      if (editingMcpId === server.id) setEditingMcpId(null);
      await loadMcpServers();
      setMcpMessage({ success: true, text: "MCP Server 已删除" });
    } catch (error) {
      setMcpMessage({ success: false, text: String(error) });
    }
  };

  const testMcpServer = async (server: McpServer) => {
    setTestingMcpId(server.id);
    setMcpMessage(null);
    try {
      const result = await invoke<McpTestResult>("test_mcp_server", { serverId: server.id });
      const preview = result.tools.slice(0, 6).join("、");
      setMcpMessage({
        success: true,
        text: `${result.serverName} 连接成功，共 ${result.toolCount} 个工具${preview ? `：${preview}` : ""}`,
      });
    } catch (error) {
      setMcpMessage({ success: false, text: String(error) });
    } finally {
      setTestingMcpId(null);
    }
  };

  const toggleMcpServer = async (server: McpServer) => {
    const form: McpFormValues = server.transport.type === "stdio"
      ? {
          ...EMPTY_MCP_FORM,
          id: server.id,
          name: server.name,
          enabled: !server.enabled,
          transportType: "stdio",
          command: server.transport.command,
          argsText: server.transport.args.join("\n"),
          env: server.transport.env.map((item) => ({ ...item, value: "" })),
        }
      : {
          ...EMPTY_MCP_FORM,
          id: server.id,
          name: server.name,
          enabled: !server.enabled,
          transportType: "streamable_http",
          url: server.transport.url,
          hasBearerToken: server.transport.has_bearer_token,
        };
    try {
      await invoke<string>("upsert_mcp_server", { server: mcpInputFromForm(form) });
      await loadMcpServers();
    } catch (error) {
      setMcpMessage({ success: false, text: String(error) });
    }
  };

  useEffect(() => {
    if (!isOpen || activeTab !== "llm") return;
    let active = true;
    const refresh = () => {
      void getSyncStatus()
        .then(async (status) => {
          const [conflicts, devices] = await Promise.all([
            listSyncConflicts(),
            status.credentialConfigured ? listSyncDevices() : Promise.resolve([]),
          ]);
          if (!active) return;
          setSyncStatus(status);
          setSyncConflicts(conflicts);
          setSyncDevices(devices);
          setSyncStatusError(null);
        })
        .catch((error) => {
          if (active) setSyncStatusError(String(error));
        });
    };
    refresh();
    const timer = window.setInterval(refresh, 5_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [activeTab, isOpen]);

  useEffect(() => {
    if (isOpen && activeTab === "llm") return;
    setSyncRecoveryMaterial(null);
    setSyncRecoveryAcknowledged(false);
    setShowSyncRestore(false);
    setSyncRecoveryKeyInput("");
    setSyncRecoveryBundleInput("");
    setSyncPairingInvite(null);
    setSyncPairingDevice(null);
    setSyncPairingCodeInput("");
    setSyncPairingJoin(null);
  }, [activeTab, isOpen]);

  const handleSyncNow = async () => {
    setIsSyncingNow(true);
    setSyncStatusError(null);
    try {
      const status = await syncNow();
      const [conflicts, devices] = await Promise.all([
        listSyncConflicts(),
        status.credentialConfigured ? listSyncDevices() : Promise.resolve([]),
      ]);
      setSyncStatus(status);
      setSyncConflicts(conflicts);
      setSyncDevices(devices);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsSyncingNow(false);
    }
  };

  const handleRevokeSyncDevice = async (device: SyncDevice) => {
    if (device.current || device.revokedAt != null) return;
    if (!window.confirm(`确定撤销设备“${device.name}”吗？撤销只会阻止后续联网；若设备可能失控，请随后立即轮换密钥。`)) return;
    setRevokingDeviceId(device.id);
    setSyncStatusError(null);
    try {
      const revoked = await revokeSyncDevice(device.id);
      setSyncDevices((devices) =>
        devices.map((current) => (current.id === revoked.id ? revoked : current)),
      );
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setRevokingDeviceId(null);
    }
  };

  const handleResolveSyncConflict = async (
    conflict: SyncConflict,
    resolution: "keep_local" | "keep_remote",
  ) => {
    const target = resolution === "keep_local" ? "本机版本" : "云端版本";
    if (!window.confirm(`确定采用${target}解决此同步冲突吗？`)) return;
    setResolvingConflictId(conflict.id);
    setSyncStatusError(null);
    try {
      await resolveSyncConflict(conflict.id, resolution);
      const [status, conflicts] = await Promise.all([getSyncStatus(), listSyncConflicts()]);
      setSyncStatus(status);
      setSyncConflicts(conflicts);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setResolvingConflictId(null);
    }
  };

  const handleSaveSyncCredential = async () => {
    if (!syncToken.trim()) return;
    setIsSavingSyncCredential(true);
    setSyncStatusError(null);
    try {
      const status = await setSyncCredential({ kind: "bearer", token: syncToken });
      setSyncStatus(status);
      setSyncDevices(await listSyncDevices());
      setSyncToken("");
      setShowSyncToken(false);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsSavingSyncCredential(false);
    }
  };

  const handleClearSyncCredential = async () => {
    setIsSavingSyncCredential(true);
    setSyncStatusError(null);
    try {
      setSyncStatus(await setSyncCredential(null));
      setSyncToken("");
      setSyncDevices([]);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsSavingSyncCredential(false);
    }
  };

  const handleBeginSyncE2eeSetup = async () => {
    setIsConfiguringSyncE2ee(true);
    setSyncStatusError(null);
    try {
      const material = await beginSyncE2eeSetup();
      setSyncRecoveryMaterial(material);
      setSyncRecoveryAcknowledged(false);
      setShowSyncRestore(false);
      setSyncStatus(await getSyncStatus());
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsConfiguringSyncE2ee(false);
    }
  };

  const handleBeginSyncE2eeRotation = async () => {
    if (!window.confirm("轮换后，新写入将立即改用新密钥；其他设备需通过配对或本次恢复材料升级后才能读取。继续吗？")) return;
    setIsConfiguringSyncE2ee(true);
    setSyncStatusError(null);
    try {
      const material = await beginSyncE2eeRotation();
      setSyncRecoveryMaterial(material);
      setSyncRecoveryAcknowledged(false);
      setShowSyncRestore(false);
      setSyncStatus(await getSyncStatus());
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsConfiguringSyncE2ee(false);
    }
  };

  const handleConfirmSyncE2eeSetup = async () => {
    if (!syncRecoveryAcknowledged) return;
    setIsConfiguringSyncE2ee(true);
    setSyncStatusError(null);
    try {
      setSyncStatus(await confirmSyncE2eeSetup());
      setSyncRecoveryMaterial(null);
      setSyncRecoveryAcknowledged(false);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsConfiguringSyncE2ee(false);
    }
  };

  const handleRestoreSyncE2ee = async () => {
    const recoveryKey = syncRecoveryKeyInput.trim();
    const recoveryBundle = syncRecoveryBundleInput.trim();
    if (!recoveryKey || !recoveryBundle) return;
    setIsConfiguringSyncE2ee(true);
    setSyncStatusError(null);
    try {
      setSyncStatus(await restoreSyncE2ee(recoveryKey, recoveryBundle));
      setSyncRecoveryKeyInput("");
      setSyncRecoveryBundleInput("");
      setShowSyncRestore(false);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsConfiguringSyncE2ee(false);
    }
  };

  const handleCopyRecoveryValue = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setSyncStatusError(null);
    } catch (error) {
      setSyncStatusError(String(error));
    }
  };

  const handleDiscardSyncE2eeSetup = async () => {
    if (!window.confirm("确定丢弃尚未确认的本机加密密钥吗？")) return;
    setIsConfiguringSyncE2ee(true);
    setSyncStatusError(null);
    try {
      setSyncStatus(await discardSyncE2eeSetup());
      setSyncRecoveryMaterial(null);
      setSyncRecoveryAcknowledged(false);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsConfiguringSyncE2ee(false);
    }
  };

  const handleStartSyncPairing = async () => {
    setIsPairingSyncDevice(true);
    setSyncStatusError(null);
    try {
      setSyncPairingInvite(await startSyncPairing());
      setSyncPairingDevice(null);
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsPairingSyncDevice(false);
    }
  };

  const handleCheckSyncPairing = async () => {
    if (!syncPairingInvite) return;
    setIsPairingSyncDevice(true);
    setSyncStatusError(null);
    try {
      setSyncPairingDevice(await getSyncPairingRequest(syncPairingInvite.sessionId));
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsPairingSyncDevice(false);
    }
  };

  const handleApproveSyncPairing = async () => {
    if (!syncPairingInvite || !syncPairingDevice) return;
    if (!window.confirm(`允许“${syncPairingDevice.deviceName}”获取同步凭证与当前完整密钥集吗？`)) return;
    setIsPairingSyncDevice(true);
    setSyncStatusError(null);
    try {
      await approveSyncPairing(syncPairingInvite.sessionId);
      setSyncPairingInvite(null);
      setSyncPairingDevice(null);
      setSyncDevices(await listSyncDevices());
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsPairingSyncDevice(false);
    }
  };

  const handleJoinSyncPairing = async () => {
    const pairingCode = syncPairingCodeInput.trim();
    const deviceName = syncPairingDeviceName.trim();
    if (!pairingCode || !deviceName) return;
    setIsPairingSyncDevice(true);
    setSyncStatusError(null);
    try {
      setSyncPairingJoin(await joinSyncPairing(pairingCode, deviceName));
    } catch (error) {
      setSyncStatusError(String(error));
    } finally {
      setIsPairingSyncDevice(false);
    }
  };

  useEffect(() => {
    if (!syncPairingJoin) return;
    let active = true;
    const finish = async () => {
      if (Date.now() >= syncPairingJoin.expiresAt) {
        if (active) {
          setSyncStatusError("配对会话已过期，请重新生成配对码");
          setSyncPairingJoin(null);
        }
        return;
      }
      try {
        const completion = await finishSyncPairing(syncPairingJoin.sessionId);
        if (!active || completion.status !== "complete" || !completion.syncStatus) return;
        setSyncStatus(completion.syncStatus);
        setSyncPairingCodeInput("");
        setSyncPairingJoin(null);
        setSyncStatusError(null);
        setSyncDevices(await listSyncDevices());
      } catch (error) {
        if (active) setSyncStatusError(String(error));
      }
    };
    void finish();
    const timer = window.setInterval(() => void finish(), 2_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [syncPairingJoin]);

  useEffect(() => {
    setModelRoleForm(modelRoles);
  }, [modelRoles]);

  if (!isOpen) return null;

  const handleSaveUserMd = () => {
    if (!activeAgentId) return;
    invoke("save_explicit_memories", {
      agentId: activeAgentId,
      userMd: userMdText,
      memoryMd: memoryMdText,
    })
      .then(() => setIsEditingUserMd(false))
      .catch(console.error);
  };

  const handleSaveMemoryMd = () => {
    if (!activeAgentId) return;
    invoke("save_explicit_memories", {
      agentId: activeAgentId,
      userMd: userMdText,
      memoryMd: memoryMdText,
    })
      .then(() => setIsEditingMemoryMd(false))
      .catch(console.error);
  };

  const refreshStructuredMemories = async () => {
    if (!activeAgentId) return;
    const [memories, embeddingStatus] = await Promise.all([
      invoke<StructuredMemory[]>("list_memories", { agentId: activeAgentId }),
      invoke<MemoryEmbeddingStatus>("get_memory_embedding_status", { agentId: activeAgentId }),
    ]);
    setStructuredMemories(memories);
    setMemoryEmbeddingStatus(embeddingStatus);
    setMemoryVectorMessage(null);
  };

  const vectorizeStructuredMemories = async () => {
    if (!activeAgentId || isVectorizingMemories) return;
    setIsVectorizingMemories(true);
    setMemoryVectorMessage(null);
    try {
      const result = await invoke<MemoryVectorizationResult>("vectorize_memories", {
        agentId: activeAgentId,
      });
      setMemoryEmbeddingStatus(result.status);
      setMemoryVectorMessage({
        success: true,
        text: result.indexed_now > 0
          ? `本次完成 ${result.indexed_now} 条记忆向量化`
          : "当前记忆向量索引已是最新状态",
      });
    } catch (error) {
      setMemoryVectorMessage({ success: false, text: String(error) });
      invoke<MemoryEmbeddingStatus>("get_memory_embedding_status", { agentId: activeAgentId })
        .then(setMemoryEmbeddingStatus)
        .catch(console.error);
    } finally {
      setIsVectorizingMemories(false);
    }
  };

  const openNewMemory = () => {
    setEditingMemoryId("new");
    setMemoryForm(EMPTY_MEMORY_FORM);
    setMemoryError(null);
  };

  const openMemoryEditor = (memory: StructuredMemory) => {
    setEditingMemoryId(memory.id);
    setMemoryForm({
      name: memory.name,
      keywords: memory.keywords.join(", "),
      content: memory.content,
    });
    setMemoryError(null);
  };

  const closeMemoryEditor = () => {
    setEditingMemoryId(null);
    setMemoryForm(EMPTY_MEMORY_FORM);
    setMemoryError(null);
  };

  const saveStructuredMemory = async () => {
    if (!activeAgentId || !editingMemoryId) return;
    if (!memoryForm.name.trim() || !memoryForm.content.trim()) {
      setMemoryError("名称和记忆内容不能为空");
      return;
    }
    const keywords = parseMemoryKeywords(memoryForm.keywords);
    setIsSavingMemory(true);
    setMemoryError(null);
    try {
      if (editingMemoryId === "new") {
        await invoke("create_memory", {
          agentId: activeAgentId,
          name: memoryForm.name,
          keywords,
          content: memoryForm.content,
        });
      } else {
        await invoke("update_memory", {
          memoryId: editingMemoryId,
          agentId: activeAgentId,
          name: memoryForm.name,
          keywords,
          content: memoryForm.content,
        });
      }
      await refreshStructuredMemories();
      closeMemoryEditor();
    } catch (error) {
      setMemoryError(String(error));
    } finally {
      setIsSavingMemory(false);
    }
  };

  const removeStructuredMemory = async (memory: StructuredMemory) => {
    if (!activeAgentId || !window.confirm(`确定删除记忆“${memory.name}”吗？`)) return;
    try {
      await invoke("delete_memory", {
        memoryId: memory.id,
        agentId: activeAgentId,
      });
      await refreshStructuredMemories();
      if (editingMemoryId === memory.id) closeMemoryEditor();
    } catch (error) {
      setMemoryError(String(error));
    }
  };

  const visibleMemories = structuredMemories.filter((memory) =>
    memoryMatchesQuery(memory, memorySearch),
  );
  const embeddingProgress = memoryEmbeddingStatus
    ? memoryEmbeddingProgress(memoryEmbeddingStatus.indexed, memoryEmbeddingStatus.total)
    : 0;

  // --- Provider editor helpers ---
  const openAddProvider = () => {
    setEditingProviderId("new");
    setFormValues(EMPTY_FORM);
    setShowApiKey(false);
    setTestResult(null);
  };

  const openEditProvider = (provider: ModelProvider) => {
    setEditingProviderId(provider.id);
    setFormValues({
      name: provider.name,
      kind: provider.kind as ProviderKind,
      api_base: provider.api_base || "",
      api_key: "",
      models: provider.models.map((model) => ({
        ...model,
        capabilities: {
          ...model.capabilities,
          input_modalities: [...model.capabilities.input_modalities],
          output_modalities: [...model.capabilities.output_modalities],
        },
      })),
      modelDraft: "",
      is_default: provider.is_default,
    });
    setShowApiKey(false);
    setTestResult(null);
  };

  const closeEditor = () => {
    setEditingProviderId(null);
    setFormValues(EMPTY_FORM);
    setShowApiKey(false);
    setTestResult(null);
    setIsTesting(false);
  };

  const handleSaveProvider = async () => {
    setIsSaving(true);
    try {
      const payload: any = {
        name: formValues.name,
        kind: formValues.kind,
        is_default: formValues.is_default,
        models: formValues.models,
      };
      if (formValues.api_base) payload.api_base = formValues.api_base;
      if (formValues.api_key) payload.api_key = formValues.api_key;
      if (editingProviderId && editingProviderId !== "new") {
        payload.id = editingProviderId;
      }
      await upsertProvider(payload);
      closeEditor();
    } catch (e) {
      console.error("Failed to save provider", e);
    } finally {
      setIsSaving(false);
    }
  };

  const handleDeleteProvider = async (providerId: string) => {
    if (!window.confirm("确定要删除此服务商配置吗？此操作不可撤销。")) return;
    await deleteProvider(providerId);
    if (editingProviderId === providerId) closeEditor();
  };

  const handleTestConnection = async (providerId: string) => {
    setIsTesting(true);
    setTestResult(null);
    try {
      const result = await invoke<{ success: boolean; message: string }>("test_provider", { providerId });
      setTestResult(result);
    } catch (e: any) {
      setTestResult({ success: false, message: e?.toString() || "连接测试失败" });
    } finally {
      setIsTesting(false);
    }
  };

  const handleFetchModels = async () => {
    setIsFetchingModels(true);
    setTestResult(null);
    try {
      const fetchedModels = await invoke<ModelDescriptor[]>("fetch_provider_models", {
        providerId: editingProviderId && editingProviderId !== "new" ? editingProviderId : null,
        kind: formValues.kind,
        apiBase: formValues.api_base || null,
        apiKey: formValues.api_key || null,
      });
      if (fetchedModels.length > 0) {
        setAvailableModels(fetchedModels);
        
        // Pre-select models that are already in formValues.models
        const existing = new Set(formValues.models.map((model) => model.id));
        
        // Also add logic if it's completely empty, maybe select nothing by default
        // or select everything. Let's just keep the existing ones checked.
        setSelectedModels(existing);
        setIsModelSelectOpen(true);
        setTestResult({ success: true, message: `成功获取 ${fetchedModels.length} 个模型` });
      } else {
        setTestResult({ success: false, message: "未获取到模型列表" });
      }
    } catch (e: any) {
      setTestResult({ success: false, message: e?.toString() || "获取模型失败" });
    } finally {
      setIsFetchingModels(false);
    }
  };

  const handleToggleModel = (model: string) => {
    setSelectedModels(prev => {
      const next = new Set(prev);
      if (next.has(model)) {
        next.delete(model);
      } else {
        next.add(model);
      }
      return next;
    });
  };

  const handleConfirmModels = () => {
    const existing = new Map(formValues.models.map((model) => [model.id, model]));
    const fetched = new Map(availableModels.map((model) => [model.id, model]));
    const modelsArr = Array.from(selectedModels)
      .map((id) => existing.get(id) || fetched.get(id))
      .filter((model): model is ModelDescriptor => Boolean(model));
    updateForm("models", modelsArr);
    setIsModelSelectOpen(false);
  };

  const updateForm = <K extends keyof ProviderFormValues>(field: K, value: ProviderFormValues[K]) => {
    setFormValues((prev) => ({ ...prev, [field]: value }));
  };

  const addManualModels = () => {
    const ids = formValues.modelDraft
      .split(",")
      .map((id) => id.trim())
      .filter(Boolean);
    if (ids.length === 0) return;
    const existing = new Set(formValues.models.map((model) => model.id));
    const additions = ids
      .filter((id) => !existing.has(id))
      .map<ModelDescriptor>((id) => ({
        id,
        context_window: null,
        capabilities: {
          input_modalities: ["text"],
          output_modalities: ["text"],
          embedding: false,
        },
      }));
    setFormValues((prev) => ({
      ...prev,
      models: [...prev.models, ...additions],
      modelDraft: "",
    }));
  };

  const toggleModelModality = (
    modelId: string,
    direction: "input_modalities" | "output_modalities",
    modality: ModelModality,
  ) => {
    setFormValues((prev) => ({
      ...prev,
      models: prev.models.map((model) => {
        if (model.id !== modelId) return model;
        const current = model.capabilities[direction];
        const next = current.includes(modality)
          ? current.filter((value) => value !== modality)
          : [...current, modality];
        return {
          ...model,
          capabilities: { ...model.capabilities, [direction]: next },
        };
      }),
    }));
  };

  const toggleEmbeddingCapability = (modelId: string) => {
    setFormValues((prev) => ({
      ...prev,
      models: prev.models.map((model) =>
        model.id === modelId
          ? {
              ...model,
              capabilities: {
                ...model.capabilities,
                embedding: !model.capabilities.embedding,
              },
            }
          : model
      ),
    }));
  };

  const updateModelContextWindow = (modelId: string, value: number | null) => {
    setFormValues((prev) => ({
      ...prev,
      models: prev.models.map((model) =>
        model.id === modelId ? { ...model, context_window: value } : model
      ),
    }));
  };

  const removeProviderModel = (modelId: string) => {
    setFormValues((prev) => ({
      ...prev,
      models: prev.models.filter((model) => model.id !== modelId),
    }));
  };

  const handleSaveModelRoles = async () => {
    setIsSavingModelRoles(true);
    setModelRoleMessage(null);
    try {
      await setModelRoles(modelRoleForm);
      setModelRoleMessage({ success: true, text: "模型分工已保存" });
    } catch (error) {
      setModelRoleMessage({ success: false, text: String(error) });
    } finally {
      setIsSavingModelRoles(false);
    }
  };

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const editingProvider = providers.find((p) => p.id === editingProviderId);

  const renderKindBadge = (kind: string) => {
    const colors = KIND_BADGE_COLORS[kind] || KIND_BADGE_COLORS.openai;
    return (
      <span className={`inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-semibold border ${colors.bg} ${colors.text} ${colors.border}`}>
        {KIND_LABELS[kind] || kind}
      </span>
    );
  };

  return (
    <div className="agnes-settings-overlay fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm">
      <div className="agnes-settings-modal w-[960px] h-[640px] border border-stone-200 bg-white rounded-2xl overflow-hidden shadow-2xl flex flex-col">
        {/* Header */}
        <header className="px-5 py-4 border-b border-stone-200 bg-stone-50 flex justify-between items-center shrink-0">
          <div className="flex items-center gap-2">
            <Sliders className="h-4.5 w-4.5 text-[#8CA38A]" />
            <span className="font-semibold text-stone-800 text-sm">配置与管理中心</span>
          </div>
          <button
            onClick={onClose}
            className="text-stone-400 hover:text-stone-800 rounded p-1 hover:bg-stone-100 transition-colors"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        {/* Main Content Body */}
        <div className="flex flex-1 overflow-hidden">
          {/* Navigation Sidebar */}
          <nav className="agnes-settings-nav w-56 border-r border-stone-200 bg-stone-50/50 p-3 flex flex-col gap-1 shrink-0">
            <button
              onClick={() => setActiveTab("general")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "general"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <Settings className="h-4 w-4 text-stone-500" />
              <span>通用设置</span>
            </button>
            <button
              onClick={() => setActiveTab("agents")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "agents"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <User className="h-4 w-4 text-stone-500" />
              <span>智能体角色详情</span>
            </button>
            <button
              onClick={() => setActiveTab("memory")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "memory"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <Database className="h-4 w-4 text-stone-500" />
              <span>记忆编辑器 (Memory)</span>
            </button>
            <button
              onClick={() => setActiveTab("storage")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "storage"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <HardDrive className="h-4 w-4 text-stone-500" />
              <span>本地存储</span>
            </button>
            <button
              onClick={() => setActiveTab("llm")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "llm"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <Sliders className="h-4 w-4 text-stone-500" />
              <span>模型与同步 (LLM)</span>
            </button>
            <button
              onClick={() => setActiveTab("tokens")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "tokens"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <BarChart3 className="h-4 w-4 text-stone-500" />
              <span>Token 统计</span>
            </button>
            <button
              onClick={() => setActiveTab("web")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "web"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <Globe2 className="h-4 w-4 text-stone-500" />
              <span>联网搜索</span>
            </button>
            <button
              onClick={() => setActiveTab("mcp")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "mcp"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <Server className="h-4 w-4 text-stone-500" />
              <span>MCP 外部工具</span>
            </button>
            <button
              onClick={() => setActiveTab("audit")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "audit"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <ShieldCheck className="h-4 w-4 text-stone-500" />
              <span>工具执行审计</span>
            </button>
            <button
              onClick={() => setActiveTab("debug")}
              className={`w-full flex items-center gap-2 px-3 py-2 rounded-xl text-xs font-semibold text-left transition-colors ${
                activeTab === "debug"
                  ? "bg-white text-zinc-900 border border-stone-200 shadow-sm"
                  : "text-stone-500 hover:bg-stone-100 hover:text-stone-900"
              }`}
            >
              <Terminal className="h-4 w-4 text-stone-500" />
              <span>提示词调试</span>
            </button>
          </nav>

          {/* Right Panel View */}
          <div className="agnes-settings-content flex-1 overflow-y-auto p-6 bg-white">
            {/* 0. GENERAL TAB */}
            {activeTab === "general" && (
              <GeneralTab />
            )}

            {activeTab === "storage" && (
              <ArtifactStorageTab />
            )}

            {/* 1. AGENTS TAB */}
            {activeTab === "agents" && (
              <div className="space-y-6">
                {/* Header */}
                <div className="flex items-start justify-between">
                  <div>
                    <h3 className="text-sm font-semibold text-stone-850">角色卡</h3>
                    <p className="text-[11px] text-stone-400">创建、编辑或切换当前对话使用的角色卡。</p>
                  </div>
                  {editingAgentId === null && (
                    <button
                      onClick={openNewAgent}
                      className="flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-indigo-600 text-white text-xs font-semibold hover:bg-indigo-700 transition-colors shrink-0"
                    >
                      <Plus className="h-3.5 w-3.5" />
                      新建角色卡
                    </button>
                  )}
                </div>

                {/* 角色卡列表（含编辑/删除） */}
                {editingAgentId === null && (
                  <div className="flex flex-col gap-1">
                    {agents.map((agent) => (
                      <div
                        key={agent.id}
                        className={`group flex items-center justify-between gap-2 w-full text-left text-xs px-3 py-2 rounded-lg transition-colors ${
                          agent.id === activeAgentId
                            ? "bg-white border border-stone-200 text-indigo-600 font-semibold shadow-sm"
                            : "text-stone-600 hover:bg-stone-100 hover:text-stone-900 border border-transparent"
                        }`}
                      >
                        <button
                          onClick={() => setActiveAgentId(agent.id).catch(console.error)}
                          className="flex-1 flex items-center gap-2 min-w-0 text-left"
                        >
                          <AgentAvatar name={agent.name} avatar={agent.avatar} size={20} />
                          <span className="truncate">{agent.name}</span>
                          {agent.thinking_mode && agent.thinking_mode !== "off" && (
                            <span className="shrink-0 text-[9px] font-semibold px-1 py-0.5 rounded bg-violet-50 text-violet-600 border border-violet-200/60">
                              {THINKING_MODE_OPTIONS.find((o) => o.value === agent.thinking_mode)?.label || agent.thinking_mode}思考
                            </span>
                          )}
                          {agent.tags && (
                            <span className="truncate text-[10px] text-stone-400 font-normal">
                              · {agent.tags}
                            </span>
                          )}
                        </button>
                        <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity shrink-0">
                          <button
                            onClick={() => openEditAgent(agent)}
                            title="编辑"
                            className="p-1 rounded-md hover:bg-stone-200 text-stone-500"
                          >
                            <Pencil className="h-3.5 w-3.5" />
                          </button>
                          <button
                            onClick={() => handleDeleteAgent(agent.id, agent.name)}
                            title="删除"
                            className="p-1 rounded-md hover:bg-red-100 text-red-500"
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                )}

                {/* 编辑表单 或 只读信息 */}
                {editingAgentId !== null ? (
                  <div className="border border-stone-200 bg-[#FAF9F5]/20 rounded-xl p-5 space-y-4 shadow-sm">
                    <div className="flex items-center justify-between pb-3 border-b border-stone-200">
                      <h4 className="font-semibold text-xs text-stone-800">
                        {editingAgentId === "new" ? "新建角色卡" : "编辑角色卡"}
                      </h4>
                      <button
                        onClick={closeAgentEditor}
                        className="p-1 rounded-md hover:bg-stone-200 text-stone-500"
                        title="关闭"
                      >
                        <X className="h-4 w-4" />
                      </button>
                    </div>

                    {/* 头像：emoji 选择 / 图片上传 / 清除 */}
                    <div className="relative flex items-center gap-4 pb-3 border-b border-stone-200">
                      <AgentAvatar name={agentForm.name || "?"} avatar={agentForm.avatar} size={64} />
                      <div className="flex flex-col gap-2">
                        <div className="flex gap-2">
                          <button
                            type="button"
                            onClick={() => setShowEmojiPicker((v) => !v)}
                            className="px-3 py-1.5 rounded-lg bg-stone-100 text-stone-600 text-xs font-semibold hover:bg-stone-200"
                          >
                            选择 Emoji
                          </button>
                          <button
                            type="button"
                            onClick={() => fileInputRef.current?.click()}
                            className="px-3 py-1.5 rounded-lg bg-stone-100 text-stone-600 text-xs font-semibold hover:bg-stone-200"
                          >
                            上传图片
                          </button>
                          {agentForm.avatar && (
                            <button
                              type="button"
                              onClick={clearAvatar}
                              className="px-3 py-1.5 rounded-lg bg-stone-100 text-red-500 text-xs font-semibold hover:bg-red-100"
                            >
                              清除
                            </button>
                          )}
                        </div>
                        <input
                          ref={fileInputRef}
                          type="file"
                          accept="image/*"
                          className="hidden"
                          onChange={onAvatarFile}
                        />
                        {showEmojiPicker && (
                          <div className="absolute left-0 top-full mt-2 z-30 w-64 max-h-48 overflow-y-auto rounded-xl border border-stone-200 bg-white p-2 shadow-lg grid grid-cols-8 gap-1">
                            {AGENT_EMOJIS.map((em) => (
                              <button
                                key={em}
                                type="button"
                                onClick={() => {
                                  setAgentForm((f) => ({ ...f, avatar: em }));
                                  setShowEmojiPicker(false);
                                }}
                                className="text-xl leading-none p-1 rounded hover:bg-stone-100"
                              >
                                {em}
                              </button>
                            ))}
                          </div>
                        )}
                      </div>
                    </div>

                    <div className="space-y-3 text-xs text-stone-800 leading-relaxed">
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">名称</label>
                        <input
                          value={agentForm.name}
                          onChange={(e) => setAgentForm((f) => ({ ...f, name: e.target.value }))}
                          placeholder="角色卡名称"
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">人格设定 (Persona)</label>
                        <textarea
                          value={agentForm.persona}
                          onChange={(e) => setAgentForm((f) => ({ ...f, persona: e.target.value }))}
                          rows={3}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">场景 (Scenario)</label>
                        <textarea
                          value={agentForm.scenario}
                          onChange={(e) => setAgentForm((f) => ({ ...f, scenario: e.target.value }))}
                          rows={2}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">系统提示词 (System Prompt)</label>
                        <textarea
                          value={agentForm.system_prompt}
                          onChange={(e) => setAgentForm((f) => ({ ...f, system_prompt: e.target.value }))}
                          rows={3}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] font-mono focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">开场白 (Greeting)</label>
                        <textarea
                          value={agentForm.greeting}
                          onChange={(e) => setAgentForm((f) => ({ ...f, greeting: e.target.value }))}
                          rows={2}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">示例对话 (Example Dialogue)</label>
                        <textarea
                          value={agentForm.example_dialogue}
                          onChange={(e) => setAgentForm((f) => ({ ...f, example_dialogue: e.target.value }))}
                          rows={2}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300 resize-y"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">默认模型（新会话沿用，输入框可覆盖）</label>
                        <select
                          value={agentForm.model}
                          onChange={(e) => setAgentForm((f) => ({ ...f, model: e.target.value }))}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 font-mono text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300"
                        >
                          <option value="">-- 请选择关联的模型 --</option>
                          {providers.map((p) => {
                            const models = p.models.length > 0 ? p.models : [];
                            return (
                              <optgroup key={p.id} label={`${p.name} (${KIND_LABELS[p.kind] || p.kind})`}>
                                {models.map((m) => {
                                  const modelVal = `${p.id}/${m.id}`;
                                  return (
                                    <option key={modelVal} value={modelVal}>
                                      {m.id}
                                    </option>
                                  );
                                })}
                              </optgroup>
                            );
                          })}
                        </select>
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">默认思考模式 / 强度（新会话沿用，输入框可覆盖）</label>
                        <div className="grid grid-cols-5 gap-1.5">
                          {THINKING_MODE_OPTIONS.map((opt) => (
                            <button
                              key={opt.value}
                              type="button"
                              title={opt.desc}
                              onClick={() => setAgentForm((f) => ({ ...f, thinkingMode: opt.value }))}
                              className={`px-1.5 py-1.5 rounded-lg text-[11px] font-semibold transition-colors border ${
                                agentForm.thinkingMode === opt.value
                                  ? "bg-indigo-50 text-indigo-700 border-indigo-300"
                                  : "bg-stone-50 text-stone-500 border-stone-200/60 hover:bg-stone-100"
                              }`}
                            >
                              {opt.label}
                            </button>
                          ))}
                        </div>
                        <p className="mt-1 text-[10px] text-stone-400">
                          关闭/自动由模型决定；轻度~深度控制思考深度。预算交由服务商默认参数，无需手动设置。
                        </p>
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">标签 (逗号分隔)</label>
                        <input
                          value={agentForm.tags}
                          onChange={(e) => setAgentForm((f) => ({ ...f, tags: e.target.value }))}
                          placeholder="例如：助手, 编程"
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300"
                        />
                      </div>
                      <div>
                        <label className="font-semibold text-stone-500 block mb-1">工具安全策略 (Tool Policy)</label>
                        <div className="bg-stone-50 rounded-lg border border-stone-200/60 px-3 py-1">
                          {renderToolToggle("shell", "Shell 命令执行")}
                          {renderToolToggle("file", "文件读写")}
                          {renderToolToggle("git", "Git 操作")}
                          {renderToolToggle("memory", "长期记忆")}
                          {renderToolToggle("planner", "日历与待办")}
                          {renderToolToggle("web", "联网搜索与网页读取")}
                          {renderToolToggle("mcp", "MCP 外部工具")}
                          {agentForm.toolPolicy.mcp.enabled && (
                            <div className="border-b border-stone-100 py-2">
                              <div className="mb-1.5 flex items-center justify-between">
                                <span className="text-[10px] font-semibold text-stone-500">允许使用的 Server</span>
                                <span className="text-[9px] tabular-nums text-stone-400">
                                  {agentForm.toolPolicy.mcp.server_ids.length} / {mcpServers.filter((server) => server.enabled).length}
                                </span>
                              </div>
                              {mcpServers.filter((server) => server.enabled).length === 0 ? (
                                <p className="text-[10px] text-stone-400">请先在 MCP 外部工具页面配置并启用 Server。</p>
                              ) : (
                                <div className="grid grid-cols-2 gap-1.5">
                                  {mcpServers.filter((server) => server.enabled).map((server) => {
                                    const selected = agentForm.toolPolicy.mcp.server_ids.includes(server.id);
                                    return (
                                      <label
                                        key={server.id}
                                        className={`flex cursor-pointer items-center gap-2 rounded-md border px-2 py-1.5 text-[10px] ${
                                          selected
                                            ? "border-indigo-200 bg-indigo-50 text-indigo-700"
                                            : "border-stone-200 bg-white text-stone-500"
                                        }`}
                                      >
                                        <input
                                          type="checkbox"
                                          checked={selected}
                                          onChange={() =>
                                            setAgentForm((form) => ({
                                              ...form,
                                              toolPolicy: {
                                                ...form.toolPolicy,
                                                mcp: {
                                                  ...form.toolPolicy.mcp,
                                                  server_ids: selected
                                                    ? form.toolPolicy.mcp.server_ids.filter((id) => id !== server.id)
                                                    : [...form.toolPolicy.mcp.server_ids, server.id],
                                                },
                                              },
                                            }))
                                          }
                                          className="h-3.5 w-3.5 accent-indigo-600"
                                        />
                                        <span className="truncate">{server.name}</span>
                                      </label>
                                    );
                                  })}
                                </div>
                              )}
                            </div>
                          )}
                          <div className="flex items-center justify-between gap-3 py-1.5 border-b border-stone-100">
                            <span className="text-xs text-stone-700">搜索来源</span>
                            <select
                              value={agentForm.toolPolicy.web.search_provider}
                              disabled={!agentForm.toolPolicy.web.enabled}
                              onChange={(event) =>
                                setAgentForm((form) => ({
                                  ...form,
                                  toolPolicy: {
                                    ...form.toolPolicy,
                                    web: {
                                      ...form.toolPolicy.web,
                                      search_provider: event.target.value as WebSearchProvider,
                                    },
                                  },
                                }))
                              }
                              className="rounded-md border border-stone-200 bg-white px-2 py-1 text-[10px] font-semibold text-stone-600 outline-none disabled:opacity-40"
                            >
                              <option value="auto">
                                自动回退链（{searchProviderForm.fallbackOrder.length} 个）
                              </option>
                              <option value="duckduckgo">DuckDuckGo</option>
                              <option value="bing">Bing</option>
                              <option value="searxng" disabled={!savedSearchProviderSettings?.searxng_base_url}>
                                SearXNG{savedSearchProviderSettings?.searxng_base_url ? "" : "（未配置）"}
                              </option>
                              <option
                                value="brave"
                                disabled={!savedSearchProviderSettings?.has_brave_api_key}
                              >
                                Brave Search{savedSearchProviderSettings?.has_brave_api_key ? "" : "（未配置）"}
                              </option>
                            </select>
                          </div>
                          <div className="flex items-center justify-between py-1.5 border-b border-stone-100">
                            <span className="text-xs text-stone-700">网络访问</span>
                            <button
                              type="button"
                              onClick={() =>
                                setAgentForm((form) => ({
                                  ...form,
                                  toolPolicy: {
                                    ...form.toolPolicy,
                                    network: {
                                      ...form.toolPolicy.network,
                                      allow: !form.toolPolicy.network.allow,
                                    },
                                  },
                                }))
                              }
                              className={`px-2 py-1 rounded-md text-[10px] font-semibold border transition-colors ${
                                agentForm.toolPolicy.network.allow
                                  ? "bg-blue-50 text-blue-700 border-blue-200"
                                  : "bg-emerald-50 text-emerald-700 border-emerald-200"
                              }`}
                            >
                              {agentForm.toolPolicy.network.allow ? "允许联网" : "隔离网络"}
                            </button>
                          </div>
                          <div className="flex items-center justify-between py-1.5">
                            <span className="text-xs text-stone-700">进程沙箱</span>
                            <span className="text-[10px] text-stone-500">
                              Landlock · rlimit · bwrap 自动增强
                            </span>
                          </div>
                        </div>
                      </div>
                    </div>

                    <div className="flex justify-end gap-2 pt-2 border-t border-stone-200">
                      <button
                        onClick={closeAgentEditor}
                        className="px-4 py-1.5 rounded-lg text-xs font-semibold text-stone-500 hover:bg-stone-100"
                      >
                        取消
                      </button>
                      <button
                        onClick={saveAgent}
                        disabled={isSavingAgent}
                        className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg bg-indigo-600 text-white text-xs font-semibold hover:bg-indigo-700 disabled:opacity-50"
                      >
                        <Check className="h-3.5 w-3.5" />
                        {isSavingAgent ? "保存中..." : "保存"}
                      </button>
                    </div>
                  </div>
                ) : activeAgent ? (
                  <div className="border border-stone-200 bg-[#FAF9F5]/20 rounded-xl p-5 space-y-4 shadow-sm">
                    <div className="flex items-center gap-3 pb-3 border-b border-stone-200">
                      <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={40} />
                      <div>
                        <h4 className="font-semibold text-xs text-stone-800">{activeAgent.name}</h4>
                        <p className="text-[10px] text-stone-500 font-mono">ID: {activeAgent.id}</p>
                      </div>
                    </div>

                    <div className="space-y-3 text-xs text-stone-800 leading-relaxed">
                      <div>
                        <span className="font-semibold text-stone-500 block mb-1">能力模型:</span>
                        <select
                          value={activeAgent.model || ""}
                          onChange={(e) => updateAgentModel(activeAgent.id, e.target.value)}
                          className="w-full bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 font-mono text-[11px] focus:outline-none focus:ring-1 focus:ring-stone-300"
                        >
                          <option value="">-- 请选择关联的模型 --</option>
                          {providers.map((p) => {
                            const models = p.models.length > 0 ? p.models : [];
                            return (
                              <optgroup key={p.id} label={`${p.name} (${KIND_LABELS[p.kind] || p.kind})`}>
                                {models.map((m) => {
                                  const modelVal = `${p.id}/${m.id}`;
                                  return (
                                    <option key={modelVal} value={modelVal}>
                                      {m.id}
                                    </option>
                                  );
                                })}
                              </optgroup>
                            );
                          })}
                        </select>
                      </div>
                      <div>
                        <span className="font-semibold text-stone-500 block mb-1">人格与背景设定 (Persona):</span>
                        <p className="bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-stone-650 whitespace-pre-wrap">
                          {activeAgent.persona || "暂无设定"}
                        </p>
                      </div>
                      <div>
                        <span className="font-semibold text-stone-500 block mb-1">工具安全策略 (Tool Policy):</span>
                        <pre className="bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 font-mono text-[10px] text-stone-600 whitespace-pre-wrap max-h-40 overflow-y-auto">
                          {activeAgent.tool_policy || "{}"}
                        </pre>
                      </div>
                    </div>
                  </div>
                ) : null}
              </div>
            )}

            {/* 2. MEMORY TAB */}
            {activeTab === "memory" && (
              <div className="space-y-6">
                <div>
                  <h3 className="text-sm font-semibold text-stone-850">本地记忆文件编辑</h3>
                  <p className="text-[11px] text-stone-400">
                    直接编辑或保存当前 Agent 的 USER.md (背景画像) 与 MEMORY.md (事实积累)。
                  </p>
                </div>

                <div className="grid grid-cols-1 xl:grid-cols-2 gap-4">
                  {/* USER.md */}
                  <div className="space-y-2 flex flex-col">
                    <div className="flex justify-between items-center text-xs">
                      <span className="font-semibold text-stone-500">USER.md (AI只读，用户画像)</span>
                      {isEditingUserMd ? (
                        <div className="flex gap-2 font-medium">
                          <button
                            onClick={() => setIsEditingUserMd(false)}
                            className="text-stone-400 hover:text-stone-600"
                          >
                            取消
                          </button>
                          <button
                            onClick={handleSaveUserMd}
                            className="text-emerald-600 hover:text-emerald-700 font-semibold"
                          >
                            保存
                          </button>
                        </div>
                      ) : (
                        <button
                          onClick={() => setIsEditingUserMd(true)}
                          className="text-[#6C806A] hover:text-[#556654] font-medium"
                        >
                          编辑
                        </button>
                      )}
                    </div>
                    {isEditingUserMd ? (
                      <textarea
                        value={userMdText}
                        onChange={(e) => setUserMdText(e.target.value)}
                        className="h-72 w-full bg-white border border-stone-300 rounded-lg p-3 font-mono text-[10px] focus:outline-none"
                      />
                    ) : (
                      <pre className="h-72 w-full bg-[#FAF9F5]/45 border border-stone-200 rounded-lg p-3 font-sans text-xs text-stone-600 overflow-y-auto whitespace-pre-wrap select-text leading-relaxed">
                        {userMdText || "# USER.md\n\n(暂无本地画像信息，请点击右上角编辑)"}
                      </pre>
                    )}
                  </div>

                  {/* MEMORY.md */}
                  <div className="space-y-2 flex flex-col">
                    <div className="flex justify-between items-center text-xs">
                      <span className="font-semibold text-stone-500">MEMORY.md (AI可写，事实积累)</span>
                      {isEditingMemoryMd ? (
                        <div className="flex gap-2 font-medium">
                          <button
                            onClick={() => setIsEditingMemoryMd(false)}
                            className="text-stone-400 hover:text-stone-600"
                          >
                            取消
                          </button>
                          <button
                            onClick={handleSaveMemoryMd}
                            className="text-emerald-600 hover:text-emerald-700 font-semibold"
                          >
                            保存
                          </button>
                        </div>
                      ) : (
                        <button
                          onClick={() => setIsEditingMemoryMd(true)}
                          className="text-[#6C806A] hover:text-[#556654] font-medium"
                        >
                          编辑
                        </button>
                      )}
                    </div>
                    {isEditingMemoryMd ? (
                      <textarea
                        value={memoryMdText}
                        onChange={(e) => setMemoryMdText(e.target.value)}
                        className="h-72 w-full bg-white border border-stone-300 rounded-lg p-3 font-mono text-[10px] focus:outline-none"
                      />
                    ) : (
                      <pre className="h-72 w-full bg-[#FAF9F5]/45 border border-stone-200 rounded-lg p-3 font-sans text-xs text-stone-600 overflow-y-auto whitespace-pre-wrap select-text leading-relaxed">
                        {memoryMdText || "# MEMORY.md\n\n(暂无事实积累，请点击右上角编辑)"}
                      </pre>
                    )}
                  </div>
                </div>

                <div className="border-t border-stone-200 pt-5 space-y-4">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <h3 className="text-sm font-semibold text-stone-850">结构化记忆库</h3>
                    <button
                      type="button"
                      onClick={openNewMemory}
                      className="inline-flex items-center gap-1.5 rounded-md bg-[#6C806A] px-3 py-1.5 text-xs font-semibold text-white hover:bg-[#596C58]"
                    >
                      <Plus className="h-3.5 w-3.5" />
                      新建记忆
                    </button>
                  </div>

                  <div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-y border-stone-200 bg-stone-50/70 px-3 py-2.5">
                    <div className="flex min-w-0 flex-1 items-center gap-3">
                      <Database className="h-4 w-4 shrink-0 text-stone-500" />
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs">
                          <span
                            className="max-w-full truncate font-semibold text-stone-700"
                            title={memoryEmbeddingStatus?.model_ref || undefined}
                          >
                            {memoryEmbeddingStatus
                              ? embeddingModelName(memoryEmbeddingStatus.model_ref)
                              : "正在读取向量索引"}
                          </span>
                          {memoryEmbeddingStatus && (
                            <span className="text-stone-500">
                              {memoryEmbeddingStatus.indexed}/{memoryEmbeddingStatus.total} 已向量化
                            </span>
                          )}
                        </div>
                        <div className="mt-1 flex items-center gap-2">
                          <div className="h-1.5 w-28 shrink-0 overflow-hidden rounded-full bg-stone-200">
                            <div
                              className={`h-full rounded-full transition-[width] ${
                                memoryEmbeddingStatus?.pending === 0 && memoryEmbeddingStatus.total > 0
                                  ? "bg-emerald-500"
                                  : "bg-amber-500"
                              }`}
                              style={{ width: `${embeddingProgress}%` }}
                            />
                          </div>
                          <span className="text-[10px] text-stone-400">
                            {!memoryEmbeddingStatus
                              ? "读取中"
                              : !memoryEmbeddingStatus.model_ref
                                ? "向量检索关闭"
                                : memoryEmbeddingStatus.total === 0
                                  ? "记忆库为空"
                                  : memoryEmbeddingStatus.pending > 0
                                    ? `${memoryEmbeddingStatus.pending} 条待处理 · ${embeddingProgress}%`
                                    : `索引已同步 · ${embeddingProgress}%`}
                          </span>
                        </div>
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={vectorizeStructuredMemories}
                      disabled={
                        isVectorizingMemories
                        || !memoryEmbeddingStatus?.model_ref
                        || memoryEmbeddingStatus.total === 0
                        || memoryEmbeddingStatus.pending === 0
                      }
                      title="向量化当前 Agent 的待处理记忆"
                      className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-semibold text-stone-700 hover:bg-stone-100 disabled:cursor-not-allowed disabled:opacity-45"
                    >
                      <RefreshCw className={`h-3.5 w-3.5 ${isVectorizingMemories ? "animate-spin" : ""}`} />
                      {isVectorizingMemories ? "向量化中" : "向量化"}
                    </button>
                  </div>

                  {memoryVectorMessage && (
                    <p
                      aria-live="polite"
                      className={`text-xs ${memoryVectorMessage.success ? "text-emerald-700" : "text-red-600"}`}
                    >
                      {memoryVectorMessage.text}
                    </p>
                  )}

                  <div className="relative max-w-sm">
                    <Search className="pointer-events-none absolute left-2.5 top-2.5 h-3.5 w-3.5 text-stone-400" />
                    <input
                      value={memorySearch}
                      onChange={(event) => setMemorySearch(event.target.value)}
                      placeholder="搜索名称、关键词或内容"
                      className="h-9 w-full rounded-md border border-stone-200 bg-white pl-8 pr-3 text-xs text-stone-700 focus:border-stone-400 focus:outline-none"
                    />
                  </div>

                  {editingMemoryId && (
                    <div className="border-y border-stone-200 bg-stone-50/70 py-4">
                      <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
                        <label className="space-y-1 text-[11px] font-medium text-stone-500">
                          <span>名称</span>
                          <input
                            value={memoryForm.name}
                            onChange={(event) => setMemoryForm((form) => ({ ...form, name: event.target.value }))}
                            className="h-9 w-full rounded-md border border-stone-200 bg-white px-3 text-xs text-stone-800 focus:border-stone-400 focus:outline-none"
                          />
                        </label>
                        <label className="space-y-1 text-[11px] font-medium text-stone-500">
                          <span>关键词</span>
                          <input
                            value={memoryForm.keywords}
                            onChange={(event) => setMemoryForm((form) => ({ ...form, keywords: event.target.value }))}
                            placeholder="多个关键词用逗号分隔"
                            className="h-9 w-full rounded-md border border-stone-200 bg-white px-3 text-xs text-stone-800 focus:border-stone-400 focus:outline-none"
                          />
                        </label>
                      </div>
                      <label className="mt-3 block space-y-1 text-[11px] font-medium text-stone-500">
                        <span>记忆内容</span>
                        <textarea
                          value={memoryForm.content}
                          onChange={(event) => setMemoryForm((form) => ({ ...form, content: event.target.value }))}
                          className="min-h-28 w-full resize-y rounded-md border border-stone-200 bg-white p-3 text-xs leading-relaxed text-stone-800 focus:border-stone-400 focus:outline-none"
                        />
                      </label>
                      {memoryError && <p className="mt-2 text-xs text-red-600">{memoryError}</p>}
                      <div className="mt-3 flex justify-end gap-2">
                        <button
                          type="button"
                          onClick={closeMemoryEditor}
                          className="rounded-md px-3 py-1.5 text-xs font-medium text-stone-500 hover:bg-stone-200"
                        >
                          取消
                        </button>
                        <button
                          type="button"
                          onClick={saveStructuredMemory}
                          disabled={isSavingMemory}
                          className="rounded-md bg-[#6C806A] px-3 py-1.5 text-xs font-semibold text-white hover:bg-[#596C58] disabled:opacity-50"
                        >
                          {isSavingMemory ? "保存中..." : "保存"}
                        </button>
                      </div>
                    </div>
                  )}

                  <div className="divide-y divide-stone-200 border-y border-stone-200">
                    {visibleMemories.length === 0 ? (
                      <p className="py-10 text-center text-xs text-stone-400">暂无结构化记忆</p>
                    ) : (
                      visibleMemories.map((memory) => (
                        <article key={memory.id} className="py-4">
                          <div className="flex items-start justify-between gap-4">
                            <div className="min-w-0 flex-1">
                              <div className="flex flex-wrap items-center gap-2">
                                <h4 className="break-words text-sm font-semibold text-stone-800">{memory.name}</h4>
                                <span className="rounded-sm border border-stone-200 bg-stone-50 px-1.5 py-0.5 text-[10px] font-medium text-stone-500">
                                  {memory.creator === "ai" ? "AI 创建" : "用户创建"}
                                </span>
                                <time className="text-[10px] text-stone-400">{formatMemoryTime(memory.created_at)}</time>
                              </div>
                              {memory.keywords.length > 0 && (
                                <div className="mt-2 flex flex-wrap gap-1.5">
                                  {memory.keywords.map((keyword) => (
                                    <span key={keyword} className="rounded-sm bg-emerald-50 px-1.5 py-0.5 text-[10px] text-emerald-700">
                                      {keyword}
                                    </span>
                                  ))}
                                </div>
                              )}
                              <p className="mt-2 whitespace-pre-wrap break-words text-xs leading-relaxed text-stone-600">{memory.content}</p>
                            </div>
                            <div className="flex shrink-0 items-center gap-1">
                              <button
                                type="button"
                                onClick={() => openMemoryEditor(memory)}
                                title="编辑记忆"
                                className="rounded p-1.5 text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                              >
                                <Pencil className="h-3.5 w-3.5" />
                              </button>
                              <button
                                type="button"
                                onClick={() => removeStructuredMemory(memory)}
                                title="删除记忆"
                                className="rounded p-1.5 text-stone-400 hover:bg-red-50 hover:text-red-600"
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </button>
                            </div>
                          </div>
                        </article>
                      ))
                    )}
                  </div>
                  {!editingMemoryId && memoryError && <p className="text-xs text-red-600">{memoryError}</p>}
                </div>
              </div>
            )}

            {/* 3. LLM TAB */}
            {activeTab === "llm" && (
              <div className="space-y-5">
                {/* Model role routing */}
                <div className="border border-stone-200 bg-[#FAF9F5]/30 rounded-xl p-5 space-y-4 shadow-sm">
                  <div className="flex items-start justify-between gap-4">
                    <div>
                      <h3 className="text-sm font-semibold text-stone-850">模型分工</h3>
                      <p className="text-[11px] text-stone-400 mt-0.5">
                        按能力和成本为不同任务指定模型。文本任务留空时回退主模型；嵌入模型留空时关闭向量检索。
                      </p>
                    </div>
                    <button
                      type="button"
                      onClick={handleSaveModelRoles}
                      disabled={isSavingModelRoles}
                      className="shrink-0 bg-[#8CA38A] text-white hover:bg-[#7A917A] rounded-lg px-3 py-1.5 text-xs font-semibold transition-colors disabled:opacity-50"
                    >
                      {isSavingModelRoles ? "保存中..." : "保存分工"}
                    </button>
                  </div>

                  <div className="grid grid-cols-2 gap-3">
                    {MODEL_ROLE_OPTIONS.map((role) => (
                      <label key={role.key} className="space-y-1">
                        <span className="flex items-center justify-between text-[11px] font-semibold text-stone-600">
                          {role.label}
                          {role.key === "embedding_model" && (
                            <span className="text-[9px] text-violet-500">独立能力</span>
                          )}
                        </span>
                        <select
                          value={modelRoleForm[role.key] || ""}
                          onChange={(event) =>
                            setModelRoleForm((current) => ({
                              ...current,
                              [role.key]: event.target.value || null,
                            }))
                          }
                          className="w-full bg-white border border-stone-200 rounded-lg px-2.5 py-2 text-[11px] text-stone-700 font-mono focus:outline-none focus:ring-1 focus:ring-[#8CA38A]/40"
                        >
                          <option value="">
                            {role.key === "embedding_model" ? "未指定（关闭向量检索）" : "未指定（自动回退）"}
                          </option>
                          {providers.map((provider) => {
                            const models = provider.models.filter((model) => modelSupportsRole(model, role.key));
                            if (models.length === 0) return null;
                            return (
                              <optgroup key={provider.id} label={provider.name}>
                                {models.map((model) => {
                                  const value = `${provider.id}/${model.id}`;
                                  return <option key={value} value={value}>{model.id}</option>;
                                })}
                              </optgroup>
                            );
                          })}
                        </select>
                        <span className="block text-[9px] leading-relaxed text-stone-400">{role.desc}</span>
                      </label>
                    ))}
                  </div>

                  <div className="border-t border-stone-200/70 pt-3">
                    <div className="flex flex-col items-stretch gap-2 sm:flex-row sm:items-start sm:justify-between sm:gap-4">
                      <div className="min-w-0">
                        <span className="text-[11px] font-semibold text-stone-700">故障备用模型</span>
                        <p className="mt-0.5 text-[9px] text-stone-400">
                          仅在当前模型尚未输出正文、思考或工具调用时，按顺序尝试下一个模型。
                        </p>
                      </div>
                      <select
                        value=""
                        disabled={modelRoleForm.fallback_models.length >= 5}
                        onChange={(event) => {
                          const value = event.target.value;
                          if (!value || modelRoleForm.fallback_models.includes(value)) return;
                          setModelRoleForm((current) => ({
                            ...current,
                            fallback_models: [...current.fallback_models, value],
                          }));
                        }}
                        className="w-full rounded-md border border-stone-200 bg-white px-2.5 py-1.5 text-[10px] font-mono text-stone-600 outline-none disabled:opacity-40 sm:w-56 sm:shrink-0"
                      >
                        <option value="">添加备用模型...</option>
                        {providers.map((provider) => {
                          const models = provider.models.filter((model) => modelSupportsRole(model, "main_model"));
                          if (models.length === 0) return null;
                          return (
                            <optgroup key={provider.id} label={provider.name}>
                              {models.map((model) => {
                                const value = `${provider.id}/${model.id}`;
                                return (
                                  <option
                                    key={value}
                                    value={value}
                                    disabled={modelRoleForm.fallback_models.includes(value)}
                                  >
                                    {model.id}
                                  </option>
                                );
                              })}
                            </optgroup>
                          );
                        })}
                      </select>
                    </div>
                    {modelRoleForm.fallback_models.length === 0 ? (
                      <p className="py-3 text-center text-[10px] text-stone-400">未配置备用模型</p>
                    ) : (
                      <div className="mt-2 divide-y divide-stone-200 border-y border-stone-200">
                        {modelRoleForm.fallback_models.map((modelRef, index) => {
                          const [providerId, ...modelParts] = modelRef.split("/");
                          const providerName = providers.find((provider) => provider.id === providerId)?.name || providerId;
                          return (
                            <div key={modelRef} className="flex h-10 items-center gap-2 px-2">
                              <span className="w-5 text-center text-[10px] tabular-nums text-stone-400">{index + 1}</span>
                              <span className="min-w-0 flex-1 truncate text-[10px] text-stone-600">
                                <span className="font-semibold text-stone-700">{providerName}</span>
                                <span className="mx-1 text-stone-300">/</span>
                                <span className="font-mono">{modelParts.join("/")}</span>
                              </span>
                              <button
                                type="button"
                                disabled={index === 0}
                                title="提高优先级"
                                onClick={() => setModelRoleForm((current) => {
                                  const next = [...current.fallback_models];
                                  [next[index - 1], next[index]] = [next[index], next[index - 1]];
                                  return { ...current, fallback_models: next };
                                })}
                                className="flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-25"
                              >
                                <ArrowUp className="h-3.5 w-3.5" />
                              </button>
                              <button
                                type="button"
                                disabled={index === modelRoleForm.fallback_models.length - 1}
                                title="降低优先级"
                                onClick={() => setModelRoleForm((current) => {
                                  const next = [...current.fallback_models];
                                  [next[index], next[index + 1]] = [next[index + 1], next[index]];
                                  return { ...current, fallback_models: next };
                                })}
                                className="flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-25"
                              >
                                <ArrowDown className="h-3.5 w-3.5" />
                              </button>
                              <button
                                type="button"
                                title="移除备用模型"
                                onClick={() => setModelRoleForm((current) => ({
                                  ...current,
                                  fallback_models: current.fallback_models.filter((value) => value !== modelRef),
                                }))}
                                className="flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-600"
                              >
                                <Trash2 className="h-3.5 w-3.5" />
                              </button>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>

                  <p className="text-[10px] text-stone-400 border-t border-stone-200/60 pt-3">
                    语音输入能力标签将在语音管线实现时补充；当前语音模型仅校验文本输出能力。嵌入模型用于本地记忆索引，切换模型后会在下次对话前自动回填。
                  </p>
                  {modelRoleMessage && (
                    <div className={`text-[11px] rounded-lg border px-3 py-2 ${
                      modelRoleMessage.success
                        ? "bg-emerald-50 border-emerald-200 text-emerald-700"
                        : "bg-rose-50 border-rose-200 text-rose-700"
                    }`}>
                      {modelRoleMessage.text}
                    </div>
                  )}
                </div>

                {/* Model provider configuration */}
                <div>
                  <h3 className="text-sm font-semibold text-stone-850">模型服务商配置</h3>
                  <p className="text-[11px] text-stone-400">管理 LLM API 提供商，配置自定义 endpoint、密钥与可用模型。</p>
                </div>

                <div
                  className={`flex items-center gap-2 border-y px-3 py-2 text-[11px] ${
                    !secretStoreStatus
                      ? "border-stone-200 bg-stone-50/60 text-stone-500"
                      : secretStoreStatus.available
                        ? "border-emerald-200 bg-emerald-50/60 text-emerald-700"
                        : "border-red-200 bg-red-50/60 text-red-700"
                  }`}
                  title={secretStoreStatus?.error || undefined}
                >
                  <ShieldCheck className="h-3.5 w-3.5 shrink-0" />
                  <span className="min-w-0 break-words">
                    {!secretStoreStatus
                      ? "正在检查系统密钥环"
                      : secretStoreStatus.available
                        ? `API Key 存储：${secretStoreStatus.backend}`
                        : `系统密钥环异常：${secretStoreStatus.error || "未知错误"}`}
                  </span>
                </div>

                {/* Provider List */}
                <div className="flex flex-col gap-2.5">
                  {providers.map((provider, providerIndex) => (
                    <div
                      key={provider.id}
                      style={{ order: providerIndex * 2 }}
                      className={`border rounded-xl p-4 shadow-sm transition-all duration-200 hover:shadow-md ${
                        editingProviderId === provider.id
                          ? "border-[#8CA38A]/50 bg-[#FAF9F5]/40 ring-1 ring-[#8CA38A]/20"
                          : "border-stone-200 bg-[#FAF9F5]/20 hover:border-stone-300"
                      }`}
                    >
                      <div className="flex items-center justify-between">
                        {/* Left: Provider info */}
                        <div className="flex items-center gap-3 min-w-0 flex-1">
                          <div className="h-8 w-8 rounded-lg bg-stone-100 border border-stone-200/60 flex items-center justify-center shrink-0">
                            <Server className="h-3.5 w-3.5 text-stone-500" />
                          </div>
                          <div className="min-w-0">
                            <div className="flex items-center gap-2">
                              <span className="text-xs font-semibold text-stone-800 truncate">{provider.name}</span>
                              {renderKindBadge(provider.kind)}
                              {provider.is_default && (
                                <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[10px] font-semibold bg-[#8CA38A]/10 text-[#6C806A] border border-[#8CA38A]/20">
                                  <Check className="h-2.5 w-2.5" />
                                  默认
                                </span>
                              )}
                              {provider.has_api_key && (
                                <span className="inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded text-[10px] font-semibold bg-amber-50 text-amber-700 border border-amber-200/60">
                                  <Key className="h-2.5 w-2.5" />
                                  已配置密钥
                                </span>
                              )}
                            </div>
                            <div className="flex items-center gap-3 mt-0.5">
                              {provider.api_base && (
                                <span className="text-[10px] text-stone-400 font-mono truncate max-w-[240px]">
                                  {provider.api_base}
                                </span>
                              )}
                              <span className="text-[10px] text-stone-400">
                                {provider.models.length} 个模型
                              </span>
                            </div>
                          </div>
                        </div>

                        {/* Right: Actions */}
                        <div className="flex items-center gap-1 shrink-0 ml-3">
                          <button
                            onClick={() => openEditProvider(provider)}
                            className="p-1.5 rounded-lg text-stone-400 hover:text-stone-700 hover:bg-stone-100 transition-colors"
                            title="编辑"
                          >
                            <Pencil className="h-3.5 w-3.5" />
                          </button>
                          <button
                            onClick={() => handleTestConnection(provider.id)}
                            className="p-1.5 rounded-lg text-stone-400 hover:text-amber-600 hover:bg-amber-50 transition-colors"
                            title="测试连接"
                            disabled={isTesting}
                          >
                            <Zap className="h-3.5 w-3.5" />
                          </button>
                          <button
                            onClick={() => handleDeleteProvider(provider.id)}
                            className="p-1.5 rounded-lg text-stone-400 hover:text-red-600 hover:bg-red-50 transition-colors"
                            title="删除"
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>

                      {provider.models.length > 0 && (
                        <div className="mt-3 pt-3 border-t border-stone-200/60 space-y-1.5">
                          {provider.models.slice(0, 4).map((model) => (
                            <div key={model.id} className="flex items-center gap-2 min-w-0">
                              <span className="text-[10px] font-mono text-stone-600 truncate min-w-0">
                                {model.id}
                              </span>
                              <span className="flex items-center gap-1 shrink-0">
                                {capabilityLabels(model.capabilities).map((label) => (
                                  <span
                                    key={label}
                                    className="rounded border border-stone-200 bg-white px-1.5 py-0.5 text-[8px] text-stone-500"
                                  >
                                    {label}
                                  </span>
                                ))}
                              </span>
                            </div>
                          ))}
                          {provider.models.length > 4 && (
                            <div className="text-[9px] text-stone-400">另有 {provider.models.length - 4} 个模型</div>
                          )}
                        </div>
                      )}

                      {/* Inline test result for this provider */}
                      {testResult && editingProviderId !== provider.id && !editingProviderId && (
                        // Show test results only if we just tested (no editor open)
                        null
                      )}
                    </div>
                  ))}

                  {providers.length === 0 && !editingProviderId && (
                    <div style={{ order: 0 }} className="border border-dashed border-stone-200 rounded-xl p-8 text-center">
                      <Server className="h-8 w-8 text-stone-300 mx-auto mb-2" />
                      <p className="text-xs text-stone-400">尚未配置任何模型服务商</p>
                      <p className="text-[10px] text-stone-350 mt-0.5">点击下方按钮添加您的第一个服务商</p>
                    </div>
                  )}
                {/* Test result (shown globally when no editor is open) */}
                {testResult && !editingProviderId && (
                  <div style={{ order: providers.length * 2 }} className={`flex items-center gap-2 px-3 py-2 rounded-lg text-xs border transition-all ${
                    testResult.success
                      ? "bg-emerald-50 border-emerald-200/60 text-emerald-700"
                      : "bg-red-50 border-red-200/60 text-red-600"
                  }`}>
                    {testResult.success ? <Check className="h-3.5 w-3.5 shrink-0" /> : <X className="h-3.5 w-3.5 shrink-0" />}
                    <span className="truncate">{testResult.message}</span>
                    <button
                      onClick={() => setTestResult(null)}
                      className="ml-auto p-0.5 hover:bg-black/5 rounded shrink-0"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                )}

                {/* Provider Editor (inline form) */}
                {editingProviderId && (
                  <div
                    style={{
                      order: editingProviderId === "new"
                        ? providers.length * 2
                        : Math.max(0, providers.findIndex((provider) => provider.id === editingProviderId) * 2 + 1),
                    }}
                    className="border border-[#8CA38A]/30 bg-[#FAF9F5]/40 rounded-xl p-5 space-y-4 shadow-sm ring-1 ring-[#8CA38A]/10"
                  >
                    <div className="flex items-center justify-between pb-3 border-b border-stone-200/60">
                      <h4 className="text-xs font-semibold text-stone-700">
                        {editingProviderId === "new" ? "添加新服务商" : "编辑服务商"}
                      </h4>
                      <button
                        onClick={closeEditor}
                        className="text-stone-400 hover:text-stone-600 p-0.5 rounded hover:bg-stone-100 transition-colors"
                      >
                        <X className="h-3.5 w-3.5" />
                      </button>
                    </div>

                    <div className="grid grid-cols-2 gap-4">
                      {/* 名称 */}
                      <div className="space-y-1">
                        <label className="text-xs font-semibold text-stone-500">名称</label>
                        <input
                          type="text"
                          value={formValues.name}
                          onChange={(e) => updateForm("name", e.target.value)}
                          placeholder="例如：My OpenAI"
                          className="w-full bg-white border border-stone-200 rounded-lg px-3 py-2 text-xs text-stone-800 focus:outline-none focus:ring-1 focus:ring-[#8CA38A]/40 focus:border-[#8CA38A] transition-shadow"
                        />
                      </div>

                      {/* 类型 */}
                      <div className="space-y-1">
                        <label className="text-xs font-semibold text-stone-500">类型</label>
                        <select
                          value={formValues.kind}
                          onChange={(e) => updateForm("kind", e.target.value as ProviderKind)}
                          className="w-full bg-white border border-stone-200 rounded-lg px-3 py-2 text-xs text-stone-800 focus:outline-none focus:ring-1 focus:ring-[#8CA38A]/40 focus:border-[#8CA38A] transition-shadow appearance-none"
                        >
                          {KIND_OPTIONS.map((opt) => (
                            <option key={opt.value} value={opt.value}>
                              {opt.label}
                            </option>
                          ))}
                        </select>
                      </div>
                    </div>

                    {/* API Base URL */}
                    <div className="space-y-1">
                      <label className="text-xs font-semibold text-stone-500">API Base URL</label>
                      <input
                        type="text"
                        value={formValues.api_base}
                        onChange={(e) => updateForm("api_base", e.target.value)}
                        placeholder={KIND_PLACEHOLDER_URL[formValues.kind] || "输入 API 地址..."}
                        className="w-full bg-white border border-stone-200 rounded-lg px-3 py-2 text-xs text-stone-800 font-mono focus:outline-none focus:ring-1 focus:ring-[#8CA38A]/40 focus:border-[#8CA38A] transition-shadow"
                      />
                    </div>

                    {/* API Key */}
                    <div className="space-y-1">
                      <label className="text-xs font-semibold text-stone-500 flex items-center gap-1.5">
                        <Key className="h-3 w-3 text-stone-400" />
                        API Key
                      </label>
                      <div className="relative">
                        <input
                          type={showApiKey ? "text" : "password"}
                          value={formValues.api_key}
                          onChange={(e) => updateForm("api_key", e.target.value)}
                          placeholder={
                            editingProviderId !== "new"
                              ? (editingProvider?.has_api_key ? "已存入系统密钥环，留空保持不变" : "输入 API Key...")
                              : "输入 API Key..."
                          }
                          className="w-full bg-white border border-stone-200 rounded-lg pl-3 pr-9 py-2 text-xs text-stone-800 font-mono focus:outline-none focus:ring-1 focus:ring-[#8CA38A]/40 focus:border-[#8CA38A] transition-shadow"
                        />
                        <button
                          type="button"
                          onClick={() => setShowApiKey((v) => !v)}
                          className="absolute right-2 top-1/2 -translate-y-1/2 text-stone-400 hover:text-stone-700 transition-colors"
                          title={showApiKey ? "隐藏 API Key" : "显示 API Key"}
                        >
                          {showApiKey ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
                        </button>
                      </div>
                      {editingProviderId !== "new" && editingProvider?.has_api_key && (
                        <p className="flex items-center gap-1 text-[10px] text-amber-600 mt-1">
                          <Key className="h-2.5 w-2.5" />
                          已存入系统密钥环（重新输入将覆盖）
                        </p>
                      )}
                    </div>

                    {/* 可用模型 */}
                    <div className="space-y-1">
                      <div className="flex items-center justify-between">
                        <label className="text-xs font-semibold text-stone-500">可用模型</label>
                        <button
                          onClick={handleFetchModels}
                          disabled={isFetchingModels || (formValues.kind !== "openai" && formValues.kind !== "openai_compatible" && formValues.kind !== "ollama")}
                          className="text-[10px] flex items-center gap-1 text-[#8CA38A] hover:text-[#6C806A] transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                          title="尝试从接口自动获取模型列表"
                        >
                          <Download className="h-3 w-3" />
                          {isFetchingModels ? "获取中..." : "获取模型"}
                        </button>
                      </div>
                      <div className="flex gap-2">
                        <input
                          type="text"
                          value={formValues.modelDraft}
                          onChange={(e) => updateForm("modelDraft", e.target.value)}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") {
                              event.preventDefault();
                              addManualModels();
                            }
                          }}
                          placeholder="输入模型名称，多个名称可用逗号分隔"
                          className="flex-1 bg-white border border-stone-200 rounded-lg px-3 py-2 text-xs text-stone-800 font-mono focus:outline-none focus:ring-1 focus:ring-[#8CA38A]/40 focus:border-[#8CA38A] transition-shadow"
                        />
                        <button
                          type="button"
                          onClick={addManualModels}
                          className="rounded-lg border border-stone-200 bg-white px-3 text-[10px] font-semibold text-stone-600 hover:bg-stone-50"
                        >
                          添加
                        </button>
                      </div>
                      <p className="text-[10px] text-stone-400">
                        示例: {KIND_EXAMPLE_MODELS[formValues.kind] || "输入模型名称"}。自动获取的标签可手动修正。
                      </p>

                      <div className="max-h-64 space-y-2 overflow-y-auto pr-1">
                        {formValues.models.map((model) => (
                          <div key={model.id} className="rounded-lg border border-stone-200 bg-white p-2.5 space-y-2">
                            <div className="flex items-center justify-between gap-2">
                              <span className="min-w-0 truncate font-mono text-[11px] text-stone-700" title={model.id}>
                                {model.id}
                              </span>
                              <button
                                type="button"
                                onClick={() => removeProviderModel(model.id)}
                                className="shrink-0 rounded p-1 text-stone-400 hover:bg-rose-50 hover:text-rose-600"
                                title="移除模型"
                              >
                                <Trash2 className="h-3 w-3" />
                              </button>
                            </div>
                            <div className="flex flex-wrap items-center gap-1.5 text-[9px]">
                              <span className="text-stone-400">输入</span>
                              {(["text", "image"] as ModelModality[]).map((modality) => (
                                <button
                                  key={`input-${modality}`}
                                  type="button"
                                  onClick={() => toggleModelModality(model.id, "input_modalities", modality)}
                                  className={`rounded border px-1.5 py-0.5 ${
                                    model.capabilities.input_modalities.includes(modality)
                                      ? "border-blue-200 bg-blue-50 text-blue-700"
                                      : "border-stone-200 bg-stone-50 text-stone-400"
                                  }`}
                                >
                                  {modality === "text" ? "文本" : "图像"}
                                </button>
                              ))}
                              <span className="ml-1 text-stone-400">输出</span>
                              {(["text", "image"] as ModelModality[]).map((modality) => (
                                <button
                                  key={`output-${modality}`}
                                  type="button"
                                  onClick={() => toggleModelModality(model.id, "output_modalities", modality)}
                                  className={`rounded border px-1.5 py-0.5 ${
                                    model.capabilities.output_modalities.includes(modality)
                                      ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                                      : "border-stone-200 bg-stone-50 text-stone-400"
                                  }`}
                                >
                                  {modality === "text" ? "文本" : "图像"}
                                </button>
                              ))}
                              <button
                                type="button"
                                onClick={() => toggleEmbeddingCapability(model.id)}
                                className={`ml-1 rounded border px-1.5 py-0.5 ${
                                  model.capabilities.embedding
                                    ? "border-violet-200 bg-violet-50 text-violet-700"
                                    : "border-stone-200 bg-stone-50 text-stone-400"
                                }`}
                              >
                                嵌入
                              </button>
                              <label className="ml-auto flex items-center gap-1 text-stone-400">
                                上下文
                                <input
                                  key={`${model.id}-${model.context_window ?? "auto"}`}
                                  type="number"
                                  min={1024}
                                  max={10000000}
                                  step={1024}
                                  defaultValue={model.context_window ?? ""}
                                  onBlur={(event) => {
                                    const raw = event.currentTarget.value.trim();
                                    if (!raw) {
                                      updateModelContextWindow(model.id, null);
                                      return;
                                    }
                                    const parsed = Number(raw);
                                    const value = Number.isFinite(parsed)
                                      ? Math.min(10000000, Math.max(1024, Math.round(parsed)))
                                      : null;
                                    event.currentTarget.value = value === null ? "" : String(value);
                                    updateModelContextWindow(model.id, value);
                                  }}
                                  placeholder="自动"
                                  className="h-6 w-24 rounded border border-stone-200 bg-stone-50 px-1.5 text-right font-mono text-[9px] text-stone-600 outline-none focus:border-[#8CA38A]"
                                />
                              </label>
                            </div>
                          </div>
                        ))}
                        {formValues.models.length === 0 && (
                          <div className="rounded-lg border border-dashed border-stone-200 py-4 text-center text-[10px] text-stone-400">
                            尚未添加模型
                          </div>
                        )}
                      </div>
                    </div>

                    {/* 设为默认 */}
                    <label className="flex items-center gap-2 cursor-pointer group">
                      <input
                        type="checkbox"
                        checked={formValues.is_default}
                        onChange={(e) => updateForm("is_default", e.target.checked)}
                        className="rounded border-stone-300 text-[#8CA38A] focus:ring-[#8CA38A]/40 h-3.5 w-3.5"
                      />
                      <span className="text-xs text-stone-600 group-hover:text-stone-800 transition-colors">设为默认服务商</span>
                    </label>

                    {/* Test result in editor */}
                    {testResult && (
                      <div className={`flex items-center gap-2 px-3 py-2 rounded-lg text-xs border transition-all ${
                        testResult.success
                          ? "bg-emerald-50 border-emerald-200/60 text-emerald-700"
                          : "bg-red-50 border-red-200/60 text-red-600"
                      }`}>
                        {testResult.success ? <Check className="h-3.5 w-3.5 shrink-0" /> : <X className="h-3.5 w-3.5 shrink-0" />}
                        <span className="truncate">{testResult.message}</span>
                      </div>
                    )}

                    {/* Actions */}
                    <div className="flex items-center justify-between pt-3 border-t border-stone-200/60">
                      <div>
                        {editingProviderId !== "new" && (
                          <button
                            onClick={() => handleTestConnection(editingProviderId)}
                            disabled={isTesting}
                            className="flex items-center gap-1.5 text-xs text-stone-500 hover:text-amber-600 font-medium transition-colors disabled:opacity-50"
                          >
                            <Zap className="h-3.5 w-3.5" />
                            {isTesting ? "测试中..." : "测试连接"}
                          </button>
                        )}
                      </div>
                      <div className="flex items-center gap-2">
                        <button
                          onClick={closeEditor}
                          className="px-3 py-1.5 rounded-lg text-xs font-medium text-stone-500 hover:text-stone-700 hover:bg-stone-100 transition-colors"
                        >
                          取消
                        </button>
                        <button
                          onClick={handleSaveProvider}
                          disabled={isSaving || !formValues.name.trim()}
                          className="bg-[#8CA38A] text-white hover:bg-[#7A917A] rounded-lg px-4 py-1.5 text-xs font-semibold transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-1.5"
                        >
                          <Check className="h-3.5 w-3.5" />
                          {isSaving ? "保存中..." : "保存"}
                        </button>
                      </div>
                    </div>
                  </div>
                )}
                </div>

                {/* Add Provider Button */}
                {!editingProviderId && (
                  <button
                    onClick={openAddProvider}
                    className="w-full border border-dashed border-stone-300 hover:border-[#8CA38A] rounded-xl p-3 text-xs font-semibold text-stone-400 hover:text-[#6C806A] hover:bg-[#FAF9F5]/30 transition-all flex items-center justify-center gap-1.5"
                  >
                    <Plus className="h-3.5 w-3.5" />
                    添加服务商
                  </button>
                )}

                {/* Section 3: Cloud Sync */}
                <div className="border-t border-stone-200 pt-5 mt-2">
                  <div className="space-y-3">
                    <div className="flex items-center justify-between gap-3">
                      <div className="flex items-center gap-2">
                        <Server className="h-4 w-4 text-stone-500" />
                        <span className="text-xs font-semibold text-stone-600">云端同步</span>
                        <span
                          className={`h-2 w-2 rounded-full ${
                            syncStatus?.state === "idle"
                              ? "bg-emerald-500"
                              : syncStatus?.state === "syncing"
                                ? "bg-blue-500"
                                : syncStatus?.state === "pending" || syncStatus?.state === "e2ee_required" || syncStatus?.state === "e2ee_pending"
                                  ? "bg-amber-500"
                                  : "bg-stone-300"
                          }`}
                        />
                      </div>
                      <button
                        type="button"
                        onClick={handleSyncNow}
                        disabled={!syncStatus?.credentialConfigured || !syncStatus?.e2ee.confirmed || !syncStatus?.e2ee.transportReady || isSyncingNow || syncStatus?.syncing}
                        title={
                          !syncStatus?.credentialConfigured
                            ? "同步凭证尚未配置"
                            : !syncStatus.e2ee.confirmed
                              ? "端到端加密尚未确认"
                              : !syncStatus.e2ee.transportReady
                                ? "加密传输尚未接入"
                                : "立即同步"
                        }
                        className="h-8 w-8 inline-flex items-center justify-center rounded-md border border-stone-200 text-stone-500 hover:text-stone-800 hover:bg-stone-50 disabled:opacity-40 disabled:cursor-not-allowed"
                      >
                        <RefreshCw className={`h-4 w-4 ${isSyncingNow || syncStatus?.syncing ? "animate-spin" : ""}`} />
                      </button>
                    </div>

                    {syncStatus && (
                      <div className="grid grid-cols-2 gap-x-5 gap-y-3 text-xs">
                        <div className="min-w-0">
                          <span className="block text-stone-400 mb-1">同步网关</span>
                          <span className="block truncate text-stone-600" title={syncStatus.gatewayUrl}>
                            {syncStatus.gatewayUrl}
                          </span>
                        </div>
                        <div className="min-w-0">
                          <span className="block text-stone-400 mb-1">本机标识</span>
                          <span className="block truncate font-mono text-stone-600" title={syncStatus.deviceId}>
                            {syncStatus.deviceId}
                          </span>
                        </div>
                        <div>
                          <span className="block text-stone-400 mb-1">待推送</span>
                          <span className="text-stone-700 tabular-nums">{syncStatus.pendingCount}</span>
                        </div>
                        <div>
                          <span className="block text-stone-400 mb-1">冲突 / 失败</span>
                          <span className="text-stone-700 tabular-nums">
                            {syncStatus.conflictCount} / {syncStatus.deadLetterCount}
                          </span>
                        </div>
                      </div>
                    )}

                    {syncStatus && (
                      <section className="border-y border-stone-200 bg-white">
                        <div className="flex min-h-10 items-center justify-between gap-3 border-b border-stone-200 px-3 py-2">
                          <span className="flex min-w-0 items-center gap-2 text-xs font-semibold text-stone-700">
                            <LockKeyhole className="h-3.5 w-3.5 shrink-0" />
                            端到端加密
                          </span>
                          <span className={`text-[10px] font-semibold tabular-nums ${
                            syncStatus.e2ee.confirmed
                              ? "text-emerald-700"
                              : syncStatus.e2ee.keysetConfigured
                                ? "text-amber-700"
                                : "text-stone-400"
                          }`}>
                            {syncStatus.e2ee.rotationPending
                              ? `v${syncStatus.e2ee.confirmedKeyVersion} → v${syncStatus.e2ee.activeKeyVersion} 待确认`
                              : syncStatus.e2ee.confirmed
                              ? `Key v${syncStatus.e2ee.activeKeyVersion}`
                              : syncStatus.e2ee.keysetConfigured
                                ? "待确认"
                                : "未配置"}
                          </span>
                        </div>

                        {!syncRecoveryMaterial && !showSyncRestore && (
                          <div className="flex min-h-11 items-center justify-between gap-2 px-3 py-2">
                            <span className={`text-[10px] ${
                              syncStatus.e2ee.confirmed && !syncStatus.e2ee.transportReady
                                ? "text-amber-700"
                                : "text-stone-500"
                            }`}>
                              {syncStatus.e2ee.confirmed && !syncStatus.e2ee.transportReady
                                ? "加密传输待接入"
                                : syncStatus.e2ee.rotationPending
                                  ? "保存新恢复材料并确认后才会启用新密钥"
                                  : syncStatus.e2ee.keysetConfigured
                                    ? "恢复材料尚未确认"
                                  : "需要本机密钥与恢复材料"}
                            </span>
                            <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
                              {!syncStatus.e2ee.keysetConfigured && (
                                <button
                                  type="button"
                                  onClick={() => setShowSyncRestore(true)}
                                  disabled={isConfiguringSyncE2ee}
                                  className="inline-flex h-8 items-center gap-1.5 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-medium text-stone-700 hover:bg-stone-50 disabled:opacity-40"
                                >
                                  <FileKey2 className="h-3.5 w-3.5" />
                                  恢复
                                </button>
                              )}
                              {syncStatus.e2ee.keysetConfigured && syncStatus.e2ee.confirmedKeyVersion == null && (
                                <button
                                  type="button"
                                  onClick={handleDiscardSyncE2eeSetup}
                                  disabled={isConfiguringSyncE2ee}
                                  title="丢弃未确认密钥"
                                  className="flex h-8 w-8 items-center justify-center rounded-md border border-stone-300 bg-white text-stone-500 hover:border-rose-200 hover:bg-rose-50 hover:text-rose-700 disabled:opacity-40"
                                >
                                  <Trash2 className="h-3.5 w-3.5" />
                                </button>
                              )}
                              <button
                                type="button"
                                onClick={handleBeginSyncE2eeSetup}
                                disabled={isConfiguringSyncE2ee}
                                className="inline-flex h-8 items-center gap-1.5 rounded-md bg-stone-800 px-2.5 text-[11px] font-medium text-white hover:bg-stone-700 disabled:opacity-40"
                              >
                                <Key className="h-3.5 w-3.5" />
                                {syncStatus.e2ee.keysetConfigured ? "导出恢复材料" : "创建密钥"}
                              </button>
                              {syncStatus.e2ee.confirmed && (
                                <>
                                  <button
                                    type="button"
                                    onClick={() => setShowSyncRestore(true)}
                                    disabled={isConfiguringSyncE2ee}
                                    className="inline-flex h-8 items-center gap-1.5 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-medium text-stone-700 hover:bg-stone-50 disabled:opacity-40"
                                  >
                                    <FileKey2 className="h-3.5 w-3.5" />
                                    导入升级
                                  </button>
                                  <button
                                    type="button"
                                    onClick={handleBeginSyncE2eeRotation}
                                    disabled={isConfiguringSyncE2ee}
                                    className="inline-flex h-8 items-center gap-1.5 rounded-md border border-amber-300 bg-amber-50 px-2.5 text-[11px] font-medium text-amber-800 hover:bg-amber-100 disabled:opacity-40"
                                  >
                                    <RefreshCw className="h-3.5 w-3.5" />
                                    轮换
                                  </button>
                                </>
                              )}
                            </div>
                          </div>
                        )}

                        {showSyncRestore && !syncRecoveryMaterial && (
                          <div className="space-y-2.5 px-3 py-3">
                            <label className="block text-[10px] font-semibold text-stone-600">
                              恢复密钥
                              <textarea
                                value={syncRecoveryKeyInput}
                                onChange={(event) => setSyncRecoveryKeyInput(event.target.value)}
                                rows={2}
                                spellCheck={false}
                                autoComplete="off"
                                className="mt-1 w-full resize-none rounded-md border border-stone-200 bg-white px-2.5 py-2 font-mono text-[10px] leading-relaxed text-stone-700 outline-none focus:border-stone-400"
                              />
                            </label>
                            <label className="block text-[10px] font-semibold text-stone-600">
                              加密恢复包
                              <textarea
                                value={syncRecoveryBundleInput}
                                onChange={(event) => setSyncRecoveryBundleInput(event.target.value)}
                                rows={3}
                                spellCheck={false}
                                autoComplete="off"
                                className="mt-1 w-full resize-none rounded-md border border-stone-200 bg-white px-2.5 py-2 font-mono text-[10px] leading-relaxed text-stone-700 outline-none focus:border-stone-400"
                              />
                            </label>
                            <div className="flex justify-end gap-2">
                              <button
                                type="button"
                                onClick={() => {
                                  setShowSyncRestore(false);
                                  setSyncRecoveryKeyInput("");
                                  setSyncRecoveryBundleInput("");
                                }}
                                className="h-8 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-medium text-stone-700 hover:bg-stone-50"
                              >
                                取消
                              </button>
                              <button
                                type="button"
                                onClick={handleRestoreSyncE2ee}
                                disabled={isConfiguringSyncE2ee || !syncRecoveryKeyInput.trim() || !syncRecoveryBundleInput.trim()}
                                className="inline-flex h-8 items-center gap-1.5 rounded-md bg-stone-800 px-2.5 text-[11px] font-medium text-white hover:bg-stone-700 disabled:opacity-40"
                              >
                                <FileKey2 className="h-3.5 w-3.5" />
                                恢复
                              </button>
                            </div>
                          </div>
                        )}

                        {syncRecoveryMaterial && (
                          <div className="space-y-2.5 px-3 py-3">
                            <div className="relative">
                              <span className="block text-[10px] font-semibold text-stone-600">恢复密钥</span>
                              <textarea
                                readOnly
                                value={syncRecoveryMaterial.recoveryKey}
                                rows={2}
                                spellCheck={false}
                                className="mt-1 w-full resize-none rounded-md border border-stone-200 bg-stone-50 px-2.5 py-2 pr-9 font-mono text-[10px] leading-relaxed text-stone-700"
                              />
                              <button
                                type="button"
                                onClick={() => handleCopyRecoveryValue(syncRecoveryMaterial.recoveryKey)}
                                title="复制恢复密钥"
                                className="absolute bottom-1.5 right-1.5 flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-white hover:text-stone-700"
                              >
                                <Copy className="h-3.5 w-3.5" />
                              </button>
                            </div>
                            <div className="relative">
                              <span className="block text-[10px] font-semibold text-stone-600">加密恢复包</span>
                              <textarea
                                readOnly
                                value={syncRecoveryMaterial.recoveryBundle}
                                rows={3}
                                spellCheck={false}
                                className="mt-1 w-full resize-none rounded-md border border-stone-200 bg-stone-50 px-2.5 py-2 pr-9 font-mono text-[10px] leading-relaxed text-stone-700"
                              />
                              <button
                                type="button"
                                onClick={() => handleCopyRecoveryValue(syncRecoveryMaterial.recoveryBundle)}
                                title="复制加密恢复包"
                                className="absolute bottom-1.5 right-1.5 flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-white hover:text-stone-700"
                              >
                                <Copy className="h-3.5 w-3.5" />
                              </button>
                            </div>
                            {!syncStatus.e2ee.confirmed ? (
                              <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
                                <label className="flex items-center gap-2 text-[10px] text-stone-600">
                                  <input
                                    type="checkbox"
                                    checked={syncRecoveryAcknowledged}
                                    onChange={(event) => setSyncRecoveryAcknowledged(event.target.checked)}
                                    className="h-3.5 w-3.5 rounded border-stone-300"
                                  />
                                  已分别保存两项恢复材料
                                </label>
                                <button
                                  type="button"
                                  onClick={handleConfirmSyncE2eeSetup}
                                  disabled={!syncRecoveryAcknowledged || isConfiguringSyncE2ee}
                                  className="inline-flex h-8 items-center gap-1.5 rounded-md bg-emerald-700 px-2.5 text-[11px] font-medium text-white hover:bg-emerald-800 disabled:opacity-40"
                                >
                                  <ShieldCheck className="h-3.5 w-3.5" />
                                  确认密钥
                                </button>
                              </div>
                            ) : (
                              <div className="flex justify-end pt-1">
                                <button
                                  type="button"
                                  onClick={() => {
                                    setSyncRecoveryMaterial(null);
                                    setSyncRecoveryAcknowledged(false);
                                  }}
                                  className="h-8 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-medium text-stone-700 hover:bg-stone-50"
                                >
                                  完成
                                </button>
                              </div>
                            )}
                          </div>
                        )}
                      </section>
                    )}

                    {syncStatus?.credentialConfigured && syncStatus.e2ee.confirmed && (
                      <section className="border-y border-stone-200 bg-white">
                        <div className="flex min-h-10 items-center justify-between gap-3 border-b border-stone-200 px-3 py-2">
                          <span className="flex items-center gap-2 text-xs font-semibold text-stone-700">
                            <Laptop className="h-3.5 w-3.5" />
                            安全配对
                          </span>
                          {!syncPairingInvite && (
                            <button
                              type="button"
                              onClick={handleStartSyncPairing}
                              disabled={isPairingSyncDevice}
                              className="inline-flex h-8 items-center gap-1.5 rounded-md bg-stone-800 px-2.5 text-[11px] font-medium text-white hover:bg-stone-700 disabled:opacity-40"
                            >
                              <Plus className="h-3.5 w-3.5" />
                              配对新设备
                            </button>
                          )}
                        </div>
                        {syncPairingInvite ? (
                          <div className="space-y-2.5 px-3 py-3">
                            <p className="text-[10px] leading-relaxed text-amber-700">
                              仅通过可信通道发送此一次性配对码；它将在 {formatSyncDeviceTime(syncPairingInvite.expiresAt)} 失效。
                            </p>
                            <div className="relative">
                              <textarea
                                readOnly
                                value={syncPairingInvite.pairingCode}
                                rows={3}
                                spellCheck={false}
                                className="w-full resize-none rounded-md border border-stone-200 bg-stone-50 px-2.5 py-2 pr-9 font-mono text-[10px] leading-relaxed text-stone-700"
                              />
                              <button
                                type="button"
                                onClick={() => handleCopyRecoveryValue(syncPairingInvite.pairingCode)}
                                title="复制一次性配对码"
                                className="absolute bottom-1.5 right-1.5 flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-white hover:text-stone-700"
                              >
                                <Copy className="h-3.5 w-3.5" />
                              </button>
                            </div>
                            {syncPairingDevice && (
                              <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[11px] text-amber-900">
                                <span className="font-semibold">{syncPairingDevice.deviceName}</span>
                                <span className="ml-2 text-amber-700">{syncPairingDevice.platform || "未知平台"}</span>
                                <p className="mt-1 break-all font-mono text-[9px] text-amber-700">{syncPairingDevice.deviceId}</p>
                              </div>
                            )}
                            <div className="flex flex-wrap justify-end gap-2">
                              <button
                                type="button"
                                onClick={() => {
                                  setSyncPairingInvite(null);
                                  setSyncPairingDevice(null);
                                }}
                                className="h-8 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-medium text-stone-700 hover:bg-stone-50"
                              >
                                关闭
                              </button>
                              {!syncPairingDevice ? (
                                <button
                                  type="button"
                                  onClick={handleCheckSyncPairing}
                                  disabled={isPairingSyncDevice}
                                  className="h-8 rounded-md bg-stone-800 px-2.5 text-[11px] font-medium text-white hover:bg-stone-700 disabled:opacity-40"
                                >
                                  {isPairingSyncDevice ? "检查中..." : "检查申请"}
                                </button>
                              ) : (
                                <button
                                  type="button"
                                  onClick={handleApproveSyncPairing}
                                  disabled={isPairingSyncDevice}
                                  className="inline-flex h-8 items-center gap-1.5 rounded-md bg-emerald-700 px-2.5 text-[11px] font-medium text-white hover:bg-emerald-800 disabled:opacity-40"
                                >
                                  <ShieldCheck className="h-3.5 w-3.5" />
                                  批准并发送密钥
                                </button>
                              )}
                            </div>
                          </div>
                        ) : (
                          <p className="px-3 py-2.5 text-[10px] leading-relaxed text-stone-500">
                            使用 SPAKE2 建立一次性认证通道，新设备会同时获得独立设备凭证与当前多版本 keyset。
                          </p>
                        )}
                      </section>
                    )}

                    {syncConflicts.length > 0 && (
                      <section className="border-y border-rose-200 bg-rose-50/40">
                        <div className="flex h-9 items-center justify-between border-b border-rose-200 px-3">
                          <span className="flex items-center gap-2 text-xs font-semibold text-rose-800">
                            <GitCompareArrows className="h-3.5 w-3.5" />
                            同步冲突
                          </span>
                          <span className="text-[11px] tabular-nums text-rose-600">{syncConflicts.length}</span>
                        </div>
                        <div className="divide-y divide-rose-200">
                          {syncConflicts.map((conflict) => {
                            const resolving = resolvingConflictId === conflict.id;
                            const localPreview = conflictPayloadPreview(
                              conflict.localPayload,
                              conflict.conflictingFields,
                              conflict.localDeleted,
                            );
                            const remotePreview = conflictPayloadPreview(
                              conflict.remotePayload,
                              conflict.conflictingFields,
                              conflict.remoteDeleted,
                            );
                            return (
                              <article key={conflict.id} className="space-y-3 px-3 py-3">
                                <div className="flex min-w-0 items-start justify-between gap-3">
                                  <div className="min-w-0">
                                    <div className="flex flex-wrap items-center gap-2">
                                      <span className="text-xs font-semibold text-stone-800">
                                        {SYNC_ENTITY_LABELS[conflict.entityType] || conflict.entityType}
                                      </span>
                                      <span className="font-mono text-[10px] text-stone-400">
                                        r{conflict.baseRevision ?? 0} → r{conflict.remoteRevision ?? "?"}
                                      </span>
                                    </div>
                                    <p className="mt-0.5 truncate font-mono text-[10px] text-stone-500" title={conflict.entityId}>
                                      {conflict.entityId}
                                    </p>
                                  </div>
                                  <div className="flex flex-wrap justify-end gap-1">
                                    {conflict.conflictingFields.map((field) => (
                                      <span key={field} className="rounded-sm border border-rose-200 bg-white px-1.5 py-0.5 font-mono text-[9px] text-rose-700">
                                        {field}
                                      </span>
                                    ))}
                                  </div>
                                </div>

                                <div className="grid grid-cols-2 gap-2">
                                  <div className="min-w-0 border-r border-rose-200 pr-2">
                                    <div className="mb-1 flex items-center gap-1.5 text-[10px] font-semibold text-stone-600">
                                      <Laptop className="h-3 w-3" />本机
                                    </div>
                                    <pre className="max-h-28 overflow-auto whitespace-pre-wrap break-all text-[10px] leading-relaxed text-stone-600">
                                      {localPreview}
                                    </pre>
                                  </div>
                                  <div className="min-w-0 pl-1">
                                    <div className="mb-1 flex items-center gap-1.5 text-[10px] font-semibold text-stone-600">
                                      <Cloud className="h-3 w-3" />云端
                                    </div>
                                    <pre className="max-h-28 overflow-auto whitespace-pre-wrap break-all text-[10px] leading-relaxed text-stone-600">
                                      {remotePreview}
                                    </pre>
                                  </div>
                                </div>

                                <div className="flex justify-end gap-2">
                                  <button
                                    type="button"
                                    onClick={() => handleResolveSyncConflict(conflict, "keep_remote")}
                                    disabled={!conflict.remoteReady || resolving}
                                    className="inline-flex h-8 items-center gap-1.5 rounded-md border border-stone-300 bg-white px-2.5 text-[11px] font-medium text-stone-700 hover:bg-stone-50 disabled:cursor-not-allowed disabled:opacity-40"
                                  >
                                    <Cloud className="h-3.5 w-3.5" />
                                    接受云端
                                  </button>
                                  <button
                                    type="button"
                                    onClick={() => handleResolveSyncConflict(conflict, "keep_local")}
                                    disabled={!conflict.remoteReady || resolving}
                                    className="inline-flex h-8 items-center gap-1.5 rounded-md bg-stone-800 px-2.5 text-[11px] font-medium text-white hover:bg-stone-700 disabled:cursor-not-allowed disabled:opacity-40"
                                  >
                                    <Laptop className="h-3.5 w-3.5" />
                                    保留本机
                                  </button>
                                </div>
                              </article>
                            );
                          })}
                        </div>
                      </section>
                    )}

                    {syncStatus && !syncStatus.credentialConfigured && !syncStatus.e2ee.keysetConfigured && (
                      <section className="border-y border-stone-200 bg-white">
                        <div className="flex h-9 items-center gap-2 border-b border-stone-200 px-3 text-xs font-semibold text-stone-700">
                          <Laptop className="h-3.5 w-3.5" />
                          加入已有设备
                        </div>
                        <div className="space-y-2.5 px-3 py-3">
                          <input
                            value={syncPairingDeviceName}
                            onChange={(event) => setSyncPairingDeviceName(event.target.value)}
                            placeholder="本机设备名称"
                            disabled={syncPairingJoin != null}
                            className="h-9 w-full rounded-md border border-stone-200 bg-white px-2.5 text-xs text-stone-700 outline-none focus:border-stone-400 disabled:bg-stone-50"
                          />
                          <textarea
                            value={syncPairingCodeInput}
                            onChange={(event) => setSyncPairingCodeInput(event.target.value)}
                            placeholder="粘贴旧设备生成的一次性配对码"
                            rows={3}
                            spellCheck={false}
                            autoComplete="off"
                            disabled={syncPairingJoin != null}
                            className="w-full resize-none rounded-md border border-stone-200 bg-white px-2.5 py-2 font-mono text-[10px] leading-relaxed text-stone-700 outline-none focus:border-stone-400 disabled:bg-stone-50"
                          />
                          <div className="flex items-center justify-between gap-2">
                            <span className="text-[10px] text-stone-500">
                              {syncPairingJoin ? "已发出申请，等待旧设备批准…" : "配对完成后自动安装凭证和 E2EE keyset"}
                            </span>
                            <button
                              type="button"
                              onClick={handleJoinSyncPairing}
                              disabled={isPairingSyncDevice || syncPairingJoin != null || !syncPairingCodeInput.trim() || !syncPairingDeviceName.trim()}
                              className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-md bg-stone-800 px-2.5 text-[11px] font-medium text-white hover:bg-stone-700 disabled:opacity-40"
                            >
                              <ShieldCheck className={`h-3.5 w-3.5 ${syncPairingJoin ? "animate-pulse" : ""}`} />
                              {syncPairingJoin ? "等待批准" : "申请配对"}
                            </button>
                          </div>
                        </div>
                      </section>
                    )}

                    {syncStatus && !syncStatus.credentialConfigured && (
                      <div className="flex items-center gap-2">
                        <div className="relative min-w-0 flex-1">
                          <Key className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-stone-400" />
                          <input
                            type={showSyncToken ? "text" : "password"}
                            value={syncToken}
                            onChange={(event) => setSyncToken(event.target.value)}
                            placeholder="设备同步令牌"
                            autoComplete="off"
                            className="h-9 w-full rounded-md border border-stone-200 bg-white pl-8 pr-9 text-xs text-stone-700 outline-none focus:border-stone-400"
                          />
                          <button
                            type="button"
                            onClick={() => setShowSyncToken((visible) => !visible)}
                            title={showSyncToken ? "隐藏令牌" : "显示令牌"}
                            className="absolute right-1 top-1/2 flex h-7 w-7 -translate-y-1/2 items-center justify-center text-stone-400 hover:text-stone-700"
                          >
                            {showSyncToken ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
                          </button>
                        </div>
                        <button
                          type="button"
                          onClick={handleSaveSyncCredential}
                          disabled={!syncToken.trim() || isSavingSyncCredential}
                          className="h-9 shrink-0 rounded-md bg-stone-800 px-3 text-xs font-medium text-white hover:bg-stone-700 disabled:cursor-not-allowed disabled:opacity-40"
                        >
                          保存
                        </button>
                      </div>
                    )}
                    {syncStatus?.credentialConfigured && (
                      <div className="flex h-9 items-center justify-between rounded-md border border-emerald-200 bg-emerald-50 px-3 text-xs text-emerald-700">
                        <span className="flex items-center gap-2">
                          <ShieldCheck className="h-3.5 w-3.5" />
                          凭证已存入系统密钥环
                        </span>
                        <button
                          type="button"
                          onClick={handleClearSyncCredential}
                          disabled={isSavingSyncCredential || isSyncingNow || syncStatus.syncing}
                          title="清除同步凭证"
                          className="flex h-7 w-7 items-center justify-center text-emerald-700 hover:text-rose-600 disabled:opacity-40"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    )}
                    {syncStatus?.credentialConfigured && syncDevices.length > 0 && (
                      <section className="border-y border-stone-200">
                        <div className="flex h-9 items-center justify-between border-b border-stone-200 px-3">
                          <span className="flex items-center gap-2 text-xs font-semibold text-stone-700">
                            <Laptop className="h-3.5 w-3.5" />
                            同步设备
                          </span>
                          <span className="text-[10px] tabular-nums text-stone-400">
                            {syncDevices.filter((device) => device.revokedAt == null).length} / {syncDevices.length}
                          </span>
                        </div>
                        <div className="divide-y divide-stone-200">
                          {syncDevices.map((device) => {
                            const revoked = device.revokedAt != null;
                            const revoking = revokingDeviceId === device.id;
                            return (
                              <div
                                key={device.id}
                                className={`flex min-h-16 items-center gap-3 px-3 py-2.5 ${revoked ? "bg-stone-50 opacity-65" : "bg-white"}`}
                              >
                                <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-md border ${
                                  revoked
                                    ? "border-stone-200 bg-stone-100 text-stone-400"
                                    : device.current
                                      ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                                      : "border-stone-200 bg-stone-50 text-stone-600"
                                }`}>
                                  {revoked ? <ShieldOff className="h-4 w-4" /> : <Laptop className="h-4 w-4" />}
                                </div>
                                <div className="min-w-0 flex-1">
                                  <div className="flex flex-wrap items-center gap-1.5">
                                    <span className="truncate text-xs font-semibold text-stone-800">{device.name}</span>
                                    {device.current && (
                                      <span className="rounded-sm border border-emerald-200 bg-emerald-50 px-1.5 py-0.5 text-[9px] font-semibold text-emerald-700">本机</span>
                                    )}
                                    {revoked && (
                                      <span className="rounded-sm border border-stone-200 bg-white px-1.5 py-0.5 text-[9px] font-semibold text-stone-500">已撤销</span>
                                    )}
                                  </div>
                                  <div className="mt-0.5 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-0.5 text-[9px] text-stone-400">
                                    <span>{device.platform || "未知平台"}</span>
                                    <span>在线 {formatSyncDeviceTime(device.lastSeenAt)}</span>
                                    <span className="tabular-nums">ack {device.lastAckCursor}</span>
                                    <span className="max-w-44 truncate font-mono" title={device.id}>{device.id}</span>
                                  </div>
                                </div>
                                {!device.current && !revoked && (
                                  <button
                                    type="button"
                                    onClick={() => handleRevokeSyncDevice(device)}
                                    disabled={revoking}
                                    title="撤销设备"
                                    className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-700 disabled:cursor-not-allowed disabled:opacity-40"
                                  >
                                    <ShieldOff className={`h-4 w-4 ${revoking ? "animate-pulse" : ""}`} />
                                  </button>
                                )}
                              </div>
                            );
                          })}
                        </div>
                      </section>
                    )}
                    {syncStatus?.lastErrorCode && (
                      <div className="text-xs text-rose-700 bg-rose-50 border border-rose-200 px-3 py-2 rounded-md">
                        {syncStatus.lastErrorCode}
                      </div>
                    )}
                    {syncStatusError && <div className="text-xs text-rose-600">{syncStatusError}</div>}
                  </div>
                </div>
              </div>
            )}

            {/* 4. WEB SEARCH TAB */}
            {activeTab === "web" && (
              <div className="space-y-5">
                <div className="flex items-start justify-between gap-4 border-b border-stone-200 pb-3">
                  <div className="min-w-0">
                    <h3 className="text-sm font-semibold text-stone-850">联网搜索 Provider</h3>
                  </div>
                  <button
                    type="button"
                    onClick={saveSearchProviders}
                    disabled={isSavingSearchProviders || searchProviderForm.fallbackOrder.length === 0}
                    className="shrink-0 rounded-md bg-indigo-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-indigo-700 disabled:opacity-40"
                  >
                    {isSavingSearchProviders ? "保存中..." : "保存配置"}
                  </button>
                </div>

                <div
                  className={`flex items-center gap-2 border-y px-3 py-2 text-[11px] ${
                    secretStoreStatus?.available
                      ? "border-emerald-200 bg-emerald-50/60 text-emerald-700"
                      : "border-rose-200 bg-rose-50/60 text-rose-700"
                  }`}
                  title={secretStoreStatus?.error || undefined}
                >
                  <Key className="h-3.5 w-3.5 shrink-0" />
                  {secretStoreStatus?.available
                    ? "Brave API Key 仅保存在本机系统密钥环"
                    : secretStoreStatus?.error || "正在检查系统密钥环"}
                </div>

                {searchProviderMessage && (
                  <div className={`border px-3 py-2 text-[11px] ${
                    searchProviderMessage.success
                      ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                      : "border-rose-200 bg-rose-50 text-rose-700"
                  }`}>
                    {searchProviderMessage.text}
                  </div>
                )}

                <section>
                  <div className="mb-2">
                    <h4 className="text-xs font-semibold text-stone-700">Provider 状态</h4>
                  </div>
                  <div className="divide-y divide-stone-200 border-y border-stone-200">
                    {SEARCH_PROVIDER_IDS.map((provider) => {
                      const configured = provider === "duckduckgo"
                        || provider === "bing"
                        || (provider === "searxng" && Boolean(savedSearchProviderSettings?.searxng_base_url))
                        || (provider === "brave" && Boolean(savedSearchProviderSettings?.has_brave_api_key));
                      const test = searchProviderTests[provider];
                      const testing = testingSearchProvider === provider;
                      return (
                        <div key={provider} className="flex min-h-12 items-center gap-3 px-2 py-2">
                          <span className={`h-2 w-2 shrink-0 rounded-full ${
                            test?.success ? "bg-emerald-500" : test ? "bg-rose-500" : "bg-stone-300"
                          }`} />
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <span className="text-[11px] font-semibold text-stone-700">{SEARCH_PROVIDER_LABELS[provider]}</span>
                              <span className="text-[9px] text-stone-400">
                                {test?.success ? "连接正常" : test ? "连接异常" : configured ? "待测试" : "未配置"}
                              </span>
                            </div>
                            {test && (
                              <p className={`truncate text-[9px] ${test.success ? "text-emerald-600" : "text-rose-600"}`} title={test.message}>
                                {test.message}{test.latency_ms > 0 ? ` · ${test.latency_ms} ms` : ""}
                              </p>
                            )}
                          </div>
                          <button
                            type="button"
                            title={configured ? "测试连接" : "请先配置并保存"}
                            disabled={!configured || testingSearchProvider !== null}
                            onClick={() => testSearchProvider(provider)}
                            className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-30"
                          >
                            <RefreshCw className={`h-3.5 w-3.5 ${testing ? "animate-spin" : ""}`} />
                          </button>
                        </div>
                      );
                    })}
                  </div>
                </section>

                <section className="space-y-3 border-t border-stone-200 pt-4">
                  <div>
                    <h4 className="text-xs font-semibold text-stone-700">SearXNG</h4>
                  </div>
                  <label className="block space-y-1">
                    <span className="text-[10px] font-semibold text-stone-600">服务地址</span>
                    <input
                      value={searchProviderForm.searxngBaseUrl}
                      onChange={(event) => setSearchProviderForm((form) => ({ ...form, searxngBaseUrl: event.target.value }))}
                      placeholder="https://search.example.com 或 http://127.0.0.1:8888"
                      className="w-full rounded-md border border-stone-200 bg-white px-3 py-2 text-[11px] font-mono text-stone-700 outline-none focus:ring-1 focus:ring-indigo-200"
                    />
                  </label>
                </section>

                <section className="space-y-3 border-t border-stone-200 pt-4">
                  <div>
                    <h4 className="text-xs font-semibold text-stone-700">Brave Search API</h4>
                  </div>
                  <label className="block space-y-1">
                    <span className="text-[10px] font-semibold text-stone-600">API Key</span>
                    <div className="flex gap-2">
                      <div className="relative min-w-0 flex-1">
                        <input
                          type={showBraveSearchKey ? "text" : "password"}
                          value={searchProviderForm.braveApiKey}
                          maxLength={1024}
                          disabled={searchProviderForm.clearBraveApiKey}
                          onChange={(event) => setSearchProviderForm((form) => ({
                            ...form,
                            braveApiKey: event.target.value,
                            clearBraveApiKey: false,
                          }))}
                          placeholder={searchProviderForm.hasBraveApiKey ? "已保存在系统密钥环；留空保持不变" : "输入 Brave Search API Key"}
                          className="w-full rounded-md border border-stone-200 bg-white px-3 py-2 pr-9 text-[11px] font-mono text-stone-700 outline-none focus:ring-1 focus:ring-indigo-200 disabled:bg-stone-50"
                        />
                        <button
                          type="button"
                          title={showBraveSearchKey ? "隐藏密钥" : "显示密钥"}
                          onClick={() => setShowBraveSearchKey((visible) => !visible)}
                          className="absolute right-2 top-1/2 -translate-y-1/2 text-stone-400 hover:text-stone-700"
                        >
                          {showBraveSearchKey ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
                        </button>
                      </div>
                      {searchProviderForm.hasBraveApiKey && (
                        <button
                          type="button"
                          onClick={() => setSearchProviderForm((form) => ({
                            ...form,
                            braveApiKey: "",
                            clearBraveApiKey: !form.clearBraveApiKey,
                            fallbackOrder: !form.clearBraveApiKey
                              ? form.fallbackOrder.filter((provider) => provider !== "brave")
                              : form.fallbackOrder,
                          }))}
                          className={`rounded-md border px-2.5 text-[10px] font-semibold ${
                            searchProviderForm.clearBraveApiKey
                              ? "border-amber-200 bg-amber-50 text-amber-700"
                              : "border-stone-200 text-stone-500 hover:bg-rose-50 hover:text-rose-600"
                          }`}
                        >
                          {searchProviderForm.clearBraveApiKey ? "保留" : "删除"}
                        </button>
                      )}
                    </div>
                  </label>
                </section>

                <section className="border-t border-stone-200 pt-4">
                  <div className="flex flex-col items-stretch gap-2 sm:flex-row sm:items-start sm:justify-between sm:gap-4">
                    <div className="min-w-0">
                      <h4 className="text-xs font-semibold text-stone-700">自动回退顺序</h4>
                    </div>
                    <select
                      value=""
                      onChange={(event) => {
                        const provider = event.target.value as SearchProviderId;
                        if (!provider || searchProviderForm.fallbackOrder.includes(provider)) return;
                        setSearchProviderForm((form) => ({ ...form, fallbackOrder: [...form.fallbackOrder, provider] }));
                      }}
                      className="w-full rounded-md border border-stone-200 bg-white px-2.5 py-1.5 text-[10px] text-stone-600 outline-none sm:w-48"
                    >
                      <option value="">添加 Provider...</option>
                      {SEARCH_PROVIDER_IDS.map((provider) => {
                        const configured = provider === "duckduckgo"
                          || provider === "bing"
                          || (provider === "searxng" && Boolean(searchProviderForm.searxngBaseUrl.trim()))
                          || (provider === "brave"
                            && !searchProviderForm.clearBraveApiKey
                            && (searchProviderForm.hasBraveApiKey || Boolean(searchProviderForm.braveApiKey.trim())));
                        return (
                          <option
                            key={provider}
                            value={provider}
                            disabled={!configured || searchProviderForm.fallbackOrder.includes(provider)}
                          >
                            {SEARCH_PROVIDER_LABELS[provider]}{configured ? "" : "（未配置）"}
                          </option>
                        );
                      })}
                    </select>
                  </div>
                  <div className="mt-2 divide-y divide-stone-200 border-y border-stone-200">
                    {searchProviderForm.fallbackOrder.map((provider, index) => (
                      <div key={provider} className="flex h-10 items-center gap-2 px-2">
                        <span className="w-5 text-center text-[10px] tabular-nums text-stone-400">{index + 1}</span>
                        <span className="min-w-0 flex-1 truncate text-[10px] font-semibold text-stone-700">
                          {SEARCH_PROVIDER_LABELS[provider]}
                        </span>
                        <button
                          type="button"
                          title="提高优先级"
                          disabled={index === 0}
                          onClick={() => setSearchProviderForm((form) => {
                            const next = [...form.fallbackOrder];
                            [next[index - 1], next[index]] = [next[index], next[index - 1]];
                            return { ...form, fallbackOrder: next };
                          })}
                          className="flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-25"
                        >
                          <ArrowUp className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          title="降低优先级"
                          disabled={index === searchProviderForm.fallbackOrder.length - 1}
                          onClick={() => setSearchProviderForm((form) => {
                            const next = [...form.fallbackOrder];
                            [next[index], next[index + 1]] = [next[index + 1], next[index]];
                            return { ...form, fallbackOrder: next };
                          })}
                          className="flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-25"
                        >
                          <ArrowDown className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          title="移出自动回退链"
                          disabled={searchProviderForm.fallbackOrder.length === 1}
                          onClick={() => setSearchProviderForm((form) => ({
                            ...form,
                            fallbackOrder: form.fallbackOrder.filter((value) => value !== provider),
                          }))}
                          className="flex h-7 w-7 items-center justify-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-600 disabled:opacity-25"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    ))}
                  </div>
                </section>
              </div>
            )}

            {/* 5. MCP TAB */}
            {activeTab === "mcp" && (
              <div className="space-y-5">
                <div className="flex items-start justify-between border-b border-stone-200 pb-3">
                  <div>
                    <h3 className="text-sm font-semibold text-stone-850">MCP Server</h3>
                    <p className="mt-0.5 text-[11px] text-stone-400">
                      接入本机 stdio 或 Streamable HTTP Server，工具需在角色卡中单独授权。
                    </p>
                  </div>
                  {editingMcpId === null && (
                    <button
                      type="button"
                      onClick={openNewMcpServer}
                      className="flex items-center gap-1.5 rounded-md bg-indigo-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-indigo-700"
                    >
                      <Plus className="h-3.5 w-3.5" />
                      新建 Server
                    </button>
                  )}
                </div>

                <div
                  className={`flex items-center gap-2 border-y px-3 py-2 text-[11px] ${
                    secretStoreStatus?.available
                      ? "border-emerald-200 bg-emerald-50/60 text-emerald-700"
                      : "border-rose-200 bg-rose-50/60 text-rose-700"
                  }`}
                  title={secretStoreStatus?.error || undefined}
                >
                  <Key className="h-3.5 w-3.5 shrink-0" />
                  {secretStoreStatus?.available
                    ? "环境变量值与 Bearer Token 仅保存在本机系统密钥环"
                    : secretStoreStatus?.error || "正在检查系统密钥环"}
                </div>

                {mcpMessage && (
                  <div className={`border px-3 py-2 text-[11px] ${
                    mcpMessage.success
                      ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                      : "border-rose-200 bg-rose-50 text-rose-700"
                  }`}>
                    {mcpMessage.text}
                  </div>
                )}

                {editingMcpId !== null ? (
                  <div className="space-y-4">
                    <div className="grid grid-cols-[1fr_auto] gap-3">
                      <label className="space-y-1">
                        <span className="text-[11px] font-semibold text-stone-600">名称</span>
                        <input
                          value={mcpForm.name}
                          onChange={(event) => setMcpForm((form) => ({ ...form, name: event.target.value }))}
                          placeholder="例如：Notion、Filesystem、公司知识库"
                          className="w-full rounded-md border border-stone-200 bg-white px-3 py-2 text-xs outline-none focus:ring-1 focus:ring-indigo-300"
                        />
                      </label>
                      <label className="space-y-1">
                        <span className="block text-[11px] font-semibold text-stone-600">状态</span>
                        <button
                          type="button"
                          onClick={() => setMcpForm((form) => ({ ...form, enabled: !form.enabled }))}
                          className={`h-[34px] rounded-md border px-3 text-[11px] font-semibold ${
                            mcpForm.enabled
                              ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                              : "border-stone-200 bg-stone-100 text-stone-500"
                          }`}
                        >
                          {mcpForm.enabled ? "已启用" : "已停用"}
                        </button>
                      </label>
                    </div>

                    <div>
                      <span className="mb-1.5 block text-[11px] font-semibold text-stone-600">连接方式</span>
                      <div className="inline-flex rounded-md border border-stone-200 bg-stone-50 p-0.5">
                        {([
                          ["stdio", "本机 stdio"],
                          ["streamable_http", "Streamable HTTP"],
                        ] as const).map(([value, label]) => (
                          <button
                            key={value}
                            type="button"
                            onClick={() => setMcpForm((form) => ({ ...form, transportType: value }))}
                            className={`rounded px-3 py-1.5 text-[10px] font-semibold ${
                              mcpForm.transportType === value
                                ? "bg-white text-stone-800 shadow-sm"
                                : "text-stone-500 hover:text-stone-800"
                            }`}
                          >
                            {label}
                          </button>
                        ))}
                      </div>
                    </div>

                    {mcpForm.transportType === "stdio" ? (
                      <div className="space-y-3 border-y border-stone-200 py-4">
                        <label className="block space-y-1">
                          <span className="text-[11px] font-semibold text-stone-600">Command</span>
                          <input
                            value={mcpForm.command}
                            onChange={(event) => setMcpForm((form) => ({ ...form, command: event.target.value }))}
                            placeholder="npx、uvx 或可执行文件绝对路径"
                            className="w-full rounded-md border border-stone-200 bg-white px-3 py-2 font-mono text-xs outline-none focus:ring-1 focus:ring-indigo-300"
                          />
                        </label>
                        <label className="block space-y-1">
                          <span className="text-[11px] font-semibold text-stone-600">Arguments</span>
                          <textarea
                            value={mcpForm.argsText}
                            onChange={(event) => setMcpForm((form) => ({ ...form, argsText: event.target.value }))}
                            placeholder={"每行一个参数，例如：\n-y\n@modelcontextprotocol/server-filesystem\n/home/user/Documents"}
                            rows={4}
                            className="w-full resize-y rounded-md border border-stone-200 bg-white px-3 py-2 font-mono text-[11px] leading-relaxed outline-none focus:ring-1 focus:ring-indigo-300"
                          />
                        </label>
                        <div className="space-y-2">
                          <div className="flex items-center justify-between">
                            <span className="text-[11px] font-semibold text-stone-600">Secret 环境变量</span>
                            <button
                              type="button"
                              onClick={() => setMcpForm((form) => ({
                                ...form,
                                env: [...form.env, { name: "", value: "", hasValue: false }],
                              }))}
                              className="flex items-center gap-1 rounded-md px-2 py-1 text-[10px] font-semibold text-indigo-600 hover:bg-indigo-50"
                            >
                              <Plus className="h-3 w-3" />
                              添加变量
                            </button>
                          </div>
                          {mcpForm.env.length === 0 ? (
                            <p className="text-[10px] text-stone-400">未配置额外环境变量。子进程仅继承 PATH、HOME、语言和临时目录。</p>
                          ) : (
                            <div className="space-y-1.5">
                              {mcpForm.env.map((item, index) => (
                                <div key={`${index}-${item.name}`} className="grid grid-cols-[180px_1fr_32px] gap-2">
                                  <input
                                    value={item.name}
                                    onChange={(event) => setMcpForm((form) => ({
                                      ...form,
                                      env: form.env.map((entry, entryIndex) =>
                                        entryIndex === index ? { ...entry, name: event.target.value } : entry
                                      ),
                                    }))}
                                    placeholder="API_TOKEN"
                                    className="rounded-md border border-stone-200 px-2.5 py-2 font-mono text-[11px] outline-none focus:ring-1 focus:ring-indigo-300"
                                  />
                                  <input
                                    type="password"
                                    value={item.value}
                                    onChange={(event) => setMcpForm((form) => ({
                                      ...form,
                                      env: form.env.map((entry, entryIndex) =>
                                        entryIndex === index ? { ...entry, value: event.target.value } : entry
                                      ),
                                    }))}
                                    placeholder={item.hasValue ? "已保存在密钥环，留空则不修改" : "输入变量值"}
                                    className="rounded-md border border-stone-200 px-2.5 py-2 text-[11px] outline-none focus:ring-1 focus:ring-indigo-300"
                                  />
                                  <button
                                    type="button"
                                    title="移除环境变量"
                                    onClick={() => setMcpForm((form) => ({
                                      ...form,
                                      env: form.env.filter((_, entryIndex) => entryIndex !== index),
                                    }))}
                                    className="flex h-8 w-8 items-center justify-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-600"
                                  >
                                    <Trash2 className="h-3.5 w-3.5" />
                                  </button>
                                </div>
                              ))}
                            </div>
                          )}
                        </div>
                      </div>
                    ) : (
                      <div className="space-y-3 border-y border-stone-200 py-4">
                        <label className="block space-y-1">
                          <span className="text-[11px] font-semibold text-stone-600">Server URL</span>
                          <input
                            value={mcpForm.url}
                            onChange={(event) => setMcpForm((form) => ({ ...form, url: event.target.value }))}
                            placeholder="https://example.com/mcp"
                            className="w-full rounded-md border border-stone-200 bg-white px-3 py-2 font-mono text-xs outline-none focus:ring-1 focus:ring-indigo-300"
                          />
                        </label>
                        <label className="block space-y-1">
                          <span className="flex items-center justify-between text-[11px] font-semibold text-stone-600">
                            Bearer Token
                            {mcpForm.hasBearerToken && !mcpForm.clearBearerToken && (
                              <button
                                type="button"
                                onClick={() => setMcpForm((form) => ({ ...form, clearBearerToken: true, bearerToken: "" }))}
                                className="text-[10px] font-medium text-rose-600 hover:underline"
                              >
                                清除已保存 Token
                              </button>
                            )}
                          </span>
                          <input
                            type="password"
                            value={mcpForm.bearerToken}
                            onChange={(event) => setMcpForm((form) => ({
                              ...form,
                              bearerToken: event.target.value,
                              clearBearerToken: false,
                            }))}
                            placeholder={
                              mcpForm.clearBearerToken
                                ? "保存后清除 Token"
                                : mcpForm.hasBearerToken
                                  ? "已保存在密钥环，留空则不修改"
                                  : "可选"
                            }
                            className="w-full rounded-md border border-stone-200 bg-white px-3 py-2 text-xs outline-none focus:ring-1 focus:ring-indigo-300"
                          />
                        </label>
                      </div>
                    )}

                    <div className="flex justify-end gap-2 border-t border-stone-200 pt-3">
                      <button
                        type="button"
                        onClick={() => {
                          setEditingMcpId(null);
                          setMcpForm(EMPTY_MCP_FORM);
                        }}
                        className="rounded-md px-3 py-1.5 text-xs font-semibold text-stone-500 hover:bg-stone-100"
                      >
                        取消
                      </button>
                      <button
                        type="button"
                        onClick={saveMcpServer}
                        disabled={isSavingMcp || !mcpForm.name.trim()}
                        className="flex items-center gap-1.5 rounded-md bg-indigo-600 px-4 py-1.5 text-xs font-semibold text-white hover:bg-indigo-700 disabled:opacity-40"
                      >
                        <Check className="h-3.5 w-3.5" />
                        {isSavingMcp ? "保存中..." : "保存"}
                      </button>
                    </div>
                  </div>
                ) : mcpServers.length === 0 ? (
                  <div className="py-16 text-center text-xs text-stone-400">尚未配置 MCP Server</div>
                ) : (
                  <div className="divide-y divide-stone-200 border-y border-stone-200">
                    {mcpServers.map((server) => (
                      <div key={server.id} className="flex min-h-16 items-center gap-3 px-2 py-3">
                        <div className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-md border ${
                          server.enabled
                            ? "border-indigo-200 bg-indigo-50 text-indigo-700"
                            : "border-stone-200 bg-stone-100 text-stone-400"
                        }`}>
                          <Server className="h-4 w-4" />
                        </div>
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <span className="truncate text-xs font-semibold text-stone-800">{server.name}</span>
                            <span className="rounded border border-stone-200 bg-stone-50 px-1.5 py-0.5 font-mono text-[9px] text-stone-500">
                              {server.transport.type === "stdio" ? "stdio" : "HTTP"}
                            </span>
                          </div>
                          <p className="mt-0.5 truncate font-mono text-[9px] text-stone-400">
                            {server.transport.type === "stdio"
                              ? [server.transport.command, ...server.transport.args].join(" ")
                              : server.transport.url}
                          </p>
                        </div>
                        <button
                          type="button"
                          onClick={() => void toggleMcpServer(server)}
                          title={server.enabled ? "停用 Server" : "启用 Server"}
                          className={`rounded-md border px-2 py-1 text-[10px] font-semibold ${
                            server.enabled
                              ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                              : "border-stone-200 bg-stone-100 text-stone-500"
                          }`}
                        >
                          {server.enabled ? "已启用" : "已停用"}
                        </button>
                        <button
                          type="button"
                          onClick={() => void testMcpServer(server)}
                          disabled={testingMcpId === server.id}
                          title="测试连接并读取工具列表"
                          className="flex h-8 w-8 items-center justify-center rounded-md text-stone-400 hover:bg-indigo-50 hover:text-indigo-700 disabled:opacity-40"
                        >
                          <Zap className={`h-3.5 w-3.5 ${testingMcpId === server.id ? "animate-pulse" : ""}`} />
                        </button>
                        <button
                          type="button"
                          onClick={() => openEditMcpServer(server)}
                          title="编辑 Server"
                          className="flex h-8 w-8 items-center justify-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-800"
                        >
                          <Pencil className="h-3.5 w-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={() => void deleteMcpServer(server)}
                          title="删除 Server"
                          className="flex h-8 w-8 items-center justify-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-700"
                        >
                          <Trash2 className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}

            {activeTab === "tokens" && (
              <div className="max-w-2xl space-y-5">
                <div className="flex items-start justify-between gap-4 border-b border-stone-200 pb-3">
                  <div>
                    <h3 className="text-sm font-semibold text-stone-850">Token 统计</h3>
                    <p className="text-[11px] text-stone-400">统计本机已完成 AI 回复的模型用量。</p>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <div className="flex rounded-md border border-stone-200 bg-stone-50 p-0.5">
                      {(["all", "agent"] as const).map((scope) => (
                        <button
                          key={scope}
                          type="button"
                          onClick={() => setTokenUsageScope(scope)}
                          className={`rounded px-2.5 py-1 text-[10px] font-semibold ${
                            tokenUsageScope === scope
                              ? "bg-white text-stone-800 shadow-sm"
                              : "text-stone-400 hover:text-stone-700"
                          }`}
                        >
                          {scope === "all" ? "全部" : "当前智能体"}
                        </button>
                      ))}
                    </div>
                    <button
                      type="button"
                      onClick={() => void loadTokenUsageStats()}
                      disabled={tokenUsageLoading}
                      title="刷新统计"
                      className="flex h-8 w-8 items-center justify-center rounded-md border border-stone-200 text-stone-500 hover:bg-stone-50 disabled:opacity-40"
                    >
                      <RefreshCw className={`h-3.5 w-3.5 ${tokenUsageLoading ? "animate-spin" : ""}`} />
                    </button>
                  </div>
                </div>

                {tokenUsageError && (
                  <div className="border-y border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700">
                    {tokenUsageError}
                  </div>
                )}

                <dl className="divide-y divide-stone-200 border-y border-stone-200">
                  {[
                    ["输入 Token", tokenUsageStats?.input_tokens ?? 0],
                    ["缓存命中 Token", tokenUsageStats?.cached_tokens ?? 0],
                    ["输出 Token", tokenUsageStats?.output_tokens ?? 0],
                    ["总 Token", tokenUsageStats?.total_tokens ?? 0],
                  ].map(([label, value]) => (
                    <div key={String(label)} className="flex min-h-12 items-center justify-between gap-4 px-2">
                      <dt className="text-xs text-stone-500">{label}</dt>
                      <dd className="font-mono text-sm font-semibold tabular-nums text-stone-800">
                        {Number(value).toLocaleString()}
                      </dd>
                    </div>
                  ))}
                </dl>

                <div className="flex items-center justify-between gap-4 text-xs">
                  <span className="text-stone-500">输入缓存命中率</span>
                  <span className="font-mono font-semibold tabular-nums text-stone-800">
                    {tokenUsageStats && tokenUsageStats.input_tokens > 0
                      ? `${((tokenUsageStats.cached_tokens / tokenUsageStats.input_tokens) * 100).toFixed(1)}%`
                      : "0.0%"}
                  </span>
                </div>
              </div>
            )}

            {/* 5. AUDIT TAB */}
            {activeTab === "audit" && (
              <div className="space-y-4">
                <div className="flex justify-between items-center border-b border-stone-200 pb-3">
                  <div>
                    <h3 className="text-sm font-semibold text-stone-850">本地操作审计流水 (SQLite)</h3>
                    <p className="text-[11px] text-stone-400">已由 Safe Tool Executor 执行并归档的命令行/文件读写历史日志。</p>
                  </div>
                </div>

                <div className="space-y-2 max-h-[380px] overflow-y-auto">
                  {auditLogs.map((log) => (
                    <div
                      key={log.id}
                      className="border border-stone-200 bg-white hover:bg-stone-50/50 shadow-sm p-3.5 rounded-xl flex items-center justify-between text-xs font-mono"
                    >
                      <div className="space-y-1 overflow-hidden mr-4">
                        <div className="flex items-center gap-2">
                          <span className="text-stone-400">{log.time}</span>
                          <span className="text-stone-800 font-semibold">{log.tool}</span>
                        </div>
                        <p className="text-stone-500 truncate text-[11px]">{log.params}</p>
                      </div>

                      <div className="flex items-center gap-3 shrink-0">
                        <span
                          className={`px-2 py-0.5 rounded text-[10px] ${
                            log.status === "succeeded" || log.status === "done"
                              ? "bg-emerald-50 text-emerald-600 border border-emerald-100/60 font-medium"
                              : log.status === "rejected"
                              ? "bg-stone-100 text-stone-500 border border-stone-200/50"
                              : "bg-rose-50 text-rose-600 border border-rose-100"
                          }`}
                        >
                          {log.status === "succeeded" || log.status === "done" ? "Succeeded" : log.status}
                        </span>
                        <span className="text-[10px] text-stone-400 bg-stone-100 px-1.5 py-0.5 rounded border border-stone-200/60">
                          风险: {log.risk}
                        </span>
                      </div>
                    </div>
                  ))}
                  {auditLogs.length === 0 && (
                    <div className="text-center py-10 text-xs text-stone-400 font-mono">
                      当前会话暂无工具执行审计流水记录
                    </div>
                  )}
                </div>
              </div>
            )}

            {/* 5. DEBUG TAB */}
            {activeTab === "debug" && (
              <div className="space-y-4">
                <div className="flex justify-between items-center border-b border-stone-200 pb-3">
                  <div>
                    <h3 className="text-sm font-semibold text-stone-850">提示词调试面板</h3>
                    <p className="text-[11px] text-stone-400">
                      显示当前智能体（{activeAgent?.name}）发送给 AI 的 System Prompt、工具定义和消息。
                      {activeSessionId ? "已包含当前会话历史。" : "未选择会话，不包含会话历史。"}
                    </p>
                  </div>
                  <button
                    onClick={loadDebugPrompt}
                    disabled={debugLoading || !activeAgentId}
                    className="flex items-center gap-1.5 bg-[#8CA38A] text-white hover:bg-[#7A917A] rounded-lg px-3 py-1.5 text-xs font-semibold transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    <Terminal className="h-3.5 w-3.5" />
                    {debugLoading ? "拼装中..." : "生成提示词预览"}
                  </button>
                </div>

                {debugError && (
                  <div className="text-xs text-rose-600 bg-rose-50 border border-rose-200/60 rounded-lg px-3 py-2">
                    {debugError}
                  </div>
                )}

                {!debugPrompt && !debugError && !debugLoading && (
                  <div className="text-center py-10 text-xs text-stone-400">
                    点击右上角「生成提示词预览」以查看当前智能体将要发送给 AI 的完整请求上下文。
                  </div>
                )}

                {debugPrompt && (
                  <div className="space-y-4">
                    <div>
                      <div className="flex items-center justify-between mb-1.5">
                        <span className="text-[11px] font-bold text-stone-500 uppercase tracking-wide">
                          System Prompt
                        </span>
                        <span className="text-[10px] text-stone-400">
                          {debugPrompt.system_prompt.length} 字符
                        </span>
                      </div>
                      <pre className="bg-stone-900 text-stone-100 text-[11px] p-3 rounded-lg border border-stone-800 overflow-x-auto whitespace-pre-wrap max-h-72 overflow-y-auto font-mono leading-relaxed">
                        {debugPrompt.system_prompt}
                      </pre>
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1.5">
                        <span className="text-[11px] font-bold text-stone-500 uppercase tracking-wide">
                          Tools ({debugPrompt.tools.length})
                        </span>
                        <span className="text-[10px] text-stone-400">模型请求的 tools 参数</span>
                      </div>
                      <pre className="bg-stone-900 text-stone-100 text-[11px] p-3 rounded-lg border border-stone-800 overflow-x-auto whitespace-pre-wrap max-h-80 overflow-y-auto font-mono leading-relaxed">
                        {JSON.stringify(debugPrompt.tools, null, 2)}
                      </pre>
                    </div>

                    <div>
                      <div className="flex items-center justify-between mb-1.5">
                        <span className="text-[11px] font-bold text-stone-500 uppercase tracking-wide">
                          Messages ({debugPrompt.messages.length}
                          {debugPrompt.discarded_count > 0
                            ? ` · 已裁剪 ${debugPrompt.discarded_count} 条`
                            : ""}
                          )
                        </span>
                        <span className="text-[10px] text-stone-400">角色交替的历史消息</span>
                      </div>
                      <div className="space-y-2 max-h-[360px] overflow-y-auto">
                        {debugPrompt.messages.map((m, i) => {
                          const role = (m.role as string) || "unknown";
                          const isUser = role === "user";
                          const isSystem = role === "system";
                          const content =
                            typeof m.content === "string"
                              ? m.content
                              : JSON.stringify(m.content, null, 2);
                          return (
                            <div
                              key={i}
                              className={`rounded-xl border p-3 text-xs ${
                                isUser
                                  ? "bg-[#F1F5F0]/70 border-[#DFE7DD]"
                                  : isSystem
                                  ? "bg-stone-50 border-stone-200"
                                  : "bg-white border-stone-200 shadow-sm"
                              }`}
                            >
                              <div className="flex items-center gap-2 mb-1.5">
                                <span
                                  className={`px-1.5 py-0.5 rounded text-[10px] font-semibold border ${
                                    isUser
                                      ? "bg-emerald-50 text-emerald-700 border-emerald-200/60"
                                      : isSystem
                                      ? "bg-stone-100 text-stone-500 border-stone-200/50"
                                      : "bg-indigo-50 text-indigo-600 border-indigo-100"
                                  }`}
                                >
                                  {role}
                                </span>
                                {Array.isArray(m.tool_calls) && m.tool_calls.length > 0 && (
                                  <span className="text-[10px] text-amber-600">
                                    含 {m.tool_calls.length} 个工具调用
                                  </span>
                                )}
                              </div>
                              <pre className="text-stone-700 whitespace-pre-wrap font-mono text-[11px] leading-relaxed max-h-48 overflow-y-auto">
                                {content}
                              </pre>
                            </div>
                          );
                        })}
                      </div>
                    </div>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
      
      {/* Model Selection Modal */}
      {isModelSelectOpen && (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/30 backdrop-blur-[2px]">
          <div className="w-[480px] max-h-[500px] bg-white border border-stone-200 rounded-2xl shadow-2xl flex flex-col overflow-hidden animate-in fade-in zoom-in-95 duration-200">
            <div className="px-5 py-3.5 border-b border-stone-200 bg-stone-50 flex justify-between items-center shrink-0">
              <h4 className="text-sm font-semibold text-stone-800">选择要导入的模型</h4>
              <button
                onClick={() => setIsModelSelectOpen(false)}
                className="text-stone-400 hover:text-stone-800 rounded p-1 hover:bg-stone-200/50 transition-colors"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            
            <div className="flex-1 overflow-y-auto p-2 bg-[#FAF9F5]/30">
              <div className="space-y-1">
                {availableModels.map(model => (
                  <label
                    key={model.id}
                    className="flex items-center gap-3 p-2.5 rounded-lg hover:bg-white border border-transparent hover:border-stone-200 hover:shadow-sm cursor-pointer transition-all group"
                  >
                    <input
                      type="checkbox"
                      checked={selectedModels.has(model.id)}
                      onChange={() => handleToggleModel(model.id)}
                      className="rounded border-stone-300 text-[#8CA38A] focus:ring-[#8CA38A]/40 h-4 w-4 shrink-0 transition-colors"
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block text-xs font-mono text-stone-700 group-hover:text-stone-900 break-all">
                        {model.id}
                      </span>
                      <span className="mt-1 flex flex-wrap gap-1">
                        {capabilityLabels(model.capabilities).map((label) => (
                          <span key={label} className="rounded border border-stone-200 bg-white px-1.5 py-0.5 text-[8px] text-stone-500">
                            {label}
                          </span>
                        ))}
                      </span>
                    </span>
                  </label>
                ))}
              </div>
            </div>
            
            <div className="px-5 py-3.5 border-t border-stone-200 bg-stone-50 flex justify-between items-center shrink-0">
              <div className="text-xs text-stone-500 font-medium">
                已选中 <span className="text-[#8CA38A] font-bold">{selectedModels.size}</span> 个模型
              </div>
              <div className="flex gap-2">
                <button
                  onClick={() => setIsModelSelectOpen(false)}
                  className="px-4 py-1.5 rounded-lg text-xs font-medium text-stone-500 hover:text-stone-700 hover:bg-stone-200/50 transition-colors"
                >
                  取消
                </button>
                <button
                  onClick={handleConfirmModels}
                  className="bg-[#8CA38A] text-white hover:bg-[#7A917A] rounded-lg px-5 py-1.5 text-xs font-semibold transition-colors flex items-center gap-1.5"
                >
                  <Check className="h-3.5 w-3.5" />
                  确认导入
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* 头像圆形裁剪浮层 */}
      {cropSrc && (
        <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/40 backdrop-blur-[2px]">
          <div className="w-[360px] bg-white border border-stone-200 rounded-2xl shadow-2xl p-5 flex flex-col items-center gap-4">
            <h4 className="text-sm font-semibold text-stone-800 self-start">裁剪圆形头像</h4>
            <canvas
              ref={cropCanvasRef}
              width={CROP_SIZE}
              height={CROP_SIZE}
              onPointerDown={onCropPointerDown}
              onPointerMove={onCropPointerMove}
              onPointerUp={onCropPointerUp}
              onPointerLeave={onCropPointerUp}
              className="rounded-full cursor-move touch-none border border-stone-200 bg-stone-100"
              style={{ width: CROP_SIZE, height: CROP_SIZE }}
            />
            <input
              type="range"
              min={1}
              max={3}
              step={0.01}
              value={cropScale}
              onChange={(e) => setCropScale(parseFloat(e.target.value))}
              className="w-full accent-indigo-600"
            />
            <p className="text-[11px] text-stone-400 self-start">拖动画面调整位置，拖动滑块缩放。</p>
            <div className="flex justify-end gap-2 w-full">
              <button
                onClick={() => setCropSrc(null)}
                className="px-4 py-1.5 rounded-lg text-xs font-semibold text-stone-500 hover:bg-stone-100"
              >
                取消
              </button>
              <button
                onClick={confirmCrop}
                className="flex items-center gap-1.5 px-4 py-1.5 rounded-lg bg-indigo-600 text-white text-xs font-semibold hover:bg-indigo-700"
              >
                <Check className="h-3.5 w-3.5" />
                确认裁剪
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

/// 通用设置标签页：会话打开模式等应用级行为配置。
const GeneralTab: React.FC = () => {
  const [openMode, setOpenMode] = useState<string>("last");
  const [translationLanguage, setTranslationLanguage] = useState<"中文" | "English">("中文");
  const [colorScheme, setColorScheme] = useState<ColorScheme>(getCachedColorScheme);
  const [autoExpandThoughts, setAutoExpandThoughtsState] = useState(getCachedAutoExpandThoughts);
  const [autoFollowStreaming, setAutoFollowStreamingState] = useState(getCachedAutoFollowStreaming);
  const [defaultMaxOutputTokens, setDefaultMaxOutputTokens] = useState(String(DEFAULT_MAX_OUTPUT_TOKENS));
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    Promise.all([
      invoke<string | null>("get_setting", { key: "ui:session_open_mode" }),
      invoke<string | null>("get_setting", { key: "ui:translation_target_language" }),
      invoke<string | null>("get_setting", { key: UI_COLOR_SCHEME_KEY }),
      invoke<string | null>("get_setting", { key: UI_AUTO_EXPAND_THOUGHTS_KEY }),
      invoke<string | null>("get_setting", { key: UI_AUTO_FOLLOW_STREAMING_KEY }),
      invoke<string | null>("get_setting", { key: UI_DEFAULT_MAX_OUTPUT_TOKENS_KEY }),
    ])
      .then(([openModeValue, languageValue, colorSchemeValue, autoExpandThoughtsValue, autoFollowStreamingValue, maxOutputTokensValue]) => {
        const nextColorScheme = normalizeColorScheme(colorSchemeValue);
        const nextAutoExpandThoughts = normalizeBooleanPreference(autoExpandThoughtsValue, true);
        const nextAutoFollowStreaming = normalizeBooleanPreference(autoFollowStreamingValue, true);
        setOpenMode(openModeValue ?? "last");
        if (languageValue === "中文" || languageValue === "English") setTranslationLanguage(languageValue);
        setColorScheme(nextColorScheme);
        setAutoExpandThoughtsState(nextAutoExpandThoughts);
        setAutoFollowStreamingState(nextAutoFollowStreaming);
        setDefaultMaxOutputTokens(String(normalizeMaxOutputTokens(maxOutputTokensValue)));
        applyColorScheme(nextColorScheme);
        setAutoExpandThoughts(nextAutoExpandThoughts);
        setAutoFollowStreaming(nextAutoFollowStreaming);
        announceUIPreferenceChange({
          colorScheme: nextColorScheme,
          autoExpandThoughts: nextAutoExpandThoughts,
          autoFollowStreaming: nextAutoFollowStreaming,
        });
      })
      .catch(console.error)
      .finally(() => setLoaded(true));
  }, []);

  const updateMode = async (mode: string) => {
    setOpenMode(mode);
    try {
      await invoke("set_setting", { key: "ui:session_open_mode", value: mode });
    } catch (e) {
      console.error("保存打开模式失败", e);
    }
  };

  const options = [
    { value: "last", label: "回到上次对话", desc: "打开时恢复上次选中的智能体与会话" },
    { value: "new", label: "自动新建会话", desc: "打开时为上次选中的智能体自动创建新会话" },
  ];

  const updateTranslationLanguage = async (language: "中文" | "English") => {
    setTranslationLanguage(language);
    try {
      await invoke("set_setting", { key: "ui:translation_target_language", value: language });
    } catch (e) {
      console.error("保存翻译目标语言失败", e);
    }
  };

  const updateColorScheme = async (scheme: ColorScheme) => {
    const previous = colorScheme;
    setColorScheme(scheme);
    applyColorScheme(scheme);
    announceUIPreferenceChange({ colorScheme: scheme });
    try {
      await invoke("set_setting", { key: UI_COLOR_SCHEME_KEY, value: scheme });
    } catch (e) {
      setColorScheme(previous);
      applyColorScheme(previous);
      announceUIPreferenceChange({ colorScheme: previous });
      console.error("保存界面主题失败", e);
    }
  };

  const updateAutoExpandThoughts = async (value: boolean) => {
    const previous = autoExpandThoughts;
    setAutoExpandThoughtsState(value);
    setAutoExpandThoughts(value);
    announceUIPreferenceChange({ autoExpandThoughts: value });
    try {
      await invoke("set_setting", { key: UI_AUTO_EXPAND_THOUGHTS_KEY, value: String(value) });
    } catch (e) {
      setAutoExpandThoughtsState(previous);
      setAutoExpandThoughts(previous);
      announceUIPreferenceChange({ autoExpandThoughts: previous });
      console.error("保存思考过程展开设置失败", e);
    }
  };

  const updateAutoFollowStreaming = async (value: boolean) => {
    const previous = autoFollowStreaming;
    setAutoFollowStreamingState(value);
    setAutoFollowStreaming(value);
    announceUIPreferenceChange({ autoFollowStreaming: value });
    try {
      await invoke("set_setting", { key: UI_AUTO_FOLLOW_STREAMING_KEY, value: String(value) });
    } catch (e) {
      setAutoFollowStreamingState(previous);
      setAutoFollowStreaming(previous);
      announceUIPreferenceChange({ autoFollowStreaming: previous });
      console.error("保存流式回复跟随设置失败", e);
    }
  };

  const updateDefaultMaxOutputTokens = async () => {
    const previous = defaultMaxOutputTokens;
    const next = normalizeMaxOutputTokens(defaultMaxOutputTokens);
    setDefaultMaxOutputTokens(String(next));
    try {
      await invoke("set_setting", {
        key: UI_DEFAULT_MAX_OUTPUT_TOKENS_KEY,
        value: String(next),
      });
    } catch (e) {
      setDefaultMaxOutputTokens(previous);
      console.error("保存默认最大输出 Token 失败", e);
    }
  };

  return (
    <div className="space-y-6 max-w-xl">
      <div>
        <h2 className="text-sm font-semibold text-stone-800 mb-1">通用设置</h2>
        <p className="text-[11px] text-stone-400">应用启动行为与界面偏好</p>
      </div>

      <div>
        <label className="mb-2 block font-semibold text-stone-500">外观</label>
        <div className="grid grid-cols-2 gap-1 rounded-lg border border-stone-200 bg-stone-100 p-1">
          {([
            { value: "light" as const, label: "浅色", icon: Sun },
            { value: "dark" as const, label: "深色", icon: Moon },
          ]).map((option) => {
            const Icon = option.icon;
            const active = colorScheme === option.value;
            return (
              <button
                key={option.value}
                type="button"
                disabled={!loaded}
                onClick={() => void updateColorScheme(option.value)}
                className={`flex items-center justify-center gap-2 rounded-md px-3 py-2 text-xs font-semibold transition-colors disabled:opacity-50 ${
                  active
                    ? "border border-stone-200 bg-white text-stone-800 shadow-sm"
                    : "border border-transparent text-stone-500 hover:text-stone-800"
                }`}
                aria-pressed={active}
              >
                <Icon className="h-3.5 w-3.5" />
                {option.label}
              </button>
            );
          })}
        </div>
      </div>

      <div>
        <label className="mb-2 block font-semibold text-stone-500">对话</label>
        <div className="space-y-2">
          <div className="flex items-center gap-3 rounded-lg border border-stone-200 bg-stone-50 px-3.5 py-3">
            <Brain className="h-4 w-4 shrink-0 text-stone-500" />
            <div className="min-w-0 flex-1">
              <p className="text-xs font-semibold text-stone-700">自动展开思考过程</p>
              <p className="mt-0.5 text-[10px] text-stone-400">新显示的思考内容默认保持展开，仍可手动收起。</p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={autoExpandThoughts}
              disabled={!loaded}
              onClick={() => void updateAutoExpandThoughts(!autoExpandThoughts)}
              className={`relative h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-50 ${
                autoExpandThoughts ? "bg-[#8CA38A]" : "bg-stone-300"
              }`}
            >
              <span
                className={`agnes-toggle-thumb absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white shadow-sm transition-transform ${
                  autoExpandThoughts ? "agnes-toggle-thumb--on" : "agnes-toggle-thumb--off"
                }`}
              />
            </button>
          </div>
          <div className="flex items-center gap-3 rounded-lg border border-stone-200 bg-stone-50 px-3.5 py-3">
            <ArrowDown className="h-4 w-4 shrink-0 text-stone-500" />
            <div className="min-w-0 flex-1">
              <p className="text-xs font-semibold text-stone-700">自动跟随 AI 回复</p>
              <p className="mt-0.5 text-[10px] text-stone-400">流式回复时持续滚动到底部；关闭后保留当前阅读位置。</p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={autoFollowStreaming}
              disabled={!loaded}
              onClick={() => void updateAutoFollowStreaming(!autoFollowStreaming)}
              className={`relative h-5 w-9 shrink-0 rounded-full transition-colors disabled:opacity-50 ${
                autoFollowStreaming ? "bg-[#8CA38A]" : "bg-stone-300"
              }`}
            >
              <span
                className={`agnes-toggle-thumb absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white shadow-sm transition-transform ${
                  autoFollowStreaming ? "agnes-toggle-thumb--on" : "agnes-toggle-thumb--off"
                }`}
              />
            </button>
          </div>
          <div className="flex items-center gap-3 rounded-lg border border-stone-200 bg-stone-50 px-3.5 py-3">
            <Gauge className="h-4 w-4 shrink-0 text-stone-500" />
            <div className="min-w-0 flex-1">
              <label htmlFor="default-max-output-tokens" className="text-xs font-semibold text-stone-700">
                默认最大输出 Token
              </label>
              <p className="mt-0.5 text-[10px] text-stone-400">新会话默认使用；每个会话仍可在模型菜单中单独覆盖。</p>
            </div>
            <input
              id="default-max-output-tokens"
              type="number"
              min={MIN_MAX_OUTPUT_TOKENS}
              max={MAX_MAX_OUTPUT_TOKENS}
              step={1024}
              value={defaultMaxOutputTokens}
              disabled={!loaded}
              onChange={(event) => setDefaultMaxOutputTokens(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") event.currentTarget.blur();
              }}
              onBlur={() => void updateDefaultMaxOutputTokens()}
              className="h-8 w-28 rounded-md border border-stone-200 bg-white px-2 text-right font-mono text-[11px] tabular-nums text-stone-700 outline-none focus:border-[#8CA38A] disabled:opacity-50"
            />
          </div>
        </div>
      </div>

      <div>
        <label className="font-semibold text-stone-500 block mb-2">打开时</label>
        <div className="space-y-2">
          {options.map((opt) => (
            <button
              key={opt.value}
              type="button"
              disabled={!loaded}
              onClick={() => updateMode(opt.value)}
              className={`w-full text-left px-3.5 py-2.5 rounded-xl border transition-colors ${
                openMode === opt.value
                  ? "bg-[#8CA38A]/10 border-[#8CA38A] text-stone-800"
                  : "bg-stone-50 border-stone-200 text-stone-600 hover:bg-stone-100"
              }`}
            >
              <div className="flex items-center gap-2">
                <span className={`w-3 h-3 rounded-full border-2 ${openMode === opt.value ? "border-[#8CA38A] bg-[#8CA38A]" : "border-stone-300"}`} />
                <span className="text-xs font-semibold">{opt.label}</span>
              </div>
              <p className="text-[10px] text-stone-400 mt-1 ml-5">{opt.desc}</p>
            </button>
          ))}
        </div>
        <p className="mt-2 text-[10px] text-stone-400">下次启动应用时生效。</p>
      </div>

      <div>
        <label htmlFor="translation-target-language" className="font-semibold text-stone-500 block mb-2">语言</label>
        <select
          id="translation-target-language"
          value={translationLanguage}
          disabled={!loaded}
          onChange={(event) => void updateTranslationLanguage(event.target.value as "中文" | "English")}
          className="w-full rounded-lg border border-stone-200 bg-white px-3 py-2.5 text-xs text-stone-700 outline-none focus:border-[#8CA38A] disabled:opacity-50"
        >
          <option value="中文">中文</option>
          <option value="English">English</option>
        </select>
      </div>
    </div>
  );
};

const ARTIFACT_QUOTA_OPTIONS = [0.25, 0.5, 1, 2, 5, 10, 20, 50, 100];

function formatLocalStorageBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unitIndex;
  return `${value >= 10 || unitIndex === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unitIndex]}`;
}

const ArtifactStorageTab: React.FC = () => {
  const [status, setStatus] = useState<ArtifactStorageStatus | null>(null);
  const [quotaGiB, setQuotaGiB] = useState("2");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await invoke<ArtifactStorageStatus>("get_artifact_storage_status");
      setStatus(next);
      setQuotaGiB(String(next.quotaBytes / 1024 ** 3));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void loadStatus();
  }, []);

  const saveQuota = async () => {
    const quotaBytes = Math.round(Number(quotaGiB) * 1024 ** 3);
    if (!Number.isSafeInteger(quotaBytes)) return;
    setSaving(true);
    setError(null);
    setMessage(null);
    try {
      const result = await invoke<ArtifactGcResult>("set_artifact_storage_quota", {
        quotaBytes,
      });
      setStatus(result.status);
      setMessage(
        result.reclaimedBytes > 0
          ? `配额已保存，并自动释放 ${formatLocalStorageBytes(result.reclaimedBytes)}。`
          : "配额已保存。",
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  };

  const cleanup = async () => {
    if (!window.confirm("清理已有远端副本的本地制品缓存和过期临时文件？当前安装目录与唯一副本不会删除。")) {
      return;
    }
    setCleaning(true);
    setError(null);
    setMessage(null);
    try {
      const result = await invoke<ArtifactGcResult>("cleanup_artifact_storage");
      setStatus(result.status);
      setMessage(
        `已释放 ${formatLocalStorageBytes(result.reclaimedBytes)}，清理 ${result.removedPaths} 个路径` +
          (result.failedPaths > 0 ? `，${result.failedPaths} 个路径清理失败。` : "。"),
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setCleaning(false);
    }
  };

  const usagePercent = status && status.quotaBytes > 0
    ? Math.min(100, (status.usedBytes / status.quotaBytes) * 100)
    : 0;

  return (
    <div className="max-w-2xl space-y-6">
      <div className="flex items-start justify-between gap-4 border-b border-stone-200 pb-3">
        <div>
          <h3 className="text-sm font-semibold text-stone-850">本地制品存储</h3>
          <p className="mt-1 text-[11px] text-stone-400">
            管理知识库加密上传缓存、跨设备安装制品和中断后留下的临时文件。
          </p>
        </div>
        <button
          type="button"
          onClick={() => void loadStatus()}
          disabled={loading}
          title="刷新容量统计"
          className="flex h-8 w-8 items-center justify-center rounded-md border border-stone-200 text-stone-500 hover:bg-stone-50 disabled:opacity-40"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>

      {error && (
        <div className="border-y border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700">
          {error}
        </div>
      )}
      {message && (
        <div className="border-y border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700">
          {message}
        </div>
      )}

      <div className="rounded-xl border border-stone-200 bg-stone-50 p-4">
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-xs font-semibold text-stone-700">本地占用</p>
            <p className="mt-1 font-mono text-lg font-semibold tabular-nums text-stone-850">
              {status ? formatLocalStorageBytes(status.usedBytes) : "—"}
              <span className="ml-1 text-xs font-normal text-stone-400">
                / {status ? formatLocalStorageBytes(status.quotaBytes) : "—"}
              </span>
            </p>
          </div>
          {status?.overQuota && (
            <span className="rounded-full bg-rose-50 px-2.5 py-1 text-[10px] font-semibold text-rose-700">
              已超出配额
            </span>
          )}
        </div>
        <div className="mt-3 h-2 overflow-hidden rounded-full bg-stone-200">
          <div
            className={`h-full rounded-full transition-all ${status?.overQuota ? "bg-rose-500" : "bg-[#8CA38A]"}`}
            style={{ width: `${usagePercent}%` }}
          />
        </div>
        <dl className="mt-4 grid grid-cols-2 gap-x-6 gap-y-3 text-[11px]">
          {[
            ["加密上传缓存", status?.outboxBytes ?? 0],
            ["本地安装制品", status?.installedBytes ?? 0],
            ["临时文件", status?.temporaryBytes ?? 0],
            ["可安全释放", status?.reclaimableBytes ?? 0],
          ].map(([label, value]) => (
            <div key={String(label)} className="flex items-center justify-between gap-3 border-b border-stone-200 pb-2">
              <dt className="text-stone-500">{label}</dt>
              <dd className="font-mono font-semibold tabular-nums text-stone-700">
                {formatLocalStorageBytes(Number(value))}
              </dd>
            </div>
          ))}
        </dl>
        <p className="mt-3 text-[10px] text-stone-400">
          当前记录 {status?.localArtifactCount ?? 0} 个本地制品。自动维护每 6 小时运行一次，只清理已有 ready 远端副本的缓存；当前文档安装目录和没有远端副本的制品始终保留。
        </p>
      </div>

      <div className="space-y-3">
        <label htmlFor="artifact-storage-quota" className="block text-xs font-semibold text-stone-600">
          容量上限
        </label>
        <div className="flex items-center gap-2">
          <select
            id="artifact-storage-quota"
            value={quotaGiB}
            onChange={(event) => setQuotaGiB(event.target.value)}
            disabled={loading || saving}
            className="min-w-40 rounded-lg border border-stone-200 bg-white px-3 py-2 text-xs text-stone-700 outline-none focus:border-[#8CA38A] disabled:opacity-50"
          >
            {ARTIFACT_QUOTA_OPTIONS.map((value) => (
              <option key={value} value={String(value)}>
                {value < 1 ? `${value * 1024} MiB` : `${value} GiB`}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => void saveQuota()}
            disabled={loading || saving}
            className="rounded-lg bg-stone-800 px-4 py-2 text-xs font-semibold text-white hover:bg-stone-900 disabled:opacity-40"
          >
            {saving ? "保存并检查中..." : "保存配额"}
          </button>
        </div>
        <p className="text-[10px] text-stone-400">降低配额后会立即执行一次安全清理。</p>
      </div>

      <div className="flex items-center justify-between gap-4 border-t border-stone-200 pt-4">
        <div>
          <p className="text-xs font-semibold text-stone-700">立即释放可回收空间</p>
          <p className="mt-1 text-[10px] text-stone-400">不会删除知识库源文件、SQLite 向量或远端 R2 对象。</p>
        </div>
        <button
          type="button"
          onClick={() => void cleanup()}
          disabled={loading || cleaning || (status?.reclaimableBytes ?? 0) === 0}
          className="flex items-center gap-1.5 rounded-lg border border-stone-200 px-3 py-2 text-xs font-semibold text-stone-600 hover:bg-stone-50 disabled:opacity-40"
        >
          <Eraser className="h-3.5 w-3.5" />
          {cleaning ? "清理中..." : "立即清理"}
        </button>
      </div>
    </div>
  );
};
