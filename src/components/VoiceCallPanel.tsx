import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, Microphone, MicrophoneSlash, PhoneDisconnect, SpeakerHigh, SpeakerSlash, Warning } from "@phosphor-icons/react";

type VoiceVisualState = "idle" | "listening" | "speaking" | "error";

interface VoiceCallPanelProps {
  agentName: string;
  sessionName?: string;
  sessionId?: string | null;
  onClose: () => void;
  onTranscript?: (text: string) => Promise<void> | void;
}

async function toMonoWav16k(blob: Blob): Promise<Blob> {
  const decodeContext = new AudioContext();
  try {
    const decoded = await decodeContext.decodeAudioData(await blob.arrayBuffer());
    const length = Math.max(1, Math.ceil(decoded.duration * 16000));
    const offline = new OfflineAudioContext(1, length, 16000);
    const source = offline.createBufferSource();
    source.buffer = decoded;
    source.connect(offline.destination);
    source.start();
    const rendered = await offline.startRendering();
    const samples = rendered.getChannelData(0);
    const buffer = new ArrayBuffer(44 + samples.length * 2);
    const view = new DataView(buffer);
    const writeText = (offset: number, value: string) => {
      for (let index = 0; index < value.length; index += 1) view.setUint8(offset + index, value.charCodeAt(index));
    };
    writeText(0, "RIFF");
    view.setUint32(4, 36 + samples.length * 2, true);
    writeText(8, "WAVE");
    writeText(12, "fmt " );
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, 16000, true);
    view.setUint32(28, 32000, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeText(36, "data");
    view.setUint32(40, samples.length * 2, true);
    for (let index = 0; index < samples.length; index += 1) {
      const sample = Math.max(-1, Math.min(1, samples[index]));
      view.setInt16(44 + index * 2, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
    }
    return new Blob([buffer], { type: "audio/wav" });
  } finally {
    await decodeContext.close();
  }
}

function blobToBase64(blob: Blob): Promise<string> {
  return blob.arrayBuffer().then((buffer) => {
    const bytes = new Uint8Array(buffer);
    let binary = "";
    const chunkSize = 0x8000;
    for (let offset = 0; offset < bytes.length; offset += chunkSize) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
    }
    return btoa(binary);
  });
}

function pcm16Base64ToFloat32(encoded: string): Float32Array<ArrayBuffer> {
  const binary = atob(encoded);
  const sampleCount = Math.floor(binary.length / 2);
  const samples = new Float32Array(sampleCount);
  for (let index = 0; index < sampleCount; index += 1) {
    const value = (binary.charCodeAt(index * 2) | (binary.charCodeAt(index * 2 + 1) << 8));
    samples[index] = (value & 0x8000 ? value - 0x10000 : value) / 32768;
  }
  return samples;
}

export function VoiceCallPanel({ agentName, sessionName, sessionId, onClose, onTranscript }: VoiceCallPanelProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const contextRef = useRef<AudioContext | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const utteranceStartedAtRef = useRef<number | null>(null);
  const silenceSinceRef = useRef<number | null>(null);
  const utteranceChunksRef = useRef<Blob[]>([]);
  const stoppingRef = useRef(false);
  const [visualState, setVisualState] = useState<VoiceVisualState>("idle");
  const [muted, setMuted] = useState(false);
  const [speakerMuted, setSpeakerMuted] = useState(false);
  const speakerMutedRef = useRef(false);
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [selectedAudioInput, setSelectedAudioInput] = useState("");
  const selectedAudioInputRef = useRef("");
  const [level, setLevel] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [utteranceCount, setUtteranceCount] = useState(0);
  const [lastUtteranceMs, setLastUtteranceMs] = useState<number | null>(null);
  const [recognizing, setRecognizing] = useState(false);
  const [transcript, setTranscript] = useState<string | null>(null);
  const [processingError, setProcessingError] = useState<string | null>(null);
  const onTranscriptRef = useRef(onTranscript);
  const mountedRef = useRef(true);
  const captureGenerationRef = useRef(0);
  const recognitionQueueRef = useRef(Promise.resolve());
  const speechBufferRef = useRef("");
  const speechQueueRef = useRef<string[]>([]);
  const speechWorkerRef = useRef(false);
  const playbackGenerationRef = useRef(0);
  const playbackContextRef = useRef<AudioContext | null>(null);
  const scheduledUntilRef = useRef(0);
  const playbackNodesRef = useRef(new Set<AudioBufferSourceNode>());
  const playbackGainsRef = useRef(new Set<GainNode>());
  const activeSynthesisRef = useRef<string | null>(null);
  const nextAudioSequenceRef = useRef(0);
  const synthesisCompletionsRef = useRef(new Map<string, { resolve: () => void; reject: (error: Error) => void }>());
  onTranscriptRef.current = onTranscript;
  const reducedMotion = useRef(false);

  const refreshAudioInputs = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    try {
      const devices = (await navigator.mediaDevices.enumerateDevices())
        .filter((device) => device.kind === "audioinput");
      setAudioInputs(devices);
      if (selectedAudioInputRef.current && !devices.some((device) => device.deviceId === selectedAudioInputRef.current)) {
        selectedAudioInputRef.current = "";
        setSelectedAudioInput("");
      }
    } catch (reason) {
      console.warn("Unable to enumerate audio input devices", reason);
    }
  }, []);

  const cancelCurrentSpeech = useCallback(() => {
    playbackGenerationRef.current += 1;
    speechQueueRef.current = [];
    speechBufferRef.current = "";
    const requestId = activeSynthesisRef.current;
    activeSynthesisRef.current = null;
    if (requestId) {
      void invoke("cancel_voice_synthesis_stream", { requestId }).catch(() => undefined);
      synthesisCompletionsRef.current.get(requestId)?.reject(new Error("voice synthesis cancelled"));
      synthesisCompletionsRef.current.delete(requestId);
    }
    for (const node of playbackNodesRef.current) {
      try { node.stop(); } catch { /* The source may already have ended. */ }
    }
    playbackNodesRef.current.clear();
    const context = playbackContextRef.current;
    if (context && context.state !== "closed") {
      scheduledUntilRef.current = context.currentTime;
    }
  }, []);

  const stopCapture = useCallback(() => {
    captureGenerationRef.current += 1;
    cancelCurrentSpeech();
    stoppingRef.current = true;
    recorderRef.current?.stop();
    recorderRef.current = null;
    utteranceStartedAtRef.current = null;
    silenceSinceRef.current = null;
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    analyserRef.current = null;
    const context = contextRef.current;
    contextRef.current = null;
    playbackContextRef.current = null;
    if (context && context.state !== "closed") void context.close();
    setVisualState("idle");
    setLevel(0);
  }, [cancelCurrentSpeech]);

  const startCapture = useCallback(async (deviceId?: string) => {
    setError(null);
    if (!navigator.mediaDevices?.getUserMedia) {
      setError("此环境无法访问麦克风。请检查系统权限或使用桌面版。");
      setVisualState("error");
      return;
    }
    try {
      const requestedDeviceId = deviceId ?? selectedAudioInputRef.current;
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
          ...(requestedDeviceId ? { deviceId: { exact: requestedDeviceId } } : {}),
        },
      });
      const captureGeneration = captureGenerationRef.current;
      const context = new AudioContext();
      const analyser = context.createAnalyser();
      analyser.fftSize = 512;
      analyser.smoothingTimeConstant = 0.78;
      context.createMediaStreamSource(stream).connect(analyser);
      if (typeof MediaRecorder === "undefined") {
        stream.getTracks().forEach((track) => track.stop());
        throw new Error("当前环境不支持浏览器音频录制");
      }
      const recorder = new MediaRecorder(stream);
      recorder.ondataavailable = (event) => {
        if (event.data.size > 0 && utteranceStartedAtRef.current !== null) {
          utteranceChunksRef.current.push(event.data);
        }
      };
      recorder.onstop = () => {
        if (captureGeneration !== captureGenerationRef.current) {
          utteranceChunksRef.current = [];
          utteranceStartedAtRef.current = null;
          return;
        }
        const startedAt = utteranceStartedAtRef.current;
        const restartRecorder = () => {
          utteranceChunksRef.current = [];
          utteranceStartedAtRef.current = null;
          silenceSinceRef.current = null;
          if (!stoppingRef.current && streamRef.current === stream && recorder.state === "inactive") {
            recorder.start(100);
          }
        };
        if (!startedAt || utteranceChunksRef.current.length === 0) {
          if (startedAt) setProcessingError("未采集到这段语音的音频数据，请重新说一次。");
          restartRecorder();
          return;
        }
        const audio = new Blob(utteranceChunksRef.current, { type: recorder.mimeType || "audio/webm" });
        const durationMs = performance.now() - startedAt;
        setUtteranceCount((count) => count + 1);
        setLastUtteranceMs(durationMs);
        const generation = captureGenerationRef.current;
        recognitionQueueRef.current = recognitionQueueRef.current.then(async () => {
          if (!mountedRef.current || generation !== captureGenerationRef.current) return;
          setRecognizing(true);
          setProcessingError(null);
          try {
            const wav = await toMonoWav16k(audio);
            if (!mountedRef.current || generation !== captureGenerationRef.current) return;
            const text = await invoke<string>("transcribe_voice", {
              sessionId: sessionId ?? null,
              audioBase64: await blobToBase64(wav),
              mimeType: "audio/wav",
              sampleRate: 16000,
            });
            if (!mountedRef.current || generation !== captureGenerationRef.current || !text.trim()) return;
            setTranscript(text.trim());
            await onTranscriptRef.current?.(text.trim());
          } catch (reason) {
            if (mountedRef.current && generation === captureGenerationRef.current) {
              setProcessingError(reason instanceof Error ? reason.message : String(reason));
            }
          } finally {
            if (mountedRef.current && generation === captureGenerationRef.current) setRecognizing(false);
          }
        });
        restartRecorder();
      };
      recorder.start(100);
      recorderRef.current = recorder;
      streamRef.current = stream;
      contextRef.current = context;
      playbackContextRef.current = context;
      scheduledUntilRef.current = context.currentTime;
      analyserRef.current = analyser;
      stoppingRef.current = false;
      setUtteranceCount(0);
      setLastUtteranceMs(null);
      setMuted(false);
      setVisualState("listening");
      void refreshAudioInputs();
    } catch (reason) {
      void refreshAudioInputs();
      const name = reason instanceof DOMException ? reason.name : "";
      const isLinux = navigator.platform.toLowerCase().includes("linux");
      setError(name === "NotAllowedError"
        ? isLinux
          ? "WebKitGTK 未能打开麦克风。请检查 PipeWire/PulseAudio 音频服务和系统麦克风开关，然后重启 Agnes。"
          : "麦克风权限未开启。请在系统设置中允许 Agnes 访问麦克风。"
        : name === "NotFoundError"
          ? "没有找到可用的麦克风。连接设备或在输入设备菜单中重新选择后重试。"
          : name === "NotReadableError"
            ? "麦克风被其他程序占用，或音频服务不可用。请关闭占用麦克风的程序并检查系统音频服务。"
          : `无法启动麦克风：${reason instanceof Error ? reason.message : String(reason)}`);
      setVisualState("error");
    }
  }, [refreshAudioInputs, sessionId]);

  useEffect(() => {
    void refreshAudioInputs();
    const devices = navigator.mediaDevices;
    if (!devices?.addEventListener) return;
    devices.addEventListener("devicechange", refreshAudioInputs);
    return () => devices.removeEventListener("devicechange", refreshAudioInputs);
  }, [refreshAudioInputs]);

  useEffect(() => {
    reducedMotion.current = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => { reducedMotion.current = media.matches; };
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlistenDelta: (() => void) | undefined;
    let unlistenFinished: (() => void) | undefined;
    let unlistenAudioChunk: (() => void) | undefined;
    let unlistenTtsComplete: (() => void) | undefined;
    const enqueueSpeech = (text: string) => {
      const clean = text.trim();
      if (clean) speechQueueRef.current.push(clean);
      if (speechWorkerRef.current) return;
      speechWorkerRef.current = true;
      void (async () => {
        while (!disposed && speechQueueRef.current.length > 0) {
          const next = speechQueueRef.current.shift();
          if (!next) continue;
          const generation = playbackGenerationRef.current;
          const requestId = crypto.randomUUID();
          try {
            setVisualState("speaking");
            activeSynthesisRef.current = requestId;
            nextAudioSequenceRef.current = 0;
            const completed = new Promise<void>((resolve, reject) => {
              synthesisCompletionsRef.current.set(requestId, { resolve, reject });
            });
            await invoke<string>("start_voice_synthesis_stream", {
              sessionId: sessionId ?? null,
              text: next,
              requestId,
            });
            if (disposed || generation !== playbackGenerationRef.current) break;
            await completed;
            if (disposed || generation !== playbackGenerationRef.current) break;
            const context = playbackContextRef.current;
            while (context && context.currentTime < scheduledUntilRef.current && generation === playbackGenerationRef.current) {
              await new Promise((resolve) => window.setTimeout(resolve, 25));
            }
          } catch (reason) {
            if (!disposed && generation === playbackGenerationRef.current) {
              setProcessingError(reason instanceof Error ? reason.message : String(reason));
            }
          } finally {
            if (activeSynthesisRef.current === requestId) activeSynthesisRef.current = null;
            synthesisCompletionsRef.current.delete(requestId);
          }
        }
        speechWorkerRef.current = false;
        if (!disposed) setVisualState(streamRef.current ? "listening" : "idle");
      })();
    };
    void listen<{ session_id: string; content: string; segments?: { kind: string; content: string }[] }>("agent://assistant_delta", (event) => {
      if (disposed || !sessionId || event.payload.session_id !== sessionId) return;
      const text = event.payload.segments
        ? event.payload.segments.filter((segment) => segment.kind === "text").map((segment) => segment.content).join("")
        : event.payload.content;
      speechBufferRef.current += text;
      const parts = speechBufferRef.current.split(/(?<=[。！？!?；;\n])/u);
      speechBufferRef.current = parts.pop() ?? "";
      for (const part of parts) enqueueSpeech(part);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenDelta = unlisten;
    });
    void listen<{ session_id: string }>("agent://run_finished", (event) => {
      if (disposed || !sessionId || event.payload.session_id !== sessionId) return;
      const remainder = speechBufferRef.current;
      speechBufferRef.current = "";
      enqueueSpeech(remainder);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenFinished = unlisten;
    });
    void listen<{
      session_id: string; id: string; sequence: number; audio_base64: string; sample_rate: number;
    }>("voice://tts_audio_chunk", (event) => {
      const chunk = event.payload;
      const requestId = chunk.id;
      const generation = playbackGenerationRef.current;
      if (disposed || !sessionId || chunk.session_id !== sessionId || activeSynthesisRef.current !== requestId || chunk.sequence !== nextAudioSequenceRef.current) return;
      void (async () => {
        try {
          let context = playbackContextRef.current;
          if (!context) {
            context = new AudioContext();
            playbackContextRef.current = context;
            scheduledUntilRef.current = context.currentTime;
          }
          if (context.state === "suspended") await context.resume();
          while (!speakerMutedRef.current && scheduledUntilRef.current - context.currentTime >= 0.35) {
            if (generation !== playbackGenerationRef.current || activeSynthesisRef.current !== requestId) return;
            await new Promise((resolve) => window.setTimeout(resolve, 20));
          }
          if (generation !== playbackGenerationRef.current || activeSynthesisRef.current !== requestId) return;
          if (!speakerMutedRef.current) {
            const samples = pcm16Base64ToFloat32(chunk.audio_base64);
            if (samples.length === 0 || chunk.sample_rate !== 22050) throw new Error("收到无效的 CosyVoice PCM 音频块");
            const buffer = context.createBuffer(1, samples.length, chunk.sample_rate);
            buffer.copyToChannel(samples, 0);
            const source = context.createBufferSource();
            const gain = context.createGain();
            source.buffer = buffer;
            gain.gain.value = speakerMutedRef.current ? 0 : 1;
            source.connect(gain);
            gain.connect(context.destination);
            const startAt = Math.max(context.currentTime + 0.015, scheduledUntilRef.current);
            scheduledUntilRef.current = startAt + buffer.duration;
            playbackNodesRef.current.add(source);
            playbackGainsRef.current.add(gain);
            source.onended = () => {
              playbackNodesRef.current.delete(source);
              playbackGainsRef.current.delete(gain);
              source.disconnect();
              gain.disconnect();
            };
            source.start(startAt);
            setVisualState("speaking");
          }
          await invoke("acknowledge_voice_synthesis_chunk", { requestId, sequence: chunk.sequence });
          nextAudioSequenceRef.current += 1;
        } catch (reason) {
          const message = reason instanceof Error ? reason.message : String(reason);
          synthesisCompletionsRef.current.get(requestId)?.reject(new Error(message));
          void invoke("cancel_voice_synthesis_stream", { requestId }).catch(() => undefined);
        }
      })();
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenAudioChunk = unlisten;
    });
    void listen<{ session_id: string; id: string; error?: string | null; complete: boolean }>("voice://tts_complete", (event) => {
      const result = event.payload;
      if (disposed || !sessionId || result.session_id !== sessionId || activeSynthesisRef.current !== result.id) return;
      const completion = synthesisCompletionsRef.current.get(result.id);
      if (!completion) return;
      if (result.error) completion.reject(new Error(result.error));
      else completion.resolve();
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenTtsComplete = unlisten;
    });
    return () => {
      disposed = true;
      unlistenDelta?.();
      unlistenFinished?.();
      unlistenAudioChunk?.();
      unlistenTtsComplete?.();
      speechQueueRef.current = [];
      speechBufferRef.current = "";
      cancelCurrentSpeech();
    };
  }, [cancelCurrentSpeech, sessionId]);

  useEffect(() => {
    speakerMutedRef.current = speakerMuted;
    for (const gain of playbackGainsRef.current) {
      const context = playbackContextRef.current;
      if (context && context.state !== "closed") {
        gain.gain.setTargetAtTime(speakerMuted ? 0 : 1, context.currentTime, 0.012);
      }
    }
  }, [speakerMuted]);

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
          const now = performance.now();
          const speechStartThreshold = 0.055;
          const speechEndThreshold = 0.05;
          const endpointSilenceMs = 850;
          const maxUtteranceMs = 20_000;
          if (rms >= speechStartThreshold) {
            if (utteranceStartedAtRef.current === null) {
              if (visualState === "speaking" || activeSynthesisRef.current) {
                cancelCurrentSpeech();
                setVisualState("listening");
              }
              utteranceStartedAtRef.current = now;
              utteranceChunksRef.current = [];
            }
            silenceSinceRef.current = null;
          } else if (utteranceStartedAtRef.current !== null) {
            if (rms >= speechEndThreshold) silenceSinceRef.current = null;
            else silenceSinceRef.current ??= now;
          }
          const utteranceStartedAt = utteranceStartedAtRef.current;
          const reachedSilenceEndpoint = silenceSinceRef.current !== null
            && now - silenceSinceRef.current >= endpointSilenceMs;
          const reachedMaximumDuration = utteranceStartedAt !== null
            && now - utteranceStartedAt >= maxUtteranceMs;
          if (reachedSilenceEndpoint || reachedMaximumDuration) {
            const recorder = recorderRef.current;
            if (recorder && recorder.state === "recording") recorder.stop();
            silenceSinceRef.current = null;
          }
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
  }, [cancelCurrentSpeech, muted, visualState]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      stopCapture();
    };
  }, [stopCapture]);

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
    if (nextMuted) {
      utteranceStartedAtRef.current = null;
      silenceSinceRef.current = null;
      utteranceChunksRef.current = [];
    }
    setMuted(nextMuted);
  };

  const status = visualState === "error" ? "麦克风不可用"
    : visualState === "speaking" ? "Agnes 正在回复"
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
            {(visualState === "listening" || visualState === "speaking") && !muted && (
              <div className="mt-1 text-[11px] text-white/45">麦克风音量 {Math.round(level * 100)}%</div>
            )}
          </div>
        </div>

        <div className="relative z-10 mb-3 w-full max-w-lg rounded-2xl border border-white/[0.08] bg-black/20 px-4 py-3 text-center backdrop-blur-sm">
          {error || processingError ? (
            <div role="alert" className="flex items-start justify-center gap-2 text-xs leading-5 text-rose-100">
              <Warning size={16} className="mt-0.5 shrink-0" aria-hidden="true" />
              <span>{error || processingError}</span>
            </div>
          ) : (
            <p className="text-xs leading-5 text-white/55">
              {visualState === "listening"
                ? recognizing
                  ? "正在识别这段语音…"
                  : transcript
                    ? `“${transcript}”${utteranceCount > 1 && lastUtteranceMs ? ` · 最近一段 ${(lastUtteranceMs / 1000).toFixed(1)} 秒` : ""}`
                    : utteranceCount > 0
                      ? `已检测到 ${utteranceCount} 段语音，等待识别结果。`
                      : "麦克风已连接。说完停顿约一秒，即可提交识别。"
                : "允许 Agnes 使用麦克风，查看实时音量与通话动态效果。"}
            </p>
          )}
        </div>

        <footer className="relative z-10 flex items-center justify-center gap-3 pb-[max(0.5rem,env(safe-area-inset-bottom))]">
          <label className="flex min-h-12 max-w-[min(52vw,260px)] items-center gap-2 rounded-full border border-white/15 bg-white/[0.06] px-3 text-xs text-white/65">
            <Microphone size={16} className="shrink-0" aria-hidden="true" />
            <span className="sr-only">输入设备</span>
            <select
              value={selectedAudioInput}
              onChange={(event) => {
                const deviceId = event.currentTarget.value;
                selectedAudioInputRef.current = deviceId;
                setSelectedAudioInput(deviceId);
                if (streamRef.current) {
                  stopCapture();
                  void startCapture(deviceId);
                }
              }}
              aria-label="麦克风输入设备"
              className="min-w-0 flex-1 cursor-pointer bg-transparent text-xs text-white outline-none focus-visible:ring-2 focus-visible:ring-[#d9a28f]"
            >
              <option value="" className="bg-[#1b1918] text-white">
                {audioInputs.length ? "系统默认输入" : "未检测到输入设备"}
              </option>
              {audioInputs.map((device, index) => (
                <option key={device.deviceId || `audio-input-${index}`} value={device.deviceId} className="bg-[#1b1918] text-white">
                  {device.label || `麦克风 ${index + 1}`}
                </option>
              ))}
            </select>
          </label>
          <button type="button" onClick={toggleMic} className={`flex min-h-12 min-w-12 items-center justify-center gap-2 rounded-full border px-4 transition-all focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f] ${muted ? "border-rose-300/30 bg-rose-400/15 text-rose-100" : "border-white/15 bg-white/[0.08] text-white hover:bg-white/[0.14]"}`} aria-label={streamRef.current ? muted ? "取消麦克风静音" : "静音麦克风" : "开启麦克风"} aria-pressed={muted}>
            {muted ? <MicrophoneSlash size={19} aria-hidden="true" /> : <Microphone size={19} aria-hidden="true" />}
            <span className="text-xs">{streamRef.current ? muted ? "取消静音" : "静音" : "开启麦克风"}</span>
          </button>
          <button type="button" onClick={() => setSpeakerMuted((value) => !value)} className={`flex min-h-12 min-w-12 items-center justify-center gap-2 rounded-full border px-4 transition-all focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f] ${speakerMuted ? "border-white/10 bg-white/[0.04] text-white/40" : "border-white/15 bg-white/[0.08] text-white hover:bg-white/[0.14]"}`} aria-label={speakerMuted ? "取消扬声器静音" : "静音扬声器"} aria-pressed={speakerMuted}>
            {speakerMuted ? <SpeakerSlash size={19} aria-hidden="true" /> : <SpeakerHigh size={19} aria-hidden="true" />}
            <span className="text-xs">扬声器</span>
          </button>
        </footer>
      </div>
    </section>
  );
}
