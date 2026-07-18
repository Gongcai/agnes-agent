import { Suspense, lazy, useState, useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatWorkspace } from "./components/ChatWorkspace";
import { KnowledgeWorkspace } from "./components/KnowledgeWorkspace";
import { PlannerWorkspace } from "./components/PlannerWorkspace";
import { SettingsModal } from "./components/SettingsModal";
import { type AppNotification } from "./components/NotificationCenter";
import type { AppFeatureId } from "./lib/features";
import { useAgentStore, setupTauriEventListeners } from "./store/useAgentStore";
import { invoke } from "@tauri-apps/api/core";

const ReadingWorkspace = lazy(() =>
  import("./components/ReadingWorkspace").then((module) => ({ default: module.ReadingWorkspace })),
);

export default function App() {
  const { init, agents, setActiveAgentId, setActiveSessionId } = useAgentStore();
  const [isSidebarOpen, setIsSidebarOpen] = useState<boolean>(true);
  const [activeFeature, setActiveFeature] = useState<AppFeatureId>("chat");
  const [requestedPlannerTaskId, setRequestedPlannerTaskId] = useState<string | null>(null);
  const [requestedPlannerEventId, setRequestedPlannerEventId] = useState<string | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState<boolean>(false);
  const [settingsTab, setSettingsTab] = useState<"general" | "agents" | "memory" | "llm" | "tokens" | "mcp" | "audit" | "debug">("agents");

  // 启动时初始化：恢复上次 agent/session 或按设置新建，并绑定 Tauri 事件桥
  useEffect(() => {
    init().catch(console.error);

    // Register active listeners for streams/tools/runs
    const cleanup = setupTauriEventListeners();

    return () => {
      cleanup().catch(console.error);
    };
  }, [init]);

  const handleOpenSettings = (tab: "general" | "agents" | "memory" | "llm" | "tokens" | "mcp" | "audit" | "debug" = "agents") => {
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
          sessions: await invoke<{ id: string }[]>("list_sessions", { agentId: agent.id }),
        })),
      );
      const owner = sessionGroups.find((group) => group.sessions.some((session) => session.id === targetId));
      if (owner) await setActiveAgentId(owner.agentId);
      await setActiveSessionId(targetId);
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

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-[#FAF9F5] text-[#2e2e38] antialiased selection:bg-emerald-100 selection:text-emerald-900">
      {/* Collapsible Left Sidebar */}
      <Sidebar
        isOpen={isSidebarOpen}
        activeFeature={activeFeature}
        onSelectFeature={setActiveFeature}
        onToggleSidebar={() => setIsSidebarOpen((open) => !open)}
        onOpenSettings={handleOpenSettings}
        onNotificationNavigate={handleNotificationNavigate}
      />

      {/* Feature view host. New local features are mounted here when enabled. */}
      {activeFeature === "chat" && (
        <ChatWorkspace
          onOpenSettings={handleOpenSettings}
        />
      )}
      {activeFeature === "knowledge" && (
        <KnowledgeWorkspace />
      )}
      {activeFeature === "reading" && (
        <Suspense fallback={<main className="grid min-w-0 flex-1 place-items-center text-sm text-stone-400">加载阅读器...</main>}>
          <ReadingWorkspace />
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

      {/* Configuration Modal Panels */}
      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
        initialTab={settingsTab}
      />
    </div>
  );
}
