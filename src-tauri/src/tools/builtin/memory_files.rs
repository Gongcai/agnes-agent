use async_trait::async_trait;
use serde_json::{json, Value};

use crate::db::repo::memory_files::{MemoryMutation, MemoryMutationOutcome};
use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

async fn agent_id(ctx: &ToolCtx<'_>) -> AppResult<String> {
    crate::memory::agent_id_for_session(ctx.db, ctx.session_id).await
}

fn required_string(args: &Value, name: &str) -> AppResult<String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::Other(format!("Missing `{name}` argument")))
}

async fn begin(ctx: &ToolCtx<'_>, agent_id: &str, path: Option<&str>) -> AppResult<()> {
    if !ctx.policy.memory.enabled {
        return fail(ctx, "Memory tools are disabled for this agent").await;
    }
    let location = match path {
        Some(path) => format!("memory://{agent_id}{path}"),
        None => format!("memory://{agent_id}/"),
    };
    ctx.update_running(&location).await
}

async fn finish_mutation(ctx: &ToolCtx<'_>, outcome: MemoryMutationOutcome) -> AppResult<Value> {
    let audit = outcome.summary.clone().unwrap_or_else(|| {
        format!(
            "{}: {}",
            outcome.code.as_deref().unwrap_or("ERROR"),
            outcome.message
        )
    });
    ctx.update_complete(
        "done",
        Some(if outcome.ok { "success" } else { "error" }),
        Some(if outcome.ok { 0 } else { 1 }),
        Some(audit),
        None,
    )
    .await?;
    Ok(serde_json::to_value(outcome)?)
}

async fn mutate(ctx: &ToolCtx<'_>, path: String, mutation: MemoryMutation) -> AppResult<Value> {
    let agent_id = agent_id(ctx).await?;
    begin(ctx, &agent_id, Some(&path)).await?;
    let outcome = ctx
        .db
        .mutate_memory_file(
            agent_id,
            path,
            mutation,
            "chat".to_string(),
            Some(ctx.session_id.to_string()),
            None,
        )
        .await?;
    finish_mutation(ctx, outcome).await
}

pub struct MemoryListTool;

#[async_trait]
impl BuiltinTool for MemoryListTool {
    fn name(&self) -> &'static str {
        "memory_list"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Refresh the complete virtual memory file listing for the current agent.",
                "parameters": {"type": "object", "properties": {}, "additionalProperties": false}
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Low
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let agent_id = agent_id(ctx).await?;
        begin(ctx, &agent_id, None).await?;
        let listing = ctx.db.list_memory_files(agent_id).await?;
        let summary = format!("Listed {} memory files", listing.files.len());
        ctx.update_complete("done", Some("success"), Some(0), Some(summary), None)
            .await?;
        Ok(serde_json::to_value(listing)?)
    }
}

// Some models ignore the `oneOf` schema and pass the path array as a
// JSON-encoded string (e.g. "[\"/profile.md\", \"/topics/x.md\"]").
// Detect that shape and decode it instead of failing with a confusing
// "must end in .md" error from the path validator.
fn parse_read_paths(args: &Value) -> AppResult<Vec<String>> {
    match args.get("path") {
        Some(Value::String(path)) => {
            let trimmed = path.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                match serde_json::from_str::<Vec<String>>(trimmed) {
                    Ok(paths) if !paths.is_empty() => Ok(paths),
                    _ => Err(AppError::Other(
                        "`path` must be a single \"/dir/name.md\" string or an array of such strings, not a JSON-encoded string".into(),
                    )),
                }
            } else {
                Ok(vec![path.clone()])
            }
        }
        Some(Value::Array(paths)) => paths
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| AppError::Other("All memory paths must be strings".into()))
            })
            .collect(),
        _ => Err(AppError::Other(
            "Missing or invalid `path` argument: pass a single \"/dir/name.md\" string or an array of up to 20 such strings".into(),
        )),
    }
}

pub struct MemoryReadTool;

#[async_trait]
impl BuiltinTool for MemoryReadTool {
    fn name(&self) -> &'static str {
        "memory_read"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Read one or up to 20 complete virtual memory files for the current agent. Returns opaque version tokens required for edits. Pass multiple paths as a JSON array of strings, e.g. [\"/profile.md\", \"/topics/x.md\"].",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "oneOf": [
                                {"type": "string"},
                                {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 20, "uniqueItems": true}
                            ]
                        }
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Low
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let paths = match parse_read_paths(ctx.args) {
            Ok(paths) => paths,
            Err(error) => return fail(ctx, &error.to_string()).await,
        };
        let agent_id = agent_id(ctx).await?;
        begin(ctx, &agent_id, paths.first().map(String::as_str)).await?;
        let files = ctx.db.read_memory_files(agent_id, paths).await?;
        let found = files.iter().filter(|file| file.exists).count();
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Read {found} memory files")),
            None,
        )
        .await?;
        Ok(json!({"files": files}))
    }
}

pub struct MemoryWriteTool;

#[async_trait]
impl BuiltinTool for MemoryWriteTool {
    fn name(&self) -> &'static str {
        "memory_write"
    }

    fn schema(&self) -> Value {
        mutation_schema(
            self.name(),
            "Create or fully rewrite one virtual Markdown memory file using an opaque version token. The content must include canonical frontmatter and tagged fact lines.",
            json!({
                "path": {"type": "string"},
                "content": {"type": "string"},
                "if_version": {"type": "string"}
            }),
            &["path", "content", "if_version"],
        )
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        mutate(
            ctx,
            required_string(ctx.args, "path")?,
            MemoryMutation::Write {
                content: required_string(ctx.args, "content")?,
                if_version: required_string(ctx.args, "if_version")?,
            },
        )
        .await
    }
}

pub struct MemoryStrReplaceTool;

#[async_trait]
impl BuiltinTool for MemoryStrReplaceTool {
    fn name(&self) -> &'static str {
        "memory_str_replace"
    }

    fn schema(&self) -> Value {
        mutation_schema(
            self.name(),
            "Replace exactly one matching string in a virtual memory file. An empty new_str deletes that exact block.",
            json!({
                "path": {"type": "string"},
                "old_str": {"type": "string", "minLength": 1},
                "new_str": {"type": "string"},
                "if_version": {"type": "string"}
            }),
            &["path", "old_str", "new_str", "if_version"],
        )
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        mutate(
            ctx,
            required_string(ctx.args, "path")?,
            MemoryMutation::Replace {
                old_str: required_string(ctx.args, "old_str")?,
                new_str: required_string(ctx.args, "new_str")?,
                if_version: required_string(ctx.args, "if_version")?,
            },
        )
        .await
    }
}

pub struct MemoryAppendTool;

#[async_trait]
impl BuiltinTool for MemoryAppendTool {
    fn name(&self) -> &'static str {
        "memory_append"
    }

    fn schema(&self) -> Value {
        mutation_schema(
            self.name(),
            "Append new tagged fact lines to an existing virtual memory file using its current version token.",
            json!({
                "path": {"type": "string"},
                "content": {"type": "string", "minLength": 1},
                "if_version": {"type": "string"}
            }),
            &["path", "content", "if_version"],
        )
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        mutate(
            ctx,
            required_string(ctx.args, "path")?,
            MemoryMutation::Append {
                content: required_string(ctx.args, "content")?,
                if_version: required_string(ctx.args, "if_version")?,
            },
        )
        .await
    }
}

pub struct MemoryDeleteTool;

#[async_trait]
impl BuiltinTool for MemoryDeleteTool {
    fn name(&self) -> &'static str {
        "memory_delete"
    }

    fn schema(&self) -> Value {
        mutation_schema(
            self.name(),
            "Delete a complete virtual memory file only when the user explicitly requested that deletion.",
            json!({
                "path": {"type": "string"},
                "if_version": {"type": "string"}
            }),
            &["path", "if_version"],
        )
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::High
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        mutate(
            ctx,
            required_string(ctx.args, "path")?,
            MemoryMutation::Delete {
                if_version: required_string(ctx.args, "if_version")?,
            },
        )
        .await
    }
}

fn mutation_schema(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            }
        }
    })
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_read_paths_accepts_single_string() {
        let args = json!({"path": "/profile.md"});
        assert_eq!(
            parse_read_paths(&args).unwrap(),
            vec!["/profile.md".to_string()]
        );
    }

    #[test]
    fn parse_read_paths_accepts_array() {
        let args = json!({"path": ["/profile.md", "/topics/dev-tooling.md"]});
        assert_eq!(
            parse_read_paths(&args).unwrap(),
            vec![
                "/profile.md".to_string(),
                "/topics/dev-tooling.md".to_string()
            ]
        );
    }

    #[test]
    fn parse_read_paths_decodes_json_encoded_array_string() {
        let args = json!({"path": "[\"/profile.md\", \"/preferences.md\", \"/topics/reading.md\"]"});
        assert_eq!(
            parse_read_paths(&args).unwrap(),
            vec![
                "/profile.md".to_string(),
                "/preferences.md".to_string(),
                "/topics/reading.md".to_string()
            ]
        );
    }

    #[test]
    fn parse_read_paths_rejects_malformed_encoded_array() {
        let args = json!({"path": "[123, 456]"});
        let error = parse_read_paths(&args).unwrap_err().to_string();
        assert!(error.contains("JSON-encoded string"), "{error}");
    }

    #[test]
    fn parse_read_paths_treats_unclosed_bracket_as_single_path() {
        // "[not json" does not end with ']' so it is treated as a literal
        // path string and rejected later by the path validator.
        let args = json!({"path": "[not json"});
        assert_eq!(
            parse_read_paths(&args).unwrap(),
            vec!["[not json".to_string()]
        );
    }

    #[test]
    fn parse_read_paths_rejects_empty_encoded_array() {
        let args = json!({"path": "[]"});
        assert!(parse_read_paths(&args).is_err());
    }

    #[test]
    fn parse_read_paths_rejects_non_string_array_items() {
        let args = json!({"path": ["/profile.md", 42]});
        assert!(parse_read_paths(&args).is_err());
    }

    #[test]
    fn parse_read_paths_rejects_missing_path() {
        assert!(parse_read_paths(&json!({})).is_err());
    }
}
