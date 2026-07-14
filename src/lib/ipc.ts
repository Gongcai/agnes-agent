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
