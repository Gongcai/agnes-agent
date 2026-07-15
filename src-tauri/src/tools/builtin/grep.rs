//! Read-only recursive regular-expression search.

use async_trait::async_trait;
use globset::Glob;
use regex::RegexBuilder;
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct GrepTool;

const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[async_trait]
impl BuiltinTool for GrepTool {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Search UTF-8 files recursively with a regular expression.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Rust-compatible regular expression."},
                        "path": {"type": "string", "description": "File or directory to search; defaults to the workspace."},
                        "glob": {"type": "string", "description": "Optional file glob relative to path, for example '**/*.ts'."},
                        "case_sensitive": {"type": "boolean", "description": "Defaults to true."},
                        "max_results": {"type": "integer", "minimum": 1, "maximum": 1000}
                    },
                    "required": ["pattern"]
                }
            }
        })
    }

    fn risk(&self, args: &Value) -> Risk {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        const SENSITIVE: &[&str] = &[".ssh", "/etc", ".aws", ".gnupg"];
        if SENSITIVE.iter().any(|part| path.contains(part)) {
            Risk::Medium
        } else {
            Risk::Low
        }
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let pattern = match ctx.args.get("pattern").and_then(Value::as_str) {
            Some(pattern) => pattern,
            None => return fail(ctx, "Missing `pattern` argument").await,
        };
        let case_sensitive = ctx
            .args
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let regex = match RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(regex) => regex,
            Err(error) => return fail(ctx, &format!("Invalid regular expression: {error}")).await,
        };
        let file_glob = ctx.args.get("glob").and_then(Value::as_str).unwrap_or("**");
        let matcher = match Glob::new(file_glob) {
            Ok(glob) => glob.compile_matcher(),
            Err(error) => return fail(ctx, &format!("Invalid file glob: {error}")).await,
        };
        let max_results = ctx
            .args
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(200)
            .clamp(1, 1000) as usize;
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
        if !root.exists() {
            return fail(
                ctx,
                &format!("Search path does not exist: {}", root.display()),
            )
            .await;
        }
        ctx.update_running(&root.to_string_lossy()).await?;

        let scan_root = root.clone();
        let result = tokio::task::spawn_blocking(move || {
            let base = if scan_root.is_dir() {
                scan_root.clone()
            } else {
                scan_root.parent().unwrap_or(&scan_root).to_path_buf()
            };
            let mut matches = Vec::new();
            let mut truncated = false;
            for entry in WalkDir::new(&scan_root).follow_links(false).into_iter() {
                let entry = match entry {
                    Ok(entry) if entry.file_type().is_file() => entry,
                    _ => continue,
                };
                let relative = entry.path().strip_prefix(&base).unwrap_or(entry.path());
                if !matcher.is_match(relative) {
                    continue;
                }
                if entry
                    .metadata()
                    .map(|metadata| metadata.len())
                    .unwrap_or(u64::MAX)
                    > MAX_SEARCH_FILE_BYTES
                {
                    continue;
                }
                let content = match std::fs::read_to_string(entry.path()) {
                    Ok(content) => content,
                    Err(_) => continue,
                };
                for (line_index, line) in content.lines().enumerate() {
                    for found in regex.find_iter(line) {
                        if matches.len() == max_results {
                            truncated = true;
                            break;
                        }
                        matches.push(json!({
                            "path": relative.to_string_lossy(),
                            "line": line_index + 1,
                            "column": found.start() + 1,
                            "text": line
                        }));
                    }
                    if truncated {
                        break;
                    }
                }
                if truncated {
                    break;
                }
            }
            (matches, truncated)
        })
        .await;

        let result = match result {
            Ok(result) => result,
            Err(error) => return fail(ctx, &format!("Search task failed: {error}")).await,
        };

        let (matches, truncated) = result;
        let mut stdout = matches
            .iter()
            .map(|item| {
                format!(
                    "{}:{}:{}:{}",
                    item["path"].as_str().unwrap_or(""),
                    item["line"].as_u64().unwrap_or(0),
                    item["column"].as_u64().unwrap_or(0),
                    item["text"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if matches.is_empty() {
            stdout = "No matches found.".to_string();
        } else if truncated {
            stdout.push_str("\n... results truncated");
        }
        let summary = format!("Found {} matches", matches.len());
        ctx.update_complete("done", Some("success"), Some(0), Some(summary), None)
            .await?;
        Ok(json!({"matches": matches, "truncated": truncated, "stdout": stdout}))
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}
