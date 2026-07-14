use serde::{Deserialize, Serialize};

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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ShellPolicy {
    pub enabled: bool,
    /// always | never（是否需人工审批）
    pub approval: String,
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
            approval: "always".into(),
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
    /// write | never（写操作需审批）
    pub approval: String,
    pub allowed_roots: Vec<String>,
}

impl Default for FilePolicy {
    fn default() -> Self {
        FilePolicy {
            enabled: true,
            approval: "write".into(),
            allowed_roots: vec!["~/Projects".into(), "~/.agnes".into()],
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct GitPolicy {
    pub enabled: bool,
    /// push | never（push 需审批）
    pub approval: String,
}

impl Default for GitPolicy {
    fn default() -> Self {
        GitPolicy {
            enabled: true,
            approval: "push".into(),
        }
    }
}
