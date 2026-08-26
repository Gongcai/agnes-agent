import React, { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  BookOpen,
  Calendar,
  CheckSquare,
  Database,
  HardDrive,
  MessageSquare,
  Moon,
  Plus,
  Search,
  Settings,
  Sun,
  X,
  Sparkles,
} from "lucide-react";
import { type AppFeatureId, type ChatMode } from "../lib/features";
import { useAgentStore } from "../store/useAgentStore";
import { applyColorScheme, getCachedColorScheme } from "../lib/uiPreferences";

interface CommandItem {
  id: string;
  title: string;
  subtitle?: string;
  category: string;
  icon: React.ReactNode;
  shortcut?: string;
  action: () => void;
}

interface CommandPaletteProps {
  isOpen: boolean;
  onClose: () => void;
  onSelectFeature: (feature: AppFeatureId) => void;
  onSelectChatMode: (mode: ChatMode) => void;
  onStartConversation: (workspaceId: string | null) => void;
  onOpenSettings: (tab?: any) => void;
}

export const CommandPalette: React.FC<CommandPaletteProps> = ({
  isOpen,
  onClose,
  onSelectFeature,
  onSelectChatMode,
  onStartConversation,
  onOpenSettings,
}) => {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const { sessions, activeAgentId, setActiveSessionId } = useAgentStore();

  useEffect(() => {
    if (isOpen) {
      setQuery("");
      setSelectedIndex(0);
      setTimeout(() => inputRef.current?.focus(), 50);
    }
  }, [isOpen]);

  const items: CommandItem[] = useMemo(() => {
    const list: CommandItem[] = [];

    // Quick Actions
    list.push({
      id: "action-new-chat",
      title: "新建会话",
      subtitle: "开启一段全新的 AI 对话",
      category: "快捷操作",
      icon: <Plus className="h-4 w-4 text-coral" />,
      shortcut: "⌘N",
      action: () => {
        onStartConversation(null);
        onClose();
      },
    });

    const isDark = getCachedColorScheme() === "dark";
    list.push({
      id: "action-toggle-theme",
      title: "切换主题外观",
      subtitle: isDark ? "切换为浅色人文模式" : "切换为暗黑夜间模式",
      category: "快捷操作",
      icon: isDark ? <Sun className="h-4 w-4 text-amber-500" /> : <Moon className="h-4 w-4 text-indigo-400" />,
      shortcut: "⌘D",
      action: () => {
        const current = getCachedColorScheme();
        applyColorScheme(current === "dark" ? "light" : "dark");
        onClose();
      },
    });

    // Navigation Features
    list.push({
      id: "nav-chat",
      title: "智能体聊天 (Chat)",
      subtitle: "进入对话工作区与 Agent 交互",
      category: "功能导航",
      icon: <MessageSquare className="h-4 w-4" />,
      shortcut: "⌘1",
      action: () => {
        onSelectFeature("chat");
        onClose();
      },
    });

    list.push({
      id: "nav-reading",
      title: "文档阅读 (Reading)",
      subtitle: "PDF / EPUB / Markdown 沉浸式 AI 阅读",
      category: "功能导航",
      icon: <BookOpen className="h-4 w-4" />,
      shortcut: "⌘2",
      action: () => {
        onSelectFeature("reading");
        onClose();
      },
    });

    list.push({
      id: "nav-knowledge",
      title: "知识库 (Knowledge)",
      subtitle: "本地向量库与知识索引管理",
      category: "功能导航",
      icon: <Database className="h-4 w-4" />,
      shortcut: "⌘3",
      action: () => {
        onSelectFeature("knowledge");
        onClose();
      },
    });

    list.push({
      id: "nav-drive",
      title: "云网盘与副本 (Drive)",
      subtitle: "夸克网盘与本地存储同步管理",
      category: "功能导航",
      icon: <HardDrive className="h-4 w-4" />,
      shortcut: "⌘4",
      action: () => {
        onSelectFeature("drive");
        onClose();
      },
    });

    list.push({
      id: "nav-calendar",
      title: "日程规划 (Calendar)",
      subtitle: "查看日历事件与时间线",
      category: "功能导航",
      icon: <Calendar className="h-4 w-4" />,
      shortcut: "⌘5",
      action: () => {
        onSelectFeature("calendar");
        onClose();
      },
    });

    list.push({
      id: "nav-tasks",
      title: "任务待办 (Tasks)",
      subtitle: "我的一天与智能任务列表",
      category: "功能导航",
      icon: <CheckSquare className="h-4 w-4" />,
      shortcut: "⌘6",
      action: () => {
        onSelectFeature("tasks");
        onClose();
      },
    });

    // Settings navigation
    list.push({
      id: "settings-general",
      title: "系统设置",
      subtitle: "偏好、模型、存储与同步配置",
      category: "系统配置",
      icon: <Settings className="h-4 w-4" />,
      action: () => {
        onOpenSettings("general");
        onClose();
      },
    });

    list.push({
      id: "settings-models",
      title: "模型与 API 密钥",
      subtitle: "配置 LLM 模型 Provider 与系统密钥",
      category: "系统配置",
      icon: <Sparkles className="h-4 w-4 text-coral" />,
      action: () => {
        onOpenSettings("models");
        onClose();
      },
    });

    // Recent Sessions
    const agentSessions = sessions
      .filter((s) => s.agent_id === activeAgentId)
      .slice(0, 8);

    for (const session of agentSessions) {
      list.push({
        id: `session-${session.id}`,
        title: session.title || "未命名会话",
        subtitle: session.workspace_id ? "工作区会话" : "普通会话",
        category: "最近会话",
        icon: <MessageSquare className="h-4 w-4 text-ink-muted" />,
        action: () => {
          setActiveSessionId(session.id);
          onSelectChatMode(session.workspace_id ? "code" : "home");
          onSelectFeature("chat");
          onClose();
        },
      });
    }

    return list;
  }, [
    activeAgentId,
    onClose,
    onOpenSettings,
    onSelectChatMode,
    onSelectFeature,
    onStartConversation,
    sessions,
    setActiveSessionId,
  ]);

  const filteredItems = useMemo(() => {
    if (!query.trim()) return items;
    const lower = query.toLowerCase();
    return items.filter(
      (item) =>
        item.title.toLowerCase().includes(lower) ||
        (item.subtitle && item.subtitle.toLowerCase().includes(lower)) ||
        item.category.toLowerCase().includes(lower)
    );
  }, [items, query]);

  useEffect(() => {
    setSelectedIndex(0);
  }, [filteredItems]);

  // Scroll selected item into view
  useEffect(() => {
    if (!listRef.current) return;
    const selectedEl = listRef.current.querySelector(
      `[data-index="${selectedIndex}"]`
    );
    if (selectedEl) {
      selectedEl.scrollIntoView({ block: "nearest" });
    }
  }, [selectedIndex]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((prev) =>
        prev < filteredItems.length - 1 ? prev + 1 : 0
      );
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((prev) =>
        prev > 0 ? prev - 1 : filteredItems.length - 1
      );
    } else if (e.key === "Enter") {
      e.preventDefault();
      const current = filteredItems[selectedIndex];
      if (current) current.action();
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  };

  if (!isOpen) return null;

  return createPortal(
    <div
      className="fixed inset-0 z-[300] flex items-start justify-center pt-[14vh] px-4 bg-ink/30 dark:bg-black/60 backdrop-blur-sm animate-fade-in"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="w-full max-w-xl overflow-hidden rounded-xl border border-hairline bg-canvas-surface text-ink shadow-floating animate-scale-in flex flex-col max-h-[70vh]"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/* Search Input Bar */}
        <div className="flex items-center gap-3 px-4 py-3.5 border-b border-hairline bg-canvas-surface">
          <Search className="h-4 w-4 text-ink-muted shrink-0" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="搜索指令、功能、会话或设置... (↑↓ 选择，↵ 确认)"
            className="flex-1 bg-transparent text-sm text-ink outline-none placeholder:text-ink-muted"
          />
          {query && (
            <button
              onClick={() => setQuery("")}
              className="p-1 rounded text-ink-muted hover:text-ink hover:bg-canvas-surface-card"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          )}
          <kbd className="hidden sm:inline-block px-1.5 py-0.5 text-[10px] font-mono text-ink-muted bg-canvas-surface-card border border-hairline rounded">
            ESC
          </kbd>
        </div>

        {/* Results List */}
        <div
          ref={listRef}
          className="flex-1 overflow-y-auto p-2 space-y-0.5 select-none"
        >
          {filteredItems.length === 0 ? (
            <div className="py-12 text-center text-xs text-ink-muted">
              未找到匹配项
            </div>
          ) : (
            filteredItems.map((item, index) => {
              const isSelected = index === selectedIndex;
              const prevItem = filteredItems[index - 1];
              const showCategory =
                !prevItem || prevItem.category !== item.category;

              return (
                <div key={item.id}>
                  {showCategory && (
                    <div className={`px-3 py-1.5 text-[10px] font-semibold tracking-wider uppercase text-ink-muted ${index > 0 ? "mt-2 pt-2 border-t border-hairline" : ""}`}>
                      {item.category}
                    </div>
                  )}
                  <div
                    data-index={index}
                    onClick={item.action}
                    onMouseEnter={() => setSelectedIndex(index)}
                    className={`flex items-center justify-between gap-3 px-3 py-2 rounded-lg cursor-pointer transition-colors ${
                      isSelected
                        ? "bg-coral/15 text-coral dark:bg-coral/20 font-medium"
                        : "text-ink hover:bg-canvas-surface-card"
                    }`}
                  >
                    <div className="flex items-center gap-2.5 min-w-0">
                      <span className="shrink-0 text-ink-secondary">{item.icon}</span>
                      <div className="min-w-0">
                        <div className="text-xs font-medium truncate">
                          {item.title}
                        </div>
                        {item.subtitle && (
                          <div className="text-[11px] text-ink-muted truncate font-normal">
                            {item.subtitle}
                          </div>
                        )}
                      </div>
                    </div>
                    {item.shortcut && (
                      <kbd className="shrink-0 px-1.5 py-0.5 text-[10px] font-mono text-ink-muted bg-canvas-surface-card border border-hairline rounded">
                        {item.shortcut}
                      </kbd>
                    )}
                  </div>
                </div>
              );
            })
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between px-4 py-2 border-t border-hairline bg-canvas-surface-subtle/50 text-[11px] text-ink-muted">
          <div className="flex items-center gap-3">
            <span>
              <kbd className="font-mono">↑↓</kbd> 导航
            </span>
            <span>
              <kbd className="font-mono">↵</kbd> 打开
            </span>
          </div>
          <span>Agnes Command</span>
        </div>
      </div>
    </div>,
    document.body
  );
};
