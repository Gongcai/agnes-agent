export type VoiceCaptureEvent =
  | { type: "level"; level: number }
  | { type: "utterance"; samples: Float32Array<ArrayBuffer>; sampleRate: number; reason: "silence" | "limit" };

const PRE_ROLL_MS = 400;
const SPEECH_ONSET_MS = 100;
const ONSET_GAP_MS = 50;
const MIN_VOICED_MS = 120;
const ENDPOINT_SILENCE_MS = 850;
const MAX_UTTERANCE_MS = 20_000;
const METER_INTERVAL_MS = 45;
const START_RMS = 0.055 / 3.4;
const END_RMS = 0.05 / 3.4;

/** Sample-clock VAD with fixed buffers, shared by worklet and legacy capture. */
export class VoiceVad {
  private readonly preRoll: Float32Array<ArrayBuffer>;
  private readonly utterance: Float32Array<ArrayBuffer>;
  private readonly onsetSamples: number;
  private readonly onsetGapSamples: number;
  private readonly minimumVoicedSamples: number;
  private readonly endpointSamples: number;
  private readonly meterIntervalSamples: number;
  private preRollOffset = 0;
  private preRollCount = 0;
  private utteranceCount = 0;
  private onsetCount = 0;
  private onsetSilenceCount = 0;
  private voicedCount = 0;
  private silenceCount = 0;
  private meterCount = 0;
  private meterEnergy = 0;
  private active = false;
  private enabled = false;

  constructor(private readonly sampleRate: number, private readonly emit: (event: VoiceCaptureEvent) => void) {
    if (!Number.isFinite(sampleRate) || sampleRate < 8000 || sampleRate > 192000) {
      throw new Error("Unsupported microphone sample rate");
    }
    const samplesFor = (ms: number) => Math.ceil(sampleRate * ms / 1000);
    this.preRoll = new Float32Array(samplesFor(PRE_ROLL_MS));
    this.utterance = new Float32Array(samplesFor(MAX_UTTERANCE_MS));
    this.onsetSamples = samplesFor(SPEECH_ONSET_MS);
    this.onsetGapSamples = samplesFor(ONSET_GAP_MS);
    this.minimumVoicedSamples = samplesFor(MIN_VOICED_MS);
    this.endpointSamples = samplesFor(ENDPOINT_SILENCE_MS);
    this.meterIntervalSamples = samplesFor(METER_INTERVAL_MS);
  }

  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    this.reset();
  }

  process(frame: Float32Array): void {
    if (!this.enabled || frame.length === 0) return;
    let sum = 0;
    for (const sample of frame) sum += sample * sample;
    const rms = Math.sqrt(sum / frame.length);
    this.meterCount += frame.length;
    this.meterEnergy += sum;
    if (this.meterCount >= this.meterIntervalSamples) {
      const level = Math.min(1, Math.sqrt(this.meterEnergy / this.meterCount) * 3.4);
      this.meterCount = 0;
      this.meterEnergy = 0;
      this.emit({ type: "level", level });
    }

    if (!this.active) {
      this.appendPreRoll(frame);
      if (rms >= START_RMS) {
        this.onsetCount += frame.length;
        this.onsetSilenceCount = 0;
      } else {
        this.onsetSilenceCount += frame.length;
        if (this.onsetSilenceCount >= this.onsetGapSamples) this.onsetCount = 0;
      }
      if (this.onsetCount < this.onsetSamples) return;
      this.active = true;
      this.voicedCount = this.onsetCount;
      const start = (this.preRollOffset - this.preRollCount + this.preRoll.length) % this.preRoll.length;
      const firstLength = Math.min(this.preRollCount, this.preRoll.length - start);
      this.utterance.set(this.preRoll.subarray(start, start + firstLength));
      this.utterance.set(this.preRoll.subarray(0, this.preRollCount - firstLength), firstLength);
      this.utteranceCount = this.preRollCount;
      return;
    }

    const remaining = this.utterance.length - this.utteranceCount;
    const count = Math.min(remaining, frame.length);
    this.utterance.set(frame.subarray(0, count), this.utteranceCount);
    this.utteranceCount += count;
    if (rms >= END_RMS) {
      this.voicedCount += count;
      this.silenceCount = 0;
    } else {
      this.silenceCount += count;
    }
    if (this.silenceCount >= this.endpointSamples || this.utteranceCount === this.utterance.length) {
      const reason = this.utteranceCount === this.utterance.length ? "limit" : "silence";
      if (this.voicedCount >= this.minimumVoicedSamples) {
        const samples = this.utterance.slice(0, this.utteranceCount);
        // Pause on the audio thread before posting to a potentially busy UI.
        this.enabled = false;
        this.reset();
        this.emit({ type: "utterance", samples, sampleRate: this.sampleRate, reason });
      } else {
        this.reset();
      }
    }
  }

  private appendPreRoll(frame: Float32Array): void {
    const input = frame.length > this.preRoll.length ? frame.subarray(frame.length - this.preRoll.length) : frame;
    const firstLength = Math.min(input.length, this.preRoll.length - this.preRollOffset);
    this.preRoll.set(input.subarray(0, firstLength), this.preRollOffset);
    this.preRoll.set(input.subarray(firstLength), 0);
    this.preRollOffset = (this.preRollOffset + input.length) % this.preRoll.length;
    this.preRollCount = Math.min(this.preRoll.length, this.preRollCount + input.length);
  }

  private reset(): void {
    // Clear only the recorded region; avoid touching the full 20-second
    // allocation on every gate change in the audio rendering thread.
    this.preRoll.fill(0);
    this.utterance.fill(0, 0, this.utteranceCount);
    this.preRollOffset = 0;
    this.preRollCount = 0;
    this.utteranceCount = 0;
    this.onsetCount = 0;
    this.onsetSilenceCount = 0;
    this.voicedCount = 0;
    this.silenceCount = 0;
    this.meterCount = 0;
    this.meterEnergy = 0;
    this.active = false;
  }
}
