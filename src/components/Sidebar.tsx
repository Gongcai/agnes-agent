import React, { useState, useEffect } from "react";
import { Plus, CornerDownRight, Settings, Pin, PinOff, Pencil, Trash2, FolderPlus, Folder, ChevronRight, ChevronDown } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useAgentStore } from "../store/useAgentStore";
import { AgentAvatar } from "./AgentAvatar";

type SettingsTab = "general" | "agents" | "memory" | "llm" | "audit" | "debug";

interface SidebarProps {
  isOpen: boolean;
  onOpenSettings: (tab?: SettingsTab) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({ isOpen, onOpenSettings }) => {
  const {
    agents,
    sessions,
    workspaces,
    activeAgentId,
    activeSessionId,
    setActiveSessionId,
    createSession,
    pinSession,
    renameSession,
    deleteSession,
    createWorkspace,
    renameWorkspace,
    deleteWorkspace,
  } = useAgentStore();

  const activeAgent = agents.find((a) => a.id === activeAgentId);
  // 普通对话：workspace_id 为空
  const standaloneSessions = sessions.filter((s) => s.agent_id === activeAgentId && !s.workspace_id);
  const agentWorkspaces = workspaces.filter((w) => w.agent_id === activeAgentId);

  // 解析当前 Agent 实际绑定的模型
  const activeModelName = activeAgent?.model
    ? activeAgent.model.includes("/")
      ? activeAgent.model.split("/").pop()
      : activeAgent.model
    : "";

  // 会话右键菜单
  const [ctxMenu, setCtxMenu] = useState<{
    sessionId: string;
    x: number;
    y: number;
    isPinned: boolean;
    title: string;
  } | null>(null);
  // 工作区右键菜单
  const [wsCtxMenu, setWsCtxMenu] = useState<{
    workspaceId: string;
    name: string;
    x: number;
    y: number;
  } | null>(null);
  const [expandedWs, setExpandedWs] = useState<Set<string>>(new Set());
  const closeCtxMenu = () => setCtxMenu(null);
  const closeWsCtxMenu = () => setWsCtxMenu(null);

  useEffect(() => {
    if (!ctxMenu && !wsCtxMenu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { closeCtxMenu(); closeWsCtxMenu(); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ctxMenu, wsCtxMenu]);

  // 默认展开所有工作区
  useEffect(() => {
    setExpandedWs(new Set(agentWorkspaces.map((w) => w.id)));
  }, [agentWorkspaces.length]);

  const handleAddStandaloneSession = () => {
    if (!activeAgentId) return;
    createSession(activeAgentId, `新会话 #${standaloneSessions.length + 1}`, null).catch(console.error);
  };

  const handleAddWorkspaceSession = (workspaceId: string) => {
    if (!activeAgentId) return;
    const wsSessions = sessions.filter((s) => s.workspace_id === workspaceId);
    createSession(activeAgentId, `会话 #${wsSessions.length + 1}`, workspaceId)
      .then(() => setExpandedWs((prev) => new Set(prev).add(workspaceId)))
      .catch(console.error);
  };

  const handleAddWorkspace = async () => {
    if (!activeAgentId) return;
    try {
      const selected = await open({ directory: true, multiple: false, title: "选择工作区文件夹" });
      if (typeof selected !== "string" || !selected) return;
      const name = selected.split(/[/\\]/).filter(Boolean).pop() || "工作区";
      await createWorkspace(activeAgentId, name, selected);
    } catch (e) {
      console.error("添加工作区失败", e);
    }
  };

  const toggleWs = (id: string) => {
    setExpandedWs((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
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
    if (!window.confirm(`确定删除会话「${ctxMenu.title}」吗？`)) { closeCtxMenu(); return; }
    deleteSession(ctxMenu.sessionId).catch(console.error);
    closeCtxMenu();
  };

  const handleWsCtxRename = () => {
    if (!wsCtxMenu) return;
    const next = window.prompt("重命名工作区", wsCtxMenu.name);
    if (next && next.trim() && next.trim() !== wsCtxMenu.name) {
      renameWorkspace(wsCtxMenu.workspaceId, next.trim()).catch(console.error);
    }
    closeWsCtxMenu();
  };
  const handleWsCtxDelete = () => {
    if (!wsCtxMenu) return;
    if (!window.confirm(`删除工作区「${wsCtxMenu.name}」？其下会话将转为普通对话保留。`)) { closeWsCtxMenu(); return; }
    deleteWorkspace(wsCtxMenu.workspaceId).catch(console.error);
    closeWsCtxMenu();
  };

  const renderSessionBtn = (sess: { id: string; title: string; pinned?: boolean }) => {
    const isActive = sess.id === activeSessionId;
    return (
      <button
        key={sess.id}
        onClick={() => setActiveSessionId(sess.id)}
        onContextMenu={(e) => {
          e.preventDefault();
          setCtxMenu({ sessionId: sess.id, x: e.clientX, y: e.clientY, isPinned: !!sess.pinned, title: sess.title });
        }}
        className={`flex w-full items-center gap-2 rounded-xl px-2.5 py-2 text-left text-xs transition-all duration-150 ${
          isActive
            ? "bg-white text-emerald-700 font-semibold border border-stone-200 shadow-[0_1px_2px_0_rgba(0,0,0,0.03)]"
            : "text-stone-600 hover:bg-stone-200/40 hover:text-stone-900"
        }`}
      >
        <CornerDownRight className="h-3.5 w-3.5 shrink-0 text-stone-400" />
        <span className="flex-1 truncate">{sess.title}</span>
        {sess.pinned && <Pin className="h-3 w-3 shrink-0 text-amber-500" />}
      </button>
    );
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
            <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={40} />
            <div className="overflow-hidden">
              <span className="font-semibold text-stone-900 block truncate text-sm">{activeAgent.name}</span>
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

      {/* Sessions List: 对话 / 工作区 */}
      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {/* 1. 对话 */}
        <div>
          <div className="flex items-center justify-between px-2 mb-2 text-[10px] font-bold text-stone-400 uppercase tracking-wider">
            <span>对话</span>
            <button onClick={handleAddStandaloneSession} className="text-stone-500 hover:text-stone-900 transition-colors" title="新建对话">
              <Plus className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="space-y-1">
            {standaloneSessions.map(renderSessionBtn)}
            {standaloneSessions.length === 0 && (
              <div className="text-center py-3 text-[11px] text-stone-400">无对话</div>
            )}
          </div>
        </div>

        {/* 2. 工作区 */}
        <div>
          <div className="flex items-center justify-between px-2 mb-2 text-[10px] font-bold text-stone-400 uppercase tracking-wider">
            <span>工作区</span>
            <button onClick={handleAddWorkspace} className="text-stone-500 hover:text-stone-900 transition-colors" title="添加工作区（选择文件夹）">
              <FolderPlus className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="space-y-1">
            {agentWorkspaces.length === 0 && (
              <div className="text-center py-3 text-[11px] text-stone-400">
                无工作区，点击右上角添加文件夹
              </div>
            )}
            {agentWorkspaces.map((ws) => {
              const expanded = expandedWs.has(ws.id);
              const wsSessions = sessions.filter((s) => s.workspace_id === ws.id);
              return (
                <div key={ws.id}>
                  <div
                    className={`flex w-full items-center gap-1.5 rounded-xl px-2 py-1.5 text-left text-xs transition-colors cursor-pointer ${
                      "text-stone-700 hover:bg-stone-200/40"
                    }`}
                    onClick={() => toggleWs(ws.id)}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setWsCtxMenu({ workspaceId: ws.id, name: ws.name, x: e.clientX, y: e.clientY });
                    }}
                    title={ws.folder_path}
                  >
                    {expanded ? <ChevronDown className="h-3.5 w-3.5 shrink-0 text-stone-400" /> : <ChevronRight className="h-3.5 w-3.5 shrink-0 text-stone-400" />}
                    <Folder className="h-3.5 w-3.5 shrink-0 text-amber-500" />
                    <span className="flex-1 truncate font-medium">{ws.name}</span>
                    <button
                      onClick={(e) => { e.stopPropagation(); handleAddWorkspaceSession(ws.id); }}
                      className="text-stone-400 hover:text-stone-900 shrink-0"
                      title="在工作区中新建会话"
                    >
                      <Plus className="h-3 w-3" />
                    </button>
                  </div>
                  {expanded && (
                    <div className="ml-3 mt-0.5 space-y-1 border-l border-stone-200/60 pl-2">
                      {wsSessions.map(renderSessionBtn)}
                      {wsSessions.length === 0 && (
                        <div className="text-center py-2 text-[10px] text-stone-400">无会话</div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
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
          <div className="fixed z-50 w-40 rounded-xl border border-stone-200 bg-white shadow-2xl py-1 text-xs text-stone-700" style={{ top: ctxMenu.y, left: ctxMenu.x }}>
            <button onClick={handleCtxPin} className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-stone-100 transition-colors">
              {ctxMenu.isPinned ? <PinOff className="h-3.5 w-3.5 text-amber-500" /> : <Pin className="h-3.5 w-3.5 text-stone-500" />}
              {ctxMenu.isPinned ? "取消置顶" : "置顶"}
            </button>
            <button onClick={handleCtxRename} className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-stone-100 transition-colors">
              <Pencil className="h-3.5 w-3.5 text-stone-500" />重命名
            </button>
            <button onClick={handleCtxDelete} className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-rose-600 hover:bg-rose-50 transition-colors">
              <Trash2 className="h-3.5 w-3.5" />删除
            </button>
          </div>
        </>
      )}

      {/* 工作区右键菜单 */}
      {wsCtxMenu && (
        <>
          <div className="fixed inset-0 z-40" onClick={closeWsCtxMenu} onContextMenu={(e) => { e.preventDefault(); closeWsCtxMenu(); }} />
          <div className="fixed z-50 w-40 rounded-xl border border-stone-200 bg-white shadow-2xl py-1 text-xs text-stone-700" style={{ top: wsCtxMenu.y, left: wsCtxMenu.x }}>
            <button onClick={handleWsCtxRename} className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-stone-100 transition-colors">
              <Pencil className="h-3.5 w-3.5 text-stone-500" />重命名
            </button>
            <button onClick={handleWsCtxDelete} className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-rose-600 hover:bg-rose-50 transition-colors">
              <Trash2 className="h-3.5 w-3.5" />删除工作区
            </button>
          </div>
        </>
      )}
    </aside>
  );
};
