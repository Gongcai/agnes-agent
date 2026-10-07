import captureWorkletUrl from "./voiceCapture.worklet.ts?worker&url";
import { withVoiceDeadline } from "./voiceDeadline";
import { VoiceVad, type VoiceCaptureEvent } from "./voiceVad";

export interface VoiceCapture {
  readonly backend: "audio-worklet" | "script-processor";
  setEnabled(enabled: boolean): void;
  close(): void;
}

interface CaptureCallbacks {
  signal: AbortSignal;
  onLevel: (level: number) => void;
  onUtterance: (samples: Float32Array<ArrayBuffer>, sampleRate: number) => void;
  onError: () => void;
}

/** Prefer the audio rendering thread; keep sample-based endpointing in fallback. */
export async function createVoiceCapture(context: AudioContext, stream: MediaStream, callbacks: CaptureCallbacks): Promise<VoiceCapture> {
  const source = context.createMediaStreamSource(stream);
  const silentGain = context.createGain();
  silentGain.gain.value = 0;
  let node: AudioWorkletNode | ScriptProcessorNode | null = null;
  let vad: VoiceVad | null = null;
  let epoch = 0;
  let enabled = false;
  let closed = false;
  let backend: VoiceCapture["backend"] = "script-processor";

  const setEnabled = (nextEnabled: boolean) => {
    if (closed) return;
    epoch += 1;
    enabled = nextEnabled;
    callbacks.onLevel(0);
    if (backend === "audio-worklet") {
      (node as AudioWorkletNode).port.postMessage({ type: "gate", epoch, enabled });
    } else {
      vad?.setEnabled(enabled);
    }
  };
  const receive = (event: VoiceCaptureEvent & { epoch: number }) => {
    if (closed || !enabled || callbacks.signal.aborted || event.epoch !== epoch) return;
    if (event.type === "level") {
      callbacks.onLevel(event.level);
    } else if (event.type === "utterance") {
      setEnabled(false);
      callbacks.onUtterance(event.samples, event.sampleRate);
    }
  };
  const close = () => {
    if (closed) return;
    setEnabled(false);
    closed = true;
    source.disconnect();
    silentGain.disconnect();
    if (backend === "audio-worklet" && node) {
      const worklet = node as AudioWorkletNode;
      worklet.onprocessorerror = null;
      worklet.port.onmessage = null;
      worklet.port.postMessage({ type: "stop" });
      worklet.port.close();
    } else if (node) {
      (node as ScriptProcessorNode).onaudioprocess = null;
    }
    node?.disconnect();
    vad = null;
  };

  try {
    if (context.audioWorklet && typeof AudioWorkletNode !== "undefined") {
      try {
        await withVoiceDeadline(context.audioWorklet.addModule(captureWorkletUrl), {
          timeoutMs: 3000,
          message: "音频处理模块加载超时",
          signal: callbacks.signal,
        });
        if (callbacks.signal.aborted) throw new DOMException("Voice capture cancelled", "AbortError");
        node = new AudioWorkletNode(context, "agnes-voice-capture", {
          numberOfInputs: 1,
          numberOfOutputs: 1,
          outputChannelCount: [1],
          channelCount: 1,
          channelCountMode: "explicit",
        });
        backend = "audio-worklet";
        node.port.onmessage = (event: MessageEvent<VoiceCaptureEvent & { epoch: number }>) => receive(event.data);
        node.onprocessorerror = () => {
          close();
          callbacks.onError();
        };
      } catch (reason) {
        if (callbacks.signal.aborted || context.state === "closed") throw reason;
        console.warn("AudioWorklet unavailable; using sample-clock capture fallback", reason);
      }
    }
    if (!node) {
      const processor = context.createScriptProcessor(1024, 1, 1);
      vad = new VoiceVad(context.sampleRate, (event) => receive({ ...event, epoch }));
      processor.onaudioprocess = (event) => {
        event.outputBuffer.getChannelData(0).fill(0);
        vad?.process(event.inputBuffer.getChannelData(0));
      };
      node = processor;
    }
    if (callbacks.signal.aborted) throw new DOMException("Voice capture cancelled", "AbortError");
    source.connect(node);
    node.connect(silentGain);
    silentGain.connect(context.destination);
    return { backend, setEnabled, close };
  } catch (reason) {
    close();
    throw reason;
  }
}
