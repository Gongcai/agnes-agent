import React, { useState, useEffect } from "react";
import { X, User, Database, Sliders, ShieldCheck, Key } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "../store/useAgentStore";

interface SettingsModalProps {
  isOpen: boolean;
  onClose: () => void;
  initialTab?: "agents" | "memory" | "llm" | "audit";
}

interface AuditLog {
  id: string;
  time: string;
  tool: string;
  params: string;
  status: string;
  risk: string;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  isOpen,
  onClose,
  initialTab = "agents",
}) => {
  const { agents, activeAgentId, activeSessionId } = useAgentStore();
  const [activeTab, setActiveTab] = useState<"agents" | "memory" | "llm" | "audit">(initialTab);
  
  // Memory MD state
  const [userMdText, setUserMdText] = useState("");
  const [memoryMdText, setMemoryMdText] = useState("");
  const [isEditingUserMd, setIsEditingUserMd] = useState(false);
  const [isEditingMemoryMd, setIsEditingMemoryMd] = useState(false);

  // Audit state
  const [auditLogs, setAuditLogs] = useState<AuditLog[]>([]);

  // Sync memory text when activeAgentId changes
  useEffect(() => {
    if (activeAgentId && activeTab === "memory") {
      invoke<{ user_md: string; memory_md: string }>("get_explicit_memories", {
        agentId: activeAgentId,
      })
        .then((res) => {
          setUserMdText(res.user_md);
          setMemoryMdText(res.memory_md);
          setIsEditingUserMd(false);
          setIsEditingMemoryMd(false);
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

  const activeAgent = agents.find((a) => a.id === activeAgentId);

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
          </nav>

          {/* Right Panel View */}
          <div className="flex-1 overflow-y-auto p-6 bg-white">
            {/* 1. AGENTS TAB */}
            {activeTab === "agents" && activeAgent && (
              <div className="space-y-6">
                <div>
                  <h3 className="text-sm font-semibold text-stone-850">智能体信息</h3>
                  <p className="text-[11px] text-stone-400">查看当前选择 of 的 Agent 静态配置及能力描述。</p>
                </div>

                <div className="border border-stone-200 bg-[#FAF9F5]/20 rounded-xl p-5 space-y-4 shadow-sm">
                  <div className="flex items-center gap-3 pb-3 border-b border-stone-200">
                    <div className="h-10 w-10 bg-indigo-50 border border-indigo-100 rounded-full flex items-center justify-center text-indigo-600 font-bold text-md">
                      {activeAgent.name.charAt(0)}
                    </div>
                    <div>
                      <h4 className="font-semibold text-xs text-stone-800">{activeAgent.name}</h4>
                      <p className="text-[10px] text-stone-500 font-mono">ID: {activeAgent.id}</p>
                    </div>
                  </div>

                  <div className="space-y-3 text-xs text-stone-800 leading-relaxed">
                    <div>
                      <span className="font-semibold text-stone-500 block mb-1">能力模型:</span>
                      <p className="bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 font-mono text-[11px]">
                        gpt-4o
                      </p>
                    </div>
                    <div>
                      <span className="font-semibold text-stone-500 block mb-1">人格与背景设定 (Persona):</span>
                      <p className="bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 text-stone-650">
                        {activeAgent.id === "agnes" 
                          ? "你叫 Agnes，是 Tavern 的首席管家。你温和有礼、逻辑严密。偏好编写清晰模块化的代码。"
                          : activeAgent.id === "nova"
                          ? "你是 Nova，经验丰富的 DevSecOps 专家 and 代码审计员。防范任何恶意指令的执行。"
                          : "你是 Bard，酒馆吟游诗人，协助创意文学与背景设计。"}
                      </p>
                    </div>
                    <div>
                      <span className="font-semibold text-stone-500 block mb-1">工具安全策略 (Tool Policy):</span>
                      <pre className="bg-stone-50 p-2.5 rounded-lg border border-stone-200/60 font-mono text-[10px] text-stone-600">
                        {activeAgent.id === "agnes"
                          ? "Shell: 必审 | File: 写操作审 | Git: 免审"
                          : activeAgent.id === "nova"
                          ? "Shell: 必审 | File: 必审 | Git: 必审"
                          : "Shell: 禁止 | File: 禁止 | Git: 禁止"}
                      </pre>
                    </div>
                  </div>
                </div>
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

                <div className="grid grid-cols-2 gap-4">
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
              </div>
            )}

            {/* 3. LLM TAB */}
            {activeTab === "llm" && (
              <div className="space-y-5">
                <div>
                  <h3 className="text-sm font-semibold text-stone-850">模型与同步参数</h3>
                  <p className="text-[11px] text-stone-400">系统底层密钥及同步配置详情。</p>
                </div>

                <div className="border border-stone-200 bg-[#FAF9F5]/30 rounded-xl p-5 space-y-4 shadow-sm">
                  <div className="space-y-2">
                    <span className="block text-xs font-semibold text-stone-500 uppercase tracking-wide">
                      系统托管凭证锁 (OS Keyring)
                    </span>
                    <div className="flex items-center gap-2 bg-white border border-stone-200 px-3 py-2 rounded-lg text-xs">
                      <Key className="h-4 w-4 text-stone-400" />
                      <input
                        type="password"
                        value="sk-proj-xxxxxxxxxxxxxxxxxxxxxxxx"
                        disabled
                        className="bg-transparent flex-1 text-stone-400 font-mono focus:outline-none"
                      />
                      <span className="text-[10px] text-stone-400 bg-stone-100 px-2 py-0.5 rounded-md border border-stone-200 font-medium">
                        加密保护中
                      </span>
                    </div>
                  </div>

                  <div className="space-y-3 pt-3 border-t border-stone-200">
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
          </div>
        </div>
      </div>
    </div>
  );
};
