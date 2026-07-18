//! ToolExecutor: 工具调用编排器。
///
/// 职责：插入审计初志 → 解析 workspace cwd → 合并有效 policy → 派发到 BuiltinTool。
/// 各工具的物理执行逻辑在 `builtin/*` 模块中。
use std::path::PathBuf;

use crate::db::repo::tools::NewToolCall;
use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};
use crate::tools::builtin::{builtin_tools, ToolCtx};
use crate::tools::policy::ToolPolicy;
use crate::tools::PermissionMode;

pub struct ToolExecutor {
    db: DbActorHandle,
}

impl ToolExecutor {
    pub fn new(db: DbActorHandle) -> Self {
        Self { db }
    }

    /// 执行工具调用：审计入志 → workspace cwd → 有效 policy → 派发。
    #[cfg(test)]
    pub async fn execute(
        &self,
        session_id: &str,
        message_id: Option<&str>,
        tool_call_id: &str,
        tool: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
    ) -> AppResult<serde_json::Value> {
        let permission_mode = self
            .db
            .get_session(session_id.to_string())
            .await?
            .and_then(|session| session.permission_mode.parse().ok())
            .unwrap_or_default();
        self.execute_with_permission_mode(
            session_id,
            message_id,
            tool_call_id,
            tool,
            arguments,
            policy,
            permission_mode,
        )
        .await
    }

    /// Execute a tool call with the permission mode captured by the approval gate.
    pub async fn execute_with_permission_mode(
        &self,
        session_id: &str,
        message_id: Option<&str>,
        tool_call_id: &str,
        tool: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
        permission_mode: PermissionMode,
    ) -> AppResult<serde_json::Value> {
        let public_arguments = crate::tools::builtin::memory_entry::public_arguments(arguments);
        // A. 审计初志
        let new_tc = NewToolCall {
            id: tool_call_id.to_string(),
            session_id: session_id.to_string(),
            message_id: message_id.map(|x| x.to_string()),
            tool: tool.to_string(),
            params: Some(public_arguments.to_string()),
            status: "running".to_string(),
            risk_level: Some(
                crate::tools::builtin::compute_risk(tool, arguments)
                    .as_str()
                    .to_string(),
            ),
            approval_policy_snapshot: Some(crate::tools::permissions::audit_snapshot(
                permission_mode,
                policy,
            )),
        };
        self.db.insert_tool_call(new_tc).await?;

        // B. Resolve the workspace cwd through the device-local binding.
        let workspace_cwd =
            crate::tools::workspace::resolve_workspace_cwd(&self.db, session_id).await;

        // C. 合并有效 policy：workspace 自动加入 allowed_cwd / allowed_roots
        let effective_policy = effective_policy(policy, &workspace_cwd);
        let sandbox =
            crate::tools::sandbox::PolicySandbox::new(&effective_policy, workspace_cwd.as_deref());

        // D. 派发到内置工具
        let ctx = ToolCtx {
            db: &self.db,
            session_id,
            tool_call_id,
            args: arguments,
            policy: &effective_policy,
            workspace_cwd,
            sandbox: &sandbox,
        };

        for impl_ in builtin_tools() {
            if impl_.name() == tool {
                return impl_.execute(&ctx).await;
            }
        }

        let err_msg = format!("未知的内置工具: {tool}");
        ctx.record_failure(&err_msg).await?;
        Err(AppError::Other(err_msg))
    }
}

/// 把 workspace 目录合并进 policy 的 allowed_cwd / allowed_roots，使 workspace 成为合法工作环境。
fn effective_policy(policy: &ToolPolicy, workspace_cwd: &Option<PathBuf>) -> ToolPolicy {
    let mut p = policy.clone();
    if let Some(ws) = workspace_cwd {
        let ws_str = ws.to_string_lossy().to_string();
        if !p.shell.allowed_cwd.contains(&ws_str) {
            p.shell.allowed_cwd.push(ws_str.clone());
        }
        if !p.file.allowed_roots.contains(&ws_str) {
            p.file.allowed_roots.push(ws_str);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::spawn_db_actor;
    use crate::tools::policy::{ApprovalTier, BwrapMode, FilePolicy, GitPolicy, ShellPolicy};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn new_agent(id: &str, name: &str) -> crate::db::repo::agents::NewAgent {
        crate::db::repo::agents::NewAgent {
            id: id.into(),
            name: name.into(),
            persona: "You are a test agent".into(),
            scenario: "".into(),
            system_prompt: "Be a test agent".into(),
            greeting: "".into(),
            example_dialogue: "".into(),
            model: "GPT-4".into(),
            tool_policy: "{}".into(),
            avatar: "".into(),
            tags: "".into(),
            thinking_mode: "off".into(),
            thinking_budget: 0,
        }
    }

    #[tokio::test]
    async fn test_tool_executor_shell_and_file() {
        let db_path = PathBuf::from("target/test_tools.db");
        if db_path.exists() {
            let _ = fs::remove_file(&db_path);
        }
        let db = spawn_db_actor(db_path.clone());
        let executor = ToolExecutor::new(db.clone());

        db.insert_agent(new_agent("test-agent", "Test Agent"))
            .await
            .unwrap();
        db.insert_agent(new_agent("other-agent", "Other Agent"))
            .await
            .unwrap();

        let temp_project = std::env::current_dir()
            .unwrap()
            .join("target/test_tool_project");
        let _ = fs::remove_dir_all(&temp_project);
        let _ = fs::create_dir_all(&temp_project);
        db.insert_workspace(crate::db::repo::workspaces::NewWorkspace {
            id: "workspace-1".into(),
            agent_id: "test-agent".into(),
            name: "Tool Test Workspace".into(),
            folder_path: temp_project.to_string_lossy().to_string(),
        })
        .await
        .unwrap();

        let session = crate::db::repo::sessions::NewSession {
            id: "sess-1".into(),
            agent_id: "test-agent".into(),
            title: "Test Title".into(),
            context_limit: None,
            compress_threshold: Some(0.8),
            recency_window: Some(15),
            reserved_output_tokens: None,
            summarizer_model: None,
            model: None,
            thinking_mode: None,
            thinking_budget: None,
            permission_mode: "auto".into(),
            workspace_id: Some("workspace-1".into()),
            origin_device_id: None,
        };
        db.insert_session(session).await.unwrap();

        let policy = ToolPolicy {
            shell: ShellPolicy {
                enabled: true,
                approval: ApprovalTier::Never,
                allowed_cwd: vec![temp_project.to_string_lossy().to_string()],
                deny_write_outside_workspace: true,
                timeout_sec: 5,
                max_output_bytes: 1000,
                env_allowlist: vec!["PATH".to_string()],
            },
            file: FilePolicy {
                enabled: true,
                approval: ApprovalTier::Never,
                allowed_roots: vec![temp_project.parent().unwrap().to_string_lossy().to_string()],
            },
            git: GitPolicy {
                enabled: true,
                approval: ApprovalTier::Never,
                timeout_sec: 5,
            },
            memory: Default::default(),
            planner: Default::default(),
            sandbox: Default::default(),
            network: Default::default(),
        };

        let test_file = temp_project.join("test_write.txt");
        let write_args = json!({ "path": "test_write.txt", "content": "Hello ToolExecutor!" });
        let write_res = executor
            .execute(
                "sess-1",
                None,
                "tc-write-1",
                "file_write",
                &write_args,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(write_res.get("success").unwrap().as_bool().unwrap(), true);
        assert_eq!(
            fs::read_to_string(&test_file).unwrap(),
            "Hello ToolExecutor!"
        );

        let read_args = json!({ "path": "test_write.txt" });
        let read_res = executor
            .execute(
                "sess-1",
                None,
                "tc-read-1",
                "file_read",
                &read_args,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(
            read_res.get("content").unwrap().as_str().unwrap(),
            "Hello ToolExecutor!"
        );

        let edit_args = json!({
            "path": "test_write.txt",
            "old_string": "Hello ToolExecutor!",
            "new_string": "Hello Edited!"
        });
        executor
            .execute(
                "sess-1",
                None,
                "tc-edit-1",
                "file_edit",
                &edit_args,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello Edited!");

        let list_args = json!({"pattern": "*.txt"});
        let list_res = executor
            .execute(
                "sess-1",
                None,
                "tc-list-1",
                "list_files",
                &list_args,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(list_res["files"][0]["path"], "test_write.txt");

        let grep_args = json!({"pattern": "Edited", "glob": "*.txt"});
        let grep_res = executor
            .execute("sess-1", None, "tc-grep-1", "grep", &grep_args, &policy)
            .await
            .unwrap();
        assert_eq!(grep_res["matches"][0]["line"], 1);

        let patch_args = json!({
            "patch": "*** Begin Patch\n*** Update File: test_write.txt\n@@\n-Hello Edited!\n\\ No newline at end of file\n+Hello Patched!\n*** Add File: added.txt\n+Added by patch\n*** End Patch\n"
        });
        let patch_res = executor
            .execute(
                "sess-1",
                None,
                "tc-patch-1",
                "apply_patch",
                &patch_args,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(patch_res["changed_files"].as_array().unwrap().len(), 2);
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello Patched!\n");
        assert_eq!(
            fs::read_to_string(temp_project.join("added.txt")).unwrap(),
            "Added by patch\n"
        );

        let outside_file = temp_project.parent().unwrap().join("sandbox-denied.txt");
        let outside_write = json!({
            "path": outside_file.to_string_lossy(),
            "content": "must not be written"
        });
        let outside_result = executor
            .execute(
                "sess-1",
                None,
                "tc-write-outside",
                "file_write",
                &outside_write,
                &policy,
            )
            .await;
        assert!(outside_result.is_err());
        assert!(!outside_file.exists());

        let bad_read_args = json!({ "path": "/etc/passwd" });
        let read_err = executor
            .execute(
                "sess-1",
                None,
                "tc-read-bad",
                "file_read",
                &bad_read_args,
                &policy,
            )
            .await;
        assert!(read_err.is_err());

        let shell_args = json!({ "command": "echo 'Hello Shell'" });
        let shell_res = executor
            .execute("sess-1", None, "tc-shell-1", "shell", &shell_args, &policy)
            .await
            .unwrap();
        assert_eq!(shell_res.get("exit_code").unwrap().as_i64().unwrap(), 0);
        assert!(shell_res
            .get("stdout")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("Hello Shell"));

        let bad_shell_args = json!({ "command": "echo 'Bad'", "cwd": "/" });
        let shell_err = executor
            .execute(
                "sess-1",
                None,
                "tc-shell-bad",
                "shell",
                &bad_shell_args,
                &policy,
            )
            .await;
        assert!(shell_err.is_err());

        let allowed_shell_write = json!({"command": "echo allowed > shell-write.txt"});
        executor
            .execute(
                "sess-1",
                None,
                "tc-shell-write",
                "shell",
                &allowed_shell_write,
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(temp_project.join("shell-write.txt")).unwrap(),
            "allowed\n"
        );

        let denied_shell_write = json!({"command": "echo denied > ../shell-escape.txt"});
        let denied_shell_result = executor
            .execute(
                "sess-1",
                None,
                "tc-shell-escape",
                "shell",
                &denied_shell_write,
                &policy,
            )
            .await;
        assert!(denied_shell_result.is_err());
        assert!(!temp_project
            .parent()
            .unwrap()
            .join("shell-escape.txt")
            .exists());

        let git_init = json!({"args": ["init"]});
        let git_init_result = executor
            .execute("sess-1", None, "tc-git-init", "git", &git_init, &policy)
            .await
            .unwrap();
        assert_eq!(git_init_result["exit_code"], 0);
        let git_status = json!({"args": ["status", "--short"]});
        let git_status_result = executor
            .execute("sess-1", None, "tc-git-status", "git", &git_status, &policy)
            .await
            .unwrap();
        assert_eq!(git_status_result["exit_code"], 0);
        let git_global_config = json!({"args": ["config", "--global", "user.name", "Denied"]});
        assert!(executor
            .execute(
                "sess-1",
                None,
                "tc-git-global-config",
                "git",
                &git_global_config,
                &policy,
            )
            .await
            .is_err());

        let mut offline_policy = policy.clone();
        offline_policy.network.allow = false;
        offline_policy.sandbox.bwrap = BwrapMode::Disabled;
        let network_command = json!({"command": "curl https://example.com"});
        assert!(executor
            .execute(
                "sess-1",
                None,
                "tc-shell-network-denied",
                "shell",
                &network_command,
                &offline_policy,
            )
            .await
            .is_err());

        db.insert_memory(crate::db::repo::memory::NewMemory {
            id: "memory-1".into(),
            agent_id: "test-agent".into(),
            name: "Package manager".into(),
            keywords: vec!["pnpm".into(), "frontend".into()],
            content: "Use pnpm for frontend dependencies.".into(),
            creator: "user".into(),
            memory_type: "Preference".into(),
            scope: "agent".into(),
            source: "test".into(),
            confidence: 1.0,
            embedding_id: None,
        })
        .await
        .unwrap();
        let memory_result = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-search-1",
                "memory_search",
                &json!({"query": "pnpm"}),
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(memory_result["memories"][0]["name"], "Package manager");
        assert_eq!(
            memory_result["memories"][0]["content"],
            "Use pnpm for frontend dependencies."
        );
        assert_eq!(memory_result["memories"][0]["id"], "memory-1");
        assert!(memory_result["memories"][0].get("agent_id").is_none());

        let spoofed_create = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-create-spoofed",
                "memory_create",
                &json!({
                    "name": "Spoofed",
                    "content": "This must not be created.",
                    "creator": "user"
                }),
                &policy,
            )
            .await;
        assert!(spoofed_create.is_err());

        let create_result = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-create-1",
                "memory_create",
                &json!({
                    "name": "Rust test command",
                    "keywords": ["cargo", " rust ", "cargo"],
                    "content": "Use cargo test for the Rust core.",
                    "__agnes_embedding": {
                        "model": "test/embed-3",
                        "vector": [1.0, 0.0, 0.0]
                    }
                }),
                &policy,
            )
            .await
            .unwrap();
        let created = &create_result["memory"];
        let created_id = created["id"].as_str().unwrap().to_string();
        let created_at = created["created_at"].as_str().unwrap().to_string();
        assert_eq!(created["creator"], "ai");
        assert_eq!(created["keywords"], json!(["cargo", "rust"]));
        assert!(created.get("agent_id").is_none());
        assert_eq!(
            db.get_memory(created_id.clone(), "test-agent".into())
                .await
                .unwrap()
                .unwrap()
                .embedding_model
                .as_deref(),
            Some("test/embed-3")
        );
        assert!(!db
            .get_tool_call("tc-memory-create-1".into())
            .await
            .unwrap()
            .unwrap()
            .params
            .unwrap()
            .contains("__agnes_embedding"));

        let created_search = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-search-created",
                "memory_search",
                &json!({"query": "cargo test"}),
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(created_search["memories"][0]["id"], created_id);

        let update_result = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-update-1",
                "memory_update",
                &json!({
                    "memory_id": created_id,
                    "keywords": ["tests"],
                    "content": "Run cargo test --no-fail-fast for the Rust core.",
                    "__agnes_embedding": {
                        "model": "test/embed-3",
                        "vector": [0.0, 1.0, 0.0]
                    }
                }),
                &policy,
            )
            .await
            .unwrap();
        let updated = &update_result["memory"];
        assert_eq!(updated["name"], "Rust test command");
        assert_eq!(updated["creator"], "ai");
        assert_eq!(updated["created_at"], created_at);
        assert_eq!(updated["keywords"], json!(["tests"]));

        let semantic_search = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-search-semantic",
                "memory_search",
                &json!({
                    "query": "no literal words match this query",
                    "__agnes_embedding": {
                        "model": "test/embed-3",
                        "vector": [0.0, 0.99, 0.01]
                    }
                }),
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(semantic_search["memories"][0]["id"], updated["id"]);
        assert!(!db
            .get_tool_call("tc-memory-search-semantic".into())
            .await
            .unwrap()
            .unwrap()
            .params
            .unwrap()
            .contains("__agnes_embedding"));

        let updated_search = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-search-updated",
                "memory_search",
                &json!({"query": "no-fail-fast"}),
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(updated_search["memories"][0]["id"], updated["id"]);
        assert!(updated_search["memories"][0].get("agent_id").is_none());

        db.insert_memory(crate::db::repo::memory::NewMemory {
            id: "other-memory".into(),
            agent_id: "other-agent".into(),
            name: "Private memory".into(),
            keywords: vec![],
            content: "Only the other agent can update this.".into(),
            creator: "ai".into(),
            memory_type: "Note".into(),
            scope: "agent".into(),
            source: "test".into(),
            confidence: 1.0,
            embedding_id: None,
        })
        .await
        .unwrap();
        let cross_agent_update = executor
            .execute(
                "sess-1",
                None,
                "tc-memory-update-cross-agent",
                "memory_update",
                &json!({"memory_id": "other-memory", "content": "Compromised"}),
                &policy,
            )
            .await;
        assert!(cross_agent_update.is_err());
        assert_eq!(
            db.get_memory("other-memory".into(), "other-agent".into())
                .await
                .unwrap()
                .unwrap()
                .content,
            "Only the other agent can update this."
        );

        // Calendar UI writes and planner tool writes share the same local
        // provider/database. This protects both directions of visibility.
        db.create_calendar(
            "calendar-ui".into(),
            "Test calendar".into(),
            Some("#4f8a6f".into()),
            "Asia/Shanghai".into(),
        )
        .await
        .unwrap();
        db.create_calendar_event(
            "event-ui".into(),
            "calendar-ui".into(),
            "Created in UI".into(),
            "2026-07-18T01:00:00Z".into(),
            "2026-07-18T02:00:00Z".into(),
            "Asia/Shanghai".into(),
            false,
            None,
        )
        .await
        .unwrap();
        let tool_read = executor
            .execute(
                "sess-1",
                None,
                "tc-calendar-list-ui-event",
                "calendar_list",
                &json!({
                    "calendar_id": "calendar-ui",
                    "range_start": "2026-07-18T00:00:00+08:00",
                    "range_end": "2026-07-19T00:00:00+08:00"
                }),
                &policy,
            )
            .await
            .unwrap();
        assert_eq!(tool_read["events"][0]["title"], "Created in UI");

        let tool_write = executor
            .execute(
                "sess-1",
                None,
                "tc-calendar-create-agent-event",
                "calendar_event_create",
                &json!({
                    "calendar_id": "calendar-ui",
                    "title": "Created by agent",
                    "starts_at": "2026-07-18T03:00:00+08:00",
                    "ends_at": "2026-07-18T04:00:00+08:00",
                    "timezone": "Asia/Shanghai",
                    "all_day": false
                }),
                &policy,
            )
            .await
            .unwrap();
        let agent_event_id = tool_write["event"]["id"].as_str().unwrap();
        let ui_read = db
            .list_calendar_events(
                "calendar-ui".into(),
                "2026-07-18T00:00:00+08:00".into(),
                "2026-07-19T00:00:00+08:00".into(),
            )
            .await
            .unwrap();
        assert!(ui_read
            .iter()
            .any(|event| event.id == agent_event_id && event.title == "Created by agent"));

        let _ = fs::remove_dir_all(&temp_project);
        let _ = fs::remove_file(&db_path);
    }
}
