import React, { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import ePub, { type Book, type Contents, type Location, type NavItem, type Rendition } from "epubjs";
import {
  ArrowLeft,
  BookMarked,
  BookOpen,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  Cpu,
  CloudUpload,
  FileUp,
  Highlighter,
  History,
  Languages,
  LoaderCircle,
  MessageCircleMore,
  NotebookPen,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Plus,
  Quote,
  RefreshCw,
  RotateCcw,
  Send,
  Settings2,
  ShieldAlert,
  SlidersHorizontal,
  Trash2,
  Type,
  X,
} from "lucide-react";
import { AnimatedDisclosure } from "./AnimatedDisclosure";
import { MarkdownMessage } from "./MarkdownMessage";
import { useAgentStore } from "../store/useAgentStore";
import {
  DEFAULT_READING_PREFERENCES,
  normalizeReadingPreferences,
  parseReadingPreferences,
  type ReadingPreferences,
} from "../lib/readingPreferences";
import {
  DEFAULT_MAX_OUTPUT_TOKENS,
  getCachedAutoFollowStreaming,
  getCachedAutoExpandThoughts,
  getResolvedColorScheme,
  subscribeUIPreferenceChanges,
  type ResolvedColorScheme,
} from "../lib/uiPreferences";

interface ReadingBook {
  id: string;
  collection_id: string | null;
  document_id: string | null;
  local_path: string | null;
  title: string;
  author: string | null;
  source_hash: string;
  model_knows_content: boolean;
  content_context_allowed: boolean;
  content_context_decided: boolean;
  progress_cfi: string | null;
  created_at: string;
  updated_at: string;
  artifact_id: string | null;
  artifact_status: string | null;
  ready_replica_count: number;
  local_artifact_status: string | null;
  local_artifact_error: string | null;
}

interface ReadingEpubPublishResult {
  artifact_id: string;
  reused: boolean;
  ready_replica_count: number;
}

interface ReadingHighlight {
  id: string;
  book_id: string;
  cfi_range: string;
  quote: string;
  context_before: string;
  context_after: string;
  note: string | null;
  color: string;
}

interface ReadingConversation {
  session_id: string;
  title: string;
  created_at: string;
  updated_at: string;
  is_current: boolean;
}

interface PendingSelection {
  cfiRange: string;
  quote: string;
  contextBefore: string;
  contextAfter: string;
}

interface SelectionMenu {
  selection: PendingSelection;
  x: number;
  y: number;
}

const HIGHLIGHT_STYLES: Record<string, Record<string, string>> = {
  yellow: { fill: "#f7d774", "fill-opacity": "0.45", "mix-blend-mode": "multiply" },
  green: { fill: "#8fbc8f", "fill-opacity": "0.42", "mix-blend-mode": "multiply" },
  blue: { fill: "#8bc4e8", "fill-opacity": "0.42", "mix-blend-mode": "multiply" },
  pink: { fill: "#e6a7c5", "fill-opacity": "0.42", "mix-blend-mode": "multiply" },
};

const HIGHLIGHT_COLORS = ["yellow", "green", "blue", "pink"] as const;

const HIGHLIGHT_COLOR_LABELS: Record<string, string> = {
  yellow: "黄色",
  green: "绿色",
  blue: "蓝色",
  pink: "粉色",
};

const HIGHLIGHT_COLOR_SWATCHES: Record<string, string> = {
  yellow: "#e8bd3f",
  green: "#67a36c",
  blue: "#5aa7d8",
  pink: "#cf7da6",
};

const APP_SERIF_FONT_FALLBACK =
  '"Noto Serif", "Noto Serif CJK SC", "Source Han Serif SC", "Songti SC", Georgia, serif';

function appSerifFontFamily(): string {
  return getComputedStyle(document.documentElement)
    .getPropertyValue("--claude-ui-font")
    .trim() || APP_SERIF_FONT_FALLBACK;
}

function epubTheme(
  colorScheme: ResolvedColorScheme,
  preferences: ReadingPreferences,
): Record<string, Record<string, string>> {
  const dark = colorScheme === "dark";
  const serifFont = `${appSerifFontFamily()} !important`;
  return {
    html: {
      background: dark ? "#1f1e1b" : "#fbfaf6",
      color: dark ? "#dedad1" : "#282723",
    },
    body: {
      background: dark ? "#1f1e1b" : "#fbfaf6",
      color: dark ? "#dedad1" : "#282723",
      "font-family": serifFont,
      "font-size": `${preferences.fontSize}px !important`,
      "line-height": `${preferences.lineHeight} !important`,
      width: "100% !important",
      "max-width": `${preferences.contentWidth}px !important`,
      margin: "0 auto !important",
      padding: "12px 32px 48px !important",
      "box-sizing": "border-box !important",
    },
    "html body, html body *": { "font-family": serifFont },
    p: { "margin-bottom": "1em" },
    a: { color: dark ? "#ea9679" : "#b95f43" },
    img: { "max-width": "100%", height: "auto" },
  };
}

function flattenToc(items: NavItem[]): NavItem[] {
  return items.flatMap((item) => [item, ...flattenToc(item.subitems ?? [])]);
}

function nearbyParagraphs(selection: Selection): { before: string; after: string } {
  const range = selection.rangeCount ? selection.getRangeAt(0) : null;
  if (!range) return { before: "", after: "" };
  const start = range.startContainer.nodeType === Node.ELEMENT_NODE
    ? range.startContainer as Element
    : range.startContainer.parentElement;
  const paragraph = start?.closest("p, li, blockquote, dd, dt, figcaption");
  if (!paragraph) return { before: "", after: "" };
  const before = paragraph.previousElementSibling?.textContent?.trim() ?? "";
  const after = paragraph.nextElementSibling?.textContent?.trim() ?? "";
  return { before, after };
}

function readingQuestion(book: ReadingBook, question: string, selection: PendingSelection | null): string {
  const attribution = book.author ? `《${book.title}》(${book.author})` : `《${book.title}》`;
  if (!selection) return `我正在阅读${attribution}。\n\n${question}`;
  return `[阅读划线 · ${attribution}]\n\n划线内容：\n${selection.quote}\n\n前文：\n${selection.contextBefore || "（无）"}\n\n后文：\n${selection.contextAfter || "（无）"}\n\n我的问题：\n${question}`;
}

function translationRequest(book: ReadingBook, selection: PendingSelection, language: string): string {
  const attribution = book.author ? `《${book.title}》(${book.author})` : `《${book.title}》`;
  return `[阅读翻译 · ${attribution}]\n\n请将以下文本翻译成${language}。只输出译文，保留原文的段落结构和必要的专有名词。\n\n原文：\n${selection.quote}`;
}

function conversationDate(timestamp: string): string {
  const date = new Date(Number(timestamp) * 1000);
  if (Number.isNaN(date.getTime())) return timestamp;
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

function handleReaderNavigationKey(event: KeyboardEvent, rendition: Rendition | null): void {
  const target = event.target as Element | null;
  if (typeof target?.closest === "function"
    && target.closest("input, textarea, select, button, [contenteditable='true']")) return;
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
  if (event.key === "ArrowLeft") {
    event.preventDefault();
    rendition?.prev().catch(console.error);
  } else if (event.key === "ArrowRight") {
    event.preventDefault();
    rendition?.next().catch(console.error);
  }
}

const EpubPane: React.FC<{
  book: ReadingBook;
  highlights: ReadingHighlight[];
  highlightMode: boolean;
  highlightColor: string;
  colorScheme: ResolvedColorScheme;
  preferences: ReadingPreferences;
  onProgress: (cfi: string) => void;
  onBackToShelf: () => void;
  onToggleHighlightMode: () => void;
  onHighlightColorChange: (color: string) => void;
  onCreateHighlight: (selection: PendingSelection) => void;
  onUpdateHighlight: (highlightId: string, note: string | null, color: string) => Promise<void>;
  onDeleteHighlight: (highlight: ReadingHighlight) => Promise<void>;
  onPreferencesChange: (preferences: ReadingPreferences) => void;
  onOpenSelectionMenu: (selection: PendingSelection, x: number, y: number) => void;
}> = ({
  book,
  highlights,
  highlightMode,
  highlightColor,
  colorScheme,
  preferences,
  onProgress,
  onBackToShelf,
  onToggleHighlightMode,
  onHighlightColorChange,
  onCreateHighlight,
  onUpdateHighlight,
  onDeleteHighlight,
  onPreferencesChange,
  onOpenSelectionMenu,
}) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const bookRef = useRef<Book | null>(null);
  const renditionRef = useRef<Rendition | null>(null);
  const highlightModeRef = useRef(highlightMode);
  const createHighlightRef = useRef(onCreateHighlight);
  const openSelectionMenuRef = useRef(onOpenSelectionMenu);
  const colorSchemeRef = useRef(colorScheme);
  const preferencesRef = useRef(preferences);
  const highlightsRef = useRef(highlights);
  const appliedHighlightsRef = useRef(new Map<string, { cfiRange: string; color: string }>());
  const [toc, setToc] = useState<NavItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [readingProgress, setReadingProgress] = useState(0);
  const [currentChapter, setCurrentChapter] = useState("");
  const [isHighlightsOpen, setIsHighlightsOpen] = useState(false);
  const [isTypographyOpen, setIsTypographyOpen] = useState(false);
  const [editingHighlightId, setEditingHighlightId] = useState<string | null>(null);
  const [noteDraft, setNoteDraft] = useState("");
  const [colorDraft, setColorDraft] = useState("yellow");

  useEffect(() => { highlightModeRef.current = highlightMode; }, [highlightMode]);
  useEffect(() => { createHighlightRef.current = onCreateHighlight; }, [onCreateHighlight]);
  useEffect(() => { openSelectionMenuRef.current = onOpenSelectionMenu; }, [onOpenSelectionMenu]);
  useEffect(() => {
    colorSchemeRef.current = colorScheme;
    preferencesRef.current = preferences;
    renditionRef.current?.themes.default(epubTheme(colorScheme, preferences));
  }, [colorScheme, preferences]);

  const syncHighlights = () => {
    const rendition = renditionRef.current;
    if (!rendition) return;
    const nextHighlights = new Map(highlightsRef.current.map((highlight) => [highlight.id, highlight]));
    for (const [id, applied] of appliedHighlightsRef.current) {
      const next = nextHighlights.get(id);
      if (!next || next.cfi_range !== applied.cfiRange || next.color !== applied.color) {
        rendition.annotations.remove(applied.cfiRange, "highlight");
        appliedHighlightsRef.current.delete(id);
      }
    }
    for (const highlight of highlightsRef.current) {
      if (appliedHighlightsRef.current.has(highlight.id)) continue;
      try {
        rendition.annotations.highlight(
          highlight.cfi_range,
          { id: highlight.id },
          () => {
            setIsHighlightsOpen(true);
            setEditingHighlightId(highlight.id);
            setNoteDraft(highlight.note ?? "");
            setColorDraft(highlight.color);
          },
          `reading-highlight-${highlight.color}`,
          HIGHLIGHT_STYLES[highlight.color] ?? HIGHLIGHT_STYLES.yellow,
        );
        appliedHighlightsRef.current.set(highlight.id, {
          cfiRange: highlight.cfi_range,
          color: highlight.color,
        });
      } catch {
        // A highlight from another EPUB revision may no longer resolve.
      }
    }
  };

  useEffect(() => {
    highlightsRef.current = highlights;
    syncHighlights();
  }, [highlights]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void invoke("set_reading_context_menu_active", { active: true });

    const openNativeSelectionMenu = (nativeX?: number, nativeY?: number) => {
      const rendition = renditionRef.current;
      if (!rendition) return;
      const contentsList = rendition.getContents() as unknown as Contents[];
      for (const contents of contentsList) {
        const selection = contents.window.getSelection();
        const quote = selection?.toString().replace(/\s+/g, " ").trim() ?? "";
        const range = selection?.rangeCount ? selection.getRangeAt(0) : null;
        if (!selection || !quote || !range || range.collapsed) continue;
        let cfiRange: string;
        try {
          cfiRange = contents.cfiFromRange(range);
        } catch {
          continue;
        }
        const nearby = nearbyParagraphs(selection);
        const frame = contents.document.defaultView?.frameElement;
        const frameRect = frame instanceof HTMLElement ? frame.getBoundingClientRect() : null;
        const rangeRect = range.getBoundingClientRect();
        const hasNativePoint = Number.isFinite(nativeX)
          && Number.isFinite(nativeY)
          && (nativeX !== 0 || nativeY !== 0);
        openSelectionMenuRef.current(
          { cfiRange, quote, contextBefore: nearby.before, contextAfter: nearby.after },
          hasNativePoint ? nativeX! : (frameRect?.left ?? 0) + rangeRect.right,
          hasNativePoint ? nativeY! : (frameRect?.top ?? 0) + rangeRect.bottom,
        );
        return;
      }
    };

    void listen<{ x?: number; y?: number }>("reading://native-context-menu", (event) => {
      openNativeSelectionMenu(event.payload.x, event.payload.y);
    }).then((remove) => {
      if (disposed) remove();
      else unlisten = remove;
    });
    return () => {
      disposed = true;
      unlisten?.();
      void invoke("set_reading_context_menu_active", { active: false });
    };
  }, [book.id]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let rendered: Rendition | null = null;
    if (!book.local_path) {
      setError("这本书正在等待 EPUB 下载到本机");
      setLoading(false);
      return;
    }
    const source = convertFileSrc(book.local_path);
    const hookedContents = new WeakSet<object>();
    const hookedFrames = new WeakSet<HTMLIFrameElement>();
    let contextObserver: MutationObserver | null = null;
    let resizeObserver: ResizeObserver | null = null;
    let resizeTimer: number | null = null;

    const load = async () => {
      setLoading(true);
      setError(null);
      try {
        const epub = ePub(source);
        bookRef.current = epub;
        await epub.ready;
        if (disposed) return;
        const navigation = await epub.loaded.navigation;
        if (disposed) return;
        setToc(flattenToc(navigation.toc));
        rendered = epub.renderTo(host, {
          width: "100%",
          height: "100%",
          flow: "scrolled-doc",
          manager: "continuous",
          allowScriptedContent: false,
        });
        renditionRef.current = rendered;
        rendered.themes.default(epubTheme(colorSchemeRef.current, preferencesRef.current));
        rendered.on("relocated", (location: Location) => {
          const cfi = location?.start?.cfi;
          if (!cfi) return;
          onProgress(cfi);
          const percentage = epub.locations.length() > 0
            ? epub.locations.percentageFromCfi(cfi)
            : location.start.percentage;
          if (Number.isFinite(percentage)) {
            setReadingProgress(Math.min(1, Math.max(0, percentage)));
          }
          const href = location.start.href?.split("#")[0];
          const chapter = flattenToc(navigation.toc).find((item) => item.href.split("#")[0] === href);
          setCurrentChapter(chapter?.label?.trim() ?? "");
        });
        const attachContextMenu = (target: Contents | { contents?: Contents }) => {
          // EPUB.js runs this hook twice: first with an IframeView and later
          // with its Contents. Both expose `document`, so prefer `contents`.
          const contents = (target as { contents?: Contents }).contents ?? target as Contents;
          if (!contents) return;
          if (hookedContents.has(contents)) return;
          hookedContents.add(contents);
          let openedOnMouseDownAt = 0;
          let lastSelection: PendingSelection | null = null;
          const readSelection = (): PendingSelection | null => {
            const selection = contents.window.getSelection();
            const quote = selection?.toString().replace(/\s+/g, " ").trim() ?? "";
            const range = selection?.rangeCount ? selection.getRangeAt(0) : null;
            if (!selection || !quote || !range || range.collapsed) return null;
            const nearby = nearbyParagraphs(selection!);
            let cfiRange: string;
            try {
              cfiRange = contents.cfiFromRange(range);
            } catch {
              return null;
            }
            return { cfiRange, quote, contextBefore: nearby.before, contextAfter: nearby.after };
          };
          const openMenuForEvent = (event: MouseEvent, coordinatesAreGlobal = false) => {
            event.preventDefault();
            event.stopImmediatePropagation();
            const selected = readSelection() ?? lastSelection;
            if (!selected) return false;
            const frame = contents.document.defaultView?.frameElement;
            const frameRect = frame instanceof HTMLElement ? frame.getBoundingClientRect() : null;
            openSelectionMenuRef.current(
              selected,
              coordinatesAreGlobal ? event.clientX : (frameRect?.left ?? 0) + event.clientX,
              coordinatesAreGlobal ? event.clientY : (frameRect?.top ?? 0) + event.clientY,
            );
            return true;
          };
          const rememberSelection = () => {
            const selected = readSelection();
            if (selected) lastSelection = selected;
          };
          const handleMouseDown = (event: MouseEvent) => {
            if (event.button !== 2) return;
            if (openMenuForEvent(event)) openedOnMouseDownAt = Date.now();
          };
          const handleContextMenu = (event: MouseEvent) => {
            // Prevent WebKit's native menu even when it briefly clears the selection.
            if (Date.now() - openedOnMouseDownAt < 750) {
              event.preventDefault();
              event.stopImmediatePropagation();
              return;
            }
            openMenuForEvent(event);
          };
          const documentTarget = contents.document;
          documentTarget.addEventListener("selectionchange", rememberSelection, true);
          documentTarget.addEventListener("mouseup", rememberSelection, true);
          contents.window.addEventListener("mousedown", handleMouseDown, true);
          contents.window.addEventListener("contextmenu", handleContextMenu, true);
          contents.window.addEventListener("keydown", (event) => {
            handleReaderNavigationKey(event, renditionRef.current);
          }, true);
          // WebKit dispatches iframe context menus on the document rather than
          // reliably forwarding them to the iframe window.
          documentTarget.addEventListener("mousedown", handleMouseDown, true);
          documentTarget.addEventListener("contextmenu", handleContextMenu, true);
          documentTarget.documentElement?.addEventListener("contextmenu", handleContextMenu, true);
          documentTarget.body?.addEventListener("contextmenu", handleContextMenu, true);
          // WebKitGTK may skip the capture listeners for an iframe context menu;
          // the event-property path still suppresses its native menu.
          documentTarget.oncontextmenu = handleContextMenu;
          if (documentTarget.documentElement) documentTarget.documentElement.oncontextmenu = handleContextMenu;
          if (documentTarget.body) documentTarget.body.oncontextmenu = handleContextMenu;
          const frame = contents.document.defaultView?.frameElement;
          if (frame instanceof HTMLElement) {
            frame.addEventListener("contextmenu", (event) => {
              openMenuForEvent(event, true);
            }, true);
          }
        };
        const attachIframeContents = (iframe: HTMLIFrameElement) => {
          const attach = () => {
            if (disposed || !rendered) return;
            const contents = (rendered.getContents() as unknown as Contents[])
              .find((item) => item.document === iframe.contentDocument);
            if (contents) attachContextMenu(contents);
          };
          if (!hookedFrames.has(iframe)) {
            hookedFrames.add(iframe);
            iframe.addEventListener("load", attach, true);
          }
          attach();
          window.setTimeout(attach, 0);
          window.setTimeout(attach, 100);
        };
        const scanIframes = () => {
          host.querySelectorAll("iframe").forEach((iframe) => attachIframeContents(iframe));
        };
        contextObserver = new MutationObserver(scanIframes);
        contextObserver.observe(host, { childList: true, subtree: true });
        rendered.hooks.content.register(attachContextMenu);
        rendered.on("rendered", (_section: unknown, view: { contents?: Contents }) => {
          attachContextMenu(view);
          syncHighlights();
        });
        rendered.on("selected", (cfiRange: string, contents: Contents) => {
          const selection = contents.window.getSelection();
          const quote = selection?.toString().replace(/\s+/g, " ").trim() ?? "";
          if (!quote) return;
          const nearby = selection ? nearbyParagraphs(selection) : { before: "", after: "" };
          const passage = {
            cfiRange,
            quote,
            contextBefore: nearby.before,
            contextAfter: nearby.after,
          };
          if (highlightModeRef.current) {
            createHighlightRef.current(passage);
            selection?.removeAllRanges();
          }
        });
        await rendered.display(book.progress_cfi || undefined);
        if (!disposed) {
          resizeObserver = new ResizeObserver(() => {
            if (resizeTimer !== null) window.clearTimeout(resizeTimer);
            resizeTimer = window.setTimeout(() => {
              resizeTimer = null;
              if (disposed || !rendered) return;
              const width = host.clientWidth;
              const height = host.clientHeight;
              if (width > 0 && height > 0) rendered.resize(width, height);
            }, 80);
          });
          resizeObserver.observe(host);
          const currentContents = rendered.getContents() as unknown as Contents[];
          currentContents.forEach(attachContextMenu);
          scanIframes();
          syncHighlights();
          setLoading(false);
          void epub.locations.generate(1_200).then(() => {
            const currentCfi = renditionRef.current?.location?.start?.cfi;
            if (!disposed && currentCfi) {
              setReadingProgress(epub.locations.percentageFromCfi(currentCfi));
            }
          }).catch(() => {
            // Location generation is optional; navigation remains available.
          });
        }
      } catch (reason) {
        if (!disposed) {
          setError(String(reason));
          setLoading(false);
        }
      }
    };
    void load();

    return () => {
      disposed = true;
      contextObserver?.disconnect();
      contextObserver = null;
      resizeObserver?.disconnect();
      resizeObserver = null;
      if (resizeTimer !== null) window.clearTimeout(resizeTimer);
      renditionRef.current?.destroy();
      bookRef.current?.destroy();
      renditionRef.current = null;
      bookRef.current = null;
      appliedHighlightsRef.current.clear();
      host.replaceChildren();
    };
  }, [book.id, book.local_path]);

  const navigate = (href: string) => renditionRef.current?.display(href).catch(console.error);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      handleReaderNavigationKey(event, renditionRef.current);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  return (
    <section className="relative flex min-w-0 flex-1 flex-col overflow-hidden bg-[#fbfaf6]">
      <div className="relative flex h-12 shrink-0 items-center gap-2 border-b border-stone-200 bg-white/70 px-3">
        <button onClick={onBackToShelf} className="rounded p-1 text-stone-500 hover:bg-stone-100" title="返回书架"><ArrowLeft className="h-4 w-4" /></button>
        <BookOpen className="h-4 w-4 text-emerald-700" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-semibold text-stone-800">{book.title}</span>
          {currentChapter && <span className="block truncate text-[9px] text-stone-400">{currentChapter}</span>}
        </span>
        <span className="w-9 shrink-0 text-right font-mono text-[9px] tabular-nums text-stone-400" title="阅读进度">
          {Math.round(readingProgress * 100)}%
        </span>
        {toc.length > 0 && (
          <select
            className="max-w-40 rounded-md border border-stone-200 bg-white px-2 py-1 text-xs text-stone-600 outline-none xl:max-w-48"
            defaultValue=""
            onChange={(event) => event.target.value && navigate(event.target.value)}
            aria-label="跳转目录"
          >
            <option value="" disabled>目录</option>
            {toc.map((item) => <option key={item.id} value={item.href}>{item.label}</option>)}
          </select>
        )}
        <button
          onClick={onToggleHighlightMode}
          className={`agnes-reading-highlight-toggle flex h-8 shrink-0 items-center gap-1.5 rounded-md border px-2 text-[10px] font-medium transition-colors ${highlightMode ? "border-amber-300 bg-amber-100 text-amber-800" : "border-stone-200 text-stone-500 hover:bg-stone-100"}`}
          title={highlightMode ? "退出划线模式" : "进入划线模式"}
          aria-pressed={highlightMode}
        >
          <Highlighter className="h-4 w-4" />
          <span className="hidden xl:inline">{highlightMode ? "划线中" : "划线"}</span>
        </button>
        <div className={`hidden h-8 shrink-0 items-center gap-1 rounded-md border border-stone-200 px-1 sm:flex ${highlightMode ? "opacity-100" : "opacity-45"}`} aria-label="划线颜色">
          {HIGHLIGHT_COLORS.map((color) => (
            <button
              key={color}
              type="button"
              disabled={!highlightMode}
              onClick={() => onHighlightColorChange(color)}
              className={`grid h-5 w-5 place-items-center rounded-full border transition-transform hover:scale-110 disabled:cursor-default ${highlightColor === color ? "border-stone-700" : "border-transparent"}`}
              title={`${HIGHLIGHT_COLOR_LABELS[color]}划线`}
              aria-label={`${HIGHLIGHT_COLOR_LABELS[color]}划线`}
              aria-pressed={highlightColor === color}
            >
              <span className="h-3 w-3 rounded-full" style={{ background: HIGHLIGHT_COLOR_SWATCHES[color] }} />
            </button>
          ))}
        </div>
        <button
          type="button"
          onClick={() => {
            setIsHighlightsOpen((open) => !open);
            setIsTypographyOpen(false);
          }}
          className={`relative rounded p-1.5 ${isHighlightsOpen ? "bg-stone-100 text-stone-800" : "text-stone-500 hover:bg-stone-100"}`}
          title="高亮与批注"
          aria-pressed={isHighlightsOpen}
        >
          <NotebookPen className="h-4 w-4" />
          {highlights.length > 0 && (
            <span className="absolute -right-1 -top-1 min-w-3.5 rounded-full bg-stone-700 px-0.5 text-center font-mono text-[8px] leading-[14px] text-white">
              {Math.min(highlights.length, 99)}
            </span>
          )}
        </button>
        <button
          type="button"
          onClick={() => {
            setIsTypographyOpen((open) => !open);
            setIsHighlightsOpen(false);
          }}
          className={`rounded p-1.5 ${isTypographyOpen ? "bg-stone-100 text-stone-800" : "text-stone-500 hover:bg-stone-100"}`}
          title="阅读排版"
          aria-pressed={isTypographyOpen}
        >
          <Settings2 className="h-4 w-4" />
        </button>
        <button onClick={() => renditionRef.current?.prev().catch(console.error)} className="rounded p-1 text-stone-500 hover:bg-stone-100" title="上一页">
          <ChevronLeft className="h-4 w-4" />
        </button>
        <button onClick={() => renditionRef.current?.next().catch(console.error)} className="rounded p-1 text-stone-500 hover:bg-stone-100" title="下一页">
          <ChevronRight className="h-4 w-4" />
        </button>
        <span className="absolute inset-x-0 bottom-0 h-px bg-stone-200">
          <span className="block h-full bg-[#c56f52] transition-[width] duration-300" style={{ width: `${readingProgress * 100}%` }} />
        </span>
      </div>

      {isHighlightsOpen && (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setIsHighlightsOpen(false)} />
          <div className="absolute right-3 top-14 z-30 flex max-h-[min(70vh,560px)] w-[min(340px,calc(100%-24px))] flex-col overflow-hidden rounded-md border border-stone-200 bg-white shadow-xl">
            <header className="flex h-10 shrink-0 items-center gap-2 border-b border-stone-100 px-3">
              <NotebookPen className="h-3.5 w-3.5 text-stone-500" />
              <span className="flex-1 text-xs font-semibold text-stone-700">高亮与批注</span>
              <span className="font-mono text-[9px] text-stone-400">{highlights.length}</span>
              <button type="button" onClick={() => setIsHighlightsOpen(false)} className="rounded p-1 text-stone-400 hover:bg-stone-100" title="关闭"><X className="h-3.5 w-3.5" /></button>
            </header>
            <div className="min-h-0 overflow-y-auto">
              {highlights.length === 0 ? (
                <div className="px-4 py-10 text-center text-xs text-stone-400">暂无高亮</div>
              ) : highlights.map((highlight) => {
                const editing = editingHighlightId === highlight.id;
                return (
                  <article key={highlight.id} className="border-b border-stone-100 px-3 py-3 last:border-b-0">
                    <div className="flex items-start gap-2">
                      <button
                        type="button"
                        onClick={() => navigate(highlight.cfi_range)}
                        className="min-w-0 flex-1 text-left"
                        title="定位到原文"
                      >
                        <span className="line-clamp-3 text-xs leading-5 text-stone-700">{highlight.quote}</span>
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setEditingHighlightId(editing ? null : highlight.id);
                          setNoteDraft(highlight.note ?? "");
                          setColorDraft(highlight.color);
                        }}
                        className="rounded p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                        title="编辑批注"
                      >
                        <Pencil className="h-3.5 w-3.5" />
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          if (window.confirm("删除这条高亮和批注？")) void onDeleteHighlight(highlight);
                        }}
                        className="rounded p-1 text-stone-400 hover:bg-rose-50 hover:text-rose-600"
                        title="删除高亮"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                    {!editing && highlight.note && (
                      <p className="mt-2 whitespace-pre-wrap border-l border-stone-200 pl-2 text-[10px] leading-4 text-stone-500">{highlight.note}</p>
                    )}
                    {editing && (
                      <div className="mt-2 space-y-2">
                        <textarea
                          value={noteDraft}
                          onChange={(event) => setNoteDraft(event.target.value)}
                          maxLength={20_000}
                          rows={3}
                          placeholder="添加批注..."
                          className="w-full resize-y rounded-md border border-stone-200 bg-stone-50 px-2 py-1.5 text-xs leading-5 text-stone-700 outline-none focus:border-stone-400"
                        />
                        <div className="flex items-center gap-1.5">
                          {HIGHLIGHT_COLORS.map((color) => (
                            <button
                              key={color}
                              type="button"
                              onClick={() => setColorDraft(color)}
                              className={`grid h-6 w-6 place-items-center rounded-full border ${colorDraft === color ? "border-stone-700" : "border-transparent"}`}
                              title={HIGHLIGHT_COLOR_LABELS[color]}
                              aria-pressed={colorDraft === color}
                            >
                              <span className="h-3.5 w-3.5 rounded-full" style={{ background: HIGHLIGHT_COLOR_SWATCHES[color] }} />
                            </button>
                          ))}
                          <button
                            type="button"
                            onClick={() => {
                              void onUpdateHighlight(highlight.id, noteDraft.trim() || null, colorDraft)
                                .then(() => setEditingHighlightId(null));
                            }}
                            className="ml-auto grid h-7 w-7 place-items-center rounded-md bg-stone-800 text-white hover:bg-stone-700"
                            title="保存批注"
                          >
                            <Check className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>
                    )}
                  </article>
                );
              })}
            </div>
          </div>
        </>
      )}

      {isTypographyOpen && (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setIsTypographyOpen(false)} />
          <div className="absolute right-3 top-14 z-30 w-72 rounded-md border border-stone-200 bg-white p-4 shadow-xl">
            <div className="mb-4 flex items-center gap-2">
              <Type className="h-4 w-4 text-stone-500" />
              <span className="flex-1 text-xs font-semibold text-stone-700">阅读排版</span>
              <button
                type="button"
                onClick={() => onPreferencesChange(DEFAULT_READING_PREFERENCES)}
                className="rounded p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                title="恢复默认排版"
              >
                <RotateCcw className="h-3.5 w-3.5" />
              </button>
            </div>
            <label className="block text-[10px] text-stone-500">
              <span className="mb-1.5 flex justify-between"><span>字号</span><span className="font-mono">{preferences.fontSize}px</span></span>
              <input type="range" min={14} max={26} step={1} value={preferences.fontSize} onChange={(event) => onPreferencesChange(normalizeReadingPreferences({ ...preferences, fontSize: Number(event.target.value) }))} className="w-full accent-[#c56f52]" />
            </label>
            <label className="mt-4 block text-[10px] text-stone-500">
              <span className="mb-1.5 flex justify-between"><span>行距</span><span className="font-mono">{preferences.lineHeight.toFixed(1)}</span></span>
              <input type="range" min={1.4} max={2.4} step={0.1} value={preferences.lineHeight} onChange={(event) => onPreferencesChange(normalizeReadingPreferences({ ...preferences, lineHeight: Number(event.target.value) }))} className="w-full accent-[#c56f52]" />
            </label>
            <label className="mt-4 block text-[10px] text-stone-500">
              <span className="mb-1.5 flex justify-between"><span>正文宽度</span><span className="font-mono">{preferences.contentWidth}px</span></span>
              <input type="range" min={560} max={1_000} step={20} value={preferences.contentWidth} onChange={(event) => onPreferencesChange(normalizeReadingPreferences({ ...preferences, contentWidth: Number(event.target.value) }))} className="w-full accent-[#c56f52]" />
            </label>
          </div>
        </>
      )}

      {loading && <div className="absolute inset-0 z-10 grid place-items-center bg-[#fbfaf6]/80"><LoaderCircle className="h-5 w-5 animate-spin text-emerald-700" /></div>}
      {error && <div className="m-5 rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs leading-relaxed text-rose-700">无法打开这本 EPUB：{error}</div>}
      <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden" />
    </section>
  );
};

const TRANSLATION_LANGUAGE_SETTING = "ui:translation_target_language";
const READING_PREFERENCES_SETTING = "ui:reading_preferences";

export const ReadingWorkspace: React.FC = () => {
  const activeAgentId = useAgentStore((state) => state.activeAgentId);
  const activeSessionId = useAgentStore((state) => state.activeSessionId);
  const messages = useAgentStore((state) => state.messages);
  const isStreaming = useAgentStore((state) => state.isStreaming);
  const sessions = useAgentStore((state) => state.sessions);
  const providers = useAgentStore((state) => state.providers);
  const loadSessions = useAgentStore((state) => state.loadSessions);
  const sendMessage = useAgentStore((state) => state.sendMessage);
  const setActiveSessionId = useAgentStore((state) => state.setActiveSessionId);
  const setSessionLlm = useAgentStore((state) => state.setSessionLlm);
  const setSessionCompressThreshold = useAgentStore((state) => state.setSessionCompressThreshold);
  const [books, setBooks] = useState<ReadingBook[]>([]);
  const [selectedBookId, setSelectedBookId] = useState<string | null>(null);
  const [highlights, setHighlights] = useState<ReadingHighlight[]>([]);
  const [quotedSelection, setQuotedSelection] = useState<PendingSelection | null>(null);
  const [selectionMenu, setSelectionMenu] = useState<SelectionMenu | null>(null);
  const [question, setQuestion] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showConsent, setShowConsent] = useState(false);
  const [readingSessionId, setReadingSessionId] = useState<string | null>(null);
  const [readingConversations, setReadingConversations] = useState<ReadingConversation[]>([]);
  const [isDiscussionOpen, setIsDiscussionOpen] = useState(true);
  const [isDiscussionOptionsOpen, setIsDiscussionOptionsOpen] = useState(false);
  const [isConversationHistoryOpen, setIsConversationHistoryOpen] = useState(false);
  const [highlightMode, setHighlightMode] = useState(false);
  const [highlightColor, setHighlightColor] = useState("yellow");
  const [translationLanguage, setTranslationLanguage] = useState("中文");
  const [readingPreferences, setReadingPreferences] = useState(DEFAULT_READING_PREFERENCES);
  const [colorScheme, setColorScheme] = useState<ResolvedColorScheme>(getResolvedColorScheme);
  const [autoExpandThoughts, setAutoExpandThoughts] = useState(getCachedAutoExpandThoughts);
  const [autoFollowStreaming, setAutoFollowStreaming] = useState(getCachedAutoFollowStreaming);
  const messageEndRef = useRef<HTMLDivElement>(null);
  const preferenceSaveTimerRef = useRef<number | null>(null);
  const pendingReadingPreferencesRef = useRef<ReadingPreferences | null>(null);

  const selectedBook = useMemo(
    () => books.find((book) => book.id === selectedBookId) ?? null,
    [books, selectedBookId],
  );
  const readingSession = sessions.find((session) => session.id === readingSessionId);
  const readingMaxTokens = readingSession?.max_tokens ?? DEFAULT_MAX_OUTPUT_TOKENS;
  const readingModelRef = readingSession?.model ?? "";
  const readingModelSeparator = readingModelRef.indexOf("/");
  const readingProviderId = readingModelSeparator >= 0 ? readingModelRef.slice(0, readingModelSeparator) : "";
  const readingModelId = readingModelSeparator >= 0 ? readingModelRef.slice(readingModelSeparator + 1) : readingModelRef;
  const readingModelDescriptor = providers
    .find((provider) => provider.id === readingProviderId)
    ?.models.find((model) => model.id === readingModelId);
  const readingContextLimit = readingSession?.context_limit ?? readingModelDescriptor?.context_window ?? 8192;
  const readingCompressThreshold = readingSession?.compress_threshold ?? 0.85;
  const latestReadingAssistant = [...messages]
    .reverse()
    .find((message) => message.role === "assistant" && message.status === "complete");
  const readingContextTokens = latestReadingAssistant?.context_tokens ?? 0;
  const readingContextPercent = Math.min(100, (readingContextTokens / Math.max(1, readingContextLimit)) * 100);
  const readingSummaryTrigger = Math.floor(readingContextLimit * readingCompressThreshold);

  const loadBooks = async () => {
    const next = await invoke<ReadingBook[]>("list_reading_books");
    setBooks(next);
    setSelectedBookId((current) => current && next.some((book) => book.id === current) ? current : null);
  };

  useEffect(() => {
    void loadBooks().catch((reason) => setError(String(reason)));
    void invoke<string | null>("get_setting", { key: TRANSLATION_LANGUAGE_SETTING })
      .then((value) => {
        if (value === "中文" || value === "English") setTranslationLanguage(value);
      })
      .catch(console.error);
    void invoke<string | null>("get_setting", { key: READING_PREFERENCES_SETTING })
      .then((value) => setReadingPreferences(parseReadingPreferences(value)))
      .catch(console.error);
  }, []);

  useEffect(() => () => {
    if (preferenceSaveTimerRef.current !== null) {
      window.clearTimeout(preferenceSaveTimerRef.current);
      preferenceSaveTimerRef.current = null;
    }
    const pending = pendingReadingPreferencesRef.current;
    if (pending) {
      pendingReadingPreferencesRef.current = null;
      void invoke("set_setting", {
        key: READING_PREFERENCES_SETTING,
        value: JSON.stringify(pending),
      }).catch(console.error);
    }
  }, []);

  useEffect(() => subscribeUIPreferenceChanges((change) => {
    if (change.resolvedColorScheme !== undefined) setColorScheme(change.resolvedColorScheme);
    if (change.autoExpandThoughts !== undefined) {
      setAutoExpandThoughts(change.autoExpandThoughts);
    }
    if (change.autoFollowStreaming !== undefined) {
      setAutoFollowStreaming(change.autoFollowStreaming);
    }
  }), []);

  useEffect(() => {
    if (!selectedBook) { setHighlights([]); return; }
    void invoke<ReadingHighlight[]>("list_reading_highlights", { bookId: selectedBook.id })
      .then(setHighlights)
      .catch((reason) => setError(String(reason)));
  }, [selectedBook?.id]);

  useEffect(() => {
    let cancelled = false;
    setReadingSessionId(null);
    setReadingConversations([]);
    setIsConversationHistoryOpen(false);
    if (!selectedBook || !activeAgentId) return () => { cancelled = true; };
    void invoke<string>("open_reading_book_conversation", { bookId: selectedBook.id, agentId: activeAgentId })
      .then(async (sessionId) => {
        await setActiveSessionId(sessionId);
        const conversations = await invoke<ReadingConversation[]>("list_reading_book_conversations", {
          bookId: selectedBook.id,
          agentId: activeAgentId,
        });
        if (!cancelled) {
          setReadingSessionId(sessionId);
          setReadingConversations(conversations);
        }
      })
      .catch((reason) => {
        if (!cancelled) setError(String(reason));
      });
    return () => { cancelled = true; };
  }, [selectedBook?.id, activeAgentId]);

  useEffect(() => {
    const latestMessage = messages[messages.length - 1];
    if (!autoFollowStreaming && latestMessage?.role !== "user") return;
    messageEndRef.current?.scrollIntoView({ behavior: isStreaming ? "auto" : "smooth" });
  }, [messages, isStreaming, autoFollowStreaming]);

  const importBook = async () => {
    if (!activeAgentId) return;
    const path = await open({
      multiple: false,
      title: "导入 EPUB 电子书",
      filters: [{ name: "EPUB 电子书", extensions: ["epub"] }],
    });
    if (typeof path !== "string") return;
    setLoading(true);
    setError(null);
    try {
      const book = await invoke<ReadingBook>("import_reading_book", { agentId: activeAgentId, path });
      await loadBooks();
      setSelectedBookId(book.id);
      setIsDiscussionOpen(true);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const publishBook = async (bookId: string) => {
    setLoading(true);
    setError(null);
    try {
      await invoke<ReadingEpubPublishResult>("publish_reading_epub", { bookId });
      await loadBooks();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const updateBook = async (command: string, args: Record<string, unknown>) => {
    if (!selectedBook) return;
    try {
      const next = await invoke<ReadingBook>(command, { bookId: selectedBook.id, ...args });
      setBooks((current) => current.map((book) => book.id === next.id ? next : book));
    } catch (reason) {
      setError(String(reason));
    }
  };

  const updateReadingPreferences = (preferences: ReadingPreferences) => {
    const normalized = normalizeReadingPreferences(preferences);
    setReadingPreferences(normalized);
    pendingReadingPreferencesRef.current = normalized;
    if (preferenceSaveTimerRef.current !== null) {
      window.clearTimeout(preferenceSaveTimerRef.current);
    }
    preferenceSaveTimerRef.current = window.setTimeout(() => {
      preferenceSaveTimerRef.current = null;
      pendingReadingPreferencesRef.current = null;
      void invoke("set_setting", {
        key: READING_PREFERENCES_SETTING,
        value: JSON.stringify(normalized),
      }).catch((reason) => setError(String(reason)));
    }, 250);
  };

  const updateHighlight = async (highlightId: string, note: string | null, color: string) => {
    try {
      const next = await invoke<ReadingHighlight>("update_reading_highlight", {
        highlightId,
        payload: { note, color },
      });
      setHighlights((current) => current.map((highlight) => highlight.id === next.id ? next : highlight));
    } catch (reason) {
      setError(String(reason));
      throw reason;
    }
  };

  const deleteHighlight = async (highlight: ReadingHighlight) => {
    try {
      await invoke("delete_reading_highlight", { highlightId: highlight.id });
      setHighlights((current) => current.filter((item) => item.id !== highlight.id));
    } catch (reason) {
      setError(String(reason));
      throw reason;
    }
  };

  const createHighlight = async (selection: PendingSelection, color = highlightColor) => {
    if (!selectedBook) return;
    try {
      const existing = highlights.find((item) => item.cfi_range === selection.cfiRange);
      if (existing) {
        await updateHighlight(existing.id, existing.note, color);
        return;
      }
      const highlight = await invoke<ReadingHighlight>("create_reading_highlight", {
        bookId: selectedBook.id,
        payload: { ...selection, color },
      });
      setHighlights((current) => current.some((item) => item.cfi_range === highlight.cfi_range) ? current : [...current, highlight]);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const submitQuestion = async (allowBookContext?: boolean) => {
    if (!selectedBook || !readingSessionId || !question.trim() || isStreaming) return;
    try {
      if (
        allowBookContext !== undefined
        && (selectedBook.content_context_allowed !== allowBookContext || !selectedBook.content_context_decided)
      ) {
        const next = await invoke<ReadingBook>("set_reading_book_content_context_allowed", {
          bookId: selectedBook.id,
          allowed: allowBookContext,
        });
        setBooks((current) => current.map((book) => book.id === next.id ? next : book));
      }
      const text = readingQuestion(selectedBook, question.trim(), quotedSelection);
      setQuestion("");
      setQuotedSelection(null);
      setShowConsent(false);
      await sendMessage(readingSessionId, text, selectedBook.id);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const ask = () => {
    if (!selectedBook || selectedBook.model_knows_content || selectedBook.content_context_decided) {
      void submitQuestion();
    } else {
      setShowConsent(true);
    }
  };

  const openSelectionMenu = (selection: PendingSelection, x: number, y: number) => {
    setSelectionMenu({
      selection,
      x: Math.min(Math.max(x + 6, 8), window.innerWidth - 184),
      y: Math.min(Math.max(y + 6, 8), window.innerHeight - 148),
    });
  };

  const copySelection = async () => {
    if (!selectionMenu) return;
    try {
      await navigator.clipboard.writeText(selectionMenu.selection.quote);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSelectionMenu(null);
    }
  };

  const translateSelection = async () => {
    if (!selectedBook || !readingSessionId || !selectionMenu || isStreaming) return;
    try {
      const text = translationRequest(selectedBook, selectionMenu.selection, translationLanguage);
      setSelectionMenu(null);
      setQuotedSelection(null);
      await sendMessage(readingSessionId, text, selectedBook.id);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const refreshReadingConversations = async (bookId: string, agentId: string) => {
    const conversations = await invoke<ReadingConversation[]>("list_reading_book_conversations", {
      bookId,
      agentId,
    });
    setReadingConversations(conversations);
  };

  const switchReadingConversation = async (sessionId: string) => {
    if (!selectedBook || !activeAgentId || sessionId === readingSessionId || isStreaming) {
      setIsConversationHistoryOpen(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      await invoke("select_reading_book_conversation", {
        bookId: selectedBook.id,
        agentId: activeAgentId,
        sessionId,
      });
      setQuotedSelection(null);
      setSelectionMenu(null);
      setQuestion("");
      await setActiveSessionId(sessionId);
      setReadingSessionId(sessionId);
      await refreshReadingConversations(selectedBook.id, activeAgentId);
      setIsConversationHistoryOpen(false);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const conversationReady = readingSessionId !== null && readingSessionId === activeSessionId;
  const openBook = (bookId: string) => {
    setSelectedBookId(bookId);
    setQuotedSelection(null);
    setSelectionMenu(null);
    setShowConsent(false);
    setIsDiscussionOpen(true);
  };
  const returnToShelf = () => {
    setSelectedBookId(null);
    setQuotedSelection(null);
    setSelectionMenu(null);
    setQuestion("");
    setShowConsent(false);
    setIsConversationHistoryOpen(false);
  };

  return (
    <main className="agnes-feature-workspace agnes-reading-workspace flex min-w-0 flex-1 flex-col bg-[#faf9f5] lg:flex-row">
      {selectedBook ? (
        <div className="relative flex min-h-64 min-w-0 flex-1 flex-col lg:min-h-0 lg:flex-row" onClick={() => setSelectionMenu(null)}>
          <EpubPane
            book={selectedBook}
            highlights={highlights}
            highlightMode={highlightMode}
            highlightColor={highlightColor}
            colorScheme={colorScheme}
            preferences={readingPreferences}
            onBackToShelf={returnToShelf}
            onToggleHighlightMode={() => setHighlightMode((enabled) => !enabled)}
            onHighlightColorChange={setHighlightColor}
            onProgress={(cfi) => {
              setBooks((current) => current.map((book) => book.id === selectedBook.id ? { ...book, progress_cfi: cfi } : book));
              void invoke("update_reading_book_progress", { bookId: selectedBook.id, cfi }).catch(console.error);
            }}
            onCreateHighlight={(selection) => void createHighlight(selection)}
            onUpdateHighlight={updateHighlight}
            onDeleteHighlight={deleteHighlight}
            onPreferencesChange={updateReadingPreferences}
            onOpenSelectionMenu={openSelectionMenu}
          />

          <aside
            className={`flex w-full shrink-0 flex-col overflow-hidden border-t border-stone-200 bg-white transition-[width,min-width,height,min-height] duration-300 ease-out motion-reduce:transition-none lg:h-auto lg:min-h-0 lg:border-l lg:border-t-0 ${
              isDiscussionOpen
                ? "h-[42%] min-h-64 lg:w-[min(32vw,400px)] lg:min-w-80"
                : "h-10 min-h-10 lg:w-9 lg:min-w-9"
            }`}
          >
            {isDiscussionOpen ? (
              <>
              <header className="relative border-b border-stone-200">
                <div className="flex h-12 items-center gap-2 px-4">
                  <MessageCircleMore className="h-4 w-4 text-emerald-700" />
                  <span className="min-w-0 flex-1 text-sm font-semibold text-stone-800">阅读讨论</span>
                  <div
                    className="hidden items-center gap-1.5 sm:flex"
                    title={`上下文 ${readingContextTokens.toLocaleString()} / ${readingContextLimit.toLocaleString()} Token；总结阈值 ${readingSummaryTrigger.toLocaleString()} Token`}
                  >
                    <div className="h-1.5 w-14 overflow-hidden rounded-full bg-stone-200">
                      <div
                        className={`h-full rounded-full ${readingContextPercent >= readingCompressThreshold * 100 ? "bg-amber-500" : "bg-emerald-600"}`}
                        style={{ width: `${readingContextPercent}%` }}
                      />
                    </div>
                    <span className="font-mono text-[9px] tabular-nums text-stone-400">{readingContextPercent.toFixed(0)}%</span>
                  </div>
                  <button
                    onClick={() => setIsConversationHistoryOpen((open) => !open)}
                    disabled={!conversationReady || loading}
                    className={`agnes-reading-panel-action rounded p-1 disabled:opacity-40 ${isConversationHistoryOpen ? "text-stone-700" : "text-stone-400 hover:text-stone-700"}`}
                    title="讨论历史"
                    aria-pressed={isConversationHistoryOpen}
                  >
                    <History className="h-4 w-4" />
                  </button>
                  <button
                    onClick={() => {
                      if (!selectedBook || !activeAgentId || isStreaming) return;
                      setLoading(true);
                      setError(null);
                      void invoke<string>("new_reading_book_conversation", {
                        bookId: selectedBook.id,
                        agentId: activeAgentId,
                      })
                        .then(async (sessionId) => {
                          setQuotedSelection(null);
                          setSelectionMenu(null);
                          setQuestion("");
                          await setActiveSessionId(sessionId);
                          await loadSessions(activeAgentId);
                          setReadingSessionId(sessionId);
                          await refreshReadingConversations(selectedBook.id, activeAgentId);
                          setIsConversationHistoryOpen(false);
                        })
                        .catch((reason) => setError(String(reason)))
                        .finally(() => setLoading(false));
                    }}
                    disabled={!conversationReady || isStreaming || loading}
                    className="agnes-reading-panel-action rounded p-1 text-stone-400 hover:text-stone-700 disabled:opacity-40"
                    title="新建讨论"
                  >
                    <Plus className="h-4 w-4" />
                  </button>
                  <button onClick={() => setIsDiscussionOptionsOpen((open) => !open)} className={`agnes-reading-panel-action rounded p-1 ${isDiscussionOptionsOpen ? "text-stone-700" : "text-stone-400 hover:text-stone-700"}`} title="讨论设置" aria-pressed={isDiscussionOptionsOpen}><SlidersHorizontal className="h-4 w-4" /></button>
                  <button onClick={() => setIsDiscussionOpen(false)} className="agnes-reading-panel-action rounded p-1 text-stone-400 hover:text-stone-700" title="收起讨论" aria-expanded="true"><PanelRightClose className="h-4 w-4" /></button>
                </div>
                {isConversationHistoryOpen && (
                  <>
                    <div className="fixed inset-0 z-20" onClick={() => setIsConversationHistoryOpen(false)} />
                    <div className="absolute right-12 top-10 z-30 w-64 overflow-hidden rounded-md border border-stone-200 bg-white shadow-xl">
                      <div className="border-b border-stone-100 px-3 py-2 text-[11px] font-semibold text-stone-500">讨论历史</div>
                      <div className="max-h-64 overflow-y-auto py-1">
                        {readingConversations.map((conversation, index) => (
                          <button
                            key={conversation.session_id}
                            onClick={() => void switchReadingConversation(conversation.session_id)}
                            disabled={isStreaming || loading}
                            className={`flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-stone-50 disabled:opacity-50 ${conversation.session_id === readingSessionId ? "bg-emerald-50" : ""}`}
                          >
                            <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${conversation.session_id === readingSessionId ? "bg-emerald-600" : "bg-stone-300"}`} />
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-xs font-medium text-stone-700">
                                {conversation.session_id === readingSessionId ? "当前讨论" : `历史讨论 ${readingConversations.length - index}`}
                              </span>
                              <span className="mt-0.5 block text-[10px] text-stone-400">{conversationDate(conversation.created_at)}</span>
                            </span>
                          </button>
                        ))}
                      </div>
                    </div>
                  </>
                )}
                {isDiscussionOptionsOpen && (
                  <div className="border-t border-stone-100 px-4 py-3">
                    <label className="flex cursor-pointer items-center gap-2 text-[11px] text-stone-500">
                      <input type="checkbox" checked={selectedBook.model_knows_content} onChange={(event) => void updateBook("update_reading_book_mode", { modelKnowsContent: event.target.checked })} className="accent-emerald-700" />
                      AI 已知这本书
                    </label>
                    {!selectedBook.model_knows_content && selectedBook.content_context_decided && (
                      <label className="mt-2 flex cursor-pointer items-center gap-2 text-[11px] text-stone-500">
                        <input type="checkbox" checked={selectedBook.content_context_allowed} onChange={(event) => void updateBook("set_reading_book_content_context_allowed", { allowed: event.target.checked })} className="accent-emerald-700" />
                        允许检索本书片段
                      </label>
                    )}
                    <label className="mt-3 flex items-center gap-2 border-t border-stone-100 pt-3 text-[11px] text-stone-500">
                      <span className="min-w-0 flex-1">最大输出 Token</span>
                      <input
                        key={`${readingSessionId}-${readingMaxTokens}`}
                        type="number"
                        min={128}
                        max={1048576}
                        step={256}
                        defaultValue={readingMaxTokens}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") event.currentTarget.blur();
                        }}
                        onBlur={(event) => {
                          if (!readingSessionId || !readingSession) return;
                          const value = Math.min(1048576, Math.max(128, Number(event.currentTarget.value) || DEFAULT_MAX_OUTPUT_TOKENS));
                          event.currentTarget.value = String(value);
                          if (value !== readingMaxTokens) {
                            void setSessionLlm(
                              readingSessionId,
                              readingSession.model,
                              readingSession.thinking_mode,
                              readingSession.thinking_budget,
                              value,
                            ).catch((reason) => setError(String(reason)));
                          }
                        }}
                        className="h-7 w-24 rounded-md border border-stone-200 bg-stone-50 px-2 text-right font-mono text-[10px] text-stone-700 outline-none focus:border-emerald-400"
                      />
                    </label>
                    <label className="mt-3 flex items-center gap-2 border-t border-stone-100 pt-3 text-[11px] text-stone-500">
                      <span className="min-w-0 flex-1">自动总结阈值</span>
                      <input
                        key={`${readingSessionId}-${readingCompressThreshold}`}
                        type="number"
                        min={0}
                        max={1}
                        step={0.05}
                        defaultValue={readingCompressThreshold}
                        onClick={(event) => event.stopPropagation()}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") event.currentTarget.blur();
                        }}
                        onBlur={(event) => {
                          if (!readingSessionId) return;
                          const value = Math.min(1, Math.max(0, Number(event.currentTarget.value) || 0));
                          event.currentTarget.value = String(value);
                          if (value !== readingCompressThreshold) {
                            void setSessionCompressThreshold(readingSessionId, value).catch((reason) => setError(String(reason)));
                          }
                        }}
                        className="h-7 w-20 rounded-md border border-stone-200 bg-stone-50 px-2 text-right font-mono text-[10px] text-stone-700 outline-none focus:border-emerald-400"
                        aria-label="自动总结阈值"
                      />
                      <span className="shrink-0 text-[9px] text-stone-400">触发 {readingSummaryTrigger.toLocaleString()}</span>
                    </label>
                  </div>
                )}
              </header>
              <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-5">
                {conversationReady && messages.map((message, messageIndex) => (
                  <div key={message.id} className={message.role === "user" ? "ml-6" : "mr-3"}>
                    <div className={`rounded-md px-3 py-2 text-sm ${message.role === "user" ? "bg-emerald-50 text-stone-800" : "border border-stone-200 bg-white text-stone-700"}`}>
                      <div className="space-y-2.5">
                        {message.parts.map((part) => {
                          if (part.kind === "model_fallback") {
                            return (
                              <div key={part._renderKey ?? part.id} className="flex items-center gap-2 border-y border-amber-200 bg-amber-50/60 px-2.5 py-2 text-[10px] text-amber-800">
                                <RefreshCw className="h-3.5 w-3.5 shrink-0" />
                                <span>{part.content}</span>
                              </div>
                            );
                          }
                          if (part.kind === "thought") {
                            const isLiveThought = isStreaming
                              && messageIndex === messages.length - 1
                              && message.status !== "complete"
                              && message._streamingInThought === true;
                            return (
                              <AnimatedDisclosure
                                key={part._renderKey ?? part.id}
                                defaultOpen={autoExpandThoughts}
                                className="agnes-thought-details group"
                                summaryClassName="agnes-thought-summary"
                                summary={(
                                  <>
                                    <span>Agent思维过程</span>
                                    <ChevronDown className="agnes-collapse-chevron h-3 w-3" />
                                  </>
                                )}
                              >
                                <div className="agnes-thought-content">
                                  <span
                                    className={`agnes-thought-status-icon ${isLiveThought ? "animate-pulse" : ""}`}
                                    aria-hidden="true"
                                  >
                                    <Cpu className="h-3 w-3" />
                                  </span>
                                  <p className="whitespace-pre-wrap">{part.content}</p>
                                </div>
                              </AnimatedDisclosure>
                            );
                          }
                          if (part.kind !== "text") return null;
                          return (
                            <MarkdownMessage
                              key={part._renderKey ?? part.id}
                              content={part.content}
                              streaming={isStreaming && message.status !== "complete"}
                            />
                          );
                        })}
                      </div>
                      {message.role === "assistant" && (
                        <div className="mt-2 border-t border-stone-100 pt-1.5 text-right font-mono text-[9px] tabular-nums text-stone-400">
                          输入 {message.input_tokens ?? 0} · 缓存 {message.cached_tokens ?? 0} · 输出 {message.output_tokens ?? 0}
                        </div>
                      )}
                    </div>
                  </div>
                ))}
                {(!conversationReady || !messages.length) && <p className="py-12 text-center text-xs text-stone-400">暂无讨论</p>}
                <div ref={messageEndRef} />
              </div>
              <div className="border-t border-stone-200 p-3">
                {quotedSelection && (
                  <div className="mb-2 flex items-center gap-2 rounded-md border border-stone-200 bg-stone-50 px-2.5 py-2 text-[11px] text-stone-600">
                    <Quote className="h-3.5 w-3.5 shrink-0 text-emerald-700" />
                    <span className="min-w-0 flex-1 truncate">{quotedSelection.quote}</span>
                    <button onClick={() => setQuotedSelection(null)} className="rounded p-0.5 text-stone-400 hover:bg-stone-200 hover:text-stone-700" title="移除引用"><X className="h-3.5 w-3.5" /></button>
                  </div>
                )}
                <textarea value={question} onChange={(event) => setQuestion(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); ask(); } }} placeholder="问问这本书..." className="h-20 w-full resize-none rounded-md border border-stone-200 bg-white px-3 py-2 text-sm outline-none focus:border-emerald-500" />
                <button onClick={ask} disabled={!question.trim() || isStreaming || !conversationReady} className="mt-2 flex w-full items-center justify-center gap-2 rounded-md bg-emerald-700 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-800 disabled:opacity-50"><Send className="h-3.5 w-3.5" />发送</button>
              </div>
              </>
            ) : (
              <button
                onClick={() => setIsDiscussionOpen(true)}
                className="agnes-reading-panel-action flex h-full w-full items-center justify-center text-stone-400 hover:text-stone-700 lg:items-start lg:pt-4"
                title="展开讨论"
                aria-expanded="false"
              >
                <PanelRightOpen className="h-4 w-4" />
              </button>
            )}
          </aside>
        </div>
      ) : (
        <section className="flex min-w-0 flex-1 flex-col bg-[#fbfaf6]">
          <header className="flex h-14 shrink-0 items-center justify-between border-b border-stone-200 bg-white/70 px-5">
            <div className="flex items-center gap-2 text-sm font-semibold text-stone-800"><BookMarked className="h-4 w-4 text-emerald-700" />书架</div>
            <button onClick={() => void importBook()} disabled={!activeAgentId || loading} className="flex items-center gap-2 rounded-md bg-emerald-700 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-800 disabled:opacity-50">
              {loading ? <LoaderCircle className="h-3.5 w-3.5 animate-spin" /> : <FileUp className="h-3.5 w-3.5" />} 导入 EPUB
            </button>
          </header>
          {books.length ? (
            <div className="grid auto-rows-min grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-3 overflow-y-auto p-5">
              {books.map((book) => (
                <article key={book.id} className="group min-h-28 rounded-md border border-stone-200 bg-white p-4 text-left text-stone-600 transition-colors hover:border-emerald-300 hover:bg-emerald-50/50">
                  <button onClick={() => book.local_path && openBook(book.id)} disabled={!book.local_path} className="block w-full text-left disabled:cursor-not-allowed disabled:opacity-55">
                    <BookOpen className="mb-5 h-5 w-5 text-emerald-700" />
                    <span className="block truncate text-sm font-semibold text-stone-700">{book.title}</span>
                    <span className="mt-1 block truncate text-[11px] text-stone-400">{book.author || "未知作者"}</span>
                    {!book.local_path && <span className="mt-2 block text-[10px] text-amber-700">等待 EPUB 下载</span>}
                  </button>
                  <div className="mt-3 flex items-center justify-between gap-2 border-t border-stone-100 pt-2">
                    <span className="truncate text-[10px] text-stone-400">
                      {book.ready_replica_count > 0 ? `已发布 · ${book.ready_replica_count} 个副本` : "仅本机"}
                    </span>
                    <button
                      onClick={() => void publishBook(book.id)}
                      disabled={!book.local_path || loading}
                      title={book.ready_replica_count > 0 ? "重新确认 EPUB 云端副本" : "发布加密 EPUB"}
                      className="flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[10px] font-medium text-emerald-700 hover:bg-emerald-100 disabled:cursor-not-allowed disabled:text-stone-300"
                    >
                      {loading ? <LoaderCircle className="h-3 w-3 animate-spin" /> : <CloudUpload className="h-3 w-3" />}
                      {book.ready_replica_count > 0 ? "已发布" : "发布"}
                    </button>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="grid flex-1 place-items-center"><BookMarked className="h-7 w-7 text-stone-300" /></div>
          )}
        </section>
      )}

      {selectionMenu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setSelectionMenu(null)} onContextMenu={(event) => { event.preventDefault(); setSelectionMenu(null); }} />
          <div className="fixed z-50 w-44 overflow-hidden rounded-md border border-stone-200 bg-white py-1 text-xs text-stone-700 shadow-2xl" style={{ left: selectionMenu.x, top: selectionMenu.y }}>
            <button onClick={() => void copySelection()} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-stone-100"><Copy className="h-3.5 w-3.5 text-stone-500" />复制</button>
            <button onClick={() => { setQuotedSelection(selectionMenu.selection); setSelectionMenu(null); setIsDiscussionOpen(true); }} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-stone-100"><Quote className="h-3.5 w-3.5 text-stone-500" />引用</button>
            <button onClick={() => void translateSelection()} disabled={!conversationReady || isStreaming} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-stone-100 disabled:cursor-not-allowed disabled:opacity-40"><Languages className="h-3.5 w-3.5 text-stone-500" />翻译</button>
            <div className="mt-1 flex items-center gap-1 border-t border-stone-100 px-3 py-2">
              <Highlighter className="mr-auto h-3.5 w-3.5 text-stone-500" />
              {HIGHLIGHT_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  onClick={() => {
                    const selection = selectionMenu.selection;
                    setSelectionMenu(null);
                    void createHighlight(selection, color);
                  }}
                  className="grid h-6 w-6 place-items-center rounded-full border border-transparent hover:border-stone-400"
                  title={`${HIGHLIGHT_COLOR_LABELS[color]}高亮`}
                  aria-label={`${HIGHLIGHT_COLOR_LABELS[color]}高亮`}
                >
                  <span className="h-3.5 w-3.5 rounded-full" style={{ background: HIGHLIGHT_COLOR_SWATCHES[color] }} />
                </button>
              ))}
            </div>
          </div>
        </>
      )}

      {showConsent && selectedBook && (
        <div className="fixed inset-0 z-50 grid place-items-center bg-stone-900/25 p-5">
          <div className="w-full max-w-md rounded-lg bg-white p-5 shadow-xl">
            <div className="flex items-center gap-2 text-sm font-semibold text-stone-900"><ShieldAlert className="h-4 w-4 text-amber-600" />允许书内检索？</div>
            <p className="mt-3 text-sm leading-relaxed text-stone-600">这本书尚未标记为 AI 已知。允许后，讨论时会向当前模型服务发送划线附近文本及最多数段本书的相关片段，不会发送整本 EPUB。</p>
            <div className="mt-5 flex justify-end gap-2">
              <button onClick={() => { setShowConsent(false); void submitQuestion(false); }} className="rounded-md border border-stone-200 px-3 py-2 text-xs font-semibold text-stone-600 hover:bg-stone-50">仅讨论选段</button>
              <button onClick={() => void submitQuestion(true)} className="rounded-md bg-emerald-700 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-800">允许书内检索</button>
            </div>
          </div>
        </div>
      )}
      {error && <div className="fixed bottom-4 left-1/2 z-[60] max-w-xl -translate-x-1/2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700 shadow">{error}</div>}
    </main>
  );
};
