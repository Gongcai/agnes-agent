use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 工具权限模型 = capability + approval + sandbox + audit。
/// 审批只是最后一关；capability 决定能访问哪里、跑多久、出多少。
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ToolPolicy {
    #[serde(default)]
    pub shell: ShellPolicy,
    #[serde(default)]
    pub file: FilePolicy,
    #[serde(default)]
    pub git: GitPolicy,
    #[serde(default)]
    pub memory: MemoryPolicy,
    #[serde(default)]
    pub planner: PlannerPolicy,
    #[serde(default)]
    pub sandbox: SandboxPolicy,
    #[serde(default)]
    pub network: NetworkPolicy,
}

/// Risk level used by audit records and approval decisions.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Risk::Low => "Low",
            Risk::Medium => "Medium",
            Risk::High => "High",
        }
    }
}

/// Unified approval tier.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalTier {
    Never,
    OnWrite,
    OnRisk,
    Always,
}

/// Accept legacy string and boolean policy values while serializing the new tier names.
impl<'de> Deserialize<'de> for ApprovalTier {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ApprovalTierValue {
            String(String),
            Bool(bool),
        }

        Ok(match ApprovalTierValue::deserialize(deserializer)? {
            ApprovalTierValue::String(value) => match value.as_str() {
                "never" => ApprovalTier::Never,
                "on_write" | "on-write" | "write" => ApprovalTier::OnWrite,
                "on_risk" | "on-risk" | "push" => ApprovalTier::OnRisk,
                "always" => ApprovalTier::Always,
                _ => ApprovalTier::Always,
            },
            ApprovalTierValue::Bool(true) => ApprovalTier::Always,
            ApprovalTierValue::Bool(false) => ApprovalTier::Never,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ShellPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub allowed_cwd: Vec<String>,
    pub deny_write_outside_workspace: bool,
    pub timeout_sec: u32,
    pub max_output_bytes: u32,
    pub env_allowlist: Vec<String>,
}

impl Default for ShellPolicy {
    fn default() -> Self {
        ShellPolicy {
            enabled: true,
            approval: ApprovalTier::OnRisk,
            allowed_cwd: vec!["~/Projects".into()],
            deny_write_outside_workspace: true,
            timeout_sec: 30,
            max_output_bytes: 200_000,
            env_allowlist: vec!["PATH".into(), "HOME".into()],
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct FilePolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub allowed_roots: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct MemoryPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct PlannerPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
}

impl Default for MemoryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            approval: ApprovalTier::OnWrite,
        }
    }
}

impl Default for PlannerPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            approval: ApprovalTier::Always,
        }
    }
}

impl Default for FilePolicy {
    fn default() -> Self {
        FilePolicy {
            enabled: true,
            approval: ApprovalTier::OnWrite,
            allowed_roots: vec!["~/Projects".into(), "~/.agnes".into()],
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct GitPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub timeout_sec: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct SandboxPolicy {
    pub landlock: bool,
    pub bwrap: BwrapMode,
    pub rlimits: bool,
    pub cpu_time_sec: u64,
    pub memory_bytes: u64,
    pub file_size_bytes: u64,
    pub max_processes: u64,
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BwrapMode {
    Auto,
    Disabled,
    Required,
}

impl Default for BwrapMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl<'de> Deserialize<'de> for BwrapMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            String(String),
            Bool(bool),
        }

        Ok(match Value::deserialize(deserializer)? {
            Value::String(value) => match value.as_str() {
                "auto" => Self::Auto,
                "disabled" | "never" => Self::Disabled,
                "required" | "always" => Self::Required,
                _ => Self::Auto,
            },
            Value::Bool(true) => Self::Auto,
            Value::Bool(false) => Self::Disabled,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NetworkPolicy {
    pub allow: bool,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self { allow: true }
    }
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            landlock: true,
            bwrap: BwrapMode::Auto,
            rlimits: true,
            cpu_time_sec: 60,
            memory_bytes: 1024 * 1024 * 1024,
            file_size_bytes: 50 * 1024 * 1024,
            max_processes: 64,
        }
    }
}

impl Default for GitPolicy {
    fn default() -> Self {
        GitPolicy {
            enabled: true,
            approval: ApprovalTier::OnRisk,
            timeout_sec: 30,
        }
    }
}

/// 展开波浪号 `~` 到 Home 目录。
pub fn expand_home(path_str: &str) -> PathBuf {
    if path_str.starts_with("~/") || path_str == "~" {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            if path_str == "~" {
                home
            } else {
                home.join(&path_str[2..])
            }
        } else {
            PathBuf::from(path_str)
        }
    } else {
        PathBuf::from(path_str)
    }
}

/// 检查目标路径是否在允许的根目录列表内。
pub fn is_path_under_roots(target: &Path, roots: &[String]) -> bool {
    let target_canonical = match target.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // 如果文件不存在，尝试规范化其父目录并拼接文件名
            if let Some(parent) = target.parent() {
                match parent.canonicalize() {
                    Ok(p) => p.join(target.file_name().unwrap_or_default()),
                    Err(_) => target.to_path_buf(),
                }
            } else {
                target.to_path_buf()
            }
        }
    };

    for root_str in roots {
        let root_path = expand_home(root_str);
        if let Ok(root_canonical) = root_path.canonicalize() {
            if target_canonical.starts_with(root_canonical) {
                return true;
            }
        } else {
            // 回退到未规范化匹配
            if target_canonical.starts_with(&root_path) {
                return true;
            }
        }
    }
    false
}

impl ToolPolicy {
    /// 验证 shell 执行策略。
    pub fn check_shell(&self, cwd: &str) -> Result<(), String> {
        if !self.shell.enabled {
            return Err("Shell 执行工具已被禁用".into());
        }
        let cwd_path = PathBuf::from(cwd);
        if !is_path_under_roots(&cwd_path, &self.shell.allowed_cwd) {
            return Err(format!(
                "工作目录 `{}` 不在允许的 shell 执行目录列表内 ({:?})",
                cwd, self.shell.allowed_cwd
            ));
        }
        Ok(())
    }

    /// 验证文件读取策略。
    pub fn check_file_read(&self, path: &str) -> Result<(), String> {
        if !self.file.enabled {
            return Err("文件工具已被禁用".into());
        }
        let file_path = PathBuf::from(path);
        if !is_path_under_roots(&file_path, &self.file.allowed_roots) {
            return Err(format!(
                "访问的路径 `{}` 不在允许的根目录列表内 ({:?})",
                path, self.file.allowed_roots
            ));
        }
        Ok(())
    }

    /// 验证文件写入策略。
    pub fn check_file_write(&self, path: &str) -> Result<(), String> {
        self.check_file_read(path) // 写入必须先满足路径范围限制
    }

    /// 验证 git 策略。
    pub fn check_git(&self) -> Result<(), String> {
        if !self.git.enabled {
            return Err("Git 工具已被禁用".into());
        }
        Ok(())
    }

    /// Validate whether local calendar and task tools are available to this agent.
    pub fn check_planner(&self) -> Result<(), String> {
        if !self.planner.enabled {
            return Err("Calendar and task tools are disabled for this agent".into());
        }
        Ok(())
    }

    /// Return the approval tier for a tool. Unknown tools fail closed.
    pub fn approval_for(&self, tool: &str) -> ApprovalTier {
        match tool {
            "shell" => self.shell.approval,
            "file_read" | "list_files" | "grep" => ApprovalTier::Never,
            "file_write" | "file_edit" | "apply_patch" => self.file.approval,
            "git" => self.git.approval,
            "memory_search" | "memory_md_view" => ApprovalTier::Never,
            "memory_create" | "memory_update" | "memory_md_edit" => self.memory.approval,
            "calendar_list" | "task_list" => ApprovalTier::Never,
            "calendar_create"
            | "calendar_event_create"
            | "calendar_update"
            | "task_create"
            | "task_complete"
            | "task_update" => self.planner.approval,
            _ => ApprovalTier::Always,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_tier_accepts_legacy_values() {
        let legacy: ToolPolicy = serde_json::from_str(
            r#"{
                "shell": {"approval": "always"},
                "file": {"approval": "write"},
                "git": {"approval": "push"}
            }"#,
        )
        .unwrap();
        assert_eq!(legacy.shell.approval, ApprovalTier::Always);
        assert_eq!(legacy.file.approval, ApprovalTier::OnWrite);
        assert_eq!(legacy.git.approval, ApprovalTier::OnRisk);

        let boolean: ToolPolicy = serde_json::from_str(
            r#"{
                "shell": {"approval": true},
                "file": {"approval": false}
            }"#,
        )
        .unwrap();
        assert_eq!(boolean.shell.approval, ApprovalTier::Always);
        assert_eq!(boolean.file.approval, ApprovalTier::Never);
    }

    #[test]
    fn approval_tier_serializes_with_new_names() {
        let value = serde_json::to_value(ToolPolicy::default()).unwrap();
        assert_eq!(value["shell"]["approval"], "on_risk");
        assert_eq!(value["file"]["approval"], "on_write");
        assert_eq!(value["git"]["approval"], "on_risk");
        assert_eq!(value["memory"]["approval"], "on_write");
        assert_eq!(value["planner"]["approval"], "always");
        assert_eq!(value["git"]["timeout_sec"], 30);
        assert_eq!(value["network"]["allow"], true);
        assert_eq!(value["sandbox"]["bwrap"], "auto");
    }

    #[test]
    fn planner_capability_can_be_disabled() {
        let mut policy = ToolPolicy::default();
        assert!(policy.check_planner().is_ok());
        policy.planner.enabled = false;
        assert!(policy.check_planner().is_err());
    }
}
