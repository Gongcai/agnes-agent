use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::tools::policy::{BwrapMode, Risk, ToolPolicy};
use crate::tools::sandbox::{PolicySandbox, SandboxGuard};

/// Session-level autonomy mode applied on top of the agent's enabled tool capabilities.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    AskForApproval,
    #[default]
    Auto,
    AcceptEdits,
    FullAccess,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AskForApproval => "ask_for_approval",
            Self::Auto => "auto",
            Self::AcceptEdits => "accept_edits",
            Self::FullAccess => "full_access",
        }
    }

    /// Expand capabilities only for Full Access while preserving enabled flags and resource limits.
    pub fn effective_policy(self, base: &ToolPolicy) -> ToolPolicy {
        let mut policy = base.clone();
        if self == Self::FullAccess {
            policy.shell.allowed_cwd = vec!["/".to_string()];
            policy.shell.deny_write_outside_workspace = false;
            policy.file.allowed_roots = vec!["/".to_string()];
            policy.read_image.allowed_roots = vec!["/".to_string()];
            policy.network.allow = true;
            policy.sandbox.landlock = false;
            policy.sandbox.bwrap = BwrapMode::Disabled;
        }
        policy
    }
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "ask_for_approval" => Ok(Self::AskForApproval),
            "auto" => Ok(Self::Auto),
            "accept_edits" => Ok(Self::AcceptEdits),
            "full_access" => Ok(Self::FullAccess),
            _ => Err(format!("未知的会话权限模式: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApprovalDecision {
    pub needs_approval: bool,
    pub reason: &'static str,
    pub is_secondary_confirmation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxEscalation {
    FileWriteRoot(PathBuf),
    /// Extend a process only to the requested cwd and statically identified
    /// write roots. Normal on-request approvals stay within this scope.
    ProcessRoots {
        cwd: Option<PathBuf>,
        writable_roots: Vec<PathBuf>,
    },
    /// Explicit approval for an opaque command whose filesystem effects cannot
    /// be bounded before execution.
    ProcessFilesystem,
}

impl SandboxEscalation {
    /// Public, path-aware scope rendered in approval cards and audit events.
    pub fn scope_summary(&self) -> serde_json::Value {
        match self {
            Self::FileWriteRoot(root) => serde_json::json!({
                "kind": "file_write_root",
                "root": root,
            }),
            Self::ProcessRoots {
                cwd,
                writable_roots,
            } => serde_json::json!({
                "kind": "process_roots",
                "cwd": cwd,
                "writable_roots": writable_roots,
            }),
            Self::ProcessFilesystem => serde_json::json!({
                "kind": "process_filesystem",
            }),
        }
    }
}

/// Determine whether one tool call needs a temporary filesystem exception.
pub fn sandbox_escalation_for_tool_call(
    tool: &str,
    args: &serde_json::Value,
    policy: &ToolPolicy,
    workspace_cwd: Option<&Path>,
) -> Option<SandboxEscalation> {
    sandbox_escalation_for_tool_call_with_runtime(
        tool,
        args,
        policy,
        workspace_cwd,
        &crate::tools::sandbox::ToolProcessRuntime::Host,
    )
}

/// Determine a temporary filesystem exception using the same process runtime
/// and guest mount table as the physical tool execution.
pub fn sandbox_escalation_for_tool_call_with_runtime(
    tool: &str,
    args: &serde_json::Value,
    policy: &ToolPolicy,
    workspace_cwd: Option<&Path>,
    process_runtime: &crate::tools::sandbox::ToolProcessRuntime,
) -> Option<SandboxEscalation> {
    let sandbox =
        PolicySandbox::new_with_process_runtime(policy, workspace_cwd, process_runtime.clone());
    match tool {
        "file_write" | "write_file" | "file_edit" => {
            let target =
                resolve_tool_path(args.get("path")?.as_str()?, workspace_cwd, policy, &sandbox);
            sandbox.check_write(&target).err().map(|_| {
                SandboxEscalation::FileWriteRoot(target.parent().unwrap_or(&target).to_path_buf())
            })
        }
        "apply_patch" => {
            let base = resolve_tool_path(
                args.get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("."),
                workspace_cwd,
                policy,
                &sandbox,
            );
            sandbox
                .check_write(&base)
                .err()
                .map(|_| SandboxEscalation::FileWriteRoot(base))
        }
        "shell" => {
            let explicitly_requested = args
                .get("sandbox_permissions")
                .and_then(serde_json::Value::as_str)
                == Some("require_escalated");
            let cwd = resolve_tool_path(
                args.get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("."),
                workspace_cwd,
                policy,
                &sandbox,
            );
            let command = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let outside_cwd = (!sandbox.uses_guest_paths()
                && policy.check_shell(&cwd.to_string_lossy()).is_err())
                || sandbox.check_cwd(&cwd).is_err();
            let mut writable_roots = Vec::new();
            let write_targets = crate::tools::builtin::shell::shell_write_targets_with_sandbox(
                command,
                &cwd,
                workspace_cwd,
                0,
                Some(&sandbox),
            );
            if policy.shell.deny_write_outside_workspace {
                match &write_targets {
                    Ok(targets) => {
                        for target in targets {
                            if sandbox.check_write(&target).is_ok() {
                                continue;
                            }
                            let Some(root) = existing_write_root(&target) else {
                                // A top-level path would require `/`; do not
                                // infer a full-filesystem grant from it.
                                return explicitly_requested
                                    .then_some(SandboxEscalation::ProcessFilesystem);
                            };
                            writable_roots.push(root);
                        }
                    }
                    Err(_) if explicitly_requested => {
                        return Some(SandboxEscalation::ProcessFilesystem);
                    }
                    Err(_) => {
                        // Opaque writes are denied by the executor's static
                        // inspection unless the model explicitly requests a
                        // user-approved exception.
                        return None;
                    }
                }
            }
            writable_roots.sort();
            writable_roots.dedup();
            if explicitly_requested && writable_roots.is_empty() && !outside_cwd {
                // An explicit request is the escape hatch for commands whose
                // side effects cannot be bounded statically. It is always
                // covered by the normal user approval gate.
                return Some(SandboxEscalation::ProcessFilesystem);
            }
            (outside_cwd || !writable_roots.is_empty()).then_some(SandboxEscalation::ProcessRoots {
                cwd: outside_cwd.then_some(cwd),
                writable_roots,
            })
        }
        "git" => {
            let explicitly_requested = args
                .get("sandbox_permissions")
                .and_then(serde_json::Value::as_str)
                == Some("require_escalated");
            let cwd = resolve_tool_path(
                args.get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("."),
                workspace_cwd,
                policy,
                &sandbox,
            );
            // Git updates repository metadata, so an external cwd must be
            // writable even when it is already readable by the process.
            let outside_cwd = sandbox.check_write(&cwd).is_err();
            if outside_cwd {
                let writable_roots = existing_git_root(&cwd)
                    .or_else(|| existing_write_root(&cwd))
                    .into_iter()
                    .collect();
                return Some(SandboxEscalation::ProcessRoots {
                    cwd: Some(cwd),
                    writable_roots,
                });
            }
            explicitly_requested.then_some(SandboxEscalation::ProcessFilesystem)
        }
        _ => None,
    }
}

/// Apply an approved exception to the policy copy used by one physical call.
pub fn apply_sandbox_escalation(policy: &mut ToolPolicy, escalation: &SandboxEscalation) {
    match escalation {
        SandboxEscalation::FileWriteRoot(root) => {
            let root = root.to_string_lossy().to_string();
            if !policy.sandbox.writable_roots.contains(&root) {
                policy.sandbox.writable_roots.push(root);
            }
        }
        SandboxEscalation::ProcessRoots {
            cwd,
            writable_roots,
        } => {
            if let Some(cwd) = cwd {
                let cwd = cwd.to_string_lossy().to_string();
                if !policy.shell.allowed_cwd.contains(&cwd) {
                    policy.shell.allowed_cwd.push(cwd);
                }
            }
            for root in writable_roots {
                let root = root.to_string_lossy().to_string();
                if !policy.sandbox.writable_roots.contains(&root) {
                    policy.sandbox.writable_roots.push(root);
                }
            }
        }
        SandboxEscalation::ProcessFilesystem => {
            policy.shell.allowed_cwd = vec!["/".to_string()];
            policy.shell.deny_write_outside_workspace = false;
            if !policy.sandbox.writable_roots.iter().any(|root| root == "/") {
                policy.sandbox.writable_roots.push("/".to_string());
            }
        }
    }
}

/// Return the smallest existing parent directory that can contain a target.
/// Granting `/` remains reserved for an explicit opaque-command approval.
fn existing_write_root(target: &Path) -> Option<PathBuf> {
    let normalized = crate::tools::builtin::normalize_path(target);
    let mut candidate = if normalized.is_dir() {
        normalized
    } else if normalized.exists() {
        normalized.parent()?.to_path_buf()
    } else {
        normalized
    };
    while !candidate.exists() {
        candidate = candidate.parent()?.to_path_buf();
    }
    let candidate = crate::tools::builtin::normalize_path(&candidate);
    (candidate != Path::new("/")).then_some(candidate)
}

/// Find the repository root for an external Git cwd so repository metadata is
/// writable without granting the whole filesystem. A non-repository cwd falls
/// back to the cwd's existing parent in the caller.
fn existing_git_root(cwd: &Path) -> Option<PathBuf> {
    let normalized = crate::tools::builtin::normalize_path(cwd);
    let mut candidate = if normalized.is_dir() {
        normalized
    } else {
        normalized.parent()?.to_path_buf()
    };
    loop {
        let git_dir = candidate.join(".git");
        if git_dir.is_dir() || git_dir.is_file() {
            return Some(candidate);
        }
        let parent = candidate.parent()?.to_path_buf();
        if parent == candidate {
            return None;
        }
        candidate = parent;
    }
}

fn resolve_tool_path(
    path: &str,
    workspace_cwd: Option<&Path>,
    policy: &ToolPolicy,
    sandbox: &PolicySandbox,
) -> PathBuf {
    let fallback_root = policy.shell.allowed_cwd.first().map(Path::new);
    sandbox
        .resolve_guest_path(path, workspace_cwd)
        .map(|(host_path, _)| host_path)
        .unwrap_or_else(|_| {
            let resolved = crate::tools::builtin::resolve_path_from_workspace(
                path,
                workspace_cwd,
                fallback_root,
            );
            crate::tools::builtin::normalize_path(&resolved.host_path)
        })
}

/// Resolve human approval for the active session mode.
pub fn approval_decision(mode: PermissionMode, tool: &str, risk: Risk) -> ApprovalDecision {
    if tool == "memory_delete" {
        return ApprovalDecision {
            needs_approval: true,
            reason: "删除整份记忆文件需要用户明确确认。",
            is_secondary_confirmation: true,
        };
    }
    match mode {
        PermissionMode::AskForApproval => ApprovalDecision {
            needs_approval: true,
            reason: "当前会话设置为每次询问，所有本地工具调用都需要你的批准。",
            is_secondary_confirmation: false,
        },
        PermissionMode::Auto if risk == Risk::High => ApprovalDecision {
            needs_approval: true,
            reason: "当前模型请求执行高风险操作，Auto 模式仍要求你进行二次确认。",
            is_secondary_confirmation: true,
        },
        PermissionMode::Auto => ApprovalDecision {
            needs_approval: false,
            reason: "Auto 模式信任当前模型对普通工具调用的判断并自动执行。",
            is_secondary_confirmation: false,
        },
        PermissionMode::AcceptEdits => {
            let accepts_without_prompt = matches!(
                tool,
                "file_read"
                    | "list_files"
                    | "grep"
                    | "file_write"
                    | "write_file"
                    | "file_edit"
                    | "apply_patch"
                    | "memory_list"
                    | "memory_read"
                    | "memory_write"
                    | "memory_str_replace"
                    | "memory_append"
                    | "conversation_search"
                    | "conversation_read"
                    | "web_search"
                    | "web_fetch"
                    | "browser_open"
                    | "calendar_list"
                    | "task_list"
                    | "get_current_time"
            );
            ApprovalDecision {
                needs_approval: !accepts_without_prompt,
                reason:
                    "接受编辑会自动处理文件、记忆和历史对话读取，但 Shell、Git 和未知工具仍需要你的批准。",
                is_secondary_confirmation: false,
            }
        }
        PermissionMode::FullAccess => ApprovalDecision {
            needs_approval: false,
            reason: "完全访问会在已启用的工具范围内直接执行，不再请求人工审批。",
            is_secondary_confirmation: false,
        },
    }
}

pub fn audit_snapshot(mode: PermissionMode, policy: &ToolPolicy) -> String {
    serde_json::json!({
        "permission_mode": mode,
        "tool_policy": policy,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_modes_parse_and_round_trip() {
        for value in ["ask_for_approval", "auto", "accept_edits", "full_access"] {
            let mode = PermissionMode::from_str(value).unwrap();
            assert_eq!(mode.as_str(), value);
        }
        assert!(PermissionMode::from_str("unrestricted").is_err());
    }

    #[test]
    fn auto_executes_normal_calls_and_marks_high_risk() {
        let medium = approval_decision(PermissionMode::Auto, "shell", Risk::Medium);
        assert!(!medium.needs_approval);
        assert!(!medium.is_secondary_confirmation);

        let high = approval_decision(PermissionMode::Auto, "shell", Risk::High);
        assert!(high.needs_approval);
        assert!(high.is_secondary_confirmation);
    }

    #[test]
    fn explicit_approval_and_full_access_are_opposites() {
        assert!(
            approval_decision(PermissionMode::AskForApproval, "file_read", Risk::Low,)
                .needs_approval
        );
        assert!(!approval_decision(PermissionMode::FullAccess, "shell", Risk::High).needs_approval);
    }

    #[test]
    fn accept_edits_only_prompts_for_command_tools() {
        for tool in [
            "file_read",
            "list_files",
            "grep",
            "file_write",
            "file_edit",
            "apply_patch",
            "memory_list",
            "memory_read",
            "memory_write",
            "memory_str_replace",
            "memory_append",
            "conversation_search",
            "conversation_read",
            "web_search",
            "web_fetch",
            "browser_open",
            "get_current_time",
        ] {
            assert!(
                !approval_decision(PermissionMode::AcceptEdits, tool, Risk::High).needs_approval
            );
        }
        assert!(
            approval_decision(PermissionMode::AcceptEdits, "shell", Risk::Medium).needs_approval
        );
        assert!(approval_decision(PermissionMode::AcceptEdits, "git", Risk::Low).needs_approval);
    }

    #[test]
    fn planner_writes_require_approval_outside_full_access() {
        for tool in [
            "calendar_create",
            "calendar_event_create",
            "calendar_update",
            "task_create",
            "task_complete",
            "task_update",
        ] {
            assert!(approval_decision(PermissionMode::Auto, tool, Risk::High).needs_approval);
            assert!(
                approval_decision(PermissionMode::AcceptEdits, tool, Risk::High).needs_approval
            );
            assert!(
                !approval_decision(PermissionMode::FullAccess, tool, Risk::High).needs_approval
            );
        }
    }

    #[test]
    fn full_access_preserves_enabled_flags_and_resource_limits() {
        let mut base = ToolPolicy::default();
        base.shell.enabled = false;
        base.sandbox.memory_bytes = 1234;

        let effective = PermissionMode::FullAccess.effective_policy(&base);
        assert!(!effective.shell.enabled);
        assert_eq!(effective.shell.allowed_cwd, vec!["/"]);
        assert_eq!(effective.file.allowed_roots, vec!["/"]);
        assert!(!effective.shell.deny_write_outside_workspace);
        assert!(effective.network.allow);
        assert!(!effective.sandbox.landlock);
        assert_eq!(effective.sandbox.bwrap, BwrapMode::Disabled);
        assert_eq!(effective.sandbox.memory_bytes, 1234);
    }

    #[cfg(unix)]
    #[test]
    fn full_access_allows_native_writes_outside_the_workspace() {
        use std::path::Path;

        use crate::tools::sandbox::{PolicySandbox, SandboxGuard};

        let effective = PermissionMode::FullAccess.effective_policy(&ToolPolicy::default());
        let sandbox = PolicySandbox::new(&effective, Some(Path::new("/tmp/workspace")));
        assert!(sandbox
            .check_write(Path::new("/var/tmp/agnes-full-access-test"))
            .is_ok());
        assert!(sandbox
            .check_write(Path::new("/tmp/workspace/.git/config"))
            .is_ok());
    }

    #[test]
    fn outside_file_write_gets_a_scoped_approval_root() {
        let workspace = Path::new("/workspace");
        let policy = ToolPolicy::default();
        let escalation = sandbox_escalation_for_tool_call(
            "file_write",
            &serde_json::json!({"path": "/opt/agnes/output.txt", "content": "ok"}),
            &policy,
            Some(workspace),
        )
        .unwrap();
        assert_eq!(
            escalation,
            SandboxEscalation::FileWriteRoot(PathBuf::from("/opt/agnes"))
        );

        let mut approved = policy;
        apply_sandbox_escalation(&mut approved, &escalation);
        let sandbox = PolicySandbox::new(&approved, Some(workspace));
        assert!(sandbox
            .check_write(Path::new("/opt/agnes/output.txt"))
            .is_ok());
        assert!(sandbox.check_write(Path::new("/opt/other.txt")).is_err());
    }

    #[test]
    fn workspace_guest_alias_does_not_request_an_external_write_scope() {
        let workspace = Path::new("/workspace");
        let policy = ToolPolicy::default();
        let escalation = sandbox_escalation_for_tool_call(
            "file_write",
            &serde_json::json!({"path": "/root/workspace/result.txt", "content": "ok"}),
            &policy,
            Some(workspace),
        );

        assert_eq!(escalation, None);
    }

    #[test]
    fn android_runtime_tmp_does_not_request_a_host_filesystem_escalation() {
        use crate::tools::sandbox::{
            AndroidProotProcessRuntime, AndroidProotRuntimeConfig, ToolProcessRuntime,
        };

        let temp = tempfile::tempdir().unwrap();
        let rootfs = temp.path().join("rootfs");
        let workspace = temp.path().join("workspace");
        let project_root = temp.path().join("user/workspace");
        let home = temp.path().join("user/home");
        let proot_tmp = temp.path().join("proot-tmp");
        for path in [
            rootfs.join("etc"),
            workspace.clone(),
            project_root.clone(),
            home.clone(),
            proot_tmp.join("shm"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let runtime = ToolProcessRuntime::AndroidProot(AndroidProotProcessRuntime::new(
            AndroidProotRuntimeConfig::new(
                temp.path().join("proot"),
                temp.path().join("loader"),
                proot_tmp.clone(),
                rootfs,
                project_root,
                false,
            )
            .with_user_home(home),
        ));
        let policy = ToolPolicy::default();

        let file = sandbox_escalation_for_tool_call_with_runtime(
            "file_write",
            &serde_json::json!({"path": "/tmp/output.txt", "content": "ok"}),
            &policy,
            Some(&workspace),
            &runtime,
        );
        let shell = sandbox_escalation_for_tool_call_with_runtime(
            "shell",
            &serde_json::json!({"cwd": "/tmp", "command": "touch output.txt"}),
            &policy,
            Some(&workspace),
            &runtime,
        );

        assert_eq!(file, None);
        assert_eq!(shell, None);
        assert!(proot_tmp.is_dir());
    }

    #[test]
    fn shell_outside_write_requires_per_call_process_escalation() {
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec!["/workspace".into()];
        policy.file.allowed_roots = vec!["/workspace".into()];
        let args = serde_json::json!({"command": "touch /opt/agnes-output"});
        let escalation = sandbox_escalation_for_tool_call(
            "shell",
            &args,
            &policy,
            Some(Path::new("/workspace")),
        );
        assert_eq!(
            escalation,
            Some(SandboxEscalation::ProcessRoots {
                cwd: None,
                writable_roots: vec![PathBuf::from("/opt")],
            })
        );

        apply_sandbox_escalation(&mut policy, escalation.as_ref().unwrap());
        let sandbox = PolicySandbox::new(&policy, Some(Path::new("/workspace")));
        assert!(sandbox.check_write(Path::new("/opt/agnes-output")).is_ok());
        assert!(sandbox.check_write(Path::new("/etc/agnes-output")).is_err());
        assert!(policy.shell.deny_write_outside_workspace);
    }

    #[test]
    fn shell_outside_cwd_is_readable_but_not_writable_without_escalation() {
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec!["/workspace".into()];
        let args = serde_json::json!({
            "command": "printf ready",
            "cwd": "/opt"
        });
        let escalation = sandbox_escalation_for_tool_call(
            "shell",
            &args,
            &policy,
            Some(Path::new("/workspace")),
        );
        assert_eq!(escalation, None);
        let sandbox = PolicySandbox::new(&policy, Some(Path::new("/workspace")));
        assert!(sandbox.check_cwd(Path::new("/opt")).is_ok());
        assert!(sandbox.check_write(Path::new("/opt/agnes-output")).is_err());
    }

    #[test]
    fn write_file_alias_uses_file_write_policy_and_scope() {
        let policy = ToolPolicy::default();
        assert_eq!(
            policy.approval_for("write_file"),
            policy.approval_for("file_write")
        );
        assert_eq!(
            sandbox_escalation_for_tool_call(
                "write_file",
                &serde_json::json!({"path": "/opt/agnes/output.txt", "content": "ok"}),
                &policy,
                Some(Path::new("/workspace")),
            ),
            sandbox_escalation_for_tool_call(
                "file_write",
                &serde_json::json!({"path": "/opt/agnes/output.txt", "content": "ok"}),
                &policy,
                Some(Path::new("/workspace")),
            )
        );
    }

    #[test]
    fn shell_top_level_write_is_not_automatically_granted() {
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec!["/workspace".into()];
        let args = serde_json::json!({"command": "touch /agnes-output"});
        assert_eq!(
            sandbox_escalation_for_tool_call(
                "shell",
                &args,
                &policy,
                Some(Path::new("/workspace"))
            ),
            None
        );
    }

    #[test]
    fn shell_can_explicitly_request_escalation_for_opaque_tools() {
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec!["/workspace".into()];
        let args = serde_json::json!({
            "command": "python build.py",
            "sandbox_permissions": "require_escalated"
        });
        assert_eq!(
            sandbox_escalation_for_tool_call(
                "shell",
                &args,
                &policy,
                Some(Path::new("/workspace"))
            ),
            Some(SandboxEscalation::ProcessFilesystem)
        );
    }

    #[cfg(unix)]
    #[test]
    fn git_outside_cwd_is_scoped_to_the_repository() {
        let external_root = PathBuf::from(format!(
            "/dev/shm/agnes-git-sandbox-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&external_root).unwrap();
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec!["/workspace".into()];
        let external_cwd = external_root.to_string_lossy().to_string();
        let args = serde_json::json!({
            "args": ["status"],
            "cwd": external_cwd
        });
        let escalation =
            sandbox_escalation_for_tool_call("git", &args, &policy, Some(Path::new("/workspace")));
        assert_eq!(
            escalation,
            Some(SandboxEscalation::ProcessRoots {
                cwd: Some(external_root.clone()),
                writable_roots: vec![external_root.clone()],
            })
        );
        apply_sandbox_escalation(&mut policy, escalation.as_ref().unwrap());
        let sandbox = PolicySandbox::new(&policy, Some(Path::new("/workspace")));
        assert!(sandbox
            .check_write(&external_root.join(".git/index"))
            .is_ok());
        assert!(sandbox
            .check_write(&external_root.join("../other/.git/index"))
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn git_nested_external_cwd_includes_repository_metadata_root() {
        let external_root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("agnes-git-nested-sandbox-{}", uuid::Uuid::new_v4()));
        let nested = external_root.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(&external_root.join(".git")).unwrap();
        let mut policy = ToolPolicy::default();
        policy.shell.allowed_cwd = vec!["/workspace".into()];
        let args = serde_json::json!({
            "args": ["status"],
            "cwd": nested.to_string_lossy()
        });
        let escalation =
            sandbox_escalation_for_tool_call("git", &args, &policy, Some(Path::new("/workspace")));
        assert_eq!(
            escalation,
            Some(SandboxEscalation::ProcessRoots {
                cwd: Some(nested.clone()),
                writable_roots: vec![external_root.clone()],
            })
        );
        std::fs::remove_dir_all(external_root).unwrap();
    }
}
