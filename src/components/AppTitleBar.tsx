import React, { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Maximize2,
  Minimize2,
  Minus,
  PanelLeft,
  Search,
  X,
} from "lucide-react";

interface AppTitleBarProps {
  title: string;
  isSidebarOpen: boolean;
  onToggleSidebar: () => void;
  onOpenCommandPalette?: () => void;
}

/** Native-feeling clean title bar for the undecorated main Tauri window. */
export const AppTitleBar: React.FC<AppTitleBarProps> = ({
  title,
  isSidebarOpen,
  onToggleSidebar,
  onOpenCommandPalette,
}) => {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    let mounted = true;
    getCurrentWindow()
      .isMaximized()
      .then((maximized) => {
        if (mounted) setIsMaximized(maximized);
      })
      .catch(() => {
        // Browser previews do not expose a Tauri window; controls remain harmless.
      });
    return () => {
      mounted = false;
    };
  }, []);

  const minimize = () => getCurrentWindow().minimize().catch(console.error);
  const toggleMaximize = () => {
    getCurrentWindow()
      .toggleMaximize()
      .then(() => setIsMaximized((maximized) => !maximized))
      .catch(console.error);
  };
  const close = () => getCurrentWindow().close().catch(console.error);

  return (
    <header
      className="agnes-titlebar flex h-10 shrink-0 items-center justify-between border-b border-hairline bg-canvas-sidebar/90 backdrop-blur-md px-2.5 text-ink-secondary select-none transition-colors"
      data-tauri-drag-region
    >
      <div className="flex min-w-0 flex-1 items-center gap-1.5" data-tauri-drag-region>
        <button
          type="button"
          onClick={onToggleSidebar}
          className="agnes-titlebar-action grid h-7 w-7 shrink-0 place-items-center rounded-md text-ink-muted hover:bg-canvas-surface-card hover:text-ink transition-colors"
          title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
          aria-label={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
        >
          <PanelLeft className="h-4 w-4" />
        </button>
        <span className="ml-1 truncate text-xs font-semibold text-ink" data-tauri-drag-region>
          Agnes
        </span>
        <span className="text-ink-muted/40" aria-hidden="true">/</span>
        <span className="truncate text-xs text-ink-muted" data-tauri-drag-region>
          {title}
        </span>
      </div>

      {onOpenCommandPalette && (
        <button
          type="button"
          onClick={onOpenCommandPalette}
          className="hidden sm:flex items-center gap-2 px-2.5 py-1 text-[11px] text-ink-muted bg-canvas-surface-card/60 hover:bg-canvas-surface-card hover:text-ink border border-hairline/60 rounded-md transition-all shadow-subtle mx-2"
          title="打开全局指令搜索 (Cmd+K)"
        >
          <Search className="h-3 w-3" />
          <span>快速指令</span>
          <kbd className="font-mono text-[9px] px-1 py-0.2 bg-canvas-surface border border-hairline rounded text-ink-muted">
            ⌘K
          </kbd>
        </button>
      )}

      <div className="flex shrink-0 items-center gap-0.5">
        <button
          type="button"
          onClick={minimize}
          className="agnes-titlebar-action grid h-7 w-8 place-items-center rounded-md text-ink-muted hover:bg-canvas-surface-card hover:text-ink transition-colors"
          title="最小化"
          aria-label="最小化"
        >
          <Minus className="h-3.5 w-3.5" />
        </button>
        <button
          type="button"
          onClick={toggleMaximize}
          className="agnes-titlebar-action grid h-7 w-8 place-items-center rounded-md text-ink-muted hover:bg-canvas-surface-card hover:text-ink transition-colors"
          title={isMaximized ? "还原窗口" : "最大化"}
          aria-label={isMaximized ? "还原窗口" : "最大化"}
        >
          {isMaximized ? <Minimize2 className="h-3.5 w-3.5" /> : <Maximize2 className="h-3.5 w-3.5" />}
        </button>
        <button
          type="button"
          onClick={close}
          className="agnes-titlebar-action agnes-titlebar-close grid h-7 w-8 place-items-center rounded-md text-ink-muted hover:bg-red-500 hover:text-white transition-colors"
          title="关闭"
          aria-label="关闭"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    </header>
  );
};

