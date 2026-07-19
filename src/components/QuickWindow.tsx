import React, { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Brain,
  ChevronRight,
  CircleStop,
  FileText,
  Languages,
  Lightbulb,
  MessageSquareText,
  Pin,
  PinOff,
  Plus,
  Send,
  ShieldAlert,
  Sparkles,
} from "lucide-react";
import { AgentAvatar } from "./AgentAvatar";
import { MarkdownMessage } from "./MarkdownMessage";
import { setupTauriEventListeners, useAgentStore } from "../store/useAgentStore";

const QUICK_SESSION_SETTING = "ui:quick_session_id";

type QuickActionId = "answer" | "translate" | "summarize" | "explain";

interface QuickAction {
  id: QuickActionId;
  label: string;
  placeholder: string;
  icon: React.ComponentType<{ className?: string }>;
  prompt: (input: string) => string;
}

const QUICK_ACTIONS: QuickAction[] = [
  {
    id: "answer",
    label: "回答此问题",
    placeholder: "输入你想问的问题...",
    icon: MessageSquareText,
    prompt: (input) => input,
  },
  {
    id: "translate",
    label: "文本翻译",
    placeholder: "输入或粘贴需要翻译的文本...",
    icon: Languages,
    prompt: (input) => `请将下面的文本翻译成简体中文；如果原文已是中文，则翻译成英文。保留原意和格式，只输出译文。\n\n${input}`,
  },
  {
    id: "summarize",
    label: "内容总结",
    placeholder: "输入或粘贴需要总结的内容...",
    icon: FileText,
    prompt: (input) => `请用简体中文总结下面的内容，先给出一句话结论，再列出关键要点。\n\n${input}`,
  },
  {
    id: "explain",
    label: "解释说明",
    placeholder: "输入需要解释的概念或内容...",
    icon: Lightbulb,
    prompt: (input) => `请用清晰、准确、容易理解的简体中文解释下面的内容，并在有帮助时给出一个简短示例。\n\n${input}`,
  },
];

async function activateQuickSession(forceNew = false): Promise<void> {
  const initialState = useAgentStore.getState();
  const agentId = initialState.activeAgentId;
  if (!agentId) return;

  const savedSessionId = forceNew
    ? null
    : await invoke<string | null>("get_setting", { key: QUICK_SESSION_SETTING });
  let sessionId = savedSessionId
    && initialState.sessions.some((session) => session.id === savedSessionId)
      ? savedSessionId
      : null;

  if (!sessionId) {
    sessionId = await invoke<string>("create_session", {
      agentId,
      title: "快速问答",
      workspaceId: null,
    });
    await invoke("set_setting", { key: QUICK_SESSION_SETTING, value: sessionId });
    await useAgentStore.getState().loadSessions(agentId);
  }

  await useAgentStore.getState().setActiveSessionId(sessionId, false);

  const state = useAgentStore.getState();
  const session = state.sessions.find((item) => item.id === sessionId);
  const agent = state.agents.find((item) => item.id === agentId);
  const quickModel = state.modelRoles.quick_model;
  if (session && quickModel && session.model !== quickModel) {
    await state.setSessionLlm(
      sessionId,
      quickModel,
      session.thinking_mode || agent?.thinking_mode || "off",
      session.thinking_budget || agent?.thinking_budget || 0,
      session.max_tokens || 2048,
    );
  }
}

export const QuickWindow: React.FC = () => {
  const {
    agents,
    sessions,
    messages,
    activeAgentId,
    activeSessionId,
    isStreaming,
    init,
    sendMessage,
    cancelRun,
    approveTool,
  } = useAgentStore();
  const [inputValue, setInputValue] = useState("");
  const [selectedActionId, setSelectedActionId] = useState<QuickActionId>("answer");
  const [isPinned, setIsPinned] = useState(false);
  const [isPreparing, setIsPreparing] = useState(true);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const messageEndRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(isPinned);
  const quickWindow = useMemo(() => getCurrentWindow(), []);

  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const activeSession = sessions.find((session) => session.id === activeSessionId);
  const selectedAction = QUICK_ACTIONS.find((action) => action.id === selectedActionId) ?? QUICK_ACTIONS[0];
  const effectiveModel = activeSession?.model || activeAgent?.model || "Agnes";
  const modelName = effectiveModel.includes("/")
    ? effectiveModel.slice(effectiveModel.indexOf("/") + 1)
    : effectiveModel;

  useEffect(() => {
    pinnedRef.current = isPinned;
  }, [isPinned]);

  useEffect(() => {
    document.body.classList.add("quick-window-body");
    const cleanupListeners = setupTauriEventListeners();
    let active = true;

    init(false)
      .then(() => activateQuickSession())
      .then(() => {
        if (active) setErrorMessage(null);
      })
      .catch((error) => {
        console.error("快速窗口初始化失败", error);
        if (active) setErrorMessage(String(error));
      })
      .finally(() => {
        if (active) setIsPreparing(false);
      });

    return () => {
      active = false;
      document.body.classList.remove("quick-window-body");
      cleanupListeners().catch(console.error);
    };
  }, [init]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        quickWindow.hide().catch(console.error);
      }
    };
    window.addEventListener("keydown", onKeyDown);

    const unlistenPromise = quickWindow.onFocusChanged(({ payload: focused }) => {
      if (focused) {
        window.setTimeout(() => inputRef.current?.focus(), 50);
      } else if (!pinnedRef.current) {
        quickWindow.hide().catch(console.error);
      }
    });

    return () => {
      window.removeEventListener("keydown", onKeyDown);
      unlistenPromise.then((unlisten) => unlisten()).catch(console.error);
    };
  }, [quickWindow]);

  useEffect(() => {
    messageEndRef.current?.scrollIntoView({ behavior: isStreaming ? "auto" : "smooth" });
  }, [messages, isStreaming]);

  const handleSend = () => {
    const trimmed = inputValue.trim();
    if (!trimmed || !activeSessionId || isStreaming || isPreparing) return;
    setErrorMessage(null);
    sendMessage(activeSessionId, selectedAction.prompt(trimmed)).catch((error) => {
      console.error("快速问答发送失败", error);
      setErrorMessage(String(error));
    });
    setInputValue("");
  };

  const handleNewSession = () => {
    if (isStreaming || isPreparing) return;
    setIsPreparing(true);
    setErrorMessage(null);
    activateQuickSession(true)
      .then(() => {
        setInputValue("");
        setSelectedActionId("answer");
        window.setTimeout(() => inputRef.current?.focus(), 0);
      })
      .catch((error) => {
        console.error("新建快速会话失败", error);
        setErrorMessage(String(error));
      })
      .finally(() => setIsPreparing(false));
  };

  const hasConversation = messages.length > 0;

  return (
    <div className="quick-window-shell flex h-screen w-screen flex-col overflow-hidden text-stone-800 antialiased">
      <header
        data-tauri-drag-region
        className="flex h-[62px] shrink-0 items-center gap-3 border-b border-stone-200/80 bg-white/95 px-4"
      >
        {activeAgent ? (
          <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={30} />
        ) : (
          <div className="grid h-[30px] w-[30px] shrink-0 place-items-center rounded-full bg-[#d97757]/10 text-[#b95f43]">
            <Sparkles className="h-4 w-4" />
          </div>
        )}
        <div data-tauri-drag-region className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium text-stone-700">
            问问 {modelName || "Agnes"} 获取帮助...
          </div>
          <div className="truncate text-[10px] text-stone-400">
            {activeAgent?.name || (isPreparing ? "正在准备快速会话" : "尚未配置 Agent")}
          </div>
        </div>
        <button
          type="button"
          onClick={handleNewSession}
          disabled={isPreparing || isStreaming || !activeAgentId}
          className="grid h-8 w-8 shrink-0 place-items-center rounded-md text-stone-400 transition-colors hover:bg-stone-100 hover:text-stone-700 disabled:opacity-30"
          title="新建快速会话"
          aria-label="新建快速会话"
        >
          <Plus className="h-4 w-4" />
        </button>
        <button
          type="button"
          onClick={() => setIsPinned((pinned) => !pinned)}
          className={`grid h-8 w-8 shrink-0 place-items-center rounded-md transition-colors ${
            isPinned
              ? "bg-[#8CA38A]/15 text-[#5F735D]"
              : "text-stone-400 hover:bg-stone-100 hover:text-stone-700"
          }`}
          title={isPinned ? "取消固定窗口" : "固定窗口"}
          aria-label={isPinned ? "取消固定窗口" : "固定窗口"}
        >
          {isPinned ? <PinOff className="h-3.5 w-3.5" /> : <Pin className="h-3.5 w-3.5" />}
        </button>
      </header>

      <section className="min-h-0 flex-1 overflow-y-auto bg-[#fbfaf7]/95">
        {!hasConversation ? (
          <div className="p-3">
            {QUICK_ACTIONS.map((action) => {
              const Icon = action.icon;
              const selected = action.id === selectedActionId;
              return (
                <button
                  type="button"
                  key={action.id}
                  onClick={() => {
                    setSelectedActionId(action.id);
                    window.setTimeout(() => inputRef.current?.focus(), 0);
                  }}
                  className={`flex h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm font-medium transition-colors ${
                    selected
                      ? "bg-stone-200/70 text-stone-800"
                      : "text-stone-700 hover:bg-stone-100"
                  }`}
                >
                  <Icon className="h-4 w-4 shrink-0 text-stone-500" />
                  <span className="min-w-0 flex-1 truncate">{action.label}</span>
                  <ChevronRight className="h-4 w-4 shrink-0 text-stone-300" />
                </button>
              );
            })}
          </div>
        ) : (
          <div className="space-y-5 p-4">
            {messages.map((message) => {
              const textParts = message.parts.filter((part) => part.kind === "text");
              const thoughtParts = message.parts.filter((part) => part.kind === "thought");
              const toolParts = message.parts.filter((part) => part.kind === "tool_call" && part.tool_call);
              const fallbackParts = message.parts.filter((part) => part.kind === "model_fallback");
              const text = textParts.map((part) => part.content).join("");
              const thought = thoughtParts.map((part) => part.content).join("");

              if (message.role === "user") {
                return (
                  <div key={message._renderKey ?? message.id} className="flex justify-end">
                    <div className="max-w-[78%] whitespace-pre-wrap break-words rounded-lg bg-stone-100 px-3.5 py-2.5 text-sm leading-6 text-stone-700">
                      {text}
                    </div>
                  </div>
                );
              }

              const streaming = isStreaming && message === messages[messages.length - 1];
              return (
                <article key={message._renderKey ?? message.id} className="space-y-3">
                  {thought && (
                    <details className="group rounded-md border border-stone-200 bg-white/70">
                      <summary className="flex h-9 cursor-pointer list-none items-center gap-2 px-3 text-[11px] font-medium text-stone-500 [&::-webkit-details-marker]:hidden">
                        <Brain className="h-3.5 w-3.5 text-[#8CA38A]" />
                        <span className="flex-1">{streaming ? "正在思考" : "思考过程"}</span>
                        <ChevronRight className="h-3.5 w-3.5 transition-transform group-open:rotate-90" />
                      </summary>
                      <div className="border-t border-stone-100 px-3 py-2 text-xs leading-5 text-stone-500">
                        <MarkdownMessage content={thought} streaming={streaming} />
                      </div>
                    </details>
                  )}
                  {fallbackParts.map((part) => (
                    <div key={part._renderKey ?? part.id} className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[11px] text-amber-800">
                      {part.content}
                    </div>
                  ))}
                  {toolParts.map((part) => {
                    const toolCall = part.tool_call!;
                    return (
                      <div key={toolCall.id} className="rounded-md border border-stone-200 bg-white px-3 py-2 text-[11px] text-stone-600">
                        <div className="flex items-center gap-2">
                          <ShieldAlert className="h-3.5 w-3.5 text-stone-400" />
                          <span className="min-w-0 flex-1 truncate font-mono">{toolCall.tool}</span>
                          <span className="text-[10px] text-stone-400">{toolCall.status}</span>
                        </div>
                        {toolCall.status === "pending_approval" && (
                          <div className="mt-2 flex justify-end gap-2 border-t border-stone-100 pt-2">
                            <button
                              type="button"
                              onClick={() => approveTool(toolCall.id, false)}
                              className="rounded-md px-2.5 py-1 text-[10px] text-rose-600 hover:bg-rose-50"
                            >
                              拒绝
                            </button>
                            <button
                              type="button"
                              onClick={() => approveTool(toolCall.id, true)}
                              className="rounded-md bg-[#8CA38A] px-2.5 py-1 text-[10px] font-medium text-white hover:bg-[#7A917A]"
                            >
                              允许
                            </button>
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {(text || streaming) && (
                    <div className="text-[13px] leading-6 text-stone-800">
                      <MarkdownMessage content={text} streaming={streaming} />
                    </div>
                  )}
                </article>
              );
            })}
            <div ref={messageEndRef} />
          </div>
        )}
      </section>

      {errorMessage && (
        <div className="shrink-0 border-t border-rose-100 bg-rose-50 px-4 py-2 text-[11px] text-rose-700">
          {errorMessage}
        </div>
      )}

      <footer className="shrink-0 border-t border-stone-200 bg-white/95 px-3 pb-2.5 pt-2">
        <div className="flex min-h-11 items-end gap-2 rounded-md border border-stone-200 bg-white px-3 py-2 focus-within:border-stone-400">
          <textarea
            ref={inputRef}
            value={inputValue}
            onChange={(event) => setInputValue(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                handleSend();
              }
            }}
            rows={1}
            autoFocus
            disabled={isPreparing || !activeSessionId}
            placeholder={isPreparing ? "正在准备快速会话..." : selectedAction.placeholder}
            className="max-h-24 min-h-6 min-w-0 flex-1 resize-none bg-transparent py-0.5 text-sm leading-5 text-stone-800 outline-none placeholder:text-stone-400 disabled:opacity-60"
          />
          {isStreaming ? (
            <button
              type="button"
              onClick={() => activeSessionId && cancelRun(activeSessionId)}
              className="grid h-7 w-7 shrink-0 place-items-center rounded-md bg-rose-600 text-white hover:bg-rose-700"
              title="停止生成"
              aria-label="停止生成"
            >
              <CircleStop className="h-3.5 w-3.5" />
            </button>
          ) : (
            <button
              type="button"
              onClick={handleSend}
              disabled={!inputValue.trim() || !activeSessionId || isPreparing}
              className="grid h-7 w-7 shrink-0 place-items-center rounded-md bg-stone-900 text-white transition-colors hover:bg-stone-700 disabled:bg-stone-200 disabled:text-stone-400"
              title="发送"
              aria-label="发送"
            >
              <Send className="h-3.5 w-3.5" />
            </button>
          )}
        </div>
        <div className="mt-1.5 flex items-center justify-between px-0.5 text-[10px] text-stone-400">
          <span>按 ESC 关闭</span>
          <span>{selectedAction.label} · Enter 发送</span>
        </div>
      </footer>
    </div>
  );
};
