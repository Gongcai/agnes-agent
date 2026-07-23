import React, { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  ArrowClockwise,
  ArrowLeft,
  ArrowRight,
  ClipboardText,
  Code,
  Copy,
  Link,
  Scissors,
  SelectionAll,
} from "@phosphor-icons/react";

type EditableTarget = HTMLInputElement | HTMLTextAreaElement | HTMLElement;

interface ContextMenuState {
  x: number;
  y: number;
  selectedText: string;
  editableTarget: EditableTarget | null;
  canGoBack: boolean;
  canGoForward: boolean;
  canInspect: boolean;
}

interface MenuItemProps {
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  disabled?: boolean;
  onSelect: () => void;
}

const EDITABLE_INPUT_TYPES = new Set(["password", "search", "tel", "text", "url"]);

function getEditableTarget(target: EventTarget | null): EditableTarget | null {
  if (target instanceof HTMLTextAreaElement) return target;
  if (target instanceof HTMLInputElement && EDITABLE_INPUT_TYPES.has(target.type)) return target;
  if (target instanceof Element) {
    const editable = target.closest<HTMLElement>('[contenteditable="true"]');
    if (editable) return editable;
  }
  return null;
}

function getSelectionText(target: EditableTarget | null): string {
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    const start = target.selectionStart ?? 0;
    const end = target.selectionEnd ?? start;
    return target.value.slice(start, end);
  }
  return window.getSelection()?.toString() ?? "";
}

function replaceEditableSelection(target: EditableTarget, replacement: string): void {
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    const start = target.selectionStart ?? target.value.length;
    const end = target.selectionEnd ?? start;
    const next = `${target.value.slice(0, start)}${replacement}${target.value.slice(end)}`;
    const prototype = target instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
    setter?.call(target, next);
    target.dispatchEvent(new Event("input", { bubbles: true }));
    target.focus();
    const cursor = start + replacement.length;
    target.setSelectionRange(cursor, cursor);
    return;
  }

  target.focus();
  document.execCommand("insertText", false, replacement);
}

function selectAllEditable(target: EditableTarget): void {
  target.focus();
  if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
    target.select();
    return;
  }
  const selection = window.getSelection();
  const range = document.createRange();
  range.selectNodeContents(target);
  selection?.removeAllRanges();
  selection?.addRange(range);
}

async function copyText(text: string): Promise<void> {
  if (!text) return;
  try {
    if (isTauri()) {
      await writeText(text);
      return;
    }
    await navigator.clipboard.writeText(text);
    return;
  } catch {
    const helper = document.createElement("textarea");
    helper.value = text;
    helper.style.position = "fixed";
    helper.style.opacity = "0";
    document.body.appendChild(helper);
    helper.select();
    document.execCommand("copy");
    helper.remove();
  }
}

async function pasteText(): Promise<string> {
  if (isTauri()) return readText();
  return navigator.clipboard.readText();
}

function textFragmentUrl(text: string): string {
  const url = new URL(window.location.href);
  url.hash = "";
  return `${url.toString()}#:~:text=${encodeURIComponent(text.trim())}`;
}

const MenuItem: React.FC<MenuItemProps> = ({ label, icon: Icon, disabled = false, onSelect }) => (
  <button
    type="button"
    role="menuitem"
    disabled={disabled}
    className="agnes-context-menu-item flex h-8 w-full items-center gap-2 rounded-md px-2.5 text-left text-xs transition-colors"
    onMouseDown={(event) => event.preventDefault()}
    onClick={onSelect}
  >
    <Icon className="h-4 w-4 shrink-0" />
    <span className="truncate">{label}</span>
  </button>
);

const MenuSeparator = () => <div className="agnes-context-menu-separator my-1 border-t" role="separator" />;

export const AppContextMenu: React.FC = () => {
  const [menu, setMenu] = useState<ContextMenuState | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 8, top: 8 });

  useEffect(() => {
    const handleContextMenu = (event: MouseEvent) => {
      if (event.defaultPrevented) return;
      const editableTarget = getEditableTarget(event.target);
      const selectedText = getSelectionText(editableTarget);
      const navigation = (window as Window & {
        navigation?: { canGoBack?: boolean; canGoForward?: boolean };
      }).navigation;
      event.preventDefault();
      setMenu({
        x: event.clientX,
        y: event.clientY,
        selectedText,
        editableTarget,
        canGoBack: navigation?.canGoBack ?? window.history.length > 1,
        canGoForward: navigation?.canGoForward ?? false,
        canInspect: import.meta.env.DEV && isTauri(),
      });
    };
    document.addEventListener("contextmenu", handleContextMenu);
    return () => document.removeEventListener("contextmenu", handleContextMenu);
  }, []);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    const onPointerDown = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) close();
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, [menu]);

  useLayoutEffect(() => {
    if (!menu || !menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    setPosition({
      left: Math.max(8, Math.min(menu.x, window.innerWidth - rect.width - 8)),
      top: Math.max(8, Math.min(menu.y, window.innerHeight - rect.height - 8)),
    });
  }, [menu]);

  if (!menu) return null;

  const close = () => setMenu(null);
  const run = (action: () => void | Promise<void>) => {
    close();
    void Promise.resolve(action()).catch(console.error);
  };
  const inspect = () => run(() => invoke("open_webview_devtools"));
  const selection = menu.selectedText;

  return createPortal(
    <div
      ref={menuRef}
      className="agnes-context-menu fixed z-[200] w-56 rounded-lg border p-1.5 shadow-2xl"
      style={{ left: position.left, top: position.top }}
      role="menu"
      onContextMenu={(event) => event.preventDefault()}
    >
      {menu.editableTarget ? (
        <>
          <MenuItem
            label="剪切"
            icon={Scissors}
            disabled={!selection}
            onSelect={() => run(async () => {
              await copyText(selection);
              replaceEditableSelection(menu.editableTarget!, "");
            })}
          />
          <MenuItem label="复制" icon={Copy} disabled={!selection} onSelect={() => run(() => copyText(selection))} />
          <MenuItem
            label="粘贴"
            icon={ClipboardText}
            onSelect={() => run(async () => replaceEditableSelection(menu.editableTarget!, await pasteText()))}
          />
          <MenuSeparator />
          <MenuItem label="全选" icon={SelectionAll} onSelect={() => run(() => selectAllEditable(menu.editableTarget!))} />
        </>
      ) : selection ? (
        <>
          <MenuItem label="复制" icon={Copy} onSelect={() => run(() => copyText(selection))} />
          <MenuItem label="复制带高亮的链接" icon={Link} onSelect={() => run(() => copyText(textFragmentUrl(selection)))} />
        </>
      ) : (
        <>
          <MenuItem label="后退" icon={ArrowLeft} disabled={!menu.canGoBack} onSelect={() => run(() => window.history.back())} />
          <MenuItem label="前进" icon={ArrowRight} disabled={!menu.canGoForward} onSelect={() => run(() => window.history.forward())} />
          <MenuItem label="重新加载" icon={ArrowClockwise} onSelect={() => run(() => window.location.reload())} />
        </>
      )}
      {menu.canInspect && (
        <>
          <MenuSeparator />
          <MenuItem label="检查元素" icon={Code} onSelect={inspect} />
        </>
      )}
    </div>,
    document.body,
  );
};
