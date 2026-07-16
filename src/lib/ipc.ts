import { invoke } from "@tauri-apps/api/core";

export interface AgentSummary {
  id: string;
  name: string;
}

/** 健康检查：Rust 直接回显，验证 IPC 通道。 */
export async function ping(): Promise<string> {
  return invoke<string>("ping");
}

/** 列出当前所有 Agent（角色卡）。 */
export async function listAgents(): Promise<AgentSummary[]> {
  return invoke<AgentSummary[]>("list_agents");
}

export interface SyncStatus {
  state: "idle" | "pending" | "syncing" | "auth_required" | "conflict" | "error";
  gatewayUrl: string;
  credentialConfigured: boolean;
  syncing: boolean;
  deviceId: string;
  pendingCount: number;
  inFlightCount: number;
  conflictCount: number;
  deadLetterCount: number;
  lastPullCursor: number;
  bootstrapState: string;
  lastSuccessAt: number | null;
  lastErrorCode: string | null;
  backoffUntil: number | null;
}

export async function getSyncStatus(): Promise<SyncStatus> {
  return invoke<SyncStatus>("get_sync_status");
}

export async function syncNow(): Promise<SyncStatus> {
  return invoke<SyncStatus>("sync_now");
}

export interface SyncConflict {
  id: string;
  entityType: string;
  entityId: string;
  baseRevision: number | null;
  remoteRevision: number | null;
  basePayload: Record<string, unknown> | null;
  localPayload: Record<string, unknown> | null;
  remotePayload: Record<string, unknown> | null;
  localDeleted: boolean;
  remoteDeleted: boolean;
  remoteReady: boolean;
  conflictingFields: string[];
  createdAt: number;
  updatedAt: number;
}

export async function listSyncConflicts(): Promise<SyncConflict[]> {
  return invoke<SyncConflict[]>("list_sync_conflicts");
}

export async function resolveSyncConflict(
  conflictId: string,
  resolution: "keep_local" | "keep_remote",
): Promise<void> {
  return invoke("resolve_sync_conflict", { conflictId, resolution });
}

export interface SyncDevice {
  id: string;
  name: string;
  platform: string | null;
  createdAt: number;
  lastSeenAt: number | null;
  revokedAt: number | null;
  lastAckCursor: number;
  current: boolean;
}

export async function listSyncDevices(): Promise<SyncDevice[]> {
  return invoke<SyncDevice[]>("list_sync_devices");
}

export async function revokeSyncDevice(deviceId: string): Promise<SyncDevice> {
  return invoke<SyncDevice>("revoke_sync_device", { deviceId });
}

export type SyncCredentialInput =
  | { kind: "bearer"; token: string }
  | { kind: "cloudflare_access"; client_id: string; client_secret: string };

export async function setSyncCredential(
  credential: SyncCredentialInput | null,
): Promise<SyncStatus> {
  return invoke<SyncStatus>("set_sync_credential", { credential });
}
