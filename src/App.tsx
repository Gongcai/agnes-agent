import { Suspense, lazy, useState, useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { AppTitleBar } from "./components/AppTitleBar";
import { ChatWorkspace } from "./components/ChatWorkspace";
import { KnowledgeWorkspace } from "./components/KnowledgeWorkspace";
import { PlannerWorkspace } from "./components/PlannerWorkspace";
import { SettingsModal } from "./components/SettingsModal";
import { AppContextMenu } from "./components/AppContextMenu";
import { CommandPalette } from "./components/CommandPalette";
import { EmptyState } from "./components/ui/empty-state";
import { Button } from "./components/ui/button";
import { Plus, BracketsCurly } from "@phosphor-icons/react";
import { type AppNotification } from "./components/NotificationCenter";
import { APP_FEATURES, type AppFeatureId, type ChatMode } from "./lib/features";
import { useAgentStore, setupTauriEventListeners } from "./store/useAgentStore";
import { invoke } from "@tauri-apps/api/core";
import {
  announceUIPreferenceChange,
  applyColorScheme,
  applyFontScale,
  normalizeBooleanPreference,
  normalizeColorScheme,
  normalizeFontScale,
  setAutoFollowStreaming,
  setAutoExpandThoughts,
  UI_AUTO_FOLLOW_STREAMING_KEY,
  UI_AUTO_EXPAND_THOUGHTS_KEY,
  UI_COLOR_SCHEME_KEY,
  UI_FONT_SCALE_KEY,
} from "./lib/uiPreferences";

const ReadingWorkspace = lazy(() =>
  import("./components/ReadingWorkspace").then((module) => ({ default: module.ReadingWorkspace })),
);
const DriveWorkspace = lazy(() =>
  import("./components/DriveWorkspace").then((module) => ({ default: module.DriveWorkspace })),
);

export default function App() {
  const {
    init,
    agents,
    sessions,
    activeAgentId,
    activeSessionId,
    draftSession,
    discardDraftSession,
    startDraftSession,
    setActiveAgentId,
    setActiveSessionId,
  } = useAgentStore();
  const [isSidebarOpen, setIsSidebarOpen] = useState<boolean>(() => (
    typeof window === "undefined" || !window.matchMedia("(max-width: 640px)").matches
  ));
  const [activeFeature, setActiveFeature] = useState<AppFeatureId>("chat");
  const [chatMode, setChatMode] = useState<ChatMode>("home");
  const [requestedPlannerTaskId, setRequestedPlannerTaskId] = useState<string | null>(null);
  const [requestedPlannerEventId, setRequestedPlannerEventId] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState<boolean>(false);
  const [isCommandPaletteOpen, setIsCommandPaletteOpen] = useState<boolean>(false);
  const [settingsTab, setSettingsTab] = useState<"profile" | "general" | "agents" | "memory" | "storage" | "models" | "sync" | "tokens" | "web" | "mcp" | "skills" | "audit" | "debug">("agents");

  // 全局 Cmd+K / Ctrl+K 快捷指令调起
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setIsCommandPaletteOpen((open) => !open);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  // 启动时初始化：恢复上次 agent/session 或按设置新建，并绑定 Tauri 事件桥
  useEffect(() => {
    init().catch(console.error);

    // Register active listeners for streams/tools/runs
    const cleanup = setupTauriEventListeners();

    return () => {
      cleanup().catch(console.error);
    };
  }, [init]);

  useEffect(() => {
    let active = true;
    Promise.all([
      invoke<string | null>("get_setting", { key: UI_COLOR_SCHEME_KEY }),
      invoke<string | null>("get_setting", { key: UI_AUTO_EXPAND_THOUGHTS_KEY }),
      invoke<string | null>("get_setting", { key: UI_AUTO_FOLLOW_STREAMING_KEY }),
      invoke<string | null>("get_setting", { key: UI_FONT_SCALE_KEY }),
    ])
      .then(([colorSchemeValue, autoExpandThoughtsValue, autoFollowStreamingValue, fontScaleValue]) => {
        if (!active) return;
        const colorScheme = normalizeColorScheme(colorSchemeValue);
        const autoExpandThoughts = normalizeBooleanPreference(autoExpandThoughtsValue, true);
        const autoFollowStreaming = normalizeBooleanPreference(autoFollowStreamingValue, true);
        const fontScale = normalizeFontScale(fontScaleValue);
        applyColorScheme(colorScheme);
        applyFontScale(fontScale);
        setAutoExpandThoughts(autoExpandThoughts);
        setAutoFollowStreaming(autoFollowStreaming);
        announceUIPreferenceChange({ colorScheme, fontScale, autoExpandThoughts, autoFollowStreaming });
      })
      .catch((error) => console.error("加载界面偏好失败", error));
    return () => { active = false; };
  }, []);

  useEffect(() => {
    const activeSession = sessions.find((session) => session.id === activeSessionId);
    if (activeSession) {
      setChatMode(activeSession.workspace_id ? "code" : "home");
    }
  }, [activeSessionId, sessions]);

  const handleSelectChatMode = (mode: ChatMode) => {
    discardDraftSession();
    setChatMode(mode);
    setActiveFeature("chat");
    if (window.matchMedia("(max-width: 640px)").matches) setIsSidebarOpen(false);
    const modeSessions = sessions.filter((session) => (
      session.agent_id === activeAgentId
      && (mode === "code" ? Boolean(session.workspace_id) : !session.workspace_id)
    ));
    if (!modeSessions.some((session) => session.id === activeSessionId) && modeSessions[0]) {
      setActiveSessionId(modeSessions[0].id).catch(console.error);
    }
  };

  const handleStartConversation = (workspaceId: string | null) => {
    if (!activeAgentId) return;
    setChatMode(workspaceId ? "code" : "home");
    setActiveFeature("chat");
    if (window.matchMedia("(max-width: 640px)").matches) setIsSidebarOpen(false);
    startDraftSession(activeAgentId, workspaceId);
  };

  const handleSelectFeature = (feature: AppFeatureId) => {
    setActiveFeature(feature);
    if (window.matchMedia("(max-width: 640px)").matches) setIsSidebarOpen(false);
  };

  const handleOpenSettings = (tab: "profile" | "general" | "agents" | "memory" | "storage" | "models" | "sync" | "tokens" | "web" | "mcp" | "skills" | "audit" | "debug" = "agents") => {
    setSettingsTab(tab);
    setIsSettingsOpen(true);
  };

  const handleNotificationNavigate = async (notification: AppNotification) => {
    const targetId = notification.target_id;
    if (!targetId) return;
    if (notification.target_kind === "chat") {
      // A completed background run may belong to another agent. Locate its
      // owner before selecting the precise session in the shared chat store.
      const sessionGroups = await Promise.all(
        agents.map(async (agent) => ({
          agentId: agent.id,
          sessions: await invoke<{ id: string; workspace_id: string | null }[]>("list_sessions", { agentId: agent.id }),
        })),
      );
      const owner = sessionGroups.find((group) => group.sessions.some((session) => session.id === targetId));
      if (owner) await setActiveAgentId(owner.agentId);
      await setActiveSessionId(targetId);
      const target = owner?.sessions.find((session) => session.id === targetId);
      setChatMode(target?.workspace_id ? "code" : "home");
      setActiveFeature("chat");
      return;
    }
    if (notification.target_kind === "task") {
      setRequestedPlannerEventId(null);
      setRequestedPlannerTaskId(targetId);
      setActiveFeature("tasks");
      return;
    }
    if (notification.target_kind === "calendar") {
      setRequestedPlannerTaskId(null);
      setRequestedPlannerEventId(targetId);
      setActiveFeature("calendar");
    }
  };

  const activeSession = sessions.find((session) => session.id === activeSessionId);
  const activeSessionMatchesMode = activeSession
    ? chatMode === "code" ? Boolean(activeSession.workspace_id) : !activeSession.workspace_id
    : false;
  const draftSessionMatchesMode = draftSession?.agentId === activeAgentId
    && (chatMode === "code" ? Boolean(draftSession.workspaceId) : !draftSession.workspaceId);
  const activeFeatureLabel = APP_FEATURES.find((feature) => feature.id === activeFeature)?.label ?? "Agnes";
  const title = activeFeature === "chat"
    ? activeSessionMatchesMode ? activeSession?.title || (chatMode === "code" ? "Code" : "Home") : (chatMode === "code" ? "Code" : "Home")
    : activeFeatureLabel;
  const hasVisibleChatSession = activeSessionMatchesMode || draftSessionMatchesMode;

  return (
    <div className="agnes-app flex h-screen w-screen flex-col overflow-hidden bg-canvas text-ink subpixel-antialiased selection:bg-coral/20 selection:text-ink">
      <AppTitleBar
        title={title}
        isSidebarOpen={isSidebarOpen}
        onToggleSidebar={() => setIsSidebarOpen((open) => !open)}
        onOpenCommandPalette={() => setIsCommandPaletteOpen(true)}
      />

      <div className="flex min-h-0 flex-1 overflow-hidden">
        {isSidebarOpen && (
          <button
            type="button"
            className="agnes-mobile-sidebar-backdrop"
            aria-label="关闭导航"
            onClick={() => setIsSidebarOpen(false)}
          />
        )}
        <Sidebar
          isOpen={isSidebarOpen}
          activeFeature={activeFeature}
          chatMode={chatMode}
          onSelectChatMode={handleSelectChatMode}
          onStartConversation={handleStartConversation}
          onSelectFeature={handleSelectFeature}
          onOpenSettings={handleOpenSettings}
          onNotificationNavigate={handleNotificationNavigate}
        />

        {/* Feature view host. New local features are mounted here when enabled. */}
        {activeFeature === "chat" && hasVisibleChatSession && (
          <ChatWorkspace onOpenSettings={handleOpenSettings} />
        )}
        {activeFeature === "chat" && !hasVisibleChatSession && (
          <main className="agnes-chat-workspace grid min-w-0 flex-1 place-items-center bg-canvas px-8 text-center">
            <EmptyState
              icon={chatMode === "code" ? <BracketsCurly className="h-6 w-6 text-coral" /> : <Plus className="h-6 w-6 text-coral" />}
              title={chatMode === "code" ? "选择或新建代码工作区" : "开启一段新的探索"}
              description={
                chatMode === "code"
                  ? "从左侧选择关联的项目文件夹，或直接在工作区中创建会话。"
                  : "点击下方按钮或左侧导航开启一段日常会话。"
              }
              action={
                <Button
                  variant="primary"
                  onClick={() => handleStartConversation(null)}
                >
                  <Plus className="mr-1.5 h-3.5 w-3.5" />
                  新建对话
                </Button>
              }
            />
          </main>
        )}
        {activeFeature === "knowledge" && <KnowledgeWorkspace />}
        {activeFeature === "reading" && (
          <Suspense fallback={<main className="grid min-w-0 flex-1 place-items-center text-sm text-ink-muted">加载阅读器...</main>}>
            <ReadingWorkspace />
          </Suspense>
        )}
        {activeFeature === "drive" && (
          <Suspense fallback={<main className="grid min-w-0 flex-1 place-items-center text-sm text-ink-muted">加载网盘...</main>}>
            <DriveWorkspace />
          </Suspense>
        )}
        {(activeFeature === "calendar" || activeFeature === "tasks") && (
          <PlannerWorkspace
            mode={activeFeature}
            requestedTaskId={requestedPlannerTaskId}
            requestedEventId={requestedPlannerEventId}
            onOpenTask={(taskId) => {
              setRequestedPlannerTaskId(taskId);
              setActiveFeature("tasks");
            }}
            onCloseRequestedTask={() => setRequestedPlannerTaskId(null)}
            onCloseRequestedEvent={() => setRequestedPlannerEventId(null)}
          />
        )}
      </div>

      {/* Configuration Modal Panels */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
        initialTab={settingsTab}
      />

      {/* Global Command Palette */}
      <CommandPalette
        isOpen={isCommandPaletteOpen}
        onClose={() => setIsCommandPaletteOpen(false)}
        onSelectFeature={handleSelectFeature}
        onSelectChatMode={handleSelectChatMode}
        onStartConversation={handleStartConversation}
        onOpenSettings={handleOpenSettings}
      />

      <AppContextMenu />
    </div>
  );
}
