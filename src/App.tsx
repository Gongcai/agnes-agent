import { Suspense, lazy, useState, useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { AppTitleBar } from "./components/AppTitleBar";
import { ChatWorkspace } from "./components/ChatWorkspace";
import { KnowledgeWorkspace } from "./components/KnowledgeWorkspace";
import { PlannerWorkspace } from "./components/PlannerWorkspace";
import { SettingsModal } from "./components/SettingsModal";
import { AppContextMenu } from "./components/AppContextMenu";
import { type AppNotification } from "./components/NotificationCenter";
import { APP_FEATURES, type AppFeatureId, type ChatMode } from "./lib/features";
import { useAgentStore, setupTauriEventListeners } from "./store/useAgentStore";
import { invoke } from "@tauri-apps/api/core";
import {
  announceUIPreferenceChange,
  applyColorScheme,
  normalizeBooleanPreference,
  normalizeColorScheme,
  setAutoFollowStreaming,
  setAutoExpandThoughts,
  UI_AUTO_FOLLOW_STREAMING_KEY,
  UI_AUTO_EXPAND_THOUGHTS_KEY,
  UI_COLOR_SCHEME_KEY,
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
  const [isSidebarOpen, setIsSidebarOpen] = useState<boolean>(true);
  const [activeFeature, setActiveFeature] = useState<AppFeatureId>("chat");
  const [chatMode, setChatMode] = useState<ChatMode>("home");
  const [requestedPlannerTaskId, setRequestedPlannerTaskId] = useState<string | null>(null);
  const [requestedPlannerEventId, setRequestedPlannerEventId] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState<boolean>(false);
  const [settingsTab, setSettingsTab] = useState<"profile" | "general" | "agents" | "memory" | "storage" | "models" | "sync" | "tokens" | "web" | "mcp" | "skills" | "audit" | "debug">("agents");

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
    ])
      .then(([colorSchemeValue, autoExpandThoughtsValue, autoFollowStreamingValue]) => {
        if (!active) return;
        const colorScheme = normalizeColorScheme(colorSchemeValue);
        const autoExpandThoughts = normalizeBooleanPreference(autoExpandThoughtsValue, true);
        const autoFollowStreaming = normalizeBooleanPreference(autoFollowStreamingValue, true);
        applyColorScheme(colorScheme);
        setAutoExpandThoughts(autoExpandThoughts);
        setAutoFollowStreaming(autoFollowStreaming);
        announceUIPreferenceChange({ colorScheme, autoExpandThoughts, autoFollowStreaming });
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
    startDraftSession(activeAgentId, workspaceId);
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
    <div className="agnes-app flex h-screen w-screen flex-col overflow-hidden bg-[#FAF9F5] text-[#2e2e38] antialiased selection:bg-orange-100 selection:text-stone-900">
      <AppTitleBar
        title={title}
        isSidebarOpen={isSidebarOpen}
        onToggleSidebar={() => setIsSidebarOpen((open) => !open)}
      />

      <div className="flex min-h-0 flex-1 overflow-hidden">
        <Sidebar
          isOpen={isSidebarOpen}
          activeFeature={activeFeature}
          chatMode={chatMode}
          onSelectChatMode={handleSelectChatMode}
          onStartConversation={handleStartConversation}
          onSelectFeature={setActiveFeature}
          onOpenSettings={handleOpenSettings}
          onNotificationNavigate={handleNotificationNavigate}
        />

        {/* Feature view host. New local features are mounted here when enabled. */}
        {activeFeature === "chat" && hasVisibleChatSession && (
          <ChatWorkspace onOpenSettings={handleOpenSettings} />
        )}
        {activeFeature === "chat" && !hasVisibleChatSession && (
          <main className="agnes-chat-workspace grid min-w-0 flex-1 place-items-center bg-white px-8 text-center">
            <div className="max-w-sm">
              <div className="mx-auto mb-4 grid h-11 w-11 place-items-center rounded-lg bg-stone-100 font-mono text-sm text-stone-500">
                {chatMode === "code" ? "</>" : "+"}
              </div>
              <h1 className="text-2xl font-normal text-stone-800">
                {chatMode === "code" ? "选择一个代码工作区" : "开始一段新对话"}
              </h1>
              <p className="mt-2 text-sm leading-6 text-stone-500">
                {chatMode === "code"
                  ? "从左侧添加项目文件夹，或在已有工作区中新建会话。"
                  : "使用左侧的新对话按钮创建日常会话。"}
              </p>
            </div>
          </main>
        )}
        {activeFeature === "knowledge" && <KnowledgeWorkspace />}
        {activeFeature === "reading" && (
          <Suspense fallback={<main className="grid min-w-0 flex-1 place-items-center text-sm text-stone-400">加载阅读器...</main>}>
            <ReadingWorkspace />
          </Suspense>
        )}
        {activeFeature === "drive" && (
          <Suspense fallback={<main className="grid min-w-0 flex-1 place-items-center text-sm text-stone-400">加载网盘...</main>}>
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
      <AppContextMenu />
    </div>
  );
}
