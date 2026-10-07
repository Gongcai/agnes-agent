import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowLeft, Microphone, MicrophoneSlash, PhoneDisconnect, SpeakerHigh, SpeakerSlash, Square, Warning } from "@phosphor-icons/react";
import { splitSpeechText } from "../lib/voiceSpeech";

type VoiceVisualState = "idle" | "connecting" | "listening" | "recognizing" | "thinking" | "speaking" | "stopping" | "error";

interface VoiceCallPanelProps {
  agentName: string;
  sessionName?: string;
  sessionId?: string | null;
  onClose: () => void;
  onTranscript: (text: string, signal: AbortSignal, runId: string) => Promise<void> | void;
  onInterrupt: (sessionId: string, runId: string) => Promise<void>;
}

const PLAYBACK_BUFFER_SECONDS = 0.8;
const MAX_QUEUED_SPEECH_CHARS = 12_000;

async function toMonoWav16k(input: Float32Array<ArrayBuffer>, inputSampleRate: number): Promise<Blob> {
  const outputLength = Math.max(1, Math.ceil(input.length * 16000 / inputSampleRate));
  const offline = new OfflineAudioContext(1, outputLength, 16000);
  const sourceBuffer = offline.createBuffer(1, input.length, inputSampleRate);
  sourceBuffer.copyToChannel(input, 0);
  const source = offline.createBufferSource();
  source.buffer = sourceBuffer;
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
  writeText(12, "fmt ");
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

export function VoiceCallPanel({ agentName, sessionName, sessionId, onClose, onTranscript, onInterrupt }: VoiceCallPanelProps) {
  const sessionIdRef = useRef(sessionId ?? null);
  // Draft promotion changes the binding without restarting the call's listeners.
  sessionIdRef.current = sessionId ?? null;
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const contextRef = useRef<AudioContext | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const playbackAnalyserRef = useRef<AnalyserNode | null>(null);
  const captureProcessorRef = useRef<ScriptProcessorNode | null>(null);
  const captureSilenceGainRef = useRef<GainNode | null>(null);
  const preRollFramesRef = useRef<Float32Array[]>([]);
  const preRollSamplesRef = useRef(0);
  const utteranceFramesRef = useRef<Float32Array[]>([]);
  const utteranceStartedAtRef = useRef<number | null>(null);
  const silenceSinceRef = useRef<number | null>(null);
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
  const [canInterrupt, setCanInterrupt] = useState(false);
  const [transcript, setTranscript] = useState<string | null>(null);
  const [processingError, setProcessingError] = useState<string | null>(null);
  const onTranscriptRef = useRef(onTranscript);
  const onInterruptRef = useRef(onInterrupt);
  const submissionAbortRef = useRef<AbortController | null>(null);
  const submissionCompletionRef = useRef<Promise<void> | null>(null);
  const mountedRef = useRef(true);
  const captureGenerationRef = useRef(0);
  const recognitionQueueRef = useRef(Promise.resolve());
  const speechBufferRef = useRef("");
  const speechQueueRef = useRef<string[]>([]);
  const speechWorkerRef = useRef<number | null>(null);
  const speechFlushTimerRef = useRef<number | null>(null);
  const replyPendingRef = useRef(false);
  const acceptingReplyRef = useRef(false);
  const activeRunIdRef = useRef<string | null>(null);
  const recognitionPendingRef = useRef(false);
  const interruptionPendingRef = useRef(false);
  const playbackGenerationRef = useRef(0);
  const playbackContextRef = useRef<AudioContext | null>(null);
  const scheduledUntilRef = useRef(0);
  const playbackNodesRef = useRef(new Set<AudioBufferSourceNode>());
  const playbackGainsRef = useRef(new Set<GainNode>());
  const activeSynthesisRef = useRef<string | null>(null);
  const captureSuppressedRef = useRef(false);
  const captureResumeTimerRef = useRef<number | null>(null);
  const audioChunkChainRef = useRef(Promise.resolve());
  const nextAudioSequenceRef = useRef(0);
  const synthesisCompletionsRef = useRef(new Map<string, { resolve: () => void; reject: (error: Error) => void }>());
  onTranscriptRef.current = onTranscript;
  onInterruptRef.current = onInterrupt;
  const reducedMotion = useRef(false);

  const clearCaptureBuffers = useCallback(() => {
    utteranceStartedAtRef.current = null;
    silenceSinceRef.current = null;
    preRollFramesRef.current = [];
    preRollSamplesRef.current = 0;
    utteranceFramesRef.current = [];
  }, []);

  const suppressCapture = useCallback(() => {
    captureSuppressedRef.current = true;
    if (captureResumeTimerRef.current !== null) {
      window.clearTimeout(captureResumeTimerRef.current);
      captureResumeTimerRef.current = null;
    }
    clearCaptureBuffers();
  }, [clearCaptureBuffers]);

  const scheduleCaptureResume = useCallback((delayMs = 220) => {
    if (captureResumeTimerRef.current !== null) window.clearTimeout(captureResumeTimerRef.current);
    captureResumeTimerRef.current = window.setTimeout(() => {
      captureResumeTimerRef.current = null;
      if (!mountedRef.current || replyPendingRef.current || recognitionPendingRef.current
        || interruptionPendingRef.current || speechWorkerRef.current !== null) return;
      clearCaptureBuffers();
      captureSuppressedRef.current = false;
      setVisualState(streamRef.current ? "listening" : "idle");
      setCanInterrupt(false);
    }, delayMs);
  }, [clearCaptureBuffers]);

  const finishReplyPlayback = useCallback(() => {
    if (!mountedRef.current || interruptionPendingRef.current || speechWorkerRef.current !== null) return;
    if (replyPendingRef.current) {
      setVisualState("thinking");
      return;
    }
    scheduleCaptureResume();
  }, [scheduleCaptureResume]);

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
    speechWorkerRef.current = null;
    if (speechFlushTimerRef.current !== null) window.clearTimeout(speechFlushTimerRef.current);
    speechFlushTimerRef.current = null;
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
    for (const gain of playbackGainsRef.current) gain.disconnect();
    playbackGainsRef.current.clear();
    const context = playbackContextRef.current;
    if (context && context.state !== "closed") {
      scheduledUntilRef.current = context.currentTime;
    }
  }, []);

  const interruptReply = useCallback(async () => {
    if (interruptionPendingRef.current) return;
    const targetSessionId = sessionIdRef.current;
    const targetRunId = activeRunIdRef.current;
    const cancelAgent = replyPendingRef.current;
    const pendingSubmission = submissionCompletionRef.current;
    interruptionPendingRef.current = true;
    acceptingReplyRef.current = false;
    replyPendingRef.current = false;
    submissionAbortRef.current?.abort();
    recognitionPendingRef.current = false;
    cancelCurrentSpeech();
    suppressCapture();
    setCanInterrupt(false);
    setVisualState("stopping");
    try {
      if (cancelAgent && targetSessionId && targetRunId) await onInterruptRef.current(targetSessionId, targetRunId);
    } catch (reason) {
      if (mountedRef.current) setProcessingError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      // A pending native send cancels its own run after returning. Wait for
      // that cleanup before allowing another transcript in this call.
      if (pendingSubmission) await pendingSubmission.catch(() => undefined);
      interruptionPendingRef.current = false;
      if (mountedRef.current) scheduleCaptureResume();
    }
  }, [cancelCurrentSpeech, scheduleCaptureResume, suppressCapture]);

  const processUtterance = useCallback((samples: Float32Array<ArrayBuffer>, sampleRate: number, generation: number) => {
    if (samples.length === 0) {
      setProcessingError("没有采集到这段语音的音频数据，请重新说一次。");
      return;
    }
    setUtteranceCount((count) => count + 1);
    setLastUtteranceMs(samples.length / sampleRate * 1000);
    const submission = new AbortController();
    submissionAbortRef.current = submission;
    acceptingReplyRef.current = false;
    activeRunIdRef.current = null;
    suppressCapture();
    recognitionPendingRef.current = true;
    setCanInterrupt(true);
    setVisualState("recognizing");
    setProcessingError(null);
    recognitionQueueRef.current = recognitionQueueRef.current.then(async () => {
      if (!mountedRef.current || submission.signal.aborted || generation !== captureGenerationRef.current) return;
      try {
        const wav = await toMonoWav16k(samples, sampleRate);
        if (!mountedRef.current || submission.signal.aborted || generation !== captureGenerationRef.current) return;
        const text = await invoke<string>("transcribe_voice", {
          sessionId: sessionIdRef.current,
          audioBase64: await blobToBase64(wav),
          mimeType: "audio/wav",
          sampleRate: 16000,
        });
        if (!mountedRef.current || submission.signal.aborted || generation !== captureGenerationRef.current || !text.trim()) return;
        setTranscript(text.trim());
        acceptingReplyRef.current = true;
        const runId = crypto.randomUUID();
        activeRunIdRef.current = runId;
        replyPendingRef.current = true;
        setVisualState("thinking");
        const submitted = Promise.resolve(onTranscriptRef.current(text.trim(), submission.signal, runId));
        submissionCompletionRef.current = submitted;
        try {
          await submitted;
        } finally {
          if (submissionCompletionRef.current === submitted) submissionCompletionRef.current = null;
        }
      } catch (reason) {
        if (mountedRef.current && !submission.signal.aborted && generation === captureGenerationRef.current) {
          replyPendingRef.current = false;
          setProcessingError(reason instanceof Error ? reason.message : String(reason));
        }
      } finally {
        if (mountedRef.current && !submission.signal.aborted && generation === captureGenerationRef.current) {
          recognitionPendingRef.current = false;
          finishReplyPlayback();
        }
      }
    });
  }, [finishReplyPlayback, suppressCapture]);

  const stopCapture = useCallback(() => {
    captureGenerationRef.current += 1;
    submissionAbortRef.current?.abort();
    recognitionQueueRef.current = Promise.resolve();
    cancelCurrentSpeech();
    acceptingReplyRef.current = false;
    replyPendingRef.current = false;
    recognitionPendingRef.current = false;
    setCanInterrupt(false);
    if (captureResumeTimerRef.current !== null) window.clearTimeout(captureResumeTimerRef.current);
    captureResumeTimerRef.current = null;
    captureSuppressedRef.current = false;
    stoppingRef.current = true;
    utteranceStartedAtRef.current = null;
    silenceSinceRef.current = null;
    preRollFramesRef.current = [];
    preRollSamplesRef.current = 0;
    utteranceFramesRef.current = [];
    captureProcessorRef.current?.disconnect();
    captureProcessorRef.current = null;
    captureSilenceGainRef.current?.disconnect();
    captureSilenceGainRef.current = null;
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    analyserRef.current = null;
    playbackAnalyserRef.current = null;
    const context = contextRef.current;
    contextRef.current = null;
    playbackContextRef.current = null;
    if (context && context.state !== "closed") void context.close();
    setVisualState("idle");
    setLevel(0);
  }, [cancelCurrentSpeech]);

  const startCapture = useCallback(async (deviceId?: string) => {
    setError(null);
    setVisualState("connecting");
    if (!navigator.mediaDevices?.getUserMedia) {
      setError("此环境无法访问麦克风。请检查系统权限或使用桌面版。");
      setVisualState("error");
      return;
    }
    const captureGeneration = captureGenerationRef.current;
    let pendingStream: MediaStream | null = null;
    let pendingContext: AudioContext | null = null;
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
      pendingStream = stream;
      if (!mountedRef.current || captureGeneration !== captureGenerationRef.current) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      const context = new AudioContext();
      pendingContext = context;
      if (context.state === "suspended") await context.resume();
      if (!mountedRef.current || captureGeneration !== captureGenerationRef.current) {
        stream.getTracks().forEach((track) => track.stop());
        void context.close();
        return;
      }
      preRollFramesRef.current = [];
      preRollSamplesRef.current = 0;
      utteranceFramesRef.current = [];
      utteranceStartedAtRef.current = null;
      silenceSinceRef.current = null;
      const analyser = context.createAnalyser();
      analyser.fftSize = 512;
      analyser.smoothingTimeConstant = 0.78;
      const source = context.createMediaStreamSource(stream);
      source.connect(analyser);
      const processor = context.createScriptProcessor(1024, 1, 1);
      const silenceGain = context.createGain();
      silenceGain.gain.value = 0;
      processor.onaudioprocess = (event) => {
        if (captureGeneration !== captureGenerationRef.current) return;
        if (captureSuppressedRef.current) return;
        const frame = new Float32Array(event.inputBuffer.getChannelData(0));
        if (utteranceStartedAtRef.current !== null) {
          utteranceFramesRef.current.push(frame);
          return;
        }
        preRollFramesRef.current.push(frame);
        preRollSamplesRef.current += frame.length;
        const maxPreRollSamples = Math.ceil(context.sampleRate * 0.4);
        while (preRollSamplesRef.current > maxPreRollSamples && preRollFramesRef.current.length > 0) {
          const removed = preRollFramesRef.current.shift();
          if (removed) preRollSamplesRef.current -= removed.length;
        }
      };
      source.connect(processor);
      processor.connect(silenceGain);
      silenceGain.connect(context.destination);
      const playbackAnalyser = context.createAnalyser();
      playbackAnalyser.fftSize = 512;
      playbackAnalyser.connect(context.destination);
      streamRef.current = stream;
      contextRef.current = context;
      playbackContextRef.current = context;
      playbackAnalyserRef.current = playbackAnalyser;
      scheduledUntilRef.current = context.currentTime;
      analyserRef.current = analyser;
      captureProcessorRef.current = processor;
      captureSilenceGainRef.current = silenceGain;
      stoppingRef.current = false;
      acceptingReplyRef.current = false;
      pendingStream = null;
      pendingContext = null;
      setUtteranceCount(0);
      setLastUtteranceMs(null);
      setMuted(false);
      setVisualState("listening");
      void refreshAudioInputs();
    } catch (reason) {
      pendingStream?.getTracks().forEach((track) => track.stop());
      if (pendingContext && pendingContext.state !== "closed") void pendingContext.close().catch(() => undefined);
      if (!mountedRef.current || captureGeneration !== captureGenerationRef.current) return;
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
  }, [refreshAudioInputs]);

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
    let unlistenStarted: (() => void) | undefined;
    let unlistenDelta: (() => void) | undefined;
    let unlistenFinished: (() => void) | undefined;
    let unlistenError: (() => void) | undefined;
    let unlistenAudioChunk: (() => void) | undefined;
    let unlistenTtsComplete: (() => void) | undefined;
    const enqueueSpeech = (text: string) => {
      const clean = text.trim();
      if (!clean) {
        finishReplyPlayback();
        return;
      }
      const queuedChars = speechQueueRef.current.reduce((total, segment) => total + segment.length, 0);
      if (queuedChars + clean.length > MAX_QUEUED_SPEECH_CHARS) {
        setProcessingError("回复过长，已停止播报。你可以继续说话，完整文本可在聊天中查看。");
        void interruptReply();
        return;
      }
      speechQueueRef.current.push(clean);
      const generation = playbackGenerationRef.current;
      if (speechWorkerRef.current === generation) return;
      speechWorkerRef.current = generation;
      setCanInterrupt(true);
      void (async () => {
        suppressCapture();
        while (!disposed && generation === playbackGenerationRef.current) {
          const next = speechQueueRef.current.shift();
          if (!next) {
            const context = playbackContextRef.current;
            if (context && context.currentTime < scheduledUntilRef.current) {
              await new Promise((resolve) => window.setTimeout(resolve, 25));
              continue;
            }
            break;
          }
          const requestId = crypto.randomUUID();
          try {
            if (playbackNodesRef.current.size === 0) setVisualState("thinking");
            activeSynthesisRef.current = requestId;
            nextAudioSequenceRef.current = 0;
            const completed = new Promise<void>((resolve, reject) => {
              synthesisCompletionsRef.current.set(requestId, { resolve, reject });
            });
            // Cancellation can arrive while the start command is still pending.
            void completed.catch(() => undefined);
            await invoke<string>("start_voice_synthesis_stream", {
              sessionId: sessionIdRef.current,
              text: next,
              requestId,
            });
            if (disposed || generation !== playbackGenerationRef.current) {
              // The earlier cancel may have arrived before native preparation
              // queued this request. Cancel again after start is confirmed.
              void invoke("cancel_voice_synthesis_stream", { requestId }).catch(() => undefined);
              break;
            }
            await completed;
            if (disposed || generation !== playbackGenerationRef.current) break;
            // Synthesize the next sentence while this sentence's buffered tail
            // is still playing. Drain only after the text queue is empty.
          } catch (reason) {
            if (!disposed && generation === playbackGenerationRef.current) {
              setProcessingError(reason instanceof Error ? reason.message : String(reason));
            }
          } finally {
            if (activeSynthesisRef.current === requestId) activeSynthesisRef.current = null;
            synthesisCompletionsRef.current.delete(requestId);
          }
        }
        if (speechWorkerRef.current === generation) {
          speechWorkerRef.current = null;
          if (!disposed) finishReplyPlayback();
        }
      })();
    };

    const flushSpeechBuffer = (flush = false) => {
      if (speechFlushTimerRef.current !== null) window.clearTimeout(speechFlushTimerRef.current);
      speechFlushTimerRef.current = null;
      const { segments, remainder } = splitSpeechText(speechBufferRef.current, flush);
      speechBufferRef.current = remainder;
      for (const segment of segments) {
        if (!acceptingReplyRef.current) break;
        enqueueSpeech(segment);
      }
      if (remainder && acceptingReplyRef.current) {
        speechFlushTimerRef.current = window.setTimeout(() => {
          speechFlushTimerRef.current = null;
          if (!disposed && acceptingReplyRef.current) flushSpeechBuffer(true);
        }, 900);
      }
    };
    const isCurrentRun = (payload: { session_id: string; run_id: string }) => !disposed
      && acceptingReplyRef.current && payload.session_id === sessionIdRef.current
      && payload.run_id === activeRunIdRef.current;

    void listen<{ session_id: string; run_id: string }>("agent://run_started", (event) => {
      if (!isCurrentRun(event.payload)) return;
      replyPendingRef.current = true;
      suppressCapture();
      setCanInterrupt(true);
      setVisualState("thinking");
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenStarted = unlisten;
    });
    void listen<{ session_id: string; run_id: string; content: string; segments?: { kind: string; content: string }[] }>("agent://assistant_delta", (event) => {
      if (!isCurrentRun(event.payload)) return;
      const text = event.payload.segments
        ? event.payload.segments.filter((segment) => segment.kind === "text").map((segment) => segment.content).join("")
        : event.payload.content;
      speechBufferRef.current += text;
      if (speechFlushTimerRef.current === null || /[。！？!?；;\n]/u.test(text) || speechBufferRef.current.length >= 180) {
        flushSpeechBuffer();
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenDelta = unlisten;
    });
    void listen<{ session_id: string; run_id: string }>("agent://run_finished", (event) => {
      if (!isCurrentRun(event.payload)) return;
      replyPendingRef.current = false;
      flushSpeechBuffer(true);
      acceptingReplyRef.current = false;
      finishReplyPlayback();
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenFinished = unlisten;
    });
    void listen<{ session_id: string; run_id: string; message: string }>("agent://run_error", (event) => {
      if (!isCurrentRun(event.payload)) return;
      replyPendingRef.current = false;
      acceptingReplyRef.current = false;
      cancelCurrentSpeech();
      if (event.payload.message !== "已取消") setProcessingError(event.payload.message);
      finishReplyPlayback();
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenError = unlisten;
    });
    void listen<{
      session_id: string; id: string; sequence: number; audio_base64: string; sample_rate: number;
    }>("voice://tts_audio_chunk", (event) => {
      const chunk = event.payload;
      const requestId = chunk.id;
      const generation = playbackGenerationRef.current;
      if (disposed || !sessionIdRef.current || chunk.session_id !== sessionIdRef.current || activeSynthesisRef.current !== requestId || chunk.sequence !== nextAudioSequenceRef.current) return;
      // Tauri event callbacks are not awaited by the event dispatcher. Keep
      // chunk rendering ordered so a fast ACK cannot race the next event.
      audioChunkChainRef.current = audioChunkChainRef.current.then(async () => {
        try {
          if (disposed || generation !== playbackGenerationRef.current || activeSynthesisRef.current !== requestId || chunk.sequence !== nextAudioSequenceRef.current) return;
          let context = playbackContextRef.current;
          if (!context) {
            context = new AudioContext();
            playbackContextRef.current = context;
            scheduledUntilRef.current = context.currentTime;
            const analyser = context.createAnalyser();
            analyser.fftSize = 512;
            analyser.connect(context.destination);
            playbackAnalyserRef.current = analyser;
          }
          if (context.state === "suspended") await context.resume();
          while (!speakerMutedRef.current && scheduledUntilRef.current - context.currentTime >= PLAYBACK_BUFFER_SECONDS) {
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
            gain.connect(playbackAnalyserRef.current ?? context.destination);
            const startAt = Math.max(context.currentTime + 0.08, scheduledUntilRef.current);
            scheduledUntilRef.current = startAt + buffer.duration;
            playbackNodesRef.current.add(source);
            playbackGainsRef.current.add(gain);
            source.onended = () => {
              playbackNodesRef.current.delete(source);
              playbackGainsRef.current.delete(gain);
              source.disconnect();
              gain.disconnect();
              if (!disposed && generation === playbackGenerationRef.current && playbackNodesRef.current.size === 0
                && (replyPendingRef.current || activeSynthesisRef.current || speechQueueRef.current.length > 0)) {
                setVisualState("thinking");
              }
            };
            source.start(startAt);
            setVisualState("speaking");
          }
          // Advance before ACK: the sidecar may send the next chunk as soon
          // as it receives the ACK.
          nextAudioSequenceRef.current += 1;
          await invoke("acknowledge_voice_synthesis_chunk", { requestId, sequence: chunk.sequence });
        } catch (reason) {
          const message = reason instanceof Error ? reason.message : String(reason);
          synthesisCompletionsRef.current.get(requestId)?.reject(new Error(message));
          void invoke("cancel_voice_synthesis_stream", { requestId }).catch(() => undefined);
        }
      }).catch((reason) => {
        const message = reason instanceof Error ? reason.message : String(reason);
        synthesisCompletionsRef.current.get(requestId)?.reject(new Error(message));
        void invoke("cancel_voice_synthesis_stream", { requestId }).catch(() => undefined);
      });
    }).then((unlisten) => {
      if (disposed) unlisten();
      else unlistenAudioChunk = unlisten;
    });
    void listen<{ session_id: string; id: string; error?: string | null; complete: boolean }>("voice://tts_complete", (event) => {
      const result = event.payload;
      if (disposed || !sessionIdRef.current || result.session_id !== sessionIdRef.current || activeSynthesisRef.current !== result.id) return;
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
      unlistenStarted?.();
      unlistenDelta?.();
      unlistenFinished?.();
      unlistenError?.();
      unlistenAudioChunk?.();
      unlistenTtsComplete?.();
      speechQueueRef.current = [];
      speechBufferRef.current = "";
      cancelCurrentSpeech();
      if (captureResumeTimerRef.current !== null) window.clearTimeout(captureResumeTimerRef.current);
      captureResumeTimerRef.current = null;
      captureSuppressedRef.current = false;
    };
  }, [cancelCurrentSpeech, finishReplyPlayback, interruptReply, suppressCapture]);

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
        if (analyser && !muted && !captureSuppressedRef.current) {
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
              utteranceStartedAtRef.current = now;
              utteranceFramesRef.current = preRollFramesRef.current;
              preRollFramesRef.current = [];
              preRollSamplesRef.current = 0;
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
            const frames = utteranceFramesRef.current;
            const sampleCount = frames.reduce((total, frame) => total + frame.length, 0);
            const samples = new Float32Array(sampleCount);
            let offset = 0;
            for (const frame of frames) {
              samples.set(frame, offset);
              offset += frame.length;
            }
            const sampleRate = contextRef.current?.sampleRate ?? 48000;
            utteranceFramesRef.current = [];
            utteranceStartedAtRef.current = null;
            silenceSinceRef.current = null;
            processUtterance(samples, sampleRate, captureGenerationRef.current);
          }
        }
        const playbackAnalyser = playbackAnalyserRef.current;
        if (visualState === "speaking" && playbackAnalyser && !speakerMutedRef.current) {
          const samples = new Uint8Array(playbackAnalyser.fftSize);
          playbackAnalyser.getByteTimeDomainData(samples);
          let sum = 0;
          for (const sample of samples) {
            const value = (sample - 128) / 128;
            sum += value * value;
          }
          rms = Math.min(1, Math.sqrt(sum / samples.length) * 3.4);
        }
        if (time - lastMeterUpdate > 45) {
          lastMeterUpdate = time;
          setLevel(rms);
        }
        const cx = bounds.width / 2;
        const cy = bounds.height / 2;
        const base = Math.min(bounds.width, bounds.height) * 0.285;
        const pending = visualState === "recognizing" || visualState === "thinking" || visualState === "connecting";
        const quietPulse = reducedMotion.current ? 0
          : pending ? Math.sin(time / 640) * 0.025
            : visualState === "listening" ? Math.sin(time / 1050) * 0.014 : 0;
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
        context.rotate(reducedMotion.current ? 0 : pending ? time / 6500
          : visualState === "listening" ? Math.sin(time / 4300) * 0.018 : 0);
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
  }, [muted, processUtterance, visualState]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const target = event.target;
      if (event.key !== "Escape" || event.repeat || event.defaultPrevented || !canInterrupt
        || target instanceof HTMLElement && target.closest('dialog, [role="dialog"]')) return;
      event.preventDefault();
      void interruptReply();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [canInterrupt, interruptReply]);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      const targetSessionId = sessionIdRef.current;
      const targetRunId = activeRunIdRef.current;
      if (replyPendingRef.current && targetSessionId && targetRunId) {
        void onInterruptRef.current(targetSessionId, targetRunId).catch((reason) => console.warn("Unable to cancel the voice reply", reason));
      }
      stopCapture();
    };
  }, [stopCapture]);

  const close = () => {
    const targetSessionId = sessionIdRef.current;
    const targetRunId = activeRunIdRef.current;
    if (replyPendingRef.current && targetSessionId && targetRunId) {
      void onInterruptRef.current(targetSessionId, targetRunId).catch((reason) => console.warn("Unable to cancel the voice reply", reason));
    }
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
      preRollFramesRef.current = [];
      preRollSamplesRef.current = 0;
      utteranceFramesRef.current = [];
    }
    setMuted(nextMuted);
  };

  const status = visualState === "error" ? "麦克风不可用"
    : visualState === "connecting" ? "正在连接麦克风"
      : visualState === "recognizing" ? "正在识别语音"
        : visualState === "thinking" ? "Agnes 正在思考"
          : visualState === "stopping" ? "正在打断回复"
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
            {(visualState === "listening" && !muted || visualState === "speaking") && (
              <div className="mt-1 text-xs text-white/65">{visualState === "speaking" ? "播放音量" : "麦克风音量"} {Math.round(level * 100)}%</div>
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
            <div className="space-y-1.5 text-xs leading-5 text-white/70">
              {transcript && <p className="max-h-20 overflow-y-auto break-words">“{transcript}”</p>}
              <p>{visualState === "recognizing" ? "正在识别这段语音，可以点击打断重新说。"
                : visualState === "thinking" ? "正在准备回复，你可以点击打断。"
                  : visualState === "speaking" ? "点击“打断回复”或按 Esc，即可继续说话。"
                    : visualState === "stopping" ? "正在停止回复，随后恢复聆听。"
                      : visualState === "listening"
                        ? utteranceCount > 0 && lastUtteranceMs
                          ? `我在听 · 最近一段 ${(lastUtteranceMs / 1000).toFixed(1)} 秒`
                          : "说完停顿约一秒，即可提交识别。"
                        : "开启麦克风，开始语音对话。"}</p>
            </div>
          )}
        </div>

        <footer className="relative z-10 flex w-full max-w-lg flex-wrap items-center justify-center gap-3 pb-[max(0.5rem,env(safe-area-inset-bottom))]">
          <button type="button" onClick={() => void interruptReply()} disabled={!canInterrupt} aria-label="打断当前回复并继续说话" aria-keyshortcuts="Escape" title="打断回复（Esc）" className="flex min-h-12 items-center justify-center gap-2 rounded-full border border-[#d9a28f]/45 bg-[#d9a28f]/15 px-5 text-sm text-[#f5d7c8] transition-colors hover:bg-[#d9a28f]/25 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f] disabled:cursor-not-allowed disabled:border-white/10 disabled:bg-white/[0.03] disabled:text-white/35">
            <Square size={17} weight="fill" aria-hidden="true" />
            <span>{visualState === "stopping" ? "正在打断…" : "打断回复"}</span>
          </button>
          <label className="flex min-h-12 max-w-[min(52vw,260px)] items-center gap-2 rounded-full border border-white/15 bg-white/[0.06] px-3 text-xs text-white/65">
            <Microphone size={16} className="shrink-0" aria-hidden="true" />
            <span className="sr-only">输入设备</span>
            <select
              value={selectedAudioInput}
              disabled={visualState !== "idle" && visualState !== "listening" && visualState !== "error"}
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
              className="min-w-0 flex-1 cursor-pointer bg-transparent text-xs text-white outline-none focus-visible:ring-2 focus-visible:ring-[#d9a28f] disabled:cursor-not-allowed disabled:opacity-50"
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
          <button type="button" onClick={toggleMic} disabled={visualState === "connecting"} className={`flex min-h-12 min-w-12 items-center justify-center gap-2 rounded-full border px-4 transition-all focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[#d9a28f] disabled:cursor-not-allowed disabled:opacity-50 ${muted ? "border-rose-300/30 bg-rose-400/15 text-rose-100" : "border-white/15 bg-white/[0.08] text-white hover:bg-white/[0.14]"}`} aria-label={streamRef.current ? muted ? "取消麦克风静音" : "静音麦克风" : "开启麦克风"} aria-pressed={muted}>
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
