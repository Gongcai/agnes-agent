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
const TOOL_DESCRIPTION: &str = "Apply a Codex-style text patch to one or more UTF-8 files in the workspace. Supported actions are *** Add File, *** Update File, and *** Delete File with relative paths. An update may include *** Move to: <path> to rename the file, with optional hunks. Update hunks start with @@ or @@ <context>; each hunk line starts with a space for context, - for removal, or + for addition. *** End of File anchors the final hunk at EOF. Matching prefers exact complete lines, then tolerates Codex-compatible line whitespace differences; a missing final newline at EOF is also tolerated.";
const PATCH_DESCRIPTION: &str = "Complete patch from *** Begin Patch through *** End Patch. Example:\n*** Begin Patch\n*** Update File: src/example.txt\n*** Move to: src/main.txt\n@@ function_name\n-old text\n+new text\n*** End of File\n*** End Patch\nAdd File content lines must start with +; Delete File has no body. Use @@ <context> to anchor a hunk to an unchanged line, or bare @@ for a search from the current position. Use *** End of File on the final hunk when it must match the file end. A move-only update may omit hunks. To preserve a line without a trailing newline, place \\ No newline at end of file immediately after that context, removal, or addition line.";

#[derive(Debug, PartialEq, Eq)]
enum PatchAction {
    Add {
        path: String,
        content: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        body: String,
    },
    Delete {
        path: String,
    },
}

#[derive(Debug)]
struct VirtualFile {
    original: Option<String>,
    current: Option<String>,
    permissions: Option<std::fs::Permissions>,
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

        let mut virtual_files: HashMap<PathBuf, VirtualFile> = HashMap::new();
        let mut order = Vec::new();
        for action in actions {
            let source_relative = match &action {
                PatchAction::Add { path, .. }
                | PatchAction::Update { path, .. }
                | PatchAction::Delete { path } => path,
            };
            let source = match resolve_patch_target(ctx, &base, source_relative) {
                Ok(target) => target,
                Err(error) => return fail(ctx, &error).await,
            };
            let destination = match &action {
                PatchAction::Update {
                    move_to: Some(path),
                    ..
                } => match resolve_patch_target(ctx, &base, path) {
                    Ok(target) if target != source => Some(target),
                    Ok(_) => {
                        return fail(ctx, "Move destination must differ from its source").await
                    }
                    Err(error) => return fail(ctx, &error).await,
                },
                _ => None,
            };
            if let Err(error) = load_virtual_file(&source, &mut virtual_files, &mut order).await {
                return fail(ctx, &error).await;
            }
            if let Some(target) = &destination {
                if let Err(error) = load_virtual_file(target, &mut virtual_files, &mut order).await
                {
                    return fail(ctx, &error).await;
                }
            }

            let current = virtual_files
                .get(&source)
                .and_then(|file| file.current.clone());
            match action {
                PatchAction::Add { content, .. } => {
                    if current.is_some() {
                        return fail(ctx, &format!("File already exists: {}", source.display()))
                            .await;
                    }
                    virtual_files.get_mut(&source).unwrap().current = Some(content);
                }
                PatchAction::Update { move_to, body, .. } => {
                    let current = match current {
                        Some(content) => content,
                        None => {
                            return fail(ctx, &format!("File does not exist: {}", source.display()))
                                .await
                        }
                    };
                    let updated = if body.is_empty() && move_to.is_some() {
                        current
                    } else {
                        match apply_update(&current, &body) {
                            Ok(updated) => updated,
                            Err(error) => {
                                return fail(
                                    ctx,
                                    &format!("Unable to update {}: {error}", source.display()),
                                )
                                .await
                            }
                        }
                    };
                    if let Some(destination) = destination {
                        if virtual_files
                            .get(&destination)
                            .is_some_and(|file| file.current.is_some())
                        {
                            return fail(
                                ctx,
                                &format!(
                                    "Move destination already exists: {}",
                                    destination.display()
                                ),
                            )
                            .await;
                        }
                        let permissions = virtual_files
                            .get(&source)
                            .and_then(|file| file.permissions.clone());
                        virtual_files.get_mut(&source).unwrap().current = None;
                        let destination_file = virtual_files.get_mut(&destination).unwrap();
                        destination_file.current = Some(updated);
                        destination_file.permissions = permissions;
                    } else {
                        virtual_files.get_mut(&source).unwrap().current = Some(updated);
                    }
                }
                PatchAction::Delete { .. } => {
                    if current.is_none() {
                        return fail(ctx, &format!("File does not exist: {}", source.display()))
                            .await;
                    }
                    virtual_files.get_mut(&source).unwrap().current = None;
                }
            }
        }

        let changes: Vec<_> = order
            .into_iter()
            .filter_map(|target| {
                let file = virtual_files.remove(&target)?;
                (file.original != file.current).then_some((target, file.current, file.permissions))
            })
            .collect();
        for (target, current, permissions) in &changes {
            if let Some(content) = current {
                if let Some(parent) = target.parent() {
                    if let Err(error) = tokio::fs::create_dir_all(parent).await {
                        return fail(
                            ctx,
                            &format!("Unable to create {}: {error}", parent.display()),
                        )
                        .await;
                    }
                }
                if let Err(error) = tokio::fs::write(target, content).await {
                    return fail(
                        ctx,
                        &format!("Unable to write {}: {error}", target.display()),
                    )
                    .await;
                }
                if let Some(permissions) = permissions {
                    if let Err(error) =
                        tokio::fs::set_permissions(target, permissions.clone()).await
                    {
                        return fail(
                            ctx,
                            &format!("Unable to set permissions on {}: {error}", target.display()),
                        )
                        .await;
                    }
                }
            }
        }
        for (target, current, _) in &changes {
            if current.is_none() {
                if let Err(error) = tokio::fs::remove_file(target).await {
                    return fail(
                        ctx,
                        &format!("Unable to delete {}: {error}", target.display()),
                    )
                    .await;
                }
            }
        }

        let changed: Vec<_> = changes
            .into_iter()
            .map(|(target, _, _)| {
                target
                    .strip_prefix(&base)
                    .unwrap_or(&target)
                    .to_string_lossy()
                    .to_string()
            })
            .collect();

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

fn resolve_patch_target(ctx: &ToolCtx<'_>, base: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    validate_relative_path(relative_path)?;
    let target = crate::tools::builtin::normalize_path(&base.join(relative_path));
    ctx.policy
        .check_file_write(&target.to_string_lossy())
        .map_err(|error| error.to_string())?;
    ctx.sandbox
        .check_write(&target)
        .map_err(|error| error.to_string())?;
    Ok(target)
}

async fn load_virtual_file(
    target: &Path,
    files: &mut HashMap<PathBuf, VirtualFile>,
    order: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if files.contains_key(target) {
        return Ok(());
    }
    let original = match tokio::fs::read_to_string(target).await {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("Unable to read {}: {error}", target.display())),
    };
    let permissions = if original.is_some() {
        Some(
            tokio::fs::metadata(target)
                .await
                .map_err(|error| {
                    format!("Unable to read metadata for {}: {error}", target.display())
                })?
                .permissions(),
        )
    } else {
        None
    };
    files.insert(
        target.to_path_buf(),
        VirtualFile {
            original: original.clone(),
            current: original,
            permissions,
        },
    );
    order.push(target.to_path_buf());
    Ok(())
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
        let move_to = if kind == "update" && index < lines.len() {
            clean_line(lines[index])
                .strip_prefix("*** Move to: ")
                .map(|path| {
                    index += 1;
                    if path.is_empty() {
                        Err("Move destination path must not be empty".to_string())
                    } else {
                        Ok(path.to_string())
                    }
                })
                .transpose()?
        } else {
            None
        };
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
            "update" => {
                if body.is_empty() && move_to.is_none() {
                    return Err("Update File action has no hunks or move destination".to_string());
                }
                actions.push(PatchAction::Update {
                    path: path.to_string(),
                    move_to,
                    body: body.concat(),
                });
            }
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

#[derive(Debug, PartialEq, Eq)]
enum HunkAnchor {
    None,
    Text(String),
    OldLine(usize),
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedHunk {
    anchor: HunkAnchor,
    old: String,
    new: String,
    end_of_file: bool,
}

fn apply_update(original: &str, body: &str) -> Result<String, String> {
    let hunks = parse_hunks(body)?;
    if hunks.is_empty() {
        return Err("Update File action has no hunks".to_string());
    }

    let mut content = original.to_string();
    let mut search_from = 0;
    for hunk in hunks {
        let (position, replaced_len) = find_hunk_match(&content, &hunk, search_from)?;
        content.replace_range(position..position + replaced_len, &hunk.new);
        search_from = position + hunk.new.len();
    }
    Ok(content)
}

fn parse_hunks(body: &str) -> Result<Vec<ParsedHunk>, String> {
    let lines: Vec<&str> = body.split_inclusive('\n').collect();
    let mut hunks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let header = clean_line(lines[index]);
        let anchor = parse_hunk_anchor(header)?;
        index += 1;
        let mut old = String::new();
        let mut new = String::new();
        let mut last_prefix = None;
        let mut end_of_file = false;
        while index < lines.len() && !is_hunk_header(clean_line(lines[index])) {
            let raw = lines[index];
            let clean = clean_line(raw);
            if clean == "*** End of File" {
                end_of_file = true;
                index += 1;
                break;
            }
            if clean == "\\ No newline at end of file" {
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
            let Some((prefix, text)) = raw.split_at_checked(1) else {
                return Err("Hunk line must start with ` `, `-`, or `+`".to_string());
            };
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
                _ => return Err(format!("Invalid hunk line: {clean}")),
            }
            index += 1;
        }
        if old.is_empty() && new.is_empty() {
            return Err("Update hunk does not contain any lines".to_string());
        }
        if end_of_file && index < lines.len() {
            return Err("*** End of File must terminate the final update hunk".to_string());
        }
        hunks.push(ParsedHunk {
            anchor,
            old,
            new,
            end_of_file,
        });
    }
    Ok(hunks)
}

fn parse_hunk_anchor(header: &str) -> Result<HunkAnchor, String> {
    if header == "@@" {
        return Ok(HunkAnchor::None);
    }
    let Some(context) = header.strip_prefix("@@ ") else {
        return Err(format!("Expected hunk header, found: {header}"));
    };
    if context.is_empty() {
        return Err("Hunk context marker must not be empty".to_string());
    }
    if let Some(line) = parse_unified_old_start(context) {
        Ok(HunkAnchor::OldLine(line))
    } else {
        Ok(HunkAnchor::Text(context.to_string()))
    }
}

fn parse_unified_old_start(context: &str) -> Option<usize> {
    let mut fields = context.split_whitespace();
    let old_range = fields.next()?.strip_prefix('-')?;
    let new_range = fields.next()?.strip_prefix('+')?;
    if fields.next().is_some_and(|field| field != "@@") {
        return None;
    }
    old_range.split(',').next()?.parse().ok().and_then(|old| {
        new_range.split(',').next()?.parse::<usize>().ok()?;
        Some(old)
    })
}

fn is_hunk_header(line: &str) -> bool {
    line == "@@" || line.starts_with("@@ ")
}

fn find_hunk_match(
    content: &str,
    hunk: &ParsedHunk,
    search_from: usize,
) -> Result<(usize, usize), String> {
    let anchor_start = match &hunk.anchor {
        HunkAnchor::None => search_from,
        HunkAnchor::Text(context) => find_anchor(content, context, search_from)?,
        HunkAnchor::OldLine(line) => line_offset(content, *line).unwrap_or(content.len()),
    };
    if hunk.old.is_empty() {
        return match hunk.anchor {
            HunkAnchor::OldLine(line) => line_offset(content, line)
                .map(|position| (position, 0))
                .ok_or_else(|| format!("Hunk line {line} is outside the file")),
            _ => Ok((content.len(), 0)),
        };
    }
    let mut matches: Vec<_> = content[anchor_start..]
        .match_indices(&hunk.old)
        .map(|(position, _)| anchor_start + position)
        .filter(|position| {
            is_line_start(content, *position)
                && exact_match_has_valid_end(content, *position, &hunk.old)
                && (!hunk.end_of_file || *position + hunk.old.len() == content.len())
        })
        .map(|position| (position, hunk.old.len()))
        .collect();
    if matches.is_empty() {
        if let Some(value) = strip_final_line_ending(&hunk.old).filter(|value| !value.is_empty()) {
            if content[anchor_start..].ends_with(value)
                && (!hunk.end_of_file || content.ends_with(value))
            {
                let position = content.len() - value.len();
                if position >= anchor_start && is_line_start(content, position) {
                    matches.push((position, value.len()));
                }
            }
        }
    }
    for mode in [WhitespaceMode::Rstrip, WhitespaceMode::Strip] {
        if matches.is_empty() {
            matches = find_line_sequence_matches(content, &hunk.old, anchor_start, mode)
                .into_iter()
                .filter(|(_, end)| !hunk.end_of_file || *end == content.len())
                .map(|(start, end)| (start, end - start))
                .collect();
        }
    }
    match matches.as_slice() {
        [matched] => Ok(*matched),
        [] => Err("Hunk context was not found. Matching prefers exact complete lines, then line whitespace fallback; verify the @@ context and line endings. Use `*** End of File` to anchor the final hunk or `\\ No newline at end of file` when its result must not end with a newline.".to_string()),
        _ => Err("Hunk context is ambiguous; include more unchanged lines".to_string()),
    }
}

#[derive(Clone, Copy)]
enum WhitespaceMode {
    Rstrip,
    Strip,
}

fn find_line_sequence_matches(
    content: &str,
    old: &str,
    search_from: usize,
    mode: WhitespaceMode,
) -> Vec<(usize, usize)> {
    let expected: Vec<_> = old.split_inclusive('\n').map(clean_line).collect();
    let actual = line_ranges(content, search_from);
    if expected.is_empty() || expected.len() > actual.len() {
        return Vec::new();
    }
    actual
        .windows(expected.len())
        .filter_map(|window| {
            (is_line_start(content, window[0].0)
                && window
                    .iter()
                    .zip(&expected)
                    .all(|((_, _, actual), expected)| {
                        normalize_line(actual, mode) == normalize_line(expected, mode)
                    }))
            .then_some((window[0].0, window[expected.len() - 1].1))
        })
        .collect()
}

fn line_ranges(content: &str, search_from: usize) -> Vec<(usize, usize, &str)> {
    let mut offset = search_from;
    content[search_from..]
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, offset, clean_line(line))
        })
        .collect()
}

fn normalize_line(line: &str, mode: WhitespaceMode) -> &str {
    match mode {
        WhitespaceMode::Rstrip => line.trim_end(),
        WhitespaceMode::Strip => line.trim(),
    }
}

fn find_anchor(content: &str, anchor: &str, search_from: usize) -> Result<usize, String> {
    for mode in [
        None,
        Some(WhitespaceMode::Rstrip),
        Some(WhitespaceMode::Strip),
    ] {
        for (start, end, line) in line_ranges(content, search_from) {
            let matches = match mode {
                None => line == anchor,
                Some(mode) => normalize_line(line, mode) == normalize_line(anchor, mode),
            };
            if is_line_start(content, start) && matches {
                return Ok(end);
            }
        }
    }
    Err(format!("Hunk context marker was not found: {anchor}"))
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
        || line.starts_with("*** Move to: ")
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
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+new\n*** Update File: old.txt\n*** Move to: moved.txt\n@@\n-old\n+updated\n*** Delete File: gone.txt\n*** End Patch\n";
        let actions = parse_patch(patch).unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(
            actions[0],
            PatchAction::Add {
                path: "new.txt".to_string(),
                content: "new\n".to_string()
            }
        );
        assert_eq!(
            actions[1],
            PatchAction::Update {
                path: "old.txt".to_string(),
                move_to: Some("moved.txt".to_string()),
                body: "@@\n-old\n+updated\n".to_string(),
            }
        );

        let move_only =
            "*** Begin Patch\n*** Update File: old.txt\n*** Move to: moved.txt\n*** End Patch\n";
        assert_eq!(
            parse_patch(move_only).unwrap(),
            vec![PatchAction::Update {
                path: "old.txt".to_string(),
                move_to: Some("moved.txt".to_string()),
                body: String::new(),
            }]
        );
    }

    #[test]
    fn applies_contextual_update() {
        let body = "@@\n alpha\n-beta\n+bravo\n gamma\n";
        let updated = apply_update("alpha\nbeta\ngamma\n", body).unwrap();
        assert_eq!(updated, "alpha\nbravo\ngamma\n");
    }

    #[test]
    fn applies_codex_context_markers_and_eof_hunks() {
        let anchored = "@@ fn second\n-value\n+updated\n";
        let source = "fn first\nvalue\nfn second\nvalue\n";
        assert_eq!(
            apply_update(source, anchored).unwrap(),
            "fn first\nvalue\nfn second\nupdated\n"
        );

        let eof = "@@\n-value\n+tail\n*** End of File\n";
        assert_eq!(
            apply_update("value\nvalue\n", eof).unwrap(),
            "value\ntail\n"
        );

        assert_eq!(
            apply_update("alpha\n", "@@\n+omega\n").unwrap(),
            "alpha\nomega\n"
        );

        let whitespace = "@@ section\n-value\n+updated\n";
        assert_eq!(
            apply_update(" section \n  value  \n", whitespace).unwrap(),
            " section \nupdated\n"
        );
    }

    #[test]
    fn rejects_invalid_context_and_non_final_eof_markers() {
        let missing_anchor = apply_update("alpha\n", "@@ omega\n-alpha\n+beta\n").unwrap_err();
        assert!(missing_anchor.contains("context marker was not found"));

        let misplaced_eof = "@@\n-alpha\n+beta\n*** End of File\n@@\n-gamma\n+delta\n";
        assert!(apply_update("alpha\ngamma\n", misplaced_eof)
            .unwrap_err()
            .contains("must terminate"));

        let ambiguous_whitespace =
            apply_update("value \nvalue\t\n", "@@\n-value\n+new\n").unwrap_err();
        assert!(ambiguous_whitespace.contains("ambiguous"));
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
        assert!(description.contains("*** Move to:"));
        assert!(patch_description.contains("*** Update File:"));
        assert!(patch_description.contains("*** End of File"));
        assert!(patch_description.contains("\\ No newline at end of file"));
    }

    #[test]
    fn missing_context_error_explains_exact_matching() {
        let error = apply_update("different\n", "@@\n-expected\n+updated\n").unwrap_err();
        assert!(error.contains("prefers exact complete lines"));
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
