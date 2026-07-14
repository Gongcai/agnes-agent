use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use serde_json::json;

use crate::db::DbActorHandle;
use crate::db::repo::tools::NewToolCall;
use crate::error::{AppError, AppResult};
use crate::tools::policy::ToolPolicy;

pub struct ToolExecutor {
    db: DbActorHandle,
}

impl ToolExecutor {
    pub fn new(db: DbActorHandle) -> Self {
        Self { db }
    }

    /// 执行工具调用。在此方法中负责：
    /// 1. Policy 规则判定 (capability sandbox)
    /// 2. 插入初始审计日志 (tool_calls)
    /// 3. 执行物理调用 (shell, file, git) 并写入执行日志 (stdout, stderr, exit_code, status)
    pub async fn execute(
        &self,
        session_id: &str,
        message_id: Option<&str>,
        tool_call_id: &str,
        tool: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
    ) -> AppResult<serde_json::Value> {
        // A. 审计初志插入
        let new_tc = NewToolCall {
            id: tool_call_id.to_string(),
            session_id: session_id.to_string(),
            message_id: message_id.map(|x| x.to_string()),
            tool: tool.to_string(),
            params: Some(arguments.to_string()),
            status: "running".to_string(),
            risk_level: Some(self.determine_risk(tool, arguments)),
            approval_policy_snapshot: Some(serde_json::to_string(policy).unwrap_or_default()),
        };
        self.db.insert_tool_call(new_tc).await?;

        // B. 工具路由与执行
        let res = match tool {
            "shell" => self.execute_shell(session_id, tool_call_id, arguments, policy).await,
            "file_read" => self.execute_file_read(session_id, tool_call_id, arguments, policy).await,
            "file_write" => self.execute_file_write(session_id, tool_call_id, arguments, policy).await,
            "git" => self.execute_git(session_id, tool_call_id, arguments, policy).await,
            _ => {
                let err_msg = format!("未知的内置工具: {tool}");
                self.record_failure(tool_call_id, &err_msg).await?;
                Err(AppError::Other(err_msg))
            }
        };

        res
    }

    /// 评估任务的风险等级 (Low | Medium | High)。
    fn determine_risk(&self, tool: &str, arguments: &serde_json::Value) -> String {
        match tool {
            "shell" => {
                let cmd = arguments.get("command").and_then(|x| x.as_str()).unwrap_or("");
                if cmd.contains("rm ") || cmd.contains("delete") || cmd.contains("sudo") {
                    "High".to_string()
                } else {
                    "Medium".to_string()
                }
            }
            "file_write" => "Medium".to_string(),
            "file_read" => "Low".to_string(),
            "git" => {
                let args = arguments.get("args").and_then(|x| x.as_array());
                if let Some(arr) = args {
                    if arr.iter().any(|v| v.as_str() == Some("push")) {
                        return "High".to_string();
                    }
                }
                "Low".to_string()
            }
            _ => "Low".to_string(),
        }
    }

    /// 统一失败审计落库。
    async fn record_failure(&self, tool_call_id: &str, err_msg: &str) -> AppResult<()> {
        self.db.update_tool_call_complete(
            tool_call_id.to_string(),
            "failed".to_string(),
            None,
            Some(-1),
            None,
            Some(err_msg.to_string()),
        ).await
    }

    /// 1. 执行 Shell 命令
    async fn execute_shell(
        &self,
        _session_id: &str,
        tool_call_id: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
    ) -> AppResult<serde_json::Value> {
        let command_str = arguments
            .get("command")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `command` 参数".into()))?;

        let cwd_str = arguments
            .get("cwd")
            .and_then(|x| x.as_str())
            .unwrap_or(".");
        
        let expanded_cwd = crate::tools::policy::expand_home(cwd_str);
        let cwd_absolute = expanded_cwd
            .canonicalize()
            .unwrap_or(expanded_cwd);

        // Policy 校验
        if let Err(e) = policy.check_shell(&cwd_absolute.to_string_lossy()) {
            self.record_failure(tool_call_id, &e).await?;
            return Err(AppError::Other(e));
        }

        self.db.update_tool_call_running(tool_call_id.to_string(), cwd_absolute.to_string_lossy().to_string()).await?;

        // 构造子进程，过滤环境变量
        let mut child = Command::new("bash");
        child.arg("-c").arg(command_str);
        child.current_dir(&cwd_absolute);
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        child.env_clear();

        for env_name in &policy.shell.env_allowlist {
            if let Some(val) = std::env::var_os(env_name) {
                child.env(env_name, val);
            }
        }

        let mut spawned = match child.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("无法生成 Shell 子进程: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        };

        // 超时控制
        let timeout_duration = Duration::from_secs(policy.shell.timeout_sec as u64);
        let run_result = tokio::time::timeout(timeout_duration, spawned.wait()).await;

        match run_result {
            Ok(Ok(status)) => {
                // 读取 stdout 与 stderr
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();

                if let Some(mut stdout) = spawned.stdout.take() {
                    let _ = stdout.take(policy.shell.max_output_bytes as u64).read_to_end(&mut stdout_buf).await;
                }
                if let Some(mut stderr) = spawned.stderr.take() {
                    let _ = stderr.take(policy.shell.max_output_bytes as u64).read_to_end(&mut stderr_buf).await;
                }

                let stdout_str = String::from_utf8_lossy(&stdout_buf).to_string();
                let stderr_str = String::from_utf8_lossy(&stderr_buf).to_string();
                let exit_code = status.code().unwrap_or(-1);
                
                let success = status.success();
                let status_name = if success { "done" } else { "failed" };

                // 审计日志写入
                self.db.update_tool_call_complete(
                    tool_call_id.to_string(),
                    status_name.to_string(),
                    Some(if success { "success" } else { "error" }.to_string()),
                    Some(exit_code),
                    Some(stdout_str.clone()),
                    Some(stderr_str.clone()),
                ).await?;

                Ok(json!({
                    "exit_code": exit_code,
                    "stdout": stdout_str,
                    "stderr": stderr_str,
                }))
            }
            Ok(Err(e)) => {
                let err_msg = format!("执行出错: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                Err(AppError::Other(err_msg))
            }
            Err(_) => {
                let _ = spawned.kill().await;
                let err_msg = format!("执行超时 (限制 {} 秒)", policy.shell.timeout_sec);
                self.db.update_tool_call_complete(
                    tool_call_id.to_string(),
                    "cancelled".to_string(),
                    None,
                    Some(-9),
                    None,
                    Some(err_msg.clone()),
                ).await?;
                Err(AppError::Other(err_msg))
            }
        }
    }

    /// 2. 读取文件
    async fn execute_file_read(
        &self,
        _session_id: &str,
        tool_call_id: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
    ) -> AppResult<serde_json::Value> {
        let path_str = arguments
            .get("path")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `path` 参数".into()))?;

        let expanded_path = crate::tools::policy::expand_home(path_str);

        // Policy 校验
        if let Err(e) = policy.check_file_read(&expanded_path.to_string_lossy()) {
            self.record_failure(tool_call_id, &e).await?;
            return Err(AppError::Other(e));
        }

        match tokio::fs::read_to_string(&expanded_path).await {
            Ok(content) => {
                self.db.update_tool_call_complete(
                    tool_call_id.to_string(),
                    "done".to_string(),
                    Some("success".to_string()),
                    Some(0),
                    Some(format!("已读取 {} 字节", content.len())),
                    None,
                ).await?;

                Ok(json!({ "content": content }))
            }
            Err(e) => {
                let err_msg = format!("无法读取文件: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                Err(AppError::Other(err_msg))
            }
        }
    }

    /// 3. 写入文件
    async fn execute_file_write(
        &self,
        _session_id: &str,
        tool_call_id: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
    ) -> AppResult<serde_json::Value> {
        let path_str = arguments
            .get("path")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `path` 参数".into()))?;

        let content = arguments
            .get("content")
            .and_then(|x| x.as_str())
            .ok_or_else(|| AppError::Other("缺少 `content` 参数".into()))?;

        let expanded_path = crate::tools::policy::expand_home(path_str);

        // Policy 校验
        if let Err(e) = policy.check_file_write(&expanded_path.to_string_lossy()) {
            self.record_failure(tool_call_id, &e).await?;
            return Err(AppError::Other(e));
        }

        // 自动创建父目录
        if let Some(parent) = expanded_path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                let err_msg = format!("无法创建父目录: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        }

        match tokio::fs::write(&expanded_path, content).await {
            Ok(_) => {
                self.db.update_tool_call_complete(
                    tool_call_id.to_string(),
                    "done".to_string(),
                    Some("success".to_string()),
                    Some(0),
                    Some(format!("已写入 {} 字节", content.len())),
                    None,
                ).await?;

                Ok(json!({ "success": true }))
            }
            Err(e) => {
                let err_msg = format!("无法写入文件: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                Err(AppError::Other(err_msg))
            }
        }
    }

    /// 4. 执行 Git 命令
    async fn execute_git(
        &self,
        _session_id: &str,
        tool_call_id: &str,
        arguments: &serde_json::Value,
        policy: &ToolPolicy,
    ) -> AppResult<serde_json::Value> {
        let args_val = arguments
            .get("args")
            .and_then(|x| x.as_array())
            .ok_or_else(|| AppError::Other("缺少 `args` 参数".into()))?;

        let mut args = Vec::new();
        for v in args_val {
            if let Some(s) = v.as_str() {
                args.push(s.to_string());
            }
        }

        let cwd_str = arguments
            .get("cwd")
            .and_then(|x| x.as_str())
            .unwrap_or(".");
        
        let expanded_cwd = crate::tools::policy::expand_home(cwd_str);
        let cwd_absolute = expanded_cwd
            .canonicalize()
            .unwrap_or(expanded_cwd);

        // Policy 校验
        if let Err(e) = policy.check_git() {
            self.record_failure(tool_call_id, &e).await?;
            return Err(AppError::Other(e));
        }

        self.db.update_tool_call_running(tool_call_id.to_string(), cwd_absolute.to_string_lossy().to_string()).await?;

        let mut child = Command::new("git");
        child.args(&args);
        child.current_dir(&cwd_absolute);
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());

        let spawned = match child.spawn() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("无法生成 Git 子进程: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                return Err(AppError::Other(err_msg));
            }
        };

        // Git 默认超时 30 秒
        let timeout_duration = Duration::from_secs(30);
        let run_result = tokio::time::timeout(timeout_duration, spawned.wait_with_output()).await;

        match run_result {
            Ok(Ok(output)) => {
                let stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr_str = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code().unwrap_or(-1);
                
                let success = output.status.success();
                let status_name = if success { "done" } else { "failed" };

                self.db.update_tool_call_complete(
                    tool_call_id.to_string(),
                    status_name.to_string(),
                    Some(if success { "success" } else { "error" }.to_string()),
                    Some(exit_code),
                    Some(stdout_str.clone()),
                    Some(stderr_str.clone()),
                ).await?;

                Ok(json!({
                    "exit_code": exit_code,
                    "stdout": stdout_str,
                    "stderr": stderr_str,
                }))
            }
            Ok(Err(e)) => {
                let err_msg = format!("执行 Git 出错: {e}");
                self.record_failure(tool_call_id, &err_msg).await?;
                Err(AppError::Other(err_msg))
            }
            Err(_) => {
                let err_msg = "Git 执行超时 (限制 30 秒)";
                self.db.update_tool_call_complete(
                    tool_call_id.to_string(),
                    "cancelled".to_string(),
                    None,
                    Some(-9),
                    None,
                    Some(err_msg.to_string()),
                ).await?;
                Err(AppError::Other(err_msg.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::spawn_db_actor;
    use crate::tools::policy::{ShellPolicy, FilePolicy, GitPolicy};
    use std::fs;

    #[tokio::test]
    async fn test_tool_executor_shell_and_file() {
        let db_path = PathBuf::from("target/test_tools.db");
        if db_path.exists() {
            let _ = fs::remove_file(&db_path);
        }

        let db = spawn_db_actor(db_path.clone());
        let executor = ToolExecutor::new(db.clone());

        // Insert Agent and Session to satisfy FK constraints
        let agent = crate::db::repo::agents::NewAgent {
            id: "test-agent".into(),
            name: "Test Agent".into(),
            persona: "You are a test agent".into(),
            system_prompt: "Be a test agent".into(),
            model: "GPT-4".into(),
            tool_policy: "{}".into(),
        };
        db.insert_agent(agent).await.unwrap();

        let session = crate::db::repo::sessions::NewSession {
            id: "sess-1".into(),
            agent_id: "test-agent".into(),
            title: "Test Title".into(),
            context_limit: None,
            compress_threshold: Some(0.8),
            recency_window: Some(15),
            reserved_output_tokens: None,
            summarizer_model: None,
            origin_device_id: None,
        };
        db.insert_session(session).await.unwrap();

        // Create a temporary project directory for testing
        let temp_project = std::env::current_dir().unwrap().join("target/test_tool_project");
        let _ = fs::create_dir_all(&temp_project);

        let policy = ToolPolicy {
            shell: ShellPolicy {
                enabled: true,
                approval: "never".to_string(),
                allowed_cwd: vec![temp_project.to_string_lossy().to_string()],
                deny_write_outside_workspace: true,
                timeout_sec: 5,
                max_output_bytes: 1000,
                env_allowlist: vec!["PATH".to_string()],
            },
            file: FilePolicy {
                enabled: true,
                approval: "never".to_string(),
                allowed_roots: vec![temp_project.to_string_lossy().to_string()],
            },
            git: GitPolicy {
                enabled: true,
                approval: "never".to_string(),
            },
        };

        // 1. Test File Write (Allowed)
        let test_file = temp_project.join("test_write.txt");
        let write_args = json!({
            "path": test_file.to_string_lossy().to_string(),
            "content": "Hello ToolExecutor!"
        });
        
        let write_res = executor.execute(
            "sess-1",
            None,
            "tc-write-1",
            "file_write",
            &write_args,
            &policy,
        ).await.unwrap();

        assert_eq!(write_res.get("success").unwrap().as_bool().unwrap(), true);
        assert!(test_file.exists());
        assert_eq!(fs::read_to_string(&test_file).unwrap(), "Hello ToolExecutor!");

        // 2. Test File Read (Allowed)
        let read_args = json!({
            "path": test_file.to_string_lossy().to_string()
        });
        let read_res = executor.execute(
            "sess-1",
            None,
            "tc-read-1",
            "file_read",
            &read_args,
            &policy,
        ).await.unwrap();
        assert_eq!(read_res.get("content").unwrap().as_str().unwrap(), "Hello ToolExecutor!");

        // 3. Test File Read (Outside allowed roots -> Error)
        let bad_file = PathBuf::from("/etc/passwd");
        let bad_read_args = json!({
            "path": bad_file.to_string_lossy().to_string()
        });
        let read_err = executor.execute(
            "sess-1",
            None,
            "tc-read-bad",
            "file_read",
            &bad_read_args,
            &policy,
        ).await;
        assert!(read_err.is_err());

        // 4. Test Shell Execution (Allowed)
        let shell_args = json!({
            "command": "echo 'Hello Shell'",
            "cwd": temp_project.to_string_lossy().to_string()
        });
        let shell_res = executor.execute(
            "sess-1",
            None,
            "tc-shell-1",
            "shell",
            &shell_args,
            &policy,
        ).await.unwrap();
        assert_eq!(shell_res.get("exit_code").unwrap().as_i64().unwrap(), 0);
        assert!(shell_res.get("stdout").unwrap().as_str().unwrap().contains("Hello Shell"));

        // 5. Test Shell Execution (Outside allowed cwd -> Error)
        let bad_shell_args = json!({
            "command": "echo 'Hello Bad'",
            "cwd": "/"
        });
        let shell_err = executor.execute(
            "sess-1",
            None,
            "tc-shell-bad",
            "shell",
            &bad_shell_args,
            &policy,
        ).await;
        assert!(shell_err.is_err());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_project);
        let _ = fs::remove_file(&db_path);
    }
}
