//! Process and path sandbox shared by all built-in tools.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::error::{AppError, AppResult};
use crate::tools::policy::{expand_home, ToolPolicy};

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

pub trait SandboxGuard: Send + Sync {
    fn check_read(&self, path: &Path) -> Result<(), String>;
    fn check_write(&self, path: &Path) -> Result<(), String>;
    fn check_cwd(&self, path: &Path) -> Result<(), String>;
    fn command(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<Command>;
}

pub struct PolicySandbox {
    read_roots: Vec<PathBuf>,
    write_roots: Vec<PathBuf>,
    cwd_roots: Vec<PathBuf>,
    process_spec: ProcessSandboxSpec,
}

impl PolicySandbox {
    pub fn new(policy: &ToolPolicy, workspace_cwd: Option<&Path>) -> Self {
        let mut read_roots = policy
            .file
            .allowed_roots
            .iter()
            .map(|path| normalize_root(&expand_home(path)))
            .collect::<Vec<_>>();
        let cwd_roots = policy
            .shell
            .allowed_cwd
            .iter()
            .map(|path| normalize_root(&expand_home(path)))
            .collect::<Vec<_>>();

        let primary_write_root = workspace_cwd
            .map(normalize_root)
            .or_else(|| cwd_roots.first().cloned());
        if let Some(workspace) = workspace_cwd.map(normalize_root) {
            read_roots.push(workspace);
        }
        deduplicate_paths(&mut read_roots);

        let mut write_roots = primary_write_root.into_iter().collect::<Vec<_>>();
        deduplicate_paths(&mut write_roots);

        let mut process_read_only = read_roots.clone();
        process_read_only.extend(cwd_roots.iter().cloned());
        process_read_only.extend(system_read_paths());
        deduplicate_paths(&mut process_read_only);

        let mut process_read_write = if policy.shell.deny_write_outside_workspace {
            write_roots.clone()
        } else {
            let mut roots = read_roots.clone();
            roots.extend(cwd_roots.iter().cloned());
            roots
        };
        process_read_write.extend(temporary_write_paths());
        deduplicate_paths(&mut process_read_write);

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
        }
    }
}

impl SandboxGuard for PolicySandbox {
    fn check_read(&self, path: &Path) -> Result<(), String> {
        check_under_roots(path, &self.read_roots, "read")
    }

    fn check_write(&self, path: &Path) -> Result<(), String> {
        check_under_roots(path, &self.write_roots, "write")
    }

    fn check_cwd(&self, path: &Path) -> Result<(), String> {
        check_under_roots(path, &self.cwd_roots, "use as a working directory")
    }

    fn command(
        &self,
        program: &str,
        args: &[String],
        env_allowlist: &[String],
    ) -> AppResult<Command> {
        let use_helper = !cfg!(test)
            && (self.process_spec.landlock || self.process_spec.limits.enabled)
            && cfg!(unix);
        let mut command = if use_helper {
            let current_exe = std::env::current_exe().map_err(|error| {
                AppError::Other(format!(
                    "Unable to locate sandbox helper executable: {error}"
                ))
            })?;
            let mut command = Command::new(current_exe);
            command.arg(SANDBOX_HELPER_FLAG).arg(program).args(args);
            command
        } else {
            let mut command = Command::new(program);
            command.args(args);
            command
        };

        command.env_clear();
        for name in env_allowlist {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        if use_helper {
            let config = serde_json::to_string(&self.process_spec)?;
            command.env(SANDBOX_CONFIG_ENV, config);
        }
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(command)
    }
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
    if roots.iter().any(|root| target.starts_with(root)) {
        Ok(())
    } else {
        Err(format!(
            "Sandbox denied permission to {operation} `{}`",
            target.display()
        ))
    }
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

fn temporary_write_paths() -> Vec<PathBuf> {
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

    set_limit(Resource::RLIMIT_CPU, limits.cpu_time_sec)?;
    set_limit(Resource::RLIMIT_AS, limits.memory_bytes)?;
    set_limit(Resource::RLIMIT_FSIZE, limits.file_size_bytes)?;
    set_limit(Resource::RLIMIT_NPROC, limits.max_processes)?;
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

    set_limit(Resource::RLIMIT_CPU, limits.cpu_time_sec)?;
    set_limit(Resource::RLIMIT_FSIZE, limits.file_size_bytes)?;
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
        assert!(sandbox.check_write(&workspace.join("output.txt")).is_ok());
        assert!(sandbox.check_write(&root.join("outside.txt")).is_err());

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
