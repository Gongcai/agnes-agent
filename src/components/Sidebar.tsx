import React, { useEffect, useState } from "react";
import {
  Brain,
  BookOpen,
  CalendarDays,
  CheckSquare2,
  ChevronDown,
  ChevronRight,
  Cloud,
  CornerDownRight,
  Database,
  Folder,
  FolderPlus,
  HardDrive,
  MessageSquare,
  Pencil,
  Pin,
  PinOff,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Settings,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { ENABLED_APP_FEATURES, type AppFeatureId } from "../lib/features";
import { useAgentStore } from "../store/useAgentStore";
import { AgentAvatar } from "./AgentAvatar";
import { NotificationCenter, type AppNotification } from "./NotificationCenter";

type SettingsTab = "general" | "agents" | "memory" | "llm" | "tokens" | "mcp" | "audit" | "debug";

interface SidebarProps {
  isOpen: boolean;
  activeFeature: AppFeatureId;
  onSelectFeature: (feature: AppFeatureId) => void;
  onToggleSidebar: () => void;
  onOpenSettings: (tab?: SettingsTab) => void;
  onNotificationNavigate: (notification: AppNotification) => void | Promise<void>;
}

const FEATURE_ICONS: Record<AppFeatureId, LucideIcon> = {
  chat: MessageSquare,
  reading: BookOpen,
  memory: Brain,
  knowledge: Database,
  drive: HardDrive,
  calendar: CalendarDays,
  tasks: CheckSquare2,
};

function readLocalBoolean(key: string, fallback: boolean): boolean {
  try {
    const value = window.localStorage.getItem(key);
    return value === null ? fallback : value === "true";
  } catch {
    return fallback;
  }
}

function writeLocalBoolean(key: string, value: boolean): void {
  try {
    window.localStorage.setItem(key, String(value));
  } catch {
    // UI preferences are optional when browser storage is unavailable.
  }
}

export const Sidebar: React.FC<SidebarProps> = ({
  isOpen,
  activeFeature,
  onSelectFeature,
  onToggleSidebar,
  onOpenSettings,
  onNotificationNavigate,
}) => {
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
  // Standalone sessions do not belong to a workspace.
  const standaloneSessions = sessions.filter((s) => s.agent_id === activeAgentId && !s.workspace_id);
  const agentWorkspaces = workspaces.filter((w) => w.agent_id === activeAgentId);
  const activeFeatureIndex = Math.max(
    0,
    ENABLED_APP_FEATURES.findIndex((feature) => feature.id === activeFeature),
  );

  // Resolve the model currently bound to the active agent.
  const activeModelName = activeAgent?.model
    ? activeAgent.model.includes("/")
      ? activeAgent.model.split("/").pop()
      : activeAgent.model
    : "";

  const [ctxMenu, setCtxMenu] = useState<{
    sessionId: string;
    x: number;
    y: number;
    isPinned: boolean;
    title: string;
  } | null>(null);
  const [wsCtxMenu, setWsCtxMenu] = useState<{
    workspaceId: string;
    name: string;
    x: number;
    y: number;
  } | null>(null);
  const [standaloneExpanded, setStandaloneExpanded] = useState(() =>
    readLocalBoolean("agnes.ui.sidebar.standalone-expanded", true),
  );
  const [workspacesExpanded, setWorkspacesExpanded] = useState(() =>
    readLocalBoolean("agnes.ui.sidebar.workspaces-expanded", true),
  );
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

  useEffect(() => {
    writeLocalBoolean(
      "agnes.ui.sidebar.standalone-expanded",
      standaloneExpanded,
    );
  }, [standaloneExpanded]);

  useEffect(() => {
    writeLocalBoolean(
      "agnes.ui.sidebar.workspaces-expanded",
      workspacesExpanded,
    );
  }, [workspacesExpanded]);

  // Expand newly loaded workspace groups by default.
  useEffect(() => {
    setExpandedWs(new Set(agentWorkspaces.map((w) => w.id)));
  }, [agentWorkspaces.length]);

  const handleAddStandaloneSession = () => {
    if (!activeAgentId) return;
    onSelectFeature("chat");
    createSession(activeAgentId, `新会话 #${standaloneSessions.length + 1}`, null).catch(console.error);
  };

  const handleAddWorkspaceSession = (workspaceId: string) => {
    if (!activeAgentId) return;
    onSelectFeature("chat");
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
        onClick={() => {
          onSelectFeature("chat");
          setActiveSessionId(sess.id);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          setCtxMenu({ sessionId: sess.id, x: e.clientX, y: e.clientY, isPinned: !!sess.pinned, title: sess.title });
        }}
        className={`agnes-session-item flex w-full items-center gap-2 rounded-xl px-2.5 py-2 text-left text-xs transition-all duration-150 ${
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
      className={`agnes-sidebar ${isOpen ? "agnes-sidebar--open" : "agnes-sidebar--collapsed"} flex h-full shrink-0 flex-col overflow-hidden border-r border-stone-200/80 bg-stone-100/60 backdrop-blur-md transition-[width] duration-300 ${
        isOpen ? "w-72" : "w-[68px]"
      }`}
    >
      {/* Active agent */}
      {activeAgent && (
        <div className={`shrink-0 border-b border-stone-200/80 ${isOpen ? "p-4" : "px-3 py-4"}`}>
          <div className={`flex items-center ${isOpen ? "gap-3" : "flex-col gap-2"}`}>
            {!isOpen && (
              <button onClick={onToggleSidebar} className="grid h-8 w-8 shrink-0 place-items-center rounded-full text-stone-500 hover:bg-stone-200/70 hover:text-stone-900" title="展开侧边栏">
                <PanelLeftOpen className="h-4 w-4" />
              </button>
            )}
            <AgentAvatar name={activeAgent.name} avatar={activeAgent.avatar} size={isOpen ? 40 : 36} />
            <div className={`min-w-0 overflow-hidden transition-opacity ${isOpen ? "opacity-100" : "hidden opacity-0"}`}>
              <span className="block truncate text-sm font-semibold text-stone-900">{activeAgent.name}</span>
              {activeModelName ? (
                <span className="inline-block max-w-full truncate rounded border border-emerald-200/50 bg-emerald-50/80 px-1.5 py-0.5 align-middle font-mono text-[10px] text-emerald-700" title={activeAgent.model}>
                  {activeModelName}
                </span>
              ) : (
                <span className="inline-block rounded border border-stone-300/40 bg-stone-200/60 px-1.5 py-0.5 font-mono text-[10px] text-stone-500">
                  未配置模型
                </span>
              )}
            </div>
            <div className={isOpen ? "ml-auto flex items-center gap-1" : "flex items-center"}>
              <NotificationCenter onNavigate={onNotificationNavigate} />
              {isOpen && (
                <button onClick={onToggleSidebar} className="grid h-8 w-8 place-items-center rounded-full text-stone-500 hover:bg-stone-200/70 hover:text-stone-900" title="收起侧边栏">
                  <PanelLeftClose className="h-4 w-4" />
                </button>
              )}
            </div>
          </div>
        </div>
      )}

      <div className={isOpen ? "px-3 pt-3" : "px-2 pt-3"}>
        <button
          type="button"
          onClick={handleAddStandaloneSession}
          disabled={!activeAgentId}
          className={`agnes-sidebar-primary-action flex w-full items-center disabled:cursor-not-allowed disabled:opacity-40 ${
            isOpen ? "gap-2 px-3" : "justify-center"
          }`}
          title={isOpen ? undefined : "新建对话"}
        >
          <Plus className="h-4 w-4 shrink-0" />
          {isOpen && <span>新建对话</span>}
        </button>
      </div>

      {/* Feature navigation remains visible in compact mode. */}
      <nav className={`shrink-0 border-b border-stone-200/80 ${isOpen ? "p-3" : "px-2 py-3"}`} aria-label="功能">
        {isOpen && (
          <div className="mb-1.5 px-2 text-[10px] font-medium text-stone-400">
            功能
          </div>
        )}
        <div className="relative space-y-1">
          <span
            className="agnes-feature-highlight pointer-events-none absolute inset-x-0 top-0 z-0 h-10 rounded-xl"
            style={{ transform: `translateY(${activeFeatureIndex * 44}px)` }}
            aria-hidden="true"
          />
          {ENABLED_APP_FEATURES.map((feature) => {
            const Icon = FEATURE_ICONS[feature.id];
            const selected = feature.id === activeFeature;
            return (
              <button
                key={feature.id}
                onClick={() => onSelectFeature(feature.id)}
                className={`agnes-feature-item relative z-10 flex h-10 w-full items-center rounded-xl transition-colors ${
                  isOpen ? "gap-2.5 px-3" : "justify-center"
                } ${
                  selected
                    ? "font-semibold text-emerald-700"
                    : "text-stone-500 hover:bg-stone-200/50 hover:text-stone-900"
                }`}
                title={isOpen ? undefined : feature.label}
                aria-current={selected ? "page" : undefined}
              >
                <Icon className="h-[18px] w-[18px] shrink-0" />
                {isOpen && <span className="truncate text-xs">{feature.label}</span>}
              </button>
            );
          })}
        </div>
      </nav>

      {/* Session navigation is hidden when the sidebar becomes an icon rail. */}
      {isOpen && (
        <div className="agnes-session-list flex-1 space-y-4 overflow-y-auto p-4">
          <section>
            <div className="mb-2 flex items-center gap-1 px-1 text-[10px] font-medium text-stone-400">
              <button
                onClick={() => setStandaloneExpanded((expanded) => !expanded)}
                className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-1 py-1 text-left hover:bg-stone-200/50 hover:text-stone-600"
                aria-expanded={standaloneExpanded}
              >
                {standaloneExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                <span className="truncate">聊天会话</span>
                <span className="font-medium text-stone-300">{standaloneSessions.length}</span>
              </button>
              <button onClick={handleAddStandaloneSession} className="rounded-md p-1 text-stone-500 transition-colors hover:bg-stone-200/60 hover:text-stone-900" title="新建对话">
                <Plus className="h-3.5 w-3.5" />
              </button>
            </div>
            {standaloneExpanded && (
              <div className="space-y-1">
                {standaloneSessions.map(renderSessionBtn)}
                {standaloneSessions.length === 0 && (
                  <div className="py-3 text-center text-[11px] text-stone-400">无对话</div>
                )}
              </div>
            )}
          </section>

          <section>
            <div className="mb-2 flex items-center gap-1 px-1 text-[10px] font-medium text-stone-400">
              <button
                onClick={() => setWorkspacesExpanded((expanded) => !expanded)}
                className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-1 py-1 text-left hover:bg-stone-200/50 hover:text-stone-600"
                aria-expanded={workspacesExpanded}
              >
                {workspacesExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                <span className="truncate">工作区会话</span>
                <span className="font-medium text-stone-300">{agentWorkspaces.length}</span>
              </button>
              <button onClick={handleAddWorkspace} className="rounded-md p-1 text-stone-500 transition-colors hover:bg-stone-200/60 hover:text-stone-900" title="添加工作区（选择文件夹）">
                <FolderPlus className="h-3.5 w-3.5" />
              </button>
            </div>
            {workspacesExpanded && (
              <div className="space-y-1">
                {agentWorkspaces.length === 0 && (
                  <div className="py-3 text-center text-[11px] leading-relaxed text-stone-400">
                    无工作区，点击右上角添加文件夹
                  </div>
                )}
                {agentWorkspaces.map((ws) => {
                  const expanded = expandedWs.has(ws.id);
                  const wsSessions = sessions.filter((s) => s.workspace_id === ws.id);
                  return (
                    <div key={ws.id}>
                      <div
                        className="flex w-full cursor-pointer items-center gap-1.5 rounded-xl px-2 py-1.5 text-left text-xs text-stone-700 transition-colors hover:bg-stone-200/40"
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
                          className="shrink-0 text-stone-400 hover:text-stone-900"
                          title="在工作区中新建会话"
                        >
                          <Plus className="h-3 w-3" />
                        </button>
                      </div>
                      {expanded && (
                        <div className="ml-3 mt-0.5 space-y-1 border-l border-stone-200/60 pl-2">
                          {wsSessions.map(renderSessionBtn)}
                          {wsSessions.length === 0 && (
                            <div className="py-2 text-center text-[10px] text-stone-400">无会话</div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </section>
        </div>
      )}

      {/* Local status and settings */}
      <div className={`mt-auto shrink-0 border-t border-stone-200 bg-stone-200/20 ${
        isOpen ? "flex items-center justify-between p-3" : "flex flex-col items-center gap-2 py-3"
      }`}>
        <div className={`flex items-center gap-2 overflow-hidden ${isOpen ? "mr-2" : "h-8 justify-center"}`} title="本地 Sidecar 已就绪">
          <Cloud className={`shrink-0 text-emerald-600 ${isOpen ? "h-3.5 w-3.5" : "h-4 w-4"}`} />
          {isOpen && <span className="truncate text-[10px] text-stone-500">本地服务已就绪</span>}
        </div>
        <button
          onClick={() => onOpenSettings("agents")}
          className="flex h-8 w-8 shrink-0 items-center justify-center rounded-xl border border-stone-200 bg-white text-stone-500 shadow-sm transition-colors hover:text-stone-900"
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
