//! 内置工具统一 trait 与执行上下文。
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::db::DbActorHandle;
use crate::error::AppResult;
use crate::secrets::SecretStore;
use crate::storage::StorageService;
use crate::tools::policy::{ApprovalTier, Risk, ToolPolicy};
use crate::tools::sandbox::SandboxGuard;
use crate::tools::terminal::TerminalManager;

pub mod apply_patch;
pub mod clock;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod git;
pub mod grep;
pub mod list_files;
pub mod memory_files;
pub mod planner;
pub mod shell;
pub mod storage;
pub mod web;

/// Narrow credential facade exposed to built-in tools.
pub struct SearchSecretAccess<'a> {
    store: &'a dyn SecretStore,
}

impl<'a> SearchSecretAccess<'a> {
    pub(crate) fn new(store: &'a dyn SecretStore) -> Self {
        Self { store }
    }

    pub async fn brave_api_key(&self) -> AppResult<Option<String>> {
        self.store
            .get(crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID)
            .await
    }
}

/// 工具执行上下文：包含审计所需的 DB 句柄、参数、policy 与 workspace cwd。
pub struct ToolCtx<'a> {
    pub db: &'a DbActorHandle,
    pub search_secrets: SearchSecretAccess<'a>,
    pub session_id: &'a str,
    pub run_id: Option<&'a str>,
    pub tool_call_id: &'a str,
    pub args: &'a Value,
    /// 已合并 workspace cwd 的有效 policy（workspace 自动加入 allowed_cwd/allowed_roots）
    pub policy: &'a ToolPolicy,
    pub workspace_cwd: Option<PathBuf>,
    pub sandbox: &'a dyn SandboxGuard,
    pub terminals: &'a TerminalManager,
    pub storage: &'a StorageService,
}

impl<'a> ToolCtx<'a> {
    /// 记录运行中状态（含 cwd）。
    pub async fn update_running(&self, cwd: &str) -> AppResult<()> {
        self.db
            .update_tool_call_running(self.tool_call_id.to_string(), cwd.to_string())
            .await
    }

    /// 记录执行完成（成功/失败）。
    pub async fn update_complete(
        &self,
        status: &str,
        result_kind: Option<&str>,
        exit_code: Option<i32>,
        stdout: Option<String>,
        stderr: Option<String>,
    ) -> AppResult<()> {
        self.db
            .update_tool_call_complete(
                self.tool_call_id.to_string(),
                status.to_string(),
                result_kind.map(|s| s.to_string()),
                exit_code,
                stdout,
                stderr,
            )
            .await
    }

    /// 统一失败落库。
    pub async fn record_failure(&self, err_msg: &str) -> AppResult<()> {
        self.update_complete("failed", None, Some(-1), None, Some(err_msg.to_string()))
            .await
    }
}

/// 内置工具统一接口。
#[async_trait]
pub trait BuiltinTool: Send + Sync {
    /// Stable name used in the LLM protocol.
    fn name(&self) -> &'static str;
    /// JSON schema exposed to model providers.
    fn schema(&self) -> Value;
    /// Risk for the supplied arguments.
    fn risk(&self, args: &Value) -> Risk;
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value>;
}

/// Return all registered built-in tools.
pub fn builtin_tools() -> Vec<Box<dyn BuiltinTool>> {
    vec![
        Box::new(clock::GetCurrentTimeTool),
        Box::new(shell::ShellTool),
        Box::new(shell::WriteStdinTool),
        Box::new(shell::StopTerminalTool),
        Box::new(file_read::FileReadTool),
        Box::new(file_write::FileWriteTool),
        Box::new(file_edit::FileEditTool),
        Box::new(list_files::ListFilesTool),
        Box::new(grep::GrepTool),
        Box::new(apply_patch::ApplyPatchTool),
        Box::new(git::GitTool),
        Box::new(memory_files::MemoryListTool),
        Box::new(memory_files::MemoryReadTool),
        Box::new(memory_files::MemoryWriteTool),
        Box::new(memory_files::MemoryStrReplaceTool),
        Box::new(memory_files::MemoryAppendTool),
        Box::new(memory_files::MemoryDeleteTool),
        Box::new(web::WebSearchTool),
        Box::new(web::WebFetchTool),
        Box::new(web::BrowserOpenTool),
        Box::new(planner::CalendarListTool),
        Box::new(planner::CalendarCreateTool),
        Box::new(planner::CalendarEventCreateTool),
        Box::new(planner::CalendarUpdateTool),
        Box::new(planner::TaskListTool),
        Box::new(planner::TaskCreateTool),
        Box::new(planner::TaskCompleteTool),
        Box::new(planner::TaskUpdateTool),
        Box::new(storage::StorageAccountsListTool),
        Box::new(storage::StorageFilesListTool),
        Box::new(storage::StorageFilesSearchTool),
        Box::new(storage::StorageFileDownloadTool),
        Box::new(storage::StorageFileUploadTool),
        Box::new(storage::StorageTransferStatusTool),
        Box::new(storage::StorageQuotaGetTool),
        Box::new(storage::StorageFilesMoveTool),
        Box::new(storage::StorageFilesTrashTool),
    ]
}

/// A path resolved in both the guest namespace and the host resource namespace.
/// Host paths remain internal to Rust; the stable resource ID is what policy and
/// audit code should use when it needs to identify a mounted resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedGuestPath {
    pub host_path: PathBuf,
    pub guest_path: PathBuf,
    pub resource_id: String,
}

/// One guest-visible bind mount and its host-side resource identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestMount {
    pub resource_id: String,
    pub host_root: PathBuf,
    pub guest_root: PathBuf,
}

impl GuestMount {
    pub fn new(
        resource_id: impl Into<String>,
        host_root: impl Into<PathBuf>,
        guest_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            resource_id: resource_id.into(),
            host_root: host_root.into(),
            guest_root: guest_root.into(),
        }
    }
}

/// Mount table shared by host-side tools and guest process launchers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestMountTable {
    mounts: Vec<GuestMount>,
}

impl GuestMountTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, mount: GuestMount) {
        self.mounts.push(mount);
    }

    fn matching_guest_mount(&self, path: &Path) -> Option<(&GuestMount, PathBuf)> {
        self.mounts
            .iter()
            .filter_map(|mount| {
                let relative = if path == mount.guest_root {
                    Some(PathBuf::new())
                } else {
                    path.strip_prefix(&mount.guest_root)
                        .ok()
                        .map(Path::to_path_buf)
                }?;
                Some((mount, relative))
            })
            .max_by_key(|(mount, _)| mount.guest_root.components().count())
            .map(|(mount, relative)| (mount, relative))
    }

    fn matching_host_mount(&self, path: &Path) -> Option<(&GuestMount, PathBuf)> {
        self.mounts
            .iter()
            .filter_map(|mount| {
                let relative = if path == mount.host_root {
                    Some(PathBuf::new())
                } else {
                    path.strip_prefix(&mount.host_root)
                        .ok()
                        .map(Path::to_path_buf)
                }?;
                Some((mount, relative))
            })
            .max_by_key(|(mount, _)| mount.host_root.components().count())
            .map(|(mount, relative)| (mount, relative))
    }
}

/// Shared guest/host path resolver for all built-in tools and policy helpers.
#[derive(Clone, Debug)]
pub struct GuestPathResolver {
    mounts: GuestMountTable,
    workspace_root: Option<PathBuf>,
    fallback_root: PathBuf,
}

impl GuestPathResolver {
    pub fn new(workspace_root: Option<&Path>, fallback_root: Option<&Path>) -> Self {
        let mut resolver = Self::with_mounts(workspace_root, fallback_root, GuestMountTable::new());
        if let Some(workspace) = workspace_root {
            resolver = resolver
                .with_mount(GuestMount::new("workspace", workspace, "/workspace"))
                .with_mount(GuestMount::new("workspace", workspace, "/root/workspace"));
        }
        resolver
    }

    pub fn with_mounts(
        workspace_root: Option<&Path>,
        fallback_root: Option<&Path>,
        mounts: GuestMountTable,
    ) -> Self {
        Self {
            mounts,
            workspace_root: workspace_root.map(Path::to_path_buf),
            fallback_root: fallback_root
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    pub fn with_mount(mut self, mount: GuestMount) -> Self {
        self.mounts.add(mount);
        self
    }

    pub fn from_context(ctx: &ToolCtx<'_>) -> Self {
        Self::new(
            ctx.workspace_cwd.as_deref(),
            ctx.policy
                .shell
                .allowed_cwd
                .first()
                .map(|path| Path::new(path)),
        )
    }

    /// Resolve a model-supplied path into the host path used by native tools.
    pub fn resolve(&self, raw: &str) -> ResolvedGuestPath {
        self.resolve_from_cwd(raw, self.workspace_root.as_deref())
    }

    /// Resolve a path relative to an explicit host cwd while preserving guest
    /// aliases such as `/workspace` and `/root/workspace`.
    pub fn resolve_from_cwd(&self, raw: &str, cwd: Option<&Path>) -> ResolvedGuestPath {
        let raw_path = Path::new(raw);

        if raw == "~" || raw.starts_with("~/") {
            let relative = raw.strip_prefix("~/").unwrap_or_default();
            let guest_home = Path::new("/root");
            let guest_path = if relative.is_empty() {
                guest_home.to_path_buf()
            } else {
                guest_home.join(relative)
            };
            if let Some((mount, mount_relative)) = self.mounts.matching_guest_mount(&guest_path) {
                return ResolvedGuestPath {
                    host_path: mount.host_root.join(mount_relative),
                    guest_path,
                    resource_id: mount.resource_id.clone(),
                };
            }
            return ResolvedGuestPath {
                host_path: crate::tools::policy::expand_home(raw),
                guest_path,
                resource_id: "guest-home".into(),
            };
        }

        if raw_path.is_absolute() {
            if let Some((mount, relative)) = self.mounts.matching_guest_mount(raw_path) {
                return ResolvedGuestPath {
                    host_path: mount.host_root.join(relative),
                    guest_path: raw_path.to_path_buf(),
                    resource_id: mount.resource_id.clone(),
                };
            }
            return ResolvedGuestPath {
                host_path: raw_path.to_path_buf(),
                guest_path: raw_path.to_path_buf(),
                resource_id: format!("host:{}", raw_path.display()),
            };
        }

        let base = cwd
            .or(self.workspace_root.as_deref())
            .unwrap_or(&self.fallback_root);
        let host_path = base.join(raw_path);
        if let Some((mount, relative)) = self.mounts.matching_host_mount(&host_path) {
            return ResolvedGuestPath {
                host_path,
                guest_path: mount.guest_root.join(relative),
                resource_id: mount.resource_id.clone(),
            };
        }
        ResolvedGuestPath {
            host_path,
            guest_path: Path::new("/workspace").join(raw_path),
            resource_id: "host".into(),
        }
    }

    pub fn resolve_host(&self, raw: &str) -> PathBuf {
        self.resolve(raw).host_path
    }
}

/// Resolve a file argument relative to the current workspace.
pub fn resolve_path(ctx: &ToolCtx<'_>, path: &str) -> PathBuf {
    GuestPathResolver::from_context(ctx).resolve_host(path)
}

/// Resolve a path with the same fallback rules as built-in tools, without a
/// live tool context. Used by approval and review code before execution.
pub fn resolve_path_from_workspace(
    path: &str,
    workspace_cwd: Option<&Path>,
    fallback_root: Option<&Path>,
) -> ResolvedGuestPath {
    GuestPathResolver::new(workspace_cwd, fallback_root).resolve(path)
}

/// Resolve a path as far as possible without requiring the leaf to exist.
pub fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut existing = path;
    let mut missing = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return path.to_path_buf();
        };
        missing.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            return path.to_path_buf();
        };
        if parent == existing {
            return path.to_path_buf();
        }
        existing = parent;
    }
    if let Ok(mut canonical) = existing.canonicalize() {
        for component in missing.into_iter().rev() {
            canonical.push(component);
        }
        canonical
    } else {
        path.to_path_buf()
    }
}

/// Compute the risk of a registered tool call.
pub fn compute_risk(tool: &str, args: &Value) -> Risk {
    for impl_ in builtin_tools() {
        if impl_.name() == tool {
            return impl_.risk(args);
        }
    }
    Risk::High
}

/// 判断操作是否为写操作（用于 OnWrite tier）。
pub fn is_write_op(tool: &str, args: &Value) -> bool {
    match tool {
        "file_write"
        | "file_edit"
        | "apply_patch"
        | "memory_write"
        | "memory_str_replace"
        | "memory_append"
        | "memory_delete"
        | "calendar_create"
        | "calendar_event_create"
        | "calendar_update"
        | "task_create"
        | "task_complete"
        | "task_update" => true,
        "storage_file_download"
        | "storage_file_upload"
        | "storage_files_move"
        | "storage_files_trash" => true,
        "file_read" | "list_files" | "grep" | "memory_list" | "memory_read" | "web_search"
        | "web_fetch" | "browser_open" | "calendar_list" | "task_list" | "get_current_time" => {
            false
        }
        "storage_accounts_list"
        | "storage_files_list"
        | "storage_files_search"
        | "storage_transfer_status"
        | "storage_quota_get" => false,
        "shell" => {
            let cmd = args.get("command").and_then(|x| x.as_str()).unwrap_or("");
            shell::command_is_write(cmd)
        }
        "write_stdin" => args
            .get("chars")
            .and_then(|value| value.as_str())
            .is_some_and(|chars| !chars.is_empty()),
        "stop_terminal" => false,
        "git" => {
            let arr = args.get("args").and_then(|x| x.as_array());
            const WRITE_CMDS: &[&str] = &[
                "push",
                "commit",
                "reset",
                "clean",
                "merge",
                "rebase",
                "checkout",
                "cherry-pick",
                "stash",
                "tag",
                "add",
                "archive",
                "bisect",
                "config",
                "fetch",
                "hash-object",
                "notes",
                "rm",
                "mv",
                "init",
                "pull",
                "restore",
                "revert",
                "switch",
                "update-index",
                "update-ref",
            ];
            arr.map(|a| {
                a.iter()
                    .any(|v| v.as_str().map(|s| WRITE_CMDS.contains(&s)).unwrap_or(false))
            })
            .unwrap_or(false)
        }
        _ => false,
    }
}

/// 统一审批判定：tier + risk + 是否写操作。
pub fn needs_approval(tool: &str, args: &Value, policy: &ToolPolicy) -> bool {
    let tier = policy.approval_for(tool);
    let risk = compute_risk(tool, args);
    match tier {
        ApprovalTier::Never => false,
        ApprovalTier::OnWrite => is_write_op(tool, args),
        ApprovalTier::OnRisk => risk == Risk::High,
        ApprovalTier::Always => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn guest_workspace_aliases_share_one_resource_identity() {
        let workspace = Path::new("/tmp/agnes-workspace");
        let resolver = GuestPathResolver::new(Some(workspace), None);
        let relative = resolver.resolve("src/lib.rs");
        let workspace_alias = resolver.resolve("/workspace/src/lib.rs");
        let project_alias = resolver.resolve("/root/workspace/src/lib.rs");

        assert_eq!(relative.host_path, workspace.join("src/lib.rs"));
        assert_eq!(relative.host_path, workspace_alias.host_path);
        assert_eq!(relative.host_path, project_alias.host_path);
        assert_eq!(relative.resource_id, "workspace");
        assert_eq!(workspace_alias.resource_id, "workspace");
        assert_eq!(project_alias.resource_id, "workspace");
    }

    #[test]
    fn explicit_cwd_does_not_change_workspace_alias_resolution() {
        let workspace = Path::new("/tmp/agnes-workspace");
        let resolver = GuestPathResolver::new(Some(workspace), None);
        let resolved = resolver.resolve_from_cwd(
            "/workspace/README.md",
            Some(&workspace.join("packages/app")),
        );

        assert_eq!(resolved.host_path, workspace.join("README.md"));
        assert_eq!(resolved.resource_id, "workspace");
    }

    #[test]
    fn custom_mounts_use_longest_guest_prefix_and_guest_home() {
        let resolver = GuestPathResolver::new(Some(Path::new("/private/workspace")), None)
            .with_mount(GuestMount::new(
                "environment-store",
                "/private/environments",
                "/opt/agnes/envs",
            ))
            .with_mount(GuestMount::new("guest-home", "/private/home", "/root"));

        let environment = resolver.resolve("/opt/agnes/envs/project/bin/python");
        assert_eq!(
            environment.host_path,
            Path::new("/private/environments/project/bin/python")
        );
        assert_eq!(environment.resource_id, "environment-store");

        let home = resolver.resolve("~/.cache/uv");
        assert_eq!(home.host_path, Path::new("/private/home/.cache/uv"));
        assert_eq!(home.guest_path, Path::new("/root/.cache/uv"));
        assert_eq!(home.resource_id, "guest-home");
    }

    #[test]
    fn risk_drives_on_risk_approval() {
        let policy = ToolPolicy::default();
        assert!(!needs_approval(
            "shell",
            &json!({"command": "cargo test"}),
            &policy
        ));
        assert!(needs_approval(
            "shell",
            &json!({"command": "rm -rf target"}),
            &policy
        ));
        assert!(needs_approval(
            "git",
            &json!({"args": ["push", "origin", "main"]}),
            &policy
        ));
        assert!(!needs_approval(
            "git",
            &json!({"args": ["status", "--short"]}),
            &policy
        ));
    }

    #[test]
    fn on_write_distinguishes_read_and_write_operations() {
        let mut policy = ToolPolicy::default();
        policy.shell.approval = ApprovalTier::OnWrite;
        assert!(!needs_approval(
            "shell",
            &json!({"command": "rg TODO src"}),
            &policy
        ));
        assert!(needs_approval(
            "shell",
            &json!({"command": "echo done > result.txt"}),
            &policy
        ));
        assert!(needs_approval(
            "file_write",
            &json!({"path": "result.txt", "content": "done"}),
            &policy
        ));
        assert!(!needs_approval(
            "file_read",
            &json!({"path": "result.txt"}),
            &policy
        ));
    }

    #[test]
    fn registry_contains_every_declared_builtin() {
        let names: Vec<_> = builtin_tools()
            .into_iter()
            .map(|tool| tool.name())
            .collect();
        assert_eq!(
            names,
            vec![
                "get_current_time",
                "shell",
                "write_stdin",
                "stop_terminal",
                "file_read",
                "file_write",
                "file_edit",
                "list_files",
                "grep",
                "apply_patch",
                "git",
                "memory_list",
                "memory_read",
                "memory_write",
                "memory_str_replace",
                "memory_append",
                "memory_delete",
                "web_search",
                "web_fetch",
                "browser_open",
                "calendar_list",
                "calendar_create",
                "calendar_event_create",
                "calendar_update",
                "task_list",
                "task_create",
                "task_complete",
                "task_update",
                "storage_accounts_list",
                "storage_files_list",
                "storage_files_search",
                "storage_file_download",
                "storage_file_upload",
                "storage_transfer_status",
                "storage_quota_get",
                "storage_files_move",
                "storage_files_trash"
            ]
        );
        assert!(builtin_tools()
            .into_iter()
            .all(|tool| tool.schema()["function"]["name"] == tool.name()));
        for tool in ["memory_write", "memory_str_replace", "memory_append"] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::Medium);
            assert!(is_write_op(tool, &json!({})));
        }
        assert_eq!(compute_risk("memory_delete", &json!({})), Risk::High);
        assert!(is_write_op("memory_delete", &json!({})));
        for tool in [
            "calendar_create",
            "calendar_event_create",
            "calendar_update",
            "task_create",
            "task_complete",
            "task_update",
        ] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::High);
            assert!(is_write_op(tool, &json!({})));
        }
        for tool in ["calendar_list", "task_list", "get_current_time"] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::Low);
            assert!(!is_write_op(tool, &json!({})));
        }
        for tool in ["web_search", "web_fetch", "browser_open"] {
            assert_eq!(compute_risk(tool, &json!({})), Risk::Low);
            assert!(!is_write_op(tool, &json!({})));
        }
    }
}
