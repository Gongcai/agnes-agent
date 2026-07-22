import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type PermissionMode = "ask_for_approval" | "auto" | "accept_edits" | "full_access";
export type ModelModality = "text" | "image";

export interface ModelCapabilities {
  input_modalities: ModelModality[];
  output_modalities: ModelModality[];
  embedding: boolean;
}

export interface ModelDescriptor {
  id: string;
  capabilities: ModelCapabilities;
  context_window: number | null;
}

export interface ModelRoleAssignments {
  main_model: string | null;
  image_model: string | null;
  summary_model: string | null;
  memory_model: string | null;
  speech_model: string | null;
  quick_model: string | null;
  embedding_model: string | null;
  fallback_models: string[];
}

const EMPTY_MODEL_ROLES: ModelRoleAssignments = {
  main_model: null,
  image_model: null,
  summary_model: null,
  memory_model: null,
  speech_model: null,
  quick_model: null,
  embedding_model: null,
  fallback_models: [],
};

export interface ToolCall {
  id: string;
  tool: string;
  args: string;
  risk: string;
  status: "pending_approval" | "running" | "succeeded" | "denied" | "failed";
  output?: string;
  cwd?: string;
  networkAllowed?: boolean;
  landlock?: boolean;
  permissionMode?: PermissionMode;
  approvalReason?: string;
  isSecondaryConfirmation?: boolean;
  diff?: string;
}

interface ToolCallEventPayload {
  session_id: string;
  run_id: string;
  tool_call_id: string;
  tool: string;
  arguments: unknown;
  risk: string;
  cwd?: string;
  network_allowed: boolean;
  landlock: boolean;
  permission_mode: PermissionMode;
  approval_reason: string;
  is_secondary_confirmation: boolean;
  diff?: string;
  status: ToolCall["status"];
}

export interface MessagePart {
  id: string;
  kind: "text" | "thought" | "tool_call" | "tool_result" | "model_fallback" | "attachment";
  content: string;
  mime_type?: string | null;
  metadata?: ChatAttachmentMetadata | null;
  tool_call?: ToolCall;
  /** Stable React key retained when a live temporary part is replaced by its DB row. */
  _renderKey?: string;
}

export type ChatAttachment =
  | {
      id: string;
      kind: "local_file";
      name: string;
      path: string;
      mediaType?: string;
    }
  | {
      id: string;
      kind: "knowledge_collection";
      name: string;
      collectionId: string;
    }
  | {
      id: string;
      kind: "skill";
      name: string;
      skillId: string;
    };

export interface ChatAttachmentMetadata {
  attachmentKind: ChatAttachment["kind"];
  id: string;
  name: string;
  path?: string;
  collectionId?: string;
  skillId?: string;
  mediaType?: string;
  size?: number;
}

export interface Message {
  id: string;
  session_id: string;
  role: "user" | "assistant";
  seq: number;
  status: string;
  parts: MessagePart[];
  created_at: string;
  parent_id: string | null;
  version_index: number;
  version_count: number;
  is_leaf: boolean;
  input_tokens: number;
  cached_tokens: number;
  output_tokens: number;
  context_tokens: number;
  /** 流式暂存：当前是否处于 <thought> 思维链中。仅用于直播渲染，不落库。 */
  _streamingInThought?: boolean;
  /** Stable React key retained across the run-finished DB reconciliation. */
  _renderKey?: string;
}

function pendingAssistantVersion(source: Message, id: string, parentId: string): Message {
  return {
    id,
    session_id: source.session_id,
    role: "assistant",
    seq: source.seq + 1,
    status: "pending",
    parts: [],
    created_at: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
    parent_id: parentId,
    version_index: source.version_count,
    version_count: source.version_count + 1,
    is_leaf: true,
    input_tokens: 0,
    cached_tokens: 0,
    output_tokens: 0,
    context_tokens: 0,
  };
}

function latestPendingAssistant(messages: Message[], sessionId: string): Message | undefined {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      message.session_id === sessionId
      && message.role === "assistant"
      && (message.status === "pending" || message.status === "streaming")
    ) {
      return message;
    }
  }
  return undefined;
}

function mergeLiveAssistantState(previous: Message, next: Message): Message {
  const parts = [...next.parts];
  for (const previousPart of previous.parts) {
    if (previousPart.kind === "tool_result") continue;
    const matchIndex = parts.findIndex((part) => (
      part.kind === previousPart.kind
      && (
        part.kind !== "tool_call"
        || part.tool_call?.id === previousPart.tool_call?.id
      )
    ));
    if (matchIndex < 0) {
      parts.push(previousPart);
      continue;
    }
    const part = parts[matchIndex];
    parts[matchIndex] = {
      ...part,
      _renderKey: previousPart._renderKey ?? previousPart.id,
      ...(part.kind === "tool_call" && previousPart.tool_call
        ? { tool_call: { ...part.tool_call, ...previousPart.tool_call } }
        : {}),
    };
  }
  return {
    ...next,
    status: previous.status === "streaming" ? "streaming" : next.status,
    parts,
    _streamingInThought: previous._streamingInThought,
    _renderKey: previous._renderKey ?? previous.id,
  };
}

export function buildOptimisticEditBranch(
  messages: Message[],
  messageId: string,
  text: string,
  nonce = Date.now(),
): Message[] | null {
  const sourceIndex = messages.findIndex((message) => message.id === messageId);
  if (sourceIndex < 0) return null;
  const source = messages[sourceIndex];
  if (source.role !== "user") return null;

  const userId = `temp_edit_user_${nonce}`;
  const editedUser: Message = {
    ...source,
    id: userId,
    status: "complete",
    parts: [
      {
        id: `temp_edit_text_${nonce}`,
        kind: "text",
        content: text,
      },
      ...source.parts.filter((part) => part.kind === "attachment"),
    ],
    created_at: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
    version_index: source.version_count,
    version_count: source.version_count + 1,
    is_leaf: false,
    _renderKey: userId,
  };
  const pendingAssistant = pendingAssistantVersion(
    { ...source, version_count: 0 },
    `temp_edit_assistant_${nonce}`,
    userId,
  );

  return [...messages.slice(0, sourceIndex), editedUser, pendingAssistant];
}

export function buildOptimisticRegenerationBranch(
  messages: Message[],
  messageId: string,
  nonce = Date.now(),
): Message[] | null {
  const sourceIndex = messages.findIndex((message) => message.id === messageId);
  if (sourceIndex < 0) return null;
  const source = messages[sourceIndex];
  if (source.role !== "assistant" || !source.parent_id) return null;

  const pendingAssistant = pendingAssistantVersion(
    source,
    `temp_regenerate_assistant_${nonce}`,
    source.parent_id,
  );
  return [...messages.slice(0, sourceIndex), pendingAssistant];
}

export function preserveLatestRunRenderKeys(
  previousMessages: Message[],
  nextMessages: Message[],
  sessionId: string,
): Message[] {
  const previous = latestPendingAssistant(previousMessages, sessionId);
  if (!previous) return nextMessages;

  let nextIndex = -1;
  for (let index = nextMessages.length - 1; index >= 0; index -= 1) {
    const message = nextMessages[index];
    if (message.session_id === sessionId && message.role === "assistant") {
      nextIndex = index;
      break;
    }
  }
  if (nextIndex < 0) return nextMessages;

  const reconciled = [...nextMessages];
  reconciled[nextIndex] = mergeLiveAssistantState(previous, nextMessages[nextIndex]);
  return reconciled;
}

export interface Session {
  id: string;
  agent_id: string;
  title: string;
  summary?: string;
  created_at: string;
  updated_at: string;
  pinned?: boolean;
  model: string;
  thinking_mode: string;
  thinking_budget: number;
  max_tokens: number;
  context_limit: number | null;
  compress_threshold: number;
  permission_mode: PermissionMode;
  workspace_id: string | null;
}

export interface Workspace {
  id: string;
  agent_id: string;
  name: string;
  folder_path: string;
  created_at: string;
  updated_at: string;
}

export interface AgentSummary {
  id: string;
  name: string;
  persona: string;
  scenario: string;
  system_prompt: string;
  greeting: string;
  example_dialogue: string;
  model: string;
  tool_policy: string;
  avatar: string;
  tags: string;
  thinking_mode: string;
  thinking_budget: number;
}

export interface AgentConfig {
  id: string;
  name: string;
  avatarColor: string;
  avatarTextColor: string;
  tags: string[];
  description: string;
  model: string;
  persona: string;
  systemPrompt: string;
  greeting: string;
  toolPolicy: any;
}

export interface ModelProvider {
  id: string;
  name: string;
  kind: string;
  api_base: string | null;
  is_default: boolean;
  models: ModelDescriptor[];
  has_api_key: boolean;
  created_at: string;
  updated_at: string;
}

interface AgentState {
  agents: AgentSummary[];
  sessions: Session[];
  messages: Message[];
  workspaces: Workspace[];
  activeAgentId: string | null;
  activeSessionId: string | null;
  isStreaming: boolean;
  providers: ModelProvider[];
  modelRoles: ModelRoleAssignments;
  
  // Actions
  init: (persistNavigation?: boolean) => Promise<void>;
  loadAgents: () => Promise<void>;
  loadSessions: (agentId: string) => Promise<void>;
  loadMessages: (sessionId: string, preserveRenderKeys?: boolean) => Promise<void>;
  createSession: (agentId: string, title: string, workspaceId?: string | null) => Promise<string>;
  deleteSession: (sessionId: string) => Promise<void>;
  pinSession: (sessionId: string, pinned: boolean) => Promise<void>;
  renameSession: (sessionId: string, title: string) => Promise<void>;
  loadWorkspaces: (agentId: string) => Promise<void>;
  createWorkspace: (agentId: string, name: string, folderPath: string) => Promise<string>;
  renameWorkspace: (workspaceId: string, name: string) => Promise<void>;
  deleteWorkspace: (workspaceId: string) => Promise<void>;
  sendMessage: (
    sessionId: string,
    text: string,
    readingBookId?: string,
    attachments?: ChatAttachment[],
  ) => Promise<void>;
  cancelRun: (sessionId: string) => Promise<void>;
  approveTool: (toolCallId: string, approved: boolean) => Promise<void>;
  setActiveAgentId: (agentId: string) => Promise<void>;
  setActiveSessionId: (sessionId: string, persist?: boolean) => Promise<void>;
  setSessionLlm: (sessionId: string, model: string, thinkingMode: string, thinkingBudget: number, maxTokens: number) => Promise<void>;
  setSessionCompressThreshold: (sessionId: string, compressThreshold: number) => Promise<void>;
  setSessionPermissionMode: (sessionId: string, permissionMode: PermissionMode) => Promise<void>;
  switchVersion: (messageId: string, direction: "prev" | "next") => Promise<void>;
  createBranch: (messageId: string) => Promise<void>;
  deleteMessage: (messageId: string) => Promise<void>;
  editAndResend: (messageId: string, text: string) => Promise<void>;
  regenerateMessage: (messageId: string) => Promise<void>;
  replaceMessageParts: (
    messageId: string,
    parts: { kind: string; content: string; tool_call_id?: string; metadata?: string }[],
  ) => Promise<void>;
  updateAgentModel: (agentId: string, model: string) => Promise<void>;
  upsertAgent: (agent: {
    id?: string;
    name: string;
    persona: string;
    scenario: string;
    system_prompt: string;
    greeting: string;
    example_dialogue: string;
    model: string;
    tool_policy: string;
    avatar: string;
    tags: string;
    thinking_mode: string;
    thinking_budget: number;
  }) => Promise<string>;
  deleteAgent: (agentId: string) => Promise<void>;
  
  // Provider actions
  loadProviders: () => Promise<void>;
  loadModelRoles: () => Promise<void>;
  setModelRoles: (roles: ModelRoleAssignments) => Promise<void>;
  upsertProvider: (provider: {
    id?: string;
    name: string;
    kind: string;
    api_base?: string;
    api_key?: string;
    is_default?: boolean;
    models?: ModelDescriptor[];
  }) => Promise<string>;
  deleteProvider: (providerId: string) => Promise<void>;

  // Local Mutations (typically called by Tauri event listeners)
  appendStreamingDelta: (content: string) => void;
  addModelFallbackNotice: (sessionId: string, content: string) => void;
  upsertLocalToolCall: (sessionId: string, toolCall: ToolCall) => void;
  updateLocalToolCallStatus: (toolCallId: string, status: ToolCall["status"], output?: string) => void;
  setStreamingState: (isStreaming: boolean) => void;
}

export const useAgentStore = create<AgentState>((set, get) => ({
  agents: [],
  sessions: [],
  messages: [],
  workspaces: [],
  activeAgentId: null,
  activeSessionId: null,
  isStreaming: false,
  providers: [],
  modelRoles: EMPTY_MODEL_ROLES,

  loadAgents: async () => {
    try {
      const agents = await invoke<AgentSummary[]>("list_agents");
      set({ agents });
      if (agents.length > 0 && !get().activeAgentId) {
        await get().setActiveAgentId(agents[0].id);
      }
    } catch (e) {
      console.error("Failed to load agents", e);
    }
  },

  // 应用启动初始化：读取持久化的上次 agent/session 与打开模式，按模式恢复或新建。
  init: async (persistNavigation = true) => {
    try {
      const agents = await invoke<AgentSummary[]>("list_agents");
      set({ agents });
      if (agents.length === 0) return;
      await get().loadModelRoles();

      const lastAgentId = await invoke<string | null>("get_setting", { key: "ui:last_agent_id" });
      const lastSessionId = await invoke<string | null>("get_setting", { key: "ui:last_session_id" });
      const openMode = (await invoke<string | null>("get_setting", { key: "ui:session_open_mode" })) ?? "last";

      const agentId = lastAgentId && agents.some((a) => a.id === lastAgentId) ? lastAgentId : agents[0].id;
      set({ activeAgentId: agentId });
      if (persistNavigation) {
        invoke("set_setting", { key: "ui:last_agent_id", value: agentId }).catch(() => {});
      }

      const sessions = await invoke<Session[]>("list_sessions", { agentId });
      set({ sessions });
      await get().loadWorkspaces(agentId);

      if (openMode === "new") {
        // 打开时自动新建会话
        const sid = await invoke<string>("create_session", { agentId, title: "新会话" });
        set({ activeSessionId: sid, messages: [] });
        if (persistNavigation) {
          invoke("set_setting", { key: "ui:last_session_id", value: sid }).catch(() => {});
        }
      } else {
        // 回到上次对话：优先上次会话，回退到首条，再回退到新建
        let sid: string | null = lastSessionId && sessions.some((s) => s.id === lastSessionId) ? lastSessionId : null;
        if (!sid && sessions.length > 0) sid = sessions[0].id;
        if (sid) {
          set({ activeSessionId: sid });
          await get().loadMessages(sid);
          if (persistNavigation) {
            invoke("set_setting", { key: "ui:last_session_id", value: sid }).catch(() => {});
          }
        } else {
          const newSid = await invoke<string>("create_session", { agentId, title: "新会话" });
          set({ activeSessionId: newSid, messages: [] });
          if (persistNavigation) {
            invoke("set_setting", { key: "ui:last_session_id", value: newSid }).catch(() => {});
          }
        }
      }
    } catch (e) {
      console.error("init failed", e);
    }
  },

  loadSessions: async (agentId: string) => {
    try {
      const sessions = await invoke<Session[]>("list_sessions", { agentId });
      set({ sessions });
    } catch (e) {
      console.error("Failed to load sessions", e);
    }
  },

  loadMessages: async (sessionId: string, preserveRenderKeys = false) => {
    try {
      const messages = await invoke<Message[]>("list_messages", { sessionId });
      set({
        messages: preserveRenderKeys
          ? preserveLatestRunRenderKeys(get().messages, messages, sessionId)
          : messages,
      });
    } catch (e) {
      console.error("Failed to load messages", e);
    }
  },

  createSession: async (agentId: string, title: string, workspaceId?: string | null) => {
    try {
      const sessionId = await invoke<string>("create_session", { agentId, title, workspaceId: workspaceId ?? null });
      await get().loadSessions(agentId);
      await get().setActiveSessionId(sessionId);
      return sessionId;
    } catch (e) {
      console.error("Failed to create session", e);
      throw e;
    }
  },

  deleteSession: async (sessionId: string) => {
    try {
      await invoke("delete_session", { sessionId });
      const { activeAgentId, activeSessionId } = get();
      if (activeAgentId) {
        await get().loadSessions(activeAgentId);
        // If the active session was deleted, switch to another one
        if (activeSessionId === sessionId) {
          const sessions = get().sessions;
          if (sessions.length > 0) {
            await get().setActiveSessionId(sessions[0].id);
          } else {
            set({ activeSessionId: null, messages: [] });
          }
        }
      }
    } catch (e) {
      console.error("Failed to delete session", e);
    }
  },

  pinSession: async (sessionId, pinned) => {
    try {
      await invoke("set_session_pin", { sessionId, pinned });
      const { activeAgentId } = get();
      if (activeAgentId) await get().loadSessions(activeAgentId);
    } catch (e) {
      console.error("Failed to pin session", e);
    }
  },

  renameSession: async (sessionId, title) => {
    try {
      await invoke("rename_session", { sessionId, title });
      const { activeAgentId } = get();
      if (activeAgentId) await get().loadSessions(activeAgentId);
    } catch (e) {
      console.error("Failed to rename session", e);
    }
  },

  loadWorkspaces: async (agentId: string) => {
    try {
      const workspaces = await invoke<Workspace[]>("list_workspaces", { agentId });
      set({ workspaces });
    } catch (e) {
      console.error("Failed to load workspaces", e);
    }
  },

  createWorkspace: async (agentId: string, name: string, folderPath: string) => {
    const id = await invoke<string>("create_workspace", { agentId, name, folderPath });
    await get().loadWorkspaces(agentId);
    return id;
  },

  renameWorkspace: async (workspaceId: string, name: string) => {
    try {
      await invoke("rename_workspace", { workspaceId, name });
      const { activeAgentId } = get();
      if (activeAgentId) await get().loadWorkspaces(activeAgentId);
    } catch (e) {
      console.error("Failed to rename workspace", e);
    }
  },

  deleteWorkspace: async (workspaceId: string) => {
    try {
      await invoke("delete_workspace", { workspaceId });
      const { activeAgentId, activeSessionId } = get();
      if (activeAgentId) {
        await get().loadWorkspaces(activeAgentId);
        await get().loadSessions(activeAgentId);
        // 若当前活动会话属于被删工作区，切到首个普通对话
        const sessions = get().sessions;
        if (sessions.find((s) => s.id === activeSessionId)?.workspace_id === workspaceId) {
          const fallback = sessions.find((s) => !s.workspace_id) ?? sessions[0];
          if (fallback) await get().setActiveSessionId(fallback.id);
          else set({ activeSessionId: null, messages: [] });
        }
      }
    } catch (e) {
      console.error("Failed to delete workspace", e);
    }
  },

  sendMessage: async (
    sessionId: string,
    text: string,
    readingBookId?: string,
    attachments: ChatAttachment[] = [],
  ) => {
    if (get().isStreaming) return;
    
    // 1. Instantly append a local user message and a pending assistant message for responsive UI
    const tempUserMsg: Message = {
      id: `temp_user_${Date.now()}`,
      session_id: sessionId,
      role: "user",
      seq: get().messages.length,
      status: "complete",
      parts: [
        { id: `p_u_${Date.now()}`, kind: "text", content: text },
        ...attachments.map((attachment, index): MessagePart => ({
          id: `p_a_${Date.now()}_${index}`,
          kind: "attachment",
          content: "",
          metadata: {
            attachmentKind: attachment.kind,
            id: attachment.id,
            name: attachment.name,
            ...(attachment.kind === "local_file"
              ? { path: attachment.path, mediaType: attachment.mediaType }
              : attachment.kind === "knowledge_collection"
                ? { collectionId: attachment.collectionId }
                : { skillId: attachment.skillId }),
          },
        })),
      ],
      created_at: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
      parent_id: get().messages.length > 0 ? get().messages[get().messages.length - 1].id : null,
      version_index: 0,
      version_count: 1,
      is_leaf: false,
      input_tokens: 0,
      cached_tokens: 0,
      output_tokens: 0,
      context_tokens: 0,
    };

    const tempAssistantMsg: Message = {
      id: `temp_assistant_${Date.now()}`,
      session_id: sessionId,
      role: "assistant",
      seq: get().messages.length + 1,
      status: "pending",
      parts: [],
      created_at: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
      parent_id: tempUserMsg.id,
      version_index: 0,
      version_count: 1,
      is_leaf: true,
      input_tokens: 0,
      cached_tokens: 0,
      output_tokens: 0,
      context_tokens: 0,
    };

    set({
      isStreaming: true,
      messages: [...get().messages, tempUserMsg, tempAssistantMsg],
    });

    try {
      await invoke("send_message", {
        sessionId,
        text,
        readingBookId: readingBookId ?? null,
        attachments,
      });
    } catch (e) {
      console.error("Failed to send message", e);
      // The backend may have persisted a failed assistant message with its
      // diagnostic text. Reload it instead of leaving a blank placeholder.
      set({
        isStreaming: false,
        messages: get().messages.filter(m => m.id !== tempAssistantMsg.id),
      });
      if (get().activeSessionId === sessionId) {
        await get().loadMessages(sessionId, true);
      }
      throw e;
    }
  },

  cancelRun: async (sessionId: string) => {
    // 乐观置为非生成中，再发取消；run_finished/run_error 也会兜底重载
    set({ isStreaming: false });
    try {
      await invoke("cancel_run", { sessionId });
      if (get().activeSessionId) {
        await get().loadMessages(get().activeSessionId!);
      }
    } catch (e) {
      console.error("取消运行失败", e);
    }
  },

  approveTool: async (toolCallId: string, approved: boolean) => {
    try {
      await invoke("approve_tool", { toolCallId, approved });
    } catch (e) {
      console.error("Failed to approve tool", e);
    }
  },

  setActiveAgentId: async (agentId: string) => {
    set({ activeAgentId: agentId });
    await get().loadSessions(agentId);
    await get().loadWorkspaces(agentId);
    // 持久化上次选中的智能体
    invoke("set_setting", { key: "ui:last_agent_id", value: agentId }).catch(() => {});

    const sessions = get().sessions;
    if (sessions.length > 0) {
      await get().setActiveSessionId(sessions[0].id);
    } else {
      set({ activeSessionId: null, messages: [] });
    }
  },

  setActiveSessionId: async (sessionId: string, persist = true) => {
    set({ activeSessionId: sessionId });
    await get().loadMessages(sessionId);
    if (persist) {
      invoke("set_setting", { key: "ui:last_session_id", value: sessionId }).catch(() => {});
    }
  },

  setSessionLlm: async (sessionId: string, model: string, thinkingMode: string, thinkingBudget: number, maxTokens: number) => {
    try {
      await invoke("set_session_llm", { sessionId, model, thinkingMode, thinkingBudget, maxTokens });
      // 立即更新本地会话状态，避免等待列表刷新
      const sessions = get().sessions.map((s) =>
        s.id === sessionId
          ? { ...s, model, thinking_mode: thinkingMode, thinking_budget: thinkingBudget, max_tokens: maxTokens }
          : s
      );
      set({ sessions });
    } catch (e) {
      console.error("设置会话模型/思考失败", e);
      throw e;
    }
  },

  setSessionCompressThreshold: async (sessionId, compressThreshold) => {
    await invoke("set_session_compress_threshold", { sessionId, compressThreshold });
    set({
      sessions: get().sessions.map((session) =>
        session.id === sessionId
          ? { ...session, compress_threshold: compressThreshold }
          : session
      ),
    });
  },

  setSessionPermissionMode: async (sessionId: string, permissionMode: PermissionMode) => {
    try {
      await invoke("set_session_permission_mode", { sessionId, permissionMode });
      const sessions = get().sessions.map((session) =>
        session.id === sessionId ? { ...session, permission_mode: permissionMode } : session
      );
      set({ sessions });
    } catch (e) {
      console.error("设置会话权限模式失败", e);
      throw e;
    }
  },

  switchVersion: async (messageId: string, direction: "prev" | "next") => {
    try {
      await invoke("switch_version", { messageId, direction });
      const { activeSessionId } = get();
      if (activeSessionId) await get().loadMessages(activeSessionId);
    } catch (e) {
      console.error("切换版本失败", e);
      throw e;
    }
  },

  createBranch: async (messageId: string) => {
    try {
      await invoke("create_branch", { messageId });
      const { activeSessionId } = get();
      if (activeSessionId) await get().loadMessages(activeSessionId);
    } catch (e) {
      console.error("创建分支失败", e);
      throw e;
    }
  },

  deleteMessage: async (messageId: string) => {
    try {
      await invoke("delete_message", { messageId });
      const { activeSessionId } = get();
      if (activeSessionId) await get().loadMessages(activeSessionId);
    } catch (e) {
      console.error("删除消息失败", e);
      throw e;
    }
  },

  editAndResend: async (messageId: string, text: string) => {
    const source = get().messages.find((message) => message.id === messageId);
    const sessionId = source?.session_id ?? get().activeSessionId;
    const optimisticMessages = buildOptimisticEditBranch(get().messages, messageId, text);
    set({
      isStreaming: true,
      ...(optimisticMessages ? { messages: optimisticMessages } : {}),
    });
    try {
      await invoke("edit_and_resend", { messageId, text });
      if (sessionId && get().activeSessionId === sessionId) {
        await get().loadMessages(sessionId, true);
      }
    } catch (e) {
      set({ isStreaming: false });
      if (sessionId && get().activeSessionId === sessionId) {
        await get().loadMessages(sessionId, true);
      }
      console.error("编辑并重发失败", e);
      throw e;
    }
  },

  regenerateMessage: async (messageId: string) => {
    const source = get().messages.find((message) => message.id === messageId);
    const sessionId = source?.session_id ?? get().activeSessionId;
    const optimisticMessages = buildOptimisticRegenerationBranch(get().messages, messageId);
    set({
      isStreaming: true,
      ...(optimisticMessages ? { messages: optimisticMessages } : {}),
    });
    try {
      await invoke("regenerate_message", { messageId });
      if (sessionId && get().activeSessionId === sessionId) {
        await get().loadMessages(sessionId, true);
      }
    } catch (e) {
      set({ isStreaming: false });
      if (sessionId && get().activeSessionId === sessionId) {
        await get().loadMessages(sessionId, true);
      }
      console.error("重新生成失败", e);
      throw e;
    }
  },

  replaceMessageParts: async (messageId, parts) => {
    try {
      await invoke("replace_message_parts", { messageId, parts });
      const { activeSessionId } = get();
      if (activeSessionId) await get().loadMessages(activeSessionId);
    } catch (e) {
      console.error("修改记忆失败", e);
      throw e;
    }
  },

  updateAgentModel: async (agentId: string, model: string) => {
    try {
      await invoke("update_agent_model", { agentId, model });
      await get().loadAgents(); // Reload agents list to reflect the changes
    } catch (e) {
      console.error("Failed to update agent model", e);
      throw e;
    }
  },

  upsertAgent: async (agent) => {
    try {
      const id = await invoke<string>("upsert_agent", { payload: agent });
      await get().loadAgents();
      return id;
    } catch (e) {
      console.error("Failed to upsert agent", e);
      throw e;
    }
  },

  deleteAgent: async (agentId: string) => {
    try {
      await invoke("delete_agent", { agentId });
      const { activeAgentId } = get();
      await get().loadAgents();
      // 若删除的是当前角色卡，切换到其余首个或清空
      if (activeAgentId === agentId) {
        const agents = get().agents;
        if (agents.length > 0) {
          await get().setActiveAgentId(agents[0].id);
        } else {
          set({ activeAgentId: null, activeSessionId: null, sessions: [], messages: [] });
        }
      }
    } catch (e) {
      console.error("Failed to delete agent", e);
      throw e;
    }
  },

  loadProviders: async () => {
    try {
      const providers = await invoke<ModelProvider[]>("list_providers");
      set({ providers });
    } catch (e) {
      console.error("Failed to load providers", e);
    }
  },

  loadModelRoles: async () => {
    try {
      const modelRoles = await invoke<ModelRoleAssignments>("get_model_roles");
      set({ modelRoles });
    } catch (e) {
      console.error("Failed to load model roles", e);
    }
  },

  setModelRoles: async (roles) => {
    try {
      await invoke("set_model_roles", { roles });
      set({ modelRoles: roles });
    } catch (e) {
      console.error("Failed to save model roles", e);
      throw e;
    }
  },

  upsertProvider: async (provider) => {
    try {
      const id = await invoke<string>("upsert_provider", { provider });
      await get().loadProviders();
      await get().loadModelRoles();
      return id;
    } catch (e) {
      console.error("Failed to upsert provider", e);
      throw e;
    }
  },

  deleteProvider: async (providerId) => {
    try {
      await invoke("delete_provider", { providerId });
      await get().loadProviders();
      await get().loadModelRoles();
    } catch (e) {
      console.error("Failed to delete provider", e);
    }
  },

  appendStreamingDelta: (content: string) => {
    const { messages } = get();
    if (messages.length === 0) return;

    const updatedMessages = [...messages];
    const lastMsg = { ...updatedMessages[updatedMessages.length - 1] };
    // 仅追加到处于 pending/streaming 的 assistant 消息（避免重生成/编辑重发后
    // 首个 delta 在 loadMessages 完成前误落到旧 complete 消息上）
    if (lastMsg.role !== "assistant" || (lastMsg.status !== "pending" && lastMsg.status !== "streaming")) return;
    lastMsg.parts = [...lastMsg.parts];

    let partSeq = lastMsg.parts.length;
    const appendKind = (kind: "text" | "thought", text: string) => {
      if (!text) return;
      const last = lastMsg.parts[lastMsg.parts.length - 1];
      if (last && last.kind === kind) {
        lastMsg.parts[lastMsg.parts.length - 1] = { ...last, content: last.content + text };
      } else {
        lastMsg.parts.push({ id: `p_a_${Date.now()}_${partSeq++}`, kind, content: text });
      }
    };

    // 当前是否处于思维链中：用跨调用持久化的标志，避免 <thought> 独立 chunk 丢失状态
    let inThought = lastMsg._streamingInThought === true;

    // 按 <thought>/</thought> 标签分段路由（标签可能跨 chunk 到达）
    let remaining = content;
    while (remaining.length > 0) {
      if (inThought) {
        const closeIdx = remaining.indexOf("</thought>");
        if (closeIdx >= 0) {
          appendKind("thought", remaining.slice(0, closeIdx));
          inThought = false;
          remaining = remaining.slice(closeIdx + "</thought>".length);
        } else {
          appendKind("thought", remaining);
          remaining = "";
        }
      } else {
        const openIdx = remaining.indexOf("<thought>");
        if (openIdx >= 0) {
          appendKind("text", remaining.slice(0, openIdx));
          inThought = true;
          remaining = remaining.slice(openIdx + "<thought>".length);
        } else {
          appendKind("text", remaining);
          remaining = "";
        }
      }
    }

    lastMsg._streamingInThought = inThought;
    lastMsg.status = "streaming";
    updatedMessages[updatedMessages.length - 1] = lastMsg;
    set({ messages: updatedMessages });
  },

  addModelFallbackNotice: (sessionId: string, content: string) => {
    const { messages } = get();
    let messageIndex = -1;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (
        message.session_id === sessionId
        && message.role === "assistant"
        && (message.status === "pending" || message.status === "streaming")
      ) {
        messageIndex = index;
        break;
      }
    }
    if (messageIndex < 0) return;
    const updatedMessages = [...messages];
    const message = { ...updatedMessages[messageIndex], status: "streaming" };
    message.parts = [
      ...message.parts,
      {
        id: `p_fallback_${Date.now()}_${message.parts.length}`,
        kind: "model_fallback",
        content,
      },
    ];
    updatedMessages[messageIndex] = message;
    set({ messages: updatedMessages });
  },

  upsertLocalToolCall: (sessionId: string, toolCall: ToolCall) => {
    const { activeSessionId, messages } = get();
    if (activeSessionId !== sessionId || messages.length === 0) return;

    let messageIndex = -1;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (
        message.session_id === sessionId &&
        message.role === "assistant" &&
        (message.status === "pending" || message.status === "streaming")
      ) {
        messageIndex = index;
        break;
      }
    }
    if (messageIndex < 0) return;

    const updatedMessages = [...messages];
    const message = { ...updatedMessages[messageIndex], status: "streaming" };
    const parts = [...message.parts];
    const partIndex = parts.findIndex(
      (part) => part.kind === "tool_call" && part.tool_call?.id === toolCall.id,
    );

    if (partIndex >= 0) {
      const existing = parts[partIndex];
      parts[partIndex] = {
        ...existing,
        tool_call: { ...existing.tool_call!, ...toolCall },
      };
    } else {
      parts.push({
        id: `p_tc_${toolCall.id}`,
        kind: "tool_call",
        content: `Calling ${toolCall.tool}`,
        tool_call: toolCall,
      });
    }

    message.parts = parts;
    updatedMessages[messageIndex] = message;
    set({ messages: updatedMessages });
  },

  updateLocalToolCallStatus: (toolCallId: string, status: ToolCall["status"], output?: string) => {
    const { messages } = get();
    if (messages.length === 0) return;

    const updatedMessages = [...messages];
    for (let messageIndex = updatedMessages.length - 1; messageIndex >= 0; messageIndex -= 1) {
      const message = updatedMessages[messageIndex];
      const partIndex = message.parts.findIndex(
        (part) => part.kind === "tool_call" && part.tool_call?.id === toolCallId,
      );
      if (partIndex < 0) continue;

      const parts = [...message.parts];
      const part = parts[partIndex];
      parts[partIndex] = {
        ...part,
        tool_call: {
          ...part.tool_call!,
          status,
          output: output !== undefined ? output : part.tool_call?.output,
        },
      };
      updatedMessages[messageIndex] = { ...message, parts };
      set({ messages: updatedMessages });
      return;
    }
  },

  setStreamingState: (isStreaming: boolean) => {
    set({ isStreaming });
  }
}));

// Setup bridge listener to bind Tauri backend events to Zustand actions
export function setupTauriEventListeners() {
  const listeners: Promise<() => void>[] = [];
  let bufferedDelta: { sessionId: string; runId: string; content: string } | null = null;
  let deltaFlushTimer: ReturnType<typeof setTimeout> | null = null;

  const flushBufferedDelta = () => {
    if (deltaFlushTimer !== null) {
      clearTimeout(deltaFlushTimer);
      deltaFlushTimer = null;
    }
    const delta = bufferedDelta;
    bufferedDelta = null;
    if (!delta) return;

    const store = useAgentStore.getState();
    if (store.activeSessionId === delta.sessionId) {
      store.appendStreamingDelta(delta.content);
    }
  };

  const queueStreamingDelta = (sessionId: string, runId: string, content: string) => {
    if (!content || useAgentStore.getState().activeSessionId !== sessionId) return;
    if (bufferedDelta && (bufferedDelta.sessionId !== sessionId || bufferedDelta.runId !== runId)) {
      flushBufferedDelta();
    }

    if (bufferedDelta) {
      bufferedDelta.content += content;
    } else {
      bufferedDelta = { sessionId, runId, content };
    }

    if (deltaFlushTimer === null) {
      deltaFlushTimer = setTimeout(flushBufferedDelta, 50);
    }
  };

  const showToolCall = (payload: ToolCallEventPayload, fallbackStatus: ToolCall["status"]) => {
    // Preserve the backend event order when text deltas are waiting in the UI batch.
    flushBufferedDelta();
    const args = typeof payload.arguments === "string"
      ? payload.arguments
      : JSON.stringify(payload.arguments ?? {});
    useAgentStore.getState().upsertLocalToolCall(payload.session_id, {
      id: payload.tool_call_id,
      tool: payload.tool,
      args,
      risk: payload.risk || (payload.tool === "shell" ? "Medium" : "High"),
      cwd: payload.cwd,
      networkAllowed: payload.network_allowed,
      landlock: payload.landlock,
      permissionMode: payload.permission_mode,
      approvalReason: payload.approval_reason,
      isSecondaryConfirmation: payload.is_secondary_confirmation,
      diff: payload.diff,
      status: payload.status || fallbackStatus,
    });
  };

  listeners.push(
    listen<{ session_id: string; run_id: string; content: string }>(
      "agent://assistant_delta",
      (event) => {
        queueStreamingDelta(
          event.payload.session_id,
          event.payload.run_id,
          event.payload.content,
        );
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string; content: string }>(
      "agent://model_fallback",
      (event) => {
        flushBufferedDelta();
        const store = useAgentStore.getState();
        if (store.activeSessionId === event.payload.session_id) {
          store.addModelFallbackNotice(event.payload.session_id, event.payload.content);
        }
      }
    )
  );

  listeners.push(
    listen<ToolCallEventPayload>(
      "agent://tool_call_pending",
      (event) => {
        showToolCall(event.payload, "pending_approval");
      }
    )
  );

  listeners.push(
    listen<ToolCallEventPayload>(
      "agent://tool_call_started",
      (event) => {
        showToolCall(event.payload, "running");
      }
    )
  );

  listeners.push(
    listen<{
      session_id: string;
      run_id: string;
      tool_call_id: string;
      tool: string;
      status?: ToolCall["status"];
      output: string;
    }>(
      "agent://tool_result",
      (event) => {
        const { session_id, tool_call_id, output } = event.payload;
        const store = useAgentStore.getState();
        if (store.activeSessionId !== session_id) return;
        const status = event.payload.status
          || (output === "Executing..." ? "running" : "succeeded");
        store.updateLocalToolCallStatus(
          tool_call_id,
          status,
          status === "running" ? undefined : output,
        );
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; title: string }>(
      "agent://session_title_updated",
      async (event) => {
        const store = useAgentStore.getState();
        if (
          store.activeAgentId
          && (
            store.activeSessionId === event.payload.session_id
            || store.sessions.some((session) => session.id === event.payload.session_id)
          )
        ) {
          await store.loadSessions(store.activeAgentId);
        }
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string }>(
      "agent://run_finished",
      async (event) => {
        flushBufferedDelta();
        const store = useAgentStore.getState();
        // Reload from SQLite to ensure database-level consistency
        if (store.activeSessionId === event.payload.session_id) {
          await store.loadMessages(event.payload.session_id, true);
          // Also refresh sessions list in case summary changed
          if (store.activeAgentId) {
            await store.loadSessions(store.activeAgentId);
          }
        }
        store.setStreamingState(false);
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string; message: string }>(
      "agent://run_error",
      async (event) => {
        flushBufferedDelta();
        const store = useAgentStore.getState();
        if (store.activeSessionId === event.payload.session_id) {
          await store.loadMessages(event.payload.session_id, true);
        }
        store.setStreamingState(false);
      }
    )
  );

  // Return unsubscribe cleanup function
  return async () => {
    if (deltaFlushTimer !== null) clearTimeout(deltaFlushTimer);
    bufferedDelta = null;
    const unsubscribes = await Promise.all(listeners);
    unsubscribes.forEach((unsub) => unsub());
  };
}
