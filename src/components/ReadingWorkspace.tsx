import React, { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import ePub, { type Book, type Contents, type NavItem, type Rendition } from "epubjs";
import {
  ArrowLeft,
  BookMarked,
  BookOpen,
  Brain,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Copy,
  CloudUpload,
  FileUp,
  Highlighter,
  History,
  Languages,
  LoaderCircle,
  MessageCircleMore,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Quote,
  RefreshCw,
  Send,
  ShieldAlert,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { MarkdownMessage } from "./MarkdownMessage";
import { ThoughtDetails } from "./ThoughtDetails";
import { useAgentStore } from "../store/useAgentStore";
import {
  DEFAULT_MAX_OUTPUT_TOKENS,
  getCachedAutoFollowStreaming,
  getCachedAutoExpandThoughts,
  getCachedColorScheme,
  subscribeUIPreferenceChanges,
  type ColorScheme,
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

function epubTheme(colorScheme: ColorScheme): Record<string, Record<string, string>> {
  const dark = colorScheme === "dark";
  return {
    html: {
      background: dark ? "#1f1e1b" : "#fbfaf6",
      color: dark ? "#dedad1" : "#282723",
    },
    body: {
      background: dark ? "#1f1e1b" : "#fbfaf6",
      color: dark ? "#dedad1" : "#282723",
      "font-family": "Georgia, 'Noto Serif SC', serif",
      "line-height": "1.8",
    },
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

const EpubPane: React.FC<{
  book: ReadingBook;
  highlights: ReadingHighlight[];
  highlightMode: boolean;
  colorScheme: ColorScheme;
  onProgress: (cfi: string) => void;
  onBackToShelf: () => void;
  onToggleHighlightMode: () => void;
  onCreateHighlight: (selection: PendingSelection) => void;
  onOpenSelectionMenu: (selection: PendingSelection, x: number, y: number) => void;
}> = ({ book, highlights, highlightMode, colorScheme, onProgress, onBackToShelf, onToggleHighlightMode, onCreateHighlight, onOpenSelectionMenu }) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const bookRef = useRef<Book | null>(null);
  const renditionRef = useRef<Rendition | null>(null);
  const highlightModeRef = useRef(highlightMode);
  const createHighlightRef = useRef(onCreateHighlight);
  const openSelectionMenuRef = useRef(onOpenSelectionMenu);
  const colorSchemeRef = useRef(colorScheme);
  const [toc, setToc] = useState<NavItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => { highlightModeRef.current = highlightMode; }, [highlightMode]);
  useEffect(() => { createHighlightRef.current = onCreateHighlight; }, [onCreateHighlight]);
  useEffect(() => { openSelectionMenuRef.current = onOpenSelectionMenu; }, [onOpenSelectionMenu]);
  useEffect(() => {
    colorSchemeRef.current = colorScheme;
    renditionRef.current?.themes.default(epubTheme(colorScheme));
  }, [colorScheme]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void invoke("set_reading_context_menu_active", { active: true });

    const openNativeSelectionMenu = () => {
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
        openSelectionMenuRef.current(
          { cfiRange, quote, contextBefore: nearby.before, contextAfter: nearby.after },
          (frameRect?.left ?? 0) + rangeRect.right,
          (frameRect?.top ?? 0) + rangeRect.bottom,
        );
        return;
      }
    };

    void listen("reading://native-context-menu", () => openNativeSelectionMenu()).then((remove) => {
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

    const applyHighlights = () => {
      if (!rendered) return;
      for (const highlight of highlights) {
        try {
          rendered.annotations.highlight(
            highlight.cfi_range,
            { id: highlight.id },
            undefined,
            `reading-highlight-${highlight.color}`,
            HIGHLIGHT_STYLES[highlight.color] ?? HIGHLIGHT_STYLES.yellow,
          );
        } catch {
          // A highlight from another EPUB revision may no longer resolve.
        }
      }
    };

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
        rendered.themes.default(epubTheme(colorSchemeRef.current));
        rendered.on("relocated", (location: { start?: { cfi?: string } }) => {
          const cfi = location?.start?.cfi;
          if (cfi) onProgress(cfi);
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
          applyHighlights();
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
          applyHighlights();
          setLoading(false);
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
      host.replaceChildren();
    };
  }, [book.id, book.local_path]);

  useEffect(() => {
    if (!renditionRef.current) return;
    for (const highlight of highlights) {
      try {
        renditionRef.current.annotations.highlight(
          highlight.cfi_range,
          { id: highlight.id },
          undefined,
          `reading-highlight-${highlight.color}`,
          HIGHLIGHT_STYLES[highlight.color] ?? HIGHLIGHT_STYLES.yellow,
        );
      } catch {
        // Ignore anchors that no longer exist in the imported EPUB revision.
      }
    }
  }, [highlights]);

  const navigate = (href: string) => renditionRef.current?.display(href).catch(console.error);

  return (
    <section className="relative flex min-w-0 flex-1 flex-col overflow-hidden bg-[#fbfaf6]">
      <div className="flex h-11 shrink-0 items-center gap-2 border-b border-stone-200 bg-white/70 px-4">
        <button onClick={onBackToShelf} className="rounded p-1 text-stone-500 hover:bg-stone-100" title="返回书架"><ArrowLeft className="h-4 w-4" /></button>
        <BookOpen className="h-4 w-4 text-emerald-700" />
        <span className="min-w-0 flex-1 truncate text-sm font-semibold text-stone-800">{book.title}</span>
        {toc.length > 0 && (
          <select
            className="max-w-48 rounded-md border border-stone-200 bg-white px-2 py-1 text-xs text-stone-600 outline-none"
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
          className={`rounded p-1 ${highlightMode ? "bg-amber-100 text-amber-700" : "text-stone-500 hover:bg-stone-100"}`}
          title={highlightMode ? "退出划线模式" : "进入划线模式"}
          aria-pressed={highlightMode}
        >
          <Highlighter className="h-4 w-4" />
        </button>
        <button onClick={() => renditionRef.current?.prev().catch(console.error)} className="rounded p-1 text-stone-500 hover:bg-stone-100" title="上一页">
          <ChevronLeft className="h-4 w-4" />
        </button>
        <button onClick={() => renditionRef.current?.next().catch(console.error)} className="rounded p-1 text-stone-500 hover:bg-stone-100" title="下一页">
          <ChevronRight className="h-4 w-4" />
        </button>
      </div>
      {loading && <div className="absolute inset-0 z-10 grid place-items-center bg-[#fbfaf6]/80"><LoaderCircle className="h-5 w-5 animate-spin text-emerald-700" /></div>}
      {error && <div className="m-5 rounded-lg border border-rose-200 bg-rose-50 p-3 text-xs leading-relaxed text-rose-700">无法打开这本 EPUB：{error}</div>}
      <div ref={hostRef} className="min-h-0 flex-1 overflow-hidden" />
    </section>
  );
};

const TRANSLATION_LANGUAGE_SETTING = "ui:translation_target_language";

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
  const [translationLanguage, setTranslationLanguage] = useState("中文");
  const [colorScheme, setColorScheme] = useState<ColorScheme>(getCachedColorScheme);
  const [autoExpandThoughts, setAutoExpandThoughts] = useState(getCachedAutoExpandThoughts);
  const [autoFollowStreaming, setAutoFollowStreaming] = useState(getCachedAutoFollowStreaming);
  const messageEndRef = useRef<HTMLDivElement>(null);

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
  }, []);

  useEffect(() => subscribeUIPreferenceChanges((change) => {
    if (change.colorScheme !== undefined) setColorScheme(change.colorScheme);
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

  const createHighlight = async (selection: PendingSelection) => {
    if (!selectedBook) return;
    try {
      const highlight = await invoke<ReadingHighlight>("create_reading_highlight", {
        bookId: selectedBook.id,
        payload: { ...selection, color: "yellow" },
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
      x: Math.min(Math.max(x, 8), window.innerWidth - 196),
      y: Math.min(Math.max(y, 8), window.innerHeight - 152),
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
            colorScheme={colorScheme}
            onBackToShelf={returnToShelf}
            onToggleHighlightMode={() => setHighlightMode((enabled) => !enabled)}
            onProgress={(cfi) => {
              setBooks((current) => current.map((book) => book.id === selectedBook.id ? { ...book, progress_cfi: cfi } : book));
              void invoke("update_reading_book_progress", { bookId: selectedBook.id, cfi }).catch(console.error);
            }}
            onCreateHighlight={(selection) => void createHighlight(selection)}
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
                    className={`rounded p-1 disabled:opacity-40 ${isConversationHistoryOpen ? "bg-stone-100 text-stone-700" : "text-stone-400 hover:bg-stone-100 hover:text-stone-700"}`}
                    title="讨论历史"
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
                    className="rounded p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-40"
                    title="新建讨论"
                  >
                    <Plus className="h-4 w-4" />
                  </button>
                  <button onClick={() => setIsDiscussionOptionsOpen((open) => !open)} className={`rounded p-1 ${isDiscussionOptionsOpen ? "bg-stone-100 text-stone-700" : "text-stone-400 hover:bg-stone-100 hover:text-stone-700"}`} title="讨论设置"><SlidersHorizontal className="h-4 w-4" /></button>
                  <button onClick={() => setIsDiscussionOpen(false)} className="rounded p-1 text-stone-400 hover:bg-stone-100 hover:text-stone-700" title="收起讨论" aria-expanded="true"><PanelRightClose className="h-4 w-4" /></button>
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
                              <ThoughtDetails
                                key={part._renderKey ?? part.id}
                                defaultOpen={autoExpandThoughts}
                                className="group rounded-r-md border-l-2 border-emerald-600 bg-stone-50 px-2.5 py-2"
                              >
                                <summary className="flex cursor-pointer select-none items-center gap-2 text-[11px] font-semibold text-emerald-700">
                                  <Brain className="h-3.5 w-3.5" />
                                  <span>思考过程</span>
                                  {isLiveThought && (
                                    <span className="ml-1 flex items-center gap-1 text-[9px] font-normal text-stone-400">
                                      <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-emerald-600" />
                                      思考中
                                    </span>
                                  )}
                                  <ChevronDown className="ml-auto h-3 w-3 transition-transform group-open:rotate-180" />
                                </summary>
                                <p className="mt-2 whitespace-pre-wrap border-t border-stone-200/70 pt-2 font-mono text-[11px] leading-relaxed text-stone-500">{part.content}</p>
                              </ThoughtDetails>
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
                className="flex h-full w-full items-center justify-center text-stone-400 hover:bg-stone-50 hover:text-stone-700 lg:items-start lg:pt-4"
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
          <div className="fixed z-50 w-40 overflow-hidden rounded-xl border border-stone-200 bg-white py-1 text-xs text-stone-700 shadow-2xl" style={{ left: selectionMenu.x, top: selectionMenu.y }}>
            <button onClick={() => void copySelection()} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-stone-100"><Copy className="h-3.5 w-3.5 text-stone-500" />复制</button>
            <button onClick={() => { setQuotedSelection(selectionMenu.selection); setSelectionMenu(null); setIsDiscussionOpen(true); }} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-stone-100"><Quote className="h-3.5 w-3.5 text-stone-500" />引用</button>
            <button onClick={() => void translateSelection()} disabled={!conversationReady || isStreaming} className="flex w-full items-center gap-2 px-3 py-1.5 text-left transition-colors hover:bg-stone-100 disabled:cursor-not-allowed disabled:opacity-40"><Languages className="h-3.5 w-3.5 text-stone-500" />翻译</button>
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
