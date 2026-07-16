import React, { useState, useEffect } from "react";
import { X, User, Database, Sliders, ShieldCheck, Key, Plus, Trash2, Pencil, Check, Zap, Server, Download, Eye, EyeOff, Terminal, Settings, Search } from "lucide-react";
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
import { formatMemoryTime, memoryMatchesQuery, parseMemoryKeywords } from "../lib/memory";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  initialTab?: "general" | "agents" | "memory" | "llm" | "audit" | "debug";
}

interface AuditLog {
  id: string;
  time: string;
  tool: string;
  params: string;
  status: string;
  risk: string;
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

const EMPTY_FORM: ProviderFormValues = {
  name: "",
  kind: "openai",
  api_base: "",
  api_key: "",
  models: [],
  modelDraft: "",
  is_default: false,
};

type ModelRoleField = keyof ModelRoleAssignments;

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
  { key: "quick_model", label: "快速模型", desc: "预留划线翻译、搜索和名词解释" },
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
      const knownKeys = new Set(["shell", "file", "git", "memory", "network", "sandbox"]);
      Object.entries(obj).forEach(([key, value]) => {
        if (!knownKeys.has(key)) base[key] = value;
      });
    }
    (["shell", "file", "git", "memory"] as const).forEach((k) => {
      const t = obj?.[k];
      if (t && typeof t === "object") {
        const legacyDefaults: Record<typeof k, ApprovalTier> = {
          shell: "on_risk",
          file: "on_write",
          git: "on_risk",
          memory: "on_write",
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
  const [activeTab, setActiveTab] = useState<"general" | "agents" | "memory" | "llm" | "audit" | "debug">(initialTab);
  
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

  // Audit state
  const [auditLogs, setAuditLogs] = useState<AuditLog[]>([]);

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
  const renderToolToggle = (key: "shell" | "file" | "git" | "memory", label: string) => (
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
      Promise.all([
        invoke<{ user_md: string; memory_md: string }>("get_explicit_memories", {
          agentId: activeAgentId,
        }),
        invoke<StructuredMemory[]>("list_memories", { agentId: activeAgentId }),
      ])
        .then(([explicit, memories]) => {
          setUserMdText(explicit.user_md);
          setMemoryMdText(explicit.memory_md);
          setStructuredMemories(memories);
          setIsEditingUserMd(false);
          setIsEditingMemoryMd(false);
          setEditingMemoryId(null);
          setMemoryError(null);
        })
        .catch(console.error);
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

  // Load providers when LLM tab is activated
  useEffect(() => {
    if (activeTab === "llm") {
      loadProviders();
      loadModelRoles();
    }
  }, [activeTab, loadProviders, loadModelRoles]);

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
    const memories = await invoke<StructuredMemory[]>("list_memories", {
      agentId: activeAgentId,
    });
    setStructuredMemories(memories);
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

  // --- Provider editor helpers ---
  const openAddProvider = () => {
    setEditingProviderId("new");
    setFormValues(EMPTY_FORM);
    setShowApiKey(false);
    setTestResult(null);
  };

  const openEditProvider = async (provider: ModelProvider) => {
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
    // 回显已保存的 API Key（默认密码形式展示，可由眼睛图标切换明文）
    try {
      const key = await invoke<string | null>("get_provider_api_key", { providerId: provider.id });
      if (key) {
        setFormValues((prev) => ({ ...prev, api_key: key }));
      }
    } catch (e) {
      console.error("Failed to load provider api key", e);
    }
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
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm">
      <div className="w-[960px] h-[640px] border border-stone-200 bg-white rounded-2xl overflow-hidden shadow-2xl flex flex-col">
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
          <nav className="w-56 border-r border-stone-200 bg-stone-50/50 p-3 flex flex-col gap-1 shrink-0">
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
          <div className="flex-1 overflow-y-auto p-6 bg-white">
            {/* 0. GENERAL TAB */}
            {activeTab === "general" && (
              <GeneralTab />
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

                {/* Provider List */}
                <div className="space-y-2.5">
                  {providers.map((provider) => (
                    <div
                      key={provider.id}
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
                    <div className="border border-dashed border-stone-200 rounded-xl p-8 text-center">
                      <Server className="h-8 w-8 text-stone-300 mx-auto mb-2" />
                      <p className="text-xs text-stone-400">尚未配置任何模型服务商</p>
                      <p className="text-[10px] text-stone-350 mt-0.5">点击下方按钮添加您的第一个服务商</p>
                    </div>
                  )}
                </div>

                {/* Test result (shown globally when no editor is open) */}
                {testResult && !editingProviderId && (
                  <div className={`flex items-center gap-2 px-3 py-2 rounded-lg text-xs border transition-all ${
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
                  <div className="border border-[#8CA38A]/30 bg-[#FAF9F5]/40 rounded-xl p-5 space-y-4 shadow-sm ring-1 ring-[#8CA38A]/10">
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
                              ? (editingProvider?.has_api_key ? "已保存，留空则保持不变" : "留空则保持原密钥不变")
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
                          已保存 API Key（重新输入将覆盖）
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

                {/* Section 3: Cloud Sync (preserved) */}
                <div className="border-t border-stone-200 pt-5 mt-2">
                  <div className="border border-stone-200 bg-[#FAF9F5]/30 rounded-xl p-5 space-y-4 shadow-sm">
                    <div className="space-y-3">
                      <span className="block text-xs font-semibold text-stone-500 uppercase tracking-wide">
                        云端同步网关 (Incremental Sync)
                      </span>
                      <div className="grid grid-cols-2 gap-4 text-xs">
                        <div>
                          <label className="block text-stone-400 mb-1">同步网关 Worker URL</label>
                          <input
                            type="text"
                            value="https://agnes-sync.caiwen.workers.dev"
                            disabled
                            className="w-full bg-stone-50 border border-stone-200 rounded-lg px-3 py-1.5 text-stone-500 focus:outline-none"
                          />
                        </div>
                        <div>
                          <label className="block text-stone-400 mb-1">本机同步标识 (Device UUID)</label>
                          <input
                            type="text"
                            value="7d938f32-cf72-4e9f-863a-ea9387d8df93"
                            disabled
                            className="w-full bg-stone-100 border border-stone-200 rounded-lg px-3 py-1.5 text-stone-400 font-mono"
                          />
                        </div>
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            )}

            {/* 4. AUDIT TAB */}
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
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    invoke<string | null>("get_setting", { key: "ui:session_open_mode" })
      .then((v) => setOpenMode(v ?? "last"))
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

  return (
    <div className="space-y-6 max-w-xl">
      <div>
        <h2 className="text-sm font-semibold text-stone-800 mb-1">通用设置</h2>
        <p className="text-[11px] text-stone-400">应用启动行为与界面偏好</p>
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
    </div>
  );
};
