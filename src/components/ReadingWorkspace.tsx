import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import ePub, { type Book, type Contents, type Location, type NavItem, type Rendition } from "epubjs";
import { ArrowLeft, BookmarkSimple, BookOpen, Check, CaretDown, CaretLeft, CaretRight, Copy, Cpu, CloudArrowUp, FileArrowUp, HardDrives, HighlighterCircle, ClockCounterClockwise, Translate, CircleNotch, ChatCircleDots, Notepad, SidebarSimple, PencilSimple, Plus, Quotes, ArrowsClockwise, ArrowCounterClockwise, PaperPlaneRight, Faders, ShieldWarning, SlidersHorizontal, Sparkle, Trash, TextT, X } from "@phosphor-icons/react";
import { AnimatedDisclosure } from "./AnimatedDisclosure";
import { MarkdownMessage } from "./MarkdownMessage";
import { useAgentStore } from "../store/useAgentStore";
import { useConfirmDialog } from "./ConfirmDialog";
import {
  DEFAULT_READING_PREFERENCES,
  normalizeReadingPreferences,
  parseReadingPreferences,
  type ReadingPreferences,
} from "../lib/readingPreferences";
import {
  DEFAULT_CONTEXT_LIMIT,
  DEFAULT_MAX_OUTPUT_TOKENS,
  getCachedAutoFollowStreaming,
  getCachedAutoExpandThoughts,
  getResolvedColorScheme,
  subscribeUIPreferenceChanges,
  type ResolvedColorScheme,
} from "../lib/uiPreferences";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { getRootZoomScale, viewportPointToLocalPoint } from "../lib/zoomGeometry";

/** Shared ease-out curve for panel / message entrances. */
const EASE_OUT: [number, number, number, number] = [0.22, 1, 0.36, 1];
/** Panel entrance is short on purpose: this is a utility surface, not a showcase. */
const PANEL_MOTION = {
  initial: { opacity: 0, y: -6, scale: 0.97 },
  animate: { opacity: 1, y: 0, scale: 1 },
  exit: { opacity: 0, y: -6, scale: 0.97 },
  transition: { duration: 0.14, ease: EASE_OUT },
} as const;

/** Offered as one-tap chips in the empty state; they fill the composer rather than sending. */
const READING_STARTER_PROMPTS = [
  "帮我总结这一章的要点",
  "作者在这里的核心论点是什么？",
  "这个概念我没看懂，讲得通俗一点",
];

const POPOVER_GAP = 6;
const POPOVER_PADDING = 8;

interface ReadingPopoverProps {
  anchorRef: React.RefObject<HTMLButtonElement | null>;
  onClose: () => void;
  className?: string;
  children: React.ReactNode;
}

/**
 * Dropdown rendered through a portal. The discussion panel lives inside an
 * `overflow-hidden` sidebar, so an in-flow absolutely positioned panel gets
 * clipped to the sidebar box — portalling to <body> is what lets it overflow.
 */
const ReadingPopover: React.FC<ReadingPopoverProps> = ({ anchorRef, onClose, className, children }) => {
  const popoverRef = useRef<HTMLDivElement>(null);
  const [layout, setLayout] = useState<{ left: number; top: number; maxHeight: number; flipped: boolean } | null>(null);
  // Kept in a ref so re-renders with a fresh inline callback don't re-run the layout effect.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useLayoutEffect(() => {
    const anchor = anchorRef.current;
    const node = popoverRef.current;
    if (!anchor || !node) return;
    const zoom = getRootZoomScale();
    const rect = anchor.getBoundingClientRect();
    const { x: anchorRight, y: anchorBottom } = viewportPointToLocalPoint(rect.right, rect.bottom, zoom);
    const anchorTop = rect.top / zoom;
    const width = node.getBoundingClientRect().width / zoom;
    const wanted = node.getBoundingClientRect().height / zoom;
    const viewportWidth = document.documentElement.clientWidth;
    const viewportHeight = document.documentElement.clientHeight;
    const spaceBelow = viewportHeight - anchorBottom - POPOVER_GAP - POPOVER_PADDING;
    const spaceAbove = anchorTop - POPOVER_GAP - POPOVER_PADDING;
    // Flip above the anchor only when that really does give more room.
    const placeAbove = wanted > spaceBelow && spaceAbove > spaceBelow;
    const maxHeight = Math.max(160, Math.min(wanted, placeAbove ? spaceAbove : spaceBelow));
    setLayout({
      left: Math.max(
        POPOVER_PADDING,
        Math.min(anchorRight - width, viewportWidth - width - POPOVER_PADDING),
      ),
      top: placeAbove
        ? Math.max(POPOVER_PADDING, anchorTop - POPOVER_GAP - maxHeight)
        : Math.max(POPOVER_PADDING, Math.min(anchorBottom + POPOVER_GAP, viewportHeight - maxHeight - POPOVER_PADDING)),
      maxHeight,
      flipped: placeAbove,
    });
    const close = () => onCloseRef.current();
    window.addEventListener("resize", close);
    return () => window.removeEventListener("resize", close);
  }, [anchorRef]);

  return createPortal(
    <>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        transition={{ duration: 0.12 }}
        className="fixed inset-0 z-[150]"
        onClick={onClose}
      />
      <motion.div
        ref={popoverRef}
        {...PANEL_MOTION}
        className={`fixed z-[151] flex flex-col overflow-hidden ${layout?.flipped ? "origin-bottom" : "origin-top"} ${layout ? "" : "invisible"} ${className ?? ""}`}
        style={{ left: layout?.left ?? 0, top: layout?.top ?? 0, maxHeight: layout?.maxHeight }}
      >
        {children}
      </motion.div>
    </>,
    document.body,
  );
};

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

const READING_THINKING_OPTIONS: { value: string; label: string; desc: string }[] = [
  { value: "off", label: "关闭", desc: "不启用思考" },
  { value: "auto", label: "自动", desc: "由模型决定思考深度" },
  { value: "low", label: "轻度", desc: "浅层思考，响应更快" },
  { value: "medium", label: "中等", desc: "常规思考深度" },
  { value: "high", label: "深度", desc: "深入推理，消耗更多 Token" },
];

const READING_THINKING_LABEL: Record<string, string> = Object.fromEntries(
  READING_THINKING_OPTIONS.map((option) => [option.value, option.label]),
);

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

interface EpubManagerHandle {
  rendered?: boolean;
  updateLayout?: () => void;
  bounds?: () => { width: number; height: number };
  stage?: { resizeFunc?: EventListener | null };
  views?: {
    forEach: (callback: (view: {
      size?: (width?: number | null, height?: number | null) => void;
    }) => void) => void;
  };
  _bounds?: { width: number; height: number };
}

function epubManager(rendition: Rendition): EpubManagerHandle | undefined {
  return (rendition as unknown as { manager?: EpubManagerHandle }).manager;
}

// rendition.resize() clears every view and re-displays the current location,
// which in scrolled-doc flow lands back on the first page because the restore
// scroll runs before the reflowed content heights settle. updateLayout()
// re-measures the stage and re-formats the already rendered views in place,
// keeping the scroll position. Existing views retain their old locked iframe
// width unless it is passed to size(), so update both the view frame and its
// inner document.
function softResizeRendition(rendition: Rendition, width: number): void {
  const manager = epubManager(rendition);
  if (!manager?.rendered || typeof manager.updateLayout !== "function") return;
  manager.updateLayout();
  manager.views?.forEach((view) => view.size?.(width, null));
  if (typeof manager.bounds === "function") manager._bounds = manager.bounds();
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
  discussionOpen: boolean;
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
  discussionOpen,
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
  const confirmDelete = useConfirmDialog();
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
  // The app-wide font scale applies CSS zoom to <html>. WebKitGTK sizes the epub
  // iframe's inner viewport by the frame's post-zoom size, so leaving the zoom in
  // place makes the book content overflow its frame and hide behind the sidebar.
  // The host div cancels the root zoom out for the whole epub region.
  const [rootZoom, setRootZoom] = useState(() => getRootZoomScale());

  useEffect(() => { highlightModeRef.current = highlightMode; }, [highlightMode]);
  useEffect(() => subscribeUIPreferenceChanges((change) => {
    if (change.fontScale !== undefined) setRootZoom(getRootZoomScale());
  }), []);
  useEffect(() => { createHighlightRef.current = onCreateHighlight; }, [onCreateHighlight]);
  useEffect(() => { openSelectionMenuRef.current = onOpenSelectionMenu; }, [onOpenSelectionMenu]);
  useEffect(() => {
    colorSchemeRef.current = colorScheme;
    preferencesRef.current = preferences;
    renditionRef.current?.themes.default(epubTheme(colorScheme, preferences));
  }, [colorScheme, preferences]);

  // The discussion sidebar animates its width over 300ms; WebKit's ResizeObserver
  // can stop firing before the transition settles, leaving the rendition sized
  // for an intermediate width. Resize again once the animation is done. Never
  // resize while the book is still loading — epubjs stalls if its display
  // pipeline is interrupted.
  useEffect(() => {
    if (loading) return;
    const resizeToHost = () => {
      const host = hostRef.current;
      const rendition = renditionRef.current;
      if (!host || !rendition) return;
      if (host.clientWidth <= 0 || host.clientHeight <= 0) return;
      softResizeRendition(rendition, host.clientWidth);
    };
    const settleTimer = window.setTimeout(resizeToHost, 350);
    return () => window.clearTimeout(settleTimer);
  }, [discussionOpen, loading, rootZoom]);

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
          // epubjs also registers a window resize listener that re-renders every
          // view (losing the scroll position); sizing is handled by the
          // ResizeObserver below through softResizeRendition instead.
          const stageHandle = epubManager(rendered)?.stage;
          if (stageHandle?.resizeFunc) {
            window.removeEventListener("resize", stageHandle.resizeFunc);
          }
          resizeObserver = new ResizeObserver(() => {
            if (resizeTimer !== null) window.clearTimeout(resizeTimer);
            resizeTimer = window.setTimeout(() => {
              resizeTimer = null;
              if (disposed || !rendered) return;
              const width = host.clientWidth;
              const height = host.clientHeight;
              if (width > 0 && height > 0) softResizeRendition(rendered, width);
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
    <section className="relative flex min-w-0 flex-1 flex-col overflow-hidden bg-canvas">
      <div className="relative flex h-12 shrink-0 items-center gap-2 border-b border-hairline bg-canvas-header px-3">
        <button onClick={onBackToShelf} className="rounded p-1 text-ink-muted hover:bg-canvas-surface-strong" title="返回书架"><ArrowLeft className="h-4 w-4" /></button>
        <BookOpen className="h-4 w-4 text-clay" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-semibold text-ink">{book.title}</span>
          {currentChapter && <span className="block truncate text-[9px] text-ink-muted-soft">{currentChapter}</span>}
        </span>
        <span className="w-9 shrink-0 text-right font-mono text-[9px] tabular-nums text-ink-muted-soft" title="阅读进度">
          {Math.round(readingProgress * 100)}%
        </span>
        {toc.length > 0 && (
          <select
            className="max-w-40 rounded-md border border-hairline bg-canvas-input px-2 py-1 text-xs text-ink-body outline-none xl:max-w-48"
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
          className={`agnes-reading-highlight-toggle flex h-8 shrink-0 items-center gap-1.5 rounded-md border px-2 text-[10px] font-medium transition-colors ${highlightMode ? "border-theme-warning bg-canvas-surface-strong text-theme-warning" : "border-hairline text-ink-muted hover:bg-canvas-surface-strong"}`}
          title={highlightMode ? "退出划线模式" : "进入划线模式"}
          aria-pressed={highlightMode}
        >
          <HighlighterCircle className="h-4 w-4" />
          <span className="hidden xl:inline">{highlightMode ? "划线中" : "划线"}</span>
        </button>
        <div className={`hidden h-8 shrink-0 items-center gap-1 rounded-md border border-hairline px-1 sm:flex ${highlightMode ? "opacity-100" : "opacity-45"}`} aria-label="划线颜色">
          {HIGHLIGHT_COLORS.map((color) => (
            <button
              key={color}
              type="button"
              disabled={!highlightMode}
              onClick={() => onHighlightColorChange(color)}
              className={`grid h-5 w-5 place-items-center rounded-full border transition-transform hover:scale-110 disabled:cursor-default ${highlightColor === color ? "border-hairline-strong" : "border-transparent"}`}
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
          className={`relative rounded p-1.5 ${isHighlightsOpen ? "bg-canvas-surface-strong text-ink" : "text-ink-muted hover:bg-canvas-surface-strong"}`}
          title="高亮与批注"
          aria-pressed={isHighlightsOpen}
        >
          <Notepad className="h-4 w-4" />
          {highlights.length > 0 && (
            <span className="absolute -right-1 -top-1 min-w-3.5 rounded-full bg-ink px-0.5 text-center font-mono text-[8px] leading-[14px] text-canvas">
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
          className={`rounded p-1.5 ${isTypographyOpen ? "bg-canvas-surface-strong text-ink" : "text-ink-muted hover:bg-canvas-surface-strong"}`}
          title="阅读排版"
          aria-pressed={isTypographyOpen}
        >
          <Faders className="h-4 w-4" />
        </button>
        <button onClick={() => renditionRef.current?.prev().catch(console.error)} className="rounded p-1 text-ink-muted hover:bg-canvas-surface-strong" title="上一页">
          <CaretLeft className="h-4 w-4" />
        </button>
        <button onClick={() => renditionRef.current?.next().catch(console.error)} className="rounded p-1 text-ink-muted hover:bg-canvas-surface-strong" title="下一页">
          <CaretRight className="h-4 w-4" />
        </button>
        <span className="absolute inset-x-0 bottom-0 h-px bg-canvas-surface-strong">
          <span className="block h-full bg-clay transition-[width] duration-300" style={{ width: `${readingProgress * 100}%` }} />
        </span>
      </div>

      {isHighlightsOpen && (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setIsHighlightsOpen(false)} />
          <div className="absolute right-3 top-14 z-30 flex max-h-[min(70vh,560px)] w-[min(340px,calc(100%-24px))] flex-col overflow-hidden rounded-md border border-hairline bg-canvas-surface-card shadow-xl">
            <header className="flex h-10 shrink-0 items-center gap-2 border-b border-hairline px-3">
              <Notepad className="h-3.5 w-3.5 text-ink-muted" />
              <span className="flex-1 text-xs font-semibold text-ink-body">高亮与批注</span>
              <span className="font-mono text-[9px] text-ink-muted-soft">{highlights.length}</span>
              <button type="button" onClick={() => setIsHighlightsOpen(false)} className="rounded p-1 text-ink-muted-soft hover:bg-canvas-surface-strong" title="关闭"><X className="h-3.5 w-3.5" /></button>
            </header>
            <div className="min-h-0 overflow-y-auto">
              {highlights.length === 0 ? (
                <div className="px-4 py-10 text-center text-xs text-ink-muted-soft">暂无高亮</div>
              ) : highlights.map((highlight) => {
                const editing = editingHighlightId === highlight.id;
                return (
                  <article key={highlight.id} className="border-b border-hairline px-3 py-3 last:border-b-0">
                    <div className="flex items-start gap-2">
                      <button
                        type="button"
                        onClick={() => navigate(highlight.cfi_range)}
                        className="min-w-0 flex-1 text-left"
                        title="定位到原文"
                      >
                        <span className="line-clamp-3 text-xs leading-5 text-ink-body">{highlight.quote}</span>
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setEditingHighlightId(editing ? null : highlight.id);
                          setNoteDraft(highlight.note ?? "");
                          setColorDraft(highlight.color);
                        }}
                        className="rounded p-1 text-ink-muted-soft hover:bg-canvas-surface-strong hover:text-ink-body"
                        title="编辑批注"
                      >
                        <PencilSimple className="h-3.5 w-3.5" />
                      </button>
                      <button
                        type="button"
                        onClick={async () => {
                          if (!await confirmDelete({
                            title: "删除这条高亮？",
                            description: "关联的批注也会一并删除，且无法恢复。",
                            confirmLabel: "删除高亮",
                          })) return;
                          await onDeleteHighlight(highlight);
                        }}
                        className="rounded p-1 text-ink-muted-soft hover:bg-rose-50 hover:text-rose-600"
                        title="删除高亮"
                      >
                        <Trash className="h-3.5 w-3.5" />
                      </button>
                    </div>
                    {!editing && highlight.note && (
                      <p className="mt-2 whitespace-pre-wrap border-l border-hairline pl-2 text-[10px] leading-4 text-ink-muted">{highlight.note}</p>
                    )}
                    {editing && (
                      <div className="mt-2 space-y-2">
                        <textarea
                          value={noteDraft}
                          onChange={(event) => setNoteDraft(event.target.value)}
                          maxLength={20_000}
                          rows={3}
                          placeholder="添加批注..."
                          className="w-full resize-y rounded-md border border-hairline bg-canvas-surface-soft px-2 py-1.5 text-xs leading-5 text-ink-body outline-none focus:border-hairline-strong"
                        />
                        <div className="flex items-center gap-1.5">
                          {HIGHLIGHT_COLORS.map((color) => (
                            <button
                              key={color}
                              type="button"
                              onClick={() => setColorDraft(color)}
                              className={`grid h-6 w-6 place-items-center rounded-full border ${colorDraft === color ? "border-hairline-strong" : "border-transparent"}`}
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
                            className="ml-auto grid h-7 w-7 place-items-center rounded-md bg-ink text-canvas transition-opacity hover:opacity-80"
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
          <div className="absolute right-3 top-14 z-30 w-72 rounded-md border border-hairline bg-canvas-surface-card p-4 shadow-xl">
            <div className="mb-4 flex items-center gap-2">
              <TextT className="h-4 w-4 text-ink-muted" />
              <span className="flex-1 text-xs font-semibold text-ink-body">阅读排版</span>
              <button
                type="button"
                onClick={() => onPreferencesChange(DEFAULT_READING_PREFERENCES)}
                className="rounded p-1 text-ink-muted-soft hover:bg-canvas-surface-strong hover:text-ink-body"
                title="恢复默认排版"
              >
                <ArrowCounterClockwise className="h-3.5 w-3.5" />
              </button>
            </div>
            <label className="block text-[10px] text-ink-muted">
              <span className="mb-1.5 flex justify-between"><span>字号</span><span className="font-mono">{preferences.fontSize}px</span></span>
              <input type="range" min={14} max={26} step={1} value={preferences.fontSize} onChange={(event) => onPreferencesChange(normalizeReadingPreferences({ ...preferences, fontSize: Number(event.target.value) }))} className="w-full accent-clay" />
            </label>
            <label className="mt-4 block text-[10px] text-ink-muted">
              <span className="mb-1.5 flex justify-between"><span>行距</span><span className="font-mono">{preferences.lineHeight.toFixed(1)}</span></span>
              <input type="range" min={1.4} max={2.4} step={0.1} value={preferences.lineHeight} onChange={(event) => onPreferencesChange(normalizeReadingPreferences({ ...preferences, lineHeight: Number(event.target.value) }))} className="w-full accent-clay" />
            </label>
            <label className="mt-4 block text-[10px] text-ink-muted">
              <span className="mb-1.5 flex justify-between"><span>正文宽度</span><span className="font-mono">{preferences.contentWidth}px</span></span>
              <input type="range" min={560} max={1_000} step={20} value={preferences.contentWidth} onChange={(event) => onPreferencesChange(normalizeReadingPreferences({ ...preferences, contentWidth: Number(event.target.value) }))} className="w-full accent-clay" />
            </label>
          </div>
        </>
      )}

      {loading && <div className="absolute inset-0 z-10 grid place-items-center bg-canvas/80"><CircleNotch className="h-5 w-5 animate-spin text-clay" /></div>}
      {error && <div className="m-5 rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs leading-relaxed text-rose-700">无法打开这本 EPUB：{error}</div>}
      <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden" style={{ zoom: rootZoom > 0 ? 1 / rootZoom : 1 }} />
    </section>
  );
};

const TRANSLATION_LANGUAGE_SETTING = "ui:translation_target_language";
const READING_PREFERENCES_SETTING = "ui:reading_preferences";

export const ReadingWorkspace: React.FC = () => {
  const agents = useAgentStore((state) => state.agents);
  const activeAgentId = useAgentStore((state) => state.activeAgentId);
  const activeSessionId = useAgentStore((state) => state.activeSessionId);
  const messages = useAgentStore((state) => state.messages);
  const isStreaming = useAgentStore((state) => state.isStreaming);
  const sessions = useAgentStore((state) => state.sessions);
  const providers = useAgentStore((state) => state.providers);
  const modelRoles = useAgentStore((state) => state.modelRoles);
  const loadSessions = useAgentStore((state) => state.loadSessions);
  const loadProviders = useAgentStore((state) => state.loadProviders);
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
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const historyButtonRef = useRef<HTMLButtonElement>(null);
  const settingsButtonRef = useRef<HTMLButtonElement>(null);
  const preferenceSaveTimerRef = useRef<number | null>(null);
  const pendingReadingPreferencesRef = useRef<ReadingPreferences | null>(null);
  const prefersReducedMotion = useReducedMotion();

  const selectedBook = useMemo(
    () => books.find((book) => book.id === selectedBookId) ?? null,
    [books, selectedBookId],
  );
  const activeAgent = agents.find((agent) => agent.id === activeAgentId);
  const readingSession = sessions.find((session) => session.id === readingSessionId);
  const readingMaxTokens = readingSession?.max_tokens ?? DEFAULT_MAX_OUTPUT_TOKENS;
  const effectiveReadingModel = readingSession?.model || activeAgent?.model || modelRoles.main_model || "";
  const readingModelRef = effectiveReadingModel;
  const readingModelSeparator = readingModelRef.indexOf("/");
  const readingProviderId = readingModelSeparator >= 0 ? readingModelRef.slice(0, readingModelSeparator) : "";
  const readingModelId = readingModelSeparator >= 0 ? readingModelRef.slice(readingModelSeparator + 1) : readingModelRef;
  const readingModelDescriptor = providers
    .find((provider) => provider.id === readingProviderId)
    ?.models.find((model) => model.id === readingModelId);
  const readingThinkingMode = readingSession?.thinking_mode || activeAgent?.thinking_mode || "off";
  const readingContextLimit = readingSession?.context_limit ?? readingModelDescriptor?.context_window ?? DEFAULT_CONTEXT_LIMIT;
  const readingCompressThreshold = readingSession?.compress_threshold ?? 0.85;
  const latestReadingAssistant = [...messages]
    .reverse()
    .find((message) => message.role === "assistant" && message.status === "complete");
  const readingContextTokens = latestReadingAssistant?.context_tokens ?? 0;
  const readingContextPercent = Math.min(100, (readingContextTokens / Math.max(1, readingContextLimit)) * 100);
  const readingSummaryTrigger = Math.floor(readingContextLimit * readingCompressThreshold);
  /** Only the newest assistant reply shows its token tally, so history stays quiet. */
  const lastAssistantIndex = useMemo(
    () => messages.reduce((last, message, index) => (message.role === "assistant" ? index : last), -1),
    [messages],
  );

  const applyReadingLlm = (
    model = effectiveReadingModel,
    thinkingMode = readingThinkingMode,
    maxTokens = readingMaxTokens,
  ) => {
    if (!readingSessionId || !model) return;
    void setSessionLlm(readingSessionId, model, thinkingMode, 0, maxTokens)
      .catch((reason) => setError(String(reason)));
  };

  const loadBooks = async () => {
    const next = await invoke<ReadingBook[]>("list_reading_books");
    setBooks(next);
    setSelectedBookId((current) => current && next.some((book) => book.id === current) ? current : null);
  };

  useEffect(() => {
    void loadProviders().catch((reason) => setError(String(reason)));
    void loadBooks().catch((reason) => setError(String(reason)));
    void invoke<string | null>("get_setting", { key: TRANSLATION_LANGUAGE_SETTING })
      .then((value) => {
        if (value === "中文" || value === "English") setTranslationLanguage(value);
      })
      .catch(console.error);
    void invoke<string | null>("get_setting", { key: READING_PREFERENCES_SETTING })
      .then((value) => setReadingPreferences(parseReadingPreferences(value)))
      .catch(console.error);
  }, [loadProviders]);

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

  // Grow the composer with its content up to a ceiling, then let it scroll.
  useEffect(() => {
    const node = composerRef.current;
    if (!node) return;
    node.style.height = "auto";
    node.style.height = `${Math.min(node.scrollHeight, 160)}px`;
  }, [question]);

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
    <main className="agnes-feature-workspace agnes-reading-workspace flex min-w-0 flex-1 flex-col bg-canvas lg:flex-row">
      {selectedBook ? (
        <div className="relative flex min-h-64 min-w-0 flex-1 flex-col lg:min-h-0 lg:flex-row" onClick={() => setSelectionMenu(null)}>
          <EpubPane
            book={selectedBook}
            highlights={highlights}
            highlightMode={highlightMode}
            highlightColor={highlightColor}
            colorScheme={colorScheme}
            preferences={readingPreferences}
            discussionOpen={isDiscussionOpen}
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
            className={`flex w-full shrink-0 flex-col overflow-hidden border-t border-hairline bg-canvas transition-[width,min-width,height,min-height] duration-300 ease-out motion-reduce:transition-none lg:h-auto lg:min-h-0 lg:border-l lg:border-t-0 ${
              isDiscussionOpen
                ? "h-[42%] min-h-64 lg:w-[min(32vw,400px)] lg:min-w-80"
                : "h-10 min-h-10 lg:w-9 lg:min-w-9"
            }`}
          >
            {isDiscussionOpen ? (
              <>
              <header className="relative border-b border-hairline">
                <div className="flex h-12 items-center gap-2 px-4">
                  <ChatCircleDots className="h-4 w-4 text-clay" />
                  <span className="min-w-0 flex-1 text-sm font-semibold text-ink">阅读讨论</span>
                  <div
                    className="hidden items-center gap-1.5 sm:flex"
                    title={`上下文 ${readingContextTokens.toLocaleString()} / ${readingContextLimit.toLocaleString()} Token；总结阈值 ${readingSummaryTrigger.toLocaleString()} Token`}
                  >
                    <div className="h-1.5 w-14 overflow-hidden rounded-full bg-canvas-surface-strong">
                      <motion.div
                        className={`h-full rounded-full ${readingContextPercent >= readingCompressThreshold * 100 ? "bg-theme-warning" : "bg-clay"}`}
                        initial={false}
                        animate={{ width: `${readingContextPercent}%` }}
                        transition={{ duration: 0.5, ease: EASE_OUT }}
                      />
                    </div>
                    <span className="font-mono text-[10px] tabular-nums text-ink-muted-soft">{readingContextPercent.toFixed(0)}%</span>
                  </div>
                  <button
                    ref={historyButtonRef}
                    onClick={() => setIsConversationHistoryOpen((open) => !open)}
                    disabled={!conversationReady || loading}
                    className={`agnes-reading-panel-action grid h-7 w-7 place-items-center rounded-md transition-transform active:scale-90 disabled:opacity-40 ${isConversationHistoryOpen ? "text-ink-body" : "text-ink-muted-soft hover:text-ink-body"}`}
                    title="讨论历史"
                    aria-pressed={isConversationHistoryOpen}
                  >
                    <ClockCounterClockwise className="h-4 w-4" />
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
                    className="agnes-reading-panel-action grid h-7 w-7 place-items-center rounded-md text-ink-muted-soft transition-transform hover:text-ink-body active:scale-90 disabled:opacity-40"
                    title="新建讨论"
                  >
                    <Plus className="h-4 w-4" />
                  </button>
                  <button ref={settingsButtonRef} onClick={() => setIsDiscussionOptionsOpen((open) => !open)} className={`agnes-reading-panel-action grid h-7 w-7 place-items-center rounded-md transition-transform active:scale-90 ${isDiscussionOptionsOpen ? "text-ink-body" : "text-ink-muted-soft hover:text-ink-body"}`} title="讨论设置" aria-pressed={isDiscussionOptionsOpen}><SlidersHorizontal className="h-4 w-4" /></button>
                  <button onClick={() => setIsDiscussionOpen(false)} className="agnes-reading-panel-action grid h-7 w-7 place-items-center rounded-md text-ink-muted-soft transition-transform hover:text-ink-body active:scale-90" title="收起讨论" aria-expanded="true"><SidebarSimple className="h-4 w-4" /></button>
                </div>
                <AnimatePresence>
                {isConversationHistoryOpen && (
                  <ReadingPopover
                    key="reading-history"
                    anchorRef={historyButtonRef}
                    onClose={() => setIsConversationHistoryOpen(false)}
                    className="w-64 rounded-md border border-hairline bg-canvas-surface-card shadow-xl"
                  >
                      <div className="border-b border-hairline px-3 py-2 text-[11px] font-semibold text-ink-muted">讨论历史</div>
                      <div className="max-h-64 overflow-y-auto py-1">
                        {readingConversations.map((conversation, index) => (
                          <button
                            key={conversation.session_id}
                            onClick={() => void switchReadingConversation(conversation.session_id)}
                            disabled={isStreaming || loading}
                            className={`flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-canvas-surface-soft disabled:opacity-50 ${conversation.session_id === readingSessionId ? "bg-clay-soft" : ""}`}
                          >
                            <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${conversation.session_id === readingSessionId ? "bg-clay" : "bg-canvas-surface-strong"}`} />
                            <span className="min-w-0 flex-1">
                              <span className="block truncate text-xs font-medium text-ink-body">
                                {conversation.session_id === readingSessionId ? "当前讨论" : `历史讨论 ${readingConversations.length - index}`}
                              </span>
                              <span className="mt-0.5 block text-[10px] text-ink-muted-soft">{conversationDate(conversation.created_at)}</span>
                            </span>
                          </button>
                        ))}
                      </div>
                  </ReadingPopover>
                )}
                </AnimatePresence>
                <AnimatePresence>
                {isDiscussionOptionsOpen && (
                  <ReadingPopover
                    key="reading-options"
                    anchorRef={settingsButtonRef}
                    onClose={() => setIsDiscussionOptionsOpen(false)}
                    className="claude-popover agnes-reading-model-popup w-[min(348px,calc(100vw-24px))] rounded-md border border-hairline bg-canvas-surface-card shadow-2xl"
                  >
                      <div className="flex shrink-0 items-center gap-2 border-b border-hairline px-3 py-2.5">
                        <SlidersHorizontal className="h-3.5 w-3.5 text-ink-muted" />
                        <span className="min-w-0 flex-1 text-xs font-semibold text-ink-body">讨论设置</span>
                        <button type="button" onClick={() => setIsDiscussionOptionsOpen(false)} className="rounded p-1 text-ink-muted-soft hover:bg-canvas-surface-strong hover:text-ink-body" title="关闭设置">
                          <X className="h-3.5 w-3.5" />
                        </button>
                      </div>
                      <div className="min-h-0 overflow-y-auto p-3">
                        <div className="mb-2 flex items-center gap-1.5 text-[10px] font-semibold text-ink-muted">
                          <Cpu className="h-3.5 w-3.5" />
                          <span>模型</span>
                          <span className="ml-auto font-mono font-normal text-ink-muted-soft">{readingModelDescriptor?.context_window ? `${Math.round(readingModelDescriptor.context_window / 1024)}K 上下文` : "上下文自动"}</span>
                        </div>
                        <div className="mb-3 rounded-md border border-hairline bg-canvas-surface-soft p-1.5">
                          {providers.length === 0 ? (
                            <div className="px-2 py-3 text-center text-[10px] text-ink-muted-soft">暂无服务商，请先在模型设置中添加</div>
                          ) : (
                            providers.map((provider) => (
                              <div key={provider.id}>
                                <div className="flex items-center gap-1.5 px-1.5 pb-1 pt-1 text-[10px] font-semibold uppercase tracking-wide text-ink-muted-soft">
                                  <HardDrives className="h-3 w-3" />
                                  <span className="min-w-0 truncate">{provider.name}</span>
                                  {provider.is_default && <span className="text-clay">默认</span>}
                                </div>
                                {provider.models.length === 0 ? (
                                  <div className="px-1.5 py-1 text-[10px] text-ink-muted-soft">无可用模型</div>
                                ) : (
                                  provider.models.map((model) => {
                                    const modelRef = `${provider.id}/${model.id}`;
                                    const active = effectiveReadingModel === modelRef;
                                    return (
                                      <button
                                        key={modelRef}
                                        type="button"
                                        disabled={!readingSessionId || isStreaming || loading}
                                        onClick={() => applyReadingLlm(modelRef)}
                                        className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left font-mono text-[10px] transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${active ? "agnes-reading-model-selected bg-clay-soft text-clay-dark" : "text-ink-body hover:bg-canvas-surface"}`}
                                      >
                                        <span className="min-w-0 flex-1 truncate">{model.id}</span>
                                        {model.capabilities.input_modalities.includes("image") && <span className="font-sans text-[10px] text-ink-muted-soft">视觉</span>}
                                        {active && <Check className="h-3 w-3 shrink-0" />}
                                      </button>
                                    );
                                  })
                                )}
                              </div>
                            ))
                          )}
                        </div>

                        <div className="border-t border-hairline pt-3">
                          <div className="mb-2 flex items-center gap-1.5 text-[10px] font-semibold text-ink-muted">
                            <Cpu className="h-3.5 w-3.5" />
                            <span>思考模式</span>
                            <span className="ml-auto text-[10px] font-normal text-ink-muted-soft">{READING_THINKING_LABEL[readingThinkingMode] ?? readingThinkingMode}</span>
                          </div>
                          <div className="grid grid-cols-5 gap-1">
                            {READING_THINKING_OPTIONS.map((option) => (
                              <button
                                key={option.value}
                                type="button"
                                disabled={!readingSessionId || isStreaming || loading}
                                title={option.desc}
                                onClick={() => applyReadingLlm(effectiveReadingModel, option.value)}
                                className={`rounded border px-1 py-1.5 text-[10px] font-semibold transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${readingThinkingMode === option.value ? "border-clay-soft bg-clay-soft text-clay-dark" : "border-hairline bg-canvas-surface-soft text-ink-muted hover:bg-canvas-surface-strong"}`}
                              >
                                {option.label}
                              </button>
                            ))}
                          </div>
                        </div>

                        <div className="mt-3 space-y-2 border-t border-hairline pt-3">
                          <label className="flex items-center gap-2 text-[10px] text-ink-muted">
                            <span className="min-w-0 flex-1">最大输出 Token</span>
                            <input
                              key={`${readingSessionId}-${readingMaxTokens}`}
                              type="number"
                              min={128}
                              max={1048576}
                              step={256}
                              defaultValue={readingMaxTokens}
                              onClick={(event) => event.stopPropagation()}
                              onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }}
                              onBlur={(event) => {
                                const value = Math.min(1048576, Math.max(128, Number(event.currentTarget.value) || DEFAULT_MAX_OUTPUT_TOKENS));
                                event.currentTarget.value = String(value);
                                if (value !== readingMaxTokens) applyReadingLlm(effectiveReadingModel, readingThinkingMode, value);
                              }}
                              className="h-7 w-24 rounded border border-hairline bg-canvas-surface-soft px-2 text-right font-mono text-[10px] text-ink-body outline-none focus:border-clay"
                            />
                          </label>
                          <label className="flex items-center gap-2 text-[10px] text-ink-muted">
                            <span className="min-w-0 flex-1">自动总结阈值</span>
                            <input
                              key={`${readingSessionId}-${readingCompressThreshold}`}
                              type="number"
                              min={0}
                              max={1}
                              step={0.05}
                              defaultValue={readingCompressThreshold}
                              onClick={(event) => event.stopPropagation()}
                              onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }}
                              onBlur={(event) => {
                                const value = Math.min(1, Math.max(0, Number(event.currentTarget.value) || 0));
                                event.currentTarget.value = String(value);
                                if (value !== readingCompressThreshold && readingSessionId) {
                                  void setSessionCompressThreshold(readingSessionId, value).catch((reason) => setError(String(reason)));
                                }
                              }}
                              className="h-7 w-20 rounded border border-hairline bg-canvas-surface-soft px-2 text-right font-mono text-[10px] text-ink-body outline-none focus:border-clay"
                              aria-label="自动总结阈值"
                            />
                            <span className="shrink-0 text-[10px] text-ink-muted-soft">触发 {readingSummaryTrigger.toLocaleString()}</span>
                          </label>
                        </div>

                        <div className="mt-3 space-y-2 border-t border-hairline pt-3">
                          <div className="mb-1 text-[10px] font-semibold text-ink-muted">书籍上下文</div>
                          <label className="flex cursor-pointer items-center gap-2 text-[10px] text-ink-muted">
                            <input type="checkbox" checked={selectedBook.model_knows_content} onChange={(event) => void updateBook("update_reading_book_mode", { modelKnowsContent: event.target.checked })} className="accent-clay" />
                            AI 已知这本书
                          </label>
                          {!selectedBook.model_knows_content && selectedBook.content_context_decided && (
                            <label className="flex cursor-pointer items-center gap-2 text-[10px] text-ink-muted">
                              <input type="checkbox" checked={selectedBook.content_context_allowed} onChange={(event) => void updateBook("set_reading_book_content_context_allowed", { allowed: event.target.checked })} className="accent-clay" />
                              允许检索本书片段
                            </label>
                          )}
                        </div>
                      </div>
                  </ReadingPopover>
                )}
                </AnimatePresence>
              </header>
              <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-5">
                {conversationReady && messages.map((message, messageIndex) => {
                  const isUser = message.role === "user";
                  const awaitingFirstToken = !isUser
                    && message.status !== "complete"
                    && !message.parts.some((part) => part.kind === "text" || part.kind === "thought" || part.kind === "model_fallback");
                  return (
                    <motion.div
                      key={message._renderKey ?? message.id}
                      initial={prefersReducedMotion ? false : { opacity: 0, y: 8 }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={{ duration: 0.24, ease: EASE_OUT }}
                      className={`flex gap-2.5 ${isUser ? "justify-end" : ""}`}
                    >
                      {!isUser && (
                        <span
                          className="mt-0.5 grid h-5 w-5 shrink-0 place-items-center rounded-full bg-clay-soft text-clay"
                          title={`输入 ${message.input_tokens ?? 0} · 缓存 ${message.cached_tokens ?? 0} · 输出 ${message.output_tokens ?? 0} Token`}
                        >
                          <Sparkle className="h-3 w-3" />
                        </span>
                      )}
                      {awaitingFirstToken ? (
                        <span className="flex items-center gap-1 py-1.5" role="status" aria-label="正在生成回复">
                          {[0, 1, 2].map((dot) => (
                            <motion.span
                              key={dot}
                              className="h-1.5 w-1.5 rounded-full bg-canvas-surface-strong"
                              animate={prefersReducedMotion ? undefined : { opacity: [0.25, 1, 0.25] }}
                              transition={{ duration: 1.1, repeat: Infinity, delay: dot * 0.16, ease: "easeInOut" }}
                            />
                          ))}
                        </span>
                      ) : (
                        <div className={isUser
                          ? "max-w-[85%] rounded-2xl rounded-br-md bg-clay-soft px-3.5 py-2.5 text-sm text-ink"
                          : "min-w-0 flex-1 text-sm text-ink-body"}
                        >
                          <div className="space-y-2.5">
                            {message.parts.map((part) => {
                              if (part.kind === "model_fallback") {
                                return (
                                  <div key={part._renderKey ?? part.id} className="flex items-center gap-2 rounded-md border border-theme-warning bg-canvas-surface-strong px-2.5 py-2 text-[10px] text-theme-warning">
                                    <ArrowsClockwise className="h-3.5 w-3.5 shrink-0" />
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
                                        <CaretDown className="agnes-collapse-chevron h-3 w-3" />
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
                          {!isUser && messageIndex === lastAssistantIndex && (
                            <div className="mt-2 font-mono text-[10px] tabular-nums text-ink-muted-soft">
                              输入 {message.input_tokens ?? 0} · 缓存 {message.cached_tokens ?? 0} · 输出 {message.output_tokens ?? 0}
                            </div>
                          )}
                        </div>
                      )}
                    </motion.div>
                  );
                })}
                {!conversationReady && (
                  <p className="py-12 text-center text-xs text-ink-muted-soft">暂无讨论</p>
                )}
                {conversationReady && !messages.length && (
                  <div className="flex flex-col items-center px-1 py-10 text-center">
                    <span className="mb-3 grid h-9 w-9 place-items-center rounded-full bg-canvas-surface-strong text-ink-muted-soft">
                      <ChatCircleDots className="h-4 w-4" />
                    </span>
                    <p className="text-xs font-medium text-ink-muted">还没有讨论</p>
                    <p className="mt-1 text-[11px] leading-relaxed text-ink-muted-soft">
                      选中正文可以直接引用提问，或从下面挑一个开始
                    </p>
                    <div className="mt-4 flex w-full flex-col gap-1.5">
                      {READING_STARTER_PROMPTS.map((prompt) => (
                        <button
                          key={prompt}
                          type="button"
                          onClick={() => {
                            setQuestion(prompt);
                            composerRef.current?.focus();
                          }}
                          className="rounded-lg border border-hairline px-3 py-2 text-left text-[11px] text-ink-body transition-colors hover:border-clay-soft hover:bg-clay-soft hover:text-ink"
                        >
                          {prompt}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
                <div ref={messageEndRef} />
              </div>
              <div className="border-t border-hairline p-3">
                <AnimatePresence initial={false}>
                  {quotedSelection && (
                    <motion.div
                      initial={{ opacity: 0, height: 0, marginBottom: 0 }}
                      animate={{ opacity: 1, height: "auto", marginBottom: 8 }}
                      exit={{ opacity: 0, height: 0, marginBottom: 0 }}
                      transition={{ duration: 0.18, ease: EASE_OUT }}
                      className="flex items-center gap-2 overflow-hidden rounded-lg border border-clay-soft bg-clay-soft px-2.5 py-2 text-[11px] text-ink-body"
                    >
                      <Quotes className="h-3.5 w-3.5 shrink-0 text-clay" />
                      <span className="min-w-0 flex-1 truncate">{quotedSelection.quote}</span>
                      <button onClick={() => setQuotedSelection(null)} className="rounded p-0.5 text-ink-muted-soft transition-colors hover:bg-clay-soft hover:text-ink-body" title="移除引用"><X className="h-3.5 w-3.5" /></button>
                    </motion.div>
                  )}
                </AnimatePresence>
                <div className="rounded-lg border border-hairline bg-canvas-input transition-colors focus-within:border-clay">
                  <textarea
                    ref={composerRef}
                    value={question}
                    onChange={(event) => setQuestion(event.target.value)}
                    onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); ask(); } }}
                    placeholder={isStreaming ? "正在生成回复…" : "问问这本书..."}
                    className="block max-h-40 min-h-[56px] w-full resize-none overflow-y-auto bg-transparent px-3 pt-2.5 text-sm leading-relaxed text-ink outline-none placeholder:text-ink-muted-soft"
                  />
                  <div className="flex items-center justify-between gap-2 px-2.5 pb-2 pt-1">
                    <span className="text-[10px] text-ink-muted-soft">Enter 发送，Shift+Enter 换行</span>
                    <button
                      type="button"
                      onClick={ask}
                      disabled={!question.trim() || isStreaming || !conversationReady}
                      className="flex h-7 items-center gap-1.5 rounded-md bg-clay px-2.5 text-[11px] font-semibold text-white transition-all hover:bg-clay-dark active:scale-[0.97] disabled:cursor-not-allowed disabled:bg-canvas-surface-strong disabled:active:scale-100"
                    >
                      {isStreaming ? <CircleNotch className="h-3.5 w-3.5 animate-spin" /> : <PaperPlaneRight className="h-3.5 w-3.5" />}
                      {isStreaming ? "生成中" : "发送"}
                    </button>
                  </div>
                </div>
              </div>
              </>
            ) : (
              <button
                onClick={() => setIsDiscussionOpen(true)}
                className="agnes-reading-panel-action flex h-full w-full items-center justify-center text-ink-muted-soft hover:text-ink-body lg:items-start lg:pt-4"
                title="展开讨论"
                aria-expanded="false"
              >
                <SidebarSimple className="h-4 w-4" />
              </button>
            )}
          </aside>
        </div>
      ) : (
        <section className="flex min-w-0 flex-1 flex-col bg-canvas">
          <header className="flex h-14 shrink-0 items-center justify-between border-b border-hairline bg-canvas-header px-5">
            <div className="flex items-center gap-2 text-sm font-semibold text-ink"><BookmarkSimple className="h-4 w-4 text-clay" />书架</div>
            <button onClick={() => void importBook()} disabled={!activeAgentId || loading} className="flex items-center gap-2 rounded-md bg-clay px-3 py-2 text-xs font-semibold text-white hover:bg-clay-dark disabled:opacity-50">
              {loading ? <CircleNotch className="h-3.5 w-3.5 animate-spin" /> : <FileArrowUp className="h-3.5 w-3.5" />} 导入 EPUB
            </button>
          </header>
          {books.length ? (
            <div className="grid auto-rows-min grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-3 overflow-y-auto p-5">
              {books.map((book) => (
                <article key={book.id} className="group min-h-28 rounded-md border border-hairline bg-canvas-surface-card p-4 text-left text-ink-body transition-colors hover:border-clay-soft hover:bg-clay-soft">
                  <button onClick={() => book.local_path && openBook(book.id)} disabled={!book.local_path} className="block w-full text-left disabled:cursor-not-allowed disabled:opacity-55">
                    <BookOpen className="mb-5 h-5 w-5 text-clay" />
                    <span className="block truncate text-sm font-semibold text-ink-body">{book.title}</span>
                    <span className="mt-1 block truncate text-[11px] text-ink-muted-soft">{book.author || "未知作者"}</span>
                    {!book.local_path && <span className="mt-2 block text-[10px] text-theme-warning">等待 EPUB 下载</span>}
                  </button>
                  <div className="mt-3 flex items-center justify-between gap-2 border-t border-hairline pt-2">
                    <span className="truncate text-[10px] text-ink-muted-soft">
                      {book.ready_replica_count > 0 ? `已发布 · ${book.ready_replica_count} 个副本` : "仅本机"}
                    </span>
                    <button
                      onClick={() => void publishBook(book.id)}
                      disabled={!book.local_path || loading}
                      title={book.ready_replica_count > 0 ? "重新确认 EPUB 云端副本" : "发布加密 EPUB"}
                      className="flex shrink-0 items-center gap-1 rounded px-1.5 py-1 text-[10px] font-medium text-clay hover:bg-clay-soft disabled:cursor-not-allowed disabled:text-ink-muted-soft"
                    >
                      {loading ? <CircleNotch className="h-3 w-3 animate-spin" /> : <CloudArrowUp className="h-3 w-3" />}
                      {book.ready_replica_count > 0 ? "已发布" : "发布"}
                    </button>
                  </div>
                </article>
              ))}
            </div>
          ) : (
            <div className="grid flex-1 place-items-center"><BookmarkSimple className="h-7 w-7 text-ink-muted-soft" /></div>
          )}
        </section>
      )}

      {selectionMenu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setSelectionMenu(null)} onContextMenu={(event) => { event.preventDefault(); setSelectionMenu(null); }} />
          <div className="fixed z-50 w-44 overflow-hidden rounded-md border border-hairline bg-canvas-surface-card py-1 text-xs text-ink-body shadow-2xl" style={{ left: selectionMenu.x, top: selectionMenu.y }}>
            <button onClick={() => void copySelection()} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-canvas-surface-strong"><Copy className="h-3.5 w-3.5 text-ink-muted" />复制</button>
            <button onClick={() => { setQuotedSelection(selectionMenu.selection); setSelectionMenu(null); setIsDiscussionOpen(true); }} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-canvas-surface-strong"><Quotes className="h-3.5 w-3.5 text-ink-muted" />引用</button>
            <button onClick={() => void translateSelection()} disabled={!conversationReady || isStreaming} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-canvas-surface-strong disabled:cursor-not-allowed disabled:opacity-40"><Translate className="h-3.5 w-3.5 text-ink-muted" />翻译</button>
            <div className="mt-1 flex items-center gap-1 border-t border-hairline px-3 py-2">
              <HighlighterCircle className="mr-auto h-3.5 w-3.5 text-ink-muted" />
              {HIGHLIGHT_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  onClick={() => {
                    const selection = selectionMenu.selection;
                    setSelectionMenu(null);
                    void createHighlight(selection, color);
                  }}
                  className="grid h-6 w-6 place-items-center rounded-full border border-transparent hover:border-hairline-strong"
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
        <div className="fixed inset-0 z-50 grid place-items-center bg-canvas-overlay p-5">
          <div className="w-full max-w-md rounded-lg bg-canvas-surface-card p-5 shadow-xl">
            <div className="flex items-center gap-2 text-sm font-semibold text-ink"><ShieldWarning className="h-4 w-4 text-theme-warning" />允许书内检索？</div>
            <p className="mt-3 text-sm leading-relaxed text-ink-body">这本书尚未标记为 AI 已知。允许后，讨论时会向当前模型服务发送划线附近文本及最多数段本书的相关片段，不会发送整本 EPUB。</p>
            <div className="mt-5 flex justify-end gap-2">
              <button onClick={() => { setShowConsent(false); void submitQuestion(false); }} className="rounded-md border border-hairline px-3 py-2 text-xs font-semibold text-ink-body hover:bg-canvas-surface-soft">仅讨论选段</button>
              <button onClick={() => void submitQuestion(true)} className="rounded-md bg-clay px-3 py-2 text-xs font-semibold text-white hover:bg-clay-dark">允许书内检索</button>
            </div>
          </div>
        </div>
      )}
      {error && <div className="fixed bottom-4 left-1/2 z-[60] max-w-xl -translate-x-1/2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700 shadow">{error}</div>}
    </main>
  );
};
