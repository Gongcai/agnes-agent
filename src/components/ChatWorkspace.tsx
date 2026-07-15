import React, { useState, useEffect, useRef } from "react";
import {
  Cpu, Terminal, Send, AlertTriangle, Menu, ChevronLeft, ShieldCheck, ChevronDown, Server, Check, Copy, GitBranch, Trash2, Pencil, RefreshCw, Brain, Square
} from "lucide-react";
import { Button } from "./ui/button";
import { useAgentStore } from "../store/useAgentStore";
import { AgentAvatar } from "./AgentAvatar";
import { MarkdownMessage } from "./MarkdownMessage";
import { ModifyMemoryModal } from "./ModifyMemoryModal";

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

interface ChatWorkspaceProps {
  isSidebarOpen: boolean;
  onToggleSidebar: () => void;
  onOpenSettings: (tab: "agents" | "memory" | "llm" | "audit" | "debug") => void;
}

export const ChatWorkspace: React.FC<ChatWorkspaceProps> = ({
  isSidebarOpen,
  onToggleSidebar,
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
    sendMessage,
    approveTool,
    cancelRun,
    setSessionLlm,
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
  const [editingMsgId, setEditingMsgId] = useState<string | null>(null);
  const [editingText, setEditingText] = useState("");
  const [memoryEditMsgId, setMemoryEditMsgId] = useState<string | null>(null);

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const activeSession = sessions.find((s) => s.id === activeSessionId);

  // 拉取服务商与模型列表，供底部模型切换器使用
  useEffect(() => {
    loadProviders().catch(console.error);
  }, [loadProviders]);

  // 当前生效的模型：优先会话级覆盖，回退角色卡默认（形如 "provider_id/model_name"）
  const effectiveModel = activeSession?.model || activeAgent?.model || "";
  const currentModel = (() => {
    if (!effectiveModel) return null;
    const idx = effectiveModel.indexOf("/");
    const pid = idx >= 0 ? effectiveModel.slice(0, idx) : "";
    const name = idx >= 0 ? effectiveModel.slice(idx + 1) : effectiveModel;
    const provider = providers.find((p) => p.id === pid);
    return { name, providerName: provider?.name ?? "" };
  })();

  // 当前生效的思考模式（会话级优先，回退角色卡）
  const currentThinkingMode = activeSession?.thinking_mode || activeAgent?.thinking_mode || "off";

  // 持久化会话级模型/思考配置
  const applySessionLlm = (model: string, thinkingMode: string, thinkingBudget: number) => {
    if (!activeSessionId) return;
    setSessionLlm(activeSessionId, model, thinkingMode, thinkingBudget).catch(console.error);
  };

  // Scroll to bottom on new messages
  useEffect(() => {
    messageEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, isStreaming]);

  const handleSend = () => {
    if (!inputVal.trim() || isStreaming || !activeSessionId) return;
    sendMessage(activeSessionId, inputVal.trim()).catch(console.error);
    setInputVal("");
  };

  return (
    <main className="flex flex-1 flex-col bg-[#FAF9F5] relative h-full">
      {/* Header bar */}
      <header className="flex h-14 items-center justify-between border-b border-stone-200 px-6 bg-white/40 backdrop-blur-md shrink-0">
        <div className="flex items-center gap-3">
          <button
            onClick={onToggleSidebar}
            className="text-stone-500 hover:text-stone-900 p-1.5 rounded-lg hover:bg-stone-200/40 transition-colors"
            title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
          >
            {isSidebarOpen ? <ChevronLeft className="h-4 w-4" /> : <Menu className="h-4 w-4" />}
          </button>
          <div className="h-4 w-[1px] bg-stone-200"></div>
          <div className="flex items-center gap-2">
            <span className="font-semibold text-stone-800 text-sm">
              {activeSession?.title || "暂无活动会话"}
            </span>
            {activeAgent && (
              <span className="text-[9px] bg-stone-200/60 border border-stone-300/20 px-1.5 py-0.5 rounded text-stone-600 font-mono font-medium">
                {activeAgent.name}
              </span>
            )}
          </div>
        </div>

        <div className="flex items-center gap-2">
          <button
            onClick={() => onOpenSettings("audit")}
            className="flex items-center gap-1.5 text-[11px] text-stone-600 hover:text-stone-900 bg-white px-2.5 py-1 rounded-lg border border-stone-200 shadow-sm transition-colors"
          >
            <ShieldCheck className="h-3.5 w-3.5 text-[#8CA38A]" />
            <span>审计流水</span>
          </button>
        </div>
      </header>

      {/* Message Panel list */}
      <div className="flex-1 overflow-y-auto p-6 space-y-6 max-w-4xl mx-auto w-full">
        {messages.map((message) => {
          const isUser = message.role === "user";
          return (
            <div
              key={message.id}
              className={`group flex gap-4 ${isUser ? "justify-end" : "justify-start"}`}
            >
              {!isUser && activeAgent && (
                <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={32} />
              )}

              <div className={`space-y-1.5 max-w-[85%] ${isUser ? "order-1" : "order-2"}`}>
                {isUser ? (
                  editingMsgId === message.id ? (
                    <div className="rounded-2xl rounded-tr-sm bg-[#F1F5F0]/70 px-3 py-2 text-sm text-stone-900 border border-[#8CA38A] shadow-sm space-y-2">
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
                    <div className="rounded-2xl rounded-tr-sm bg-[#F1F5F0]/70 px-4 py-2.5 text-sm text-stone-900 border border-[#DFE7DD] shadow-sm">
                      <p className="whitespace-pre-wrap leading-relaxed">
                        {message.parts.map((p) => p.content).join("")}
                      </p>
                    </div>
                  )
                ) : (
                  <div className="space-y-3.5">
                    {message.parts.map((part, index) => {
                      // tool_result 已在工具卡片中展示（tc.output），跳过避免重复泄漏为正文
                      if (part.kind === "tool_result") return null;
                      // tool_call 片段若无关联工具数据，也不当作正文渲染
                      if (part.kind === "tool_call" && !part.tool_call) return null;
                      // 1. Thought Process (reasoning)
                      if (part.kind === "thought") {
                        return (
                          <details
                            key={part.id || index}
                            open
                            className="group border-l-2 border-[#8CA38A] bg-stone-100/60 rounded-r-xl p-3 transition-colors"
                          >
                            <summary className="flex items-center gap-2 cursor-pointer text-xs font-semibold text-[#6C806A] select-none hover:text-[#556654]">
                              <Cpu className="h-3.5 w-3.5" />
                              <span>Agent 思维过程 (Thought)</span>
                              <ChevronDown className="h-3 w-3 ml-auto group-open:rotate-180 transition-transform" />
                            </summary>
                            <p className="text-xs text-stone-600 mt-2 font-mono leading-relaxed pl-5 whitespace-pre-wrap border-t border-stone-200/40 pt-2">
                              {part.content}
                            </p>
                          </details>
                        );
                      }

                      // 2. Tool Card
                      if (part.kind === "tool_call" && part.tool_call) {
                        const tc = part.tool_call;
                        const isHighRisk = tc.risk === "High";
                        const isPending = tc.status === "pending_approval";

                        return (
                          <div
                            key={part.id || index}
                            className={`border rounded-xl overflow-hidden transition-all duration-200 ${
                              isPending
                                ? isHighRisk
                                  ? "border-rose-300 bg-rose-50/50"
                                  : "border-amber-300 bg-amber-50/50 animate-pulse"
                                : tc.status === "denied"
                                ? "border-stone-200 bg-stone-100/40 opacity-70"
                                : "border-stone-200 bg-white shadow-sm"
                            }`}
                          >
                            <div className="px-4 py-2 flex items-center justify-between text-xs font-medium border-b border-stone-200 bg-stone-100/30">
                              <span className="flex items-center gap-1.5 text-stone-800">
                                <Terminal className="h-3.5 w-3.5 text-stone-500" />
                                <span>调用本地工具: {tc.tool}</span>
                              </span>
                              <span
                                className={`px-2 py-0.5 rounded text-[10px] ${
                                  isHighRisk ? "bg-rose-100 text-rose-700" : "bg-stone-200/80 text-stone-600"
                                }`}
                              >
                                风险: {tc.risk}
                              </span>
                            </div>

                            <div className="p-4 space-y-3 text-xs text-stone-800">
                              <div>
                                <span className="text-stone-500 font-mono">调用参数:</span>
                                <pre className="font-mono text-zinc-100 bg-zinc-900 p-3 rounded-lg border border-zinc-800 overflow-x-auto text-[11px] mt-1 shadow-inner">
                                  {tc.args}
                                </pre>
                              </div>

                              {isPending && (
                                <div className="bg-white p-2.5 rounded-lg border border-stone-200 flex items-start gap-2 shadow-sm">
                                  <AlertTriangle className="h-4 w-4 text-amber-500 shrink-0 mt-0.5" />
                                  <p className="text-[11px] text-stone-500 leading-relaxed">
                                    根据规则，运行此命令行需要人工审核批准。
                                  </p>
                                </div>
                              )}

                              {tc.output && (
                                <pre className="p-3 text-[10px] font-mono bg-zinc-900 text-zinc-300 max-h-36 overflow-y-auto whitespace-pre-wrap border border-zinc-800 rounded-lg shadow-inner">
                                  {tc.output}
                                </pre>
                              )}
                            </div>

                            {isPending && (
                              <div className="px-4 py-2.5 bg-stone-50 border-t border-stone-200/80 flex justify-end gap-2">
                                <button
                                  onClick={() => approveTool(tc.id, false).catch(console.error)}
                                  className="px-3 py-1 text-xs text-rose-600 bg-rose-50 hover:bg-rose-100 rounded-lg border border-rose-200 transition-all font-medium"
                                >
                                  拒绝执行
                                </button>
                                <button
                                  onClick={() => approveTool(tc.id, true).catch(console.error)}
                                  className="px-3 py-1 text-xs text-emerald-700 bg-emerald-50 hover:bg-emerald-100 rounded-lg border border-emerald-200 transition-all font-semibold"
                                >
                                  授权运行
                                </button>
                              </div>
                            )}
                          </div>
                        );
                      }

                      // 3. Regular response text rendering (Markdown + LaTeX)
                      return (
                        <MarkdownMessage
                          key={part.id || index}
                          content={part.content}
                        />
                      );
                    })}
                  </div>
                )}

                {/* 悬浮操作栏 */}
                <div className="opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-0.5 mt-1">
                  <button
                    onClick={() => {
                      const text = message.parts.map((p) => p.content).join("");
                      navigator.clipboard?.writeText(text).catch(console.error);
                    }}
                    className="p-1 rounded text-stone-400 hover:text-stone-700 hover:bg-stone-200/60"
                    title="复制消息"
                  >
                    <Copy className="h-3 w-3" />
                  </button>
                  {isUser && (
                    <button
                      onClick={() => {
                        setEditingMsgId(message.id);
                        setEditingText(message.parts.map((p) => p.content).join(""));
                      }}
                      className="p-1 rounded text-stone-400 hover:text-stone-700 hover:bg-stone-200/60"
                      title="编辑并重发"
                    >
                      <Pencil className="h-3 w-3" />
                    </button>
                  )}
                  {!isUser && (
                    <button
                      onClick={() => regenerateMessage(message.id).catch(console.error)}
                      disabled={isStreaming}
                      className="p-1 rounded text-stone-400 hover:text-stone-700 hover:bg-stone-200/60 disabled:opacity-30"
                      title="单条重新生成"
                    >
                      <RefreshCw className="h-3 w-3" />
                    </button>
                  )}
                  {!isUser && (
                    <button
                      onClick={() => setMemoryEditMsgId(message.id)}
                      disabled={message.status !== "complete"}
                      className="p-1 rounded text-stone-400 hover:text-stone-700 hover:bg-stone-200/60 disabled:opacity-30"
                      title="修改记忆"
                    >
                      <Brain className="h-3 w-3" />
                    </button>
                  )}
                  <button
                    onClick={() => createBranch(message.id).catch(console.error)}
                    className="p-1 rounded text-stone-400 hover:text-stone-700 hover:bg-stone-200/60"
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
                    className="p-1 rounded text-stone-400 hover:text-red-600 hover:bg-red-50 disabled:opacity-30 disabled:hover:bg-transparent disabled:hover:text-stone-400"
                    title={message.is_leaf ? "删除消息" : "仅可删除末梢消息"}
                  >
                    <Trash2 className="h-3 w-3" />
                  </button>
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
      <div className="border-t border-stone-200 bg-[#FAF9F5]/40 p-4 shrink-0">
        <div className="max-w-4xl mx-auto relative rounded-xl border border-stone-300/80 bg-white p-2.5 focus-within:border-stone-400 shadow-sm transition-all">
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
              activeAgent
                ? `向 ${activeAgent.name} 发送消息... (Enter 发送)`
                : "选择一个会话以开始..."
            }
            className="w-full resize-none bg-transparent px-3 py-1 text-sm text-stone-900 placeholder:text-stone-450 focus:outline-none h-12"
          />
          <div className="flex items-center justify-between border-t border-stone-100 pt-2 px-1 text-[10px] text-stone-400">
            <span>Agent 本地执行受系统沙箱安全策略保护</span>
            <div className="flex items-center gap-2">
              {/* Model switcher (Provider -> Model) */}
              <div className="relative">
                <button
                  onClick={() => setModelPickerOpen((v) => !v)}
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
                                const val = `${p.id}/${m}`;
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
                                    {m}
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
                  disabled={!inputVal.trim() || !activeSessionId}
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

      <ModifyMemoryModal
        message={memoryEditMsgId ? messages.find((m) => m.id === memoryEditMsgId) ?? null : null}
        onClose={() => setMemoryEditMsgId(null)}
      />
    </main>
  );
};
