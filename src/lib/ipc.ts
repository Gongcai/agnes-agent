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

export interface SyncE2eeStatus {
  keysetConfigured: boolean;
  confirmed: boolean;
  activeKeyVersion: number | null;
  confirmedKeyVersion: number | null;
  rotationPending: boolean;
  transportReady: boolean;
}

export interface SyncStatus {
  state:
    | "idle"
    | "pending"
    | "syncing"
    | "auth_required"
    | "e2ee_required"
    | "e2ee_pending"
    | "conflict"
    | "error";
  gatewayUrl: string;
  credentialConfigured: boolean;
  syncing: boolean;
  e2ee: SyncE2eeStatus;
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
  e2eeKeyVersion: number | null;
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

export interface SyncRecoveryMaterial {
  recoveryKey: string;
  recoveryBundle: string;
  activeKeyVersion: number;
}

export async function beginSyncE2eeSetup(): Promise<SyncRecoveryMaterial> {
  return invoke<SyncRecoveryMaterial>("begin_sync_e2ee_setup");
}

export async function beginSyncE2eeRotation(): Promise<SyncRecoveryMaterial> {
  return invoke<SyncRecoveryMaterial>("begin_sync_e2ee_rotation");
}

export async function confirmSyncE2eeSetup(): Promise<SyncStatus> {
  return invoke<SyncStatus>("confirm_sync_e2ee_setup");
}

export async function restoreSyncE2ee(
  recoveryKey: string,
  recoveryBundle: string,
): Promise<SyncStatus> {
  return invoke<SyncStatus>("restore_sync_e2ee", { recoveryKey, recoveryBundle });
}

export async function discardSyncE2eeSetup(): Promise<SyncStatus> {
  return invoke<SyncStatus>("discard_sync_e2ee_setup");
}

export interface SyncPairingInvite {
  sessionId: string;
  pairingCode: string;
  expiresAt: number;
}

export interface SyncPairingDevice {
  sessionId: string;
  deviceId: string;
  deviceName: string;
  platform: string | null;
  expiresAt: number;
}

export interface SyncPairingJoinStarted {
  sessionId: string;
  expiresAt: number;
}

export interface SyncPairingCompletion {
  status: "pending" | "complete";
  syncStatus: SyncStatus | null;
}

export async function startSyncPairing(): Promise<SyncPairingInvite> {
  return invoke<SyncPairingInvite>("start_sync_pairing");
}

export async function getSyncPairingRequest(sessionId: string): Promise<SyncPairingDevice> {
  return invoke<SyncPairingDevice>("get_sync_pairing_request", { sessionId });
}

export async function approveSyncPairing(sessionId: string): Promise<SyncPairingDevice> {
  return invoke<SyncPairingDevice>("approve_sync_pairing", { sessionId });
}

export async function joinSyncPairing(
  pairingCode: string,
  deviceName: string,
): Promise<SyncPairingJoinStarted> {
  return invoke<SyncPairingJoinStarted>("join_sync_pairing", { pairingCode, deviceName });
}

export async function finishSyncPairing(sessionId: string): Promise<SyncPairingCompletion> {
  return invoke<SyncPairingCompletion>("finish_sync_pairing", { sessionId });
}
