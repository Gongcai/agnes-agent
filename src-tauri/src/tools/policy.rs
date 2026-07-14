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
}
