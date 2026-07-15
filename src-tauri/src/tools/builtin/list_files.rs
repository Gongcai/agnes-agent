//! Read-only recursive file listing with glob filtering.

use async_trait::async_trait;
use globset::Glob;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct ListFilesTool;

#[async_trait]
impl BuiltinTool for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "List files and directories below a workspace path using a glob pattern.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Directory to search; defaults to the workspace root."},
                        "pattern": {"type": "string", "description": "Glob relative to path, for example '**/*.rs'. Defaults to '**'."},
                        "max_results": {"type": "integer", "minimum": 1, "maximum": 5000}
                    }
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Low
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let path_arg = ctx.args.get("path").and_then(Value::as_str).unwrap_or(".");
        let root = crate::tools::builtin::normalize_path(&crate::tools::builtin::resolve_path(
            ctx, path_arg,
        ));
        if let Err(error) = ctx.policy.check_file_read(&root.to_string_lossy()) {
            return fail(ctx, &error).await;
        }
        if let Err(error) = ctx.sandbox.check_read(&root) {
            return fail(ctx, &error).await;
        }
        if !root.is_dir() {
            return fail(ctx, &format!("Not a directory: {}", root.display())).await;
        }

        let pattern = ctx
            .args
            .get("pattern")
            .and_then(Value::as_str)
            .unwrap_or("**");
        let matcher = match Glob::new(pattern) {
            Ok(glob) => glob.compile_matcher(),
            Err(error) => return fail(ctx, &format!("Invalid glob pattern: {error}")).await,
        };
        let max_results = ctx
            .args
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(500)
            .clamp(1, 5000) as usize;
        ctx.update_running(&root.to_string_lossy()).await?;

        let scan_root = root.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut entries = Vec::new();
            let mut truncated = false;
            for entry in WalkDir::new(&scan_root).follow_links(false).into_iter() {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if entry.path() == scan_root {
                    continue;
                }
                let relative = match entry.path().strip_prefix(&scan_root) {
                    Ok(relative) => relative,
                    Err(_) => continue,
                };
                if !matcher.is_match(relative) {
                    continue;
                }
                if entries.len() == max_results {
                    truncated = true;
                    break;
                }
                let kind = if entry.file_type().is_dir() {
                    "directory"
                } else if entry.file_type().is_symlink() {
                    "symlink"
                } else {
                    "file"
                };
                let size = std::fs::symlink_metadata(entry.path())
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                entries.push(json!({
                    "path": relative.to_string_lossy(),
                    "kind": kind,
                    "size": size
                }));
            }
            entries.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
            (entries, truncated)
        })
        .await;

        let result = match result {
            Ok(result) => result,
            Err(error) => return fail(ctx, &format!("File listing task failed: {error}")).await,
        };

        let (entries, truncated) = result;
        let mut stdout = entries
            .iter()
            .filter_map(|entry| entry["path"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if truncated {
            stdout.push_str("\n... results truncated");
        }
        let summary = format!("Listed {} entries", entries.len());
        ctx.update_complete("done", Some("success"), Some(0), Some(summary), None)
            .await?;
        Ok(json!({"files": entries, "truncated": truncated, "stdout": stdout}))
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}
