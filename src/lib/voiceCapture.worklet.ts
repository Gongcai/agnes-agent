import { VoiceVad } from "./voiceVad";

declare const sampleRate: number;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(name: string, processor: typeof AudioWorkletProcessor): void;

class AgnesVoiceCaptureProcessor extends AudioWorkletProcessor {
  private epoch = 0;
  private stopped = false;
  private readonly vad = new VoiceVad(sampleRate, (event) => {
    this.port.postMessage({ ...event, epoch: this.epoch }, event.type === "utterance" ? [event.samples.buffer] : []);
  });

  constructor() {
    super();
    this.port.onmessage = (event: MessageEvent) => {
      const command = event.data;
      if (command?.type === "stop") {
        this.stopped = true;
        this.vad.setEnabled(false);
      } else if (command?.type === "gate" && typeof command.enabled === "boolean" && Number.isSafeInteger(command.epoch)) {
        this.epoch = command.epoch;
        this.vad.setEnabled(command.enabled);
      }
    };
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    for (const output of outputs) for (const channel of output) channel.fill(0);
    if (this.stopped) return false;
    const input = inputs[0]?.[0];
    if (input) this.vad.process(input);
    return true;
  }
}

registerProcessor("agnes-voice-capture", AgnesVoiceCaptureProcessor);
