export class VoicePhaseTimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "VoicePhaseTimeoutError";
  }
}

interface VoiceDeadlineOptions<T> {
  timeoutMs: number;
  message: string;
  signal?: AbortSignal;
  onLateResolve?: (value: T) => void;
}

/** Bound UI waits while still cleaning up resources acquired after cancellation. */
export function withVoiceDeadline<T>(task: Promise<T>, options: VoiceDeadlineOptions<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    const cleanup = () => {
      window.clearTimeout(timer);
      options.signal?.removeEventListener("abort", abort);
    };
    const fail = (error: Error) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const abort = () => fail(new DOMException("Voice operation cancelled", "AbortError"));
    const timer = window.setTimeout(() => fail(new VoicePhaseTimeoutError(options.message)), options.timeoutMs);
    options.signal?.addEventListener("abort", abort, { once: true });
    if (options.signal?.aborted) abort();
    task.then((value) => {
      if (settled) {
        try { options.onLateResolve?.(value); } catch (reason) { console.warn("Unable to release late voice resources", reason); }
        return;
      }
      settled = true;
      cleanup();
      resolve(value);
    }, (reason) => fail(reason instanceof Error ? reason : new Error(String(reason))));
  });
}
