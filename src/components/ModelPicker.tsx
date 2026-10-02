import React, { useDeferredValue, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, Cpu, HardDrives, MagnifyingGlass, SlidersHorizontal, X } from "@phosphor-icons/react";
import type { ModelProvider } from "../store/useAgentStore";
import { filterModelProviders } from "../lib/modelPicker";

export type ModelPickerTab = "models" | "settings";

interface ModelPickerProps {
  anchorRef: React.RefObject<HTMLDivElement>;
  providers: ModelProvider[];
  effectiveModel: string;
  tab: ModelPickerTab;
  onTabChange: (tab: ModelPickerTab) => void;
  onSelect: (model: string) => void;
  onClose: () => void;
  children: React.ReactNode;
}

export function ModelPicker({ anchorRef, providers, effectiveModel, tab, onTabChange, onSelect, onClose, children }: ModelPickerProps) {
  const id = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const focusTabRef = useRef(false);
  const [query, setQuery] = useState("");
  const [providerId, setProviderId] = useState("");
  const deferredQuery = useDeferredValue(query);
  const filtered = useMemo(() => filterModelProviders(providers, deferredQuery, providerId), [providers, deferredQuery, providerId]);
  const total = providers.reduce((count, provider) => count + provider.models.length, 0);
  const count = filtered.reduce((sum, provider) => sum + provider.models.length, 0);
  const current = useMemo(() => {
    for (const provider of providers) {
      const model = provider.models.find((item) => `${provider.id}/${item.id}` === effectiveModel);
      if (model) return { value: effectiveModel, name: model.id, providerName: provider.name };
    }
    return null;
  }, [providers, effectiveModel]);
  const [position, setPosition] = useState<React.CSSProperties>({ visibility: "hidden" });

  useLayoutEffect(() => {
    const place = () => {
      const anchor = anchorRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const viewport = window.visualViewport;
      const width = viewport?.width ?? window.innerWidth;
      const height = viewport?.height ?? window.innerHeight;
      const offsetTop = viewport?.offsetTop ?? 0;
      const offsetLeft = viewport?.offsetLeft ?? 0;
      const panelWidth = Math.min(400, width - 24);
      const above = anchor.top - offsetTop - 20;
      const below = offsetTop + height - anchor.bottom - 20;
      const openAbove = above >= Math.min(360, below);
      const available = openAbove ? above : below;
      const useViewport = available < Math.min(320, height - 24);
      setPosition({
        width: panelWidth,
        left: Math.max(offsetLeft + 12, Math.min(anchor.right - panelWidth, offsetLeft + width - panelWidth - 12)),
        maxHeight: Math.min(480, Math.max(0, useViewport ? height - 24 : available)),
        ...(useViewport ? { top: offsetTop + 12 } : openAbove ? { bottom: window.innerHeight - anchor.top + 8 } : { top: anchor.bottom + 8 }),
      });
    };
    place();
    const observer = new ResizeObserver(place);
    if (anchorRef.current) observer.observe(anchorRef.current);
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    window.visualViewport?.addEventListener("resize", place);
    window.visualViewport?.addEventListener("scroll", place);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
      window.visualViewport?.removeEventListener("resize", place);
      window.visualViewport?.removeEventListener("scroll", place);
    };
  }, [anchorRef]);

  useEffect(() => {
    if (tab === "models" && !focusTabRef.current) searchRef.current?.focus({ preventScroll: true });
    else panelRef.current?.querySelector<HTMLButtonElement>('[role="tab"][aria-selected="true"]')?.focus({ preventScroll: true });
    focusTabRef.current = false;
  }, [tab]);

  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !panelRef.current?.contains(event.target) && !anchorRef.current?.contains(event.target)) onClose();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      onClose();
      anchorRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", escape);
    };
  }, [anchorRef, onClose]);

  useEffect(() => {
    listRef.current?.scrollTo({ top: 0 });
  }, [deferredQuery, providerId]);

  const closeAndFocus = () => {
    onClose();
    anchorRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  };
  const choose = (value: string) => {
    if (value !== effectiveModel) onSelect(value);
    closeAndFocus();
  };
  const modelButton = (value: string, name: string, providerName?: string) => (
    <button
      key={value}
      type="button"
      data-model-option
      aria-pressed={effectiveModel === value}
      onClick={() => choose(value)}
      className={`model-picker-option flex min-h-11 w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition-colors ${effectiveModel === value ? "bg-clay-soft text-ink" : "text-ink-body hover:bg-canvas-surface-soft"}`}
    >
      <span className="min-w-0 flex-1">
        <span className="block break-words font-mono text-xs [overflow-wrap:anywhere]">{name}</span>
        {providerName && <span className="mt-0.5 block break-words text-xs text-ink-muted">{providerName}</span>}
      </span>
      {effectiveModel === value && <Check className="h-4 w-4 shrink-0 text-clay" aria-hidden="true" />}
    </button>
  );

  return createPortal(
    <div
      ref={panelRef}
      id="chat-model-picker"
      role="dialog"
      aria-label="模型与会话设置"
      style={position}
      className="claude-popover model-picker fixed z-[100] flex flex-col overflow-hidden rounded-xl border border-hairline bg-canvas-surface text-ink shadow-modal"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          closeAndFocus();
        }
      }}
      onBlur={(event) => {
        if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget) && !anchorRef.current?.contains(event.relatedTarget)) onClose();
      }}
    >
      <div className="flex shrink-0 items-center gap-2 border-b border-hairline px-3 py-2">
        <div role="tablist" aria-label="模型选择与会话设置" className="flex min-w-0 flex-1 gap-1">
          {(["models", "settings"] as const).map((value) => (
            <button
              key={value}
              id={`${id}-${value}-tab`}
              type="button"
              role="tab"
              aria-selected={tab === value}
              aria-controls={`${id}-${value}-panel`}
              tabIndex={tab === value ? 0 : -1}
              onClick={() => onTabChange(value)}
              onKeyDown={(event) => {
                if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
                  event.preventDefault();
                  const next = event.key === "Home" ? "models" : event.key === "End" ? "settings" : value === "models" ? "settings" : "models";
                  if (next !== tab) {
                    focusTabRef.current = true;
                    onTabChange(next);
                  }
                }
              }}
              className={`inline-flex min-h-9 items-center gap-2 rounded-lg px-3 text-xs font-medium transition-colors ${tab === value ? "bg-canvas-surface-strong text-ink" : "text-ink-muted hover:bg-canvas-surface-soft"}`}
            >
              {value === "models" ? <Cpu className="h-4 w-4" aria-hidden="true" /> : <SlidersHorizontal className="h-4 w-4" aria-hidden="true" />}
              {value === "models" ? "模型" : "会话设置"}
            </button>
          ))}
        </div>
        <button type="button" aria-label="关闭模型与会话设置" onClick={closeAndFocus} className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg text-ink-muted hover:bg-canvas-surface-soft">
          <X className="h-4 w-4" aria-hidden="true" />
        </button>
      </div>

      <div role="tabpanel" id={`${id}-${tab}-panel`} aria-labelledby={`${id}-${tab}-tab`} className="flex min-h-0 flex-col">
        {tab === "models" ? <>
          <div className="shrink-0 space-y-2 border-b border-hairline p-3">
            <div className="flex min-h-11 items-center gap-2 rounded-lg border border-hairline-strong bg-canvas-input px-3 focus-within:ring-2 focus-within:ring-clay">
              <MagnifyingGlass className="h-4 w-4 shrink-0 text-ink-muted" aria-hidden="true" />
              <input
                ref={searchRef}
                type="search"
                aria-label="搜索模型或提供商"
                placeholder="搜索模型或提供商…"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  const options = listRef.current?.querySelectorAll<HTMLButtonElement>("[data-model-option]");
                  if (!options?.length || event.nativeEvent.isComposing) return;
                  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                    event.preventDefault();
                    options[event.key === "ArrowDown" ? 0 : options.length - 1].focus();
                  } else if (event.key === "Enter" && query.trim() && query === deferredQuery) {
                    event.preventDefault();
                    options[0].click();
                  }
                }}
                className="min-w-0 flex-1 bg-transparent py-2 text-sm text-ink outline-none placeholder:text-ink-muted"
              />
            </div>
            <div className="flex items-center gap-2">
              <label htmlFor={`${id}-provider`} className="shrink-0 text-xs text-ink-muted">提供商</label>
              <select id={`${id}-provider`} value={providerId} onChange={(event) => setProviderId(event.target.value)} className="h-10 min-w-0 flex-1 rounded-lg border border-hairline bg-canvas-input px-2 text-xs text-ink">
                <option value="">全部提供商（{providers.length}）</option>
                {providers.map((provider) => <option key={provider.id} value={provider.id}>{provider.name}（{provider.models.length}）</option>)}
              </select>
            </div>
          </div>
          <div
            ref={listRef}
            className="min-h-0 overflow-y-auto overscroll-contain p-2"
            aria-label="可选模型"
            onKeyDown={(event) => {
              const options = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("[data-model-option]"));
              const index = options.indexOf(event.target as HTMLButtonElement);
              if (index < 0) return;
              if (event.key === "ArrowDown" || event.key === "ArrowUp") {
                event.preventDefault();
                const next = index + (event.key === "ArrowDown" ? 1 : -1);
                if (next < 0) searchRef.current?.focus();
                else options[Math.min(next, options.length - 1)]?.focus();
              } else if (event.key === "Home" || event.key === "End") {
                event.preventDefault();
                options[event.key === "Home" ? 0 : options.length - 1]?.focus();
              }
            }}
          >
            {current && !query.trim() && !providerId && <div className="mb-2 border-b border-hairline pb-2">
              <div className="px-3 py-1 text-xs font-medium text-ink-muted">当前模型</div>
              {modelButton(current.value, current.name, current.providerName)}
            </div>}
            <div role="status" aria-live="polite" aria-atomic="true" className="px-3 py-2 text-xs text-ink-muted">
              {query.trim() || providerId ? `找到 ${count} 个模型` : `${total} 个模型 · ${providers.length} 个提供商`}
            </div>
            {filtered.map((provider) => <section key={provider.id} aria-label={provider.name} className="mb-2">
              <div className="flex items-center gap-2 px-3 py-2 text-xs font-medium text-ink-muted">
                <HardDrives className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                <span className="min-w-0 flex-1 break-words">{provider.name}</span>
                <span>{provider.models.length}</span>
              </div>
              {provider.models.map((model) => modelButton(`${provider.id}/${model.id}`, model.id))}
            </section>)}
            {count === 0 && <div className="px-3 py-6 text-center text-sm">
              <p>{total === 0 ? "暂无可用模型" : "没有匹配的模型"}</p>
              <p className="mt-1 text-xs leading-relaxed text-ink-muted">{total === 0 ? "请到设置 → 模型中添加提供商和模型。" : "试试模型名称的一部分，或切换提供商。"}</p>
              {total > 0 && <button type="button" className="mt-3 min-h-10 rounded-lg border border-hairline px-3 text-xs hover:bg-canvas-surface-soft" onClick={() => { setQuery(""); setProviderId(""); searchRef.current?.focus(); }}>清除筛选</button>}
            </div>}
          </div>
        </> : <div className="model-picker-settings min-h-0 overflow-y-auto overscroll-contain p-3">{children}</div>}
      </div>
    </div>,
    document.body,
  );
}
