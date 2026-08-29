#[cfg(target_os = "android")]
use std::io::Read;
#[cfg(any(target_os = "android", test))]
use std::path::Component;
use std::path::Path;
#[cfg(target_os = "android")]
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(target_os = "android")]
use std::sync::Mutex;

use serde::Serialize;
#[cfg(target_os = "android")]
use serde::{de::DeserializeOwned, Deserialize};
#[cfg(target_os = "android")]
use sha2::{Digest, Sha256};
#[cfg(target_os = "android")]
use tauri::plugin::{Builder as PluginBuilder, PluginApi, PluginHandle, TauriPlugin};
#[cfg(target_os = "android")]
use tauri::{Manager, Wry};
#[cfg(target_os = "android")]
use uuid::Uuid;
#[cfg(target_os = "android")]
use zip::ZipArchive;

use crate::agent::AgentManager;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatus {
    pub platform: String,
    pub execution_mode: String,
    pub state: String,
    pub detail: String,
    pub runtime_version: Option<String>,
    pub abi: Option<String>,
}

pub trait AgentRuntime: Send + Sync {
    fn status(&self) -> AgentRuntimeStatus;

    fn install_package(&self, _package_path: &Path) -> AppResult<AgentRuntimeStatus> {
        Err(AppError::Other(
            "当前平台不支持安装 Android Linux Runtime".into(),
        ))
    }

    fn start(&self) -> AppResult<AgentRuntimeStatus> {
        Err(AppError::Other(
            "当前平台不支持启动 Android Linux Runtime".into(),
        ))
    }

    fn stop(&self) -> AppResult<AgentRuntimeStatus> {
        Ok(self.status())
    }
}

#[cfg(target_os = "android")]
const ANDROID_RUNTIME_PLUGIN_ID: &str = "com.agnes.agent.runtime";

#[cfg(target_os = "android")]
#[derive(Clone)]
pub struct AndroidRuntimePlugin(pub PluginHandle<Wry>);

#[cfg(target_os = "android")]
pub fn android_plugin() -> TauriPlugin<Wry> {
    PluginBuilder::new("android-runtime")
        .setup(|app, api: PluginApi<Wry, ()>| {
            let handle = api.register_android_plugin(ANDROID_RUNTIME_PLUGIN_ID, "RuntimePlugin")?;
            app.manage(AndroidRuntimePlugin(handle));
            Ok(())
        })
        .build()
}

pub struct DesktopSidecarRuntime {
    agent: Arc<AgentManager>,
}

impl DesktopSidecarRuntime {
    pub fn new(agent: Arc<AgentManager>) -> Self {
        Self { agent }
    }
}

impl AgentRuntime for DesktopSidecarRuntime {
    fn status(&self) -> AgentRuntimeStatus {
        let running = self.agent.is_running();
        AgentRuntimeStatus {
            platform: "desktop".into(),
            execution_mode: "python-sidecar".into(),
            state: if running { "ready" } else { "stopped" }.into(),
            detail: if running {
                "桌面 Agent Runtime 已就绪".into()
            } else {
                "桌面 Agent Runtime 未启动".into()
            },
            runtime_version: None,
            abi: None,
        }
    }
}

#[cfg(target_os = "android")]
const MAX_RUNTIME_FILES: usize = 200_000;
#[cfg(target_os = "android")]
const MAX_RUNTIME_BYTES: u64 = 2 * 1024 * 1024 * 1024;
#[cfg(target_os = "android")]
const MAX_RUNTIME_FILE_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(target_os = "android")]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest {
    schema_version: u32,
    runtime_version: String,
    platform: String,
    abi: String,
    rootfs_sha256: String,
    rootfs_size: u64,
    agent_version: String,
}

#[cfg(target_os = "android")]
#[derive(Debug, Clone)]
struct RuntimeSnapshot {
    state: String,
    detail: String,
    runtime_version: Option<String>,
    abi: Option<String>,
}

#[cfg(target_os = "android")]
pub struct AndroidLinuxRuntime {
    root_dir: PathBuf,
    native_plugin: PluginHandle<Wry>,
    snapshot: Mutex<RuntimeSnapshot>,
}

#[cfg(target_os = "android")]
impl AndroidLinuxRuntime {
    pub fn new(data_dir: &Path, native_plugin: PluginHandle<Wry>) -> Self {
        let root_dir = data_dir.join("android-runtime");
        let snapshot = load_snapshot(&root_dir);
        Self {
            root_dir,
            native_plugin,
            snapshot: Mutex::new(snapshot),
        }
    }

    fn native_service_status(&self) -> AppResult<bool> {
        #[derive(Debug, Deserialize)]
        struct NativeServiceStatus {
            running: bool,
        }

        self.native_plugin
            .run_mobile_plugin::<NativeServiceStatus>("getStatus", ())
            .map(|status| status.running)
            .map_err(|error| {
                AppError::Other(format!("无法读取 Android Runtime Service 状态: {error}"))
            })
    }

    fn run_native_command<T: DeserializeOwned>(&self, command: &str) -> AppResult<T> {
        self.native_plugin
            .run_mobile_plugin(command, ())
            .map_err(|error| AppError::Other(format!("Android Runtime Service 调用失败: {error}")))
    }

    fn install_package_inner(&self, package_path: &Path) -> AppResult<RuntimeSnapshot> {
        let package_file = std::fs::File::open(package_path)?;
        let mut archive = ZipArchive::new(package_file).map_err(|error| {
            AppError::Other(format!("无法读取 Android Runtime 安装包: {error}"))
        })?;
        let manifest = read_manifest(&mut archive)?;
        validate_manifest(&manifest)?;

        std::fs::create_dir_all(self.root_dir.join("versions"))?;
        let staging = self
            .root_dir
            .join("versions")
            .join(format!(".staging-{}", Uuid::new_v4()));
        let result = (|| {
            let rootfs_dir = staging.join("rootfs");
            std::fs::create_dir_all(&rootfs_dir)?;
            extract_rootfs(&mut archive, &rootfs_dir)?;

            let (digest, size) = digest_rootfs(&rootfs_dir)?;
            if digest != manifest.rootfs_sha256 || size != manifest.rootfs_size {
                return Err(AppError::Other(format!(
                    "Android Runtime rootfs 校验失败: sha256={digest}, size={size}"
                )));
            }
            std::fs::write(
                staging.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest)?,
            )?;

            let version_dir = self
                .root_dir
                .join("versions")
                .join(&manifest.runtime_version);
            if version_dir.exists() {
                return Err(AppError::Other(format!(
                    "Android Runtime 版本已安装: {}",
                    manifest.runtime_version
                )));
            }
            std::fs::rename(&staging, &version_dir)?;

            let current = std::fs::read_to_string(self.root_dir.join("current")).ok();
            if let Some(previous) = current.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                std::fs::write(self.root_dir.join("previous.tmp"), previous)?;
                std::fs::rename(
                    self.root_dir.join("previous.tmp"),
                    self.root_dir.join("previous"),
                )?;
            }
            std::fs::write(self.root_dir.join("current.tmp"), &manifest.runtime_version)?;
            std::fs::rename(
                self.root_dir.join("current.tmp"),
                self.root_dir.join("current"),
            )?;

            Ok(RuntimeSnapshot {
                state: "installed".into(),
                detail: format!(
                    "Android Linux Runtime {} 已安装，等待 PRoot runner",
                    manifest.runtime_version
                ),
                runtime_version: Some(manifest.runtime_version),
                abi: Some(manifest.abi),
            })
        })();

        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }
}

#[cfg(target_os = "android")]
impl AgentRuntime for AndroidLinuxRuntime {
    fn status(&self) -> AgentRuntimeStatus {
        let service_running = self.native_service_status().unwrap_or(false);
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned");
        if snapshot.runtime_version.is_some() {
            if service_running {
                snapshot.state = "service-running".into();
                snapshot.detail = "Android Runtime 前台服务正在运行，等待 PRoot runner".into();
            } else if snapshot.state == "service-running" {
                snapshot.state = "installed".into();
                snapshot.detail = format!(
                    "Android Linux Runtime {} 已安装，前台服务未运行",
                    snapshot.runtime_version.as_deref().unwrap_or("unknown")
                );
            }
        }
        AgentRuntimeStatus {
            platform: "android".into(),
            execution_mode: "linux-runtime".into(),
            state: snapshot.state.clone(),
            detail: snapshot.detail.clone(),
            runtime_version: snapshot.runtime_version.clone(),
            abi: snapshot.abi.clone(),
        }
    }

    fn install_package(&self, package_path: &Path) -> AppResult<AgentRuntimeStatus> {
        if self.native_service_status().unwrap_or(false) {
            return Err(AppError::Other(
                "Android Runtime Service 正在运行，请先停止后再安装新版本".into(),
            ));
        }
        let snapshot = self.install_package_inner(package_path)?;
        *self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned") = snapshot;
        Ok(self.status())
    }

    fn start(&self) -> AppResult<AgentRuntimeStatus> {
        let status = self.status();
        if status.state == "not-installed" {
            return Err(AppError::Other("请先安装 Android Linux Runtime".into()));
        }
        let _: serde_json::Value = self.run_native_command("start")?;
        Ok(self.status())
    }

    fn stop(&self) -> AppResult<AgentRuntimeStatus> {
        let _: serde_json::Value = self.run_native_command("stop")?;
        Ok(self.status())
    }
}

#[cfg(target_os = "android")]
fn read_manifest<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> AppResult<RuntimeManifest> {
    let mut entry = archive
        .by_name("manifest.json")
        .map_err(|error| AppError::Other(format!("安装包缺少 manifest.json: {error}")))?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(target_os = "android")]
fn validate_manifest(manifest: &RuntimeManifest) -> AppResult<()> {
    if manifest.schema_version != 1 {
        return Err(AppError::Other(format!(
            "不支持的 Android Runtime manifest 版本: {}",
            manifest.schema_version
        )));
    }
    if manifest.platform != "android" {
        return Err(AppError::Other("Runtime 安装包平台不是 Android".into()));
    }
    if !matches!(manifest.abi.as_str(), "arm64-v8a" | "x86_64") {
        return Err(AppError::Other(format!(
            "不支持的 Android ABI: {}",
            manifest.abi
        )));
    }
    if !valid_version(&manifest.runtime_version) {
        return Err(AppError::Other("Runtime 版本号包含非法路径字符".into()));
    }
    if manifest.agent_version.trim().is_empty() {
        return Err(AppError::Other("Runtime manifest 缺少 agentVersion".into()));
    }
    if manifest.rootfs_sha256.len() != 64
        || !manifest
            .rootfs_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::Other(
            "Runtime manifest 的 rootfsSha256 无效".into(),
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "android", test))]
fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[cfg(target_os = "android")]
fn extract_rootfs<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    rootfs_dir: &Path,
) -> AppResult<()> {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AppError::Other(format!("无法读取 Runtime 安装包条目: {error}")))?;
        let name = entry.name().to_string();
        let Some(relative_name) = name.strip_prefix("rootfs/") else {
            continue;
        };
        if relative_name.is_empty() || !safe_relative_path(relative_name) {
            return Err(AppError::Other(format!(
                "Runtime 安装包包含非法路径: {name}"
            )));
        }
        let target = rootfs_dir.join(relative_name);
        if entry.is_dir() {
            std::fs::create_dir_all(target)?;
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(AppError::Other(format!(
                "Runtime 安装包不允许符号链接: {name}"
            )));
        }
        file_count += 1;
        if file_count > MAX_RUNTIME_FILES {
            return Err(AppError::Other("Runtime 安装包文件数量超出限制".into()));
        }
        let declared_size = entry.size();
        if declared_size > MAX_RUNTIME_FILE_BYTES
            || total_bytes.saturating_add(declared_size) > MAX_RUNTIME_BYTES
        {
            return Err(AppError::Other("Runtime 安装包大小超出限制".into()));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(target)?;
        let copied = std::io::copy(&mut entry, &mut output)?;
        if copied != declared_size {
            return Err(AppError::Other(format!(
                "Runtime 安装包条目大小不一致: {name}"
            )));
        }
        total_bytes += copied;
    }
    if file_count == 0 {
        return Err(AppError::Other("Runtime 安装包不包含 rootfs".into()));
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn digest_rootfs(rootfs_dir: &Path) -> AppResult<(String, u64)> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(rootfs_dir) {
        let entry = entry.map_err(|error| AppError::Other(format!("扫描 rootfs 失败: {error}")))?;
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(rootfs_dir)
                .map_err(|error| AppError::Other(format!("rootfs 路径无效: {error}")))?
                .to_string_lossy()
                .replace('\\', "/");
            entries.push((relative, entry.path().to_path_buf()));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for (relative, path) in entries {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        let bytes = std::fs::read(path)?;
        total += bytes.len() as u64;
        hasher.update(&bytes);
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

#[cfg(target_os = "android")]
fn load_snapshot(root_dir: &Path) -> RuntimeSnapshot {
    let current = std::fs::read_to_string(root_dir.join("current"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| valid_version(value));
    if let Some(version) = current {
        let version_dir = root_dir.join("versions").join(&version);
        if version_dir.join("rootfs").is_dir() && version_dir.join("manifest.json").is_file() {
            let abi = std::fs::read_to_string(version_dir.join("manifest.json"))
                .ok()
                .and_then(|bytes| serde_json::from_str::<RuntimeManifest>(&bytes).ok())
                .map(|manifest| manifest.abi);
            return RuntimeSnapshot {
                state: "installed".into(),
                detail: format!("Android Linux Runtime {version} 已安装，等待 PRoot runner"),
                runtime_version: Some(version),
                abi,
            };
        }
    }
    RuntimeSnapshot {
        state: "not-installed".into(),
        detail: "Android Linux Runtime 尚未安装".into(),
        runtime_version: None,
        abi: None,
    }
}

#[cfg(any(target_os = "android", test))]
fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::{safe_relative_path, valid_version};

    #[test]
    fn runtime_versions_are_safe_directory_names() {
        assert!(valid_version("ubuntu-24.04.1"));
        assert!(!valid_version("../current"));
        assert!(!valid_version(""));
    }

    #[test]
    fn rootfs_paths_cannot_escape_install_directory() {
        assert!(safe_relative_path("usr/bin/python3"));
        assert!(!safe_relative_path("../outside"));
        assert!(!safe_relative_path("/etc/passwd"));
    }
}
