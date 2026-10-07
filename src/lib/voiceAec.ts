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
  actualDeviceId: string | null;
  aecSourceLabel: string | null;
}> {
  const aec = await getVoiceAecStatus(signal);
  // The AEC manager owns the default source while it is running. A device id
  // kept by the WebView may point to a stale Agnes source from an older run.
  const wantsDefaultAec = !!aec?.running;
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
  const matchesSource = (device: MediaDeviceInfo) => {
    if (device.kind !== "audioinput" || device.deviceId === "default" || device.deviceId === "communications" || !aec?.source_label) return false;
    const label = device.label.trim().toLowerCase();
    const sourceLabel = aec.source_label.trim().toLowerCase();
    // WebKitGTK may expose only the friendly "Agnes" label instead of the
    // complete PipeWire description, so accept the stable product prefix.
    return label.includes(sourceLabel) || (sourceLabel.startsWith("agnes") && label.startsWith("agnes"));
  };
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
    // The AEC manager makes the virtual source the PulseAudio default and
    // verifies its routes. Do not require WebKitGTK to expose the same label.
    let filtered = wantsDefaultAec || (!!aec?.running && !!selected && matchesSource(selected));
    if (filtered) requireRoutes();
    stream = await open(wantsDefaultAec ? "" : filtered ? selected!.deviceId : deviceId, filtered);
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
    const actualDeviceId = stream.getAudioTracks()[0]?.getSettings().deviceId || null;
    const result = {
      stream,
      pipewireAec: filtered,
      actualDeviceId,
      aecSourceLabel: filtered ? aec?.source_label || null : null,
    };
    stream = null;
    return result;
  } finally {
    signal.removeEventListener("abort", release);
    release();
  }
}
