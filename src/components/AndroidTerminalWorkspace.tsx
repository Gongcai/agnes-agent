import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowsClockwise, Check, TerminalWindow, X } from "@phosphor-icons/react";
import {
  closeUserLinuxTerminal,
  createUserLinuxTerminal,
  interruptUserLinuxTerminal,
  listUserLinuxTerminals,
  pollUserLinuxTerminal,
  resizeUserLinuxTerminal,
  type UserLinuxTerminalInfo,
  type UserLinuxTerminalPoll,
  writeUserLinuxTerminal,
} from "../lib/ipc";

interface Props {
  onClose: () => void;
}

const DEFAULT_ROWS = 30;
const DEFAULT_COLS = 100;

export function AndroidTerminalWorkspace({ onClose }: Props) {
  const [terminals, setTerminals] = useState<UserLinuxTerminalInfo[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [output, setOutput] = useState("");
  const [input, setInput] = useState("");
  const [cursor, setCursor] = useState(0);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const outputRef = useRef<HTMLPreElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const activeTerminal = terminals.find((terminal) => terminal.sessionId === activeId);
  const statusLabel = running
    ? "运行中"
    : activeTerminal?.signal
      ? `已中断 (${activeTerminal.signal})`
      : activeTerminal?.exitCode !== null && activeTerminal?.exitCode !== undefined
        ? `已退出 (${activeTerminal.exitCode})`
        : "已停止";

  const refreshTerminals = useCallback(async () => {
    try {
      setTerminals(await listUserLinuxTerminals());
    } catch (reason) {
      setError(String(reason));
    }
  }, []);

  const applyPoll = useCallback((poll: UserLinuxTerminalPoll, replace = false) => {
    setActiveId(poll.sessionId);
    setCursor(poll.nextCursor);
    setRunning(poll.running);
    setOutput((previous) => replace ? poll.output : previous + poll.output);
  }, []);

  const pollActive = useCallback(async () => {
    if (!activeId) return;
    try {
      applyPoll(await pollUserLinuxTerminal(activeId, cursor));
      await refreshTerminals();
    } catch (reason) {
      setError(String(reason));
    }
  }, [activeId, applyPoll, cursor, refreshTerminals]);

  useEffect(() => {
    refreshTerminals();
  }, [refreshTerminals]);

  useEffect(() => {
    if (!activeId || !running) return;
    const timer = window.setInterval(pollActive, 250);
    return () => window.clearInterval(timer);
  }, [activeId, pollActive, running]);

  useEffect(() => {
    if (outputRef.current) outputRef.current.scrollTop = outputRef.current.scrollHeight;
  }, [output]);

  const createTerminal = async () => {
    try {
      setError(null);
      const poll = await createUserLinuxTerminal("/root", DEFAULT_ROWS, DEFAULT_COLS);
      setOutput("");
      setCursor(0);
      applyPoll(poll, true);
      await refreshTerminals();
      inputRef.current?.focus();
    } catch (reason) {
      setError(String(reason));
    }
  };

  const selectTerminal = async (terminal: UserLinuxTerminalInfo) => {
    try {
      setError(null);
      setOutput("");
      setCursor(0);
      applyPoll(await pollUserLinuxTerminal(terminal.sessionId, 0), true);
    } catch (reason) {
      setError(String(reason));
    }
  };

  const sendInput = async () => {
    if (!activeId || !input) return;
    try {
      await writeUserLinuxTerminal(activeId, input);
      setInput("");
      await pollActive();
    } catch (reason) {
      setError(String(reason));
    }
  };

  const resize = async (delta: number) => {
    const current = terminals.find((terminal) => terminal.sessionId === activeId);
    if (!activeId || !current) return;
    try {
      await resizeUserLinuxTerminal(activeId, current.rows, Math.max(40, current.cols + delta));
      await refreshTerminals();
    } catch (reason) {
      setError(String(reason));
    }
  };

  const interrupt = async () => {
    if (!activeId) return;
    try {
      await interruptUserLinuxTerminal(activeId);
      await pollActive();
    } catch (reason) {
      setError(String(reason));
    }
  };

  const closeTerminal = async () => {
    if (!activeId) return;
    try {
      await closeUserLinuxTerminal(activeId);
      setActiveId(null);
      setOutput("");
      setCursor(0);
      setRunning(false);
      await refreshTerminals();
    } catch (reason) {
      setError(String(reason));
    }
  };

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-[#111313] text-stone-100" aria-label="Linux 终端">
      <header className="flex min-h-14 items-center gap-3 border-b border-white/10 px-4">
        <TerminalWindow className="h-5 w-5 text-coral" />
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-sm font-semibold">Ubuntu 终端</h1>
          <p className="truncate text-[11px] text-stone-500">
            用户 PTY · {statusLabel}{activeTerminal ? ` · ${activeTerminal.cwd}` : ""}
          </p>
        </div>
        <button type="button" className="rounded-md p-2 text-stone-400 hover:bg-white/10 hover:text-white" onClick={createTerminal} title="新建终端">
          <TerminalWindow className="h-4 w-4" />
        </button>
        <button type="button" className="rounded-md p-2 text-stone-400 hover:bg-white/10 hover:text-white" onClick={refreshTerminals} title="刷新终端">
          <ArrowsClockwise className="h-4 w-4" />
        </button>
        <button type="button" className="rounded-md p-2 text-stone-400 hover:bg-white/10 hover:text-white" onClick={onClose} title="关闭终端">
          <X className="h-4 w-4" />
        </button>
      </header>
      <div className="flex min-h-0 flex-1">
        <aside className="w-56 shrink-0 border-r border-white/10 p-3">
          <div className="mb-2 text-[10px] uppercase tracking-wider text-stone-500">会话</div>
          <div className="space-y-1">
            {terminals.map((terminal) => (
              <button
                key={terminal.sessionId}
                type="button"
                onClick={() => selectTerminal(terminal)}
                className={`flex w-full items-center gap-2 rounded-md px-2 py-2 text-left text-xs ${terminal.sessionId === activeId ? "bg-white/10 text-white" : "text-stone-400 hover:bg-white/5 hover:text-stone-200"}`}
              >
                <span className={`h-1.5 w-1.5 rounded-full ${terminal.running ? "bg-emerald-400" : "bg-stone-600"}`} />
                <span className="min-w-0 flex-1 truncate">{terminal.sessionId.slice(-8)}</span>
                {!terminal.running && <Check className="h-3 w-3 text-stone-600" />}
              </button>
            ))}
            {terminals.length === 0 && <p className="px-2 py-3 text-xs text-stone-600">暂无终端</p>}
          </div>
        </aside>
        <section className="flex min-w-0 flex-1 flex-col">
          <pre ref={outputRef} className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap break-words p-4 font-mono text-xs leading-5 text-stone-200">{output || "点击新建终端启动 Ubuntu shell"}</pre>
          {error && <div className="border-t border-red-400/20 bg-red-950/30 px-4 py-2 text-xs text-red-300">{error}</div>}
          <div className="flex items-center gap-2 border-t border-white/10 p-3">
            <button type="button" className="rounded-md px-2 py-1.5 text-xs text-stone-400 hover:bg-white/10 hover:text-white" onClick={interrupt} disabled={!activeId || !running} title="发送 Ctrl+C">Ctrl+C</button>
            <button type="button" className="rounded-md px-2 py-1.5 text-xs text-stone-400 hover:bg-white/10 hover:text-white" onClick={() => resize(-10)} disabled={!activeId} title="缩小终端宽度">-</button>
            <button type="button" className="rounded-md px-2 py-1.5 text-xs text-stone-400 hover:bg-white/10 hover:text-white" onClick={() => resize(10)} disabled={!activeId} title="放大终端宽度">+</button>
            <input
              ref={inputRef}
              value={input}
              onChange={(event) => setInput(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  sendInput();
                }
              }}
              placeholder={activeId ? "输入命令或原始终端输入" : "先创建终端"}
              disabled={!activeId}
              className="min-w-0 flex-1 rounded-md border border-white/10 bg-black/30 px-3 py-2 font-mono text-xs text-white outline-none focus:border-coral/60 disabled:opacity-40"
            />
            <button type="button" className="rounded-md bg-coral px-3 py-2 text-xs font-medium text-white disabled:opacity-40" onClick={sendInput} disabled={!activeId || !input}>发送</button>
            <button type="button" className="rounded-md p-2 text-stone-400 hover:bg-white/10 hover:text-white" onClick={closeTerminal} disabled={!activeId} title="关闭当前会话"><X className="h-4 w-4" /></button>
          </div>
        </section>
      </div>
    </main>
  );
}
