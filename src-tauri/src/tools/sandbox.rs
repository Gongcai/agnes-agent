//! Process and path sandbox shared by all built-in tools.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{GuestMount, GuestPathResolver, ResolvedGuestPath};
use crate::tools::policy::{expand_home, BwrapMode, ToolPolicy};

const SANDBOX_HELPER_FLAG: &str = "--agnes-sandbox-exec";
const SANDBOX_CONFIG_ENV: &str = "AGNES_INTERNAL_SANDBOX_CONFIG";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ResourceLimits {
    enabled: bool,
    cpu_time_sec: u64,
    memory_bytes: u64,
    file_size_bytes: u64,
    max_processes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProcessSandboxSpec {
    landlock: bool,
    read_only: Vec<PathBuf>,
    read_write: Vec<PathBuf>,
    limits: ResourceLimits,
}

/// Fully resolved launch specification shared by pipe-backed and PTY-backed
/// runners. Sandbox wrapping and environment filtering must stay identical for
/// both execution paths.
#[derive(Debug)]
pub struct SandboxCommand {
    program: OsString,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
}

#[derive(Clone, Debug, Default)]
pub enum ToolProcessRuntime {
    #[default]
    Host,
    AndroidProot(AndroidProotProcessRuntime),
}

#[derive(Clone, Debug)]
pub struct AndroidProotProcessRuntime {
    config: AndroidProotRuntimeConfig,
}

#[derive(Debug)]
pub(crate) struct HiddenExecResult {
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
    pub(crate) output_truncated: bool,
    pub(crate) output_requests_input: bool,
}

/// A bind mount in the Android guest namespace. The resource ID is kept next
/// to the paths so launchers and host-side policy code can refer to the same
/// logical resource without exposing Android implementation paths to agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AndroidProotBindMount {
    pub(crate) resource_id: &'static str,
    pub(crate) source: PathBuf,
    pub(crate) target: &'static str,
}

/// Immutable Android PRoot settings produced by the Runtime supervisor.
/// Agent and tool runners share this value so capability-dependent flags and
/// guest mounts cannot drift between launch paths.
#[derive(Clone, Debug)]
pub(crate) struct AndroidProotRuntimeConfig {
    proot_path: PathBuf,
    proot_loader_path: PathBuf,
    proot_tmp_dir: PathBuf,
    rootfs_dir: PathBuf,
    android_workspace_root: PathBuf,
    link2symlink_enabled: bool,
    runtime_data_dir: Option<PathBuf>,
    environment_store_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    user_workspace_dir: Option<PathBuf>,
    user_home_dir: Option<PathBuf>,
    shared_storage_dir: Option<PathBuf>,
}

impl AndroidProotRuntimeConfig {
    pub fn new(
        proot_path: PathBuf,
        proot_loader_path: PathBuf,
        proot_tmp_dir: PathBuf,
        rootfs_dir: PathBuf,
        android_workspace_root: PathBuf,
        link2symlink_enabled: bool,
    ) -> Self {
        Self {
            proot_path,
            proot_loader_path,
            proot_tmp_dir,
            rootfs_dir,
            android_workspace_root,
            link2symlink_enabled,
            runtime_data_dir: None,
            environment_store_dir: None,
            cache_dir: None,
            user_workspace_dir: None,
            user_home_dir: None,
            shared_storage_dir: None,
        }
    }

    pub(crate) fn with_runtime_mounts(
        mut self,
        runtime_data_dir: PathBuf,
        environment_store_dir: PathBuf,
        cache_dir: PathBuf,
    ) -> Self {
        self.runtime_data_dir = Some(runtime_data_dir);
        self.environment_store_dir = Some(environment_store_dir);
        self.cache_dir = Some(cache_dir);
        self
    }

    pub(crate) fn with_user_workspace(mut self, workspace_dir: PathBuf) -> Self {
        self.user_workspace_dir = Some(workspace_dir);
        self
    }

    pub(crate) fn with_user_home(mut self, home_dir: PathBuf) -> Self {
        self.user_home_dir = Some(home_dir);
        self
    }

    pub(crate) fn with_shared_storage(mut self, storage_dir: PathBuf) -> Self {
        self.shared_storage_dir = Some(storage_dir);
        self
    }

    pub(crate) fn with_link2symlink_enabled(mut self, enabled: bool) -> Self {
        self.link2symlink_enabled = enabled;
        self
    }

    pub(crate) fn proot_path(&self) -> &Path {
        &self.proot_path
    }

    pub(crate) fn rootfs_dir(&self) -> &Path {
        &self.rootfs_dir
    }

    pub(crate) fn validate(&self) -> AppResult<()> {
        for (path, label) in [
            (&self.proot_path, "PRoot"),
            (&self.proot_loader_path, "PRoot loader"),
        ] {
            if !path.is_absolute() || !path.is_file() {
                return Err(AppError::Other(format!(
                    "Android {label} path is invalid: {}",
                    path.display()
                )));
            }
        }
        if !self.proot_tmp_dir.is_absolute() || !self.proot_tmp_dir.is_dir() {
            return Err(AppError::Other(format!(
                "Android PRoot temporary directory is invalid: {}",
                self.proot_tmp_dir.display()
            )));
        }
        let proot_shm_dir = self.proot_tmp_dir.join("shm");
        if !proot_shm_dir.is_dir() {
            return Err(AppError::Other(format!(
                "Android PRoot shared-memory directory is invalid: {}",
                proot_shm_dir.display()
            )));
        }
        if !self.rootfs_dir.is_absolute()
            || !self.rootfs_dir.is_dir()
            || !self.rootfs_dir.join("usr/bin/env").is_file()
        {
            return Err(AppError::Other(format!(
                "Android Linux rootfs is invalid: {}",
                self.rootfs_dir.display()
            )));
        }
        if !self.android_workspace_root.is_absolute() || !self.android_workspace_root.is_dir() {
            return Err(AppError::Other(format!(
                "Android Linux project workspace is invalid: {}",
                self.android_workspace_root.display()
            )));
        }
        Ok(())
    }

    pub(crate) fn base_args(&self) -> Vec<OsString> {
        let mut args = vec![
            OsString::from("-0"),
            OsString::from("-r"),
            self.rootfs_dir.clone().into_os_string(),
        ];
        if self.link2symlink_enabled {
            args.push(OsString::from("--link2symlink"));
        }
        for mount in self.base_mounts() {
            append_bind_mount(&mut args, &mount);
        }
        args
    }

    pub(crate) fn append_workspace_args(
        &self,
        args: &mut Vec<OsString>,
        workspace_root: &Path,
        guest_cwd: &Path,
    ) {
        for mount in self.workspace_mounts(workspace_root) {
            append_bind_mount(args, &mount);
        }
        args.extend([OsString::from("-w"), guest_cwd.as_os_str().to_owned()]);
    }

    pub(crate) fn base_mounts(&self) -> Vec<AndroidProotBindMount> {
        vec![
            AndroidProotBindMount {
                resource_id: "android-dev",
                source: PathBuf::from("/dev"),
                target: "/dev",
            },
            AndroidProotBindMount {
                resource_id: "android-dev-pts",
                source: PathBuf::from("/dev/pts"),
                target: "/dev/pts",
            },
            AndroidProotBindMount {
                resource_id: "android-dev-fd",
                source: PathBuf::from("/proc/self/fd"),
                target: "/dev/fd",
            },
            AndroidProotBindMount {
                resource_id: "android-stdin",
                source: PathBuf::from("/proc/self/fd/0"),
                target: "/dev/stdin",
            },
            AndroidProotBindMount {
                resource_id: "android-stdout",
                source: PathBuf::from("/proc/self/fd/1"),
                target: "/dev/stdout",
            },
            AndroidProotBindMount {
                resource_id: "android-stderr",
                source: PathBuf::from("/proc/self/fd/2"),
                target: "/dev/stderr",
            },
            AndroidProotBindMount {
                resource_id: "android-proc",
                source: PathBuf::from("/proc"),
                target: "/proc",
            },
            AndroidProotBindMount {
                resource_id: "android-sys",
                source: PathBuf::from("/sys"),
                target: "/sys",
            },
            AndroidProotBindMount {
                resource_id: "runtime-tmp",
                source: self.proot_tmp_dir.clone(),
                target: "/tmp",
            },
            AndroidProotBindMount {
                resource_id: "runtime-shm",
                source: self.proot_tmp_dir.join("shm"),
                target: "/dev/shm",
            },
        ]
    }

    pub(crate) fn workspace_mounts(&self, workspace_root: &Path) -> Vec<AndroidProotBindMount> {
        let mut mounts = vec![
            AndroidProotBindMount {
                resource_id: "workspace",
                source: workspace_root.to_path_buf(),
                target: "/workspace",
            },
            AndroidProotBindMount {
                resource_id: "project-root",
                source: self.android_workspace_root.clone(),
                target: "/root/workspace",
            },
        ];
        if let Some(source) = &self.user_home_dir {
            mounts.push(AndroidProotBindMount {
                resource_id: "user-home",
                source: source.clone(),
                target: "/root",
            });
        }
        if let Some(source) = &self.runtime_data_dir {
            mounts.push(AndroidProotBindMount {
                resource_id: "runtime-data",
                source: source.clone(),
                target: "/opt/agnes/runtime-data",
            });
        }
        if let Some(source) = &self.environment_store_dir {
            mounts.push(AndroidProotBindMount {
                resource_id: "env-store",
                source: source.clone(),
                target: "/opt/agnes/envs",
            });
        }
        if let Some(source) = &self.cache_dir {
            mounts.push(AndroidProotBindMount {
                resource_id: "runtime-cache",
                source: source.clone(),
                target: "/opt/agnes/cache",
            });
        }
        if let Some(source) = &self.shared_storage_dir {
            mounts.push(AndroidProotBindMount {
                resource_id: "shared-storage",
                source: source.clone(),
                target: "/sdcard",
            });
            mounts.push(AndroidProotBindMount {
                resource_id: "shared-storage",
                source: source.clone(),
                target: "/storage/emulated/0",
            });
        }
        mounts
    }

    fn guest_path_resolver(&self, workspace_root: &Path) -> GuestPathResolver {
        let mut resolver = GuestPathResolver::new(Some(workspace_root), None)
            .with_mount(GuestMount::new("rootfs", self.rootfs_dir.clone(), "/"));
        for mount in self
            .base_mounts()
            .into_iter()
            .chain(self.workspace_mounts(workspace_root))
        {
            resolver = resolver.with_mount(GuestMount::new(
                mount.resource_id,
                mount.source,
                mount.target,
            ));
        }
        resolver
    }

    fn host_mount_roots(&self, workspace_root: &Path) -> Vec<PathBuf> {
        self.base_mounts()
            .into_iter()
            .chain(self.workspace_mounts(workspace_root))
            .map(|mount| mount.source)
            .collect()
    }

    fn default_writable_roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.proot_tmp_dir.clone()];
        if let Some(home) = &self.user_home_dir {
            roots.extend(
                [".cache", ".local", ".cargo", ".rustup"]
                    .into_iter()
                    .map(|path| home.join(path)),
            );
        }
        if let Some(storage) = &self.shared_storage_dir {
            roots.push(storage.clone());
        }
        roots
    }

    pub(crate) fn user_command_spec(
        &self,
        cwd: Option<&str>,
    ) -> AppResult<(SandboxCommand, PathBuf, PathBuf)> {
        self.validate()?;
        let workspace = self.user_workspace_dir.as_ref().ok_or_else(|| {
            AppError::Other("Android user terminal workspace is not configured".into())
        })?;
        if !workspace.is_dir() {
            return Err(AppError::Other(format!(
                "Android user terminal workspace is invalid: {}",
                workspace.display()
            )));
        }
        let guest_cwd = normalize_guest_cwd(cwd.unwrap_or("/root"))?;
        let home = self.user_home_dir.as_ref().ok_or_else(|| {
            AppError::Other("Android user terminal home is not configured".into())
        })?;
        if !home.is_dir() {
            return Err(AppError::Other(format!(
                "Android user terminal home is invalid: {}",
                home.display()
            )));
        }
        let mounts = self.workspace_mounts(workspace);
        let host_cwd = mounts
            .iter()
            .find_map(|mount| {
                let target = Path::new(mount.target);
                let relative = if guest_cwd == target {
                    Some(PathBuf::new())
                } else {
                    guest_cwd.strip_prefix(target).ok().map(Path::to_path_buf)
                }?;
                Some(mount.source.join(relative))
            })
            .unwrap_or_else(|| {
                self.rootfs_dir
                    .join(guest_cwd.strip_prefix("/").unwrap_or(&guest_cwd))
            });
        if !host_cwd.is_dir() {
            return Err(AppError::Other(format!(
                "Android user terminal cwd is not a directory: {}",
                guest_cwd.display()
            )));
        }

        let mut args = self.base_args();
        for mount in mounts {
            append_bind_mount(&mut args, &mount);
        }
        args.extend([
            OsString::from("-w"),
            guest_cwd.as_os_str().to_owned(),
            OsString::from("/usr/bin/env"),
            OsString::from("bash"),
            OsString::from("--login"),
        ]);
        Ok((
            SandboxCommand {
                program: self.proot_path.clone().into_os_string(),
                args,
                env: self.environment(),
            },
            host_cwd,
            guest_cwd,
        ))
    }

    /// Resolves a path for the Runtime file UI with the same mount table used
    /// by PRoot processes. Runtime internals and Android pseudo-filesystems
    /// are intentionally excluded from this user-facing surface.
    pub(crate) fn resolve_user_visible_file_path(&self, raw: &str) -> AppResult<ResolvedGuestPath> {
        let path = Path::new(raw);
        if !path.is_absolute() {
            return Err(AppError::Other(
                "Runtime 文件路径必须是绝对 guest 路径".into(),
            ));
        }
        let workspace = self.user_workspace_dir.as_deref().ok_or_else(|| {
            AppError::Other("Android user terminal workspace is not configured".into())
        })?;
        let resolved = self.guest_path_resolver(workspace).resolve(raw);
        if matches!(
            resolved.resource_id.as_str(),
            "rootfs" | "workspace" | "project-root" | "user-home" | "shared-storage"
        ) {
            Ok(resolved)
        } else {
            Err(AppError::Other(format!(
                "Runtime 文件浏览不开放该路径资源: {}",
                resolved.guest_path.display()
            )))
        }
    }

    pub(crate) fn user_visible_resource_root(&self, resource_id: &str) -> Option<PathBuf> {
        if resource_id == "rootfs" {
            return Some(self.rootfs_dir.clone());
        }
        let workspace = self.user_workspace_dir.as_deref()?;
        self.workspace_mounts(workspace)
            .into_iter()
            .find(|mount| mount.resource_id == resource_id)
            .map(|mount| mount.source)
    }

    pub(crate) fn environment(&self) -> Vec<(OsString, OsString)> {
        [
            ("HOME", OsString::from("/root")),
            (
                "PATH",
                OsString::from(
                    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games",
                ),
            ),
            ("TERM", OsString::from("dumb")),
            ("CI", OsString::from("1")),
            ("DEBIAN_FRONTEND", OsString::from("noninteractive")),
            ("NO_COLOR", OsString::from("1")),
            ("TMPDIR", OsString::from("/tmp")),
            ("LANG", OsString::from("C.UTF-8")),
            ("LC_ALL", OsString::from("C.UTF-8")),
            ("UV_LINK_MODE", OsString::from("copy")),
            ("LD_LIBRARY_PATH", OsString::new()),
            ("PROOT_TMP_DIR", self.proot_tmp_dir.as_os_str().to_owned()),
            (
                "PROOT_LOADER",
                self.proot_loader_path.as_os_str().to_owned(),
            ),
            ("PROOT_NO_SECCOMP", OsString::from("1")),
        ]
        .into_iter()
        .map(|(name, value)| (OsString::from(name), value))
        .collect()
    }
}

fn append_bind_mount(args: &mut Vec<OsString>, mount: &AndroidProotBindMount) {
    args.extend([
        OsString::from("-b"),
        OsString::from(format!("{}:{}", mount.source.display(), mount.target)),
    ]);
}

fn normalize_guest_cwd(raw: &str) -> AppResult<PathBuf> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(AppError::Other(
            "Android user terminal cwd must be an absolute guest path".into(),
        ));
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                return Err(AppError::Other(
                    "Android user terminal cwd cannot contain `..`".into(),
                ));
            }
            std::path::Component::Prefix(_) => {
                return Err(AppError::Other(
                    "Android user terminal cwd has an invalid path prefix".into(),
                ));
            }
        }
    }
    Ok(normalized)
}

impl AndroidProotProcessRuntime {
    pub(crate) fn new(config: AndroidProotRuntimeConfig) -> Self {
        Self { config }
    }

    fn command_spec(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
        workspace_root: Option<&Path>,
        cwd: Option<&Path>,
    ) -> AppResult<SandboxCommand> {
        self.config.validate()?;

        let workspace_root = workspace_root
            .map(normalize_root)
            .filter(|path| path.is_absolute() && path.is_dir())
            .ok_or_else(|| {
                AppError::Other("Android Linux tools require a valid workspace".into())
            })?;
        let cwd = cwd
            .map(normalize_root)
            .unwrap_or_else(|| workspace_root.clone());
        let resolver = self.config.guest_path_resolver(&workspace_root);
        let resolved_cwd = resolver.resolve_from_cwd(".", Some(&cwd));
        if resolved_cwd.resource_id == "host" {
            return Err(AppError::Other(format!(
                "Android Linux tool cwd is not a mounted Runtime path: {}",
                cwd.display()
            )));
        }
        let guest_cwd = resolved_cwd.guest_path;

        let mut proot_args = self.config.base_args();
        self.config
            .append_workspace_args(&mut proot_args, &workspace_root, &guest_cwd);
        proot_args.extend([OsString::from("/usr/bin/env"), OsString::from(program)]);
        proot_args.extend(args.iter().map(OsString::from));

        // Operit starts its Linux shell with a clean environment. Preserve
        // only explicitly allowed variables so Android host paths cannot leak
        // into the Ubuntu userspace.
        let mut env = if env_allowlist.is_empty() {
            Vec::new()
        } else {
            filtered_environment(env_allowlist)
        };
        for (name, value) in self.config.environment() {
            set_environment(&mut env, name, value);
        }

        Ok(SandboxCommand {
            program: self.config.proot_path.clone().into_os_string(),
            args: proot_args,
            env,
        })
    }
}

impl ToolProcessRuntime {
    pub(crate) fn is_android_proot(&self) -> bool {
        matches!(self, Self::AndroidProot(_))
    }

    /// Convert a host path into the stable path visible to the Agent process.
    /// Android's Agent has a fixed mount table, so it cannot consume a host
    /// absolute path from a prompt even when the corresponding tool runtime
    /// can mount that path for a single command.
    pub(crate) fn guest_path_for_host(
        &self,
        host_path: &Path,
        agent_workspace_root: &Path,
    ) -> PathBuf {
        match self {
            Self::Host => host_path.to_path_buf(),
            Self::AndroidProot(runtime) => {
                runtime
                    .config
                    .guest_path_resolver(agent_workspace_root)
                    .resolve_policy_path(&host_path.to_string_lossy())
                    .guest_path
            }
        }
    }
}

impl SandboxCommand {
    #[cfg(test)]
    pub(crate) fn direct(program: String, args: Vec<String>, env: Vec<(String, String)>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: env
                .into_iter()
                .map(|(name, value)| (name.into(), value.into()))
                .collect(),
        }
    }

    pub fn into_tokio(self) -> Command {
        let mut command = Command::new(self.program);
        command.args(self.args);
        command.env_clear();
        command.envs(self.env);
        command.kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command.stdin(Stdio::null());
        command
    }

    pub fn into_pty(self) -> portable_pty::CommandBuilder {
        let mut command = portable_pty::CommandBuilder::new(self.program);
        command.args(self.args);
        command.env_clear();
        for (name, value) in self.env {
            command.env(name, value);
        }
        command
    }
}

/// Execute an Agent command without a PTY or an input channel. This is the
/// execution plane for Android hidden calls; user terminals use TerminalManager
/// and must never share its session, stdin, or output state.
pub(crate) async fn execute_hidden(
    command: SandboxCommand,
    cwd: &Path,
    max_output_bytes: usize,
    timeout_sec: u32,
) -> AppResult<HiddenExecResult> {
    let stream_limit = (max_output_bytes.max(2) / 2).max(1);
    let mut child = command
        .into_tokio()
        .current_dir(cwd)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|error| AppError::Other(format!("Unable to spawn hidden command: {error}")))?;

    let pid = child.id().and_then(|pid| i32::try_from(pid).ok());
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(read_stream_capped(stdout, stream_limit)));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_stream_capped(stderr, stream_limit)));

    let wait_result = if timeout_sec == 0 {
        child.wait().await.map_err(|error| error.to_string())
    } else {
        match tokio::time::timeout(
            std::time::Duration::from_secs(timeout_sec as u64),
            child.wait(),
        )
        .await
        {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(_) => {
                kill_process_tree(&mut child, pid).await;
                return Ok(hidden_capture_result(stdout_task, stderr_task, Some(-9), true).await);
            }
        }
    };

    let status =
        wait_result.map_err(|error| AppError::Other(format!("Hidden command failed: {error}")))?;
    cleanup_process_group(pid);
    let exit_code = status.code();
    Ok(hidden_capture_result(stdout_task, stderr_task, exit_code, false).await)
}

async fn hidden_capture_result(
    stdout_task: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>,
    stderr_task: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>,
    exit_code: Option<i32>,
    timed_out: bool,
) -> HiddenExecResult {
    let (stdout_bytes, stdout_truncated) = join_capture(stdout_task).await;
    let (stderr_bytes, stderr_truncated) = join_capture(stderr_task).await;
    let stdout = render_captured_output(stdout_bytes, stdout_truncated);
    let stderr = render_captured_output(stderr_bytes, stderr_truncated);
    let output_requests_input = output_requests_input(&stdout) || output_requests_input(&stderr);
    HiddenExecResult {
        stdout,
        stderr,
        exit_code,
        timed_out,
        output_truncated: stdout_truncated || stderr_truncated,
        output_requests_input,
    }
}

async fn kill_process_tree(child: &mut tokio::process::Child, pid: Option<i32>) {
    cleanup_process_group(pid);
    let _ = child.kill().await;
    let _ = child.wait().await;
}

fn cleanup_process_group(pid: Option<i32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        use nix::sys::signal::{killpg, Signal};
        use nix::unistd::Pid;
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

fn output_requests_input(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    [
        "[y/n]",
        "[y/n/?]",
        "password:",
        "passphrase:",
        "press enter",
        "continue?",
        "do you want to continue",
        "enter value",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

enum ProcessIsolation {
    Allowed,
    Bubblewrap {
        path: PathBuf,
        isolate_network: bool,
    },
    Heuristic,
    RequiredUnavailable,
}

pub trait SandboxGuard: Send + Sync {
    fn is_non_interactive(&self) -> bool {
        false
    }

    fn check_read(&self, path: &Path) -> Result<(), String>;
    fn check_write(&self, path: &Path) -> Result<(), String>;
    fn check_cwd(&self, path: &Path) -> Result<(), String>;
    /// Resolve a user-visible guest path to its host path and stable guest path.
    /// Android overrides this with the Runtime mount table; desktop keeps the
    /// existing workspace aliases.
    fn resolve_guest_path(&self, raw: &str, cwd: Option<&Path>) -> AppResult<(PathBuf, PathBuf)>;
    fn command(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<Command> {
        Ok(self
            .command_spec(program, args, env_allowlist)?
            .into_tokio())
    }
    fn command_spec(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<SandboxCommand>;
    fn command_spec_at(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
        _cwd: &Path,
    ) -> AppResult<SandboxCommand> {
        self.command_spec(program, args, env_allowlist)
    }
    /// Git is the one built-in tool allowed to update repository metadata.
    fn git_command(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<Command> {
        Ok(self
            .command_spec(program, args, env_allowlist)?
            .into_tokio())
    }
    fn git_command_at(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
        cwd: &Path,
    ) -> AppResult<Command> {
        let mut command = self.git_command(program, args, env_allowlist)?;
        command.current_dir(cwd);
        Ok(command)
    }
}

pub struct PolicySandbox {
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    cwd_roots: Vec<PathBuf>,
    process_spec: ProcessSandboxSpec,
    process_isolation: ProcessIsolation,
    protected_paths: Vec<PathBuf>,
    process_runtime: ToolProcessRuntime,
    workspace_root: Option<PathBuf>,
}

impl PolicySandbox {
    pub fn new(policy: &ToolPolicy, workspace_cwd: Option<&Path>) -> Self {
        Self::new_with_process_runtime(policy, workspace_cwd, ToolProcessRuntime::Host)
    }

    pub fn new_with_process_runtime(
        policy: &ToolPolicy,
        workspace_cwd: Option<&Path>,
        process_runtime: ToolProcessRuntime,
    ) -> Self {
        let runtime_resolver = match (&process_runtime, workspace_cwd) {
            (ToolProcessRuntime::AndroidProot(runtime), Some(workspace)) => {
                Some(runtime.config.guest_path_resolver(workspace))
            }
            _ => None,
        };
        let resolve_policy_root = |path: &str| {
            runtime_resolver
                .as_ref()
                .map(|resolver| resolver.resolve_policy_path(path).host_path)
                .unwrap_or_else(|| expand_home(path))
        };
        let mut configured_write_roots = policy
            .sandbox
            .writable_roots
            .iter()
            .map(|path| normalize_root(&resolve_policy_root(path)))
            .collect::<Vec<_>>();
        if let ToolProcessRuntime::AndroidProot(runtime) = &process_runtime {
            configured_write_roots.extend(
                runtime
                    .config
                    .default_writable_roots()
                    .into_iter()
                    .map(|path| normalize_root(&path)),
            );
        }
        deduplicate_paths(&mut configured_write_roots);

        let temp_write_roots = if runtime_resolver.is_some() {
            Vec::new()
        } else {
            temporary_write_paths()
        };

        let mut read_roots = if policy.sandbox.allow_read_outside_workspace {
            filesystem_read_roots()
        } else {
            policy
                .file
                .allowed_roots
                .iter()
                .map(|path| normalize_root(&resolve_policy_root(path)))
                .collect::<Vec<_>>()
        };
        if matches!(&process_runtime, ToolProcessRuntime::AndroidProot(_)) {
            // Unbound absolute guest paths resolve inside the bundled rootfs.
            // It is readable, but it must not become a writable or cwd root.
            if let ToolProcessRuntime::AndroidProot(runtime) = &process_runtime {
                read_roots.push(normalize_root(&runtime.config.rootfs_dir));
            }
        }
        let mut cwd_roots = policy
            .shell
            .allowed_cwd
            .iter()
            .map(|path| normalize_root(&resolve_policy_root(path)))
            .collect::<Vec<_>>();
        cwd_roots.extend(configured_write_roots.iter().cloned());
        cwd_roots.extend(temp_write_roots.iter().cloned());
        if let (ToolProcessRuntime::AndroidProot(runtime), Some(workspace)) =
            (&process_runtime, workspace_cwd)
        {
            cwd_roots.extend(
                runtime
                    .config
                    .host_mount_roots(workspace)
                    .into_iter()
                    .map(|path| normalize_root(&path)),
            );
        }
        deduplicate_paths(&mut cwd_roots);

        let primary_write_root = workspace_cwd
            .map(normalize_root)
            .or_else(|| cwd_roots.first().cloned());
        if let Some(workspace) = workspace_cwd.map(normalize_root) {
            read_roots.push(workspace);
        }
        read_roots.extend(configured_write_roots.iter().cloned());
        read_roots.extend(temp_write_roots.iter().cloned());
        deduplicate_paths(&mut read_roots);

        let mut write_roots = if policy.shell.deny_write_outside_workspace {
            let mut roots = primary_write_root.into_iter().collect::<Vec<_>>();
            roots.extend(configured_write_roots.iter().cloned());
            roots.extend(temp_write_roots.iter().cloned());
            roots
        } else {
            let mut roots = read_roots.clone();
            roots.extend(cwd_roots.iter().cloned());
            roots
        };
        deduplicate_paths(&mut write_roots);

        let mut process_read_only = read_roots.clone();
        process_read_only.extend(cwd_roots.iter().cloned());
        process_read_only.extend(system_read_paths());
        deduplicate_paths(&mut process_read_only);

        let mut process_read_write = write_roots.clone();
        deduplicate_paths(&mut process_read_write);

        let process_isolation = match policy.sandbox.bwrap {
            BwrapMode::Disabled if policy.network.allow => ProcessIsolation::Allowed,
            BwrapMode::Disabled => ProcessIsolation::Heuristic,
            BwrapMode::Auto => detect_bwrap()
                .map(|path| ProcessIsolation::Bubblewrap {
                    path,
                    isolate_network: !policy.network.allow,
                })
                .unwrap_or(if policy.network.allow {
                    ProcessIsolation::Allowed
                } else {
                    ProcessIsolation::Heuristic
                }),
            BwrapMode::Required => detect_bwrap()
                .map(|path| ProcessIsolation::Bubblewrap {
                    path,
                    isolate_network: !policy.network.allow,
                })
                .unwrap_or(ProcessIsolation::RequiredUnavailable),
        };

        let protect_metadata = policy.shell.deny_write_outside_workspace
            || policy.sandbox.landlock
            || policy.sandbox.bwrap != BwrapMode::Disabled;
        let mut protected_paths: Vec<PathBuf> = protect_metadata
            .then(|| workspace_cwd.map(normalize_root))
            .flatten()
            .into_iter()
            .flat_map(|workspace| {
                [".git", ".codex", ".agents"]
                    .into_iter()
                    .map(move |name| normalize_root(&workspace.join(name)))
            })
            .collect();
        if matches!(&process_runtime, ToolProcessRuntime::AndroidProot(_)) {
            // A fake-root guest must not turn direct shell/file writes into
            // writes to the runtime's loader and system-library directories.
            protected_paths.extend([PathBuf::from("/lib"), PathBuf::from("/lib64")]);
        }

        Self {
            read_roots,
            write_roots,
            cwd_roots,
            process_spec: ProcessSandboxSpec {
                landlock: policy.sandbox.landlock,
                read_only: process_read_only,
                read_write: process_read_write,
                limits: ResourceLimits {
                    enabled: policy.sandbox.rlimits,
                    cpu_time_sec: policy.sandbox.cpu_time_sec,
                    memory_bytes: policy.sandbox.memory_bytes,
                    file_size_bytes: policy.sandbox.file_size_bytes,
                    max_processes: policy.sandbox.max_processes,
                },
            },
            process_isolation,
            protected_paths,
            process_runtime,
            workspace_root: workspace_cwd.map(normalize_root),
        }
    }
}

impl SandboxGuard for PolicySandbox {
    fn is_non_interactive(&self) -> bool {
        self.process_runtime.is_android_proot()
    }

    fn check_read(&self, path: &Path) -> Result<(), String> {
        check_under_roots(path, &self.read_roots, "read")
    }

    fn check_write(&self, path: &Path) -> Result<(), String> {
        let target = crate::tools::builtin::normalize_path(path);
        if self
            .protected_paths
            .iter()
            .any(|protected| is_path_within(&target, protected))
        {
            return Err(format!(
                "Sandbox denied permission to write protected metadata path `{}`",
                target.display()
            ));
        }
        check_under_roots(path, &self.write_roots, "write")
    }

    fn check_cwd(&self, path: &Path) -> Result<(), String> {
        check_under_roots(path, &self.cwd_roots, "working-directory")
            .or_else(|_| check_under_roots(path, &self.read_roots, "readable working-directory"))
    }

    fn resolve_guest_path(&self, raw: &str, cwd: Option<&Path>) -> AppResult<(PathBuf, PathBuf)> {
        let resolver = match &self.process_runtime {
            ToolProcessRuntime::AndroidProot(runtime) => self
                .workspace_root
                .as_deref()
                .map(|root| runtime.config.guest_path_resolver(root)),
            ToolProcessRuntime::Host => None,
        }
        .unwrap_or_else(|| GuestPathResolver::new(self.workspace_root.as_deref(), None));
        let resolved = resolver.resolve_from_cwd(raw, cwd);
        Ok((
            crate::tools::builtin::normalize_path(&resolved.host_path),
            resolved.guest_path,
        ))
    }

    fn command_spec(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<SandboxCommand> {
        self.command_spec_internal(program, args, env_allowlist, false, None)
    }

    fn command_spec_at(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
        cwd: &Path,
    ) -> AppResult<SandboxCommand> {
        self.command_spec_internal(program, args, env_allowlist, false, Some(cwd))
    }

    fn git_command(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<Command> {
        Ok(self
            .command_spec_internal(program, args, env_allowlist, true, None)?
            .into_tokio())
    }

    fn git_command_at(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
        cwd: &Path,
    ) -> AppResult<Command> {
        let mut command = self
            .command_spec_internal(program, args, env_allowlist, true, Some(cwd))?
            .into_tokio();
        command.current_dir(cwd);
        Ok(command)
    }
}

impl PolicySandbox {
    fn command_spec_internal(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
        allow_protected_metadata: bool,
        cwd: Option<&Path>,
    ) -> AppResult<SandboxCommand> {
        match &self.process_isolation {
            ProcessIsolation::RequiredUnavailable => {
                return Err(AppError::Other(
                    "Network isolation requires bubblewrap, but it is unavailable".to_string(),
                ));
            }
            ProcessIsolation::Heuristic if is_network_action(program, args) => {
                return Err(AppError::Other(
                    "Network access is disabled by the tool policy".to_string(),
                ));
            }
            _ => {}
        }

        if let ToolProcessRuntime::AndroidProot(runtime) = &self.process_runtime {
            return runtime.command_spec(
                program,
                args,
                env_allowlist,
                self.workspace_root.as_deref(),
                cwd,
            );
        }

        let use_helper = !cfg!(test)
            && (self.process_spec.landlock || self.process_spec.limits.enabled)
            && cfg!(unix);
        let config = if use_helper {
            Some(serde_json::to_string(&self.process_spec)?)
        } else {
            None
        };
        let (target, target_args) = if use_helper {
            let current_exe = std::env::current_exe().map_err(|error| {
                AppError::Other(format!(
                    "Unable to locate sandbox helper executable: {error}"
                ))
            })?;
            let mut helper_args =
                vec![OsString::from(SANDBOX_HELPER_FLAG), OsString::from(program)];
            helper_args.extend(args.iter().map(OsString::from));
            (current_exe.into_os_string(), helper_args)
        } else {
            (
                OsString::from(program),
                args.iter().map(OsString::from).collect(),
            )
        };

        let (launch_program, launch_args) = if let ProcessIsolation::Bubblewrap {
            path,
            isolate_network,
        } = &self.process_isolation
        {
            let mut command_args = Vec::new();
            if *isolate_network {
                command_args.push(OsString::from("--unshare-net"));
            }
            let root_is_writable = self
                .process_spec
                .read_write
                .iter()
                .any(|path| path == Path::new("/"));
            command_args.extend(
                [
                    "--unshare-pid",
                    "--die-with-parent",
                    "--new-session",
                    if root_is_writable {
                        "--bind"
                    } else {
                        "--ro-bind"
                    },
                    "/",
                    "/",
                    "--dev-bind",
                    "/dev",
                    "/dev",
                    "--proc",
                    "/proc",
                ]
                .into_iter()
                .map(OsString::from),
            );
            for path in &self.process_spec.read_write {
                if !path.exists()
                    || is_path_within(path, Path::new("/dev"))
                    || path == Path::new("/")
                {
                    continue;
                }
                command_args.push(OsString::from("--bind"));
                command_args.push(path.clone().into_os_string());
                command_args.push(path.clone().into_os_string());
            }
            if !allow_protected_metadata {
                for path in &self.protected_paths {
                    if path.exists() {
                        command_args.push(OsString::from("--ro-bind"));
                        command_args.push(path.clone().into_os_string());
                        command_args.push(path.clone().into_os_string());
                    }
                }
            }
            command_args.push(OsString::from("--"));
            command_args.push(target.clone());
            command_args.extend(target_args);
            (path.clone().into_os_string(), command_args)
        } else {
            (target, target_args)
        };

        let mut env = filtered_environment(env_allowlist);
        if let Some(config) = config {
            env.push((OsString::from(SANDBOX_CONFIG_ENV), OsString::from(config)));
        }
        Ok(SandboxCommand {
            program: launch_program,
            args: launch_args,
            env,
        })
    }
}

fn filtered_environment(env_allowlist: &[String]) -> Vec<(OsString, OsString)> {
    let mut env = Vec::new();
    if env_allowlist.is_empty() {
        for (name, value) in std::env::vars_os() {
            if safe_environment_name(&name) {
                env.push((name, value));
            }
        }
    } else {
        for name in env_allowlist {
            if safe_environment_name(std::ffi::OsStr::new(name)) {
                if let Some(value) = std::env::var_os(name) {
                    env.push((OsString::from(name), value));
                }
            }
        }
    }
    env
}

fn set_environment(
    env: &mut Vec<(OsString, OsString)>,
    name: impl Into<OsString>,
    value: impl Into<OsString>,
) {
    let name = name.into();
    env.retain(|(existing, _)| existing != &name);
    env.push((name, value.into()));
}

fn safe_environment_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if name == SANDBOX_CONFIG_ENV || name == "AGENT_PROTOCOL_TOKEN" || name == "AGENT_WS_URL" {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    !["KEY", "SECRET", "TOKEN"]
        .iter()
        .any(|marker| upper.contains(marker))
}

fn detect_bwrap() -> Option<PathBuf> {
    static DETECTED: OnceLock<Option<PathBuf>> = OnceLock::new();
    DETECTED.get_or_init(probe_bwrap).clone()
}

#[cfg(target_os = "linux")]
fn probe_bwrap() -> Option<PathBuf> {
    let mut candidates = vec![PathBuf::from("/usr/bin/bwrap"), PathBuf::from("/bin/bwrap")];
    if let Some(path_var) = std::env::var_os("PATH") {
        candidates
            .extend(std::env::split_paths(&path_var).map(|directory| directory.join("bwrap")));
    }
    deduplicate_paths(&mut candidates);
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        let status = std::process::Command::new(&candidate)
            .args(["--unshare-net", "--ro-bind", "/", "/", "--", "/bin/true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if status.is_ok_and(|status| status.success()) {
            return candidate.canonicalize().ok().or(Some(candidate));
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn probe_bwrap() -> Option<PathBuf> {
    None
}

fn is_network_action(program: &str, args: &[String]) -> bool {
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    let joined = args.join(" ").to_ascii_lowercase();
    if program == "git" {
        return [
            "push",
            "pull",
            "fetch",
            "clone",
            "ls-remote",
            "remote-http",
            "remote-https",
            "remote-ssh",
            "submodule",
        ]
        .iter()
        .any(|operation| args.iter().any(|arg| arg == operation))
            || args
                .windows(2)
                .any(|pair| pair[0] == "remote" && pair[1] == "update")
            || (args.iter().any(|arg| arg == "archive")
                && args.iter().any(|arg| arg.starts_with("--remote")));
    }

    const NETWORK_PROGRAMS: &[&str] = &[
        "curl", "wget", "ssh", "scp", "sftp", "nc", "ncat", "netcat", "telnet", "ftp", "rsync",
    ];
    let contains_network_program = joined
        .split(|character: char| {
            character.is_whitespace() || matches!(character, ';' | '|' | '&' | '(' | ')')
        })
        .filter(|token| !token.is_empty())
        .any(|token| {
            Path::new(token.trim_matches(|character| character == '\'' || character == '"'))
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| NETWORK_PROGRAMS.contains(&name))
        });
    NETWORK_PROGRAMS.contains(&program.as_str())
        || contains_network_program
        || joined.contains("http://")
        || joined.contains("https://")
        || joined.contains("/dev/tcp/")
        || ((joined.contains("python") || joined.contains("node"))
            && (joined.contains("socket")
                || joined.contains("requests")
                || joined.contains("urlopen")
                || joined.contains("fetch(")))
}

pub fn run_sandbox_helper_if_requested() -> Option<i32> {
    let mut args = std::env::args_os();
    let _program = args.next();
    if args.next().as_deref() != Some(std::ffi::OsStr::new(SANDBOX_HELPER_FLAG)) {
        return None;
    }

    let target = match args.next() {
        Some(target) => target,
        None => {
            eprintln!("[sandbox] Missing target executable");
            return Some(126);
        }
    };
    let target_args: Vec<OsString> = args.collect();
    let config = match std::env::var(SANDBOX_CONFIG_ENV)
        .map_err(|error| error.to_string())
        .and_then(|json| {
            serde_json::from_str::<ProcessSandboxSpec>(&json).map_err(|error| error.to_string())
        }) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("[sandbox] Invalid helper configuration: {error}");
            return Some(126);
        }
    };

    if let Err(error) = apply_resource_limits(&config.limits) {
        eprintln!("[sandbox] Unable to apply resource limits: {error}");
        return Some(126);
    }
    if config.landlock {
        match apply_landlock(&config) {
            Ok(true) => {}
            Ok(false) => {
                eprintln!("[sandbox] Landlock is unavailable; using path-policy fallback");
            }
            Err(error) => {
                eprintln!("[sandbox] Unable to apply Landlock rules: {error}");
                return Some(126);
            }
        }
    }

    std::env::remove_var(SANDBOX_CONFIG_ENV);
    Some(exec_target(target, target_args))
}

pub async fn read_stream_capped<R>(mut reader: R, max_bytes: usize) -> (Vec<u8>, bool)
where
    R: AsyncRead + Unpin,
{
    let mut captured = Vec::with_capacity(max_bytes.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let read = match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let remaining = max_bytes.saturating_sub(captured.len());
        let keep = read.min(remaining);
        captured.extend_from_slice(&buffer[..keep]);
        truncated |= keep < read;
    }
    (captured, truncated)
}

pub fn render_captured_output(bytes: Vec<u8>, truncated: bool) -> String {
    let mut output = String::from_utf8_lossy(&bytes).to_string();
    if truncated {
        output.push_str("\n... output truncated by policy");
    }
    output
}

/// Map process and policy failures to stable categories consumed by the tool
/// protocol. The original stderr remains available for debugging.
pub(crate) fn classify_runtime_failure(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if [
        "sandbox denied",
        "outside the allowed",
        "unapproved absolute path",
        "cannot be safely scoped",
        "network access is disabled",
        "requires bubblewrap",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "policy_denied";
    }
    if lower.contains("read-only file system") || lower.contains("erofs") {
        return "filesystem_read_only";
    }
    if lower.contains("no such file or directory") || lower.contains("enoent") {
        return "path_not_found";
    }
    if [
        "cannot create symlink",
        "failed to create symbolic link",
        "invalid cross-device link",
        "operation not permitted",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || (lower.contains("permission denied")
            && (lower.contains("symlink")
                || lower.contains("symbolic link")
                || lower.contains(" -> ")))
    {
        return "android_filesystem_unsupported";
    }
    if lower.contains("permission denied") || lower.contains("eacces") {
        return "permission_denied";
    }
    if lower.contains("android linux runtime")
        || lower.contains("android proot")
        || lower.contains("proot loader")
        || lower.contains("rootfs")
    {
        return "runtime_unavailable";
    }
    "process_failed"
}

pub async fn join_capture(
    task: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>,
) -> (Vec<u8>, bool) {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => (Vec::new(), false),
    }
}

fn check_under_roots(path: &Path, roots: &[PathBuf], operation: &str) -> Result<(), String> {
    let target = crate::tools::builtin::normalize_path(path);
    if roots.iter().any(|root| is_path_within(&target, root)) {
        Ok(())
    } else {
        Err(format!(
            "Path `{}` is outside the allowed {operation} roots; use a workspace-relative path or an approved writable directory",
            target.display()
        ))
    }
}

fn is_path_within(path: &Path, root: &Path) -> bool {
    path == root || path.strip_prefix(root).is_ok()
}

fn normalize_root(path: &Path) -> PathBuf {
    crate::tools::builtin::normalize_path(path)
}

fn deduplicate_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
}

fn system_read_paths() -> Vec<PathBuf> {
    let mut paths = [
        "/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/proc", "/sys", "/run", "/dev",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.exists())
    .collect::<Vec<_>>();

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for relative in [".cargo/bin", ".rustup", ".gitconfig"] {
            let path = home.join(relative);
            if path.exists() {
                paths.push(path);
            }
        }
    }
    if let Some(path_var) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path_var).filter(|path| path.exists()));
    }
    paths
        .into_iter()
        .map(|path| normalize_root(&path))
        .collect()
}

fn filesystem_read_roots() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        vec![PathBuf::from("/")]
    }
    #[cfg(not(unix))]
    {
        std::env::current_dir()
            .ok()
            .and_then(|path| path.ancestors().last().map(Path::to_path_buf))
            .into_iter()
            .collect()
    }
}

pub(crate) fn temporary_write_paths() -> Vec<PathBuf> {
    let mut paths = ["/tmp", "/var/tmp", "/dev/null", "/dev/zero", "/dev/full"]
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if let Some(tmpdir) = std::env::var_os("TMPDIR").map(PathBuf::from) {
        if tmpdir.exists() {
            paths.push(tmpdir);
        }
    }
    paths
        .into_iter()
        .map(|path| normalize_root(&path))
        .collect()
}

#[cfg(target_os = "linux")]
fn apply_landlock(config: &ProcessSandboxSpec) -> Result<bool, String> {
    use landlock::{
        Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, RulesetStatus, ABI,
    };

    let abi = ABI::V7;
    let access_all = AccessFs::from_all(abi);
    let access_read = AccessFs::from_read(abi);
    let mut ruleset = Ruleset::default()
        .handle_access(access_all)
        .map_err(|error| error.to_string())?
        .create()
        .map_err(|error| error.to_string())?;

    for path in &config.read_only {
        if !path.exists() {
            continue;
        }
        let access = if path.is_dir() {
            access_read
        } else {
            access_read & AccessFs::from_file(abi)
        };
        let fd = PathFd::new(path).map_err(|error| error.to_string())?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|error| error.to_string())?;
    }
    for path in &config.read_write {
        if !path.exists() {
            continue;
        }
        let access = if path.is_dir() {
            access_all
        } else {
            AccessFs::from_file(abi)
        };
        let fd = PathFd::new(path).map_err(|error| error.to_string())?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|error| error.to_string())?;
    }

    let status = ruleset
        .set_compatibility(CompatLevel::BestEffort)
        .restrict_self()
        .map_err(|error| error.to_string())?;
    if status.ruleset == RulesetStatus::PartiallyEnforced {
        eprintln!("[sandbox] Landlock restrictions are only partially enforced");
    }
    Ok(status.ruleset != RulesetStatus::NotEnforced)
}

#[cfg(not(target_os = "linux"))]
fn apply_landlock(_config: &ProcessSandboxSpec) -> Result<bool, String> {
    Ok(false)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn apply_resource_limits(limits: &ResourceLimits) -> Result<(), String> {
    use nix::sys::resource::{getrlimit, setrlimit, Resource, RLIM_INFINITY};

    if !limits.enabled {
        return Ok(());
    }

    fn set_limit(resource: Resource, requested: u64) -> Result<(), String> {
        let (_, hard) = getrlimit(resource).map_err(|error| error.to_string())?;
        let limit = if hard == RLIM_INFINITY {
            requested
        } else {
            requested.min(hard)
        };
        setrlimit(resource, limit, limit).map_err(|error| error.to_string())
    }

    if limits.cpu_time_sec > 0 {
        set_limit(Resource::RLIMIT_CPU, limits.cpu_time_sec)?;
    }
    if limits.memory_bytes > 0 {
        set_limit(Resource::RLIMIT_AS, limits.memory_bytes)?;
    }
    if limits.file_size_bytes > 0 {
        set_limit(Resource::RLIMIT_FSIZE, limits.file_size_bytes)?;
    }
    if limits.max_processes > 0 {
        set_limit(Resource::RLIMIT_NPROC, limits.max_processes)?;
    }
    Ok(())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
fn apply_resource_limits(limits: &ResourceLimits) -> Result<(), String> {
    use nix::sys::resource::{getrlimit, setrlimit, Resource, RLIM_INFINITY};

    if !limits.enabled {
        return Ok(());
    }

    fn set_limit(resource: Resource, requested: u64) -> Result<(), String> {
        let (_, hard) = getrlimit(resource).map_err(|error| error.to_string())?;
        let limit = if hard == RLIM_INFINITY {
            requested
        } else {
            requested.min(hard)
        };
        setrlimit(resource, limit, limit).map_err(|error| error.to_string())
    }

    if limits.cpu_time_sec > 0 {
        set_limit(Resource::RLIMIT_CPU, limits.cpu_time_sec)?;
    }
    if limits.file_size_bytes > 0 {
        set_limit(Resource::RLIMIT_FSIZE, limits.file_size_bytes)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_resource_limits(_limits: &ResourceLimits) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn exec_target(target: OsString, args: Vec<OsString>) -> i32 {
    use std::os::unix::process::CommandExt;

    let error = std::process::Command::new(target)
        .args(args)
        .env_remove(SANDBOX_CONFIG_ENV)
        .exec();
    eprintln!("[sandbox] Unable to execute target: {error}");
    126
}

#[cfg(not(unix))]
fn exec_target(target: OsString, args: Vec<OsString>) -> i32 {
    match std::process::Command::new(target).args(args).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("[sandbox] Unable to execute target: {error}");
            126
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::policy::ToolPolicy;

    #[test]
    fn native_write_scope_is_limited_to_workspace() {
        let root = std::env::current_dir().unwrap();
        let workspace = root.join("target/sandbox-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut policy = ToolPolicy::default();
        policy.file.allowed_roots = vec![root.to_string_lossy().to_string()];
        policy.shell.allowed_cwd = vec![root.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));

        assert!(sandbox.check_read(&root.join("Cargo.toml")).is_ok());
        #[cfg(unix)]
        assert!(sandbox.check_read(Path::new("/etc/passwd")).is_ok());
        assert!(sandbox.check_write(&workspace.join("output.txt")).is_ok());
        assert!(sandbox.check_write(&root.join("outside.txt")).is_err());
        assert!(sandbox
            .check_write(&root.join(format!(
                "{}-sibling/output.txt",
                workspace.file_name().unwrap().to_string_lossy()
            )))
            .is_err());
        for protected in [".git", ".codex", ".agents"] {
            assert!(sandbox
                .check_write(&workspace.join(protected).join("metadata"))
                .is_err());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let outside = root.join("target/sandbox-outside");
            std::fs::create_dir_all(&outside).unwrap();
            let link = workspace.join("escape-link");
            let _ = std::fs::remove_file(&link);
            symlink(&outside, &link).unwrap();
            assert!(sandbox.check_write(&link.join("escaped.txt")).is_err());
            let _ = std::fs::remove_file(link);
            let _ = std::fs::remove_dir_all(outside);
        }
    }

    #[test]
    fn strict_read_mode_rejects_paths_outside_configured_roots() {
        let root = std::env::current_dir().unwrap();
        let workspace = root.join("target/sandbox-strict-read-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut policy = ToolPolicy::default();
        policy.sandbox.allow_read_outside_workspace = false;
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));

        assert!(sandbox.check_read(&workspace.join("README.md")).is_ok());
        #[cfg(unix)]
        assert!(sandbox.check_read(Path::new("/etc/passwd")).is_err());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn native_tools_can_use_system_temp_paths() {
        let workspace = std::env::current_dir()
            .unwrap()
            .join("target/sandbox-temp-workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let mut policy = ToolPolicy::default();
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));

        let temp_file =
            std::env::temp_dir().join(format!("agnes-sandbox-temp-{}.txt", uuid::Uuid::new_v4()));
        assert!(sandbox.check_read(&temp_file).is_ok());
        assert!(sandbox.check_write(&temp_file).is_ok());

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn read_only_shell_can_use_a_workspace_external_cwd() {
        let policy = ToolPolicy::default();
        let sandbox = PolicySandbox::new(&policy, Some(Path::new("/tmp/agnes-workspace")));

        assert!(sandbox.check_cwd(Path::new("/usr/share")).is_ok());
        assert!(sandbox
            .check_write(Path::new("/usr/share/agnes-output"))
            .is_err());
    }

    #[test]
    fn normalized_parent_path_cannot_escape_write_root() {
        let temp = tempfile::tempdir_in("target").unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let mut policy = ToolPolicy::default();
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));

        assert!(sandbox
            .check_write(&workspace.join("../etc/probe"))
            .is_err());
        let sibling = workspace
            .parent()
            .unwrap()
            .join("workspace-sibling")
            .join("probe");
        assert!(sandbox.check_write(&sibling).is_err());
    }

    #[test]
    fn inherited_environment_keeps_runtime_paths_and_filters_credentials() {
        assert!(safe_environment_name(std::ffi::OsStr::new("PATH")));
        assert!(safe_environment_name(std::ffi::OsStr::new("UV_CACHE_DIR")));
        assert!(!safe_environment_name(std::ffi::OsStr::new(
            "OPENAI_API_KEY"
        )));
        assert!(!safe_environment_name(std::ffi::OsStr::new(
            "CLIENT_SECRET"
        )));
        assert!(!safe_environment_name(std::ffi::OsStr::new("ACCESS_TOKEN")));
        assert!(!safe_environment_name(std::ffi::OsStr::new(
            "AGENT_PROTOCOL_TOKEN"
        )));

        let mut policy = ToolPolicy::default();
        policy.sandbox.bwrap = BwrapMode::Disabled;
        policy.sandbox.landlock = false;
        let sandbox = PolicySandbox::new(&policy, None);
        let command = sandbox.command_spec("true", &[], &[]).unwrap();
        if let Some(path) = std::env::var_os("PATH") {
            assert!(command
                .env
                .iter()
                .any(|(name, value)| name == "PATH" && value == &path));
        }
        assert!(command
            .env
            .iter()
            .all(|(name, _)| safe_environment_name(name)));
    }

    #[test]
    fn android_proot_maps_workspace_cwd_into_linux_root() {
        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let workspace = temp.path().join("workspace");
        let nested = workspace.join("src");
        let proot = temp.path().join("proot");
        let loader = temp.path().join("loader");
        let proot_tmp = temp.path().join("proot-tmp");
        let android_workspace_root = temp.path().join("user/workspace");
        std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&proot_tmp).unwrap();
        std::fs::create_dir_all(proot_tmp.join("shm")).unwrap();
        std::fs::create_dir_all(&android_workspace_root).unwrap();
        for path in [&rootfs.join("usr/bin/env"), &proot, &loader] {
            std::fs::write(path, "test").unwrap();
        }

        let runtime = AndroidProotProcessRuntime::new(AndroidProotRuntimeConfig::new(
            proot,
            loader.clone(),
            proot_tmp.clone(),
            rootfs.clone(),
            android_workspace_root.clone(),
            false,
        ));
        let command = runtime
            .command_spec(
                "bash",
                &["-c".into(), "pwd".into()],
                &[],
                Some(&workspace),
                Some(&nested),
            )
            .unwrap();
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let expected_prefix = vec![
            "-0".into(),
            "-r".into(),
            rootfs.to_string_lossy().to_string(),
            "-b".into(),
            "/dev:/dev".into(),
            "-b".into(),
            "/dev/pts:/dev/pts".into(),
            "-b".into(),
            "/proc/self/fd:/dev/fd".into(),
            "-b".into(),
            "/proc/self/fd/0:/dev/stdin".into(),
            "-b".into(),
            "/proc/self/fd/1:/dev/stdout".into(),
            "-b".into(),
            "/proc/self/fd/2:/dev/stderr".into(),
            "-b".into(),
            "/proc:/proc".into(),
            "-b".into(),
            "/sys:/sys".into(),
            "-b".into(),
            format!("{}:/tmp", proot_tmp.display()),
            "-b".into(),
            format!("{}:/dev/shm", proot_tmp.join("shm").display()),
            "-b".into(),
            format!("{}:/workspace", workspace.display()),
            "-b".into(),
            format!("{}:/root/workspace", android_workspace_root.display()),
            "-w".into(),
            "/workspace/src".into(),
            "/usr/bin/env".into(),
        ];
        assert_eq!(args[0..expected_prefix.len()], expected_prefix);
        assert_eq!(args[expected_prefix.len()], "bash");
        assert_eq!(args[expected_prefix.len() + 1], "-c");
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "PROOT_LOADER" && value == loader.as_os_str() }));
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "PROOT_TMP_DIR" && value == proot_tmp.as_os_str() }));
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "LANG" && value == "C.UTF-8" }));
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "LC_ALL" && value == "C.UTF-8" }));
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "UV_LINK_MODE" && value == "copy" }));
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "CI" && value == "1" }));
        assert!(command
            .env
            .iter()
            .any(|(name, value)| { name == "DEBIAN_FRONTEND" && value == "noninteractive" }));
        assert!(command.env.iter().any(|(name, value)| {
            name == "PATH"
                && value
                    .to_string_lossy()
                    .split(':')
                    .any(|path| path == "/usr/games")
        }));
        assert!(command
            .env
            .iter()
            .all(|(name, _)| safe_environment_name(name)));
    }

    #[test]
    fn android_proot_rejects_cwd_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        let android_workspace_root = temp.path().join("user/workspace");
        let proot_tmp = temp.path().join("proot-tmp");
        std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&android_workspace_root).unwrap();
        std::fs::create_dir_all(&proot_tmp).unwrap();
        std::fs::create_dir_all(proot_tmp.join("shm")).unwrap();
        for name in ["env", "proot", "loader"] {
            let path = if name == "env" {
                rootfs.join("usr/bin").join(name)
            } else {
                temp.path().join(name)
            };
            std::fs::write(path, "test").unwrap();
        }

        let runtime = AndroidProotProcessRuntime::new(AndroidProotRuntimeConfig::new(
            temp.path().join("proot"),
            temp.path().join("loader"),
            proot_tmp,
            rootfs,
            android_workspace_root,
            false,
        ));
        let error = runtime
            .command_spec("bash", &[], &[], Some(&workspace), Some(&outside))
            .unwrap_err();
        assert!(error.to_string().contains("not a mounted Runtime path"));
    }

    #[test]
    fn android_proot_maps_runtime_tmp_cwd_into_linux_root() {
        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let workspace = temp.path().join("workspace");
        let proot_tmp = temp.path().join("proot-tmp");
        let android_workspace_root = temp.path().join("user/workspace");
        let runtime_data = temp.path().join("runtime");
        let environments = temp.path().join("environments");
        let cache = temp.path().join("cache");
        std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&android_workspace_root).unwrap();
        std::fs::create_dir_all(proot_tmp.join("shm")).unwrap();
        for path in [&runtime_data, &environments, &cache] {
            std::fs::create_dir_all(path).unwrap();
        }
        for path in [
            &rootfs.join("usr/bin/env"),
            &temp.path().join("proot"),
            &temp.path().join("loader"),
        ] {
            std::fs::write(path, "test").unwrap();
        }

        let runtime = AndroidProotProcessRuntime::new(
            AndroidProotRuntimeConfig::new(
                temp.path().join("proot"),
                temp.path().join("loader"),
                proot_tmp.clone(),
                rootfs,
                android_workspace_root,
                false,
            )
            .with_runtime_mounts(runtime_data, environments, cache),
        );
        let command = runtime
            .command_spec(
                "bash",
                &["-c".into(), "pwd".into()],
                &[],
                Some(&workspace),
                Some(&proot_tmp),
            )
            .unwrap();
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|window| window[0] == "-w" && window[1] == "/tmp"));
    }

    #[test]
    fn android_proot_link2symlink_is_capability_controlled() {
        let config = |enabled| {
            AndroidProotRuntimeConfig::new(
                PathBuf::from("/proot"),
                PathBuf::from("/loader"),
                PathBuf::from("/tmp/proot"),
                PathBuf::from("/rootfs"),
                PathBuf::from("/workspace-root"),
                enabled,
            )
        };

        assert!(!config(false)
            .base_args()
            .iter()
            .any(|argument| argument == "--link2symlink"));
        assert!(config(true)
            .base_args()
            .iter()
            .any(|argument| argument == "--link2symlink"));
    }

    #[test]
    fn android_proot_runtime_mounts_have_stable_resource_ids_and_targets() {
        let config = AndroidProotRuntimeConfig::new(
            PathBuf::from("/proot"),
            PathBuf::from("/loader"),
            PathBuf::from("/tmp/proot"),
            PathBuf::from("/rootfs"),
            PathBuf::from("/android/workspace"),
            false,
        )
        .with_runtime_mounts(
            PathBuf::from("/runtime"),
            PathBuf::from("/runtime/environments"),
            PathBuf::from("/runtime/cache"),
        );

        let mounts = config.workspace_mounts(Path::new("/selected/workspace"));
        assert_eq!(
            mounts
                .iter()
                .map(|mount| (mount.resource_id, mount.target))
                .collect::<Vec<_>>(),
            vec![
                ("workspace", "/workspace"),
                ("project-root", "/root/workspace"),
                ("runtime-data", "/opt/agnes/runtime-data"),
                ("env-store", "/opt/agnes/envs"),
                ("runtime-cache", "/opt/agnes/cache"),
            ]
        );
        assert_eq!(
            config
                .base_mounts()
                .iter()
                .map(|mount| (mount.resource_id, mount.target))
                .collect::<Vec<_>>(),
            vec![
                ("android-dev", "/dev"),
                ("android-dev-pts", "/dev/pts"),
                ("android-dev-fd", "/dev/fd"),
                ("android-stdin", "/dev/stdin"),
                ("android-stdout", "/dev/stdout"),
                ("android-stderr", "/dev/stderr"),
                ("android-proc", "/proc"),
                ("android-sys", "/sys"),
                ("runtime-tmp", "/tmp"),
                ("runtime-shm", "/dev/shm"),
            ]
        );
    }

    #[test]
    fn android_agent_paths_use_the_fixed_agent_mount_table() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("Documents/Agnes/Home");
        let android_workspace_root = temp.path().join("android-runtime/user/workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&android_workspace_root).unwrap();

        let runtime = ToolProcessRuntime::AndroidProot(AndroidProotProcessRuntime::new(
            AndroidProotRuntimeConfig::new(
                PathBuf::from("/proot"),
                PathBuf::from("/loader"),
                PathBuf::from("/tmp/proot"),
                PathBuf::from("/rootfs"),
                android_workspace_root.clone(),
                false,
            ),
        ));

        assert_eq!(
            runtime.guest_path_for_host(&home.join(".agnes/cache/attachments"), &home),
            PathBuf::from("/workspace/.agnes/cache/attachments")
        );
        assert_eq!(
            runtime.guest_path_for_host(
                &android_workspace_root.join("tiny-tool/.agnes/cache/attachments"),
                &home,
            ),
            PathBuf::from("/root/workspace/tiny-tool/.agnes/cache/attachments")
        );
    }

    #[test]
    fn android_shared_storage_has_both_guest_aliases() {
        let config = AndroidProotRuntimeConfig::new(
            PathBuf::from("/proot"),
            PathBuf::from("/loader"),
            PathBuf::from("/tmp/proot"),
            PathBuf::from("/rootfs"),
            PathBuf::from("/android/workspace"),
            false,
        )
        .with_shared_storage(PathBuf::from("/storage/emulated/0"));
        let mounts = config.workspace_mounts(Path::new("/workspace"));
        assert_eq!(
            mounts
                .iter()
                .filter(|mount| mount.resource_id == "shared-storage")
                .map(|mount| mount.target)
                .collect::<Vec<_>>(),
            vec!["/sdcard", "/storage/emulated/0"]
        );
    }

    #[test]
    fn runtime_file_browser_reuses_mount_table_and_excludes_internals() {
        let config = AndroidProotRuntimeConfig::new(
            PathBuf::from("/proot"),
            PathBuf::from("/loader"),
            PathBuf::from("/tmp/proot"),
            PathBuf::from("/rootfs"),
            PathBuf::from("/android/projects"),
            false,
        )
        .with_runtime_mounts(
            PathBuf::from("/runtime"),
            PathBuf::from("/runtime/environments"),
            PathBuf::from("/runtime/cache"),
        )
        .with_user_workspace(PathBuf::from("/android/workspace"))
        .with_user_home(PathBuf::from("/runtime/home"))
        .with_shared_storage(PathBuf::from("/storage/emulated/0"));

        let home = config
            .resolve_user_visible_file_path("/root/notes.txt")
            .unwrap();
        assert_eq!(home.resource_id, "user-home");
        assert_eq!(home.host_path, PathBuf::from("/runtime/home/notes.txt"));
        let storage = config
            .resolve_user_visible_file_path("/storage/emulated/0/Download/app.py")
            .unwrap();
        assert_eq!(storage.resource_id, "shared-storage");
        assert_eq!(
            storage.host_path,
            PathBuf::from("/storage/emulated/0/Download/app.py")
        );
        assert!(config
            .resolve_user_visible_file_path("/opt/agnes/runtime-data/config.json")
            .is_err());
        assert!(config
            .resolve_user_visible_file_path("relative.txt")
            .is_err());
    }

    #[test]
    fn runtime_failure_categories_remain_distinct() {
        assert_eq!(
            classify_runtime_failure("Sandbox denied permission to write protected metadata"),
            "policy_denied"
        );
        assert_eq!(
            classify_runtime_failure("Read-only file system (EROFS)"),
            "filesystem_read_only"
        );
        assert_eq!(
            classify_runtime_failure("No such file or directory"),
            "path_not_found"
        );
        assert_eq!(
            classify_runtime_failure("ln: failed to create symbolic link: Permission denied"),
            "android_filesystem_unsupported"
        );
        assert_eq!(
            classify_runtime_failure("bash: ./tool: Permission denied"),
            "permission_denied"
        );
    }

    #[test]
    fn android_sandbox_resolves_guest_paths_to_runtime_sources() {
        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let workspace = temp.path().join("workspace");
        let project_root = temp.path().join("user/workspace");
        let home = temp.path().join("user/home");
        let runtime_data = temp.path().join("runtime");
        let environments = runtime_data.join("environments");
        let cache = runtime_data.join("cache");
        let proot_tmp = temp.path().join("proot-tmp");
        let shared_storage = temp.path().join("storage/emulated/0");
        for path in [
            rootfs.join("etc"),
            workspace.clone(),
            project_root.clone(),
            home.join(".cache"),
            environments.clone(),
            cache.clone(),
            proot_tmp.join("shm"),
            shared_storage.clone(),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }

        let runtime = ToolProcessRuntime::AndroidProot(AndroidProotProcessRuntime::new(
            AndroidProotRuntimeConfig::new(
                temp.path().join("proot"),
                temp.path().join("loader"),
                proot_tmp.clone(),
                rootfs.clone(),
                project_root,
                false,
            )
            .with_runtime_mounts(runtime_data.clone(), environments.clone(), cache)
            .with_user_home(home.clone())
            .with_shared_storage(shared_storage.clone()),
        ));
        let sandbox = PolicySandbox::new_with_process_runtime(
            &ToolPolicy::default(),
            Some(&workspace),
            runtime,
        );

        assert_eq!(
            sandbox
                .resolve_guest_path("/etc/os-release", Some(&workspace))
                .unwrap()
                .0,
            rootfs.join("etc/os-release")
        );
        assert_eq!(
            sandbox
                .resolve_guest_path("/tmp/build", Some(&workspace))
                .unwrap()
                .0,
            proot_tmp.join("build")
        );
        assert_eq!(
            sandbox
                .resolve_guest_path("~/.cache/uv", Some(&workspace))
                .unwrap()
                .0,
            home.join(".cache/uv")
        );
        assert_eq!(
            sandbox
                .resolve_guest_path("/opt/agnes/envs/project", Some(&workspace))
                .unwrap()
                .0,
            environments.join("project")
        );
        assert_eq!(
            sandbox
                .resolve_guest_path("/sdcard/source.py", Some(&workspace))
                .unwrap()
                .0,
            shared_storage.join("source.py")
        );
        assert_eq!(
            sandbox
                .resolve_guest_path("/storage/emulated/0/source.py", Some(&workspace))
                .unwrap()
                .0,
            shared_storage.join("source.py")
        );
        assert!(sandbox.check_read(&rootfs.join("etc/os-release")).is_ok());
        assert!(sandbox.check_write(&proot_tmp.join("build")).is_ok());
        assert!(sandbox.check_write(&home.join(".cache/uv")).is_ok());
        assert!(sandbox
            .check_read(&shared_storage.join("source.py"))
            .is_ok());
        assert!(sandbox
            .check_write(&shared_storage.join("source.py"))
            .is_ok());
        assert!(sandbox.check_write(&rootfs.join("etc/override")).is_err());
    }

    #[test]
    fn android_user_terminal_maps_guest_cwds_to_runtime_mounts() {
        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let selected_workspace = temp.path().join("selected-workspace");
        let project_root = temp.path().join("user/workspace");
        let home = temp.path().join("user/home");
        let runtime_data = temp.path().join("runtime");
        let environments = runtime_data.join("environments");
        let cache = runtime_data.join("cache");
        let proot = temp.path().join("proot");
        let loader = temp.path().join("loader");
        let proot_tmp = temp.path().join("proot-tmp");
        std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
        std::fs::create_dir_all(&selected_workspace).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&environments).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(proot_tmp.join("shm")).unwrap();
        std::fs::write(rootfs.join("usr/bin/env"), "test").unwrap();
        std::fs::write(&proot, "test").unwrap();
        std::fs::write(&loader, "test").unwrap();

        let config = AndroidProotRuntimeConfig::new(
            proot,
            loader,
            proot_tmp,
            rootfs,
            project_root.clone(),
            false,
        )
        .with_runtime_mounts(runtime_data, environments, cache)
        .with_user_workspace(selected_workspace.clone())
        .with_user_home(home.clone());

        for (guest_cwd, expected_host_cwd) in [
            ("/root", home),
            ("/root/workspace", project_root),
            ("/workspace", selected_workspace),
            ("/opt/agnes/runtime-data", temp.path().join("runtime")),
            ("/opt/agnes/envs", temp.path().join("runtime/environments")),
            ("/opt/agnes/cache", temp.path().join("runtime/cache")),
        ] {
            let (command, host_cwd, normalized_guest_cwd) =
                config.user_command_spec(Some(guest_cwd)).unwrap();
            assert_eq!(host_cwd, expected_host_cwd);
            assert_eq!(normalized_guest_cwd, Path::new(guest_cwd));
            let args = command
                .args
                .iter()
                .map(|argument| argument.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            assert!(args
                .windows(2)
                .any(|window| window[0] == "-w" && window[1] == guest_cwd));
            assert!(args
                .windows(2)
                .any(|window| window[0] == "/usr/bin/env" && window[1] == "bash"));
        }
    }

    #[test]
    fn android_user_terminal_rejects_relative_and_parent_cwds() {
        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let home = temp.path().join("home");
        let workspace = temp.path().join("workspace");
        let proot = temp.path().join("proot");
        let loader = temp.path().join("loader");
        let proot_tmp = temp.path().join("proot-tmp");
        std::fs::create_dir_all(rootfs.join("usr/bin")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(proot_tmp.join("shm")).unwrap();
        std::fs::write(rootfs.join("usr/bin/env"), "test").unwrap();
        std::fs::write(&proot, "test").unwrap();
        std::fs::write(&loader, "test").unwrap();
        let config = AndroidProotRuntimeConfig::new(
            proot,
            loader,
            proot_tmp,
            rootfs,
            workspace.clone(),
            false,
        )
        .with_user_workspace(workspace)
        .with_user_home(home);

        let relative = config.user_command_spec(Some("root"));
        assert!(relative
            .unwrap_err()
            .to_string()
            .contains("absolute guest path"));
        let parent = config.user_command_spec(Some("/root/../workspace"));
        assert!(parent
            .unwrap_err()
            .to_string()
            .contains("cannot contain `..`"));
    }

    #[tokio::test]
    async fn hidden_exec_uses_null_stdin_and_reports_interactive_output() {
        let result = execute_hidden(
            SandboxCommand::direct(
                "/bin/bash".to_string(),
                vec![
                    "-c".to_string(),
                    "printf 'Do you want to continue? [Y/n]' >&2".to_string(),
                ],
                Vec::new(),
            ),
            Path::new("."),
            1024,
            5,
        )
        .await
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
        assert!(result.output_requests_input);
        assert!(result.stderr.contains("Do you want to continue?"));
    }

    #[test]
    fn hidden_exec_detects_commands_that_need_a_terminal() {
        assert!(crate::tools::builtin::shell::command_likely_interactive(
            "apt install jq"
        ));
        assert!(!crate::tools::builtin::shell::command_likely_interactive(
            "apt-get install -y jq"
        ));
        assert!(crate::tools::builtin::shell::command_likely_interactive(
            "read answer"
        ));
    }

    #[tokio::test]
    async fn hidden_exec_times_out_and_caps_output() {
        let result = execute_hidden(
            SandboxCommand::direct(
                "/bin/bash".to_string(),
                vec!["-c".to_string(), "printf 123456789; sleep 5".to_string()],
                Vec::new(),
            ),
            Path::new("."),
            4,
            1,
        )
        .await
        .unwrap();

        assert!(result.timed_out);
        assert_eq!(result.exit_code, Some(-9));
        assert!(result.output_truncated);
        assert!(result.stdout.contains("output truncated"));
    }

    #[cfg(unix)]
    #[test]
    fn configured_writable_root_resolves_symlink_target() {
        use std::os::unix::fs::symlink;

        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("sandbox-writable-root-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let external_cache = root.join("external-cache");
        let linked_cache = root.join("home-cache");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(external_cache.join("uv")).unwrap();
        symlink(&external_cache, &linked_cache).unwrap();

        let mut policy = ToolPolicy::default();
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        policy.sandbox.writable_roots = vec![linked_cache.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));

        let cache_lock = linked_cache.join("uv/.lock");
        assert!(policy
            .check_file_write(&cache_lock.to_string_lossy())
            .is_ok());
        assert!(sandbox.check_read(&cache_lock).is_ok());
        assert!(sandbox.check_write(&cache_lock).is_ok());
        assert!(sandbox
            .check_write(&root.join("unrelated/output.txt"))
            .is_err());
        assert!(sandbox
            .process_spec
            .read_write
            .contains(&external_cache.canonicalize().unwrap()));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn network_fallback_rejects_known_network_commands() {
        let mut policy = ToolPolicy::default();
        policy.network.allow = false;
        policy.sandbox.bwrap = BwrapMode::Disabled;
        policy.sandbox.landlock = false;
        policy.sandbox.rlimits = false;
        let sandbox = PolicySandbox::new(&policy, None);
        let curl = vec!["-c".to_string(), "curl https://example.com".to_string()];
        assert!(sandbox
            .command("bash", &curl, &["PATH".to_string()])
            .is_err());
        let local = vec!["-c".to_string(), "printf local".to_string()];
        assert!(sandbox
            .command("bash", &local, &["PATH".to_string()])
            .is_ok());
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bubblewrap_blocks_loopback_connections_when_available() {
        if detect_bwrap().is_none() {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let workspace = std::env::current_dir()
            .unwrap()
            .join("target/bwrap-network");
        std::fs::create_dir_all(&workspace).unwrap();

        let mut policy = ToolPolicy::default();
        policy.network.allow = false;
        policy.sandbox.landlock = false;
        policy.sandbox.rlimits = false;
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));
        let args = vec![
            "-c".to_string(),
            format!("exec 3<>/dev/tcp/127.0.0.1/{port}"),
        ];
        let mut command = sandbox
            .command("bash", &args, &["PATH".to_string()])
            .unwrap();
        command.current_dir(&workspace);
        let status = command.status().await.unwrap();
        assert!(!status.success());
        drop(listener);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(all(target_os = "linux", unix))]
    #[tokio::test]
    async fn bubblewrap_mounts_symlinked_writable_roots_read_write() {
        use std::os::unix::fs::symlink;

        if detect_bwrap().is_none() {
            return;
        }
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("bwrap-writable-root-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let external_cache = root.join("external-cache");
        let linked_cache = root.join("home-cache");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(external_cache.join("uv")).unwrap();
        symlink(&external_cache, &linked_cache).unwrap();

        let mut policy = ToolPolicy::default();
        policy.sandbox.landlock = false;
        policy.sandbox.rlimits = false;
        policy.sandbox.writable_roots = vec![linked_cache.to_string_lossy().to_string()];
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));
        let marker = linked_cache.join("uv/package-manager.lock");
        let mut command = sandbox
            .command(
                "touch",
                &[marker.to_string_lossy().to_string()],
                &["PATH".to_string()],
            )
            .unwrap();
        command.current_dir(&workspace);
        assert!(command.status().await.unwrap().success());
        assert!(external_cache.join("uv/package-manager.lock").is_file());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bubblewrap_protects_metadata_but_git_command_can_update_it() {
        if detect_bwrap().is_none() {
            return;
        }
        let workspace = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("bwrap-protected-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&workspace)
            .status()
            .unwrap()
            .success());

        let mut policy = ToolPolicy::default();
        policy.sandbox.landlock = false;
        policy.sandbox.rlimits = false;
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));

        let mut generic = sandbox
            .command(
                "touch",
                &[".git/blocked".to_string()],
                &["PATH".to_string()],
            )
            .unwrap();
        generic.current_dir(&workspace);
        assert!(!generic.status().await.unwrap().success());
        assert!(!workspace.join(".git/blocked").exists());

        let mut git = sandbox
            .git_command(
                "git",
                &[
                    "config".to_string(),
                    "sandbox.test".to_string(),
                    "allowed".to_string(),
                ],
                &["PATH".to_string()],
            )
            .unwrap();
        git.current_dir(&workspace);
        assert!(git.status().await.unwrap().success());

        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn bubblewrap_command_runs_inside_a_pty() {
        if detect_bwrap().is_none() {
            return;
        }
        let workspace = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("bwrap-pty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut policy = ToolPolicy::default();
        policy.sandbox.landlock = false;
        policy.sandbox.rlimits = false;
        policy.shell.allowed_cwd = vec![workspace.to_string_lossy().to_string()];
        policy.file.allowed_roots = vec![workspace.to_string_lossy().to_string()];
        let sandbox = PolicySandbox::new(&policy, Some(&workspace));
        let command = sandbox
            .command_spec(
                "bash",
                &["-c".to_string(), "printf bwrap-pty-ok".to_string()],
                &["PATH".to_string(), "HOME".to_string()],
            )
            .unwrap();
        let terminals = crate::tools::terminal::TerminalManager::default();
        let id = terminals
            .spawn(command, &workspace, "owner", "run", 1024, 0)
            .unwrap();

        let mut completed = None;
        for _ in 0..100 {
            let poll = terminals
                .poll(&id, "owner", std::time::Duration::from_millis(50))
                .await
                .unwrap();
            if !poll.running {
                completed = Some(poll);
                break;
            }
        }
        let poll = completed.expect("bubblewrap PTY command did not exit");
        assert_eq!(poll.exit_code, Some(0), "{}", poll.output);
        assert!(poll.output.contains("bwrap-pty-ok"));

        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn landlock_enforces_write_boundary_when_available() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target/landlock-boundary");
        let writable = root.join("writable");
        std::fs::create_dir_all(&writable).unwrap();
        let writable_for_thread = writable.clone();
        let outside = root.join("outside.txt");
        let outside_for_thread = outside.clone();

        let enforced = std::thread::spawn(move || {
            let config = ProcessSandboxSpec {
                landlock: true,
                read_only: Vec::new(),
                read_write: vec![writable_for_thread.clone()],
                limits: ResourceLimits {
                    enabled: false,
                    cpu_time_sec: 0,
                    memory_bytes: 0,
                    file_size_bytes: 0,
                    max_processes: 0,
                },
            };
            let enforced = apply_landlock(&config).unwrap();
            if enforced {
                std::fs::write(writable_for_thread.join("inside.txt"), "allowed").unwrap();
                assert!(std::fs::write(outside_for_thread, "denied").is_err());
            }
            enforced
        })
        .join()
        .unwrap();

        if enforced {
            assert!(writable.join("inside.txt").exists());
            assert!(!outside.exists());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
