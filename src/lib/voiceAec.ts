import { invoke, isTauri } from "@tauri-apps/api/core";
import { withVoiceDeadline } from "./voiceDeadline";

export interface VoiceAudioDevice {
  name: string;
  description: string;
}

export interface VoiceAecStatus {
  supported: boolean;
  running: boolean;
  routes_ready: boolean;
  needs_reconnect: boolean;
  source_name: string | null;
  source_label: string | null;
  input: VoiceAudioDevice | null;
  output: VoiceAudioDevice | null;
  message: string | null;
}

export async function getVoiceAecStatus(signal?: AbortSignal): Promise<VoiceAecStatus | null> {
  if (!isTauri()) return null;
  return withVoiceDeadline(invoke<VoiceAecStatus>("get_voice_aec_status"), {
    timeoutMs: 10_000,
    message: "读取回声消除状态超时，请在语音设置中重新连接后重试。",
    signal,
  });
}

export async function openVoiceMicrophone(deviceId: string, signal: AbortSignal): Promise<{
  stream: MediaStream;
  pipewireAec: boolean;
}> {
  const aec = await getVoiceAecStatus(signal);
  const wantsDefaultAec = !deviceId && !!aec?.running;
  const requireRoutes = () => {
    if (!aec?.routes_ready || !aec.source_label) {
      throw new Error(aec?.message || "回声消除尚未连接，请在语音设置中重新连接后重试。");
    }
  };
  if (wantsDefaultAec) requireRoutes();
  const enumerate = () => withVoiceDeadline(navigator.mediaDevices.enumerateDevices(), {
    timeoutMs: 3_000,
    message: "读取麦克风列表超时，请重新连接输入设备。",
    signal,
  });
  const matchesSource = (device: MediaDeviceInfo) => device.kind === "audioinput"
    && device.deviceId !== "default" && device.deviceId !== "communications"
    && !!aec?.source_label && device.label.includes(aec.source_label);
  const open = (id: string, filtered: boolean) => withVoiceDeadline(navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: !filtered,
      noiseSuppression: !filtered,
      autoGainControl: !filtered,
      ...(id ? { deviceId: { exact: id } } : {}),
    },
  }), {
    timeoutMs: 60_000,
    message: "麦克风连接超时，请确认权限和输入设备后重试。",
    signal,
    onLateResolve: (lateStream) => lateStream.getTracks().forEach((track) => track.stop()),
  });
  let stream: MediaStream | null = null;
  const release = () => stream?.getTracks().forEach((track) => track.stop());
  signal.addEventListener("abort", release, { once: true });
  try {
    const before = aec?.running ? await enumerate() : [];
    let selected = wantsDefaultAec ? before.find(matchesSource) : before.find((device) => device.deviceId === deviceId);
    let filtered = !!aec?.running && !!selected && matchesSource(selected);
    if (filtered) requireRoutes();
    stream = await open(filtered ? selected!.deviceId : deviceId, filtered);
    // First permission can reveal labels that were hidden before getUserMedia.
    if (aec?.running && !filtered) {
      const devices = await enumerate();
      const actualId = stream.getAudioTracks()[0]?.getSettings().deviceId;
      selected = wantsDefaultAec ? devices.find(matchesSource) : devices.find((device) => device.deviceId === actualId);
      if (wantsDefaultAec || selected && matchesSource(selected)) {
        requireRoutes();
        if (!selected || !matchesSource(selected)) {
          throw new Error("回声消除输入未出现在麦克风列表，请在语音设置中重新连接后重试。");
        }
        release();
        stream = null;
        stream = await open(selected.deviceId, true);
        filtered = true;
      }
    }
    if (signal.aborted) throw new DOMException("Microphone connection cancelled", "AbortError");
    const result = { stream, pipewireAec: filtered };
    stream = null;
    return result;
  } finally {
    signal.removeEventListener("abort", release);
    release();
  }
}
