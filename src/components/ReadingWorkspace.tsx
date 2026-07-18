import React, { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import ePub, { type Book, type Contents, type NavItem, type Rendition } from "epubjs";
import {
  BookMarked,
  BookOpen,
  ChevronLeft,
  ChevronRight,
  FileUp,
  Highlighter,
  LoaderCircle,
  Menu,
  MessageCircleMore,
  PanelLeftClose,
  Send,
  ShieldAlert,
  Sparkles,
  X,
} from "lucide-react";
import { MarkdownMessage } from "./MarkdownMessage";
import { useAgentStore } from "../store/useAgentStore";

interface ReadingBook {
  id: string;
  collection_id: string;
  document_id: string;
  local_path: string;
  title: string;
  author: string | null;
  source_hash: string;
  model_knows_content: boolean;
  content_context_allowed: boolean;
  content_context_decided: boolean;
  progress_cfi: string | null;
  created_at: string;
  updated_at: string;
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

interface PendingSelection {
  cfiRange: string;
  quote: string;
  contextBefore: string;
  contextAfter: string;
}

interface ReadingWorkspaceProps {
  isSidebarOpen: boolean;
  onToggleSidebar: () => void;
}

const HIGHLIGHT_STYLES: Record<string, Record<string, string>> = {
  yellow: { fill: "#f7d774", "fill-opacity": "0.45", "mix-blend-mode": "multiply" },
  green: { fill: "#8fbc8f", "fill-opacity": "0.42", "mix-blend-mode": "multiply" },
  blue: { fill: "#8bc4e8", "fill-opacity": "0.42", "mix-blend-mode": "multiply" },
  pink: { fill: "#e6a7c5", "fill-opacity": "0.42", "mix-blend-mode": "multiply" },
};

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

const EpubPane: React.FC<{
  book: ReadingBook;
  highlights: ReadingHighlight[];
  onProgress: (cfi: string) => void;
  onSelectPassage: (selection: PendingSelection) => void;
}> = ({ book, highlights, onProgress, onSelectPassage }) => {
  const hostRef = useRef<HTMLDivElement>(null);
  const bookRef = useRef<Book | null>(null);
  const renditionRef = useRef<Rendition | null>(null);
  const [toc, setToc] = useState<NavItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let disposed = false;
    let rendered: Rendition | null = null;
    const source = convertFileSrc(book.local_path);

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
        rendered.themes.default({
          body: { color: "#282723", "font-family": "Georgia, 'Noto Serif SC', serif", "line-height": "1.8" },
          p: { "margin-bottom": "1em" },
          img: { "max-width": "100%", height: "auto" },
        });
        rendered.on("relocated", (location: { start?: { cfi?: string } }) => {
          const cfi = location?.start?.cfi;
          if (cfi) onProgress(cfi);
        });
        rendered.on("rendered", applyHighlights);
        rendered.on("selected", (cfiRange: string, contents: Contents) => {
          const selection = contents.window.getSelection();
          const quote = selection?.toString().replace(/\s+/g, " ").trim() ?? "";
          if (!quote) return;
          const nearby = selection ? nearbyParagraphs(selection) : { before: "", after: "" };
          onSelectPassage({
            cfiRange,
            quote,
            contextBefore: nearby.before,
            contextAfter: nearby.after,
          });
          selection?.removeAllRanges();
        });
        await rendered.display(book.progress_cfi || undefined);
        if (!disposed) {
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

export const ReadingWorkspace: React.FC<ReadingWorkspaceProps> = ({ isSidebarOpen, onToggleSidebar }) => {
  const activeAgentId = useAgentStore((state) => state.activeAgentId);
  const activeSessionId = useAgentStore((state) => state.activeSessionId);
  const messages = useAgentStore((state) => state.messages);
  const isStreaming = useAgentStore((state) => state.isStreaming);
  const sendMessage = useAgentStore((state) => state.sendMessage);
  const setActiveSessionId = useAgentStore((state) => state.setActiveSessionId);
  const [books, setBooks] = useState<ReadingBook[]>([]);
  const [selectedBookId, setSelectedBookId] = useState<string | null>(null);
  const [highlights, setHighlights] = useState<ReadingHighlight[]>([]);
  const [pendingSelection, setPendingSelection] = useState<PendingSelection | null>(null);
  const [question, setQuestion] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showConsent, setShowConsent] = useState(false);
  const [readingSessionId, setReadingSessionId] = useState<string | null>(null);
  const messageEndRef = useRef<HTMLDivElement>(null);

  const selectedBook = useMemo(
    () => books.find((book) => book.id === selectedBookId) ?? null,
    [books, selectedBookId],
  );

  const loadBooks = async () => {
    const next = await invoke<ReadingBook[]>("list_reading_books");
    setBooks(next);
    setSelectedBookId((current) => current && next.some((book) => book.id === current) ? current : next[0]?.id ?? null);
  };

  useEffect(() => { void loadBooks().catch((reason) => setError(String(reason))); }, []);

  useEffect(() => {
    if (!selectedBook) { setHighlights([]); return; }
    void invoke<ReadingHighlight[]>("list_reading_highlights", { bookId: selectedBook.id })
      .then(setHighlights)
      .catch((reason) => setError(String(reason)));
  }, [selectedBook?.id]);

  useEffect(() => {
    let cancelled = false;
    setReadingSessionId(null);
    if (!selectedBook || !activeAgentId) return () => { cancelled = true; };
    void invoke<string>("open_reading_book_conversation", { bookId: selectedBook.id, agentId: activeAgentId })
      .then(async (sessionId) => {
        await setActiveSessionId(sessionId);
        if (!cancelled) setReadingSessionId(sessionId);
      })
      .catch((reason) => {
        if (!cancelled) setError(String(reason));
      });
    return () => { cancelled = true; };
  }, [selectedBook?.id, activeAgentId]);

  useEffect(() => { messageEndRef.current?.scrollIntoView({ behavior: isStreaming ? "auto" : "smooth" }); }, [messages, isStreaming]);

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

  const saveHighlight = async () => {
    if (!selectedBook || !pendingSelection) return;
    try {
      const highlight = await invoke<ReadingHighlight>("create_reading_highlight", {
        bookId: selectedBook.id,
        payload: { ...pendingSelection, color: "yellow" },
      });
      setHighlights((current) => [...current, highlight]);
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
      const text = readingQuestion(selectedBook, question.trim(), pendingSelection);
      setQuestion("");
      setShowConsent(false);
      await sendMessage(readingSessionId, text);
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

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-[#faf9f5] lg:flex-row">
      <section className="flex h-36 shrink-0 flex-col border-b border-stone-200 bg-white/55 lg:h-auto lg:w-60 lg:border-b-0 lg:border-r">
        <header className="flex h-14 items-center justify-between border-b border-stone-200 px-4">
          <div className="flex items-center gap-2 text-sm font-semibold text-stone-800"><BookMarked className="h-4 w-4 text-emerald-700" />阅读</div>
          <button onClick={onToggleSidebar} className="rounded p-1 text-stone-500 hover:bg-stone-100" title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}>
            {isSidebarOpen ? <PanelLeftClose className="h-4 w-4" /> : <Menu className="h-4 w-4" />}
          </button>
        </header>
        <div className="flex items-center gap-2 border-b border-stone-200 p-3">
          <button onClick={() => void importBook()} disabled={!activeAgentId || loading} className="flex flex-1 items-center justify-center gap-2 rounded-md bg-emerald-700 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-800 disabled:opacity-50">
            {loading ? <LoaderCircle className="h-3.5 w-3.5 animate-spin" /> : <FileUp className="h-3.5 w-3.5" />} 导入 EPUB
          </button>
        </div>
        <div className="flex min-w-0 flex-1 gap-1 overflow-x-auto p-2 lg:block lg:overflow-y-auto">
          {books.map((book) => (
            <button key={book.id} onClick={() => { setSelectedBookId(book.id); setPendingSelection(null); setShowConsent(false); }} className={`min-w-36 rounded-lg px-3 py-2.5 text-left lg:mb-1 lg:w-full ${book.id === selectedBookId ? "bg-emerald-50 text-emerald-900" : "text-stone-600 hover:bg-stone-100"}`}>
              <span className="block truncate text-xs font-semibold">{book.title}</span>
              <span className="mt-0.5 block truncate text-[10px] text-stone-400">{book.author || "未知作者"}</span>
            </button>
          ))}
          {!books.length && <p className="px-4 py-10 text-center text-xs leading-relaxed text-stone-400">导入 EPUB 后，可边阅读边与 AI 讨论划线内容。</p>}
        </div>
      </section>

      {selectedBook ? (
        <>
          <div className="relative flex min-h-64 min-w-0 flex-1 lg:min-h-0">
            <EpubPane
              book={selectedBook}
              highlights={highlights}
              onProgress={(cfi) => {
                setBooks((current) => current.map((book) => book.id === selectedBook.id ? { ...book, progress_cfi: cfi } : book));
                void invoke("update_reading_book_progress", { bookId: selectedBook.id, cfi }).catch(console.error);
              }}
              onSelectPassage={setPendingSelection}
            />
            {pendingSelection && (
              <div className="absolute bottom-4 left-1/2 z-20 flex max-w-[calc(100%-2rem)] -translate-x-1/2 items-center gap-2 rounded-lg border border-stone-200 bg-white px-3 py-2 shadow-lg">
                <Highlighter className="h-4 w-4 shrink-0 text-amber-600" />
                <span className="max-w-48 truncate text-xs text-stone-600">已选 {pendingSelection.quote.length} 个字符</span>
                <button onClick={() => void saveHighlight()} className="rounded-md bg-amber-100 px-2 py-1 text-[11px] font-semibold text-amber-800 hover:bg-amber-200">保存高亮</button>
                <button onClick={() => setQuestion((current) => current || "请分析这段文字。") } className="rounded-md bg-emerald-700 px-2 py-1 text-[11px] font-semibold text-white hover:bg-emerald-800">讨论</button>
                <button onClick={() => setPendingSelection(null)} className="rounded p-1 text-stone-400 hover:bg-stone-100" title="取消选择"><X className="h-3.5 w-3.5" /></button>
              </div>
            )}
          </div>

          <aside className="flex h-[42%] min-h-64 w-full shrink-0 flex-col border-t border-stone-200 bg-white/75 lg:h-auto lg:w-[min(36vw,430px)] lg:min-w-80 lg:border-l lg:border-t-0">
            <header className="border-b border-stone-200 px-4 py-3">
              <div className="flex items-center gap-2 text-sm font-semibold text-stone-800"><MessageCircleMore className="h-4 w-4 text-emerald-700" />阅读讨论</div>
              <label className="mt-2 flex cursor-pointer items-center gap-2 text-[11px] text-stone-500">
                <input type="checkbox" checked={selectedBook.model_knows_content} onChange={(event) => void updateBook("update_reading_book_mode", { modelKnowsContent: event.target.checked })} className="accent-emerald-700" />
                AI 已知这本书，仅发送划线与附近段落
              </label>
              {!selectedBook.model_knows_content && selectedBook.content_context_decided && (
                <label className="mt-1.5 flex cursor-pointer items-center gap-2 text-[11px] text-stone-500">
                  <input type="checkbox" checked={selectedBook.content_context_allowed} onChange={(event) => void updateBook("set_reading_book_content_context_allowed", { allowed: event.target.checked })} className="accent-emerald-700" />
                  允许当前模型检索本书相关片段
                </label>
              )}
            </header>
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-4 py-5">
              {readingSessionId === activeSessionId && messages.map((message) => (
                <div key={message.id} className={message.role === "user" ? "ml-6" : "mr-3"}>
                  <div className={`rounded-lg px-3 py-2 text-sm ${message.role === "user" ? "bg-emerald-50 text-stone-800" : "border border-stone-200 bg-white text-stone-700"}`}>
                    {message.parts.filter((part) => part.kind === "text").map((part) => (
                      <MarkdownMessage key={part.id} content={part.content} streaming={isStreaming && message.status !== "complete"} />
                    ))}
                  </div>
                </div>
              ))}
              {(!readingSessionId || readingSessionId !== activeSessionId || !messages.length) && <p className="py-12 text-center text-xs leading-relaxed text-stone-400">选中正文后可保存高亮，或直接提问。</p>}
              <div ref={messageEndRef} />
            </div>
            <div className="border-t border-stone-200 p-3">
              {pendingSelection && <div className="mb-2 rounded-md bg-amber-50 px-2.5 py-2 text-[11px] leading-relaxed text-amber-800">将引用当前划线及前后段落。</div>}
              <textarea value={question} onChange={(event) => setQuestion(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); ask(); } }} placeholder="问问这本书..." className="h-20 w-full resize-none rounded-md border border-stone-200 bg-white px-3 py-2 text-sm outline-none focus:border-emerald-500" />
              <button onClick={ask} disabled={!question.trim() || isStreaming || !readingSessionId || readingSessionId !== activeSessionId} className="mt-2 flex w-full items-center justify-center gap-2 rounded-md bg-emerald-700 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-800 disabled:opacity-50"><Send className="h-3.5 w-3.5" />发送</button>
            </div>
          </aside>
        </>
      ) : (
        <div className="grid flex-1 place-items-center text-sm text-stone-400"><div className="text-center"><Sparkles className="mx-auto mb-3 h-6 w-6 text-emerald-700" />从书架导入一本 EPUB 开始阅读。</div></div>
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
      {error && <div className="fixed bottom-4 left-1/2 z-50 max-w-xl -translate-x-1/2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700 shadow">{error}</div>}
    </main>
  );
};
