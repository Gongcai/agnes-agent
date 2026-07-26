use std::collections::HashSet;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::user_profile::UserProfileInheritanceMode;

pub const MAX_MEMORY_FILES: i64 = 256;
pub const MAX_MEMORY_FILE_BYTES: usize = 16 * 1024;
pub const MAX_MEMORY_READ_PATHS: usize = 20;
pub const MAX_MEMORY_READ_BYTES: usize = 256 * 1024;
const MAX_DESCRIPTION_CHARS: usize = 160;
const MAX_ALIAS_CHARS: usize = 80;
const MAX_ALIASES: usize = 7;
const MAX_PROFILE_WORDS: usize = 300;
pub const USER_MD_PATH: &str = "/USER.md";
const USER_MD_DESCRIPTION: &str =
    "User-managed read-only background information for every conversation";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFileRow {
    pub id: String,
    pub agent_id: String,
    pub path: String,
    pub kind: String,
    pub name: String,
    pub description: String,
    pub sources: Vec<String>,
    pub aliases: Vec<String>,
    pub markdown: String,
    pub content_hash: String,
    pub version: i64,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub origin_device_id: Option<String>,
}

impl MemoryFileRow {
    pub fn version_token(&self) -> String {
        version_token(self.version, &self.content_hash)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryListingItem {
    pub path: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryReadResult {
    pub path: String,
    pub exists: bool,
    pub content: Option<String>,
    pub version: Option<String>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryListing {
    pub files: Vec<MemoryListingItem>,
    pub listing_epoch: i64,
}

#[derive(Debug, Clone)]
pub struct MemoryPromptSnapshot {
    pub user_md: AgentUserMdState,
    pub base_profile_json: Option<String>,
    pub profile: Option<String>,
    pub preferences: Option<String>,
    pub listing: MemoryListing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUserMdState {
    pub content: String,
    pub mode: UserProfileInheritanceMode,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMutationOutcome {
    pub ok: bool,
    pub code: Option<String>,
    pub message: String,
    pub path: String,
    pub version: Option<String>,
    pub current_content: Option<String>,
    pub current_version: Option<String>,
    pub listing_epoch: i64,
    pub summary: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MemoryMutation {
    Write {
        content: String,
        if_version: String,
    },
    Replace {
        old_str: String,
        new_str: String,
        if_version: String,
    },
    Append {
        content: String,
        if_version: String,
    },
    Delete {
        if_version: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryFrontmatter {
    name: String,
    description: String,
    sources: Vec<String>,
    aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inherit: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedMemoryFile {
    kind: String,
    is_user_md: bool,
    frontmatter: MemoryFrontmatter,
    facts: Vec<String>,
}

pub(crate) struct ValidatedSyncMemoryFile {
    pub kind: String,
    pub name: String,
    pub description: String,
    pub sources: Vec<String>,
    pub aliases: Vec<String>,
    pub markdown: String,
    pub content_hash: String,
}

#[derive(Debug, Clone)]
struct DomainError {
    code: &'static str,
    message: String,
}

impl DomainError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn timestamp() -> String {
    Utc::now().to_rfc3339()
}

fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

pub fn sync_entity_id(agent_id: &str, path: &str) -> AppResult<String> {
    parse_path(path).map_err(|error| AppError::Other(error.message))?;
    if agent_id.trim().is_empty() {
        return Err(AppError::Other("Memory agent id cannot be empty".into()));
    }
    let identity = format!("memory://{agent_id}{path}");
    Ok(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, identity.as_bytes()).to_string())
}

fn version_token(version: i64, hash: &str) -> String {
    let prefix = hash.get(..16).unwrap_or(hash);
    format!("v{version}:{prefix}")
}

fn parse_path(path: &str) -> Result<(String, String), DomainError> {
    if path.contains('\0')
        || path.contains('\\')
        || path.contains("//")
        || path.contains("/./")
        || path.contains("..")
        || path.contains('%')
    {
        return Err(DomainError::new("INVALID_PATH", "Invalid memory path"));
    }
    let (kind, stem) = match path {
        USER_MD_PATH => ("profile", "USER"),
        "/profile.md" => ("profile", "profile"),
        "/preferences.md" => ("preferences", "preferences"),
        _ => {
            let without_suffix = path
                .strip_suffix(".md")
                .ok_or_else(|| DomainError::new("INVALID_PATH", "Memory paths must end in .md"))?;
            let mut parts = without_suffix.split('/');
            if parts.next() != Some("") {
                return Err(DomainError::new(
                    "INVALID_PATH",
                    "Memory paths must be absolute virtual paths",
                ));
            }
            let directory = parts.next().unwrap_or_default();
            let stem = parts.next().unwrap_or_default();
            if parts.next().is_some() || stem.is_empty() || !valid_slug(stem) {
                return Err(DomainError::new("INVALID_PATH", "Invalid memory path slug"));
            }
            let kind = match directory {
                "topics" => "topic",
                "areas" => "area",
                "people" => "person",
                _ => {
                    return Err(DomainError::new(
                        "INVALID_PATH",
                        "Memory path must be profile, preferences, topics, areas, or people",
                    ))
                }
            };
            (kind, stem)
        }
    };
    Ok((kind.to_string(), stem.to_string()))
}

fn valid_slug(value: &str) -> bool {
    if value.starts_with('-') || value.ends_with('-') || value.contains("--") {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn split_document(content: &str) -> Result<(String, String), DomainError> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let rest = normalized.strip_prefix("---\n").ok_or_else(|| {
        DomainError::new(
            "INVALID_FRONTMATTER",
            "Memory files must start with YAML frontmatter",
        )
    })?;
    let marker = rest.find("\n---\n").ok_or_else(|| {
        DomainError::new(
            "INVALID_FRONTMATTER",
            "Memory frontmatter must end with ---",
        )
    })?;
    let yaml = rest[..marker].to_string();
    let body = rest[marker + 5..].trim().to_string();
    Ok((yaml, body))
}

fn parse_document(path: &str, content: &str) -> Result<ParsedMemoryFile, DomainError> {
    if content.as_bytes().len() > MAX_MEMORY_FILE_BYTES {
        return Err(DomainError::new(
            "SIZE_LIMIT",
            format!("Memory file exceeds {MAX_MEMORY_FILE_BYTES} bytes"),
        ));
    }
    let (kind, stem) = parse_path(path)?;
    let (yaml, body) = split_document(content)?;
    if yaml.contains('&') || yaml.contains('*') || yaml.contains('!') {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "YAML anchors, aliases, and custom tags are not allowed",
        ));
    }
    let frontmatter: MemoryFrontmatter = serde_yaml::from_str(&yaml).map_err(|error| {
        DomainError::new(
            "INVALID_FRONTMATTER",
            format!("Invalid frontmatter: {error}"),
        )
    })?;
    if frontmatter.name != stem {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Frontmatter name must match the path stem",
        ));
    }
    let is_user_md = path == USER_MD_PATH;
    if is_user_md {
        let mode = frontmatter.inherit.as_deref().ok_or_else(|| {
            DomainError::new("INVALID_FRONTMATTER", "USER.md requires an inherit mode")
        })?;
        UserProfileInheritanceMode::parse(mode)
            .map_err(|error| DomainError::new("INVALID_FRONTMATTER", error.to_string()))?;
    } else if frontmatter.inherit.is_some() {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Only USER.md may define an inherit mode",
        ));
    }
    if frontmatter.description.trim().is_empty()
        || frontmatter.description.contains(['\r', '\n'])
        || frontmatter.description.chars().count() > MAX_DESCRIPTION_CHARS
    {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Description must be one non-empty line of at most 160 characters",
        ));
    }
    if frontmatter.sources.is_empty() {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "At least one memory source is required",
        ));
    }
    let allowed_sources = ["chat", "user"];
    if frontmatter
        .sources
        .iter()
        .any(|source| !allowed_sources.contains(&source.as_str()))
    {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Memory sources contain an unsupported surface",
        ));
    }
    if !matches!(kind.as_str(), "area" | "person") && !frontmatter.aliases.is_empty() {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Aliases are only allowed for area and person files",
        ));
    }
    if frontmatter.aliases.len() > MAX_ALIASES
        || frontmatter.aliases.iter().any(|alias| {
            alias.trim().is_empty()
                || alias.contains(['\r', '\n'])
                || alias.chars().count() > MAX_ALIAS_CHARS
        })
    {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Aliases must contain at most 7 non-empty single-line values",
        ));
    }
    ensure_unique(&frontmatter.sources, "sources")?;
    ensure_unique(&frontmatter.aliases, "aliases")?;

    let facts = if body.is_empty() {
        Vec::new()
    } else {
        body.lines().map(str::to_string).collect::<Vec<_>>()
    };
    if !is_user_md {
        for fact in &facts {
            if fact.trim().is_empty()
                || !(fact.starts_with("- [stated] ")
                    || fact.starts_with("- [observed] ")
                    || fact.starts_with("- [inferred] "))
            {
                return Err(DomainError::new(
                    "INVALID_FACT_TAG",
                    "Every memory body line must be a tagged fact bullet",
                ));
            }
        }
    }
    if path == "/profile.md"
        && facts
            .iter()
            .flat_map(|line| line.split_whitespace())
            .count()
            > MAX_PROFILE_WORDS
    {
        return Err(DomainError::new(
            "SIZE_LIMIT",
            "Profile body exceeds 300 words",
        ));
    }
    Ok(ParsedMemoryFile {
        kind,
        is_user_md,
        frontmatter,
        facts,
    })
}

fn ensure_unique(values: &[String], field: &str) -> Result<(), DomainError> {
    let mut seen = HashSet::new();
    if values
        .iter()
        .any(|value| !seen.insert(value.trim().to_lowercase()))
    {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            format!("Frontmatter {field} values must be unique"),
        ));
    }
    Ok(())
}

fn normalize_for_surface(
    mut parsed: ParsedMemoryFile,
    previous: Option<&ParsedMemoryFile>,
    actor_surface: &str,
) -> Result<ParsedMemoryFile, DomainError> {
    if !matches!(actor_surface, "chat" | "user" | "sync") {
        return Err(DomainError::new(
            "INVALID_FRONTMATTER",
            "Unsupported memory actor surface",
        ));
    }
    if parsed.is_user_md && actor_surface == "chat" {
        return Err(DomainError::new(
            "READ_ONLY",
            "USER.md is managed by the user and is read-only for chat",
        ));
    }
    if let Some(previous) = previous {
        if previous.frontmatter.sources.iter().any(|source| {
            !parsed
                .frontmatter
                .sources
                .iter()
                .any(|candidate| candidate == source)
        }) {
            return Err(DomainError::new(
                "SOURCE_REMOVAL_FORBIDDEN",
                "Existing memory sources cannot be removed",
            ));
        }
        if !parsed.is_user_md && matches!(actor_surface, "chat" | "user") {
            for fact in &parsed.facts {
                if (fact.starts_with("- [observed] ") || fact.starts_with("- [inferred] "))
                    && !previous.facts.contains(fact)
                {
                    return Err(DomainError::new(
                        "INVALID_FACT_TAG",
                        "Chat and user surfaces may preserve but cannot create observed or inferred facts",
                    ));
                }
            }

            if previous.facts.iter().any(|fact| {
                (fact.starts_with("- [observed] ") || fact.starts_with("- [inferred] "))
                    && !parsed.facts.contains(fact)
            }) {
                return Err(DomainError::new(
                    "INVALID_FACT_TAG",
                    "Chat and user surfaces must preserve observed and inferred facts unchanged",
                ));
            }
        }
        if actor_surface != "sync" {
            parsed.frontmatter.sources = previous.frontmatter.sources.clone();
        }
    } else if !parsed.is_user_md
        && matches!(actor_surface, "chat" | "user")
        && parsed
            .facts
            .iter()
            .any(|fact| !fact.starts_with("- [stated] "))
    {
        return Err(DomainError::new(
            "INVALID_FACT_TAG",
            "Chat and user surfaces may only create stated facts",
        ));
    }
    let required_source = (actor_surface != "sync").then_some(actor_surface);
    if let Some(required_source) = required_source {
        if previous.is_none() {
            parsed.frontmatter.sources.clear();
        }
        if !parsed
            .frontmatter
            .sources
            .iter()
            .any(|source| source == required_source)
        {
            parsed.frontmatter.sources.push(required_source.to_string());
        }
    }
    parsed.frontmatter.description = parsed.frontmatter.description.trim().to_string();
    parsed.frontmatter.sources.sort();
    parsed.frontmatter.sources.dedup();
    parsed.frontmatter.aliases = parsed
        .frontmatter
        .aliases
        .into_iter()
        .map(|alias| alias.trim().to_string())
        .collect();
    Ok(parsed)
}

fn render_document(parsed: &ParsedMemoryFile) -> Result<String, DomainError> {
    let yaml = serde_yaml::to_string(&parsed.frontmatter).map_err(|error| {
        DomainError::new(
            "INVALID_FRONTMATTER",
            format!("Unable to serialize frontmatter: {error}"),
        )
    })?;
    let yaml = yaml.trim_start_matches("---\n").trim_end();
    let body = parsed.facts.join("\n");
    let content = if body.is_empty() {
        format!("---\n{yaml}\n---\n")
    } else {
        format!("---\n{yaml}\n---\n{body}\n")
    };
    if content.as_bytes().len() > MAX_MEMORY_FILE_BYTES {
        return Err(DomainError::new(
            "SIZE_LIMIT",
            format!("Memory file exceeds {MAX_MEMORY_FILE_BYTES} bytes"),
        ));
    }
    Ok(content)
}

pub(crate) fn validate_synced_document(
    path: &str,
    content: &str,
) -> AppResult<ValidatedSyncMemoryFile> {
    let parsed = parse_document(path, content)
        .and_then(|parsed| normalize_for_surface(parsed, None, "sync"))
        .map_err(|error| AppError::Other(error.message))?;
    let markdown = render_document(&parsed).map_err(|error| AppError::Other(error.message))?;
    Ok(ValidatedSyncMemoryFile {
        kind: parsed.kind,
        name: parsed.frontmatter.name,
        description: parsed.frontmatter.description,
        sources: parsed.frontmatter.sources,
        aliases: parsed.frontmatter.aliases,
        content_hash: content_hash(&markdown),
        markdown,
    })
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryFileRow> {
    let sources_json: String = row.get(6)?;
    let aliases_json: String = row.get(7)?;
    Ok(MemoryFileRow {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        path: row.get(2)?,
        kind: row.get(3)?,
        name: row.get(4)?,
        description: row.get(5)?,
        sources: serde_json::from_str(&sources_json).unwrap_or_default(),
        aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
        markdown: row.get(8)?,
        content_hash: row.get(9)?,
        version: row.get(10)?,
        status: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        deleted_at: row.get(14)?,
        origin_device_id: row.get(15)?,
    })
}

const SELECT_COLUMNS: &str = "id, agent_id, path, kind, name, description, sources_json, \
aliases_json, markdown, content_hash, version, status, created_at, updated_at, deleted_at, \
origin_device_id";

pub fn list(conn: &Connection, agent_id: &str) -> AppResult<Vec<MemoryFileRow>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {SELECT_COLUMNS} FROM memory_files \
         WHERE agent_id = ?1 AND path <> '/USER.md' \
         AND status = 'active' AND deleted_at IS NULL ORDER BY path"
    ))?;
    let rows = statement.query_map([agent_id], row_from_sql)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get(conn: &Connection, agent_id: &str, path: &str) -> AppResult<Option<MemoryFileRow>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM memory_files \
                 WHERE agent_id = ?1 AND path = ?2 AND status = 'active' AND deleted_at IS NULL"
            ),
            params![agent_id, path],
            row_from_sql,
        )
        .optional()?)
}

fn get_any(conn: &Connection, agent_id: &str, path: &str) -> AppResult<Option<MemoryFileRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SELECT_COLUMNS} FROM memory_files WHERE agent_id = ?1 AND path = ?2"),
            params![agent_id, path],
            row_from_sql,
        )
        .optional()?)
}

fn listing_epoch(conn: &Connection, agent_id: &str) -> AppResult<i64> {
    Ok(conn
        .query_row(
            "SELECT epoch FROM memory_listing_state WHERE agent_id = ?1",
            [agent_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn listing(conn: &Connection, agent_id: &str) -> AppResult<MemoryListing> {
    let files = list(conn, agent_id)?
        .into_iter()
        .map(|row| MemoryListingItem {
            path: row.path,
            description: row.description,
            aliases: row.aliases,
            sources: row.sources,
        })
        .collect();
    Ok(MemoryListing {
        files,
        listing_epoch: listing_epoch(conn, agent_id)?,
    })
}

fn prompt_body(conn: &Connection, agent_id: &str, path: &str) -> AppResult<Option<String>> {
    let Some(file) = get(conn, agent_id, path)? else {
        return Ok(None);
    };
    let parsed = parse_document(path, &file.markdown).map_err(|error| {
        AppError::Other(format!("Stored memory file is invalid: {}", error.message))
    })?;
    let body = parsed.facts.join("\n");
    Ok((!body.is_empty()).then_some(body))
}

pub fn get_agent_user_md(conn: &Connection, agent_id: &str) -> AppResult<AgentUserMdState> {
    let Some(file) = get(conn, agent_id, USER_MD_PATH)? else {
        return Ok(AgentUserMdState {
            content: String::new(),
            mode: UserProfileInheritanceMode::Auto,
            version: None,
        });
    };
    let parsed = parse_document(USER_MD_PATH, &file.markdown).map_err(|error| {
        AppError::Other(format!("Stored USER.md is invalid: {}", error.message))
    })?;
    let mode =
        UserProfileInheritanceMode::parse(parsed.frontmatter.inherit.as_deref().unwrap_or("auto"))?;
    Ok(AgentUserMdState {
        content: parsed.facts.join("\n"),
        mode,
        version: Some(file.version_token()),
    })
}

pub fn build_agent_user_md(content: &str, mode: UserProfileInheritanceMode) -> AppResult<String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let body = normalized.trim();
    let parsed = ParsedMemoryFile {
        kind: "profile".to_string(),
        is_user_md: true,
        frontmatter: MemoryFrontmatter {
            name: "USER".to_string(),
            description: USER_MD_DESCRIPTION.to_string(),
            sources: vec!["user".to_string()],
            aliases: Vec::new(),
            inherit: Some(mode.as_str().to_string()),
        },
        facts: if body.is_empty() {
            Vec::new()
        } else {
            body.lines().map(str::to_string).collect()
        },
    };
    render_document(&parsed).map_err(|error| AppError::Other(error.message))
}

pub fn prompt_snapshot(conn: &Connection, agent_id: &str) -> AppResult<MemoryPromptSnapshot> {
    Ok(MemoryPromptSnapshot {
        user_md: get_agent_user_md(conn, agent_id)?,
        base_profile_json: super::settings::get(conn, crate::user_profile::USER_PROFILE_SETTING)?,
        profile: prompt_body(conn, agent_id, "/profile.md")?,
        preferences: prompt_body(conn, agent_id, "/preferences.md")?,
        listing: listing(conn, agent_id)?,
    })
}

pub fn read_many(
    conn: &Connection,
    agent_id: &str,
    paths: &[String],
) -> AppResult<Vec<MemoryReadResult>> {
    if paths.is_empty() || paths.len() > MAX_MEMORY_READ_PATHS {
        return Err(AppError::Other(format!(
            "memory_read accepts between 1 and {MAX_MEMORY_READ_PATHS} paths"
        )));
    }
    let mut seen = HashSet::new();
    let mut bytes = 0usize;
    let mut results = Vec::with_capacity(paths.len());
    for path in paths {
        parse_path(path).map_err(|error| AppError::Other(error.message))?;
        if !seen.insert(path) {
            return Err(AppError::Other("memory_read paths must be unique".into()));
        }
        let file = get(conn, agent_id, path)?;
        if let Some(file) = file {
            bytes += file.markdown.as_bytes().len();
            if bytes > MAX_MEMORY_READ_BYTES {
                return Err(AppError::Other(format!(
                    "memory_read result exceeds {MAX_MEMORY_READ_BYTES} bytes"
                )));
            }
            let version = file.version_token();
            results.push(MemoryReadResult {
                path: path.clone(),
                exists: true,
                content: Some(file.markdown),
                version: Some(version),
                content_hash: Some(file.content_hash),
            });
        } else {
            results.push(MemoryReadResult {
                path: path.clone(),
                exists: false,
                content: None,
                version: None,
                content_hash: None,
            });
        }
    }
    Ok(results)
}

fn current_or_failure(
    conn: &Connection,
    agent_id: &str,
    path: &str,
    code: &'static str,
    message: impl Into<String>,
) -> AppResult<MemoryMutationOutcome> {
    let current = get(conn, agent_id, path)?;
    Ok(MemoryMutationOutcome {
        ok: false,
        code: Some(code.to_string()),
        message: message.into(),
        path: path.to_string(),
        version: None,
        current_content: current.as_ref().map(|row| row.markdown.clone()),
        current_version: current.as_ref().map(MemoryFileRow::version_token),
        listing_epoch: listing_epoch(conn, agent_id)?,
        summary: None,
    })
}

fn bump_listing_epoch(tx: &Transaction<'_>, agent_id: &str) -> AppResult<i64> {
    tx.execute(
        "INSERT INTO memory_listing_state(agent_id, epoch) VALUES (?1, 1) \
         ON CONFLICT(agent_id) DO UPDATE SET epoch = epoch + 1",
        [agent_id],
    )?;
    Ok(tx.query_row(
        "SELECT epoch FROM memory_listing_state WHERE agent_id = ?1",
        [agent_id],
        |row| row.get(0),
    )?)
}

fn device_id(tx: &Transaction<'_>) -> String {
    tx.query_row(
        "SELECT device_id FROM sync_runtime_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )
    .unwrap_or_else(|_| "local".to_string())
}

fn validate_version(current: &MemoryFileRow, supplied: &str) -> Result<(), DomainError> {
    if supplied != current.version_token() {
        return Err(DomainError::new(
            "VERSION_CONFLICT",
            "The memory file changed after it was read",
        ));
    }
    Ok(())
}

pub fn mutate(
    conn: &mut Connection,
    agent_id: &str,
    path: &str,
    mutation: MemoryMutation,
    actor_surface: &str,
    session_id: Option<&str>,
    message_id: Option<&str>,
) -> AppResult<MemoryMutationOutcome> {
    if let Err(error) = parse_path(path) {
        return current_or_failure(conn, agent_id, path, error.code, error.message);
    }
    if path == USER_MD_PATH && actor_surface == "chat" {
        return current_or_failure(
            conn,
            agent_id,
            path,
            "READ_ONLY",
            "USER.md is managed by the user and is read-only for chat",
        );
    }
    let current = get_any(conn, agent_id, path)?;
    let active_current = current
        .as_ref()
        .filter(|row| row.status == "active" && row.deleted_at.is_none());

    if let MemoryMutation::Delete { if_version } = &mutation {
        let Some(current) = active_current else {
            return current_or_failure(conn, agent_id, path, "NOT_FOUND", "Memory file not found");
        };
        if let Err(error) = validate_version(current, if_version) {
            return current_or_failure(conn, agent_id, path, error.code, error.message);
        }
        let now = timestamp();
        let tx = conn.transaction()?;
        let next_version = current.version + 1;
        tx.execute(
            "UPDATE memory_files SET status = 'deleted', version = ?1, updated_at = ?2, \
             deleted_at = ?2, origin_device_id = ?3 WHERE id = ?4",
            params![next_version, now, device_id(&tx), current.id],
        )?;
        tx.execute(
            "INSERT INTO memory_file_revisions \
             (id, memory_file_id, base_version, resulting_version, operation, before_markdown, \
              after_markdown, actor_surface, session_id, message_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'delete', ?5, NULL, ?6, ?7, ?8, ?9)",
            params![
                uuid::Uuid::new_v4().to_string(),
                current.id,
                current.version,
                next_version,
                current.markdown,
                actor_surface,
                session_id,
                message_id,
                now,
            ],
        )?;
        let epoch = bump_listing_epoch(&tx, agent_id)?;
        super::sync::enqueue_projection(
            &tx,
            crate::sync::payload::SyncEntityType::MemoryFile,
            &current.id,
            next_version,
            true,
            &serde_json::json!({}),
        )?;
        tx.commit()?;
        return Ok(MemoryMutationOutcome {
            ok: true,
            code: None,
            message: "Memory file deleted".to_string(),
            path: path.to_string(),
            version: None,
            current_content: None,
            current_version: None,
            listing_epoch: epoch,
            summary: Some("Deleted memory file".to_string()),
        });
    }

    let (candidate, supplied_version, operation, summary) = match mutation {
        MemoryMutation::Write {
            content,
            if_version,
        } => (
            content,
            if_version,
            if active_current.is_some() {
                "write"
            } else {
                "create"
            },
            if active_current.is_some() {
                "Rewrote memory file"
            } else {
                "Created memory file"
            },
        ),
        MemoryMutation::Replace {
            old_str,
            new_str,
            if_version,
        } => {
            let Some(current) = active_current else {
                return current_or_failure(
                    conn,
                    agent_id,
                    path,
                    "NOT_FOUND",
                    "Memory file not found",
                );
            };
            if let Err(error) = validate_version(current, &if_version) {
                return current_or_failure(conn, agent_id, path, error.code, error.message);
            }
            let occurrences = current.markdown.match_indices(&old_str).count();
            if occurrences == 0 {
                return current_or_failure(
                    conn,
                    agent_id,
                    path,
                    "MATCH_NOT_FOUND",
                    "old_str was not found",
                );
            }
            if occurrences != 1 {
                return current_or_failure(
                    conn,
                    agent_id,
                    path,
                    "MATCH_AMBIGUOUS",
                    format!("old_str occurs {occurrences} times"),
                );
            }
            (
                current.markdown.replacen(&old_str, &new_str, 1),
                if_version,
                "replace",
                "Replaced one exact memory block",
            )
        }
        MemoryMutation::Append {
            content,
            if_version,
        } => {
            let Some(current) = active_current else {
                return current_or_failure(
                    conn,
                    agent_id,
                    path,
                    "NOT_FOUND",
                    "Memory file not found",
                );
            };
            if let Err(error) = validate_version(current, &if_version) {
                return current_or_failure(conn, agent_id, path, error.code, error.message);
            }
            let addition = content.replace("\r\n", "\n").replace('\r', "\n");
            let addition = addition.trim();
            if addition.is_empty() || addition.contains("---") {
                return current_or_failure(
                    conn,
                    agent_id,
                    path,
                    "INVALID_FACT_TAG",
                    "Append content must contain tagged fact lines only",
                );
            }
            if addition
                .lines()
                .any(|line| current.markdown.lines().any(|existing| existing == line))
            {
                return current_or_failure(
                    conn,
                    agent_id,
                    path,
                    "ALREADY_EXISTS",
                    "One or more fact lines already exist",
                );
            }
            (
                format!(
                    "{}{}\n",
                    current.markdown.trim_end(),
                    format!("\n{addition}")
                ),
                if_version,
                "append",
                "Appended memory facts",
            )
        }
        MemoryMutation::Delete { .. } => unreachable!(),
    };

    let creating = active_current.is_none();
    if creating && supplied_version != "new" {
        return current_or_failure(
            conn,
            agent_id,
            path,
            "NOT_FOUND",
            "New memory files require if_version=new",
        );
    }
    if !creating {
        if supplied_version == "new" {
            return current_or_failure(
                conn,
                agent_id,
                path,
                "ALREADY_EXISTS",
                "Memory file already exists",
            );
        }
        if let Err(error) = validate_version(active_current.unwrap(), &supplied_version) {
            return current_or_failure(conn, agent_id, path, error.code, error.message);
        }
    }

    let previous_parsed = match active_current {
        Some(current) => Some(parse_document(path, &current.markdown).map_err(|error| {
            AppError::Other(format!("Stored memory file is invalid: {}", error.message))
        })?),
        None => None,
    };
    let parsed = match parse_document(path, &candidate)
        .and_then(|parsed| normalize_for_surface(parsed, previous_parsed.as_ref(), actor_surface))
    {
        Ok(parsed) => parsed,
        Err(error) => {
            return current_or_failure(conn, agent_id, path, error.code, error.message);
        }
    };
    let canonical = match render_document(&parsed) {
        Ok(content) => content,
        Err(error) => {
            return current_or_failure(conn, agent_id, path, error.code, error.message);
        }
    };
    if active_current.is_none() {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_files WHERE agent_id = ?1 AND status = 'active' AND deleted_at IS NULL",
            [agent_id],
            |row| row.get(0),
        )?;
        if count >= MAX_MEMORY_FILES {
            return current_or_failure(
                conn,
                agent_id,
                path,
                "SIZE_LIMIT",
                format!("Agent already has {MAX_MEMORY_FILES} active memory files"),
            );
        }
    }
    let conflicting_name: Option<String> = conn
        .query_row(
            "SELECT path FROM memory_files WHERE agent_id = ?1 AND name = ?2 \
             AND status = 'active' AND deleted_at IS NULL AND path <> ?3",
            params![agent_id, parsed.frontmatter.name, path],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(conflicting_path) = conflicting_name {
        return current_or_failure(
            conn,
            agent_id,
            path,
            "NAME_CONFLICT",
            format!("Memory name is already used by {conflicting_path}"),
        );
    }
    if active_current.is_some_and(|current| current.markdown == canonical) {
        let current = active_current.unwrap();
        return Ok(MemoryMutationOutcome {
            ok: true,
            code: None,
            message: "Memory file is unchanged".to_string(),
            path: path.to_string(),
            version: Some(current.version_token()),
            current_content: None,
            current_version: None,
            listing_epoch: listing_epoch(conn, agent_id)?,
            summary: None,
        });
    }

    let now = timestamp();
    let hash = content_hash(&canonical);
    let tx = conn.transaction()?;
    let origin_device_id = device_id(&tx);
    let (id, base_version, next_version, before_markdown) = if let Some(current) = current {
        (
            current.id.clone(),
            Some(current.version),
            current.version + 1,
            Some(current.markdown.clone()),
        )
    } else {
        (sync_entity_id(agent_id, path)?, None, 1, None)
    };
    tx.execute(
        "INSERT INTO memory_files \
         (id, agent_id, path, kind, name, description, sources_json, aliases_json, markdown, \
          content_hash, version, status, created_at, updated_at, deleted_at, origin_device_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'active', ?12, ?12, NULL, ?13) \
         ON CONFLICT(agent_id, path) DO UPDATE SET kind=excluded.kind, name=excluded.name, \
          description=excluded.description, sources_json=excluded.sources_json, \
          aliases_json=excluded.aliases_json, markdown=excluded.markdown, \
          content_hash=excluded.content_hash, version=excluded.version, status='active', \
          updated_at=excluded.updated_at, deleted_at=NULL, origin_device_id=excluded.origin_device_id",
        params![
            id,
            agent_id,
            path,
            parsed.kind,
            parsed.frontmatter.name,
            parsed.frontmatter.description,
            serde_json::to_string(&parsed.frontmatter.sources)?,
            serde_json::to_string(&parsed.frontmatter.aliases)?,
            canonical,
            hash,
            next_version,
            now,
            origin_device_id,
        ],
    )?;
    tx.execute(
        "INSERT INTO memory_file_revisions \
         (id, memory_file_id, base_version, resulting_version, operation, before_markdown, \
          after_markdown, actor_surface, session_id, message_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            uuid::Uuid::new_v4().to_string(),
            id,
            base_version,
            next_version,
            operation,
            before_markdown,
            canonical,
            actor_surface,
            session_id,
            message_id,
            now,
        ],
    )?;
    let epoch = bump_listing_epoch(&tx, agent_id)?;
    let synced = get_any(&tx, agent_id, path)?
        .ok_or_else(|| AppError::Other("Memory file disappeared during sync enqueue".into()))?;
    let sync_source = serde_json::json!({
        "id": synced.id,
        "agent_id": synced.agent_id,
        "path": synced.path,
        "kind": synced.kind,
        "name": synced.name,
        "description": synced.description,
        "sources": synced.sources,
        "aliases": synced.aliases,
        "markdown": synced.markdown,
        "content_hash": synced.content_hash,
        "status": synced.status,
        "created_at": synced.created_at,
        "updated_at": synced.updated_at,
        "version": synced.version,
        "deleted_at": synced.deleted_at,
        "origin_device_id": synced.origin_device_id,
    });
    super::sync::enqueue_projection(
        &tx,
        crate::sync::payload::SyncEntityType::MemoryFile,
        &synced.id,
        synced.version,
        false,
        &sync_source,
    )?;
    tx.commit()?;
    Ok(MemoryMutationOutcome {
        ok: true,
        code: None,
        message: summary.to_string(),
        path: path.to_string(),
        version: Some(version_token(next_version, &hash)),
        current_content: None,
        current_version: None,
        listing_epoch: epoch,
        summary: Some(summary.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        conn
    }

    fn topic(content: &str) -> String {
        format!(
            "---\nname: food\ndescription: Food preferences to use for recommendations\nsources: [chat]\naliases: []\n---\n- [stated] {content}\n"
        )
    }

    #[test]
    fn creates_reads_and_updates_with_cas() {
        let mut conn = setup();
        let created = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Write {
                content: topic("likes noodles"),
                if_version: "new".into(),
            },
            "chat",
            Some("session-1"),
            None,
        )
        .unwrap();
        assert!(created.ok);
        let token = created.version.unwrap();
        let updated = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Append {
                content: "- [stated] prefers spicy broth".into(),
                if_version: token.clone(),
            },
            "chat",
            Some("session-1"),
            None,
        )
        .unwrap();
        assert!(updated.ok);
        let conflict = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Append {
                content: "- [stated] likes dumplings".into(),
                if_version: token,
            },
            "chat",
            None,
            None,
        )
        .unwrap();
        assert_eq!(conflict.code.as_deref(), Some("VERSION_CONFLICT"));
        assert!(conflict.current_content.unwrap().contains("spicy broth"));
    }

    #[test]
    fn rejects_invalid_paths_and_chat_inferences() {
        let mut conn = setup();
        let invalid = mutate(
            &mut conn,
            "agnes",
            "/topics/../secret.md",
            MemoryMutation::Write {
                content: topic("likes noodles"),
                if_version: "new".into(),
            },
            "chat",
            None,
            None,
        )
        .unwrap();
        assert_eq!(invalid.code.as_deref(), Some("INVALID_PATH"));
        let inferred = topic("likes noodles").replace("[stated]", "[inferred]");
        let rejected = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Write {
                content: inferred,
                if_version: "new".into(),
            },
            "chat",
            None,
            None,
        )
        .unwrap();
        assert_eq!(rejected.code.as_deref(), Some("INVALID_FACT_TAG"));
    }

    #[test]
    fn chat_and_user_surfaces_preserve_trusted_fact_tags() {
        let mut conn = setup();
        let synced = topic("likes noodles").replace(
            "- [stated] likes noodles",
            "- [observed] orders noodles\n- [stated] likes noodles",
        );
        let created = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Write {
                content: synced,
                if_version: "new".into(),
            },
            "sync",
            None,
            None,
        )
        .unwrap();
        let version = created.version.unwrap();

        for surface in ["chat", "user"] {
            let forged = topic("likes noodles").replace("[stated]", "[inferred]");
            let result = mutate(
                &mut conn,
                "agnes",
                "/topics/food.md",
                MemoryMutation::Write {
                    content: forged,
                    if_version: version.clone(),
                },
                surface,
                None,
                None,
            )
            .unwrap();
            assert_eq!(result.code.as_deref(), Some("INVALID_FACT_TAG"));
        }
    }

    #[test]
    fn memory_entity_id_is_stable_for_agent_and_path() {
        let first = sync_entity_id("agent-1", "/topics/food.md").unwrap();
        assert_eq!(first, sync_entity_id("agent-1", "/topics/food.md").unwrap());
        assert_ne!(first, sync_entity_id("agent-2", "/topics/food.md").unwrap());
        assert_ne!(
            first,
            sync_entity_id("agent-1", "/topics/travel.md").unwrap()
        );
    }

    #[test]
    fn user_md_is_user_managed_hidden_and_versioned() {
        let mut conn = setup();
        let content = "# USER.md\n\n- Works on Agnes\n- Prefers concise answers";
        let document = build_agent_user_md(content, UserProfileInheritanceMode::Inherit).unwrap();
        let created = mutate(
            &mut conn,
            "agnes",
            USER_MD_PATH,
            MemoryMutation::Write {
                content: document.clone(),
                if_version: "new".into(),
            },
            "user",
            None,
            None,
        )
        .unwrap();
        assert!(created.ok);

        let state = get_agent_user_md(&conn, "agnes").unwrap();
        assert_eq!(state.content, content);
        assert_eq!(state.mode, UserProfileInheritanceMode::Inherit);
        assert!(listing(&conn, "agnes").unwrap().files.is_empty());

        let rejected = mutate(
            &mut conn,
            "agnes",
            USER_MD_PATH,
            MemoryMutation::Write {
                content: document,
                if_version: state.version.clone().unwrap(),
            },
            "chat",
            Some("session-1"),
            None,
        )
        .unwrap();
        assert_eq!(rejected.code.as_deref(), Some("READ_ONLY"));

        let delete_rejected = mutate(
            &mut conn,
            "agnes",
            USER_MD_PATH,
            MemoryMutation::Delete {
                if_version: state.version.unwrap(),
            },
            "chat",
            Some("session-1"),
            None,
        )
        .unwrap();
        assert_eq!(delete_rejected.code.as_deref(), Some("READ_ONLY"));
    }

    #[test]
    fn actor_surface_owns_sources_and_mutation_enqueues_in_same_transaction() {
        let mut conn = setup();
        let forged = topic("likes noodles").replace("sources: [chat]", "sources: [user]");
        let created = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Write {
                content: forged,
                if_version: "new".into(),
            },
            "chat",
            None,
            None,
        )
        .unwrap();
        assert!(created.ok);
        let row = get(&conn, "agnes", "/topics/food.md").unwrap().unwrap();
        assert_eq!(row.sources, vec!["chat"]);
        let outbox: (String, String) = conn
            .query_row(
                "SELECT entity_type, source_payload FROM sync_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(outbox.0, "memory_file");
        assert!(outbox.1.contains("/topics/food.md"));
        assert!(!outbox.1.contains("embedding"));
    }

    #[test]
    fn memory_write_rolls_back_when_outbox_insert_fails() {
        let mut conn = setup();
        conn.execute_batch(
            "CREATE TRIGGER fail_memory_outbox BEFORE INSERT ON sync_outbox
             WHEN NEW.entity_type = 'memory_file'
             BEGIN SELECT RAISE(ABORT, 'outbox failed'); END;",
        )
        .unwrap();
        let result = mutate(
            &mut conn,
            "agnes",
            "/topics/food.md",
            MemoryMutation::Write {
                content: topic("likes noodles"),
                if_version: "new".into(),
            },
            "chat",
            None,
            None,
        );
        assert!(result.is_err());
        assert!(get(&conn, "agnes", "/topics/food.md").unwrap().is_none());
    }
}
