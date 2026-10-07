import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, Microphone, MicrophoneSlash, PhoneDisconnect, SpeakerHigh, SpeakerSlash, Warning } from "@phosphor-icons/react";

type VoiceVisualState = "idle" | "listening" | "error";

interface VoiceCallPanelProps {
  agentName: string;
  sessionName?: string;
  onClose: () => void;
}

export function VoiceCallPanel({ agentName, sessionName, onClose }: VoiceCallPanelProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const contextRef = useRef<AudioContext | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const [visualState, setVisualState] = useState<VoiceVisualState>("idle");
  const [muted, setMuted] = useState(false);
  const [speakerMuted, setSpeakerMuted] = useState(false);
  const [level, setLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const reducedMotion = useRef(false);

  const stopCapture = useCallback(() => {
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    analyserRef.current = null;
    const context = contextRef.current;
    contextRef.current = null;
    if (context && context.state !== "closed") void context.close();
    setVisualState("idle");
    setLevel(0);
  }, []);

  const startCapture = useCallback(async () => {
    setError(null);
    if (!navigator.mediaDevices?.getUserMedia) {
      setError("此环境无法访问麦克风。请检查系统权限或使用桌面版。");
      setVisualState("error");
      return;
    }
    try {
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      });
      const context = new AudioContext();
      const analyser = context.createAnalyser();
      analyser.fftSize = 512;
      analyser.smoothingTimeConstant = 0.78;
      context.createMediaStreamSource(stream).connect(analyser);
      streamRef.current = stream;
      contextRef.current = context;
      analyserRef.current = analyser;
      setMuted(false);
      setVisualState("listening");
    } catch (reason) {
      const name = reason instanceof DOMException ? reason.name : "";
      setError(name === "NotAllowedError"
        ? "麦克风权限未开启。请在系统设置中允许 Agnes 访问麦克风。"
        : name === "NotFoundError"
          ? "没有找到可用的麦克风。连接设备后重试。"
          : `无法启动麦克风：${reason instanceof Error ? reason.message : String(reason)}`);
      setVisualState("error");
    }
  }, []);

  useEffect(() => {
    reducedMotion.current = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => { reducedMotion.current = media.matches; };
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    let frame = 0;
    let lastMeterUpdate = 0;
    const draw = (time: number) => {
      const canvas = canvasRef.current;
      const context = canvas?.getContext("2d");
      if (canvas && context) {
        const bounds = canvas.getBoundingClientRect();
        const scale = window.devicePixelRatio || 1;
        const width = Math.max(1, Math.round(bounds.width * scale));
        const height = Math.max(1, Math.round(bounds.height * scale));
        if (canvas.width !== width || canvas.height !== height) {
          canvas.width = width;
          canvas.height = height;
        }
        context.setTransform(scale, 0, 0, scale, 0, 0);
        context.clearRect(0, 0, bounds.width, bounds.height);
        const analyser = analyserRef.current;
        let rms = 0;
        if (analyser && !muted) {
          const samples = new Uint8Array(analyser.fftSize);
          analyser.getByteTimeDomainData(samples);
          let sum = 0;
          for (const sample of samples) {
            const value = (sample - 128) / 128;
            sum += value * value;
          }
          rms = Math.min(1, Math.sqrt(sum / samples.length) * 3.4);
          if (time - lastMeterUpdate > 45) {
            lastMeterUpdate = time;
            setLevel(rms);
          }
        } else if (time - lastMeterUpdate > 45) {
          lastMeterUpdate = time;
          setLevel(0);
        }
        const cx = bounds.width / 2;
        const cy = bounds.height / 2;
        const base = Math.min(bounds.width, bounds.height) * 0.285;
        const quietPulse = visualState === "listening" && !reducedMotion.current
          ? Math.sin(time / 1050) * 0.014 : 0;
        const radius = base * (1 + quietPulse + rms * 0.19);

        const glow = context.createRadialGradient(cx, cy, radius * 0.2, cx, cy, radius * 1.8);
        glow.addColorStop(0, visualState === "error" ? "rgba(206,103,91,0.20)" : "rgba(207,130,107,0.23)");
        glow.addColorStop(0.58, "rgba(165,150,146,0.08)");
        glow.addColorStop(1, "rgba(165,150,146,0)");
        context.fillStyle = glow;
        context.beginPath();
        context.arc(cx, cy, radius * 1.8, 0, Math.PI * 2);
        context.fill();

        const gradient = context.createRadialGradient(
          cx - radius * 0.3, cy - radius * 0.38, radius * 0.05, cx, cy, radius * 1.08,
        );
        if (visualState === "error") {
          gradient.addColorStop(0, "#fff5f1");
          gradient.addColorStop(0.48, "#e6b4a8");
          gradient.addColorStop(1, "#9f625e");
        } else {
          gradient.addColorStop(0, "#fffefa");
          gradient.addColorStop(0.43, "#e9ded7");
          gradient.addColorStop(0.78, "#c59a8e");
          gradient.addColorStop(1, "#916f6c");
        }
        context.save();
        context.translate(cx, cy);
        context.rotate(visualState === "listening" && !reducedMotion.current ? Math.sin(time / 4300) * 0.018 : 0);
        context.beginPath();
        const points = 96;
        for (let index = 0; index <= points; index += 1) {
          const angle = (index / points) * Math.PI * 2;
          const wave = reducedMotion.current ? 0 : Math.sin(angle * 3 + time / 780) * rms * 0.075
            + Math.sin(angle * 5 - time / 1650) * (visualState === "listening" ? 0.012 : 0.004);
          const r = radius * (1 + wave);
          const x = Math.cos(angle) * r;
          const y = Math.sin(angle) * r;
          if (index === 0) context.moveTo(x, y);
          else context.lineTo(x, y);
        }
        context.closePath();
        context.fillStyle = gradient;
        context.shadowColor = "rgba(202,151,135,0.22)";
        context.shadowBlur = 28 + rms * 24;
        context.fill();
        context.restore();
      }
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, [muted, visualState]);

  useEffect(() => () => stopCapture(), [stopCapture]);

  const close = () => {
    stopCapture();
    onClose();
  };

  const toggleMic = () => {
    if (!streamRef.current) {
      void startCapture();
      return;
    }
    const nextMuted = !muted;
    streamRef.current.getAudioTracks().forEach((track) => { track.enabled = !nextMuted; });
    setMuted(nextMuted);
  };

  const status = visualState === "error" ? "麦克风不可用"
    : !streamRef.current ? "准备开始"
      : muted ? "麦克风已静音"
        : level > 0.035 ? "正在聆听" : "我在听";

  return (
    <section className="agnes-voice-call absolute inset-0 z-40 flex min-h-0 flex-col overflow-hidden text-white" aria-label="Agnes 语音对话">
      <header className="flex h-14 shrink-0 items-center justify-between border-b border-white/10 px-4 sm:px-7">
        <button type="button" onClick={close} className="flex min-h-11 items-center gap-2 rounded-full px-3 text-sm text-white/75 transition-colors hover:bg-white/10 hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f]" aria-label="返回聊天">
          <ArrowLeft size={18} aria-hidden="true" />
          <span>返回聊天</span>
        </button>
        <div className="min-w-0 px-3 text-center">
          <div className="truncate text-[13px] font-medium tracking-wide text-white/90">{agentName}</div>
          <div className="truncate text-[10px] text-white/45">{sessionName || "语音对话"}</div>
        </div>
        <button type="button" onClick={close} className="flex min-h-11 items-center gap-2 rounded-full border border-rose-300/20 bg-rose-400/10 px-3.5 text-xs font-medium text-rose-100 transition-colors hover:bg-rose-400/20 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-rose-200" aria-label="结束语音对话">
          <PhoneDisconnect size={16} aria-hidden="true" />
          <span className="hidden sm:inline">结束</span>
        </button>
      </header>

      <div className="relative flex min-h-0 flex-1 flex-col items-center justify-center px-5 pb-4 pt-2">
        <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_50%_48%,rgba(105,78,74,0.24),transparent_48%),linear-gradient(145deg,#171717,#101010_58%,#191615)]" aria-hidden="true" />
        <div className="relative z-10 flex min-h-0 w-full flex-1 flex-col items-center justify-center">
          <div className="relative aspect-square w-[min(66vw,360px)] max-h-[48vh] sm:w-[min(40vw,420px)] sm:max-h-[52vh]">
            <canvas ref={canvasRef} className="h-full w-full" aria-hidden="true" />
          </div>
          <div className="mt-1 min-h-12 text-center">
            <div className="text-sm font-medium tracking-wide text-white/90" aria-live="polite">{status}</div>
            {visualState === "listening" && !muted && (
              <div className="mt-1 text-[11px] text-white/45">麦克风音量 {Math.round(level * 100)}%</div>
            )}
          </div>
        </div>

        <div className="relative z-10 mb-3 w-full max-w-lg rounded-2xl border border-white/[0.08] bg-black/20 px-4 py-3 text-center backdrop-blur-sm">
          {error ? (
            <div role="alert" className="flex items-start justify-center gap-2 text-xs leading-5 text-rose-100">
              <Warning size={16} className="mt-0.5 shrink-0" aria-hidden="true" />
              <span>{error}</span>
            </div>
          ) : (
            <p className="text-xs leading-5 text-white/55">
              {visualState === "listening"
                ? "麦克风已连接。云端识别与语音回复接入后，即可开始实时对话。"
                : "允许 Agnes 使用麦克风，查看实时音量与通话动态效果。"}
            </p>
          )}
        </div>

        <footer className="relative z-10 flex items-center justify-center gap-3 pb-[max(0.5rem,env(safe-area-inset-bottom))]">
          <button type="button" onClick={toggleMic} className={`flex min-h-12 min-w-12 items-center justify-center gap-2 rounded-full border px-4 transition-all focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f] ${muted ? "border-rose-300/30 bg-rose-400/15 text-rose-100" : "border-white/15 bg-white/[0.08] text-white hover:bg-white/[0.14]"}`} aria-label={streamRef.current ? muted ? "取消麦克风静音" : "静音麦克风" : "开启麦克风"} aria-pressed={muted}>
            {muted ? <MicrophoneSlash size={19} aria-hidden="true" /> : <Microphone size={19} aria-hidden="true" />}
            <span className="text-xs">{streamRef.current ? muted ? "取消静音" : "静音" : "开启麦克风"}</span>
          </button>
          <button type="button" onClick={() => setSpeakerMuted((value) => !value)} className={`flex min-h-12 min-w-12 items-center justify-center gap-2 rounded-full border px-4 transition-all focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f] ${speakerMuted ? "border-white/10 bg-white/[0.04] text-white/40" : "border-white/15 bg-white/[0.08] text-white hover:bg-white/[0.14]"}`} aria-label={speakerMuted ? "取消扬声器静音" : "静音扬声器"} aria-pressed={speakerMuted} disabled title="语音播放接入后可用">
            {speakerMuted ? <SpeakerSlash size={19} aria-hidden="true" /> : <SpeakerHigh size={19} aria-hidden="true" />}
            <span className="text-xs">扬声器</span>
          </button>
        </footer>
      </div>
    </section>
  );
}
