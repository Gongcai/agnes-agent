import React, { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowsOutSimple,
  Minus,
  SidebarSimple,
  Square,
  X,
} from "@phosphor-icons/react";

interface AppTitleBarProps {
  title: string;
  isSidebarOpen: boolean;
  onToggleSidebar: () => void;
}

/** Small native-feeling title bar for the undecorated main Tauri window. */
export const AppTitleBar: React.FC<AppTitleBarProps> = ({
  title,
  isSidebarOpen,
  onToggleSidebar,
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
    <header className="agnes-titlebar flex h-10 shrink-0 items-center border-b border-stone-200/80 bg-white/90 px-2 text-stone-500 select-none" data-tauri-drag-region>
      <div className="flex min-w-0 flex-1 items-center gap-1" data-tauri-drag-region>
        <button
          type="button"
          onClick={onToggleSidebar}
          className="agnes-titlebar-action grid h-7 w-7 shrink-0 place-items-center rounded-md"
          title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
          aria-label={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
        >
          <SidebarSimple className="h-4 w-4" weight="regular" />
        </button>
        <span className="ml-1 truncate text-xs font-medium text-stone-700" data-tauri-drag-region>
          Agnes
        </span>
        <span className="mx-1 text-stone-300" aria-hidden="true">/</span>
        <span className="truncate text-xs text-stone-500" data-tauri-drag-region>{title}</span>
      </div>

      <div className="flex shrink-0 items-center gap-0.5">
        <button type="button" onClick={minimize} className="agnes-titlebar-action grid h-7 w-8 place-items-center rounded-md" title="最小化" aria-label="最小化">
          <Minus className="h-3.5 w-3.5" />
        </button>
        <button type="button" onClick={toggleMaximize} className="agnes-titlebar-action grid h-7 w-8 place-items-center rounded-md" title={isMaximized ? "还原窗口" : "最大化"} aria-label={isMaximized ? "还原窗口" : "最大化"}>
          {isMaximized ? <ArrowsOutSimple className="h-3.5 w-3.5" /> : <Square className="h-3 w-3" />}
        </button>
        <button type="button" onClick={close} className="agnes-titlebar-action agnes-titlebar-close grid h-7 w-8 place-items-center rounded-md" title="关闭" aria-label="关闭">
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
    </header>
  );
};
