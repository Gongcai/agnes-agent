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
const TOOL_DESCRIPTION: &str = "Apply an exact Codex-style text patch to one or more UTF-8 files in the workspace. Supported actions are *** Add File, *** Update File, and *** Delete File with relative paths; rename/move is not supported. Update hunks begin with @@ and each hunk line must start with a space for context, - for removal, or + for addition. Content and whitespace matching is exact, although a missing final newline at EOF is tolerated.";
const PATCH_DESCRIPTION: &str = "Complete patch from *** Begin Patch through *** End Patch. Example:\n*** Begin Patch\n*** Update File: src/example.txt\n@@\n-old text\n+new text\n*** End Patch\nAdd File content lines must start with +; Delete File has no body. Insertion-only hunks require a numeric header such as @@ -1,0 +1 @@. To keep a line without a trailing newline, place \\ No newline at end of file immediately after that context, removal, or addition line.";

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
                "description": TOOL_DESCRIPTION,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": PATCH_DESCRIPTION},
                        "cwd": {"type": "string", "description": "Optional workspace-relative base directory; defaults to the workspace root."}
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
            if let Err(error) = ctx.sandbox.check_write(&target) {
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

        let (position, replaced_len) = if old.is_empty() {
            let line = parse_old_start(header)
                .ok_or_else(|| "Insertion-only hunks require numeric line ranges".to_string())?;
            (
                line_offset(&content, line)
                    .ok_or_else(|| format!("Hunk line {line} is outside the file"))?,
                0,
            )
        } else {
            find_hunk_match(&content, &old, header, search_from)?
        };
        content.replace_range(position..position + replaced_len, &new);
        search_from = position + new.len();
    }
    Ok(content)
}

fn find_hunk_match(
    content: &str,
    old: &str,
    header: &str,
    search_from: usize,
) -> Result<(usize, usize), String> {
    let old_without_final_newline = strip_final_line_ending(old).filter(|value| !value.is_empty());
    if let Some(line) = parse_old_start(header) {
        if let Some(expected) = line_offset(content, line) {
            if content[expected..].starts_with(old)
                && exact_match_has_valid_end(content, expected, old)
            {
                return Ok((expected, old.len()));
            }
            if old_without_final_newline.is_some_and(|value| content[expected..] == *value) {
                return Ok((expected, content.len() - expected));
            }
        }
    }
    let mut matches: Vec<_> = content[search_from..]
        .match_indices(old)
        .map(|(position, _)| search_from + position)
        .filter(|position| {
            is_line_start(content, *position) && exact_match_has_valid_end(content, *position, old)
        })
        .map(|position| (position, old.len()))
        .collect();
    if let Some(value) = old_without_final_newline {
        if content[search_from..].ends_with(value) {
            let position = content.len() - value.len();
            if is_line_start(content, position) {
                matches.push((position, value.len()));
            }
        }
    }
    match matches.as_slice() {
        [matched] => Ok(*matched),
        [] => Err("Hunk context was not found. Matching is exact; verify whitespace and line endings. Use `\\ No newline at end of file` after a patch line when its result must not end with a newline.".to_string()),
        _ => Err("Hunk context is ambiguous; include more unchanged lines".to_string()),
    }
}

fn is_line_start(content: &str, position: usize) -> bool {
    position == 0 || content.as_bytes().get(position - 1) == Some(&b'\n')
}

fn exact_match_has_valid_end(content: &str, position: usize, old: &str) -> bool {
    strip_final_line_ending(old).is_some() || position + old.len() == content.len()
}

fn strip_final_line_ending(value: &str) -> Option<&str> {
    value
        .strip_suffix("\r\n")
        .or_else(|| value.strip_suffix('\n'))
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
    fn applies_update_when_the_source_has_no_trailing_newline() {
        let body = "@@\n-alpha\n+bravo\n omega\n";
        let updated = apply_update("alpha\nomega", body).unwrap();
        assert_eq!(updated, "bravo\nomega\n");

        let preserve_body =
            "@@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        assert_eq!(apply_update("old", preserve_body).unwrap(), "new");
    }

    #[test]
    fn schema_documents_the_supported_patch_contract() {
        let schema = ApplyPatchTool.schema();
        let description = schema["function"]["description"].as_str().unwrap();
        let patch_description = schema["function"]["parameters"]["properties"]["patch"]
            ["description"]
            .as_str()
            .unwrap();
        assert!(description.contains("*** Add File"));
        assert!(description.contains("rename/move is not supported"));
        assert!(patch_description.contains("*** Update File:"));
        assert!(patch_description.contains("\\ No newline at end of file"));
    }

    #[test]
    fn missing_context_error_explains_exact_matching() {
        let error = apply_update("different\n", "@@\n-expected\n+updated\n").unwrap_err();
        assert!(error.contains("Matching is exact"));
        assert!(error.contains("No newline at end of file"));
    }

    #[test]
    fn contextual_hunks_only_match_complete_lines() {
        let error = apply_update("prefixexpected\n", "@@\n-expected\n+updated\n").unwrap_err();
        assert!(error.contains("Hunk context was not found"));

        let no_newline_body =
            "@@\n-expected\n\\ No newline at end of file\n+updated\n\\ No newline at end of file\n";
        assert!(apply_update("expected suffix", no_newline_body).is_err());
    }

    #[test]
    fn rejects_ambiguous_and_escaping_changes() {
        let ambiguous = "@@\n-same\n+changed\n";
        assert!(apply_update("same\nsame\n", ambiguous).is_err());
        assert!(validate_relative_path(Path::new("../outside.txt")).is_err());
        assert!(validate_relative_path(Path::new("inside.txt")).is_ok());
    }
}
