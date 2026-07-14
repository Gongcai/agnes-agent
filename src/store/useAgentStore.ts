import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface ToolCall {
  id: string;
  tool: string;
  args: string;
  risk: string;
  status: "pending_approval" | "running" | "succeeded" | "denied" | "failed";
  output?: string;
}

export interface MessagePart {
  id: string;
  kind: "text" | "thought" | "tool_call" | "tool_result";
  content: string;
  tool_call?: ToolCall;
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
  /** 流式暂存：当前是否处于 <thought> 思维链中。仅用于直播渲染，不落库。 */
  _streamingInThought?: boolean;
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
  models: string[];
  has_api_key: boolean;
  created_at: string;
  updated_at: string;
}

interface AgentState {
  agents: AgentSummary[];
  sessions: Session[];
  messages: Message[];
  activeAgentId: string | null;
  activeSessionId: string | null;
  isStreaming: boolean;
  providers: ModelProvider[];
  
  // Actions
  loadAgents: () => Promise<void>;
  loadSessions: (agentId: string) => Promise<void>;
  loadMessages: (sessionId: string) => Promise<void>;
  createSession: (agentId: string, title: string) => Promise<string>;
  deleteSession: (sessionId: string) => Promise<void>;
  pinSession: (sessionId: string, pinned: boolean) => Promise<void>;
  renameSession: (sessionId: string, title: string) => Promise<void>;
  sendMessage: (sessionId: string, text: string) => Promise<void>;
  approveTool: (toolCallId: string, approved: boolean) => Promise<void>;
  setActiveAgentId: (agentId: string) => Promise<void>;
  setActiveSessionId: (sessionId: string) => Promise<void>;
  setSessionLlm: (sessionId: string, model: string, thinkingMode: string, thinkingBudget: number) => Promise<void>;
  switchVersion: (messageId: string, direction: "prev" | "next") => Promise<void>;
  createBranch: (messageId: string) => Promise<void>;
  deleteMessage: (messageId: string) => Promise<void>;
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
  upsertProvider: (provider: {
    id?: string;
    name: string;
    kind: string;
    api_base?: string;
    api_key?: string;
    is_default?: boolean;
    models?: string[];
  }) => Promise<string>;
  deleteProvider: (providerId: string) => Promise<void>;

  // Local Mutations (typically called by Tauri event listeners)
  appendStreamingDelta: (content: string) => void;
  updateLocalToolCallStatus: (toolCallId: string, status: string, output?: string) => void;
  setStreamingState: (isStreaming: boolean) => void;
}

export const useAgentStore = create<AgentState>((set, get) => ({
  agents: [],
  sessions: [],
  messages: [],
  activeAgentId: null,
  activeSessionId: null,
  isStreaming: false,
  providers: [],

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

  loadSessions: async (agentId: string) => {
    try {
      const sessions = await invoke<Session[]>("list_sessions", { agentId });
      set({ sessions });
    } catch (e) {
      console.error("Failed to load sessions", e);
    }
  },

  loadMessages: async (sessionId: string) => {
    try {
      const messages = await invoke<Message[]>("list_messages", { sessionId });
      set({ messages });
    } catch (e) {
      console.error("Failed to load messages", e);
    }
  },

  createSession: async (agentId: string, title: string) => {
    try {
      const sessionId = await invoke<string>("create_session", { agentId, title });
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

  sendMessage: async (sessionId: string, text: string) => {
    if (get().isStreaming) return;
    
    // 1. Instantly append a local user message and a pending assistant message for responsive UI
    const tempUserMsg: Message = {
      id: `temp_user_${Date.now()}`,
      session_id: sessionId,
      role: "user",
      seq: get().messages.length,
      status: "complete",
      parts: [{ id: `p_u_${Date.now()}`, kind: "text", content: text }],
      created_at: new Date().toLocaleTimeString("zh-CN", { hour12: false }),
      parent_id: get().messages.length > 0 ? get().messages[get().messages.length - 1].id : null,
      version_index: 0,
      version_count: 1,
      is_leaf: false,
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
    };

    set({
      isStreaming: true,
      messages: [...get().messages, tempUserMsg, tempAssistantMsg],
    });

    try {
      await invoke("send_message", { sessionId, text });
    } catch (e) {
      console.error("Failed to send message", e);
      // Remove assistant pending placeholder and reset streaming on immediate error
      set({
        isStreaming: false,
        messages: get().messages.filter(m => m.id !== tempAssistantMsg.id),
      });
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
    
    const sessions = get().sessions;
    if (sessions.length > 0) {
      await get().setActiveSessionId(sessions[0].id);
    } else {
      set({ activeSessionId: null, messages: [] });
    }
  },

  setActiveSessionId: async (sessionId: string) => {
    set({ activeSessionId: sessionId });
    await get().loadMessages(sessionId);
  },

  setSessionLlm: async (sessionId: string, model: string, thinkingMode: string, thinkingBudget: number) => {
    try {
      await invoke("set_session_llm", { sessionId, model, thinkingMode, thinkingBudget });
      // 立即更新本地会话状态，避免等待列表刷新
      const sessions = get().sessions.map((s) =>
        s.id === sessionId ? { ...s, model, thinking_mode: thinkingMode, thinking_budget: thinkingBudget } : s
      );
      set({ sessions });
    } catch (e) {
      console.error("设置会话模型/思考失败", e);
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

  upsertProvider: async (provider) => {
    try {
      const id = await invoke<string>("upsert_provider", { provider });
      await get().loadProviders();
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
    } catch (e) {
      console.error("Failed to delete provider", e);
    }
  },

  appendStreamingDelta: (content: string) => {
    const { messages } = get();
    if (messages.length === 0) return;

    const updatedMessages = [...messages];
    const lastMsg = { ...updatedMessages[updatedMessages.length - 1] };
    if (lastMsg.role !== "assistant") return;
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
    updatedMessages[updatedMessages.length - 1] = lastMsg;
    set({ messages: updatedMessages });
  },

  updateLocalToolCallStatus: (toolCallId: string, status: string, output?: string) => {
    const { messages } = get();
    if (messages.length === 0) return;

    const updatedMessages = [...messages];
    const lastMsg = { ...updatedMessages[updatedMessages.length - 1] };

    if (lastMsg.role === "assistant") {
      lastMsg.parts = lastMsg.parts.map(part => {
        if (part.kind === "tool_call" && part.tool_call?.id === toolCallId) {
          return {
            ...part,
            tool_call: {
              ...part.tool_call,
              status: status as any,
              output: output !== undefined ? output : part.tool_call.output,
            }
          };
        }
        return part;
      });
      updatedMessages[updatedMessages.length - 1] = lastMsg;
      set({ messages: updatedMessages });
    }
  },

  setStreamingState: (isStreaming: boolean) => {
    set({ isStreaming });
  }
}));

// Setup bridge listener to bind Tauri backend events to Zustand actions
export function setupTauriEventListeners() {
  const listeners: Promise<() => void>[] = [];

  listeners.push(
    listen<{ session_id: string; run_id: string; content: string }>(
      "agent://assistant_delta",
      (event) => {
        useAgentStore.getState().appendStreamingDelta(event.payload.content);
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string; tool_call_id: string; tool: string; arguments: any }>(
      "agent://tool_call_pending",
      (event) => {
        const { tool_call_id, tool, arguments: args } = event.payload;
        // Inject a pending tool call into the current assistant message's parts list for visual card rendering
        const { messages } = useAgentStore.getState();
        if (messages.length === 0) return;
        
        const updatedMessages = [...messages];
        const lastMsg = { ...updatedMessages[updatedMessages.length - 1] };
        
        if (lastMsg.role === "assistant") {
          lastMsg.parts = [...lastMsg.parts, {
            id: `p_tc_${tool_call_id}`,
            kind: "tool_call",
            content: `Suspended for user approval: calling ${tool}`,
            tool_call: {
              id: tool_call_id,
              tool,
              args: typeof args === "string" ? args : JSON.stringify(args),
              risk: tool === "shell" ? "Medium" : "High",
              status: "pending_approval"
            }
          }];
          updatedMessages[updatedMessages.length - 1] = lastMsg;
          useAgentStore.setState({ messages: updatedMessages });
        }
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string; tool_call_id: string; tool: string; output: string }>(
      "agent://tool_result",
      (event) => {
        const { tool_call_id, output } = event.payload;
        const store = useAgentStore.getState();
        
        // Find if we already have the tool call in parts list
        const lastMsg = store.messages[store.messages.length - 1];
        if (lastMsg && lastMsg.role === "assistant") {
          const hasToolCall = lastMsg.parts.some(p => p.kind === "tool_call" && p.tool_call?.id === tool_call_id);
          
          if (hasToolCall) {
            // Update its status
            const status = output === "Executing..." ? "running" : "succeeded";
            store.updateLocalToolCallStatus(tool_call_id, status, output);
          }
        }
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string }>(
      "agent://run_finished",
      async (_event) => {
        const store = useAgentStore.getState();
        store.setStreamingState(false);
        // Reload from SQLite to ensure database-level consistency
        if (store.activeSessionId) {
          await store.loadMessages(store.activeSessionId);
          // Also refresh sessions list in case summary changed
          if (store.activeAgentId) {
            await store.loadSessions(store.activeAgentId);
          }
        }
      }
    )
  );

  listeners.push(
    listen<{ session_id: string; run_id: string; message: string }>(
      "agent://run_error",
      async (_event) => {
        const store = useAgentStore.getState();
        store.setStreamingState(false);
        if (store.activeSessionId) {
          await store.loadMessages(store.activeSessionId);
        }
      }
    )
  );

  // Return unsubscribe cleanup function
  return async () => {
    const unsubscribes = await Promise.all(listeners);
    unsubscribes.forEach((unsub) => unsub());
  };
}
