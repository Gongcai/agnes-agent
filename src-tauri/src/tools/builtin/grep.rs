//! Read-only recursive regular-expression search.

use std::ffi::OsStr;
use std::path::Path;

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

pub struct GrepTool;

const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
const BINARY_PROBE_BYTES: usize = 8192;
const DEFAULT_MAX_RESULTS: u64 = 100;
const MAX_LINE_CHARS: usize = 300;
const MAX_STDOUT_BYTES: usize = 64 * 1024;

const VCS_DIRS: &[&str] = &[".git", ".hg", ".svn"];

const DEFAULT_IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    ".pnpm-store",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    ".parcel-cache",
    ".sidecar-build",
    ".sidecar-dist",
    ".package-dist",
    "coverage",
    "vendor",
];

const DEFAULT_IGNORED_FILES: &[&str] = &[
    "Cargo.lock",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "uv.lock",
    "poetry.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
];

const DEFAULT_IGNORED_SUFFIXES: &[&str] = &[
    ".min.js",
    ".min.mjs",
    ".min.css",
    ".bundle.js",
    ".map",
];

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
                "description": "Search UTF-8 text files recursively with a regular expression. Respects .gitignore and skips dependency/build directories and lockfiles by default.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Rust-compatible regular expression."},
                        "path": {"type": "string", "description": "File or directory to search; defaults to the workspace."},
                        "glob": {"type": "string", "description": "Optional file glob (e.g. '*.rs' or 'src/**/*.ts')."},
                        "case_sensitive": {"type": "boolean", "description": "Defaults to true."},
                        "include_ignored": {"type": "boolean", "description": "Search gitignored files, build/dependency directories, and lockfiles. Defaults to false."},
                        "max_results": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Maximum matching lines to return; defaults to 100."}
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
        let raw_glob = ctx
            .args
            .get("glob")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|glob| !glob.is_empty() && *glob != "**");
        let matcher = match build_file_matcher(raw_glob) {
            Ok(matcher) => matcher,
            Err(error) => return fail(ctx, &format!("Invalid file glob: {error}")).await,
        };
        let include_ignored = ctx
            .args
            .get("include_ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_results = ctx
            .args
            .get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_RESULTS)
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
        let options = SearchOptions {
            include_ignored,
            max_results,
            skip_generated_files: scan_root.is_dir() && !include_ignored && matcher.is_none(),
        };
        let result = tokio::task::spawn_blocking(move || {
            search_path(&scan_root, &regex, matcher.as_ref(), &options)
        })
        .await;

        let result = match result {
            Ok(result) => result,
            Err(error) => return fail(ctx, &format!("Search task failed: {error}")).await,
        };

        let SearchOutput {
            matches,
            truncated,
            stdout,
        } = result;
        let summary = format!("Found {} matches", matches.len());
        ctx.update_complete("done", Some("success"), Some(0), Some(summary), None)
            .await?;
        Ok(json!({"matches": matches, "truncated": truncated, "stdout": stdout}))
    }
}

#[derive(Clone, Copy)]
struct SearchOptions {
    include_ignored: bool,
    max_results: usize,
    skip_generated_files: bool,
}

struct SearchOutput {
    matches: Vec<Value>,
    truncated: bool,
    stdout: String,
}

fn build_file_matcher(raw_glob: Option<&str>) -> Result<Option<GlobSet>, globset::Error> {
    let Some(raw) = raw_glob else {
        return Ok(None);
    };
    let normalized = raw.strip_prefix("./").unwrap_or(raw);
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(normalized)?);
    if !normalized.contains('/') {
        builder.add(Glob::new(&format!("**/{normalized}"))?);
    }
    Ok(Some(builder.build()?))
}

fn should_skip_dir(name: &OsStr, include_ignored: bool) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if include_ignored {
        VCS_DIRS.contains(&name)
    } else {
        DEFAULT_IGNORED_DIRS.contains(&name)
    }
}

fn is_generated_or_lock_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if DEFAULT_IGNORED_FILES.contains(&file_name) {
        return true;
    }
    let lower = file_name.to_ascii_lowercase();
    DEFAULT_IGNORED_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn read_utf8_text_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let probe_end = bytes.len().min(BINARY_PROBE_BYTES);
    if bytes[..probe_end].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn format_match_snippet(
    line: &str,
    match_start_byte: usize,
    match_end_byte: usize,
    max_chars: usize,
) -> String {
    let total_chars = line.chars().count();
    if total_chars <= max_chars {
        return line.to_string();
    }

    let match_start_char = line[..match_start_byte].chars().count();
    let _ = match_end_byte;

    let leading_context = max_chars / 3;
    let mut start_char = match_start_char.saturating_sub(leading_context);
    let end_char = (start_char + max_chars).min(total_chars);
    if end_char == total_chars {
        start_char = total_chars.saturating_sub(max_chars);
    }

    let snippet: String = line
        .chars()
        .skip(start_char)
        .take(end_char - start_char)
        .collect();

    let prefix = if start_char > 0 { "..." } else { "" };
    let suffix = if end_char < total_chars { "..." } else { "" };
    format!("{prefix}{snippet}{suffix}")
}

fn search_path(
    scan_root: &Path,
    regex: &Regex,
    matcher: Option<&GlobSet>,
    options: &SearchOptions,
) -> SearchOutput {
    let base = if scan_root.is_dir() {
        scan_root.to_path_buf()
    } else {
        scan_root.parent().unwrap_or(scan_root).to_path_buf()
    };

    let mut builder = WalkBuilder::new(scan_root);
    builder
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right));

    if options.include_ignored {
        builder.standard_filters(false);
    } else {
        builder
            .hidden(false)
            .parents(false)
            .ignore(true)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .require_git(false);
    }

    let include_ignored = options.include_ignored;
    builder.filter_entry(move |entry| {
        if entry.depth() == 0 {
            return true;
        }
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            return !should_skip_dir(entry.file_name(), include_ignored);
        }
        true
    });

    let mut matches = Vec::new();
    let mut formatted_lines = Vec::new();
    let mut stdout_bytes = 0usize;
    let mut truncated = false;

    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) if entry.file_type().is_some_and(|ft| ft.is_file()) => entry,
            _ => continue,
        };
        let relative = entry.path().strip_prefix(&base).unwrap_or(entry.path());
        if let Some(matcher) = matcher {
            if !matcher.is_match(relative) {
                continue;
            }
        }
        if options.skip_generated_files && is_generated_or_lock_file(relative) {
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
        let Some(content) = read_utf8_text_file(entry.path()) else {
            continue;
        };

        let rel_display = relative.to_string_lossy();
        for (line_index, raw_line) in content.lines().enumerate() {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let Some(found) = regex.find(line) else {
                continue;
            };
            if matches.len() == options.max_results {
                truncated = true;
                break;
            }
            let line_num = line_index + 1;
            let column = line[..found.start()].chars().count() + 1;
            let snippet = format_match_snippet(line, found.start(), found.end(), MAX_LINE_CHARS);
            let formatted = format!("{rel_display}:{line_num}:{column}:{snippet}");
            let added_bytes = formatted.len() + usize::from(!formatted_lines.is_empty());
            if !formatted_lines.is_empty() && stdout_bytes + added_bytes > MAX_STDOUT_BYTES {
                truncated = true;
                break;
            }
            stdout_bytes += added_bytes;
            formatted_lines.push(formatted);
            matches.push(json!({
                "path": rel_display.as_ref(),
                "line": line_num,
                "column": column,
                "text": snippet
            }));
        }
        if truncated {
            break;
        }
    }

    let mut stdout = if formatted_lines.is_empty() {
        "No matches found.".to_string()
    } else {
        formatted_lines.join("\n")
    };
    if truncated {
        stdout.push_str(&format!(
            "\n... results truncated at {} matches (narrow search with path or glob)",
            matches.len()
        ));
    }

    SearchOutput {
        matches,
        truncated,
        stdout,
    }
}

async fn fail<T>(ctx: &ToolCtx<'_>, message: &str) -> AppResult<T> {
    ctx.record_failure(message).await?;
    Err(AppError::Other(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn default_options() -> SearchOptions {
        SearchOptions {
            include_ignored: false,
            max_results: 100,
            skip_generated_files: true,
        }
    }

    #[test]
    fn skips_gitignored_dependency_build_and_lock_files_by_default() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".gitignore"), "ignored_dir/\nsecret.log\n").unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::create_dir_all(temp.path().join("node_modules/pkg")).unwrap();
        fs::create_dir_all(temp.path().join("target/debug")).unwrap();
        fs::create_dir_all(temp.path().join(".venv/lib")).unwrap();
        fs::create_dir_all(temp.path().join("ignored_dir")).unwrap();

        fs::write(temp.path().join("src/main.rs"), "let needle = 42;\n").unwrap();
        fs::write(
            temp.path().join("node_modules/pkg/index.js"),
            "const needle = 1;\n",
        )
        .unwrap();
        fs::write(temp.path().join("target/debug/out.txt"), "needle\n").unwrap();
        fs::write(temp.path().join(".venv/lib/site.py"), "needle = True\n").unwrap();
        fs::write(temp.path().join("ignored_dir/data.txt"), "needle\n").unwrap();
        fs::write(temp.path().join("secret.log"), "needle\n").unwrap();
        fs::write(temp.path().join("Cargo.lock"), "name = \"needle\"\n").unwrap();
        fs::write(temp.path().join("app.min.js"), "var needle=1;\n").unwrap();

        let regex = Regex::new("needle").unwrap();
        let out = search_path(temp.path(), &regex, None, &default_options());
        let paths: Vec<_> = out
            .matches
            .iter()
            .map(|item| item["path"].as_str().unwrap())
            .collect();

        assert!(!out.truncated);
        assert_eq!(paths, ["src/main.rs"]);
    }

    #[test]
    fn explicit_glob_or_single_file_or_include_ignored_can_search_filtered_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(temp.path().join("ignored.txt"), "needle in ignored\n").unwrap();
        fs::write(temp.path().join("Cargo.lock"), "name = \"needle\"\n").unwrap();

        let regex = Regex::new("needle").unwrap();

        // Single-file path searches Cargo.lock directly.
        let single_file_opts = SearchOptions {
            include_ignored: false,
            max_results: 100,
            skip_generated_files: false,
        };
        let single = search_path(
            &temp.path().join("Cargo.lock"),
            &regex,
            None,
            &single_file_opts,
        );
        assert_eq!(single.matches.len(), 1);

        // Explicit glob searches lockfiles without needing include_ignored.
        let lock_matcher = build_file_matcher(Some("*.lock")).unwrap();
        let glob_opts = SearchOptions {
            include_ignored: false,
            max_results: 100,
            skip_generated_files: false,
        };
        let by_glob = search_path(temp.path(), &regex, lock_matcher.as_ref(), &glob_opts);
        assert_eq!(by_glob.matches.len(), 1);
        assert_eq!(by_glob.matches[0]["path"], "Cargo.lock");

        // include_ignored searches gitignored files too.
        let include_opts = SearchOptions {
            include_ignored: true,
            max_results: 100,
            skip_generated_files: false,
        };
        let all = search_path(temp.path(), &regex, None, &include_opts);
        assert_eq!(all.matches.len(), 2);
    }

    #[test]
    fn filename_glob_matches_nested_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("nested/deeper")).unwrap();
        fs::write(temp.path().join("root.rs"), "fn needle() {}\n").unwrap();
        fs::write(
            temp.path().join("nested/deeper/lib.rs"),
            "pub fn needle() {}\n",
        )
        .unwrap();
        fs::write(temp.path().join("nested/deeper/readme.md"), "needle\n").unwrap();

        let regex = Regex::new("needle").unwrap();
        let matcher = build_file_matcher(Some("*.rs")).unwrap();
        let out = search_path(
            temp.path(),
            &regex,
            matcher.as_ref(),
            &SearchOptions {
                skip_generated_files: false,
                ..default_options()
            },
        );
        let paths: Vec<_> = out
            .matches
            .iter()
            .map(|item| item["path"].as_str().unwrap())
            .collect();

        assert_eq!(paths, ["nested/deeper/lib.rs", "root.rs"]);
    }

    #[test]
    fn deduplicates_multiple_hits_per_line_and_truncates_long_lines() {
        let temp = tempfile::tempdir().unwrap();
        let long_line = format!(
            "{};needle_one;{};needle_two;{}",
            "a".repeat(250),
            "b".repeat(50),
            "c".repeat(250)
        );
        fs::write(temp.path().join("minified.txt"), &long_line).unwrap();
        fs::write(temp.path().join("binary.bin"), b"needle\0binary").unwrap();

        let regex = Regex::new("needle_\\w+").unwrap();
        let out = search_path(temp.path(), &regex, None, &default_options());

        assert_eq!(out.matches.len(), 1);
        assert_eq!(out.matches[0]["column"], 252);
        let snippet = out.matches[0]["text"].as_str().unwrap();
        assert!(snippet.contains("needle_one"));
        assert!(snippet.starts_with("..."));
        assert!(snippet.ends_with("..."));
        assert!(snippet.chars().count() <= MAX_LINE_CHARS + 6);

        // Greedy matches wider than MAX_LINE_CHARS still keep the match start visible.
        let greedy = Regex::new("needle_one.*needle_two").unwrap();
        let greedy_out = search_path(temp.path(), &greedy, None, &default_options());
        assert!(greedy_out.matches[0]["text"]
            .as_str()
            .unwrap()
            .contains("needle_one"));
    }
}
