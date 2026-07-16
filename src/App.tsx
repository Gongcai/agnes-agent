import { useState, useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatWorkspace } from "./components/ChatWorkspace";
import { SettingsModal } from "./components/SettingsModal";
import type { AppFeatureId } from "./lib/features";
import { useAgentStore, setupTauriEventListeners } from "./store/useAgentStore";

export default function App() {
  const { init } = useAgentStore();
  const [isSidebarOpen, setIsSidebarOpen] = useState<boolean>(true);
  const [activeFeature, setActiveFeature] = useState<AppFeatureId>("chat");
  const [isSettingsOpen, setIsSettingsOpen] = useState<boolean>(false);
  const [settingsTab, setSettingsTab] = useState<"general" | "agents" | "memory" | "llm" | "audit" | "debug">("agents");

  // 启动时初始化：恢复上次 agent/session 或按设置新建，并绑定 Tauri 事件桥
  useEffect(() => {
    init().catch(console.error);

    // Register active listeners for streams/tools/runs
    const cleanup = setupTauriEventListeners();

    return () => {
      cleanup().catch(console.error);
    };
  }, [init]);

  const handleOpenSettings = (tab: "general" | "agents" | "memory" | "llm" | "audit" | "debug" = "agents") => {
    setSettingsTab(tab);
    setIsSettingsOpen(true);
  };

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-[#FAF9F5] text-[#2e2e38] antialiased selection:bg-emerald-100 selection:text-emerald-900">
      {/* Collapsible Left Sidebar */}
      <Sidebar
        isOpen={isSidebarOpen}
        activeFeature={activeFeature}
        onSelectFeature={setActiveFeature}
        onOpenSettings={handleOpenSettings}
      />

      {/* Feature view host. New local features are mounted here when enabled. */}
      {activeFeature === "chat" && (
        <ChatWorkspace
          isSidebarOpen={isSidebarOpen}
          onToggleSidebar={() => setIsSidebarOpen((open) => !open)}
          onOpenSettings={handleOpenSettings}
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
