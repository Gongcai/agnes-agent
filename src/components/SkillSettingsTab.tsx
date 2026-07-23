import React, { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Check,
  ExternalLink,
  FolderInput,
  GitBranch,
  LoaderCircle,
  Puzzle,
  RefreshCw,
  Trash2,
} from "lucide-react";
import {
  installSkillsFromGit,
  installSkillsFromPath,
  listInstalledSkills,
  openSkillDirectory,
  setSkillEnabled,
  uninstallSkill,
  type InstalledSkill,
} from "../lib/skills";
import { useConfirmDialog } from "./ConfirmDialog";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
}

export const SkillSettingsTab: React.FC = () => {
  const confirmDelete = useConfirmDialog();
  const [skills, setSkills] = useState<InstalledSkill[]>([]);
  const [gitUrl, setGitUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [busySkillId, setBusySkillId] = useState<string | null>(null);
  const [message, setMessage] = useState<{ success: boolean; text: string } | null>(null);

  const loadSkills = useCallback(async () => {
    setLoading(true);
    try {
      setSkills(await listInstalledSkills());
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSkills();
  }, [loadSkills]);

  const installLocal = async () => {
    let selected: string | string[] | null;
    try {
      selected = await open({
        directory: true,
        multiple: false,
        title: "选择 Skill 目录或包含多个 Skills 的仓库目录",
      });
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
      return;
    }
    if (!selected || Array.isArray(selected)) return;
    setLoading(true);
    setMessage(null);
    try {
      const installed = await installSkillsFromPath(selected);
      setMessage({
        success: true,
        text: `已安装 ${installed.length} 个 Skill：${installed.map((skill) => skill.name).join("、")}`,
      });
      await loadSkills();
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    } finally {
      setLoading(false);
    }
  };

  const installGit = async () => {
    const url = gitUrl.trim();
    if (!url) return;
    setLoading(true);
    setMessage(null);
    try {
      const installed = await installSkillsFromGit(url);
      setGitUrl("");
      setMessage({
        success: true,
        text: `已从 Git 仓库安装 ${installed.length} 个 Skill：${installed.map((skill) => skill.name).join("、")}`,
      });
      await loadSkills();
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    } finally {
      setLoading(false);
    }
  };

  const toggleEnabled = async (skill: InstalledSkill) => {
    setBusySkillId(skill.id);
    setMessage(null);
    try {
      const updated = await setSkillEnabled(skill.id, !skill.enabled);
      setSkills((current) => current.map((item) => item.id === updated.id ? updated : item));
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    } finally {
      setBusySkillId(null);
    }
  };

  const removeSkill = async (skill: InstalledSkill) => {
    if (!await confirmDelete({
      title: `卸载 Skill「${skill.name}」？`,
      description: "安装目录会移入本地回收区，可从系统回收区恢复。",
      confirmLabel: "卸载 Skill",
    })) return;
    setBusySkillId(skill.id);
    setMessage(null);
    try {
      await uninstallSkill(skill.id);
      setSkills((current) => current.filter((item) => item.id !== skill.id));
      setMessage({ success: true, text: `已卸载 ${skill.name}` });
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    } finally {
      setBusySkillId(null);
    }
  };

  const reinstallSkill = async (skill: InstalledSkill) => {
    setBusySkillId(skill.id);
    setMessage(null);
    try {
      const installed = skill.sourceKind === "git"
        ? await installSkillsFromGit(skill.sourceLabel)
        : await installSkillsFromPath(skill.sourceLabel);
      setMessage({
        success: true,
        text: `已从原始来源更新：${installed.map((item) => item.name).join("、")}`,
      });
      await loadSkills();
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    } finally {
      setBusySkillId(null);
    }
  };

  const revealSkill = async (skill: InstalledSkill) => {
    try {
      await openSkillDirectory(skill.id);
    } catch (reason) {
      setMessage({ success: false, text: String(reason) });
    }
  };

  return (
    <div className="space-y-5">
      <div>
        <h3 className="text-sm font-semibold text-stone-850">Skills</h3>
        <p className="mt-1 text-[11px] leading-relaxed text-stone-400">
          安装兼容 SKILL.md 的工作流说明。Skill 只在你于对话中明确选择时加载，不能覆盖工具权限、审批和沙箱策略。
        </p>
      </div>

      <div className="grid gap-3 rounded-xl border border-stone-200 bg-stone-50/60 p-4">
        <div className="flex items-center justify-between gap-3">
          <div>
            <div className="text-xs font-semibold text-stone-700">从本地目录安装</div>
            <div className="mt-0.5 text-[10px] text-stone-400">可选择单个 Skill 目录，也可扫描包含多个 SKILL.md 的仓库。</div>
          </div>
          <button
            type="button"
            onClick={() => void installLocal()}
            disabled={loading}
            className="flex shrink-0 items-center gap-1.5 rounded-lg border border-stone-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-stone-700 shadow-sm transition-colors hover:bg-stone-100 disabled:opacity-50"
          >
            <FolderInput className="h-3.5 w-3.5" />
            选择目录
          </button>
        </div>

        <div className="border-t border-stone-200 pt-3">
          <div className="mb-2 flex items-center gap-2 text-xs font-semibold text-stone-700">
            <GitBranch className="h-3.5 w-3.5" />
            从 HTTPS Git 仓库安装
          </div>
          <div className="flex gap-2">
            <input
              value={gitUrl}
              onChange={(event) => setGitUrl(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void installGit();
              }}
              placeholder="https://github.com/owner/skills.git"
              className="min-w-0 flex-1 rounded-lg border border-stone-200 bg-white px-3 py-2 font-mono text-[10px] text-stone-700 outline-none focus:border-[#8CA38A]"
            />
            <button
              type="button"
              onClick={() => void installGit()}
              disabled={loading || !gitUrl.trim()}
              className="rounded-lg bg-stone-900 px-3 py-2 text-[10px] font-semibold text-white transition-colors hover:bg-stone-800 disabled:opacity-40"
            >
              克隆并安装
            </button>
          </div>
        </div>
      </div>

      {message && (
        <div className={`rounded-lg border px-3 py-2 text-[10px] ${
          message.success
            ? "border-emerald-200 bg-emerald-50 text-emerald-700"
            : "border-rose-200 bg-rose-50 text-rose-700"
        }`}>
          {message.text}
        </div>
      )}

      <div className="flex items-center justify-between">
        <div className="text-xs font-semibold text-stone-700">已安装 · {skills.length}</div>
        <button
          type="button"
          onClick={() => void loadSkills()}
          disabled={loading}
          className="rounded-lg p-1.5 text-stone-400 transition-colors hover:bg-stone-100 hover:text-stone-700 disabled:opacity-40"
          title="刷新 Skills"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>

      {loading && skills.length === 0 ? (
        <div className="grid place-items-center py-12 text-stone-400">
          <LoaderCircle className="h-5 w-5 animate-spin" />
        </div>
      ) : skills.length === 0 ? (
        <div className="rounded-xl border border-dashed border-stone-250 px-6 py-12 text-center">
          <Puzzle className="mx-auto mb-3 h-6 w-6 text-stone-300" />
          <div className="text-xs font-semibold text-stone-500">尚未安装 Skill</div>
          <div className="mt-1 text-[10px] text-stone-400">安装后即可从聊天输入框的附件菜单中选择。</div>
        </div>
      ) : (
        <div className="space-y-2">
          {skills.map((skill) => {
            const busy = busySkillId === skill.id;
            return (
              <div key={skill.id} className="rounded-xl border border-stone-200 bg-white p-3 shadow-sm">
                <div className="flex items-start gap-3">
                  <span className={`mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg ${
                    skill.enabled ? "bg-violet-50 text-violet-600" : "bg-stone-100 text-stone-400"
                  }`}>
                    <Puzzle className="h-4 w-4" />
                  </span>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-xs font-semibold text-stone-800">{skill.name}</span>
                      {skill.version && <span className="rounded bg-stone-100 px-1.5 py-0.5 font-mono text-[8px] text-stone-400">v{skill.version}</span>}
                      {skill.enabled && <Check className="h-3 w-3 shrink-0 text-emerald-600" />}
                    </div>
                    <p className="mt-1 text-[10px] leading-relaxed text-stone-500">{skill.description}</p>
                    <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-[9px] text-stone-400">
                      <span>{skill.sourceKind === "git" ? "Git 安装" : "本地安装"}</span>
                      <span>{skill.fileCount} 个文件</span>
                      <span>{formatBytes(skill.totalBytes)}</span>
                      {skill.author && <span>作者：{skill.author}</span>}
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <button
                      type="button"
                      onClick={() => void toggleEnabled(skill)}
                      disabled={busy}
                      className={`rounded-lg px-2.5 py-1.5 text-[9px] font-semibold transition-colors disabled:opacity-40 ${
                        skill.enabled
                          ? "bg-emerald-50 text-emerald-700 hover:bg-emerald-100"
                          : "bg-stone-100 text-stone-500 hover:bg-stone-200"
                      }`}
                    >
                      {skill.enabled ? "已启用" : "已停用"}
                    </button>
                    <button
                      type="button"
                      onClick={() => void reinstallSkill(skill)}
                      disabled={busy}
                      className="rounded-lg p-1.5 text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-40"
                      title="从原始来源重新安装"
                    >
                      <RefreshCw className={`h-3.5 w-3.5 ${busy ? "animate-spin" : ""}`} />
                    </button>
                    <button
                      type="button"
                      onClick={() => void revealSkill(skill)}
                      className="rounded-lg p-1.5 text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                      title="打开安装目录"
                    >
                      <ExternalLink className="h-3.5 w-3.5" />
                    </button>
                    <button
                      type="button"
                      onClick={() => void removeSkill(skill)}
                      disabled={busy}
                      className="rounded-lg p-1.5 text-stone-400 hover:bg-rose-50 hover:text-rose-600 disabled:opacity-40"
                      title="卸载 Skill"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
