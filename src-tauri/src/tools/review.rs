//! Read-only change previews for file mutation tool approvals.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tools::builtin::normalize_path;
use crate::tools::policy::{expand_home, ToolPolicy};

const MAX_PREVIEW_CHARS: usize = 32_000;
const MAX_SOURCE_CHARS: usize = 512_000;
const MAX_SOURCE_BYTES: u64 = (MAX_SOURCE_CHARS * 4) as u64;

/// Build a bounded preview without mutating the workspace.
pub async fn preview_tool_call(
    tool: &str,
    args: &Value,
    workspace_cwd: Option<&Path>,
    policy: &ToolPolicy,
) -> Option<String> {
    let preview = match tool {
        "apply_patch" => args
            .get("patch")
            .and_then(Value::as_str)
            .map(str::to_string),
        "file_edit" => preview_file_edit(args, workspace_cwd, policy).await,
        "file_write" => preview_file_write(args, workspace_cwd, policy).await,
        _ => None,
    }?;
    Some(bound_preview(preview))
}

async fn preview_file_edit(
    args: &Value,
    workspace_cwd: Option<&Path>,
    policy: &ToolPolicy,
) -> Option<String> {
    let path = preview_path(args.get("path")?.as_str()?, workspace_cwd, policy)?;
    let old = args.get("old_string")?.as_str()?;
    let new = args.get("new_string")?.as_str()?;
    if old.is_empty() || !allowed_path(policy, workspace_cwd, &path) {
        return None;
    }
    let before = read_preview_file(&path).await?;
    let occurrences = before.match_indices(old).count();
    let replace_all = args
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if occurrences == 0 || (!replace_all && occurrences != 1) {
        return None;
    }
    let after = if replace_all {
        before.replace(old, new)
    } else {
        before.replacen(old, new, 1)
    };
    Some(format_diff(&path, &before, &after))
}

async fn preview_file_write(
    args: &Value,
    workspace_cwd: Option<&Path>,
    policy: &ToolPolicy,
) -> Option<String> {
    let path = preview_path(args.get("path")?.as_str()?, workspace_cwd, policy)?;
    let content = args.get("content")?.as_str()?;
    if content.chars().count() > MAX_SOURCE_CHARS {
        return Some(format!(
            "变更预览不可用：目标内容超过 {} 字符上限",
            MAX_SOURCE_CHARS
        ));
    }
    if !allowed_path(policy, workspace_cwd, &path) {
        return None;
    }
    let before = match read_preview_file(&path).await {
        Some(content) => content,
        None if !path.exists() => String::new(),
        None => return None,
    };
    Some(format_diff(&path, &before, content))
}

async fn read_preview_file(path: &Path) -> Option<String> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES => return None,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(String::new()),
        Err(_) => return None,
    }
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(_) => return None,
    };
    (content.chars().count() <= MAX_SOURCE_CHARS).then_some(content)
}

fn preview_path(path: &str, workspace_cwd: Option<&Path>, policy: &ToolPolicy) -> Option<PathBuf> {
    let expanded = expand_home(path);
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
                    .map(|path| expand_home(path))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join(expanded)
    };
    Some(normalize_path(&resolved))
}

fn allowed_path(policy: &ToolPolicy, workspace_cwd: Option<&Path>, path: &Path) -> bool {
    if !policy.file.enabled {
        return false;
    }
    let path = path.to_string_lossy();
    policy.check_file_read(&path).is_ok()
        || workspace_cwd.is_some_and(|root| path_under_root(path.as_ref(), root))
}

fn path_under_root(path: &str, root: &Path) -> bool {
    let path = Path::new(path);
    let root = normalize_path(root);
    path == root || path.starts_with(root)
}

fn format_diff(path: &Path, before: &str, after: &str) -> String {
    let patch = diffy::create_patch(before, after).to_string();
    format!("文件: {}\n{}", path.display(), patch)
}

fn bound_preview(preview: String) -> String {
    let mut chars = preview.chars();
    let bounded: String = chars.by_ref().take(MAX_PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        format!("{bounded}\n... 变更预览已截断")
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy_for(root: &Path) -> ToolPolicy {
        let mut policy = ToolPolicy::default();
        policy
            .file
            .allowed_roots
            .push(root.to_string_lossy().to_string());
        policy
    }

    #[tokio::test]
    async fn previews_file_edit_without_writing() {
        let root = std::env::temp_dir().join(format!("agnes-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        std::fs::write(&path, "before\n").unwrap();

        let preview = preview_tool_call(
            "file_edit",
            &json!({"path": "notes.txt", "old_string": "before", "new_string": "after"}),
            Some(&root),
            &policy_for(&root),
        )
        .await
        .unwrap();

        assert!(preview.contains("before"));
        assert!(preview.contains("after"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn previews_new_file_and_raw_patches() {
        let root = std::env::temp_dir().join(format!("agnes-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let policy = policy_for(&root);

        let file_preview = preview_tool_call(
            "file_write",
            &json!({"path": "new.txt", "content": "new content\n"}),
            Some(&root),
            &policy,
        )
        .await
        .unwrap();
        assert!(file_preview.contains("new content"));

        let patch = "*** Begin Patch\n*** Add File: new.txt\n+new content\n*** End Patch";
        let patch_preview = preview_tool_call(
            "apply_patch",
            &json!({"patch": patch}),
            Some(&root),
            &policy,
        )
        .await
        .unwrap();
        assert_eq!(patch_preview, patch);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn does_not_preview_files_outside_allowed_roots() {
        let root = std::env::temp_dir().join(format!("agnes-review-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!("agnes-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let path = outside.join("private.txt");
        std::fs::write(&path, "private\n").unwrap();

        let preview = preview_tool_call(
            "file_edit",
            &json!({"path": path, "old_string": "private", "new_string": "public"}),
            Some(&root),
            &policy_for(&root),
        )
        .await;

        assert!(preview.is_none());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[tokio::test]
    async fn does_not_read_files_when_file_tools_are_disabled() {
        let root = std::env::temp_dir().join(format!("agnes-review-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        std::fs::write(&path, "before\n").unwrap();
        let mut policy = policy_for(&root);
        policy.file.enabled = false;

        let preview = preview_tool_call(
            "file_edit",
            &json!({"path": path, "old_string": "before", "new_string": "after"}),
            Some(&root),
            &policy,
        )
        .await;

        assert!(preview.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
