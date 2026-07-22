import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Brain,
  BookOpen,
  Code,
  CalendarDots as CalendarDays,
  CheckSquare as CheckSquare2,
  CaretDown as ChevronDown,
  CaretRight as ChevronRight,
  CaretUp as ChevronUp,
  Database,
  Folder,
  FolderPlus,
  HardDrive,
  House,
  ChatsTeardrop as MessageSquare,
  PencilSimple as Pencil,
  PushPinSimple as Pin,
  PushPinSimpleSlash as PinOff,
  Plus,
  GearSix as Settings,
  Trash as Trash2,
  type Icon as PhosphorIcon,
} from "@phosphor-icons/react";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { ENABLED_APP_FEATURES, type AppFeatureId, type ChatMode } from "../lib/features";
import { useAgentStore } from "../store/useAgentStore";
import { NotificationCenter, type AppNotification } from "./NotificationCenter";

type SettingsTab = "profile" | "general" | "agents" | "memory" | "storage" | "llm" | "tokens" | "web" | "mcp" | "skills" | "audit" | "debug";

interface SidebarUserProfile {
  avatar: string;
  name: string;
}

interface SidebarProps {
  isOpen: boolean;
  activeFeature: AppFeatureId;
  chatMode: ChatMode;
  onSelectChatMode: (mode: ChatMode) => void;
  onSelectFeature: (feature: AppFeatureId) => void;
  onOpenSettings: (tab?: SettingsTab) => void;
  onNotificationNavigate: (notification: AppNotification) => void | Promise<void>;
}

const FEATURE_ICONS: Record<AppFeatureId, PhosphorIcon> = {
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
  chatMode,
  onSelectChatMode,
  onSelectFeature,
  onOpenSettings,
  onNotificationNavigate,
}) => {
  const {
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

  // Standalone sessions do not belong to a workspace.
  const standaloneSessions = sessions.filter((s) => s.agent_id === activeAgentId && !s.workspace_id);
  const agentWorkspaces = workspaces.filter((w) => w.agent_id === activeAgentId);

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
  const [moreExpanded, setMoreExpanded] = useState(() =>
    readLocalBoolean("agnes.ui.sidebar.more-expanded", false),
  );
  const [accountMenuOpen, setAccountMenuOpen] = useState(false);
  const [userProfile, setUserProfile] = useState<SidebarUserProfile>({ avatar: "", name: "" });

  useEffect(() => {
    let active = true;
    const loadUserProfile = () => {
      invoke<SidebarUserProfile>("get_user_profile")
        .then((profile) => {
          if (active) setUserProfile(profile);
        })
        .catch(console.error);
    };
    loadUserProfile();
    window.addEventListener("agnes:user-profile-change", loadUserProfile);
    return () => {
      active = false;
      window.removeEventListener("agnes:user-profile-change", loadUserProfile);
    };
  }, []);
  const sidebarRef = useRef<HTMLElement>(null);
  const [featureHighlight, setFeatureHighlight] = useState<{
    x: number;
    y: number;
    width: number;
    height: number;
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

  useEffect(() => {
    writeLocalBoolean("agnes.ui.sidebar.more-expanded", moreExpanded);
  }, [moreExpanded]);

  useEffect(() => {
    if (activeFeature !== "chat" && activeFeature !== "drive") setMoreExpanded(true);
  }, [activeFeature]);

  useLayoutEffect(() => {
    const sidebar = sidebarRef.current;
    if (!sidebar) return;

    let frame = 0;
    let stopped = false;
    const startedAt = performance.now();

    const measure = () => {
      if (stopped) return;
      const target = activeFeature === "chat"
        ? null
        : sidebar.querySelector<HTMLElement>(`[data-feature-id="${activeFeature}"]`);

      if (!target) {
        if (activeFeature === "chat") setFeatureHighlight(null);
        return;
      }

      const sidebarRect = sidebar.getBoundingClientRect();
      const targetRect = target.getBoundingClientRect();
      setFeatureHighlight({
        x: targetRect.left - sidebarRect.left,
        y: targetRect.top - sidebarRect.top,
        width: targetRect.width,
        height: targetRect.height,
      });
    };

    const followLayoutTransition = (timestamp: number) => {
      measure();
      if (timestamp - startedAt < 360) frame = requestAnimationFrame(followLayoutTransition);
    };

    frame = requestAnimationFrame(followLayoutTransition);
    const resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(sidebar);
    window.addEventListener("resize", measure);

    return () => {
      stopped = true;
      cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, [activeFeature, isOpen, moreExpanded]);

  // Expand newly loaded workspace groups by default.
  useEffect(() => {
    setExpandedWs(new Set(agentWorkspaces.map((w) => w.id)));
  }, [agentWorkspaces.length]);

  const handleAddStandaloneSession = () => {
    if (!activeAgentId) return;
    onSelectFeature("chat");
    createSession(activeAgentId, `新会话 #${standaloneSessions.length + 1}`, null).catch(console.error);
  };

  const handleNewConversation = () => {
    if (chatMode === "code") {
      const workspace = agentWorkspaces[0];
      if (workspace) {
        handleAddWorkspaceSession(workspace.id);
      } else {
        setMoreExpanded(true);
        onSelectFeature("drive");
      }
      return;
    }
    handleAddStandaloneSession();
  };

  const toggleAccountMenu = () => {
    setAccountMenuOpen((open) => !open);
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
          setAccountMenuOpen(false);
          setWsCtxMenu(null);
          setCtxMenu({ sessionId: sess.id, x: e.clientX, y: e.clientY, isPinned: !!sess.pinned, title: sess.title });
        }}
        className={`agnes-session-item flex w-full items-center gap-2 rounded-xl px-2.5 py-2 text-left text-xs transition-all duration-150 ${
          isActive
            ? "bg-white text-emerald-700 font-semibold border border-stone-200 shadow-[0_1px_2px_0_rgba(0,0,0,0.03)]"
            : "text-stone-600 hover:bg-stone-200/40 hover:text-stone-900"
        }`}
      >
        <MessageSquare className="h-3.5 w-3.5 shrink-0 text-stone-400" />
        <span className="flex-1 truncate">{sess.title}</span>
        {sess.pinned && <Pin className="h-3 w-3 shrink-0 text-amber-500" />}
      </button>
    );
  };

  return (
    <aside
      ref={sidebarRef}
      data-open={isOpen}
      className={`agnes-sidebar ${isOpen ? "agnes-sidebar--open" : "agnes-sidebar--collapsed"} flex h-full shrink-0 flex-col overflow-hidden border-r border-stone-200/80 bg-stone-100/60 backdrop-blur-md transition-[width] duration-300 ${
        isOpen ? "w-72" : "w-12"
      }`}
    >
      <div
        className="agnes-feature-highlight"
        aria-hidden="true"
        style={featureHighlight ? {
          width: featureHighlight.width,
          height: featureHighlight.height,
          opacity: 1,
          transform: `translate3d(${featureHighlight.x}px, ${featureHighlight.y}px, 0)`,
        } : { opacity: 0 }}
      />
      <div className="agnes-sidebar-top px-3 pt-3">
        <div
          className="agnes-mode-switch mb-2 flex rounded-lg bg-stone-100 p-0.5"
          data-mode={chatMode}
          role="tablist"
          aria-label="会话类型"
        >
          <span className="agnes-mode-highlight" aria-hidden="true" />
          <button
            type="button"
            role="tab"
            aria-selected={chatMode === "home"}
            onClick={() => onSelectChatMode("home")}
            className={`agnes-mode-tab flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-md py-1.5 text-xs transition-colors ${chatMode === "home" ? "font-medium text-stone-800" : "text-stone-500 hover:text-stone-800"}`}
            title="日常会话"
          >
            <House className="h-3.5 w-3.5 shrink-0" />
            <span className="agnes-sidebar-label">Home</span>
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={chatMode === "code"}
            onClick={() => onSelectChatMode("code")}
            className={`agnes-mode-tab flex min-w-0 flex-1 items-center justify-center gap-1.5 rounded-md py-1.5 text-xs transition-colors ${chatMode === "code" ? "font-medium text-stone-800" : "text-stone-500 hover:text-stone-800"}`}
            title="编程会话"
          >
            <Code className="h-3.5 w-3.5 shrink-0" />
            <span className="agnes-sidebar-label">Code</span>
          </button>
        </div>
        <button
          type="button"
          onClick={handleNewConversation}
          disabled={!activeAgentId}
          className="agnes-sidebar-primary-action flex w-full items-center gap-2 px-3 disabled:cursor-not-allowed disabled:opacity-40"
          title={isOpen ? undefined : "新建对话"}
        >
          <span className="agnes-sidebar-primary-icon grid h-6 w-6 shrink-0 place-items-center rounded-full">
            <Plus className="h-4 w-4" weight="regular" />
          </span>
          <span className="agnes-sidebar-label">新建对话</span>
        </button>
        <button
          type="button"
          onClick={() => onSelectFeature("drive")}
          data-feature-id="drive"
          className={`agnes-sidebar-primary-action mt-1 flex w-full items-center gap-2 px-3 ${activeFeature === "drive" ? "font-medium text-stone-900" : ""}`}
          title={isOpen ? undefined : "网盘"}
          aria-current={activeFeature === "drive" ? "page" : undefined}
        >
          <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full text-stone-500">
            <HardDrive className="h-4 w-4" />
          </span>
          <span className="agnes-sidebar-label">网盘</span>
        </button>
      </div>

      <nav className="agnes-sidebar-nav shrink-0 border-b border-stone-200/80 px-3 pb-3 pt-1" aria-label="更多功能">
        {moreExpanded && (
          <div className="mb-1 space-y-1">
            {ENABLED_APP_FEATURES.filter((feature) => feature.id !== "chat" && feature.id !== "drive").map((feature) => {
              const Icon = FEATURE_ICONS[feature.id];
              const selected = feature.id === activeFeature;
              return (
                <button
                  key={feature.id}
                  onClick={() => onSelectFeature(feature.id)}
                  data-feature-id={feature.id}
                  className={`agnes-sidebar-primary-action flex h-9 w-full items-center gap-2 px-3 ${selected ? "font-medium text-stone-900" : ""}`}
                  title={isOpen ? undefined : feature.label}
                  aria-current={selected ? "page" : undefined}
                >
                  <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full text-stone-500">
                    <Icon className="h-4 w-4" weight="regular" />
                  </span>
                  <span className="agnes-sidebar-label truncate text-xs">{feature.label}</span>
                </button>
              );
            })}
          </div>
        )}
        <button
          type="button"
          onClick={() => setMoreExpanded((expanded) => !expanded)}
          className="agnes-sidebar-primary-action flex h-9 w-full items-center gap-2 px-3"
          aria-expanded={moreExpanded}
          title={moreExpanded ? "收起更多功能" : "展开更多功能"}
        >
          <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full text-stone-500">
            {moreExpanded ? <ChevronDown className="h-4 w-4" /> : <ChevronRight className="h-4 w-4" />}
          </span>
          <span className="agnes-sidebar-label">更多功能</span>
        </button>
      </nav>

      {/* Session navigation is hidden when the sidebar becomes an icon rail. */}
      <div className="agnes-session-list flex-1 space-y-4 overflow-y-auto p-4" aria-hidden={!isOpen}>
        {chatMode === "home" ? (
          <section>
            <div className="mb-2 flex items-center gap-1 px-1 text-[10px] font-medium text-stone-400">
              <button
                onClick={() => setStandaloneExpanded((expanded) => !expanded)}
                className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-1 py-1 text-left hover:bg-stone-200/50 hover:text-stone-600"
                aria-expanded={standaloneExpanded}
              >
                {standaloneExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                <span className="truncate">最近对话</span>
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
        ) : (
          <section>
            <div className="mb-2 flex items-center gap-1 px-1 text-[10px] font-medium text-stone-400">
              <button
                onClick={() => setWorkspacesExpanded((expanded) => !expanded)}
                className="flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-1 py-1 text-left hover:bg-stone-200/50 hover:text-stone-600"
                aria-expanded={workspacesExpanded}
              >
                {workspacesExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
                <span className="truncate">代码工作区</span>
                <span className="font-medium text-stone-300">{agentWorkspaces.length}</span>
              </button>
              <button onClick={handleAddWorkspace} className="rounded-md p-1 text-stone-500 transition-colors hover:bg-stone-200/60 hover:text-stone-900" title="添加工作区（选择文件夹）">
                <FolderPlus className="h-3.5 w-3.5" />
              </button>
            </div>
            {workspacesExpanded && (
              <div className="space-y-1">
                {agentWorkspaces.length === 0 && (
                  <div className="rounded-lg border border-dashed border-stone-200 px-3 py-4 text-center text-[11px] leading-relaxed text-stone-400">
                    <p>还没有代码工作区</p>
                    <button
                      type="button"
                      onClick={handleAddWorkspace}
                      className="mt-2 inline-flex items-center gap-1 rounded-md px-2 py-1 text-stone-600 hover:bg-stone-100 hover:text-stone-900"
                    >
                      <FolderPlus className="h-3.5 w-3.5" />
                      添加项目
                    </button>
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
                          setAccountMenuOpen(false);
                          setCtxMenu(null);
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
        )}
      </div>

      {/* Account entry */}
      <div className="agnes-sidebar-account relative mt-auto shrink-0 border-t border-stone-200 bg-stone-200/20 px-2 py-1">
        <div className="agnes-account-actions" data-open={accountMenuOpen}>
          <div className="agnes-account-actions-inner space-y-1">
            <NotificationCenter
              onNavigate={onNotificationNavigate}
              className="w-full"
              triggerVariant="menu"
            />
            <button
              type="button"
              onClick={() => {
                setAccountMenuOpen(false);
                onOpenSettings("profile");
              }}
              className="agnes-account-action flex h-9 w-full items-center gap-2 rounded-lg px-2 text-left text-xs text-stone-600 transition-colors"
            >
              <span className="agnes-account-action-icon grid h-7 w-7 shrink-0 place-items-center">
                <Settings className="h-4 w-4" />
              </span>
              <span className="agnes-sidebar-label">设置</span>
            </button>
          </div>
        </div>
        <button
          type="button"
          onClick={toggleAccountMenu}
          className="agnes-account-trigger flex h-9 w-full items-center gap-2 rounded-lg px-2 text-left transition-colors hover:bg-stone-100"
          aria-expanded={accountMenuOpen}
          aria-label="打开账户菜单"
        >
          <span className="grid h-6 w-6 shrink-0 place-items-center overflow-hidden rounded-full bg-stone-300 text-[11px] font-semibold text-stone-700">
            {userProfile.avatar
              ? <img src={userProfile.avatar} alt="" className="h-full w-full object-cover" />
              : userProfile.name.trim().slice(0, 1).toUpperCase() || "A"}
          </span>
          <span className="agnes-sidebar-label min-w-0 flex-1 truncate text-xs font-medium text-stone-700">
            {userProfile.name.trim() || "AGENS"}
          </span>
          {accountMenuOpen
            ? <ChevronUp className="agnes-account-chevron h-3.5 w-3.5 shrink-0 text-stone-400" />
            : <ChevronDown className="agnes-account-chevron h-3.5 w-3.5 shrink-0 text-stone-400" />}
        </button>
      </div>

      {/* 会话右键菜单 */}
      {ctxMenu && createPortal(
        <>
          <div className="fixed inset-0 z-40" onClick={closeCtxMenu} onContextMenu={(e) => { e.preventDefault(); closeCtxMenu(); }} />
          <div
            className="claude-popover fixed z-50 w-40 rounded-xl border border-stone-200 bg-white py-1 text-xs text-stone-700 shadow-2xl"
            style={{
              top: Math.min(Math.max(ctxMenu.y, 8), Math.max(8, window.innerHeight - 104)),
              left: Math.min(Math.max(ctxMenu.x, 8), Math.max(8, window.innerWidth - 168)),
            }}
          >
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
        </>,
        document.body,
      )}

      {/* 工作区右键菜单 */}
      {wsCtxMenu && createPortal(
        <>
          <div className="fixed inset-0 z-40" onClick={closeWsCtxMenu} onContextMenu={(e) => { e.preventDefault(); closeWsCtxMenu(); }} />
          <div
            className="claude-popover fixed z-50 w-40 rounded-xl border border-stone-200 bg-white py-1 text-xs text-stone-700 shadow-2xl"
            style={{
              top: Math.min(Math.max(wsCtxMenu.y, 8), Math.max(8, window.innerHeight - 76)),
              left: Math.min(Math.max(wsCtxMenu.x, 8), Math.max(8, window.innerWidth - 168)),
            }}
          >
            <button onClick={handleWsCtxRename} className="w-full flex items-center gap-2 px-3 py-1.5 text-left hover:bg-stone-100 transition-colors">
              <Pencil className="h-3.5 w-3.5 text-stone-500" />重命名
            </button>
            <button onClick={handleWsCtxDelete} className="w-full flex items-center gap-2 px-3 py-1.5 text-left text-rose-600 hover:bg-rose-50 transition-colors">
              <Trash2 className="h-3.5 w-3.5" />删除工作区
            </button>
          </div>
        </>,
        document.body,
      )}
    </aside>
  );
};
