//! Native application of Codex-style multi-file patches.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct ApplyPatchTool;

const MAX_PATCH_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum PatchAction {
    Add { path: String, content: String },
    Update { path: String, body: String },
    Delete { path: String },
}

#[async_trait]
impl BuiltinTool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Apply a Codex-style multi-file patch inside the workspace. Paths must be relative and the patch must use Begin Patch/Add File/Update File/Delete File markers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": "Complete patch text beginning with '*** Begin Patch' and ending with '*** End Patch'."},
                        "cwd": {"type": "string", "description": "Optional base directory; defaults to the workspace."}
                    },
                    "required": ["patch"]
                }
            }
        })
    }

    fn risk(&self, _args: &Value) -> Risk {
        Risk::Medium
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let patch = match ctx.args.get("patch").and_then(Value::as_str) {
            Some(patch) => patch,
            None => return fail(ctx, "Missing `patch` argument").await,
        };
        if patch.len() > MAX_PATCH_BYTES {
            return fail(ctx, "Patch exceeds the 2 MiB limit").await;
        }
        let actions = match parse_patch(patch) {
            Ok(actions) => actions,
            Err(error) => return fail(ctx, &error).await,
        };
        let cwd = ctx.args.get("cwd").and_then(Value::as_str).unwrap_or(".");
        let base =
            crate::tools::builtin::normalize_path(&crate::tools::builtin::resolve_path(ctx, cwd));
        if !base.is_dir() {
            return fail(
                ctx,
                &format!("Patch base is not a directory: {}", base.display()),
            )
            .await;
        }
        ctx.update_running(&base.to_string_lossy()).await?;

        let mut virtual_files: HashMap<PathBuf, Option<String>> = HashMap::new();
        let mut order = Vec::new();
        for action in actions {
            let relative = match &action {
                PatchAction::Add { path, .. }
                | PatchAction::Update { path, .. }
                | PatchAction::Delete { path } => path,
            };
            let relative_path = Path::new(relative);
            if let Err(error) = validate_relative_path(relative_path) {
                return fail(ctx, &error).await;
            }
            let target = crate::tools::builtin::normalize_path(&base.join(relative_path));
            if let Err(error) = ctx.policy.check_file_write(&target.to_string_lossy()) {
                return fail(ctx, &error).await;
            }
            if !virtual_files.contains_key(&target) {
                let initial = match tokio::fs::read_to_string(&target).await {
                    Ok(content) => Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        return fail(
                            ctx,
                            &format!("Unable to read {}: {error}", target.display()),
                        )
                        .await
                    }
                };
                virtual_files.insert(target.clone(), initial);
                order.push(target.clone());
            }

            let current = virtual_files.get(&target).cloned().flatten();
            let next = match action {
                PatchAction::Add { content, .. } => {
                    if current.is_some() {
                        return fail(ctx, &format!("File already exists: {}", target.display()))
                            .await;
                    }
                    Some(content)
                }
                PatchAction::Update { body, .. } => {
                    let current = match current {
                        Some(content) => content,
                        None => {
                            return fail(ctx, &format!("File does not exist: {}", target.display()))
                                .await
                        }
                    };
                    match apply_update(&current, &body) {
                        Ok(updated) => Some(updated),
                        Err(error) => {
                            return fail(
                                ctx,
                                &format!("Unable to update {}: {error}", target.display()),
                            )
                            .await
                        }
                    }
                }
                PatchAction::Delete { .. } => {
                    if current.is_none() {
                        return fail(ctx, &format!("File does not exist: {}", target.display()))
                            .await;
                    }
                    None
                }
            };
            virtual_files.insert(target, next);
        }

        let mut changed = Vec::new();
        for target in order {
            match virtual_files.remove(&target).flatten() {
                Some(content) => {
                    if let Some(parent) = target.parent() {
                        if let Err(error) = tokio::fs::create_dir_all(parent).await {
                            return fail(
                                ctx,
                                &format!("Unable to create {}: {error}", parent.display()),
                            )
                            .await;
                        }
                    }
                    if let Err(error) = tokio::fs::write(&target, content).await {
                        return fail(
                            ctx,
                            &format!("Unable to write {}: {error}", target.display()),
                        )
                        .await;
                    }
                }
                None => {
                    if let Err(error) = tokio::fs::remove_file(&target).await {
                        return fail(
                            ctx,
                            &format!("Unable to delete {}: {error}", target.display()),
                        )
                        .await;
                    }
                }
            }
            changed.push(
                target
                    .strip_prefix(&base)
                    .unwrap_or(&target)
                    .to_string_lossy()
                    .to_string(),
            );
        }

        let summary = format!(
            "Applied patch to {} file(s): {}",
            changed.len(),
            changed.join(", ")
        );
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(summary.clone()),
            None,
        )
        .await?;
        Ok(json!({"success": true, "changed_files": changed, "stdout": summary}))
    }
}

fn parse_patch(input: &str) -> Result<Vec<PatchAction>, String> {
    let lines: Vec<&str> = input.split_inclusive('\n').collect();
    if lines.is_empty() || clean_line(lines[0]) != "*** Begin Patch" {
        return Err("Patch must begin with `*** Begin Patch`".to_string());
    }

    let mut actions = Vec::new();
    let mut index = 1;
    let mut found_end = false;
    while index < lines.len() {
        let header = clean_line(lines[index]);
        if header == "*** End Patch" {
            found_end = true;
            index += 1;
            break;
        }

        let (kind, path) = if let Some(path) = header.strip_prefix("*** Add File: ") {
            ("add", path)
        } else if let Some(path) = header.strip_prefix("*** Update File: ") {
            ("update", path)
        } else if let Some(path) = header.strip_prefix("*** Delete File: ") {
            ("delete", path)
        } else if header.is_empty() {
            index += 1;
            continue;
        } else {
            return Err(format!("Unexpected patch marker: {header}"));
        };
        if path.is_empty() {
            return Err("Patch action path must not be empty".to_string());
        }

        index += 1;
        let body_start = index;
        while index < lines.len() && !is_action_marker(clean_line(lines[index])) {
            index += 1;
        }
        let body = &lines[body_start..index];
        match kind {
            "add" => {
                let mut content = String::new();
                for line in body {
                    if clean_line(line) == "\\ No newline at end of file" {
                        if content.ends_with('\n') {
                            content.pop();
                        }
                        continue;
                    }
                    let added = line.strip_prefix('+').ok_or_else(|| {
                        "Every Add File content line must start with `+`".to_string()
                    })?;
                    content.push_str(added);
                }
                actions.push(PatchAction::Add {
                    path: path.to_string(),
                    content,
                });
            }
            "update" => actions.push(PatchAction::Update {
                path: path.to_string(),
                body: body.concat(),
            }),
            "delete" => {
                if body.iter().any(|line| !clean_line(line).is_empty()) {
                    return Err("Delete File actions cannot contain a body".to_string());
                }
                actions.push(PatchAction::Delete {
                    path: path.to_string(),
                });
            }
            _ => unreachable!(),
        }
    }

    if !found_end {
        return Err("Patch must end with `*** End Patch`".to_string());
    }
    if lines[index..]
        .iter()
        .any(|line| !clean_line(line).is_empty())
    {
        return Err("Unexpected content after `*** End Patch`".to_string());
    }
    if actions.is_empty() {
        return Err("Patch does not contain any file actions".to_string());
    }
    Ok(actions)
}

fn apply_update(original: &str, body: &str) -> Result<String, String> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    if lines.is_empty() {
        return Err("Update File action has no hunks".to_string());
    }

    let mut content = original.to_string();
    let mut search_from = 0;
    let mut index = 0;
    while index < lines.len() {
        let header = clean_line(lines[index]);
        if !header.starts_with("@@") {
            return Err(format!("Expected hunk header, found: {header}"));
        }
        index += 1;
        let mut old = String::new();
        let mut new = String::new();
        let mut last_prefix = None;
        while index < lines.len() && !clean_line(lines[index]).starts_with("@@") {
            let raw = lines[index];
            if clean_line(raw) == "\\ No newline at end of file" {
                match last_prefix {
                    Some('-') => remove_final_newline(&mut old),
                    Some('+') => remove_final_newline(&mut new),
                    Some(' ') => {
                        remove_final_newline(&mut old);
                        remove_final_newline(&mut new);
                    }
                    _ => return Err("Invalid no-newline marker".to_string()),
                }
                index += 1;
                continue;
            }
            let (prefix, text) = raw.split_at(1);
            match prefix {
                " " => {
                    old.push_str(text);
                    new.push_str(text);
                    last_prefix = Some(' ');
                }
                "-" => {
                    old.push_str(text);
                    last_prefix = Some('-');
                }
                "+" => {
                    new.push_str(text);
                    last_prefix = Some('+');
                }
                _ => return Err(format!("Invalid hunk line: {}", clean_line(raw))),
            }
            index += 1;
        }

        let position = if old.is_empty() {
            let line = parse_old_start(header)
                .ok_or_else(|| "Insertion-only hunks require numeric line ranges".to_string())?;
            line_offset(&content, line)
                .ok_or_else(|| format!("Hunk line {line} is outside the file"))?
        } else {
            find_hunk_position(&content, &old, header, search_from)?
        };
        content.replace_range(position..position + old.len(), &new);
        search_from = position + new.len();
    }
    Ok(content)
}

fn find_hunk_position(
    content: &str,
    old: &str,
    header: &str,
    search_from: usize,
) -> Result<usize, String> {
    if let Some(line) = parse_old_start(header) {
        if let Some(expected) = line_offset(content, line) {
            if content[expected..].starts_with(old) {
                return Ok(expected);
            }
        }
    }
    let matches: Vec<_> = content[search_from..]
        .match_indices(old)
        .map(|(position, _)| search_from + position)
        .collect();
    match matches.as_slice() {
        [position] => Ok(*position),
        [] => Err("Hunk context was not found".to_string()),
        _ => Err("Hunk context is ambiguous; include more unchanged lines".to_string()),
    }
}

fn parse_old_start(header: &str) -> Option<usize> {
    let range = header.strip_prefix("@@ -")?.split_whitespace().next()?;
    range.split(',').next()?.parse().ok()
}

fn line_offset(content: &str, one_based_line: usize) -> Option<usize> {
    if one_based_line == 0 || one_based_line == 1 {
        return Some(0);
    }
    let mut current_line = 1;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            current_line += 1;
            if current_line == one_based_line {
                return Some(index + 1);
            }
        }
    }
    None
}

fn validate_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("Patch path must be relative: {}", path.display()));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "Patch path cannot escape its base: {}",
            path.display()
        ));
    }
    Ok(())
}

fn clean_line(line: &str) -> &str {
    let without_lf = line.strip_suffix('\n').unwrap_or(line);
    without_lf.strip_suffix('\r').unwrap_or(without_lf)
}

fn is_action_marker(line: &str) -> bool {
    line == "*** End Patch"
        || line.starts_with("*** Add File: ")
        || line.starts_with("*** Update File: ")
        || line.starts_with("*** Delete File: ")
}

fn remove_final_newline(value: &mut String) {
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_action_types() {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+new\n*** Update File: old.txt\n@@\n-old\n+updated\n*** Delete File: gone.txt\n*** End Patch\n";
        let actions = parse_patch(patch).unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions[0],
            PatchAction::Add {
                path: "new.txt".to_string(),
                content: "new\n".to_string()
            }
        );
    }

    #[test]
    fn applies_contextual_update() {
        let body = "@@\n alpha\n-beta\n+bravo\n gamma\n";
        let updated = apply_update("alpha\nbeta\ngamma\n", body).unwrap();
        assert_eq!(updated, "alpha\nbravo\ngamma\n");
    }

    #[test]
    fn rejects_ambiguous_and_escaping_changes() {
        let ambiguous = "@@\n-same\n+changed\n";
        assert!(apply_update("same\nsame\n", ambiguous).is_err());
        assert!(validate_relative_path(Path::new("../outside.txt")).is_err());
        assert!(validate_relative_path(Path::new("inside.txt")).is_ok());
    }
}
