import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVoiceAecStatus, type VoiceAecStatus } from "../lib/voiceAec";

export function VoiceAecSettings() {
  const [status, setStatus] = useState<VoiceAecStatus | null>(null);
  const [hidden, setHidden] = useState(false);
  const [action, setAction] = useState<"connect" | "stop" | null>(null);
  const busy = action !== null;
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const revisionRef = useRef(0);
  const busyRef = useRef(false);

  useEffect(() => {
    mountedRef.current = true;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      const revision = revisionRef.current;
      try {
        if (!busyRef.current) {
          const current = await getVoiceAecStatus();
          if (!disposed && revision === revisionRef.current) {
            setHidden(!current?.supported);
            setStatus(current);
          }
        }
      } catch (reason) {
        if (!disposed && revision === revisionRef.current) setError(String(reason));
      } finally {
        if (!disposed) timer = setTimeout(() => void refresh(), 3_000);
      }
    };
    void refresh();
    return () => {
      disposed = true;
      mountedRef.current = false;
      if (timer) clearTimeout(timer);
    };
  }, []);

  const update = async (enabled: boolean) => {
    if (busyRef.current) return;
    busyRef.current = true;
    revisionRef.current += 1;
    setAction(enabled ? "connect" : "stop");
    setError(null);
    try {
      if (enabled) {
        const next = await invoke<VoiceAecStatus>("start_voice_aec");
        if (mountedRef.current) setStatus(next);
      } else {
        await invoke("stop_voice_aec");
        const next = await getVoiceAecStatus();
        if (mountedRef.current) setStatus(next);
      }
    } catch (reason) {
      if (mountedRef.current) setError(String(reason));
      const current = await getVoiceAecStatus().catch(() => null);
      if (mountedRef.current && current) setStatus(current);
    } finally {
      busyRef.current = false;
      if (mountedRef.current) setAction(null);
    }
  };

  if (hidden) return null;
  return (
    <section className="space-y-3 border-b border-stone-200 pb-5" aria-labelledby="voice-aec-heading">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="space-y-1">
          <h4 id="voice-aec-heading" className="text-xs font-semibold text-stone-700">扬声器回声消除 · Linux</h4>
          <p className="text-xs leading-5 text-stone-600">过滤麦克风拾取的播放声音，减少 Agnes 听到自己的回复。</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <button type="button" disabled={busy || !status} onClick={() => void update(true)} className="min-h-11 rounded-md border border-stone-300 px-3 text-xs font-semibold text-stone-700 transition-colors hover:bg-stone-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500 disabled:cursor-not-allowed disabled:opacity-50">
            {action === "connect" ? "正在连接…" : status?.running ? "重新连接" : "启用回声消除"}
          </button>
          {status?.running && <button type="button" disabled={busy} onClick={() => void update(false)} className="min-h-11 rounded-md border border-stone-200 px-3 text-xs text-stone-700 transition-colors hover:bg-stone-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500 disabled:cursor-not-allowed disabled:opacity-50">{action === "stop" ? "正在停用…" : "停用"}</button>}
        </div>
      </div>
      <div role="status" aria-live="polite" className="text-xs leading-5 text-stone-700">
        {!status ? "正在读取音频设备…" : !status.running ? "尚未启用" : status.routes_ready ? "输入与播放参考已连接 · 声音效果待试听" : "音频设备连接需要恢复"}
      </div>
      {(error || status?.message) && <p role="alert" className="text-xs leading-5 text-rose-700">{error || status?.message}</p>}
      {status && <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs leading-5">
        <dt className="text-stone-600">麦克风</dt><dd className="break-words text-stone-700">{status.input?.description || "未找到物理麦克风"}</dd>
        <dt className="text-stone-600">播放参考</dt><dd className="break-words text-stone-700">{status.output?.description || "未找到输出设备"}</dd>
      </dl>}
      <p className="text-xs leading-5 text-stone-600">启用后，语音对话会优先使用过滤后的麦克风。切换系统输入或输出后请重新连接；关闭 Agnes 后自动释放，下次启动需再次启用。</p>
      {status?.running && <p className="text-xs leading-5 text-stone-600">语音对话会在播放期间保持回声消除采集，允许直接插话；按钮或 Esc 仍可立即打断回复。</p>}
    </section>
  );
}
