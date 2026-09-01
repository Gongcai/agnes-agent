#[cfg(target_os = "android")]
use std::collections::HashSet;
#[cfg(target_os = "android")]
use std::io::Read;
#[cfg(target_os = "android")]
use std::net::IpAddr;
#[cfg(target_os = "android")]
use std::os::unix::fs::{symlink, PermissionsExt};
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
#[cfg(target_os = "android")]
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
#[cfg(target_os = "android")]
use crate::mcp::McpManager;
#[cfg(target_os = "android")]
use crate::secrets::SharedSecretStore;
#[cfg(target_os = "android")]
use crate::storage::StorageService;
#[cfg(target_os = "android")]
use crate::tools::sandbox::AndroidProotRuntimeConfig;
#[cfg(target_os = "android")]
use crate::tools::terminal::UserTerminalManager;
use crate::tools::terminal::{UserTerminalInfo, UserTerminalPoll};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatus {
    pub platform: String,
    pub execution_mode: String,
    pub state: String,
    pub detail: String,
    pub runtime_version: Option<String>,
    pub previous_runtime_version: Option<String>,
    pub abi: Option<String>,
    pub bundled_runtime_available: bool,
    pub proot_available: bool,
}

pub trait AgentRuntime: Send + Sync {
    fn status(&self) -> AgentRuntimeStatus;

    fn install_package(&self, _package_path: &Path) -> AppResult<AgentRuntimeStatus> {
        Err(AppError::Other(
            "当前平台不支持安装 Android Linux Runtime".into(),
        ))
    }

    fn rollback_package(&self) -> AppResult<AgentRuntimeStatus> {
        Err(AppError::Other(
            "当前平台不支持回滚 Android Linux Runtime".into(),
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

    fn user_terminal_list(&self) -> AppResult<Vec<UserTerminalInfo>> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
    }

    fn user_terminal_create(
        &self,
        _cwd: Option<&str>,
        _rows: u16,
        _cols: u16,
    ) -> AppResult<UserTerminalPoll> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
    }

    fn user_terminal_poll(&self, _id: &str, _cursor: u64) -> AppResult<UserTerminalPoll> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
    }

    fn user_terminal_write(&self, _id: &str, _input: &str) -> AppResult<()> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
    }

    fn user_terminal_resize(&self, _id: &str, _rows: u16, _cols: u16) -> AppResult<()> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
    }

    fn user_terminal_interrupt(&self, _id: &str) -> AppResult<()> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
    }

    fn user_terminal_close(&self, _id: &str) -> AppResult<()> {
        Err(AppError::Other("当前平台不支持用户 Linux 终端".into()))
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
            previous_runtime_version: None,
            abi: None,
            bundled_runtime_available: false,
            proot_available: false,
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
    previous_runtime_version: Option<String>,
    abi: Option<String>,
}

#[cfg(target_os = "android")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeRuntimeEnvironment {
    abi: String,
    bundled_runtime_available: bool,
    proot_available: bool,
    proot_path: String,
    proot_loader_available: bool,
    proot_loader_path: String,
    dns_servers: Vec<String>,
}

#[cfg(target_os = "android")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AndroidRuntimeCapabilityProfile {
    schema_version: u32,
    link2symlink_enabled: bool,
    proc: AndroidProcCapabilities,
    filesystems: AndroidFilesystemCapabilities,
}

#[cfg(target_os = "android")]
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct AndroidProcCapabilities {
    version_readable: bool,
    stat_readable: bool,
    loadavg_readable: bool,
    uptime_readable: bool,
    vmstat_readable: bool,
}

#[cfg(target_os = "android")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AndroidFilesystemCapabilities {
    rootfs: FilesystemCapabilities,
    tmp: FilesystemCapabilities,
    environment_store: FilesystemCapabilities,
    home_workspace: FilesystemCapabilities,
    project_workspace: FilesystemCapabilities,
}

#[cfg(target_os = "android")]
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct FilesystemCapabilities {
    read_write_rename: bool,
    symlink: bool,
    hardlink: bool,
    direct_exec: bool,
}

#[cfg(target_os = "android")]
#[derive(Debug)]
struct AndroidProotProbe {
    link2symlink_enabled: bool,
    proc: AndroidProcCapabilities,
}

#[cfg(target_os = "android")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreparedBundledRuntime {
    abi: String,
    package_path: String,
    package_size: u64,
}

#[cfg(target_os = "android")]
pub struct AndroidLinuxRuntime {
    root_dir: PathBuf,
    native_plugin: PluginHandle<Wry>,
    agent: Arc<AgentManager>,
    db: DbActorHandle,
    app_handle: tauri::AppHandle,
    mcp: Arc<McpManager>,
    secrets: SharedSecretStore,
    home_workspace_dir: PathBuf,
    storage: Arc<StorageService>,
    snapshot: Mutex<RuntimeSnapshot>,
    user_terminals: Arc<UserTerminalManager>,
    user_terminal_config: Mutex<Option<AndroidProotRuntimeConfig>>,
}

#[cfg(target_os = "android")]
impl AndroidLinuxRuntime {
    pub fn new(
        data_dir: &Path,
        native_plugin: PluginHandle<Wry>,
        agent: Arc<AgentManager>,
        db: DbActorHandle,
        app_handle: tauri::AppHandle,
        mcp: Arc<McpManager>,
        secrets: SharedSecretStore,
        home_workspace_dir: PathBuf,
        storage: Arc<StorageService>,
    ) -> Self {
        let root_dir = data_dir.join("android-runtime");
        let snapshot = load_snapshot(&root_dir);
        Self {
            root_dir,
            native_plugin,
            agent,
            db,
            app_handle,
            mcp,
            secrets,
            home_workspace_dir,
            storage,
            snapshot: Mutex::new(snapshot),
            user_terminals: Arc::new(UserTerminalManager::default()),
            user_terminal_config: Mutex::new(None),
        }
    }

    fn current_rootfs(&self) -> AppResult<PathBuf> {
        let version = std::fs::read_to_string(self.root_dir.join("current"))
            .map_err(|_| AppError::Other("请先安装 Android Linux Runtime".into()))?;
        let version = version.trim();
        if !valid_version(version) {
            return Err(AppError::Other("当前 Runtime 版本记录无效".into()));
        }
        let rootfs_dir = self.root_dir.join("versions").join(version).join("rootfs");
        if !rootfs_dir.is_dir() {
            return Err(AppError::Other("当前 Runtime rootfs 不存在".into()));
        }
        Ok(rootfs_dir)
    }

    fn update_install_progress(&self, detail: impl Into<String>) {
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned");
        snapshot.state = "installing".into();
        snapshot.detail = detail.into();
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

    fn runtime_environment(&self) -> AppResult<NativeRuntimeEnvironment> {
        let environment: NativeRuntimeEnvironment =
            self.run_native_command("getRuntimeEnvironment")?;
        if environment.abi != target_android_abi() {
            return Err(AppError::Other(format!(
                "Android 原生 Runtime ABI 与 Rust 目标不匹配: native={}, rust={}",
                environment.abi,
                target_android_abi()
            )));
        }
        Ok(environment)
    }

    fn install_bundled_runtime(&self) -> AppResult<()> {
        self.update_install_progress("正在从 APK 复制内置 Runtime 安装包");
        let prepared: PreparedBundledRuntime = self.run_native_command("prepareBundledRuntime")?;
        if prepared.abi != target_android_abi() {
            return Err(AppError::Other(format!(
                "内置 Runtime ABI 与当前设备不匹配: package={}, device={}",
                prepared.abi,
                target_android_abi()
            )));
        }
        if prepared.package_size == 0 {
            return Err(AppError::Other("内置 Runtime 安装包为空".into()));
        }
        let package_path = PathBuf::from(prepared.package_path);
        if !package_path.is_absolute() || !package_path.is_file() {
            return Err(AppError::Other("内置 Runtime 安装包路径无效".into()));
        }
        self.update_install_progress("内置 Runtime 安装包已准备，正在读取清单");
        let result = self.install_package_inner(&package_path);
        let _ = std::fs::remove_file(&package_path);
        let snapshot = result?;
        *self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned") = snapshot;
        Ok(())
    }

    fn install_package_inner(&self, package_path: &Path) -> AppResult<RuntimeSnapshot> {
        let package_file = std::fs::File::open(package_path)?;
        let mut archive = ZipArchive::new(package_file).map_err(|error| {
            AppError::Other(format!("无法读取 Android Runtime 安装包: {error}"))
        })?;
        let manifest = read_manifest(&mut archive)?;
        validate_manifest(&manifest)?;
        if manifest.abi != target_android_abi() {
            return Err(AppError::Other(format!(
                "Runtime ABI 与当前设备不匹配: package={}, device={}",
                manifest.abi,
                target_android_abi()
            )));
        }

        self.update_install_progress(format!(
            "正在解压 Android Linux Runtime {}",
            manifest.runtime_version
        ));
        std::fs::create_dir_all(self.root_dir.join("versions"))?;
        let staging = self
            .root_dir
            .join("versions")
            .join(format!(".staging-{}", Uuid::new_v4()));
        let result = (|| {
            let rootfs_dir = staging.join("rootfs");
            std::fs::create_dir_all(&rootfs_dir)?;
            extract_rootfs(&mut archive, &rootfs_dir)?;
            ensure_rootfs_runtime_layout(&rootfs_dir)?;

            self.update_install_progress("Runtime 已解压，正在校验文件完整性");
            let (digest, size) = digest_rootfs(&rootfs_dir)?;
            if digest != manifest.rootfs_sha256 || size != manifest.rootfs_size {
                return Err(AppError::Other(format!(
                    "Android Runtime rootfs 校验失败: sha256={digest}, size={size}"
                )));
            }
            enforce_guest_python_policy(&rootfs_dir)?;
            self.update_install_progress("Runtime 校验通过，正在写入版本信息");
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

            let current = read_runtime_pointer(&self.root_dir.join("current"), "current").ok();
            if let Some(previous) = current.as_deref() {
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
                previous_runtime_version: current,
                abi: Some(manifest.abi),
            })
        })();

        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }

    fn rollback_package_inner(&self) -> AppResult<RuntimeSnapshot> {
        let current = read_runtime_pointer(&self.root_dir.join("current"), "current")?;
        let previous = read_runtime_pointer(&self.root_dir.join("previous"), "previous")?;
        if current == previous {
            return Err(AppError::Other("Android Runtime 的上一版本记录无效".into()));
        }

        let previous_snapshot = installed_runtime_snapshot(&self.root_dir, &previous)?;
        swap_runtime_pointers(&self.root_dir, &current, &previous)?;
        Ok(RuntimeSnapshot {
            state: "installed".into(),
            detail: format!(
                "Android Linux Runtime 已回滚到 {}，等待 PRoot runner",
                previous
            ),
            runtime_version: Some(previous),
            previous_runtime_version: Some(current),
            abi: previous_snapshot.abi,
        })
    }
}

#[cfg(target_os = "android")]
impl AgentRuntime for AndroidLinuxRuntime {
    fn status(&self) -> AgentRuntimeStatus {
        let environment_result = self.runtime_environment();
        let environment_error = environment_result.as_ref().err().map(ToString::to_string);
        let environment = environment_result.ok();
        let service_running = self.native_service_status().unwrap_or(false);
        let agent_running = self.agent.is_running();
        let agent_connected = self.agent.is_connected();
        let mut snapshot = self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned");
        if snapshot.runtime_version.is_some() {
            if service_running && agent_running && agent_connected {
                snapshot.state = "ready".into();
                snapshot.detail = "Android Linux Runtime 与 Agent 已就绪".into();
            } else if service_running && agent_running {
                snapshot.state = "starting".into();
                snapshot.detail = "Android Linux Runtime 正在等待 Agent Protocol 握手".into();
            } else if service_running {
                snapshot.state = "service-running".into();
                snapshot.detail = "Android Runtime 前台服务正在运行，等待 Agent".into();
            } else if matches!(
                snapshot.state.as_str(),
                "service-running" | "starting" | "ready"
            ) {
                snapshot.state = "installed".into();
                snapshot.detail = format!(
                    "Android Linux Runtime {} 已安装，前台服务未运行",
                    snapshot.runtime_version.as_deref().unwrap_or("unknown")
                );
            }
        } else if snapshot.state != "installing"
            && environment
                .as_ref()
                .is_some_and(|value| value.bundled_runtime_available && value.proot_available)
        {
            snapshot.state = "bundled-available".into();
            snapshot.detail = "应用已内置 Android Linux Runtime，首次启动时将自动安装".into();
            snapshot.abi = environment.as_ref().map(|value| value.abi.clone());
        } else if snapshot.state != "installing" && environment_error.is_some() {
            snapshot.state = "error".into();
            snapshot.detail = format!(
                "无法读取 Android Runtime 环境：{}",
                environment_error.as_deref().unwrap_or("unknown error")
            );
        }
        AgentRuntimeStatus {
            platform: "android".into(),
            execution_mode: "linux-runtime".into(),
            state: snapshot.state.clone(),
            detail: snapshot.detail.clone(),
            runtime_version: snapshot.runtime_version.clone(),
            previous_runtime_version: snapshot.previous_runtime_version.clone(),
            abi: snapshot.abi.clone(),
            bundled_runtime_available: environment
                .as_ref()
                .is_some_and(|value| value.bundled_runtime_available),
            proot_available: environment
                .as_ref()
                .is_some_and(|value| value.proot_available),
        }
    }

    fn install_package(&self, package_path: &Path) -> AppResult<AgentRuntimeStatus> {
        if self.native_service_status()? {
            return Err(AppError::Other(
                "Android Runtime Service 正在运行，请先停止后再安装新版本".into(),
            ));
        }
        self.update_install_progress("正在读取 Android Runtime 安装包");
        let snapshot = match self.install_package_inner(package_path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                *self
                    .snapshot
                    .lock()
                    .expect("runtime snapshot lock poisoned") = load_snapshot(&self.root_dir);
                return Err(error);
            }
        };
        *self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned") = snapshot;
        // A successful manual install should leave the Runtime usable immediately.
        self.start()
    }

    fn rollback_package(&self) -> AppResult<AgentRuntimeStatus> {
        if self.native_service_status()? {
            return Err(AppError::Other(
                "Android Runtime Service 正在运行，请先停止后再回滚版本".into(),
            ));
        }
        let snapshot = self.rollback_package_inner()?;
        *self
            .snapshot
            .lock()
            .expect("runtime snapshot lock poisoned") = snapshot;
        Ok(self.status())
    }

    fn start(&self) -> AppResult<AgentRuntimeStatus> {
        if self.agent.is_running() {
            return Ok(self.status());
        }
        let status = self.status();
        if status.runtime_version.is_none() {
            if !status.bundled_runtime_available || !status.proot_available {
                return Err(AppError::Other(
                    "当前 APK 未包含适配本设备 ABI 的完整 Android Linux Runtime".into(),
                ));
            }
            {
                let mut snapshot = self
                    .snapshot
                    .lock()
                    .expect("runtime snapshot lock poisoned");
                snapshot.state = "installing".into();
                snapshot.detail = "正在安装内置 Android Linux Runtime".into();
            }
            if let Err(error) = self.install_bundled_runtime() {
                *self
                    .snapshot
                    .lock()
                    .expect("runtime snapshot lock poisoned") = load_snapshot(&self.root_dir);
                return Err(error);
            }
        }
        {
            let mut snapshot = self
                .snapshot
                .lock()
                .expect("runtime snapshot lock poisoned");
            snapshot.state = "starting".into();
            snapshot.detail = "Runtime 已安装，正在启动 PRoot 与 Agent".into();
        }
        let rootfs_dir = self.current_rootfs()?;
        let environment = self.runtime_environment()?;
        if !environment.proot_available {
            return Err(AppError::Other("当前 APK 未包含 PRoot 原生库".into()));
        }
        let proot_path = PathBuf::from(environment.proot_path);
        if !proot_path.is_absolute() || !proot_path.is_file() {
            return Err(AppError::Other("Android PRoot 原生库路径无效".into()));
        }
        if !environment.proot_loader_available {
            return Err(AppError::Other("当前 APK 未包含 PRoot loader".into()));
        }
        let proot_loader_path = PathBuf::from(environment.proot_loader_path);
        if !proot_loader_path.is_absolute() || !proot_loader_path.is_file() {
            return Err(AppError::Other("Android PRoot loader 路径无效".into()));
        }
        enforce_guest_python_policy(&rootfs_dir)?;
        sync_android_resolv_conf(&rootfs_dir, &environment.dns_servers)?;
        let proot_tmp_dir = self.root_dir.join("tmp");
        std::fs::create_dir_all(&proot_tmp_dir).map_err(|error| {
            AppError::Other(format!("无法创建 Android PRoot 临时目录: {error}"))
        })?;
        let proot_shm_dir = proot_tmp_dir.join("shm");
        std::fs::create_dir_all(&proot_shm_dir).map_err(|error| {
            AppError::Other(format!("无法创建 Android PRoot 共享内存目录: {error}"))
        })?;
        // PRoot may expose the bind source with a different guest UID. Keep
        // the app-private semaphore directory writable after that mapping.
        std::fs::set_permissions(&proot_shm_dir, std::fs::Permissions::from_mode(0o1777))?;
        let android_workspace_root = crate::tools::workspace::android_workspace_root(
            self.root_dir
                .parent()
                .ok_or_else(|| AppError::Other("Android Runtime 数据目录无效".into()))?,
        );
        std::fs::create_dir_all(&android_workspace_root).map_err(|error| {
            AppError::Other(format!("无法创建 Android Linux 用户工作区: {error}"))
        })?;
        let environment_store = self.root_dir.join("user/environments");
        let user_home = self.root_dir.join("user/home");
        let uv_cache_dir = self.root_dir.join("user/cache/uv");
        std::fs::create_dir_all(&environment_store).map_err(|error| {
            AppError::Other(format!("无法创建 Android Python 环境目录: {error}"))
        })?;
        std::fs::create_dir_all(&user_home).map_err(|error| {
            AppError::Other(format!("无法创建 Android Linux 用户目录: {error}"))
        })?;
        std::fs::create_dir_all(&uv_cache_dir)
            .map_err(|error| AppError::Other(format!("无法创建 Android uv 缓存目录: {error}")))?;
        let proot_config = AndroidProotRuntimeConfig::new(
            proot_path,
            proot_loader_path,
            proot_tmp_dir,
            rootfs_dir.clone(),
            android_workspace_root.clone(),
            false,
        )
        .with_runtime_mounts(
            self.root_dir.clone(),
            environment_store.clone(),
            self.root_dir.join("user/cache"),
        )
        .with_user_workspace(android_workspace_root.clone())
        .with_user_home(user_home);
        let proot_probe = probe_android_rootfs(&proot_config)?;
        let capability_profile = AndroidRuntimeCapabilityProfile {
            schema_version: 1,
            link2symlink_enabled: proot_probe.link2symlink_enabled,
            proc: proot_probe.proc,
            filesystems: AndroidFilesystemCapabilities {
                rootfs: probe_filesystem_capabilities(&rootfs_dir),
                tmp: probe_filesystem_capabilities(&proot_tmp_dir),
                environment_store: probe_filesystem_capabilities(&environment_store),
                home_workspace: probe_filesystem_capabilities(&self.home_workspace_dir),
                project_workspace: probe_filesystem_capabilities(&android_workspace_root),
            },
        };
        persist_android_runtime_capabilities(&self.root_dir, &capability_profile)?;
        let proot_config =
            proot_config.with_link2symlink_enabled(capability_profile.link2symlink_enabled);
        let _: serde_json::Value = self.run_native_command("start")?;
        if let Err(error) = self.agent.start_android(
            self.db.clone(),
            self.app_handle.clone(),
            self.mcp.clone(),
            self.secrets.clone(),
            self.home_workspace_dir.clone(),
            self.storage.clone(),
            proot_config.clone(),
        ) {
            let _ = self.run_native_command::<serde_json::Value>("stop");
            return Err(error);
        }
        *self
            .user_terminal_config
            .lock()
            .expect("user terminal config lock poisoned") = Some(proot_config);
        Ok(self.status())
    }

    fn stop(&self) -> AppResult<AgentRuntimeStatus> {
        self.user_terminals.stop_all();
        *self
            .user_terminal_config
            .lock()
            .expect("user terminal config lock poisoned") = None;
        self.agent.shutdown();
        let _: serde_json::Value = self.run_native_command("stop")?;
        Ok(self.status())
    }

    fn user_terminal_list(&self) -> AppResult<Vec<UserTerminalInfo>> {
        Ok(self.user_terminals.list())
    }

    fn user_terminal_create(
        &self,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
    ) -> AppResult<UserTerminalPoll> {
        let config = self
            .user_terminal_config
            .lock()
            .expect("user terminal config lock poisoned")
            .clone()
            .ok_or_else(|| AppError::Other("请先启动 Android Linux Runtime".into()))?;
        let (command, host_cwd, guest_cwd) = config.user_command_spec(cwd)?;
        self.user_terminals
            .create(command, &host_cwd, &guest_cwd, rows, cols, 2 * 1024 * 1024)
    }

    fn user_terminal_poll(&self, id: &str, cursor: u64) -> AppResult<UserTerminalPoll> {
        self.user_terminals.poll(id, cursor)
    }

    fn user_terminal_write(&self, id: &str, input: &str) -> AppResult<()> {
        if input.len() > 64 * 1024 {
            return Err(AppError::Other("终端输入超过 64 KiB".into()));
        }
        self.user_terminals.write(id, input)
    }

    fn user_terminal_resize(&self, id: &str, rows: u16, cols: u16) -> AppResult<()> {
        self.user_terminals.resize(id, rows, cols)
    }

    fn user_terminal_interrupt(&self, id: &str) -> AppResult<()> {
        self.user_terminals.signal_interrupt(id)
    }

    fn user_terminal_close(&self, id: &str) -> AppResult<()> {
        self.user_terminals.close(id)
    }
}

#[cfg(target_os = "android")]
fn sync_android_resolv_conf(rootfs_dir: &Path, dns_servers: &[String]) -> AppResult<()> {
    let servers = dns_servers
        .iter()
        .filter_map(|server| server.parse::<IpAddr>().ok())
        .map(|server| format!("nameserver {server}"))
        .collect::<Vec<_>>();
    if servers.is_empty() {
        return Err(AppError::Other(
            "无法读取 Android 当前网络的 DNS 服务器".into(),
        ));
    }

    let etc_dir = rootfs_dir.join("etc");
    std::fs::create_dir_all(&etc_dir)?;
    let temporary = etc_dir.join("resolv.conf.agnes.tmp");
    let contents = format!(
        "# Managed by Agnes from Android ConnectivityManager.\n{}\n",
        servers.join("\n")
    );
    std::fs::write(&temporary, contents)?;
    std::fs::rename(temporary, etc_dir.join("resolv.conf"))?;
    Ok(())
}

#[cfg(target_os = "android")]
fn ensure_rootfs_runtime_layout(rootfs_dir: &Path) -> AppResult<()> {
    // Empty directories are easy to lose while preparing a ZIP rootfs. Keep
    // the targets present because Android cannot provide /dev/shm for PRoot.
    create_safe_directories(rootfs_dir, Path::new("dev/shm"))?;
    create_safe_directories(rootfs_dir, Path::new("tmp"))?;
    std::fs::set_permissions(
        rootfs_dir.join("dev/shm"),
        std::fs::Permissions::from_mode(0o1777),
    )?;
    std::fs::set_permissions(
        rootfs_dir.join("tmp"),
        std::fs::Permissions::from_mode(0o1777),
    )?;
    Ok(())
}

#[cfg(any(target_os = "android", test))]
const LEGACY_AGNES_PIP_CONFIG: &str = "[global]\nbreak-system-packages = true\n";

#[cfg(any(target_os = "android", test))]
fn enforce_guest_python_policy(rootfs_dir: &Path) -> AppResult<()> {
    let pip_conf = rootfs_dir.join("etc/pip.conf");
    match std::fs::read_to_string(&pip_conf) {
        Ok(contents) if contents == LEGACY_AGNES_PIP_CONFIG => {
            std::fs::remove_file(pip_conf)?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn probe_android_rootfs(config: &AndroidProotRuntimeConfig) -> AppResult<AndroidProotProbe> {
    let minimal = run_android_rootfs_probe(config, false)?;
    if minimal.status.success() {
        return Ok(AndroidProotProbe {
            link2symlink_enabled: false,
            proc: parse_android_proc_capabilities(&minimal.stdout),
        });
    }

    let compatibility = run_android_rootfs_probe(config, true)?;
    if compatibility.status.success() {
        return Ok(AndroidProotProbe {
            link2symlink_enabled: true,
            proc: parse_android_proc_capabilities(&compatibility.stdout),
        });
    }

    Err(AppError::Other(format!(
        "Android Linux Runtime 启动探测失败（minimal: {}; link2symlink fallback: {}）",
        proot_probe_failure_detail(&minimal),
        proot_probe_failure_detail(&compatibility)
    )))
}

#[cfg(target_os = "android")]
fn run_android_rootfs_probe(
    config: &AndroidProotRuntimeConfig,
    link2symlink_enabled: bool,
) -> AppResult<std::process::Output> {
    let mut command = std::process::Command::new(config.proot_path());
    let mut args = config.base_args();
    if link2symlink_enabled {
        args.insert(0, "--link2symlink".into());
    }
    command
        .env_clear()
        .args(args)
        .args([
            "-w",
            "/root",
            "/usr/bin/python3",
            "-c",
            r#"import multiprocessing
queue = multiprocessing.SimpleQueue()
queue.close()
readable = []
for name in ("version", "stat", "loadavg", "uptime", "vmstat"):
    try:
        with open("/proc/" + name, "rb") as handle:
            handle.read(1)
        readable.append(name)
    except OSError:
        pass
print("AGNES_PROC_READABLE=" + ",".join(readable))
print("AGNES_PROBE_OK=1")"#,
        ])
        .envs(config.environment())
        // Android's seccomp acceleration can deny the IPC operations used by
        // glibc's POSIX semaphore implementation inside the PRoot guest.
        .env("PROOT_NO_SECCOMP", "1")
        .env("LD_LIBRARY_PATH", "")
        .env("HOME", "/root")
        .env("TERM", "dumb")
        .env("LANG", "C.UTF-8")
        .env("LC_ALL", "C.UTF-8")
        .env("TMPDIR", "/tmp")
        .env(
            "PATH",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games",
        )
        .output()
        .map_err(|error| AppError::Other(format!("Android PRoot 启动探测失败: {error}")))
}

#[cfg(target_os = "android")]
fn proot_probe_failure_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        format!("exit code {:?}", output.status.code())
    } else {
        stderr
    }
}

#[cfg(target_os = "android")]
fn parse_android_proc_capabilities(stdout: &[u8]) -> AndroidProcCapabilities {
    let output = String::from_utf8_lossy(stdout);
    let readable = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("AGNES_PROC_READABLE="))
        .unwrap_or_default()
        .split(',')
        .collect::<HashSet<_>>();
    AndroidProcCapabilities {
        version_readable: readable.contains("version"),
        stat_readable: readable.contains("stat"),
        loadavg_readable: readable.contains("loadavg"),
        uptime_readable: readable.contains("uptime"),
        vmstat_readable: readable.contains("vmstat"),
    }
}

#[cfg(target_os = "android")]
fn probe_filesystem_capabilities(directory: &Path) -> FilesystemCapabilities {
    let mut capabilities = FilesystemCapabilities::default();
    let probe_dir = directory.join(format!(".agnes-capability-{}", Uuid::new_v4()));
    if std::fs::create_dir(&probe_dir).is_err() {
        return capabilities;
    }

    let source = probe_dir.join("source");
    let renamed = probe_dir.join("renamed");
    if std::fs::write(&source, b"agnes-capability")
        .and_then(|_| std::fs::read(&source))
        .is_ok_and(|contents| contents == b"agnes-capability")
        && std::fs::rename(&source, &renamed).is_ok()
        && std::fs::read(&renamed).is_ok_and(|contents| contents == b"agnes-capability")
    {
        capabilities.read_write_rename = true;
    }

    let symlink_path = probe_dir.join("symlink");
    if symlink("renamed", &symlink_path).is_ok()
        && std::fs::read_link(&symlink_path).is_ok_and(|target| target == Path::new("renamed"))
    {
        capabilities.symlink = true;
    }

    let hardlink_path = probe_dir.join("hardlink");
    if std::fs::hard_link(&renamed, &hardlink_path).is_ok() {
        capabilities.hardlink = true;
    }

    let executable = probe_dir.join("exec-probe.sh");
    if std::fs::write(&executable, b"#!/system/bin/sh\nexit 0\n").is_ok()
        && std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).is_ok()
    {
        capabilities.direct_exec = std::process::Command::new(&executable)
            .env_clear()
            .status()
            .is_ok_and(|status| status.success());
    }

    let _ = std::fs::remove_dir_all(probe_dir);
    capabilities
}

#[cfg(target_os = "android")]
fn persist_android_runtime_capabilities(
    root_dir: &Path,
    profile: &AndroidRuntimeCapabilityProfile,
) -> AppResult<()> {
    let temporary = root_dir.join("capabilities.json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(profile)?)?;
    std::fs::rename(temporary, root_dir.join("capabilities.json"))?;
    Ok(())
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

#[cfg(target_os = "android")]
fn target_android_abi() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64-v8a",
        "x86_64" => "x86_64",
        _ => "unsupported",
    }
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
    let mut extracted_paths = HashSet::new();
    let mut directory_modes = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AppError::Other(format!("无法读取 Runtime 安装包条目: {error}")))?;
        let name = entry.name().to_string();
        let Some(relative_name) = name.strip_prefix("rootfs/") else {
            continue;
        };
        if relative_name.is_empty() {
            if entry.is_dir() {
                continue;
            }
            return Err(AppError::Other("Runtime rootfs 根条目必须是目录".into()));
        }
        if !safe_relative_path(relative_name) {
            return Err(AppError::Other(format!(
                "Runtime 安装包包含非法路径: {name}"
            )));
        }
        if !extracted_paths.insert(relative_name.to_owned()) {
            return Err(AppError::Other(format!(
                "Runtime 安装包包含重复路径: {name}"
            )));
        }
        let relative_path = Path::new(relative_name);
        let target = rootfs_dir.join(relative_name);
        if entry.is_dir() {
            create_safe_directories(rootfs_dir, relative_path)?;
            if let Some(mode) = entry.unix_mode() {
                directory_modes.push((target, mode & 0o1777));
            }
            continue;
        }
        create_safe_parent(rootfs_dir, relative_path)?;
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
        let unix_mode = entry.unix_mode().unwrap_or(0o100644);
        let file_type = unix_mode & 0o170000;
        if file_type == 0o120000 {
            let mut link_bytes = Vec::new();
            entry.read_to_end(&mut link_bytes)?;
            let link_target = std::str::from_utf8(&link_bytes)
                .map_err(|_| AppError::Other(format!("Runtime 软链接目标不是 UTF-8: {name}")))?;
            if !safe_symlink_target(relative_path, link_target) {
                return Err(AppError::Other(format!(
                    "Runtime 软链接目标越出 rootfs: {name} -> {link_target}"
                )));
            }
            symlink(link_target, &target)?;
            total_bytes += link_bytes.len() as u64;
            continue;
        }
        if file_type != 0 && file_type != 0o100000 {
            return Err(AppError::Other(format!(
                "Runtime 安装包包含不支持的文件类型: {name}"
            )));
        }
        let mut output = std::fs::File::create(&target)?;
        let copied = std::io::copy(&mut entry, &mut output)?;
        if copied != declared_size {
            return Err(AppError::Other(format!(
                "Runtime 安装包条目大小不一致: {name}"
            )));
        }
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(unix_mode & 0o777))?;
        total_bytes += copied;
    }
    directory_modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in directory_modes {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    if file_count == 0 {
        return Err(AppError::Other("Runtime 安装包不包含 rootfs".into()));
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn create_safe_parent(rootfs_dir: &Path, relative_path: &Path) -> AppResult<()> {
    if let Some(parent) = relative_path.parent() {
        create_safe_directories(rootfs_dir, parent)?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn create_safe_directories(rootfs_dir: &Path, relative_path: &Path) -> AppResult<()> {
    let mut current = rootfs_dir.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(name) = component else {
            return Err(AppError::Other("Runtime 目录路径无效".into()));
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::Other(format!(
                    "Runtime 条目不能写入软链接目录: {}",
                    current.display()
                )));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(AppError::Other(format!(
                    "Runtime 目录与文件冲突: {}",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn digest_rootfs(rootfs_dir: &Path) -> AppResult<(String, u64)> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(rootfs_dir) {
        let entry = entry.map_err(|error| AppError::Other(format!("扫描 rootfs 失败: {error}")))?;
        if entry.file_type().is_file() || entry.file_type().is_symlink() {
            let relative = entry
                .path()
                .strip_prefix(rootfs_dir)
                .map_err(|error| AppError::Other(format!("rootfs 路径无效: {error}")))?
                .to_string_lossy()
                .replace('\\', "/");
            entries.push((
                relative,
                entry.path().to_path_buf(),
                entry.file_type().is_symlink(),
            ));
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for (relative, path, is_symlink) in entries {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        let bytes = if is_symlink {
            std::fs::read_link(path)?
                .to_string_lossy()
                .as_bytes()
                .to_vec()
        } else {
            std::fs::read(path)?
        };
        total += bytes.len() as u64;
        hasher.update(&bytes);
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

#[cfg(any(target_os = "android", test))]
fn safe_symlink_target(link_path: &Path, value: &str) -> bool {
    if value.is_empty() || value.contains('\0') {
        return false;
    }
    let target = Path::new(value);
    let mut depth = if target.is_absolute() {
        0
    } else {
        link_path
            .parent()
            .map(|parent| parent.components().count())
            .unwrap_or(0)
    };
    for component in target.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::Prefix(_) => return false,
        }
    }
    true
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
                previous_runtime_version: read_runtime_pointer(
                    &root_dir.join("previous"),
                    "previous",
                )
                .ok()
                .filter(|previous| installed_runtime_snapshot(root_dir, previous).is_ok()),
                abi,
            };
        }
    }
    RuntimeSnapshot {
        state: "not-installed".into(),
        detail: "Android Linux Runtime 尚未安装".into(),
        runtime_version: None,
        previous_runtime_version: None,
        abi: None,
    }
}

#[cfg(any(target_os = "android", test))]
fn read_runtime_pointer(path: &Path, name: &str) -> AppResult<String> {
    let value = std::fs::read_to_string(path)
        .map_err(|error| AppError::Other(format!("无法读取 Runtime {name} 版本记录: {error}")))?;
    let value = value.trim();
    if !valid_version(value) {
        return Err(AppError::Other(format!("Runtime {name} 版本记录无效")));
    }
    Ok(value.to_owned())
}

#[cfg(target_os = "android")]
fn installed_runtime_snapshot(root_dir: &Path, version: &str) -> AppResult<RuntimeSnapshot> {
    let version_dir = root_dir.join("versions").join(version);
    if !version_dir.join("rootfs").is_dir() || !version_dir.join("manifest.json").is_file() {
        return Err(AppError::Other(format!(
            "Android Runtime 版本 {version} 不完整"
        )));
    }
    let manifest: RuntimeManifest =
        serde_json::from_slice(&std::fs::read(version_dir.join("manifest.json"))?)?;
    validate_manifest(&manifest)?;
    if manifest.runtime_version != version || manifest.abi != target_android_abi() {
        return Err(AppError::Other(format!(
            "Android Runtime 版本 {version} 与当前设备不匹配"
        )));
    }
    let (digest, size) = digest_rootfs(&version_dir.join("rootfs"))?;
    if digest != manifest.rootfs_sha256 || size != manifest.rootfs_size {
        return Err(AppError::Other(format!(
            "Android Runtime 版本 {version} 的 rootfs 校验失败"
        )));
    }
    Ok(RuntimeSnapshot {
        state: "installed".into(),
        detail: String::new(),
        runtime_version: Some(version.to_owned()),
        previous_runtime_version: None,
        abi: Some(manifest.abi),
    })
}

#[cfg(any(target_os = "android", test))]
fn swap_runtime_pointers(root_dir: &Path, current: &str, previous: &str) -> AppResult<()> {
    if !valid_version(current) || !valid_version(previous) || current == previous {
        return Err(AppError::Other("Runtime 版本指针无效".into()));
    }
    let current_path = root_dir.join("current");
    let previous_path = root_dir.join("previous");
    let current_tmp = root_dir.join("current.rollback.tmp");
    let previous_tmp = root_dir.join("previous.rollback.tmp");

    std::fs::write(&current_tmp, previous)?;
    std::fs::write(&previous_tmp, current)?;

    let result = (|| {
        std::fs::rename(&current_tmp, &current_path)?;
        std::fs::rename(&previous_tmp, &previous_path)?;
        Ok::<_, std::io::Error>(())
    })();

    if let Err(error) = result {
        let restore_path = root_dir.join("current.rollback.restore");
        let restore_result = (|| {
            std::fs::write(&restore_path, current)?;
            std::fs::rename(&restore_path, &current_path)?;
            Ok::<_, std::io::Error>(())
        })();
        let _ = std::fs::remove_file(&current_tmp);
        let _ = std::fs::remove_file(&previous_tmp);
        let _ = std::fs::remove_file(&restore_path);
        if let Err(restore_error) = restore_result {
            return Err(AppError::Other(format!(
                "Runtime 回滚失败，且无法恢复当前版本: {error}; {restore_error}"
            )));
        }
        return Err(AppError::Other(format!("Runtime 回滚失败: {error}")));
    }
    Ok(())
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
    use std::path::Path;

    use super::{
        enforce_guest_python_policy, read_runtime_pointer, safe_relative_path, safe_symlink_target,
        swap_runtime_pointers, valid_version, LEGACY_AGNES_PIP_CONFIG,
    };
    use std::fs;
    use tempfile::tempdir;

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

    #[test]
    fn rootfs_symlinks_stay_inside_linux_root() {
        assert!(safe_symlink_target(Path::new("etc/mtab"), "/proc/mounts"));
        assert!(safe_symlink_target(
            Path::new("usr/bin/tool"),
            "../lib/tool"
        ));
        assert!(!safe_symlink_target(
            Path::new("usr/bin/tool"),
            "../../../outside"
        ));
    }

    #[test]
    fn legacy_agnes_global_pip_override_is_removed() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc")).unwrap();
        fs::write(root.path().join("etc/pip.conf"), LEGACY_AGNES_PIP_CONFIG).unwrap();

        enforce_guest_python_policy(root.path()).unwrap();

        assert!(!root.path().join("etc/pip.conf").exists());
    }

    #[test]
    fn user_managed_pip_configuration_is_preserved() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc")).unwrap();
        let custom = "[global]\nindex-url = https://example.invalid/simple\n";
        fs::write(root.path().join("etc/pip.conf"), custom).unwrap();

        enforce_guest_python_policy(root.path()).unwrap();

        assert_eq!(
            fs::read_to_string(root.path().join("etc/pip.conf")).unwrap(),
            custom
        );
    }

    #[test]
    fn runtime_pointer_swap_exchanges_current_and_previous() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("current"), "v2").unwrap();
        fs::write(root.path().join("previous"), "v1").unwrap();

        swap_runtime_pointers(root.path(), "v2", "v1").unwrap();

        assert_eq!(
            read_runtime_pointer(&root.path().join("current"), "current").unwrap(),
            "v1"
        );
        assert_eq!(
            read_runtime_pointer(&root.path().join("previous"), "previous").unwrap(),
            "v2"
        );
    }

    #[test]
    fn runtime_pointer_swap_rejects_invalid_versions_without_changing_files() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("current"), "v2").unwrap();
        fs::write(root.path().join("previous"), "../outside").unwrap();

        let result = read_runtime_pointer(&root.path().join("previous"), "previous");
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("current")).unwrap(),
            "v2"
        );
    }

    #[test]
    fn runtime_pointer_swap_restores_current_when_previous_update_fails() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("current"), "v2").unwrap();
        fs::create_dir(root.path().join("previous")).unwrap();

        let result = swap_runtime_pointers(root.path(), "v2", "v1");

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("current")).unwrap(),
            "v2"
        );
        assert!(!root.path().join("current.rollback.tmp").exists());
    }
}
