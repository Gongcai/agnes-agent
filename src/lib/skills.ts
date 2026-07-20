import { invoke } from "@tauri-apps/api/core";

export interface InstalledSkill {
  id: string;
  name: string;
  description: string;
  version: string | null;
  author: string | null;
  enabled: boolean;
  sourceKind: "local" | "git";
  sourceLabel: string;
  installedAt: string;
  updatedAt: string;
  fileCount: number;
  totalBytes: number;
  contentHash: string;
}

export function listInstalledSkills(): Promise<InstalledSkill[]> {
  return invoke("list_installed_skills");
}

export function installSkillsFromPath(path: string): Promise<InstalledSkill[]> {
  return invoke("install_skills_from_path", { path });
}

export function installSkillsFromGit(url: string): Promise<InstalledSkill[]> {
  return invoke("install_skills_from_git", { url });
}

export function setSkillEnabled(skillId: string, enabled: boolean): Promise<InstalledSkill> {
  return invoke("set_skill_enabled", { skillId, enabled });
}

export function uninstallSkill(skillId: string): Promise<void> {
  return invoke("uninstall_skill", { skillId });
}

export function openSkillDirectory(skillId: string): Promise<void> {
  return invoke("open_skill_directory", { skillId });
}
