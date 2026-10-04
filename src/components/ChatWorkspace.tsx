import React, { useState, useEffect, useRef } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { readImage, readText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-dialog";
import { Warning, Brain, Check, CaretDown, Copy, Cpu, Books, FileText, ImageSquare, GitBranch, PencilSimple, Plus, PuzzlePiece, ArrowsClockwise, ArrowUp as PaperPlaneRight, ShieldCheck, Square, Terminal, Trash, X } from "@phosphor-icons/react";
import { Button } from "./ui/button";
import { useAgentStore } from "../store/useAgentStore";
import type {
  ChatAttachment,
  ChatAttachmentMetadata,
  PermissionMode,
  SandboxEscalationScope,
  ToolCall,
} from "../store/useAgentStore";
import { AgentAvatar } from "./AgentAvatar";
import { AppLogo } from "./AppLogo";
import { AnimatedDisclosure } from "./AnimatedDisclosure";
import { ChatScrollNav } from "./ChatScrollNav";
import { ContextUsageRing } from "./ContextUsageRing";
import { ModelPicker, type ModelPickerTab } from "./ModelPicker";
import { MarkdownMessage } from "./MarkdownMessage";
import { ModifyMemoryModal } from "./ModifyMemoryModal";
import { useConfirmDialog } from "./ConfirmDialog";
import { listInstalledSkills, type InstalledSkill } from "../lib/skills";
import {
  DEFAULT_CONTEXT_LIMIT,
  DEFAULT_MAX_OUTPUT_TOKENS,
  getCachedAutoFollowStreaming,
  getCachedAutoExpandThoughts,
  subscribeUIPreferenceChanges,
} from "../lib/uiPreferences";
import { selectConversationGreeting } from "../lib/greetings";

// 思考模式/强度选项（与角色卡编辑器保持一致）
const THINKING_OPTIONS: { value: string; label: string; desc: string }[] = [
  { value: "off", label: "关闭", desc: "不启用思考" },
  { value: "auto", label: "自动", desc: "由模型决定思考深度" },
  { value: "low", label: "轻度", desc: "浅层思考，响应更快" },
  { value: "medium", label: "中等", desc: "常规思考深度" },
  { value: "high", label: "深度", desc: "深入推理，消耗更多 token" },
];

const THINKING_LABEL: Record<string, string> = {
  off: "关闭",
  auto: "自动",
  low: "轻度",
  medium: "中等",
  high: "深度",
};

const PERMISSION_OPTIONS: {
  value: PermissionMode;
  label: string;
  desc: string;
}[] = [
  {
    value: "ask_for_approval",
    label: "每次询问",
    desc: "每次本地工具调用前都请求你的批准",
  },
  {
    value: "auto",
    label: "自动模式",
    desc: "由当前模型决定普通调用；高风险操作需要二次确认",
  },
  {
    value: "accept_edits",
    label: "接受编辑",
    desc: "自动读写文件，Shell 与 Git 命令仍会询问",
  },
  {
    value: "full_access",
    label: "完全访问",
    desc: "已启用工具可访问系统并直接执行，不再询问",
  },
];

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif", "svg"]);

function inferImageMediaType(name: string, mediaType?: string) {
  if (mediaType?.startsWith("image/")) return mediaType;
  const extension = name.split(".").pop()?.toLowerCase();
  if (extension === "jpg" || extension === "jpeg") return "image/jpeg";
  if (extension && IMAGE_EXTENSIONS.has(extension)) return `image/${extension}`;
  return null;
}

async function detectImageMediaType(file: File) {
  const inferred = inferImageMediaType(file.name, file.type);
  if (inferred) return inferred;
  const bytes = new Uint8Array(await file.slice(0, 16).arrayBuffer());
  if (bytes.length >= 8 && bytes.slice(0, 8).every((value, index) => value === [137, 80, 78, 71, 13, 10, 26, 10][index])) {
    return "image/png";
  }
  if (bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return "image/jpeg";
  if (String.fromCharCode(...bytes.slice(0, 6)).startsWith("GIF8")) return "image/gif";
  if (
    String.fromCharCode(...bytes.slice(0, 4)) === "RIFF"
    && String.fromCharCode(...bytes.slice(8, 12)) === "WEBP"
  ) return "image/webp";
  if (bytes[0] === 0x42 && bytes[1] === 0x4d) return "image/bmp";
  if (String.fromCharCode(...bytes.slice(4, 12)).includes("ftypavif")) return "image/avif";
  return null;
}

async function readNativeClipboardImageFile() {
  let image: Awaited<ReturnType<typeof readImage>> | null = null;
  try {
    image = await readImage();
    const [{ width, height }, rgba] = await Promise.all([image.size(), image.rgba()]);
    if (width <= 0 || height <= 0 || width * height > 50_000_000) return null;
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return null;
    const pixels = new Uint8ClampedArray(new ArrayBuffer(rgba.byteLength));
    pixels.set(rgba);
    context.putImageData(new ImageData(pixels, width, height), 0, 0);
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
    return blob
      ? new File([blob], `pasted-image-${Date.now()}.png`, { type: "image/png" })
      : null;
  } catch {
    return null;
  } finally {
    await image?.close().catch(() => {});
  }
}

function fileUriToPath(value: string) {
  const uri = value.split(/[\r\n]/).map((item) => item.trim()).find((item) => item && !item.startsWith("#"));
  if (!uri || !uri.toLowerCase().startsWith("file://")) return null;
  try {
    const parsed = new URL(uri);
    const path = decodeURIComponent(parsed.pathname);
    return path || null;
  } catch {
    return null;
  }
}

const PERMISSION_LABEL: Record<PermissionMode, string> = Object.fromEntries(
  PERMISSION_OPTIONS.map((option) => [option.value, option.label]),
) as Record<PermissionMode, string>;

const TOOL_STATUS_LABEL: Record<string, string> = {
  pending_approval: "等待批准",
  running: "执行中",
  succeeded: "已完成",
  denied: "已拒绝",
  failed: "执行失败",
};

interface KnowledgeCollectionOption {
  id: string;
  name: string;
  scope: string;
  permission: string;
  document_count: number;
}

function attachmentIcon(kind: ChatAttachment["kind"] | ChatAttachmentMetadata["attachmentKind"]) {
  if (kind === "knowledge_collection") return Books;
  if (kind === "skill") return PuzzlePiece;
  return FileText;
}

function attachmentIconForMetadata(metadata: ChatAttachmentMetadata) {
  if (metadata.attachmentKind === "local_file" && metadata.mediaType?.startsWith("image/")) {
    return ImageSquare;
  }
  return attachmentIcon(metadata.attachmentKind);
}

const AttachmentChip: React.FC<{
  metadata: ChatAttachmentMetadata;
  onRemove?: () => void;
}> = ({ metadata, onRemove }) => {
  const Icon = attachmentIconForMetadata(metadata);
  const isKnowledge = metadata.attachmentKind === "knowledge_collection";
  const isSkill = metadata.attachmentKind === "skill";
  const isImage = metadata.attachmentKind === "local_file" && Boolean(metadata.mediaType?.startsWith("image/"));
  const typeLabel = isKnowledge
    ? "知识库"
    : isSkill
      ? "Skill"
      : isImage
        ? "图片"
        : "本地文件";

  return (
    <span
      className={`inline-flex max-w-72 items-center gap-1.5 rounded-lg border px-2.5 py-1 text-[11px] select-none transition-all ${
        isKnowledge
          ? "border-amber-300/80 bg-amber-50/80 text-amber-900 dark:border-amber-900/40 dark:bg-amber-950/20 dark:text-amber-200"
          : isSkill
            ? "border-violet-300/80 bg-violet-50/80 text-violet-900 dark:border-violet-900/40 dark:bg-violet-950/20 dark:text-violet-200"
            : isImage
              ? "border-emerald-300/80 bg-emerald-50/80 text-emerald-900 dark:border-emerald-900/40 dark:bg-emerald-950/20 dark:text-emerald-200"
              : "border-stone-300/80 bg-stone-100/80 text-stone-800 dark:border-stone-800 dark:bg-stone-900/50 dark:text-stone-300"
      }`}
      title={`${typeLabel}：${metadata.name}`}
    >
      <Icon className="h-3.5 w-3.5 shrink-0 opacity-80" />
      <span className="truncate font-medium">{metadata.name}</span>
      <span className="shrink-0 text-[10px] opacity-60">· {typeLabel}</span>
      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          className="-mr-1 rounded p-0.5 opacity-60 transition-opacity hover:opacity-100 hover:bg-black/5 dark:hover:bg-white/10"
          title="移除附件"
          aria-label="移除附件"
        >
          <X className="h-3 w-3" />
        </button>
      )}
    </span>
  );
};

function parseToolArgs(args: string): unknown {
  try {
    return JSON.parse(args);
  } catch {
    return args;
  }
}

function toolCallPreview(toolCall: ToolCall): string {
  const parsed = parseToolArgs(toolCall.args);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return typeof parsed === "string" ? parsed : toolCall.args;
  }

  const values = parsed as Record<string, unknown>;
  if (typeof values.command === "string") return values.command;
  if (toolCall.tool === "git" && Array.isArray(values.args)) {
    return `git ${values.args.map(String).join(" ")}`;
  }
  for (const key of ["path", "pattern", "query", "cwd"]) {
    if (typeof values[key] === "string") return values[key] as string;
  }
  return toolCall.args;
}

function formattedToolArgs(args: string): string {
  const parsed = parseToolArgs(args);
  return typeof parsed === "string" ? parsed : JSON.stringify(parsed, null, 2);
}

const MEMORY_MUTATION_TOOLS = new Set([
  "memory_write",
  "memory_str_replace",
  "memory_append",
  "memory_delete",
]);

function savedMemoryPath(toolCall: ToolCall): string | null {
  if (!MEMORY_MUTATION_TOOLS.has(toolCall.tool) || toolCall.status !== "succeeded" || !toolCall.output) {
    return null;
  }
  const output = parseToolArgs(toolCall.output);
  if (!output || typeof output !== "object" || Array.isArray(output)) return null;
  const result = output as Record<string, unknown>;
  return result.ok === true && typeof result.summary === "string" && typeof result.path === "string"
    ? result.path
    : null;
}

function sandboxScopeText(scope: SandboxEscalationScope): string {
  if (scope.kind === "process_filesystem") return "全文件系统（仅本次调用）";
  if (scope.kind === "file_write_root") return `写入目录：${scope.root ?? "未指定"}`;
  const parts = [
    scope.cwd ? `工作目录：${scope.cwd}` : null,
    ...(scope.writable_roots ?? []).map((root) => `写入目录：${root}`),
  ].filter(Boolean);
  return parts.join("；") || "仅当前进程范围";
}

const ToolCallCard: React.FC<{
  toolCall: ToolCall;
  onApprove: (toolCallId: string, approved: boolean) => Promise<void>;
}> = React.memo(({ toolCall: tc, onApprove }) => {
  const isHighRisk = tc.risk === "High";
  const isPending = tc.status === "pending_approval";
  const shouldAutoExpand = isPending || tc.status === "failed" || tc.status === "denied";
  const [expanded, setExpanded] = useState(shouldAutoExpand);
  const [copied, setCopied] = useState(false);
  const preview = toolCallPreview(tc);
  const memoryPath = savedMemoryPath(tc);
  const statusLabel = TOOL_STATUS_LABEL[tc.status] || tc.status;

  useEffect(() => {
    if (shouldAutoExpand) setExpanded(true);
  }, [shouldAutoExpand]);

  const copyOutput = (text: string) => {
    navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  return (
    <AnimatedDisclosure
      open={expanded}
      onOpenChange={setExpanded}
      className="agnes-tool-details group/tool my-2"
      data-status={tc.status}
      summaryClassName="agnes-tool-summary"
      summary={(
        <>
          <span className="agnes-tool-title font-medium">{memoryPath ? "已保存记忆" : "工具调用"}</span>
          <code className="agnes-tool-name font-mono px-1.5 py-0.5 rounded bg-canvas-surface-card border border-hairline text-ink text-[11px]">{tc.tool}</code>
          <code className="agnes-tool-preview text-ink-muted" title={memoryPath ?? preview}>
            {memoryPath ?? preview}
          </code>
          <span className={`agnes-tool-status ${tc.status === "running" ? "animate-pulse" : ""}`}>
            <span className="agnes-tool-status-dot" aria-hidden="true" />
            {statusLabel}
          </span>
          {isHighRisk && (
            <span className="agnes-tool-risk px-1.5 py-0.2 rounded text-[10px] font-medium bg-rose-500/10 text-rose-600 border border-rose-500/20">
              高风险
            </span>
          )}
          <CaretDown className="agnes-collapse-chevron h-3 w-3 shrink-0 text-ink-muted" />
        </>
      )}
    >
      <div className="agnes-tool-content space-y-3 pt-1">
        <span className="agnes-tool-rail-icon" aria-hidden="true">
          <Terminal className="h-3 w-3" />
        </span>

        {tc.args && Object.keys(tc.args).length > 0 && (
          <div>
            <div className="mb-1 text-[10px] font-medium uppercase tracking-wider text-ink-muted">调用参数</div>
            <pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-canvas-surface-card border border-hairline px-3 py-2 font-mono text-[11px] leading-relaxed text-ink-body shadow-subtle">
              {formattedToolArgs(tc.args)}
            </pre>
          </div>
        )}

        {(tc.cwd || tc.networkAllowed !== undefined || tc.permissionMode) && (
          <div className="flex flex-wrap gap-1.5 text-[10px] text-ink-muted">
            {tc.cwd && (
              <span className="max-w-full truncate rounded-md bg-canvas-surface-card border border-hairline px-2 py-0.5 font-mono" title={tc.cwd}>
                cwd: {tc.cwd}
              </span>
            )}
            {tc.networkAllowed !== undefined && (
              <span className="rounded-md bg-canvas-surface-card border border-hairline px-2 py-0.5">
                网络: {tc.networkAllowed ? "允许" : "关闭"} · {tc.landlock ? "Landlock" : "路径策略"}
              </span>
            )}
            {tc.permissionMode && (
              <span className="rounded-md bg-canvas-surface-card border border-hairline px-2 py-0.5">
                {PERMISSION_LABEL[tc.permissionMode]}
              </span>
            )}
          </div>
        )}

        {tc.sandboxEscalationScope && (
          <div className="rounded-md border border-amber-300/70 bg-amber-50/50 px-2.5 py-2 text-[10px] text-ink-secondary dark:bg-amber-950/20">
            <div className="font-medium text-ink">本次沙箱授权范围</div>
            <div className="mt-0.5 break-all font-mono">{sandboxScopeText(tc.sandboxEscalationScope)}</div>
          </div>
        )}

        {tc.diff && (
          <div>
            <div className="mb-1 text-[10px] font-medium uppercase tracking-wider text-ink-muted">变更对比 (Diff)</div>
            <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-[#181715] border border-white/10 px-3 py-2.5 font-mono text-[11px] leading-relaxed text-[#e6dfd8] shadow-elevated">
              {tc.diff}
            </pre>
          </div>
        )}

        {isPending && (
          <div className={`flex items-start gap-2.5 rounded-lg border p-3 shadow-subtle ${
            tc.isSecondaryConfirmation
              ? "border-rose-300/80 bg-rose-50/50 dark:bg-rose-950/20"
              : "border-amber-300/80 bg-amber-50/50 dark:bg-amber-950/20"
          }`}>
            <Warning className={`mt-0.5 h-4 w-4 shrink-0 ${
              tc.isSecondaryConfirmation ? "text-rose-600" : "text-amber-600"
            }`} />
            <div className="flex-1">
              <div className="text-xs font-semibold text-ink">
                {tc.isSecondaryConfirmation ? "需要高风险二次确认" : "需要审批权限"}
              </div>
              <p className="mt-0.5 text-[11px] leading-relaxed text-ink-secondary">
                {tc.approvalReason || "根据当前安全规则，该操作将直接访问本机系统，请确认是否授权运行。"}
              </p>
            </div>
          </div>
        )}

        {tc.output && (
          <div>
            <div className="mb-1 flex items-center justify-between text-[10px] font-medium uppercase tracking-wider text-ink-muted">
              <span>终端输出</span>
              <button
                type="button"
                onClick={() => copyOutput(tc.output || "")}
                className="flex items-center gap-1 text-ink-muted hover:text-ink transition-colors"
              >
                {copied ? <Check className="h-3 w-3 text-emerald-500" /> : <Copy className="h-3 w-3" />}
                <span>{copied ? "已复制" : "复制输出"}</span>
              </button>
            </div>
            <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-[#181715] border border-white/10 px-3 py-2.5 font-mono text-[11px] leading-relaxed text-[#e6dfd8] shadow-elevated">
              {tc.output}
            </pre>
          </div>
        )}

        {isPending && (
          <div className="flex justify-end items-center gap-2 border-t border-hairline pt-2.5">
            <Button
              variant="dangerGhost"
              size="sm"
              onClick={() => onApprove(tc.id, false).catch(console.error)}
            >
              拒绝
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={() => onApprove(tc.id, true).catch(console.error)}
            >
              {tc.isSecondaryConfirmation ? "确认高危放行" : "授权运行"}
            </Button>
          </div>
        )}
      </div>
    </AnimatedDisclosure>
  );
});

interface ChatWorkspaceProps {
  onOpenSettings: (tab: "agents" | "memory" | "models" | "sync" | "tokens" | "skills" | "audit" | "debug" | "runtime") => void;
}

interface ChatUserProfile {
  name: string;
}

export const ChatWorkspace: React.FC<ChatWorkspaceProps> = ({
  onOpenSettings,
}) => {
  const confirmDelete = useConfirmDialog();
  const {
    agents,
    sessions,
    messages,
    activeAgentId,
    activeSessionId,
    draftSession,
    isStreaming,
    isPreparingSession,
    providers,
    modelRoles,
    sendMessage,
    approveTool,
    cancelRun,
    setSessionLlm,
    setSessionPermissionMode,
    updateDraftSession,
    switchVersion,
    createBranch,
    deleteMessage,
    editAndResend,
    regenerateMessage,
    loadProviders,
  } = useAgentStore();

  const [inputVal, setInputVal] = useState("");
  const [mobileInputActive, setMobileInputActive] = useState(false);
  const desktopTextareaRef = useRef<HTMLTextAreaElement>(null);
  const [isDragOver, setIsDragOver] = useState(false);
  const mobileInputBlurTimerRef = useRef<number | null>(null);
  const messageEndRef = useRef<HTMLDivElement>(null);
  const chatStageRef = useRef<HTMLDivElement>(null);
  const messagesContainerRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLDivElement>(null);
  const modelPickerRef = useRef<HTMLDivElement>(null);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const [modelPickerTab, setModelPickerTab] = useState<ModelPickerTab>("models");
  const [attachmentPicker, setAttachmentPicker] = useState<"menu" | "knowledge" | "skill" | null>(null);
  const [attachments, setAttachments] = useState<ChatAttachment[]>([]);
  const [knowledgeCollections, setKnowledgeCollections] = useState<KnowledgeCollectionOption[]>([]);
  const [knowledgeLoading, setKnowledgeLoading] = useState(false);
  const [installedSkills, setInstalledSkills] = useState<InstalledSkill[]>([]);
  const [skillsLoading, setSkillsLoading] = useState(false);
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState("");
  const [memoryEditMsgId, setMemoryEditMsgId] = useState<string | null>(null);
  const [autoExpandThoughts, setAutoExpandThoughts] = useState(getCachedAutoExpandThoughts);
  const [autoFollowStreaming, setAutoFollowStreaming] = useState(getCachedAutoFollowStreaming);
  const [userName, setUserName] = useState("");

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const activeSession = sessions.find((s) => s.id === activeSessionId);
  const hasSessionTarget = Boolean(activeSessionId || draftSession);
  const isEmptyConversation = messages.length === 0;
  const welcomeGreeting = selectConversationGreeting(
    userName,
    new Date(),
    draftSession?.id ?? activeSessionId ?? "new-conversation",
  );

  const resizeTextarea = (textarea: HTMLTextAreaElement, minH: number, maxH: number) => {
    // Reset first so deleting text can shrink the control as well as growing it.
    textarea.style.height = "auto";
    const contentHeight = textarea.scrollHeight;
    const nextH = Math.min(Math.max(contentHeight, minH), maxH);
    textarea.style.height = `${nextH}px`;
    textarea.style.overflowY = contentHeight > maxH ? "auto" : "hidden";
  };

  const adjustTextareaHeight = () => {
    const textarea = desktopTextareaRef.current;
    if (!textarea) return;
    const isMobile = window.matchMedia("(max-width: 640px)").matches;
    resizeTextarea(textarea, isEmptyConversation ? 48 : 38, isMobile ? 136 : 220);
  };

  useEffect(() => {
    adjustTextareaHeight();
  }, [inputVal, isEmptyConversation]);

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!isDragOver) setIsDragOver(true);
  };

  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!e.currentTarget.contains(e.relatedTarget as Node)) {
      setIsDragOver(false);
    }
  };

  const handleDrop = async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragOver(false);
    const droppedFiles = Array.from(e.dataTransfer.files);
    if (droppedFiles.length === 0) return;
    for (const file of droppedFiles) {
      if (file.type.startsWith("image/")) {
        await appendPastedImage(file);
      } else {
        const filePath = (file as unknown as { path?: string }).path;
        if (filePath) {
          appendLocalPaths([filePath]);
        } else {
          await appendPastedImage(file);
        }
      }
    }
  };

  // 拉取服务商与模型列表，供底部模型切换器使用
  useEffect(() => {
    loadProviders().catch(console.error);
  }, [loadProviders]);

  useEffect(() => {
    let active = true;
    const loadUserName = () => {
      invoke<ChatUserProfile>("get_user_profile")
        .then((profile) => {
          if (active) setUserName(profile.name.trim());
        })
        .catch(console.error);
    };
    loadUserName();
    window.addEventListener("agnes:user-profile-change", loadUserName);
    return () => {
      active = false;
      window.removeEventListener("agnes:user-profile-change", loadUserName);
    };
  }, []);

  useEffect(() => subscribeUIPreferenceChanges((change) => {
    if (change.autoExpandThoughts !== undefined) {
      setAutoExpandThoughts(change.autoExpandThoughts);
    }
    if (change.autoFollowStreaming !== undefined) {
      setAutoFollowStreaming(change.autoFollowStreaming);
    }
  }), []);

  useEffect(() => {
    setAttachments([]);
    setAttachmentPicker(null);
    setAttachmentError(null);
    setMobileInputActive(false);
  }, [activeSessionId]);

  useEffect(() => {
    if (!draftSession) return;
    setInputVal("");
    setAttachments([]);
    setAttachmentPicker(null);
    setAttachmentError(null);
    setMobileInputActive(false);
  }, [draftSession?.id]);

  useEffect(() => {
    const handleNativeImeChange = (event: Event) => {
      const detail = (event as CustomEvent<{ visible?: boolean }>).detail;
      if (detail?.visible === false) setMobileInputActive(false);
    };
    window.addEventListener("agnes:native-ime-change", handleNativeImeChange);
    return () => window.removeEventListener("agnes:native-ime-change", handleNativeImeChange);
  }, []);

  useEffect(() => () => {
    if (mobileInputBlurTimerRef.current !== null) {
      window.clearTimeout(mobileInputBlurTimerRef.current);
    }
  }, []);

  const openMobileInput = () => {
    if (!window.matchMedia("(max-width: 640px)").matches) return;
    if (mobileInputBlurTimerRef.current !== null) {
      window.clearTimeout(mobileInputBlurTimerRef.current);
      mobileInputBlurTimerRef.current = null;
    }
    setMobileInputActive(true);
  };

  const closeMobileInputIfBlurred = () => {
    if (mobileInputBlurTimerRef.current !== null) {
      window.clearTimeout(mobileInputBlurTimerRef.current);
    }
    mobileInputBlurTimerRef.current = window.setTimeout(() => {
      mobileInputBlurTimerRef.current = null;
      const activeElement = document.activeElement;
      if (!activeElement?.closest(".agnes-chat-composer")) {
        setMobileInputActive(false);
      }
    }, 80);
  };

  const appendLocalPaths = (paths: string[]) => {
    setAttachments((current) => {
      const existingPaths = new Set(
        current
          .filter((item): item is Extract<ChatAttachment, { kind: "local_file" }> => item.kind === "local_file")
          .map((item) => item.path)
          .filter((path): path is string => Boolean(path)),
      );
      const additions = paths
        .filter((path) => !existingPaths.has(path))
        .map((path): ChatAttachment => ({
          id: crypto.randomUUID(),
          kind: "local_file",
          name: path.split(/[\\/]/).pop() || "未命名附件",
          path,
        }));
      return [...current, ...additions].slice(0, 8);
    });
  };

  const chooseLocalFiles = async (imagesOnly: boolean) => {
    setAttachmentPicker(null);
    setAttachmentError(null);
    try {
      const selected = await open({
        multiple: true,
        title: imagesOnly ? "添加图片附件" : "添加本地文件附件",
        ...(imagesOnly
          ? {
              filters: [
                {
                  name: "图片",
                  extensions: ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"],
                },
              ],
            }
          : {}),
      });
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (paths.length === 0) return;
      appendLocalPaths(paths);
    } catch (reason) {
      setAttachmentError(String(reason));
    }
  };

  const addLocalImages = () => chooseLocalFiles(true);
  const addLocalFiles = () => chooseLocalFiles(false);

  const appendPastedImage = async (file: File) => {
    setAttachmentError(null);
    const mediaType = await detectImageMediaType(file);
    if (!mediaType) {
      setAttachmentError("剪贴板内容不是支持的图片格式");
      return;
    }
    if (file.size > 20 * 1024 * 1024) {
      setAttachmentError("图片不能超过 20 MiB");
      return;
    }
    if (attachments.length >= 8) {
      setAttachmentError("每条消息最多添加 8 个附件");
      return;
    }
    try {
      const dataUrl = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result || ""));
        reader.onerror = () => reject(new Error("无法读取剪贴板图片"));
        reader.readAsDataURL(file);
      });
      const comma = dataUrl.indexOf(",");
      if (comma < 0) throw new Error("剪贴板图片数据格式无效");
      const extension = mediaType.split("/")[1]?.replace("jpeg", "jpg") || "png";
      const pastedAttachment: ChatAttachment = {
        id: crypto.randomUUID(),
        kind: "local_file",
        name: file.name || `pasted-image-${Date.now()}.${extension}`,
        mediaType,
        dataBase64: dataUrl.slice(comma + 1),
      };
      setAttachments((current) => [...current, pastedAttachment].slice(0, 8));
    } catch (reason) {
      setAttachmentError(String(reason));
    }
  };

  const pasteTextAtCursor = (textarea: HTMLTextAreaElement, text: string) => {
    const current = textarea.value;
    const start = textarea.selectionStart ?? current.length;
    const end = textarea.selectionEnd ?? start;
    const next = `${current.slice(0, start)}${text}${current.slice(end)}`;
    setInputVal(next);
    requestAnimationFrame(() => {
      if (!textarea.isConnected) return;
      const cursor = start + text.length;
      textarea.setSelectionRange(cursor, cursor);
    });
  };

  const handleNativePaste = async (textarea: HTMLTextAreaElement) => {
    const image = await readNativeClipboardImageFile();
    if (image) {
      await appendPastedImage(image);
      return;
    }
    try {
      const text = await readText();
      if (text) pasteTextAtCursor(textarea, text);
    } catch (reason) {
      setAttachmentError(`无法读取剪贴板内容：${String(reason)}`);
    }
  };

  const handlePaste = async (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const clipboardItems = Array.from(event.clipboardData.items);
    const clipboardFiles = Array.from(event.clipboardData.files);
    const imageItem = clipboardItems.find((item) => item.type.startsWith("image/"));
    let file = imageItem?.getAsFile()
      ?? clipboardFiles.find((candidate) => Boolean(inferImageMediaType(candidate.name, candidate.type)))
      ?? (clipboardFiles.length === 1 ? clipboardFiles[0] : null);
    const imageUri = fileUriToPath(event.clipboardData.getData("text/uri-list"));
    const imageUriIsImage = Boolean(imageUri && inferImageMediaType(imageUri));
    const clipboardMayContainImage = Boolean(imageItem || file || imageUriIsImage);

    if (clipboardMayContainImage) event.preventDefault();

    // WebKitGTK may expose a pasted bitmap only through the asynchronous Clipboard API.
    if (!file && !imageUriIsImage && isTauri()) {
      file = await readNativeClipboardImageFile();
    }
    if (!file && !imageUriIsImage && navigator.clipboard && "read" in navigator.clipboard) {
      try {
        const nativeItems = await navigator.clipboard.read();
        for (const item of nativeItems) {
          const imageType = item.types.find((type) => type.startsWith("image/"));
          if (!imageType) continue;
          file = new File([await item.getType(imageType)], `pasted-image-${Date.now()}.png`, {
            type: imageType,
          });
          break;
        }
      } catch {
        // The browser may deny clipboard-read permission; the visible error below is more useful.
      }
    }

    if (!file && imageUriIsImage) {
      appendLocalPaths([imageUri!]);
      setAttachmentError(null);
      return;
    }
    if (!file) {
      if (clipboardItems.some((item) => item.type.startsWith("image/"))) {
        setAttachmentError("无法读取剪贴板图片，请使用附件菜单选择图片");
      }
      return;
    }

    await appendPastedImage(file);
  };

  const openKnowledgePicker = async () => {
    if (!activeAgentId) return;
    setAttachmentPicker("knowledge");
    setAttachmentError(null);
    setKnowledgeLoading(true);
    try {
      const collections = await invoke<KnowledgeCollectionOption[]>("list_knowledge_collections", {
        agentId: activeAgentId,
      });
      setKnowledgeCollections(collections);
    } catch (reason) {
      setAttachmentError(String(reason));
      setKnowledgeCollections([]);
    } finally {
      setKnowledgeLoading(false);
    }
  };

  const selectKnowledgeCollection = (collection: KnowledgeCollectionOption) => {
    setAttachments((current) => [
      ...current.filter((item) => item.kind !== "knowledge_collection"),
      {
        id: crypto.randomUUID(),
        kind: "knowledge_collection",
        name: collection.name,
        collectionId: collection.id,
      },
    ]);
    setAttachmentPicker(null);
    setAttachmentError(null);
  };

  const openSkillPicker = async () => {
    setAttachmentPicker("skill");
    setAttachmentError(null);
    setSkillsLoading(true);
    try {
      const skills = await listInstalledSkills();
      setInstalledSkills(skills.filter((skill) => skill.enabled));
    } catch (reason) {
      setAttachmentError(String(reason));
      setInstalledSkills([]);
    } finally {
      setSkillsLoading(false);
    }
  };

  const toggleSkillAttachment = (skill: InstalledSkill) => {
    setAttachments((current) => {
      const selected = current.some((item) => item.kind === "skill" && item.skillId === skill.id);
      if (selected) {
        return current.filter((item) => item.kind !== "skill" || item.skillId !== skill.id);
      }
      if (current.length >= 8) return current;
      return [
        ...current,
        {
          id: crypto.randomUUID(),
          kind: "skill",
          name: skill.name,
          skillId: skill.id,
        },
      ];
    });
    setAttachmentError(null);
  };

  // 当前生效的模型：优先会话级覆盖，回退角色卡默认（形如 "provider_id/model_name"）
  const effectiveModel = draftSession?.model
    ?? (activeSession?.model || activeAgent?.model || modelRoles.main_model || "");
  const currentModel = (() => {
    if (!effectiveModel) return null;
    const idx = effectiveModel.indexOf("/");
    const pid = idx >= 0 ? effectiveModel.slice(0, idx) : "";
    const name = idx >= 0 ? effectiveModel.slice(idx + 1) : effectiveModel;
    const provider = providers.find((p) => p.id === pid);
    return {
      name,
      providerName: provider?.name ?? "",
      descriptor: provider?.models.find((model) => model.id === name),
    };
  })();

  // 当前生效的思考模式（会话级优先，回退角色卡）
  const currentThinkingMode = draftSession?.thinkingMode
    ?? (activeSession?.thinking_mode || activeAgent?.thinking_mode || "off");
  // 当前生效的流式开关（会话级优先，回退角色卡默认，默认开启）
  const currentStreaming = draftSession?.streaming
    ?? (activeSession?.streaming ?? activeAgent?.streaming ?? true);
  const currentMaxTokens = draftSession?.maxTokens ?? activeSession?.max_tokens ?? DEFAULT_MAX_OUTPUT_TOKENS;
  const contextLimit = activeSession?.context_limit ?? currentModel?.descriptor?.context_window ?? DEFAULT_CONTEXT_LIMIT;
  const currentCompressThreshold = draftSession?.compressThreshold ?? activeSession?.compress_threshold ?? 0.85;
  const summaryTriggerTokens = Math.floor(contextLimit * currentCompressThreshold);
  let currentContextTokens = 0;
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (message.role === "assistant" && message.context_tokens > 0) {
      currentContextTokens = message.context_tokens;
      break;
    }
  }
  const currentPermissionMode = draftSession?.permissionMode ?? (activeSession?.permission_mode || "auto");
  const currentPermissionOption = PERMISSION_OPTIONS.find(
    (option) => option.value === currentPermissionMode,
  ) ?? PERMISSION_OPTIONS[1];

  // 持久化会话级模型/思考配置
  const applySessionLlm = (
    model: string,
    thinkingMode: string,
    thinkingBudget: number,
    maxTokens = currentMaxTokens,
    streaming = currentStreaming,
  ) => {
    if (activeSessionId) {
      setSessionLlm(activeSessionId, model, thinkingMode, thinkingBudget, maxTokens, streaming).catch(console.error);
      return;
    }
    if (draftSession) {
      updateDraftSession({ model, thinkingMode, thinkingBudget, maxTokens, streaming });
    }
  };

  const applyPermissionMode = (permissionMode: PermissionMode) => {
    if (activeSessionId) {
      setSessionPermissionMode(activeSessionId, permissionMode).catch(console.error);
      return;
    }
    if (draftSession) updateDraftSession({ permissionMode });
  };

  // Keep user sends visible, while streaming follow remains an optional UI preference.
  useEffect(() => {
    const latestMessage = messages[messages.length - 1];
    if (!autoFollowStreaming && latestMessage?.role !== "user") return;
    messageEndRef.current?.scrollIntoView({ behavior: isStreaming ? "auto" : "smooth" });
  }, [messages, isStreaming, autoFollowStreaming]);

  // Expose the composer height so the floating scroll nav can stay clear of it.
  useEffect(() => {
    const stage = chatStageRef.current;
    const composer = composerRef.current;
    if (!stage || !composer) return;
    const applyHeight = () => {
      stage.style.setProperty("--agnes-composer-h", `${composer.offsetHeight}px`);
    };
    applyHeight();
    const observer = new ResizeObserver(applyHeight);
    observer.observe(composer);
    return () => observer.disconnect();
  }, []);

  const handleSend = async () => {
    if (
      (!inputVal.trim() && attachments.length === 0)
      || isStreaming
      || isPreparingSession
      || !hasSessionTarget
    ) return;
    const text = inputVal.trim() || "请查看附件。";
    setAttachmentError(null);
    try {
      await sendMessage(activeSessionId, text, undefined, attachments);
      setInputVal("");
      setAttachments([]);
      setAttachmentPicker(null);
    } catch (reason) {
      setAttachmentError(String(reason));
    }
  };

  return (
    <main className="agnes-chat-workspace flex flex-1 flex-col bg-[#FAF9F5] relative h-full">
      <div ref={chatStageRef} className="agnes-chat-stage relative min-h-0 flex-1 overflow-hidden">
      {/* Message Panel list */}
      <div
        ref={messagesContainerRef}
        className={`agnes-chat-messages absolute inset-0 -translate-x-1/2 mx-auto w-full max-w-4xl space-y-6 overflow-y-auto p-6 ${
          isEmptyConversation
            ? "pointer-events-none transition-[opacity,transform] duration-500 ease-out motion-reduce:transition-none translate-y-4 opacity-0"
            : "opacity-100"
        }`}
      >
        {messages.map((message, messageIndex) => {
          const isUser = message.role === "user";
          const messageText = message.parts
            .filter((part) => part.kind === "text")
            .map((part) => part.content)
            .join("");
          const messageAttachments = message.parts.filter(
            (part) => part.kind === "attachment" && part.metadata,
          );
          const isLiveAssistant = isStreaming
            && !isUser
            && messageIndex === messages.length - 1
            && (message.status === "pending" || message.status === "streaming");
          const hasAssistantOutput = message.parts.some((part) => (
            part.kind === "tool_call"
              ? Boolean(part.tool_call)
              : Boolean(part.content?.trim())
          ));
          return (
            <div
              key={message._renderKey ?? message.id}
              data-mid={message.id}
              className={`agnes-message-row group flex gap-4 animate-in fade-in slide-in-from-bottom-2 duration-300 ease-out ${isUser ? "justify-end" : "agnes-message-row--assistant"}`}
            >
              {!isUser && activeAgent && (
                <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={32} />
              )}

              <div
                className={`agnes-message-column space-y-1.5 ${
                  isUser && editingMsgId === message.id
                    ? "agnes-message-column--editing w-full max-w-[620px]"
                    : "max-w-[85%] min-w-0"
                } ${isUser ? "order-1" : "order-2"}`}
              >
                {isUser ? (
                  editingMsgId === message.id ? (
                    <div className="agnes-message-editor space-y-3 rounded-xl border px-3.5 py-3 text-sm">
                      <textarea
                        value={editingText}
                        onChange={(e) => setEditingText(e.target.value)}
                        autoFocus
                        className="agnes-message-editor-input w-full resize-y rounded-lg border px-3 py-2.5 text-sm leading-relaxed"
                        rows={4}
                      />
                      <div className="agnes-message-editor-actions flex justify-end gap-2">
                        <button
                          onClick={() => { setEditingMsgId(null); setEditingText(""); }}
                          className="inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[11px] font-medium transition-colors"
                        >
                          <X className="h-3 w-3" />
                          取消
                        </button>
                        <button
                          onClick={() => {
                            const t = editingText.trim();
                            if (!t) return;
                            setEditingMsgId(null);
                            setEditingText("");
                            editAndResend(message.id, t).catch(console.error);
                          }}
                          className="inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[11px] font-semibold transition-colors"
                        >
                          <ArrowsClockwise className="h-3 w-3" />
                          重发
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div className="agnes-user-bubble space-y-2 rounded-2xl rounded-tr-sm border border-hairline bg-canvas-surface-card px-4 py-2.5 text-[13.5px] text-ink shadow-subtle">
                      {messageAttachments.length > 0 && (
                        <div className="flex flex-wrap gap-1.5">
                          {messageAttachments.map((part) => (
                            <AttachmentChip key={part.id} metadata={part.metadata!} />
                          ))}
                        </div>
                      )}
                      {messageText && (
                        <p className="whitespace-pre-wrap leading-relaxed">{messageText}</p>
                      )}
                    </div>
                  )
                ) : (
                  <div className="agnes-assistant-body space-y-3.5">
                    {isLiveAssistant && !hasAssistantOutput && (
                      <div className="agnes-assistant-waiting flex items-center gap-2" role="status" aria-live="polite">
                        <span className="agnes-assistant-waiting-dots inline-flex items-center gap-1" aria-hidden="true">
                          <span />
                          <span />
                          <span />
                        </span>
                        <span>正在等待回复…</span>
                      </div>
                    )}
                    {message.parts.map((part) => {
                      // tool_result 已在工具卡片中展示（tc.output），跳过避免重复泄漏为正文
                      if (part.kind === "tool_result") return null;
                      // tool_call 片段若无关联工具数据，也不当作正文渲染
                      if (part.kind === "tool_call" && !part.tool_call) return null;
                      if (part.kind === "model_fallback") {
                        return (
                          <div
                            key={part._renderKey ?? part.id}
                            className="flex items-center gap-2 border-y border-amber-200 bg-amber-50/60 px-3 py-2 text-[11px] text-amber-800"
                          >
                            <ArrowsClockwise className="h-3.5 w-3.5 shrink-0" />
                            <span>{part.content}</span>
                          </div>
                        );
                      }
                      // 1. Thought Process (reasoning)
                      if (part.kind === "thought") {
                        const isLiveThought = isLiveAssistant && message._streamingInThought === true;
                        return (
                          <AnimatedDisclosure
                            key={part._renderKey ?? part.id}
                            defaultOpen={autoExpandThoughts}
                            className="agnes-thought-details group"
                            summaryClassName="agnes-thought-summary"
                            summary={(
                              <>
                                <span>Agent思维过程</span>
                                <CaretDown className="agnes-collapse-chevron h-3 w-3" />
                              </>
                            )}
                          >
                            <div className="agnes-thought-content">
                              <span
                                className={`agnes-thought-status-icon ${isLiveThought ? "animate-pulse" : ""}`}
                                aria-hidden="true"
                              >
                                <Cpu className="h-3 w-3" />
                              </span>
                              <p className="whitespace-pre-wrap">{part.content}</p>
                            </div>
                          </AnimatedDisclosure>
                        );
                      }

                      // 2. Tool Card
                      if (part.kind === "tool_call" && part.tool_call) {
                        return (
                          <ToolCallCard
                            key={part._renderKey ?? part.id}
                            toolCall={part.tool_call}
                            onApprove={approveTool}
                          />
                        );
                      }

                      // 3. Regular response text rendering (Markdown + LaTeX)
                      return (
                        <MarkdownMessage
                          key={part._renderKey ?? part.id}
                          content={part.content}
                          streaming={isLiveAssistant}
                        />
                      );
                    })}
                  </div>
                )}

                {/* Message actions and usage. Usage remains visible so a completed
                    response can be audited without hovering the message. */}
                <div className="mt-1 flex min-h-5 items-center gap-1">
                  <div className="flex items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                    <button
                      onClick={() => {
                        navigator.clipboard?.writeText(messageText).catch(console.error);
                      }}
                      className="rounded p-1 text-stone-400 hover:bg-stone-200/60 hover:text-stone-700"
                      title="复制消息"
                    >
                      <Copy className="h-3 w-3" />
                    </button>
                    {isUser && (
                      <button
                        onClick={() => {
                          setEditingMsgId(message.id);
                          setEditingText(messageText);
                        }}
                        className="rounded p-1 text-stone-400 hover:bg-stone-200/60 hover:text-stone-700"
                        title="编辑并重发"
                      >
                        <PencilSimple className="h-3 w-3" />
                      </button>
                    )}
                    {!isUser && (
                      <button
                        onClick={() => regenerateMessage(message.id).catch(console.error)}
                        disabled={isStreaming}
                        className="rounded p-1 text-stone-400 hover:bg-stone-200/60 hover:text-stone-700 disabled:opacity-30"
                        title="单条重新生成"
                      >
                        <ArrowsClockwise className="h-3 w-3" />
                      </button>
                    )}
                    {!isUser && (
                      <button
                        onClick={() => setMemoryEditMsgId(message.id)}
                        disabled={message.status !== "complete"}
                        className="rounded p-1 text-stone-400 hover:bg-stone-200/60 hover:text-stone-700 disabled:opacity-30"
                        title="修改记忆"
                      >
                        <Brain className="h-3 w-3" />
                      </button>
                    )}
                    <button
                      onClick={() => createBranch(message.id).catch(console.error)}
                      className="rounded p-1 text-stone-400 hover:bg-stone-200/60 hover:text-stone-700"
                      title="从此处创建分支"
                    >
                      <GitBranch className="h-3 w-3" />
                    </button>
                    <button
                      onClick={async () => {
                        if (!await confirmDelete({
                          title: "删除这条消息？",
                          description: "消息删除后无法恢复。",
                          confirmLabel: "删除消息",
                        })) return;
                        await deleteMessage(message.id);
                      }}
                      disabled={!message.is_leaf || isStreaming || message.status === "pending" || message.status === "streaming"}
                      className="rounded p-1 text-stone-400 hover:bg-red-50 hover:text-red-600 disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-stone-400"
                      title={message.is_leaf ? "删除消息" : "仅可删除末梢消息"}
                    >
                      <Trash className="h-3 w-3" />
                    </button>
                  </div>
                  {!isUser && (
                    <span className="ml-auto whitespace-nowrap font-mono text-[9px] tabular-nums text-stone-400">
                      输入 {message.input_tokens ?? 0} · 缓存 {message.cached_tokens ?? 0} · 输出 {message.output_tokens ?? 0}
                    </span>
                  )}
                </div>

                {message.version_count > 1 && (
                  <div className="flex items-center gap-1 text-[10px] text-stone-400 mt-1">
                    <button
                      onClick={() => switchVersion(message.id, "prev").catch(console.error)}
                      disabled={message.version_index === 0}
                      className="px-1 rounded hover:bg-stone-200 disabled:opacity-30 disabled:hover:bg-transparent"
                      title="上一个版本"
                    >
                      ‹
                    </button>
                    <span className="font-mono">
                      {message.version_index + 1}/{message.version_count}
                    </span>
                    <button
                      onClick={() => switchVersion(message.id, "next").catch(console.error)}
                      disabled={message.version_index + 1 >= message.version_count}
                      className="px-1 rounded hover:bg-stone-200 disabled:opacity-30 disabled:hover:bg-transparent"
                      title="下一个版本"
                    >
                      ›
                    </button>
                  </div>
                )}

                <span className="block text-[9px] text-stone-400 mt-1">
                  {message.created_at || "Just now"}
                </span>
              </div>
            </div>
          );
        })}

        {isStreaming && messages[messages.length - 1]?.role === "user" && (
          <div className="agnes-message-row agnes-message-row--assistant flex gap-4">
            {activeAgent && (
              <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={32} />
            )}
            <div className="agnes-message-column bg-white border border-stone-200 px-4 py-2.5 rounded-2xl rounded-tl-sm flex items-center gap-1 shadow-sm">
              <span className="w-1.5 h-1.5 rounded-full bg-stone-400 animate-bounce"></span>
              <span className="w-1.5 h-1.5 rounded-full bg-stone-400 animate-bounce delay-100"></span>
              <span className="w-1.5 h-1.5 rounded-full bg-stone-400 animate-bounce delay-200"></span>
              <span className="text-[11px] text-stone-400 ml-1 font-mono">
                {activeAgent?.name || "Agnes"} 思考中...
              </span>
            </div>
          </div>
        )}

        <div ref={messageEndRef} />
      </div>

      {/* Input box */}
      <div
        ref={composerRef}
        className={`agnes-chat-composer absolute inset-x-0 -translate-x-1/2 z-20 border-t border-stone-200 bg-[#FAF9F5] p-4 transition-[bottom,transform,background-color] duration-500 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${
          mobileInputActive ? "agnes-chat-composer--mobile-keyboard-open" : ""
        } ${
          isEmptyConversation
            ? "agnes-chat-composer--empty bottom-1/2 translate-y-1/2"
            : "bottom-0 translate-y-0"
        }`}
      >
        <div
          className={`agnes-chat-empty-welcome pointer-events-none absolute inset-x-5 bottom-full mb-7 text-center transition-[opacity,transform] duration-300 ease-out motion-reduce:delay-0 motion-reduce:transition-none ${
            isEmptyConversation
              ? "translate-y-0 opacity-100 delay-150"
              : "translate-y-4 opacity-0"
          }`}
          aria-hidden={!isEmptyConversation}
        >
          <div className="flex items-center justify-center gap-3">
            <AppLogo />
            <h1 className="text-3xl font-serif font-normal text-ink">
              {welcomeGreeting}
            </h1>
          </div>
        </div>
        <div
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
          onDrop={(e) => void handleDrop(e)}
          className={`agnes-chat-composer-box relative mx-auto max-w-4xl rounded-2xl border border-stone-300/80 bg-white p-2.5 shadow-sm transition-[max-width,border-radius,box-shadow] duration-500 focus-within:border-stone-400 ${isEmptyConversation ? "agnes-chat-composer-box--empty" : ""}`}
        >
          {isDragOver && (
            <div className="absolute inset-0 z-30 flex items-center justify-center rounded-2xl border-2 border-dashed border-coral/60 bg-canvas/90 backdrop-blur-sm pointer-events-none transition-all">
              <div className="flex items-center gap-2 text-sm font-medium text-coral">
                <Plus className="h-5 w-5" />
                <span>松开即可添加为附件</span>
              </div>
            </div>
          )}
          {attachments.length > 0 && (
            <div className="flex flex-wrap gap-1.5 px-2 pb-2">
              {attachments.map((attachment) => (
                <AttachmentChip
                  key={attachment.id}
                  metadata={{
                    attachmentKind: attachment.kind,
                    id: attachment.id,
                    name: attachment.name,
                    ...(attachment.kind === "local_file"
                      ? { path: attachment.path, mediaType: attachment.mediaType }
                      : attachment.kind === "knowledge_collection"
                        ? { collectionId: attachment.collectionId }
                        : { skillId: attachment.skillId }),
                  }}
                  onRemove={() => setAttachments((current) => current.filter((item) => item.id !== attachment.id))}
                />
              ))}
            </div>
          )}
          <textarea
            ref={desktopTextareaRef}
            value={inputVal}
            onChange={(e) => setInputVal(e.target.value)}
            onInput={adjustTextareaHeight}
            onFocus={openMobileInput}
            onBlur={closeMobileInputIfBlurred}
            onPaste={(event) => void handlePaste(event)}
            onKeyDown={(e) => {
              if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "v" && isTauri()) {
                e.preventDefault();
                void handleNativePaste(e.currentTarget);
                return;
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
            placeholder={
              isEmptyConversation
                ? "输入一个问题、想法或任务… (Shift+Enter 换行)"
                : activeAgent
                ? `向 ${activeAgent.name} 发送消息... (Enter 发送)`
                : "选择一个会话以开始..."
            }
            className="w-full resize-none bg-transparent px-3 py-1.5 text-[14.5px] leading-relaxed text-ink placeholder:text-ink-muted/50 focus:outline-none"
          />
          {attachmentError && (
            <div className="mx-2 mb-2 rounded-md bg-rose-50 px-2 py-1.5 text-[10px] text-rose-700">
              {attachmentError}
            </div>
          )}
          <div className="flex items-center justify-between pt-2 px-1 text-[10px] text-stone-400">
            <div className="flex min-w-0 items-center gap-2">
              <div className="relative shrink-0">
                <button
                  type="button"
                  onClick={() => {
                    setAttachmentPicker((current) => current ? null : "menu");
                    setModelPickerOpen(false);
                    setAttachmentError(null);
                  }}
                  disabled={!hasSessionTarget || isStreaming || isPreparingSession}
                  className={`flex h-7 w-7 items-center justify-center rounded-lg border transition-all disabled:opacity-40 ${
                    attachmentPicker
                      ? "rotate-45 border-stone-300 bg-stone-100 text-stone-800"
                      : "border-stone-200 bg-white text-stone-500 hover:border-stone-300 hover:bg-stone-50 hover:text-stone-900"
                  }`}
                  title="添加附件"
                  aria-label="添加附件"
                >
                  <Plus className="h-3.5 w-3.5" />
                </button>

                {attachmentPicker && (
                  <>
                    <div className="fixed inset-0 z-40" onClick={() => setAttachmentPicker(null)} />
                    <div className="absolute bottom-full left-0 z-50 mb-2 w-80 overflow-hidden rounded-2xl border border-stone-200 bg-white shadow-2xl">
                      {attachmentPicker === "menu" ? (
                        <div className="p-2">
                          <div className="px-2 pb-1.5 pt-1 text-[10px] font-semibold uppercase tracking-[0.12em] text-stone-400">
                            添加到对话
                          </div>
                          <button
                            type="button"
                            onClick={() => void addLocalImages()}
                            className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-stone-700 transition-colors hover:bg-stone-100"
                          >
                            <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-[#8CA38A]/10 text-[#5F735D]">
                              <ImageSquare className="h-4 w-4" />
                            </span>
                            <span className="min-w-0 flex-1">
                              <span className="block text-[12px] font-semibold">图片</span>
                              <span className="block text-[10px] text-stone-400">发送原图或由图片处理模型识别</span>
                            </span>
                          </button>
                          <button
                            type="button"
                            onClick={() => void addLocalFiles()}
                            className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-stone-700 transition-colors hover:bg-stone-100"
                          >
                            <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-[#8CA38A]/10 text-[#5F735D]">
                              <FileText className="h-4 w-4" />
                            </span>
                            <span className="min-w-0 flex-1">
                              <span className="block text-[12px] font-semibold">本地文件</span>
                              <span className="block text-[10px] text-stone-400">支持 DOCX、PDF、表格及其他文件，AI 可用工具读取</span>
                            </span>
                          </button>
                          <button
                            type="button"
                            onClick={() => void openKnowledgePicker()}
                            className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-stone-700 transition-colors hover:bg-stone-100"
                          >
                            <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-amber-50 text-amber-700">
                              <Books className="h-4 w-4" />
                            </span>
                            <span className="min-w-0 flex-1">
                              <span className="block text-[12px] font-semibold">指定知识库</span>
                              <span className="block text-[10px] text-stone-400">将本轮检索限定在选中的知识库</span>
                            </span>
                          </button>
                          <button
                            type="button"
                            onClick={() => void openSkillPicker()}
                            className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-stone-700 transition-colors hover:bg-stone-100"
                          >
                            <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-violet-50 text-violet-600">
                              <PuzzlePiece className="h-4 w-4" />
                            </span>
                            <span className="min-w-0 flex-1">
                              <span className="block text-[12px] font-semibold">Skills</span>
                              <span className="block text-[10px] text-stone-400">为本轮加载已安装的工作流能力</span>
                            </span>
                          </button>
                        </div>
                      ) : attachmentPicker === "knowledge" ? (
                        <div>
                          <div className="flex items-center gap-2 border-b border-stone-100 px-3 py-2.5">
                            <button
                              type="button"
                              onClick={() => setAttachmentPicker("menu")}
                              className="rounded-lg p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                              title="返回"
                            >
                              <CaretDown className="h-3.5 w-3.5 rotate-90" />
                            </button>
                            <div>
                              <div className="text-[12px] font-semibold text-stone-700">选择知识库</div>
                              <div className="text-[9px] text-stone-400">每条消息可指定一个知识库</div>
                            </div>
                          </div>
                          <div className="max-h-64 overflow-y-auto p-2">
                            {knowledgeLoading ? (
                              <div className="px-3 py-8 text-center text-[10px] text-stone-400">正在加载知识库…</div>
                            ) : knowledgeCollections.length === 0 ? (
                              <div className="px-3 py-8 text-center text-[10px] text-stone-400">暂无可用知识库</div>
                            ) : (
                              knowledgeCollections.map((collection) => (
                                <button
                                  key={collection.id}
                                  type="button"
                                  onClick={() => selectKnowledgeCollection(collection)}
                                  className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition-colors hover:bg-stone-100"
                                >
                                  <Books className="h-4 w-4 shrink-0 text-amber-700" />
                                  <span className="min-w-0 flex-1">
                                    <span className="block truncate text-[11px] font-semibold text-stone-700">{collection.name}</span>
                                    <span className="block text-[9px] text-stone-400">{collection.document_count} 个文档</span>
                                  </span>
                                  {attachments.some((item) => item.kind === "knowledge_collection" && item.collectionId === collection.id) && (
                                    <Check className="h-3.5 w-3.5 text-[#6C806A]" />
                                  )}
                                </button>
                              ))
                            )}
                          </div>
                        </div>
                      ) : (
                        <div>
                          <div className="flex items-center gap-2 border-b border-stone-100 px-3 py-2.5">
                            <button
                              type="button"
                              onClick={() => setAttachmentPicker("menu")}
                              className="rounded-lg p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                              title="返回"
                            >
                              <CaretDown className="h-3.5 w-3.5 rotate-90" />
                            </button>
                            <div className="min-w-0 flex-1">
                              <div className="text-[12px] font-semibold text-stone-700">选择 Skills</div>
                              <div className="text-[9px] text-stone-400">可多选，仅对本轮消息生效</div>
                            </div>
                            <button
                              type="button"
                              onClick={() => {
                                setAttachmentPicker(null);
                                onOpenSettings("skills");
                              }}
                              className="rounded-lg px-2 py-1 text-[9px] font-semibold text-violet-600 hover:bg-violet-50"
                            >
                              管理
                            </button>
                          </div>
                          <div className="max-h-64 overflow-y-auto p-2">
                            {skillsLoading ? (
                              <div className="px-3 py-8 text-center text-[10px] text-stone-400">正在加载 Skills…</div>
                            ) : installedSkills.length === 0 ? (
                              <div className="px-4 py-8 text-center">
                                <PuzzlePiece className="mx-auto mb-2 h-5 w-5 text-stone-300" />
                                <div className="text-[10px] font-semibold text-stone-500">暂无已启用的 Skill</div>
                                <button
                                  type="button"
                                  onClick={() => {
                                    setAttachmentPicker(null);
                                    onOpenSettings("skills");
                                  }}
                                  className="mt-2 rounded-lg bg-violet-50 px-2.5 py-1.5 text-[9px] font-semibold text-violet-700 hover:bg-violet-100"
                                >
                                  前往安装
                                </button>
                              </div>
                            ) : (
                              installedSkills.map((skill) => {
                                const selected = attachments.some(
                                  (item) => item.kind === "skill" && item.skillId === skill.id,
                                );
                                return (
                                  <button
                                    key={skill.id}
                                    type="button"
                                    onClick={() => toggleSkillAttachment(skill)}
                                    className={`flex w-full items-start gap-3 rounded-xl px-3 py-2.5 text-left transition-colors ${
                                      selected ? "bg-violet-50" : "hover:bg-stone-100"
                                    }`}
                                  >
                                    <PuzzlePiece className={`mt-0.5 h-4 w-4 shrink-0 ${selected ? "text-violet-600" : "text-stone-400"}`} />
                                    <span className="min-w-0 flex-1">
                                      <span className="block truncate text-[11px] font-semibold text-stone-700">{skill.name}</span>
                                      <span className="mt-0.5 line-clamp-2 block text-[9px] leading-relaxed text-stone-400">{skill.description}</span>
                                    </span>
                                    {selected && <Check className="mt-0.5 h-3.5 w-3.5 shrink-0 text-violet-600" />}
                                  </button>
                                );
                              })
                            )}
                          </div>
                        </div>
                      )}
                    </div>
                  </>
                )}
              </div>
              <div className="flex items-center gap-1.5 text-[11px] text-ink-muted">
                <button
                  type="button"
                  onClick={() => {
                    setModelPickerTab("settings");
                    setModelPickerOpen(true);
                    setAttachmentPicker(null);
                  }}
                  className={`inline-flex items-center gap-1 whitespace-nowrap rounded-md px-1.5 py-0.5 transition-colors hover:bg-canvas-surface-card ${
                    currentPermissionMode === "full_access"
                      ? "text-rose-600 font-medium bg-rose-50/80 dark:bg-rose-950/30"
                      : "text-ink-muted hover:text-ink"
                  }`}
                  title={currentPermissionOption.desc}
                >
                  <ShieldCheck className="h-3 w-3" />
                  <span>{currentPermissionOption.label}</span>
                </button>
                {attachments.length > 0 && (
                  <span className="inline-flex items-center gap-1 rounded-md bg-coral/10 px-1.5 py-0.5 text-coral font-medium text-[10px]">
                    {attachments.length} 个附件
                  </span>
                )}
              </div>
            </div>
            <div className="flex items-center gap-2">
              {/* Model, thinking, and permission switcher */}
              <div ref={modelPickerRef} className="relative">
                <button
                  onClick={() => {
                    setModelPickerTab("models");
                    setModelPickerOpen((v) => !v);
                    setAttachmentPicker(null);
                  }}
                  className="flex items-center gap-1.5 rounded-lg border border-stone-200 bg-white px-2.5 py-1 text-[10px] text-stone-600 hover:text-stone-900 hover:bg-stone-50 transition-colors"
                  type="button"
                  aria-haspopup="dialog"
                  aria-expanded={modelPickerOpen}
                  aria-controls={modelPickerOpen ? "chat-model-picker" : undefined}
                  title="模型与会话设置"
                >
                  <Cpu className="h-3 w-3 text-[#8CA38A]" />
                  <span className="max-w-[96px] truncate font-mono sm:max-w-[160px]">
                    {currentModel ? currentModel.name : "选择模型"}
                    {currentThinkingMode !== "off" && (
                      <span className="ml-1 text-[9px] text-violet-500 not-italic font-sans">
                        · {THINKING_LABEL[currentThinkingMode] ?? currentThinkingMode}思考
                      </span>
                    )}
                  </span>
                  <ContextUsageRing
                    usedTokens={currentContextTokens}
                    limitTokens={contextLimit}
                    warningThreshold={currentCompressThreshold}
                  />
                  <CaretDown className="h-3 w-3" />
                </button>

                {modelPickerOpen && (
                  <ModelPicker
                    anchorRef={modelPickerRef}
                    providers={providers}
                    effectiveModel={effectiveModel}
                    tab={modelPickerTab}
                    onTabChange={setModelPickerTab}
                    onSelect={(model) => applySessionLlm(model, currentThinkingMode, 0)}
                    onClose={() => setModelPickerOpen(false)}
                  >
                      <div className="px-1">
                        <div className="mb-1 text-xs font-semibold text-ink-muted">
                          回复方式
                        </div>
                        <div className="grid grid-cols-2 gap-1">
                          {[
                            { value: true, label: "流式" },
                            { value: false, label: "非流式 · 整段" },
                          ].map((opt) => (
                            <button
                              key={String(opt.value)}
                              type="button"
                              aria-pressed={currentStreaming === opt.value}
                              onClick={(event) => {
                                event.stopPropagation();
                                applySessionLlm(effectiveModel, currentThinkingMode, 0, currentMaxTokens, opt.value);
                              }}
                              className={`min-h-11 px-1 py-1 rounded-md text-xs font-semibold transition-colors border ${
                                currentStreaming === opt.value
                                  ? "bg-clay-soft text-ink border-clay"
                                  : "bg-canvas-surface-soft text-ink-muted border-hairline hover:bg-canvas-surface-strong"
                              }`}
                            >
                              {opt.label}
                            </button>
                          ))}
                        </div>
                      </div>
                      {/* Thinking depth for the current session. */}
                      <div className="mt-3 pt-3 border-t border-hairline">
                        <div className="px-1 mb-1 text-xs font-semibold text-ink-muted">
                          思考模式 / 强度
                        </div>
                        <div className="grid grid-cols-5 gap-1 px-1">
                          {THINKING_OPTIONS.map((opt) => (
                            <button
                              key={opt.value}
                              type="button"
                              title={opt.desc}
                              aria-pressed={currentThinkingMode === opt.value}
                              onClick={() => applySessionLlm(effectiveModel, opt.value, 0)}
                              className={`min-h-11 px-1 py-1 rounded-md text-xs font-semibold transition-colors border ${
                                currentThinkingMode === opt.value
                                  ? "bg-clay-soft text-ink border-clay"
                                  : "bg-canvas-surface-soft text-ink-muted border-hairline hover:bg-canvas-surface-strong"
                              }`}
                            >
                              {opt.label}
                            </button>
                          ))}
                        </div>
                      </div>
                      <div className="mt-3 border-t border-hairline pt-3">
                        <div className="mb-1 flex items-center gap-1.5 px-1 text-xs font-semibold text-ink-muted">
                          <ShieldCheck className="h-3 w-3" />
                          <span>运行模式</span>
                        </div>
                        <div className="grid grid-cols-2 gap-1 px-1">
                          {PERMISSION_OPTIONS.map((option) => {
                            const isActive = option.value === currentPermissionMode;
                            const isFullAccess = option.value === "full_access";
                            return (
                              <button
                                key={option.value}
                                type="button"
                                aria-pressed={isActive}
                                disabled={!hasSessionTarget || isStreaming || isPreparingSession}
                                onClick={() => applyPermissionMode(option.value)}
                                className={`min-h-14 rounded-lg border px-2 py-1.5 text-left transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                                  isActive
                                    ? isFullAccess
                                      ? "border-rose-400/50 bg-rose-500/10 text-rose-600 dark:text-rose-300"
                                      : "border-clay bg-clay-soft text-ink"
                                    : isFullAccess
                                      ? "border-hairline text-rose-600 dark:text-rose-300 hover:border-rose-400/50 hover:bg-rose-500/10"
                                      : "border-hairline text-ink-body hover:bg-canvas-surface-strong"
                                }`}
                              >
                                <span className="flex items-center justify-between gap-1 text-xs font-semibold">
                                  {option.label}
                                  {isActive && <Check className="h-3 w-3 shrink-0" />}
                                </span>
                                <span className="mt-0.5 line-clamp-2 block text-[11px] leading-relaxed opacity-70">
                                  {option.desc}
                                </span>
                              </button>
                            );
                          })}
                        </div>
                      </div>
                      <div className="mt-2 flex items-center gap-2 border-t border-hairline px-1 pt-2">
                        <label htmlFor="session-max-tokens" className="min-w-0 flex-1 text-xs font-semibold text-ink-muted">
                          最大输出 Token
                        </label>
                        <input
                          key={`${activeSessionId}-${currentMaxTokens}`}
                          id="session-max-tokens"
                          type="number"
                          min={128}
                          max={1048576}
                          step={256}
                          defaultValue={currentMaxTokens}
                          onClick={(event) => event.stopPropagation()}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") event.currentTarget.blur();
                          }}
                          onBlur={(event) => {
                            const value = Math.min(1048576, Math.max(128, Number(event.currentTarget.value) || DEFAULT_MAX_OUTPUT_TOKENS));
                            event.currentTarget.value = String(value);
                            if (value !== currentMaxTokens) {
                              applySessionLlm(effectiveModel, currentThinkingMode, 0, value);
                            }
                          }}
                          className="h-10 w-24 rounded-md border border-hairline bg-canvas-surface-soft px-2 text-right font-mono text-xs text-ink outline-none focus:border-emerald-400"
                          aria-label="最大输出 Token"
                        />
                      </div>
                      <div className="mt-2 flex items-center gap-2 border-t border-hairline px-1 pt-2">
                        <label htmlFor="session-compress-threshold" className="min-w-0 flex-1 text-xs font-semibold text-ink-muted">
                          自动总结阈值
                        </label>
                        <input
                          key={`${activeSessionId}-${currentCompressThreshold}`}
                          id="session-compress-threshold"
                          type="number"
                          min={0}
                          max={1}
                          step={0.05}
                          defaultValue={currentCompressThreshold}
                          onClick={(event) => event.stopPropagation()}
                          onKeyDown={(event) => {
                            if (event.key === "Enter") event.currentTarget.blur();
                          }}
                          onBlur={(event) => {
                            const value = Math.min(1, Math.max(0, Number(event.currentTarget.value) || 0));
                            event.currentTarget.value = String(value);
                            if (value === currentCompressThreshold) return;
                            if (activeSessionId) {
                              useAgentStore.getState().setSessionCompressThreshold(activeSessionId, value).catch(console.error);
                            } else if (draftSession) {
                              updateDraftSession({ compressThreshold: value });
                            }
                          }}
                          className="h-10 w-20 rounded-md border border-hairline bg-canvas-surface-soft px-2 text-right font-mono text-xs text-ink outline-none focus:border-emerald-400"
                          aria-label="自动总结阈值"
                        />
                        <span className="shrink-0 text-[11px] text-ink-muted">
                          触发 {summaryTriggerTokens.toLocaleString()}
                        </span>
                      </div>
                  </ModelPicker>
                )}
              </div>

              <span className="hidden sm:inline-block text-[10px] text-ink-muted/50 select-none mr-0.5">
                Shift+Enter 换行
              </span>

              {isStreaming ? (
                <button
                  type="button"
                  onClick={() => activeSessionId && cancelRun(activeSessionId).catch(console.error)}
                  className="agnes-chat-stop-button flex h-7 w-7 sm:h-8 sm:w-8 items-center justify-center rounded-lg bg-red-600 text-white shadow-sm transition-all hover:bg-red-700 active:scale-95"
                  title="停止生成"
                  aria-label="停止生成"
                >
                  <Square className="h-3 w-3 fill-current" />
                </button>
              ) : (
                <button
                  type="button"
                  onClick={handleSend}
                  disabled={
                    (!inputVal.trim() && attachments.length === 0)
                    || !hasSessionTarget
                    || isPreparingSession
                  }
                  className="agnes-chat-send-button flex h-7 w-7 sm:h-8 sm:w-8 items-center justify-center rounded-lg bg-coral text-white shadow-sm transition-all hover:bg-coral/90 active:scale-95 disabled:opacity-30 disabled:cursor-not-allowed disabled:hover:scale-100"
                  title={hasSessionTarget ? "发送 (Enter)" : "请先选择或新建会话"}
                  aria-label="发送"
                >
                  <PaperPlaneRight className="h-3.5 w-3.5 sm:h-4 sm:w-4" weight="bold" />
                </button>
              )}
            </div>
          </div>
        </div>
      </div>

      <ChatScrollNav
        containerRef={messagesContainerRef}
        messages={messages}
        visible={!isEmptyConversation}
      />

      </div>

      <ModifyMemoryModal
        message={memoryEditMsgId ? messages.find((m) => m.id === memoryEditMsgId) ?? null : null}
        onClose={() => setMemoryEditMsgId(null)}
      />
    </main>
  );
};
