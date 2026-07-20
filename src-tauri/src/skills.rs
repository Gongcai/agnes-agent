use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

use crate::error::{AppError, AppResult};

const SKILL_ENTRY_FILE: &str = "SKILL.md";
const SKILL_METADATA_FILE: &str = ".agnes-skill.json";
const MAX_SKILLS_PER_INSTALL: usize = 20;
const MAX_SKILL_FILES: usize = 500;
const MAX_SKILL_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_SKILL_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_SKILL_INSTRUCTIONS_BYTES: u64 = 256 * 1024;

fn skill_fs_lock() -> AppResult<MutexGuard<'static, ()>> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| AppError::Other("Skill 文件锁已损坏".into()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub enabled: bool,
    pub source_kind: String,
    pub source_label: String,
    pub installed_at: String,
    pub updated_at: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPromptContext {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub root_path: String,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone)]
struct ParsedSkill {
    name: String,
    description: String,
    version: Option<String>,
    author: Option<String>,
    instructions: String,
}

#[derive(Debug, Clone)]
enum SkillSource {
    Local(String),
    Git(String),
}

impl SkillSource {
    fn kind(&self) -> &'static str {
        match self {
            Self::Local(_) => "local",
            Self::Git(_) => "git",
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Local(value) | Self::Git(value) => value,
        }
    }
}

pub fn skills_root() -> AppResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Other("无法获取用户目录以存储 Skills".into()))?;
    Ok(home.join(".agnes").join("skills"))
}

fn validate_skill_id(id: &str) -> AppResult<()> {
    if id.is_empty()
        || id.len() > 80
        || !id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(AppError::Other("Invalid Skill id".into()));
    }
    Ok(())
}

fn skill_id(name: &str) -> String {
    let mut id = String::new();
    let mut previous_dash = false;
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
            previous_dash = false;
        } else if matches!(character, '-' | '_' | ' ') && !id.is_empty() && !previous_dash {
            id.push('-');
            previous_dash = true;
        }
    }
    let id = id.trim_matches('-');
    if id.is_empty() {
        let digest = Sha256::digest(name.as_bytes());
        return format!("skill-{}", hex_prefix(&digest, 12));
    }
    id.chars()
        .take(64)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    bytes
        .iter()
        .flat_map(|byte| format!("{byte:02x}").chars().collect::<Vec<_>>())
        .take(length)
        .collect()
}

fn strip_yaml_scalar(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0];
        let last = trimmed.as_bytes()[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn frontmatter_value(lines: &[&str], key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with(' ') || line.starts_with('\t') || !line.starts_with(&prefix) {
            continue;
        }
        let value = line[prefix.len()..].trim();
        if value == "|" || value == ">" {
            let mut block = Vec::new();
            for next in lines.iter().skip(index + 1) {
                if next.trim().is_empty() {
                    block.push(String::new());
                    continue;
                }
                if !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                block.push(next.trim_start().to_string());
            }
            let joined = if value == ">" {
                block.join(" ")
            } else {
                block.join("\n")
            };
            return Some(joined.trim().to_string());
        }
        return Some(strip_yaml_scalar(value));
    }
    None
}

fn parse_skill_markdown(content: &str) -> AppResult<ParsedSkill> {
    let normalized = content.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut lines = normalized.lines();
    if lines.next() != Some("---") {
        return Err(AppError::Other(
            "SKILL.md 必须以 YAML frontmatter 开头".into(),
        ));
    }
    let remaining = lines.collect::<Vec<_>>();
    let closing = remaining
        .iter()
        .position(|line| *line == "---")
        .ok_or_else(|| AppError::Other("SKILL.md 缺少 frontmatter 结束标记".into()))?;
    let metadata = &remaining[..closing];
    let instructions = remaining[closing + 1..].join("\n").trim().to_string();
    let name = frontmatter_value(metadata, "name")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Other("SKILL.md frontmatter 缺少 name".into()))?;
    let description = frontmatter_value(metadata, "description")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Other("SKILL.md frontmatter 缺少 description".into()))?;
    if name.chars().count() > 100 {
        return Err(AppError::Other("Skill 名称不能超过 100 个字符".into()));
    }
    if description.chars().count() > 4_000 {
        return Err(AppError::Other("Skill 描述不能超过 4000 个字符".into()));
    }
    if instructions.is_empty() {
        return Err(AppError::Other("SKILL.md 没有可执行的说明正文".into()));
    }
    if instructions.len() as u64 > MAX_SKILL_INSTRUCTIONS_BYTES {
        return Err(AppError::Other(format!(
            "SKILL.md 正文不能超过 {} KiB",
            MAX_SKILL_INSTRUCTIONS_BYTES / 1024
        )));
    }
    Ok(ParsedSkill {
        name,
        description,
        version: frontmatter_value(metadata, "version").filter(|value| !value.is_empty()),
        author: frontmatter_value(metadata, "author").filter(|value| !value.is_empty()),
        instructions,
    })
}

fn should_descend(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    !matches!(
        entry.file_name().to_string_lossy().as_ref(),
        ".git" | "node_modules" | "target" | "__pycache__" | ".venv" | "venv"
    )
}

fn discover_skill_directories(source: &Path) -> AppResult<Vec<PathBuf>> {
    let source = source.canonicalize()?;
    if source.is_file() {
        if source.file_name().and_then(|value| value.to_str()) != Some(SKILL_ENTRY_FILE) {
            return Err(AppError::Other("请选择 SKILL.md 或包含它的目录".into()));
        }
        return Ok(vec![source.parent().unwrap_or(&source).to_path_buf()]);
    }
    if !source.is_dir() {
        return Err(AppError::Other(
            "Skill 安装来源必须是目录或 SKILL.md".into(),
        ));
    }
    if source.join(SKILL_ENTRY_FILE).is_file() {
        return Ok(vec![source]);
    }

    let mut directories = Vec::new();
    for entry in WalkDir::new(&source)
        .max_depth(6)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend)
    {
        let entry = entry.map_err(|error| AppError::Other(format!("扫描 Skill 失败：{error}")))?;
        if entry.file_type().is_file() && entry.file_name() == SKILL_ENTRY_FILE {
            if let Some(parent) = entry.path().parent() {
                directories.push(parent.to_path_buf());
            }
        }
    }
    directories.sort();
    directories.dedup();
    if directories.is_empty() {
        return Err(AppError::Other("所选来源中没有找到 SKILL.md".into()));
    }
    if directories.len() > MAX_SKILLS_PER_INSTALL {
        return Err(AppError::Other(format!(
            "单次最多安装 {MAX_SKILLS_PER_INSTALL} 个 Skills"
        )));
    }
    Ok(directories)
}

fn safe_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn copy_skill_directory(source: &Path, destination: &Path) -> AppResult<(usize, u64, String)> {
    fs::create_dir_all(destination)?;
    let mut file_count = 0_usize;
    let mut total_bytes = 0_u64;
    let mut digest = Sha256::new();

    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend)
    {
        let entry = entry.map_err(|error| AppError::Other(format!("复制 Skill 失败：{error}")))?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| AppError::Other("Skill 路径越界".into()))?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        if !safe_relative_path(relative) {
            return Err(AppError::Other("Skill 包含不安全的相对路径".into()));
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::Other(format!(
                "Skill 不允许包含符号链接：{}",
                relative.display()
            )));
        }
        if relative.file_name().and_then(|value| value.to_str()) == Some(SKILL_METADATA_FILE) {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| AppError::Other(format!("读取 Skill 文件信息失败：{error}")))?;
        if metadata.len() > MAX_SKILL_FILE_BYTES {
            return Err(AppError::Other(format!(
                "Skill 文件超过 {} MiB：{}",
                MAX_SKILL_FILE_BYTES / 1024 / 1024,
                relative.display()
            )));
        }
        file_count += 1;
        total_bytes = total_bytes.saturating_add(metadata.len());
        if file_count > MAX_SKILL_FILES || total_bytes > MAX_SKILL_TOTAL_BYTES {
            return Err(AppError::Other(format!(
                "Skill 超过安装上限（最多 {MAX_SKILL_FILES} 个文件、{} MiB）",
                MAX_SKILL_TOTAL_BYTES / 1024 / 1024
            )));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = fs::read(entry.path())?;
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(&bytes);
        fs::write(&target, bytes)?;
        fs::set_permissions(&target, metadata.permissions())?;
    }
    Ok((file_count, total_bytes, format!("{:x}", digest.finalize())))
}

fn write_metadata(directory: &Path, metadata: &InstalledSkill) -> AppResult<()> {
    let temporary = directory.join(format!("{SKILL_METADATA_FILE}.tmp"));
    let target = directory.join(SKILL_METADATA_FILE);
    let backup = directory.join(format!("{SKILL_METADATA_FILE}.backup"));
    fs::write(&temporary, serde_json::to_vec_pretty(metadata)?)?;
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    if target.exists() {
        fs::rename(&target, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        if backup.exists() {
            let _ = fs::rename(&backup, &target);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

fn read_metadata(directory: &Path) -> AppResult<InstalledSkill> {
    let bytes = fs::read(directory.join(SKILL_METADATA_FILE))?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

fn install_one(directory: &Path, source: &SkillSource, root: &Path) -> AppResult<InstalledSkill> {
    let skill_md = directory.join(SKILL_ENTRY_FILE);
    let content = fs::read_to_string(&skill_md)
        .map_err(|error| AppError::Other(format!("无法读取 {}：{error}", skill_md.display())))?;
    let parsed = parse_skill_markdown(&content)?;
    let id = skill_id(&parsed.name);
    validate_skill_id(&id)?;
    let destination = root.join(&id);
    let existing = destination
        .is_dir()
        .then(|| read_metadata(&destination))
        .transpose()?;
    if let Some(existing) = existing.as_ref() {
        if existing.name != parsed.name {
            return Err(AppError::Other(format!(
                "Skill 名称 `{}` 与已安装的 `{}` 生成了相同标识 `{id}`",
                parsed.name, existing.name
            )));
        }
    }
    let staging = root.join(format!(".install-{id}-{}", uuid::Uuid::new_v4()));
    let backup = root.join(format!(".backup-{id}-{}", uuid::Uuid::new_v4()));
    let result = (|| -> AppResult<InstalledSkill> {
        let (file_count, total_bytes, content_hash) = copy_skill_directory(directory, &staging)?;
        let now = chrono::Utc::now().to_rfc3339();
        let metadata = InstalledSkill {
            id: id.clone(),
            name: parsed.name,
            description: parsed.description,
            version: parsed.version,
            author: parsed.author,
            enabled: existing.as_ref().map(|value| value.enabled).unwrap_or(true),
            source_kind: source.kind().into(),
            source_label: source.label().into(),
            installed_at: existing
                .as_ref()
                .map(|value| value.installed_at.clone())
                .unwrap_or_else(|| now.clone()),
            updated_at: now,
            file_count,
            total_bytes,
            content_hash,
        };
        write_metadata(&staging, &metadata)?;
        if destination.exists() {
            fs::rename(&destination, &backup)?;
        }
        if let Err(error) = fs::rename(&staging, &destination) {
            if backup.exists() {
                let _ = fs::rename(&backup, &destination);
            }
            return Err(error.into());
        }
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        Ok(metadata)
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn install_from_source(path: &Path, source: SkillSource) -> AppResult<Vec<InstalledSkill>> {
    let root = skills_root()?;
    fs::create_dir_all(&root)?;
    let directories = discover_skill_directories(path)?;
    let mut ids = HashSet::new();
    for directory in &directories {
        let content = fs::read_to_string(directory.join(SKILL_ENTRY_FILE))?;
        let parsed = parse_skill_markdown(&content)?;
        let id = skill_id(&parsed.name);
        if !ids.insert(id) {
            return Err(AppError::Other(format!(
                "安装来源包含重复的 Skill 名称：{}",
                parsed.name
            )));
        }
    }
    let mut installed = Vec::with_capacity(directories.len());
    for directory in directories {
        let skill = install_one(&directory, &source, &root)?;
        installed.push(skill);
    }
    Ok(installed)
}

pub fn install_from_path(path: String) -> AppResult<Vec<InstalledSkill>> {
    let _guard = skill_fs_lock()?;
    let canonical = PathBuf::from(&path).canonicalize()?;
    install_from_source(
        &canonical,
        SkillSource::Local(canonical.to_string_lossy().to_string()),
    )
}

pub async fn install_from_git(url: String) -> AppResult<Vec<InstalledSkill>> {
    let parsed =
        reqwest::Url::parse(url.trim()).map_err(|_| AppError::Other("Git 仓库地址无效".into()))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(AppError::Other(
            "仅支持不含内嵌凭证的 HTTPS Git 仓库地址".into(),
        ));
    }
    let root = skills_root()?;
    fs::create_dir_all(&root)?;
    let checkout = root.join(format!(".git-install-{}", uuid::Uuid::new_v4()));
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "-c",
            "protocol.file.allow=never",
            "clone",
            "--depth=1",
            "--filter=blob:none",
            "--",
            parsed.as_str(),
        ])
        .arg(&checkout)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let output = match tokio::time::timeout(Duration::from_secs(60), command.output()).await {
        Ok(result) => result?,
        Err(_) => {
            let _ = fs::remove_dir_all(&checkout);
            return Err(AppError::Other("克隆 Skill 仓库超时".into()));
        }
    };
    if !output.status.success() {
        let _ = fs::remove_dir_all(&checkout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Other(format!(
            "克隆 Skill 仓库失败：{}",
            stderr.trim().chars().take(2_000).collect::<String>()
        )));
    }
    let source = SkillSource::Git(parsed.to_string());
    let checkout_for_install = checkout.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _guard = skill_fs_lock()?;
        install_from_source(&checkout_for_install, source)
    })
    .await
    .map_err(|error| AppError::Other(format!("Skill 安装任务异常中止：{error}")))?;
    let _ = fs::remove_dir_all(&checkout);
    result
}

pub fn list_installed() -> AppResult<Vec<InstalledSkill>> {
    let _guard = skill_fs_lock()?;
    let root = skills_root()?;
    list_installed_at(&root)
}

fn list_installed_at(root: &Path) -> AppResult<Vec<InstalledSkill>> {
    fs::create_dir_all(&root)?;
    let mut skills = Vec::new();
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let directory = entry.path();
        if !directory.join(SKILL_METADATA_FILE).is_file()
            || !directory.join(SKILL_ENTRY_FILE).is_file()
        {
            continue;
        }
        skills.push(read_metadata(&directory)?);
    }
    skills.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    Ok(skills)
}

pub fn get_installed(id: &str) -> AppResult<InstalledSkill> {
    let _guard = skill_fs_lock()?;
    get_installed_at(&skills_root()?, id)
}

fn get_installed_at(root: &Path, id: &str) -> AppResult<InstalledSkill> {
    validate_skill_id(id)?;
    let directory = root.join(id);
    if !directory.is_dir() {
        return Err(AppError::Other("Skill 未安装".into()));
    }
    let metadata = read_metadata(&directory)?;
    if metadata.id != id {
        return Err(AppError::Other("Skill 元数据标识不匹配".into()));
    }
    Ok(metadata)
}

pub fn set_enabled(id: &str, enabled: bool) -> AppResult<InstalledSkill> {
    let _guard = skill_fs_lock()?;
    let root = skills_root()?;
    let mut metadata = get_installed_at(&root, id)?;
    metadata.enabled = enabled;
    metadata.updated_at = chrono::Utc::now().to_rfc3339();
    write_metadata(&root.join(id), &metadata)?;
    Ok(metadata)
}

pub fn uninstall(id: &str) -> AppResult<()> {
    let _guard = skill_fs_lock()?;
    let root = skills_root()?;
    let metadata = get_installed_at(&root, id)?;
    let source = root.join(&metadata.id);
    let trash = root.join(".trash");
    fs::create_dir_all(&trash)?;
    let destination = trash.join(format!(
        "{}-{}",
        metadata.id,
        chrono::Utc::now().timestamp_millis()
    ));
    fs::rename(source, destination)?;
    Ok(())
}

pub fn open_directory(id: &str) -> AppResult<()> {
    let directory = {
        let _guard = skill_fs_lock()?;
        let root = skills_root()?;
        let metadata = get_installed_at(&root, id)?;
        root.join(metadata.id)
    };
    open::that(directory).map_err(|error| AppError::Other(format!("无法打开 Skill 目录：{error}")))
}

pub fn load_for_prompt(id: &str) -> AppResult<SkillPromptContext> {
    let _guard = skill_fs_lock()?;
    load_for_prompt_at(&skills_root()?, id)
}

fn load_for_prompt_at(skills_root: &Path, id: &str) -> AppResult<SkillPromptContext> {
    let metadata = get_installed_at(skills_root, id)?;
    if !metadata.enabled {
        return Err(AppError::Other(format!("Skill 已停用：{}", metadata.name)));
    }
    let root = skills_root.join(&metadata.id).canonicalize()?;
    let content = fs::read_to_string(root.join(SKILL_ENTRY_FILE))?;
    let parsed = parse_skill_markdown(&content)?;
    let mut resources = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_descend)
    {
        let entry =
            entry.map_err(|error| AppError::Other(format!("读取 Skill 资源失败：{error}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
        let file_name = relative.file_name().and_then(|value| value.to_str());
        if file_name == Some(SKILL_ENTRY_FILE) || file_name == Some(SKILL_METADATA_FILE) {
            continue;
        }
        resources.push(relative.to_string_lossy().to_string());
        if resources.len() >= 100 {
            break;
        }
    }
    Ok(SkillPromptContext {
        id: metadata.id,
        name: metadata.name,
        description: metadata.description,
        instructions: parsed.instructions,
        root_path: root.to_string_lossy().to_string(),
        resources,
    })
}

pub fn selected_skill_roots(parts: &[crate::db::repo::messages::MessagePartRow]) -> Vec<PathBuf> {
    let Ok(_guard) = skill_fs_lock() else {
        return Vec::new();
    };
    let Ok(root) = skills_root() else {
        return Vec::new();
    };
    selected_skill_roots_at(parts, &root)
}

fn selected_skill_roots_at(
    parts: &[crate::db::repo::messages::MessagePartRow],
    root: &Path,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for part in parts.iter().filter(|part| part.kind == "attachment") {
        let Some(metadata) = part
            .metadata
            .as_deref()
            .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        else {
            continue;
        };
        if metadata
            .get("attachmentKind")
            .and_then(serde_json::Value::as_str)
            != Some("skill")
        {
            continue;
        }
        let Some(id) = metadata.get("skillId").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Ok(context) = load_for_prompt_at(root, id) {
            roots.push(PathBuf::from(context.root_path));
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_skill_frontmatter_and_body() {
        let parsed = parse_skill_markdown(
            "---\nname: document-review\ndescription: |\n  Review documents carefully.\n  Cite exact sections.\nversion: 1.2.0\nauthor: Agnes\n---\n# Workflow\n\nRead the input first.\n",
        )
        .unwrap();
        assert_eq!(parsed.name, "document-review");
        assert_eq!(
            parsed.description,
            "Review documents carefully.\nCite exact sections."
        );
        assert_eq!(parsed.version.as_deref(), Some("1.2.0"));
        assert_eq!(parsed.author.as_deref(), Some("Agnes"));
        assert!(parsed.instructions.starts_with("# Workflow"));
    }

    #[test]
    fn rejects_skill_without_required_metadata() {
        let error = parse_skill_markdown("---\nname: incomplete\n---\nDo work").unwrap_err();
        assert!(error.to_string().contains("description"));
    }

    #[tokio::test]
    async fn git_install_rejects_non_https_and_embedded_credentials() {
        assert!(install_from_git("file:///tmp/skills".into()).await.is_err());
        assert!(
            install_from_git("https://user:secret@example.com/skills.git".into())
                .await
                .is_err()
        );
    }

    #[test]
    fn discovers_nested_skill_directories_without_git_metadata() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("skills/one")).unwrap();
        fs::create_dir_all(root.path().join(".git/hidden")).unwrap();
        fs::write(
            root.path().join("skills/one/SKILL.md"),
            "---\nname: one\ndescription: First skill\n---\nUse it.",
        )
        .unwrap();
        fs::write(
            root.path().join(".git/hidden/SKILL.md"),
            "---\nname: hidden\ndescription: Hidden\n---\nIgnore.",
        )
        .unwrap();
        let found = discover_skill_directories(root.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("skills/one"));
    }

    #[test]
    fn installs_and_loads_a_skill_with_resources() {
        let source = tempfile::tempdir().unwrap();
        let installed_root = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("references")).unwrap();
        fs::write(
            source.path().join(SKILL_ENTRY_FILE),
            "---\nname: document-review\ndescription: Review documents with citations.\nversion: 1.0.0\n---\n# Workflow\n\nRead references/guide.md first.",
        )
        .unwrap();
        fs::write(
            source.path().join("references/guide.md"),
            "Always cite the section heading.",
        )
        .unwrap();

        let installed = install_one(
            source.path(),
            &SkillSource::Local(source.path().display().to_string()),
            installed_root.path(),
        )
        .unwrap();
        assert_eq!(installed.id, "document-review");
        assert_eq!(installed.file_count, 2);
        assert!(installed_root
            .path()
            .join("document-review/references/guide.md")
            .is_file());

        let prompt = load_for_prompt_at(installed_root.path(), "document-review").unwrap();
        assert!(prompt
            .instructions
            .contains("Read references/guide.md first."));
        assert_eq!(prompt.resources, vec!["references/guide.md"]);
        assert!(Path::new(&prompt.root_path).is_absolute());

        let mut disabled = installed.clone();
        disabled.enabled = false;
        write_metadata(&installed_root.path().join("document-review"), &disabled).unwrap();
        fs::write(
            source.path().join(SKILL_ENTRY_FILE),
            "---\nname: document-review\ndescription: Review documents with citations.\nversion: 1.1.0\n---\n# Workflow\n\nUse the updated workflow.",
        )
        .unwrap();
        let updated = install_one(
            source.path(),
            &SkillSource::Local(source.path().display().to_string()),
            installed_root.path(),
        )
        .unwrap();
        assert!(!updated.enabled);
        assert_eq!(updated.installed_at, installed.installed_at);
        assert_eq!(updated.version.as_deref(), Some("1.1.0"));

        let attachment = crate::db::repo::messages::MessagePartRow {
            id: "part-1".into(),
            message_id: "message-1".into(),
            kind: "attachment".into(),
            ordinal: 1,
            mime_type: Some("application/x-agnes-skill".into()),
            tool_call_id: None,
            content: String::new(),
            metadata: Some(
                serde_json::json!({
                    "attachmentKind": "skill",
                    "skillId": "document-review",
                })
                .to_string(),
            ),
        };
        assert!(selected_skill_roots_at(&[attachment], installed_root.path()).is_empty());

        let mut enabled = updated;
        enabled.enabled = true;
        write_metadata(&installed_root.path().join("document-review"), &enabled).unwrap();
        let roots = selected_skill_roots_at(
            &[crate::db::repo::messages::MessagePartRow {
                id: "part-2".into(),
                message_id: "message-1".into(),
                kind: "attachment".into(),
                ordinal: 1,
                mime_type: Some("application/x-agnes-skill".into()),
                tool_call_id: None,
                content: String::new(),
                metadata: Some(
                    serde_json::json!({
                        "attachmentKind": "skill",
                        "skillId": "document-review",
                    })
                    .to_string(),
                ),
            }],
            installed_root.path(),
        );
        assert_eq!(roots.len(), 1);
        assert!(roots[0].ends_with("document-review"));
    }
}
