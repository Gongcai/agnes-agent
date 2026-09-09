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
    pub read_image: ReadImagePolicy,
    #[serde(default)]
    pub git: GitPolicy,
    #[serde(default)]
    pub memory: MemoryPolicy,
    #[serde(default)]
    pub planner: PlannerPolicy,
    #[serde(default)]
    pub web: WebPolicy,
    #[serde(default)]
    pub mcp: McpPolicy,
    #[serde(default)]
    pub storage: StoragePolicy,
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
            allowed_cwd: Vec::new(),
            deny_write_outside_workspace: true,
            // A zero value means no wall-clock deadline. Callers can still
            // provide a bounded wait window when polling a terminal session.
            timeout_sec: 0,
            max_output_bytes: 200_000,
            // An empty allowlist inherits the host environment after the
            // sandbox removes credential-shaped variables.
            env_allowlist: Vec::new(),
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

/// Policy for the read-only visual inspection capability.
///
/// This is intentionally separate from `FilePolicy`: an agent may be allowed
/// to inspect images while ordinary text/file editing tools remain disabled.
#[derive(Serialize, Debug, Clone)]
pub struct ReadImagePolicy {
    pub enabled: bool,
    pub allowed_roots: Vec<String>,
}

impl<'de> Deserialize<'de> for ReadImagePolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Fields {
            #[serde(default = "default_enabled")]
            enabled: bool,
            #[serde(default)]
            allowed_roots: Vec<String>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Value {
            Enabled(bool),
            Fields(Fields),
        }

        match Value::deserialize(deserializer)? {
            Value::Enabled(enabled) => Ok(Self {
                enabled,
                allowed_roots: Vec::new(),
            }),
            Value::Fields(fields) => Ok(Self {
                enabled: fields.enabled,
                allowed_roots: fields.allowed_roots,
            }),
        }
    }
}

fn default_enabled() -> bool {
    true
}

impl Default for ReadImagePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_roots: Vec::new(),
        }
    }
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

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct WebPolicy {
    pub enabled: bool,
    pub approval: ApprovalTier,
    pub search_provider: String,
    pub timeout_sec: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct McpPolicy {
    pub enabled: bool,
    pub server_ids: Vec<String>,
    pub approval: ApprovalTier,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct StoragePolicy {
    pub enabled: bool,
    pub allowed_accounts: Vec<String>,
    pub max_download_bytes: u64,
    pub max_upload_bytes: u64,
    pub approval: ApprovalTier,
}

impl Default for StoragePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_accounts: Vec::new(),
            max_download_bytes: 1024 * 1024 * 1024,
            max_upload_bytes: 1024 * 1024 * 1024,
            approval: ApprovalTier::Always,
        }
    }
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

impl Default for WebPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            approval: ApprovalTier::Never,
            search_provider: "auto".into(),
            timeout_sec: 15,
        }
    }
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            server_ids: Vec::new(),
            approval: ApprovalTier::Always,
        }
    }
}

impl Default for FilePolicy {
    fn default() -> Self {
        FilePolicy {
            enabled: true,
            approval: ApprovalTier::OnWrite,
            allowed_roots: Vec::new(),
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
    pub allow_read_outside_workspace: bool,
    pub writable_roots: Vec<String>,
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
            // Hard process/file/memory limits are opt-in. Fixed low limits
            // make ordinary compilers and package managers unusable.
            rlimits: false,
            // Match Codex workspace mode: reads are broadly available while
            // writes remain constrained to explicit writable roots.
            allow_read_outside_workspace: true,
            writable_roots: vec![
                "~/.cache".into(),
                "~/.local".into(),
                "~/.cargo".into(),
                "~/.rustup".into(),
                "/tmp".into(),
            ],
            // Zero means unlimited for the corresponding optional limit.
            cpu_time_sec: 0,
            memory_bytes: 0,
            file_size_bytes: 0,
            max_processes: 0,
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
    /// Replace the device-local workspace path with a stable symbolic label before
    /// sending policy context to a model.
    pub fn redacted_for_prompt(&self, workspace_cwd: Option<&Path>) -> Self {
        let Some(workspace_cwd) = workspace_cwd else {
            return self.clone();
        };
        let workspace = workspace_cwd.to_string_lossy();
        let mut policy = self.clone();
        for path in &mut policy.shell.allowed_cwd {
            if path == workspace.as_ref() {
                *path = "$WORKSPACE".to_string();
            }
        }
        for path in &mut policy.file.allowed_roots {
            if path == workspace.as_ref() {
                *path = "$WORKSPACE".to_string();
            }
        }
        for path in &mut policy.read_image.allowed_roots {
            if path == workspace.as_ref() {
                *path = "$WORKSPACE".to_string();
            }
        }
        policy
    }

    /// 验证 shell 执行策略。
    pub fn check_shell(&self, cwd: &str) -> Result<(), String> {
        if !self.shell.enabled {
            return Err("Shell 执行工具已被禁用".into());
        }
        let cwd_path = PathBuf::from(cwd);
        let temporary_roots = crate::tools::sandbox::temporary_write_paths()
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        if !is_path_under_roots(&cwd_path, &self.shell.allowed_cwd)
            && !is_path_under_roots(&cwd_path, &self.file.allowed_roots)
            && !is_path_under_roots(&cwd_path, &self.sandbox.writable_roots)
            && !is_path_under_roots(&cwd_path, &temporary_roots)
        {
            // Read-only shell commands may inspect directories outside the
            // workspace when the sandbox explicitly permits those reads.
            if self.sandbox.allow_read_outside_workspace {
                return Ok(());
            }
            return Err(format!(
                "工作目录 `{}` 不在 workspace、额外可写目录或系统临时目录内",
                cwd
            ));
        }
        Ok(())
    }

    /// 验证文件读取策略。
    pub fn check_file_read(&self, path: &str) -> Result<(), String> {
        if !self.file.enabled {
            return Err("文件工具已被禁用".into());
        }
        if self.sandbox.allow_read_outside_workspace {
            return Ok(());
        }
        let file_path = PathBuf::from(path);
        let temporary_roots = crate::tools::sandbox::temporary_write_paths()
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        if !is_path_under_roots(&file_path, &self.file.allowed_roots)
            && !is_path_under_roots(&file_path, &self.sandbox.writable_roots)
            && !is_path_under_roots(&file_path, &temporary_roots)
        {
            return Err(format!(
                "访问的路径 `{}` 不在允许的只读或可写根目录内",
                path
            ));
        }
        Ok(())
    }

    /// Validate whether the visual inspection capability may read a path.
    ///
    /// Include legacy file roots in the boundary so agents created before the
    /// standalone image capability was introduced keep working in strict mode.
    pub fn check_image_read(&self, path: &str) -> Result<(), String> {
        if !self.read_image.enabled {
            return Err("图片读取工具已被禁用".into());
        }
        if self.sandbox.allow_read_outside_workspace {
            return Ok(());
        }
        let file_path = PathBuf::from(path);
        let temporary_roots = crate::tools::sandbox::temporary_write_paths()
            .into_iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let mut allowed_roots = self.read_image.allowed_roots.clone();
        for root in &self.file.allowed_roots {
            if !allowed_roots.contains(root) {
                allowed_roots.push(root.clone());
            }
        }
        if !is_path_under_roots(&file_path, &allowed_roots)
            && !is_path_under_roots(&file_path, &self.sandbox.writable_roots)
            && !is_path_under_roots(&file_path, &temporary_roots)
        {
            return Err(format!(
                "访问的图片路径 `{}` 不在允许的只读或可写根目录内",
                path
            ));
        }
        Ok(())
    }

    /// 验证文件写入策略。
    pub fn check_file_write(&self, path: &str) -> Result<(), String> {
        let _ = path;
        if !self.file.enabled {
            return Err("文件工具已被禁用".into());
        }
        // PolicySandbox owns the write boundary. Read access must not imply
        // write access, and strict read mode must not block writable roots.
        Ok(())
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

    /// Validate whether read-only web research is available to this agent.
    pub fn check_web(&self) -> Result<(), String> {
        if !self.web.enabled {
            return Err("Web research tools are disabled for this agent".into());
        }
        if !self.network.allow {
            return Err("Network access is disabled for this agent".into());
        }
        Ok(())
    }

    pub fn check_storage(&self, account_id: &str) -> Result<(), String> {
        if !self.storage.enabled {
            return Err("Storage tools are disabled for this agent".into());
        }
        if account_id.trim().is_empty() {
            return Err("Storage account ID must not be empty".into());
        }
        if !self.storage.allowed_accounts.is_empty()
            && !self
                .storage
                .allowed_accounts
                .iter()
                .any(|allowed| allowed == account_id)
        {
            return Err("Storage account is not allowed for this agent".into());
        }
        Ok(())
    }

    /// Return the approval tier for a tool. Unknown tools fail closed.
    pub fn approval_for(&self, tool: &str) -> ApprovalTier {
        let tool = crate::tools::builtin::canonical_tool_name(tool);
        if tool.starts_with("mcp__") {
            return self.mcp.approval;
        }
        match tool {
            "shell" | "write_stdin" | "stop_terminal" => self.shell.approval,
            "file_read" | "read_image" | "list_files" | "grep" => ApprovalTier::Never,
            "file_write" | "file_edit" | "apply_patch" => self.file.approval,
            "git" => self.git.approval,
            "memory_list" | "memory_read" => ApprovalTier::Never,
            "memory_write" | "memory_str_replace" | "memory_append" | "memory_delete" => {
                self.memory.approval
            }
            "web_search" | "web_fetch" | "browser_open" => self.web.approval,
            "storage_accounts_list"
            | "storage_files_list"
            | "storage_files_search"
            | "storage_transfer_status"
            | "storage_quota_get" => ApprovalTier::Never,
            "storage_file_download"
            | "storage_file_upload"
            | "storage_files_move"
            | "storage_files_trash" => self.storage.approval,
            "calendar_list" | "task_list" | "get_current_time" => ApprovalTier::Never,
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
    fn defaults_do_not_claim_a_hard_coded_projects_directory() {
        let policy = ToolPolicy::default();
        assert!(policy.shell.allowed_cwd.is_empty());
        assert!(policy.file.allowed_roots.is_empty());
    }

    #[test]
    fn prompt_policy_redacts_only_the_effective_workspace_path() {
        let workspace = PathBuf::from("/home/example/Documents/Agnes/Home");
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string(), "/tmp".into()];
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string(), "/opt".into()];

        let redacted = policy.redacted_for_prompt(Some(&workspace));

        assert_eq!(redacted.shell.allowed_cwd, vec!["$WORKSPACE", "/tmp"]);
        assert_eq!(redacted.file.allowed_roots, vec!["$WORKSPACE", "/opt"]);
        assert_eq!(
            policy.shell.allowed_cwd[0],
            "/home/example/Documents/Agnes/Home"
        );
    }

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
    fn legacy_agents_enable_image_reading_without_file_tools() {
        let policy: ToolPolicy = serde_json::from_str(r#"{"file":{"enabled":false}}"#).unwrap();

        assert!(!policy.file.enabled);
        assert!(policy.read_image.enabled);
        assert!(policy.check_image_read("/tmp/attached-image.png").is_ok());
    }

    #[test]
    fn image_read_policy_accepts_legacy_boolean_values() {
        let disabled: ToolPolicy = serde_json::from_str(r#"{"read_image":false}"#).unwrap();
        assert!(!disabled.read_image.enabled);
        assert!(disabled
            .check_image_read("/tmp/attached-image.png")
            .is_err());

        let enabled: ToolPolicy = serde_json::from_str(r#"{"read_image":true}"#).unwrap();
        assert!(enabled.read_image.enabled);
    }

    #[test]
    fn read_outside_workspace_allows_shell_working_directory() {
        let policy = ToolPolicy::default();
        assert!(policy.check_shell("/usr/share").is_ok());
    }

    #[test]
    fn explicitly_allowed_read_root_allows_shell_working_directory() {
        let mut policy = ToolPolicy::default();
        policy.sandbox.allow_read_outside_workspace = false;
        policy.file.allowed_roots = vec!["/usr/share".into()];

        assert!(policy.check_shell("/usr/share").is_ok());
    }

    #[test]
    fn approval_tier_serializes_with_new_names() {
        let value = serde_json::to_value(ToolPolicy::default()).unwrap();
        assert_eq!(value["shell"]["approval"], "on_risk");
        assert_eq!(value["file"]["approval"], "on_write");
        assert_eq!(value["read_image"]["enabled"], true);
        assert_eq!(value["git"]["approval"], "on_risk");
        assert_eq!(value["memory"]["approval"], "on_write");
        assert_eq!(value["planner"]["approval"], "always");
        assert_eq!(value["web"]["approval"], "never");
        assert_eq!(value["web"]["search_provider"], "auto");
        assert_eq!(value["mcp"]["enabled"], false);
        assert_eq!(value["mcp"]["approval"], "always");
        assert_eq!(value["git"]["timeout_sec"], 30);
        assert_eq!(value["network"]["allow"], true);
        assert_eq!(value["sandbox"]["bwrap"], "auto");
        assert_eq!(value["sandbox"]["allow_read_outside_workspace"], true);
        assert_eq!(
            value["sandbox"]["writable_roots"],
            serde_json::json!(["~/.cache", "~/.local", "~/.cargo", "~/.rustup", "/tmp"])
        );
    }

    #[test]
    fn legacy_sandbox_policy_gets_default_writable_roots() {
        let policy: ToolPolicy =
            serde_json::from_str(r#"{"sandbox":{"landlock":true,"bwrap":"auto","rlimits":true}}"#)
                .unwrap();
        assert_eq!(
            policy.sandbox.writable_roots,
            vec!["~/.cache", "~/.local", "~/.cargo", "~/.rustup", "/tmp"]
        );
        assert!(policy.sandbox.allow_read_outside_workspace);
    }

    #[test]
    fn workspace_mode_reads_broadly_but_strict_mode_honors_roots() {
        let mut policy = ToolPolicy::default();
        policy.file.allowed_roots = vec!["/workspace".into()];
        assert!(policy.check_file_read("/etc/passwd").is_ok());

        policy.sandbox.allow_read_outside_workspace = false;
        assert!(policy.check_file_read("/etc/passwd").is_err());
        assert!(policy.check_file_read("/workspace/README.md").is_ok());
    }

    #[test]
    fn image_read_has_its_own_capability_switch_and_legacy_roots() {
        let root =
            std::env::temp_dir().join(format!("agnes-image-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let image = root.join("image.bin");
        std::fs::write(&image, b"image").unwrap();

        let mut policy = ToolPolicy::default();
        policy.sandbox.allow_read_outside_workspace = false;
        policy.file.enabled = false;
        policy.file.allowed_roots = vec![root.to_string_lossy().to_string()];
        assert!(policy.check_image_read(&image.to_string_lossy()).is_ok());

        policy.read_image.enabled = false;
        assert!(policy.check_image_read(&image.to_string_lossy()).is_err());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn planner_capability_can_be_disabled() {
        let mut policy = ToolPolicy::default();
        assert!(policy.check_planner().is_ok());
        policy.planner.enabled = false;
        assert!(policy.check_planner().is_err());
    }

    #[test]
    fn web_capability_requires_both_tool_and_network_access() {
        let mut policy = ToolPolicy::default();
        assert!(policy.check_web().is_ok());
        policy.network.allow = false;
        assert!(policy.check_web().is_err());
        policy.network.allow = true;
        policy.web.enabled = false;
        assert!(policy.check_web().is_err());
    }
}
