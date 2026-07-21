import { describe, expect, it } from "vitest";
import type { SyncE2eeStatus } from "./ipc";
import { syncE2eeStatusMessage } from "./syncStatus";

const READY_STATUS: SyncE2eeStatus = {
  keysetConfigured: true,
  confirmed: true,
  activeKeyVersion: 2,
  confirmedKeyVersion: 2,
  rotationPending: false,
  transportReady: true,
};

describe("syncE2eeStatusMessage", () => {
  it("reports a confirmed recovery material when E2EE is ready", () => {
    expect(syncE2eeStatusMessage(READY_STATUS)).toBe("恢复材料已确认");
  });

  it.each([
    [
      { ...READY_STATUS, keysetConfigured: false, confirmed: false, activeKeyVersion: null, confirmedKeyVersion: null },
      "需要本机密钥与恢复材料",
    ],
    [
      { ...READY_STATUS, confirmed: false, confirmedKeyVersion: null },
      "恢复材料尚未确认",
    ],
    [
      { ...READY_STATUS, confirmed: false, activeKeyVersion: 3, rotationPending: true },
      "保存新恢复材料并确认后才会启用新密钥",
    ],
    [
      { ...READY_STATUS, transportReady: false },
      "加密传输待接入",
    ],
  ])("maps the remaining E2EE states", (status, message) => {
    expect(syncE2eeStatusMessage(status)).toBe(message);
  });
});
