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

export type SyncCredentialInput =
  | { kind: "bearer"; token: string }
  | { kind: "cloudflare_access"; client_id: string; client_secret: string };

export async function setSyncCredential(
  credential: SyncCredentialInput | null,
): Promise<SyncStatus> {
  return invoke<SyncStatus>("set_sync_credential", { credential });
}
