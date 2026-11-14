import React, { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowsOut, ArrowsIn, Minus, SidebarSimple, MagnifyingGlass, X } from "@phosphor-icons/react";
import { AppLogo } from "./AppLogo";

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
      className="agnes-titlebar relative flex h-10 shrink-0 items-center justify-between px-2.5 text-ink-secondary select-none transition-colors"
      data-sidebar-open={isSidebarOpen}
      data-tauri-drag-region
    >
      <div className="flex min-w-0 flex-1 items-center gap-1.5 pr-3" data-tauri-drag-region>
        <button
          type="button"
          onClick={onToggleSidebar}
          className="agnes-titlebar-action agnes-sidebar-toggle grid h-7 w-7 shrink-0 place-items-center rounded-md text-ink-muted hover:bg-canvas-surface-card hover:text-ink transition-colors"
          title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
          aria-label={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
        >
          <SidebarSimple className="h-4 w-4" />
        </button>
        <AppLogo size={18} className="ml-1 pointer-events-none opacity-95" />
        <span className="truncate text-[0.84375rem] font-medium tracking-tight text-ink" data-tauri-drag-region>
          Agnes
        </span>
        <span className="text-xs text-ink-muted/35" aria-hidden="true">/</span>
        <span className="truncate text-[0.84375rem] text-ink-muted" data-tauri-drag-region>
          {title}
        </span>
      </div>

      {onOpenCommandPalette && (
        <button
          type="button"
          onClick={onOpenCommandPalette}
          className="agnes-command-trigger hidden sm:flex w-56 lg:w-64 shrink-0 items-center justify-between gap-2 rounded-lg border border-hairline px-3 py-1 text-[0.8125rem] text-ink-muted transition-all"
          title="打开全局指令搜索 (Cmd+K)"
        >
          <span className="flex items-center gap-1.5 truncate">
            <MagnifyingGlass className="h-3.5 w-3.5 shrink-0 opacity-70" />
            <span className="truncate">快速指令…</span>
          </span>
          <kbd className="agnes-command-kbd shrink-0 rounded px-1.5 py-0.5 font-mono text-[0.6875rem] leading-none text-ink-muted/80">
            ⌘K
          </kbd>
        </button>
      )}

      <div className="agnes-window-controls flex flex-1 items-center justify-end gap-1 pl-3" data-tauri-drag-region>
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
          {isMaximized ? <ArrowsIn className="h-3.5 w-3.5" /> : <ArrowsOut className="h-3.5 w-3.5" />}
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
