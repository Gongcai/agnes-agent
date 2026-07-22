import React, { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Warning as AlertTriangle,
  Brain,
  Check,
  CaretDown as ChevronDown,
  Copy,
  Cpu,
  Books,
  FileText,
  GitBranch,
  PencilSimple as Pencil,
  Plus,
  PuzzlePiece,
  ArrowsClockwise as RefreshCw,
  ArrowUp as Send,
  HardDrives as Server,
  ShieldCheck,
  Sparkle as Sparkles,
  Square,
  TerminalWindow as Terminal,
  Trash as Trash2,
  X,
} from "@phosphor-icons/react";
import { Button } from "./ui/button";
import { useAgentStore } from "../store/useAgentStore";
import type {
  ChatAttachment,
  ChatAttachmentMetadata,
  PermissionMode,
  ToolCall,
} from "../store/useAgentStore";
import { AgentAvatar } from "./AgentAvatar";
import { MarkdownMessage } from "./MarkdownMessage";
import { ModifyMemoryModal } from "./ModifyMemoryModal";
import { ThoughtDetails } from "./ThoughtDetails";
import { listInstalledSkills, type InstalledSkill } from "../lib/skills";
import {
  DEFAULT_MAX_OUTPUT_TOKENS,
  getCachedAutoFollowStreaming,
  getCachedAutoExpandThoughts,
  subscribeUIPreferenceChanges,
} from "../lib/uiPreferences";

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

const AttachmentChip: React.FC<{
  metadata: ChatAttachmentMetadata;
  onRemove?: () => void;
}> = ({ metadata, onRemove }) => {
  const Icon = attachmentIcon(metadata.attachmentKind);
  const typeLabel = metadata.attachmentKind === "knowledge_collection"
    ? "知识库"
    : metadata.attachmentKind === "skill"
      ? "Skill"
      : "本地文件";
  return (
    <span
      className="inline-flex max-w-64 items-center gap-1.5 rounded-lg border border-stone-200 bg-stone-50 px-2 py-1 text-[10px] text-stone-600"
      title={`${typeLabel}：${metadata.name}`}
    >
      <Icon className="h-3.5 w-3.5 shrink-0 text-[#6C806A]" />
      <span className="truncate font-medium">{metadata.name}</span>
      <span className="shrink-0 text-[9px] text-stone-400">{typeLabel}</span>
      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          className="-mr-1 rounded p-0.5 text-stone-400 transition-colors hover:bg-stone-200 hover:text-stone-700"
          title="移除附件"
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

const ToolCallCard: React.FC<{
  toolCall: ToolCall;
  onApprove: (toolCallId: string, approved: boolean) => Promise<void>;
}> = React.memo(({ toolCall: tc, onApprove }) => {
  const isHighRisk = tc.risk === "High";
  const isPending = tc.status === "pending_approval";
  const shouldAutoExpand = isPending || tc.status === "failed" || tc.status === "denied";
  const [expanded, setExpanded] = useState(shouldAutoExpand);
  const preview = toolCallPreview(tc);
  const statusLabel = TOOL_STATUS_LABEL[tc.status] || tc.status;

  useEffect(() => {
    if (shouldAutoExpand) setExpanded(true);
  }, [shouldAutoExpand]);

  return (
    <details
      open={expanded}
      onToggle={(event) => setExpanded(event.currentTarget.open)}
      className={`group/tool overflow-hidden rounded-lg border transition-colors ${
        isPending
          ? isHighRisk
            ? "border-rose-300 bg-rose-50/40"
            : "border-amber-300 bg-amber-50/40"
          : tc.status === "failed" || tc.status === "denied"
          ? "border-rose-200 bg-rose-50/20"
          : "border-stone-200 bg-white"
      }`}
    >
      <summary className="flex min-w-0 cursor-pointer list-none items-center gap-2 px-3 py-2 text-xs select-none hover:bg-stone-50/80 [&::-webkit-details-marker]:hidden">
        <Terminal className="h-3.5 w-3.5 shrink-0 text-stone-500" />
        <span className="shrink-0 font-semibold text-stone-700">{tc.tool}</span>
        <code className="min-w-0 flex-1 truncate text-[10px] font-normal text-stone-400" title={preview}>
          {preview}
        </code>
        <span className={`shrink-0 rounded px-2 py-0.5 text-[10px] ${
          tc.status === "running"
            ? "bg-blue-100 text-blue-700 animate-pulse"
            : tc.status === "succeeded"
            ? "bg-emerald-100 text-emerald-700"
            : tc.status === "failed" || tc.status === "denied"
            ? "bg-rose-100 text-rose-700"
            : "bg-amber-100 text-amber-700"
        }`}>
          {statusLabel}
        </span>
        <span className={`shrink-0 rounded px-2 py-0.5 text-[10px] ${
          isHighRisk ? "bg-rose-100 text-rose-700" : "bg-stone-100 text-stone-500"
        }`}>
          {tc.risk}
        </span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0 text-stone-400 transition-transform group-open/tool:rotate-180" />
      </summary>

      {expanded && (
        <div className="space-y-3 border-t border-stone-200/80 px-3 py-3 text-xs text-stone-700">
          <div>
            <div className="mb-1 text-[10px] font-medium text-stone-400">参数</div>
            <pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all rounded-md bg-stone-100 px-3 py-2 font-mono text-[10px] leading-relaxed text-stone-700">
              {formattedToolArgs(tc.args)}
            </pre>
          </div>

          {(tc.cwd || tc.networkAllowed !== undefined || tc.permissionMode) && (
            <div className="flex flex-wrap gap-1.5 text-[9px] text-stone-500">
              {tc.cwd && (
                <span className="max-w-full truncate rounded bg-stone-100 px-2 py-1 font-mono" title={tc.cwd}>
                  cwd: {tc.cwd}
                </span>
              )}
              {tc.networkAllowed !== undefined && (
                <span className="rounded bg-stone-100 px-2 py-1">
                  网络: {tc.networkAllowed ? "允许" : "关闭"} · {tc.landlock ? "Landlock" : "路径策略"}
                </span>
              )}
              {tc.permissionMode && (
                <span className="rounded bg-stone-100 px-2 py-1">
                  {PERMISSION_LABEL[tc.permissionMode]}
                </span>
              )}
            </div>
          )}

          {tc.diff && (
            <div>
              <div className="mb-1 text-[10px] font-medium text-stone-400">变更预览</div>
              <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-md bg-stone-950 px-3 py-2 font-mono text-[10px] leading-relaxed text-stone-200">
                {tc.diff}
              </pre>
            </div>
          )}

          {isPending && (
            <div className={`flex items-start gap-2 rounded-md border bg-white p-2.5 ${
              tc.isSecondaryConfirmation ? "border-rose-200" : "border-amber-200"
            }`}>
              <AlertTriangle className={`mt-0.5 h-3.5 w-3.5 shrink-0 ${
                tc.isSecondaryConfirmation ? "text-rose-500" : "text-amber-500"
              }`} />
              <p className="text-[10px] leading-relaxed text-stone-600">
                {tc.approvalReason || "根据当前权限规则，此工具调用需要人工审核批准。"}
              </p>
            </div>
          )}

          {tc.output && (
            <div>
              <div className="mb-1 text-[10px] font-medium text-stone-400">输出</div>
              <pre className="max-h-44 overflow-auto whitespace-pre-wrap break-all rounded-md bg-zinc-900 px-3 py-2 font-mono text-[10px] leading-relaxed text-zinc-300">
                {tc.output}
              </pre>
            </div>
          )}

          {isPending && (
            <div className="flex justify-end gap-2 border-t border-stone-200/70 pt-2">
              <button
                onClick={() => onApprove(tc.id, false).catch(console.error)}
                className="rounded-md border border-rose-200 bg-rose-50 px-3 py-1 text-[10px] font-medium text-rose-600 hover:bg-rose-100"
              >
                拒绝
              </button>
              <button
                onClick={() => onApprove(tc.id, true).catch(console.error)}
                className="rounded-md border border-emerald-200 bg-emerald-50 px-3 py-1 text-[10px] font-semibold text-emerald-700 hover:bg-emerald-100"
              >
                {tc.isSecondaryConfirmation ? "确认高危操作" : "授权运行"}
              </button>
            </div>
          )}
        </div>
      )}
    </details>
  );
});

interface ChatWorkspaceProps {
  onOpenSettings: (tab: "agents" | "memory" | "llm" | "tokens" | "skills" | "audit" | "debug") => void;
}

export const ChatWorkspace: React.FC<ChatWorkspaceProps> = ({
  onOpenSettings,
}) => {
  const {
    agents,
    sessions,
    messages,
    activeAgentId,
    activeSessionId,
    isStreaming,
    providers,
    modelRoles,
    sendMessage,
    approveTool,
    cancelRun,
    setSessionLlm,
    setSessionPermissionMode,
    switchVersion,
    createBranch,
    deleteMessage,
    editAndResend,
    regenerateMessage,
    loadProviders,
  } = useAgentStore();

  const [inputVal, setInputVal] = useState("");
  const messageEndRef = useRef<HTMLDivElement>(null);
  const [modelPickerOpen, setModelPickerOpen] = useState(false);
  const [permissionPickerOpen, setPermissionPickerOpen] = useState(false);
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

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const activeSession = sessions.find((s) => s.id === activeSessionId);
  const isEmptyConversation = messages.length === 0;

  // 拉取服务商与模型列表，供底部模型切换器使用
  useEffect(() => {
    loadProviders().catch(console.error);
  }, [loadProviders]);

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
  }, [activeSessionId]);

  const addLocalFiles = async () => {
    setAttachmentPicker(null);
    setAttachmentError(null);
    try {
      const selected = await open({
        multiple: true,
        title: "添加本地文本附件",
        filters: [
          {
            name: "文本、Markdown、CSV 与 JSON",
            extensions: ["md", "markdown", "txt", "rst", "log", "csv", "json"],
          },
        ],
      });
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (paths.length === 0) return;
      setAttachments((current) => {
        const existingPaths = new Set(
          current
            .filter((item): item is Extract<ChatAttachment, { kind: "local_file" }> => item.kind === "local_file")
            .map((item) => item.path),
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
    } catch (reason) {
      setAttachmentError(String(reason));
    }
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
  const effectiveModel = activeSession?.model || activeAgent?.model || modelRoles.main_model || "";
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
  const currentThinkingMode = activeSession?.thinking_mode || activeAgent?.thinking_mode || "off";
  const currentMaxTokens = activeSession?.max_tokens ?? DEFAULT_MAX_OUTPUT_TOKENS;
  const contextLimit = activeSession?.context_limit ?? currentModel?.descriptor?.context_window ?? 8192;
  const currentCompressThreshold = activeSession?.compress_threshold ?? 0.85;
  const summaryTriggerTokens = Math.floor(contextLimit * currentCompressThreshold);
  const currentPermissionMode = activeSession?.permission_mode || "auto";

  // 持久化会话级模型/思考配置
  const applySessionLlm = (
    model: string,
    thinkingMode: string,
    thinkingBudget: number,
    maxTokens = currentMaxTokens,
  ) => {
    if (!activeSessionId) return;
    setSessionLlm(activeSessionId, model, thinkingMode, thinkingBudget, maxTokens).catch(console.error);
  };

  const applyPermissionMode = (permissionMode: PermissionMode) => {
    if (!activeSessionId) return;
    setSessionPermissionMode(activeSessionId, permissionMode).catch(console.error);
  };

  // Keep user sends visible, while streaming follow remains an optional UI preference.
  useEffect(() => {
    const latestMessage = messages[messages.length - 1];
    if (!autoFollowStreaming && latestMessage?.role !== "user") return;
    messageEndRef.current?.scrollIntoView({ behavior: isStreaming ? "auto" : "smooth" });
  }, [messages, isStreaming, autoFollowStreaming]);

  const handleSend = async () => {
    if ((!inputVal.trim() && attachments.length === 0) || isStreaming || !activeSessionId) return;
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
      <div className="agnes-chat-stage relative min-h-0 flex-1 overflow-hidden">
      {/* Message Panel list */}
      <div
        className={`agnes-chat-messages absolute inset-0 mx-auto w-full max-w-4xl space-y-6 overflow-y-auto p-6 transition-[opacity,transform] duration-500 ease-out motion-reduce:transition-none ${
          isEmptyConversation
            ? "pointer-events-none translate-y-4 opacity-0"
            : "translate-y-0 opacity-100"
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
          return (
            <div
              key={message._renderKey ?? message.id}
              className={`agnes-message-row group flex gap-4 ${isUser ? "justify-end" : "justify-start"}`}
            >
              {!isUser && activeAgent && (
                <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={32} />
              )}

              <div className={`space-y-1.5 max-w-[85%] ${isUser ? "order-1" : "order-2"}`}>
                {isUser ? (
                  editingMsgId === message.id ? (
                    <div className="agnes-user-bubble rounded-2xl rounded-tr-sm bg-[#F1F5F0]/70 px-3 py-2 text-sm text-stone-900 border border-[#8CA38A] shadow-sm space-y-2">
                      <textarea
                        value={editingText}
                        onChange={(e) => setEditingText(e.target.value)}
                        autoFocus
                        className="w-full bg-white/70 rounded-lg p-2 text-sm focus:outline-none focus:ring-1 focus:ring-[#8CA38A] resize-none"
                        rows={3}
                      />
                      <div className="flex justify-end gap-1.5">
                        <button
                          onClick={() => { setEditingMsgId(null); setEditingText(""); }}
                          className="px-2.5 py-1 rounded-lg text-[11px] text-stone-500 hover:bg-stone-200/60"
                        >
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
                          className="px-2.5 py-1 rounded-lg text-[11px] font-semibold bg-[#8CA38A] text-white hover:bg-[#7A917A]"
                        >
                          重发
                        </button>
                      </div>
                    </div>
                  ) : (
                    <div className="agnes-user-bubble space-y-2 rounded-2xl rounded-tr-sm border border-[#DFE7DD] bg-[#F1F5F0]/70 px-4 py-2.5 text-sm text-stone-900 shadow-sm">
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
                            <RefreshCw className="h-3.5 w-3.5 shrink-0" />
                            <span>{part.content}</span>
                          </div>
                        );
                      }
                      // 1. Thought Process (reasoning)
                      if (part.kind === "thought") {
                        const isLiveThought = isLiveAssistant && message._streamingInThought === true;
                        return (
                          <ThoughtDetails
                            key={part._renderKey ?? part.id}
                            defaultOpen={autoExpandThoughts}
                            className="group border-l-2 border-[#8CA38A] bg-stone-100/60 rounded-r-xl p-3 transition-colors"
                          >
                            <summary className="flex items-center gap-2 cursor-pointer text-xs font-semibold text-[#6C806A] select-none hover:text-[#556654]">
                              <Cpu className="h-3.5 w-3.5" />
                              <span>Agent 思维过程 (Thought)</span>
                              {isLiveThought && (
                                <span className="ml-1 flex items-center gap-1 text-[10px] font-normal text-stone-400">
                                  <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-[#8CA38A]" />
                                  思考中
                                </span>
                              )}
                              <ChevronDown className="h-3 w-3 ml-auto group-open:rotate-180 transition-transform" />
                            </summary>
                            <p className="text-xs text-stone-600 mt-2 font-mono leading-relaxed pl-5 whitespace-pre-wrap border-t border-stone-200/40 pt-2">
                              {part.content}
                            </p>
                          </ThoughtDetails>
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
                        <Pencil className="h-3 w-3" />
                      </button>
                    )}
                    {!isUser && (
                      <button
                        onClick={() => regenerateMessage(message.id).catch(console.error)}
                        disabled={isStreaming}
                        className="rounded p-1 text-stone-400 hover:bg-stone-200/60 hover:text-stone-700 disabled:opacity-30"
                        title="单条重新生成"
                      >
                        <RefreshCw className="h-3 w-3" />
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
                      onClick={() => {
                        if (window.confirm("删除这条消息？")) {
                          deleteMessage(message.id).catch(console.error);
                        }
                      }}
                      disabled={!message.is_leaf || isStreaming || message.status === "pending" || message.status === "streaming"}
                      className="rounded p-1 text-stone-400 hover:bg-red-50 hover:text-red-600 disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-stone-400"
                      title={message.is_leaf ? "删除消息" : "仅可删除末梢消息"}
                    >
                      <Trash2 className="h-3 w-3" />
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
          <div className="flex gap-4 justify-start">
            {activeAgent && (
              <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={32} />
            )}
            <div className="bg-white border border-stone-200 px-4 py-2.5 rounded-2xl rounded-tl-sm flex items-center gap-1 shadow-sm">
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
        className={`agnes-chat-composer absolute inset-x-0 z-20 border-t border-stone-200 bg-[#FAF9F5]/40 p-4 transition-[bottom,transform,background-color] duration-500 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none ${
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
            <Sparkles className="h-7 w-7 text-[var(--claude-clay)]" weight="regular" />
            <h1 className="text-3xl font-normal text-[var(--claude-ink)]">
              {activeAgent ? `${activeAgent.name} returns!` : "Agnes returns!"}
            </h1>
          </div>
        </div>
        <div className={`agnes-chat-composer-box relative mx-auto max-w-4xl rounded-xl border border-stone-300/80 bg-white p-2.5 shadow-sm transition-[max-width,border-radius,box-shadow] duration-500 focus-within:border-stone-400 ${isEmptyConversation ? "agnes-chat-composer-box--empty" : ""}`}>
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
            value={inputVal}
            onChange={(e) => setInputVal(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
            placeholder={
              isEmptyConversation
                ? "输入一个问题、想法或任务…"
                : activeAgent
                ? `向 ${activeAgent.name} 发送消息... (Enter 发送)`
                : "选择一个会话以开始..."
            }
            className="w-full resize-none bg-transparent px-3 py-1 text-sm text-stone-900 placeholder:text-stone-450 focus:outline-none h-12"
          />
          {attachmentError && (
            <div className="mx-2 mb-2 rounded-md bg-rose-50 px-2 py-1.5 text-[10px] text-rose-700">
              {attachmentError}
            </div>
          )}
          <div className="flex items-center justify-between border-t border-stone-100 pt-2 px-1 text-[10px] text-stone-400">
            <div className="flex min-w-0 items-center gap-2">
              <div className="relative shrink-0">
                <button
                  type="button"
                  onClick={() => {
                    setAttachmentPicker((current) => current ? null : "menu");
                    setModelPickerOpen(false);
                    setPermissionPickerOpen(false);
                    setAttachmentError(null);
                  }}
                  disabled={!activeSessionId || isStreaming}
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
                            onClick={() => void addLocalFiles()}
                            className="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left text-stone-700 transition-colors hover:bg-stone-100"
                          >
                            <span className="flex h-8 w-8 items-center justify-center rounded-lg bg-[#8CA38A]/10 text-[#5F735D]">
                              <FileText className="h-4 w-4" />
                            </span>
                            <span className="min-w-0 flex-1">
                              <span className="block text-[12px] font-semibold">本地文件</span>
                              <span className="block text-[10px] text-stone-400">UTF-8 文本，单个最大 512 KiB</span>
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
                              <ChevronDown className="h-3.5 w-3.5 rotate-90" />
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
                              <ChevronDown className="h-3.5 w-3.5 rotate-90" />
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
              <span className="truncate">
                {currentPermissionMode === "full_access"
                  ? "完全访问已开启：文件与网络沙箱限制已放宽"
                  : attachments.length > 0
                    ? `${attachments.length} 个附件将随本条消息发送`
                    : "Agent 本地执行受系统沙箱安全策略保护"}
              </span>
            </div>
            <div className="flex items-center gap-2">
              {/* Session permission mode switcher */}
              <div className="relative">
                <button
                  onClick={() => {
                    setPermissionPickerOpen((open) => !open);
                    setModelPickerOpen(false);
                    setAttachmentPicker(null);
                  }}
                  disabled={!activeSessionId || isStreaming}
                  className={`flex items-center gap-1.5 rounded-lg border px-2.5 py-1 text-[10px] transition-colors disabled:opacity-40 ${
                    currentPermissionMode === "full_access"
                      ? "border-rose-200 bg-rose-50 text-rose-700 hover:bg-rose-100"
                      : "border-stone-200 bg-white text-stone-600 hover:text-stone-900 hover:bg-stone-50"
                  }`}
                  title={PERMISSION_OPTIONS.find((option) => option.value === currentPermissionMode)?.desc}
                >
                  <ShieldCheck className="h-3 w-3" />
                  <span>{PERMISSION_LABEL[currentPermissionMode]}</span>
                  <ChevronDown className="h-3 w-3" />
                </button>

                {permissionPickerOpen && (
                  <>
                    <div
                      className="fixed inset-0 z-40"
                      onClick={() => setPermissionPickerOpen(false)}
                    />
                    <div className="absolute bottom-full right-0 mb-2 z-50 w-72 rounded-xl border border-stone-200 bg-white shadow-2xl p-2">
                      <div className="px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-stone-400">
                        会话权限
                      </div>
                      {PERMISSION_OPTIONS.map((option) => {
                        const isActive = option.value === currentPermissionMode;
                        const isFullAccess = option.value === "full_access";
                        return (
                          <button
                            key={option.value}
                            type="button"
                            onClick={() => {
                              applyPermissionMode(option.value);
                              setPermissionPickerOpen(false);
                            }}
                            className={`w-full rounded-lg px-2.5 py-2 text-left transition-colors ${
                              isActive
                                ? isFullAccess
                                  ? "bg-rose-50 text-rose-700"
                                  : "bg-[#8CA38A]/10 text-[#5F735D]"
                                : isFullAccess
                                  ? "text-rose-600 hover:bg-rose-50"
                                  : "text-stone-600 hover:bg-stone-100"
                            }`}
                          >
                            <span className="flex items-center justify-between text-[11px] font-semibold">
                              {option.label}
                              {isActive && <Check className="h-3 w-3" />}
                            </span>
                            <span className="mt-0.5 block text-[10px] leading-relaxed opacity-70">
                              {option.desc}
                            </span>
                          </button>
                        );
                      })}
                    </div>
                  </>
                )}
              </div>

              {/* Model switcher (Provider -> Model) */}
              <div className="relative">
                <button
                  onClick={() => {
                    setModelPickerOpen((v) => !v);
                    setPermissionPickerOpen(false);
                    setAttachmentPicker(null);
                  }}
                  className="flex items-center gap-1.5 rounded-lg border border-stone-200 bg-white px-2.5 py-1 text-[10px] text-stone-600 hover:text-stone-900 hover:bg-stone-50 transition-colors"
                  title="切换模型"
                >
                  <Cpu className="h-3 w-3 text-[#8CA38A]" />
                  <span className="max-w-[160px] truncate font-mono">
                    {currentModel ? currentModel.name : "选择模型"}
                    {currentThinkingMode !== "off" && (
                      <span className="ml-1 text-[9px] text-violet-500 not-italic font-sans">
                        · {THINKING_LABEL[currentThinkingMode] ?? currentThinkingMode}思考
                      </span>
                    )}
                  </span>
                  <ChevronDown className="h-3 w-3" />
                </button>

                {modelPickerOpen && (
                  <>
                    {/* 点击空白关闭 */}
                    <div
                      className="fixed inset-0 z-40"
                      onClick={() => setModelPickerOpen(false)}
                    />
                    <div className="absolute bottom-full right-0 mb-2 z-50 w-72 max-h-80 overflow-y-auto rounded-xl border border-stone-200 bg-white shadow-2xl p-2">
                      {providers.length === 0 ? (
                        <div className="px-3 py-6 text-center text-[11px] text-stone-400">
                          暂无服务商，请到设置 → 模型与同步中添加
                        </div>
                      ) : (
                        providers.map((p) => (
                          <div key={p.id} className="mb-1">
                            <div className="flex items-center gap-1.5 px-2 py-1 text-[10px] font-semibold uppercase tracking-wide text-stone-400">
                              <Server className="h-3 w-3" />
                              <span className="truncate">{p.name}</span>
                              {p.is_default && (
                                <span className="text-[#6C806A]">默认</span>
                              )}
                            </div>
                            {p.models.length === 0 ? (
                              <div className="px-3 py-1 text-[10px] text-stone-300">
                                无可用模型
                              </div>
                            ) : (
                              p.models.map((m) => {
                                const val = `${p.id}/${m.id}`;
                                const isActive = effectiveModel === val;
                                return (
                                  <button
                                    key={val}
                                    onClick={() => {
                                      applySessionLlm(val, currentThinkingMode, 0);
                                      setModelPickerOpen(false);
                                    }}
                                    className={`w-full text-left px-3 py-1.5 rounded-lg text-[11px] font-mono transition-colors ${
                                      isActive
                                        ? "bg-[#8CA38A]/10 text-[#6C806A] font-semibold"
                                        : "text-stone-600 hover:bg-stone-100"
                                    }`}
                                  >
                                    {m.id}
                                    {isActive && (
                                      <Check className="h-3 w-3 inline ml-1" />
                                    )}
                                  </button>
                                );
                              })
                            )}
                          </div>
                        ))
                      )}
                      {/* 思考模式/强度（与模型选择并列） */}
                      <div className="mt-2 pt-2 border-t border-stone-100">
                        <div className="px-1 mb-1 text-[10px] font-semibold text-stone-500">
                          思考模式 / 强度
                        </div>
                        <div className="grid grid-cols-5 gap-1 px-1">
                          {THINKING_OPTIONS.map((opt) => (
                            <button
                              key={opt.value}
                              type="button"
                              title={opt.desc}
                              onClick={() => applySessionLlm(effectiveModel, opt.value, 0)}
                              className={`px-1 py-1 rounded-md text-[10px] font-semibold transition-colors border ${
                                currentThinkingMode === opt.value
                                  ? "bg-indigo-50 text-indigo-700 border-indigo-300"
                                  : "bg-stone-50 text-stone-500 border-stone-200/60 hover:bg-stone-100"
                              }`}
                            >
                              {opt.label}
                            </button>
                          ))}
                        </div>
                      </div>
                      <div className="mt-2 flex items-center gap-2 border-t border-stone-100 px-1 pt-2">
                        <label htmlFor="session-max-tokens" className="min-w-0 flex-1 text-[10px] font-semibold text-stone-500">
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
                          className="h-7 w-24 rounded-md border border-stone-200 bg-stone-50 px-2 text-right font-mono text-[10px] text-stone-700 outline-none focus:border-emerald-400"
                          aria-label="最大输出 Token"
                        />
                      </div>
                      <div className="mt-2 flex items-center gap-2 border-t border-stone-100 px-1 pt-2">
                        <label htmlFor="session-compress-threshold" className="min-w-0 flex-1 text-[10px] font-semibold text-stone-500">
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
                            if (value !== currentCompressThreshold && activeSessionId) {
                              useAgentStore.getState().setSessionCompressThreshold(activeSessionId, value).catch(console.error);
                            }
                          }}
                          className="h-7 w-20 rounded-md border border-stone-200 bg-stone-50 px-2 text-right font-mono text-[10px] text-stone-700 outline-none focus:border-emerald-400"
                          aria-label="自动总结阈值"
                        />
                        <span className="shrink-0 text-[9px] text-stone-400">
                          触发 {summaryTriggerTokens.toLocaleString()}
                        </span>
                      </div>
                    </div>
                  </>
                )}
              </div>

              {isStreaming ? (
                <button
                  onClick={() => activeSessionId && cancelRun(activeSessionId).catch(console.error)}
                  className="flex items-center gap-1 rounded-lg bg-red-600 hover:bg-red-700 text-white px-3.5 py-1 h-6 text-[10px] font-semibold shadow-sm transition-colors"
                  title="停止生成"
                >
                  <Square className="h-2.5 w-2.5 fill-current" />
                  <span>停止</span>
                </button>
              ) : (
                <Button
                  onClick={handleSend}
                  disabled={(!inputVal.trim() && attachments.length === 0) || !activeSessionId}
                  className="rounded-lg bg-stone-900 hover:bg-stone-850 text-white px-3.5 py-1 h-6 text-[10px] font-semibold shadow-sm"
                >
                  <Send className="h-3 w-3 mr-1" />
                  <span>发送</span>
                </Button>
              )}
            </div>
          </div>
        </div>
      </div>
      </div>

      <ModifyMemoryModal
        message={memoryEditMsgId ? messages.find((m) => m.id === memoryEditMsgId) ?? null : null}
        onClose={() => setMemoryEditMsgId(null)}
      />
    </main>
  );
};
