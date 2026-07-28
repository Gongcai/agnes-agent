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
    ProcessFilesystem,
}

/// Determine whether one tool call needs a temporary filesystem exception.
pub fn sandbox_escalation_for_tool_call(
    tool: &str,
    args: &serde_json::Value,
    policy: &ToolPolicy,
    workspace_cwd: Option<&Path>,
) -> Option<SandboxEscalation> {
    let sandbox = PolicySandbox::new(policy, workspace_cwd);
    match tool {
        "file_write" | "file_edit" => {
            let target = resolve_tool_path(args.get("path")?.as_str()?, workspace_cwd, policy);
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
            );
            let command = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let outside_cwd = policy.check_shell(&cwd.to_string_lossy()).is_err()
                || sandbox.check_cwd(&cwd).is_err();
            let outside_write = policy.shell.deny_write_outside_workspace
                && crate::tools::builtin::shell::validate_shell_write_scope(
                    command, &cwd, &sandbox, 0,
                )
                .is_err();
            (explicitly_requested || outside_cwd || outside_write)
                .then_some(SandboxEscalation::ProcessFilesystem)
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
            );
            (explicitly_requested || sandbox.check_cwd(&cwd).is_err())
                .then_some(SandboxEscalation::ProcessFilesystem)
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
        SandboxEscalation::ProcessFilesystem => {
            policy.shell.allowed_cwd = vec!["/".to_string()];
            policy.shell.deny_write_outside_workspace = false;
            if !policy.sandbox.writable_roots.iter().any(|root| root == "/") {
                policy.sandbox.writable_roots.push("/".to_string());
            }
        }
    }
}

fn resolve_tool_path(path: &str, workspace_cwd: Option<&Path>, policy: &ToolPolicy) -> PathBuf {
    let expanded = crate::tools::policy::expand_home(path);
    let resolved = if expanded.is_absolute() {
        expanded
    } else {
        workspace_cwd
            .map(Path::to_path_buf)
            .or_else(|| {
                policy
                    .shell
                    .allowed_cwd
                    .first()
                    .map(|path| crate::tools::policy::expand_home(path))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join(expanded)
    };
    crate::tools::builtin::normalize_path(&resolved)
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
                    | "file_edit"
                    | "apply_patch"
                    | "memory_list"
                    | "memory_read"
                    | "memory_write"
                    | "memory_str_replace"
                    | "memory_append"
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
                    "接受编辑会自动处理文件和记忆读写，但 Shell、Git 和未知工具仍需要你的批准。",
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
        assert_eq!(escalation, Some(SandboxEscalation::ProcessFilesystem));

        apply_sandbox_escalation(&mut policy, escalation.as_ref().unwrap());
        let sandbox = PolicySandbox::new(&policy, Some(Path::new("/workspace")));
        assert!(sandbox.check_write(Path::new("/opt/agnes-output")).is_ok());
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
}
