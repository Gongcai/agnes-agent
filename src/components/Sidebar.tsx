import React, { useState, useEffect } from "react";
import { Plus, CornerDownRight, Settings, Pin, PinOff, Pencil, Trash2 } from "lucide-react";
import { useAgentStore } from "../store/useAgentStore";

interface SidebarProps {
  isOpen: boolean;
  onOpenSettings: (tab?: "agents" | "memory" | "llm" | "audit" | "debug") => void;
}

export const Sidebar: React.FC<SidebarProps> = ({ isOpen, onOpenSettings }) => {
  const {
    agents,
    sessions,
    activeAgentId,
    activeSessionId,
    setActiveSessionId,
    createSession,
    pinSession,
    renameSession,
    deleteSession,
  } = useAgentStore();

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  const activeSessionList = sessions.filter((s) => s.agent_id === activeAgentId);

  // 解析当前 Agent 实际绑定的模型（agents.model 形如 "provider_id/model_name"）
  const activeModelName = activeAgent?.model
    ? activeAgent.model.includes("/")
      ? activeAgent.model.split("/").pop()
      : activeAgent.model
    : "";

  // 会话右键菜单状态：{ sessionId, x, y, isPinned, title } | null
  const [ctxMenu, setCtxMenu] = useState<{
    sessionId: string;
    x: number;
    y: number;
    isPinned: boolean;
    title: string;
  } | null>(null);
  const closeCtxMenu = () => setCtxMenu(null);

  // 点击空白或按 Esc 关闭右键菜单
  useEffect(() => {
    if (!ctxMenu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeCtxMenu();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ctxMenu]);

  const handleAddSession = () => {
    if (!activeAgentId) return;
    const title = `新会话 #${activeSessionList.length + 1}`;
    createSession(activeAgentId, title).catch(console.error);
  };

  const handleCtxPin = () => {
    if (!ctxMenu) return;
    pinSession(ctxMenu.sessionId, !ctxMenu.isPinned).catch(console.error);
    closeCtxMenu();
  };

  const handleCtxRename = () => {
    if (!ctxMenu) return;
    const next = window.prompt("重命名会话", ctxMenu.title);
    if (next && next.trim() && next.trim() !== ctxMenu.title) {
      renameSession(ctxMenu.sessionId, next.trim()).catch(console.error);
    }
    closeCtxMenu();
  };

  const handleCtxDelete = () => {
    if (!ctxMenu) return;
    if (!window.confirm(`确定删除会话「${ctxMenu.title}」吗？此操作不可撤销。`)) {
      closeCtxMenu();
      return;
    }
    deleteSession(ctxMenu.sessionId).catch(console.error);
    closeCtxMenu();
  };

  return (
    <aside
      className={`flex flex-col border-r border-stone-200/80 bg-stone-100/50 backdrop-blur-md transition-all duration-300 ${
        isOpen ? "w-64" : "w-0 border-r-0 overflow-hidden"
      }`}
    >
      {/* Top Active Agent Card */}
      {activeAgent && (
        <div className="border-b border-stone-200/80 p-4">
          <div className="flex items-center gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-indigo-50 border border-indigo-100 font-bold text-indigo-600 text-md shadow-sm">
              {activeAgent.name.charAt(0)}
            </div>
            <div className="overflow-hidden">
              <span className="font-semibold text-stone-900 block truncate text-sm">
                {activeAgent.name}
              </span>
              {activeModelName ? (
                <span className="text-[10px] bg-emerald-50/80 text-emerald-700 px-1.5 py-0.5 rounded font-mono border border-emerald-200/50 inline-block max-w-full truncate align-middle" title={activeAgent.model}>
                  {activeModelName}
                </span>
              ) : (
                <span className="text-[10px] bg-stone-200/60 text-stone-500 px-1.5 py-0.5 rounded font-mono border border-stone-300/40 inline-block">
                  未配置模型
                </span>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Sessions List */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        <div>
          <div className="flex items-center justify-between px-2 mb-2 text-[10px] font-bold text-stone-400 uppercase tracking-wider">
            <span>当前会话</span>
            <button
              onClick={handleAddSession}
              className="text-stone-500 hover:text-stone-900 transition-colors"
              title="新建会话"
            >
              <Plus className="h-3.5 w-3.5" />
            </button>
          </div>

          <div className="space-y-1">
            {activeSessionList.map((sess) => {
              const isActive = sess.id === activeSessionId;
              return (
                <button
                  key={sess.id}
                  onClick={() => setActiveSessionId(sess.id)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    setCtxMenu({
                      sessionId: sess.id,
                      x: e.clientX,
                      y: e.clientY,
                      isPinned: !!sess.pinned,
                      title: sess.title,
                    });
                  }}
                  className={`flex w-full items-center gap-2 rounded-xl px-2.5 py-2 text-left text-xs transition-all duration-150 ${
                    isActive
                      ? "bg-white text-emerald-700 font-semibold border border-stone-200 shadow-[0_1px_2px_0_rgba(0,0,0,0.03)]"
                      : "text-stone-600 hover:bg-stone-200/40 hover:text-stone-900"
                  }`}
                >
                  <CornerDownRight className="h-3.5 w-3.5 shrink-0 text-stone-400" />
                  <span className="flex-1 truncate">{sess.title}</span>
                  {sess.pinned && (
                    <Pin className="h-3 w-3 shrink-0 text-amber-500" />
                  )}
                </button>
              );
            })}
            {activeSessionList.length === 0 && (
              <div className="text-center py-6 text-[11px] text-stone-400">
                无会话，请点击右上角新建
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Sidebar Footer */}
      <div className="mt-auto border-t border-stone-200 p-3 bg-stone-200/20 flex items-center justify-between">
        <div className="flex items-center gap-2 overflow-hidden mr-2">
          <div className="h-1.5 w-1.5 rounded-full bg-emerald-500"></div>
          <span className="text-[10px] text-stone-500 truncate">Sidecar 已就绪</span>
        </div>
        <button
          onClick={() => onOpenSettings("agents")}
          className="flex h-8 w-8 items-center justify-center rounded-xl bg-white text-stone-500 hover:text-stone-900 transition-colors border border-stone-200 shadow-sm"
          title="控制中心"
        >
          <Settings className="h-4 w-4" />
        </button>
      </div>

      {/* 会话右键菜单 */}
      {ctxMenu && (
        <>
          <div className="fixed inset-0 z-40" onClick={closeCtxMenu} onContextMenu={(e) => { e.preventDefault(); closeCtxMenu(); }} />
          <div
            className="fixed z-50 w-40 rounded-xl border border-stone-200 bg-white shadow-2xl py-1 text-xs text-stone-700"
            style={{ top: ctxMenu.y, left: ctxMenu.x }}
          >
            <button
              onClick={handleCtxPin}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-stone-100 transition-colors"
            >
              {ctxMenu.isPinned ? <PinOff className="h-3.5 w-3.5 text-amber-500" /> : <Pin className="h-3.5 w-3.5 text-stone-500" />}
              {ctxMenu.isPinned ? "取消置顶" : "置顶"}
            </button>
            <button
              onClick={handleCtxRename}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-stone-100 transition-colors"
            >
              <Pencil className="h-3.5 w-3.5 text-stone-500" />
              重命名
            </button>
            <button
              onClick={handleCtxDelete}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-rose-600 hover:bg-rose-50 transition-colors"
            >
              <Trash2 className="h-3.5 w-3.5" />
              删除
            </button>
          </div>
        </>
      )}
    </aside>
  );
};
