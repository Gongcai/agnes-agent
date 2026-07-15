use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::tools::policy::{BwrapMode, Risk, ToolPolicy};

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

/// Resolve human approval for the current placeholder implementation of Auto mode.
pub fn approval_decision(mode: PermissionMode, tool: &str, risk: Risk) -> ApprovalDecision {
    match mode {
        PermissionMode::AskForApproval => ApprovalDecision {
            needs_approval: true,
            reason: "当前会话设置为每次询问，所有本地工具调用都需要你的批准。",
            is_secondary_confirmation: false,
        },
        PermissionMode::Auto if risk == Risk::High => ApprovalDecision {
            needs_approval: true,
            reason: "Auto 决策模型尚未接入；此调用属于高风险操作，必须由你二次确认。",
            is_secondary_confirmation: true,
        },
        PermissionMode::Auto => ApprovalDecision {
            needs_approval: true,
            reason: "Auto 决策模型尚未接入，本次调用暂由你决定是否执行。",
            is_secondary_confirmation: false,
        },
        PermissionMode::AcceptEdits => {
            let accepts_without_prompt = matches!(
                tool,
                "file_read" | "list_files" | "grep" | "file_write" | "file_edit" | "apply_patch"
            );
            ApprovalDecision {
                needs_approval: !accepts_without_prompt,
                reason: "接受编辑会自动处理文件读写，但 Shell、Git 和未知工具仍需要你的批准。",
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
    fn auto_is_a_user_decision_placeholder_and_marks_high_risk() {
        let medium = approval_decision(PermissionMode::Auto, "shell", Risk::Medium);
        assert!(medium.needs_approval);
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
    }
}
