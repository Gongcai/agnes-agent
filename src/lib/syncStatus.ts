import type { SyncE2eeStatus } from "./ipc";

export function syncE2eeStatusMessage(status: SyncE2eeStatus): string {
  if (status.rotationPending) {
    return "保存新恢复材料并确认后才会启用新密钥";
  }
  if (!status.keysetConfigured) {
    return "需要本机密钥与恢复材料";
  }
  if (!status.confirmed) {
    return "恢复材料尚未确认";
  }
  if (!status.transportReady) {
    return "加密传输待接入";
  }
  return "恢复材料已确认";
}
