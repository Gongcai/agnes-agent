use std::path::Path;
#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::AppHandle;
#[cfg(not(debug_assertions))]
use tauri_plugin_shell::{process::CommandEvent, ShellExt};

use crate::db::repo::knowledge::{DocumentParserProfile, NewDocumentChunk};
use crate::error::{AppError, AppResult};

const DOCUMENT_PARSER_TIMEOUT: Duration = Duration::from_secs(180);
const DOCUMENT_PARSER_SCHEMA_VERSION: u32 = 1;
const MAX_SOURCE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_PARSER_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOCUMENT_CHUNKS: usize = 100_000;
const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;

fn safe_profile_value(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[derive(Debug, Deserialize)]
struct ParserErrorResponse {
    error: String,
}

#[derive(Debug, Deserialize)]
pub struct ParsedDocument {
    schema_version: u32,
    pub title: String,
    pub media_type: String,
    pub source_hash: String,
    pub size: i64,
    pub parser_profile: DocumentParserProfileDto,
    pub chunks: Vec<ParsedDocumentChunk>,
}

#[derive(Debug, Deserialize)]
pub struct DocumentParserProfileDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub options_hash: String,
}

#[derive(Debug, Deserialize)]
pub struct ParsedDocumentChunk {
    pub content: String,
    pub page: Option<i64>,
    pub section_path: Option<String>,
    pub token_count: i64,
    pub metadata: serde_json::Value,
}

impl ParsedDocument {
    pub fn validate(self) -> AppResult<Self> {
        if self.schema_version != DOCUMENT_PARSER_SCHEMA_VERSION {
            return Err(AppError::Other(format!(
                "不支持的文档解析协议版本：{}",
                self.schema_version
            )));
        }
        if self.title.trim().is_empty()
            || self.title.len() > 4_096
            || self.media_type.trim().is_empty()
            || self.media_type.len() > 256
            || self.source_hash.len() != 64
            || !self
                .source_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.size <= 0
            || !safe_profile_value(&self.parser_profile.id, 128)
            || !safe_profile_value(&self.parser_profile.name, 128)
            || !safe_profile_value(&self.parser_profile.version, 64)
            || self.parser_profile.options_hash.len() != 64
            || !self
                .parser_profile
                .options_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.chunks.is_empty()
            || self.chunks.len() > MAX_DOCUMENT_CHUNKS
        {
            return Err(AppError::Other("文档解析器返回了无效结果".into()));
        }
        if self.chunks.iter().any(|chunk| {
            chunk.content.trim().is_empty()
                || chunk.content.len() > MAX_CHUNK_BYTES
                || chunk.token_count <= 0
                || chunk.page.is_some_and(|page| page <= 0)
                || chunk
                    .section_path
                    .as_ref()
                    .is_some_and(|section| section.len() > 4_096)
                || !chunk.metadata.is_object()
        }) {
            return Err(AppError::Other("文档解析器返回了无效切块".into()));
        }
        Ok(self)
    }

    pub fn profile(&self) -> DocumentParserProfile {
        DocumentParserProfile {
            id: self.parser_profile.id.clone(),
            name: self.parser_profile.name.clone(),
            version: self.parser_profile.version.clone(),
            options_hash: self.parser_profile.options_hash.clone(),
        }
    }

    pub fn into_chunks(self) -> AppResult<Vec<NewDocumentChunk>> {
        self.chunks
            .into_iter()
            .map(|chunk| {
                Ok(NewDocumentChunk {
                    content: chunk.content,
                    page: chunk.page,
                    section_path: chunk.section_path,
                    token_count: chunk.token_count,
                    metadata: serde_json::to_string(&chunk.metadata)?,
                })
            })
            .collect()
    }
}

fn parser_arguments(path: &Path, title_hint: Option<&str>, media_type: &str) -> Vec<String> {
    let mut arguments = vec!["--path".into(), path.to_string_lossy().into_owned()];
    arguments.push("--media-type".into());
    arguments.push(media_type.into());
    if let Some(title) = title_hint.map(str::trim).filter(|title| !title.is_empty()) {
        arguments.push("--title".into());
        arguments.push(title.into());
    }
    arguments
}

#[cfg(debug_assertions)]
async fn run_parser(
    _app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
) -> AppResult<(i32, Vec<u8>, Vec<u8>)> {
    let mut command = tokio::process::Command::new("uv");
    command
        .args(["run", "python", "document_parserd.py"])
        .args(parser_arguments(path, title_hint, media_type))
        .current_dir(resolve_document_parser_dir())
        .kill_on_drop(true);
    let output = tokio::time::timeout(DOCUMENT_PARSER_TIMEOUT, command.output())
        .await
        .map_err(|_| AppError::Other("文档解析超过 180 秒，已终止".into()))?
        .map_err(|error| {
            AppError::Other(format!(
                "启动文档解析器失败：{error}（确认 uv 在 PATH 且 document-parser/ 存在）"
            ))
        })?;
    if output.stdout.len() > MAX_PARSER_OUTPUT_BYTES {
        return Err(AppError::Other("文档解析结果超过 64 MiB 上限".into()));
    }
    Ok((
        output.status.code().unwrap_or(-1),
        output.stdout,
        output.stderr,
    ))
}

#[cfg(not(debug_assertions))]
async fn run_parser(
    app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
) -> AppResult<(i32, Vec<u8>, Vec<u8>)> {
    let command = app_handle
        .shell()
        .sidecar("document-parserd")
        .map_err(|error| AppError::Other(format!("解析内置文档解析器失败：{error}")))?
        .args(parser_arguments(path, title_hint, media_type));
    let (mut events, child) = command
        .spawn()
        .map_err(|error| AppError::Other(format!("启动内置文档解析器失败：{error}")))?;
    let wait_for_exit = async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(bytes) => stdout.extend(bytes),
                CommandEvent::Stderr(bytes) => stderr.extend(bytes),
                CommandEvent::Error(error) => {
                    stderr.extend_from_slice(error.as_bytes());
                    stderr.push(b'\n');
                }
                CommandEvent::Terminated(payload) => {
                    return Ok::<_, AppError>((payload.code.unwrap_or(-1), stdout, stderr));
                }
                _ => {}
            }
            if stdout.len() > MAX_PARSER_OUTPUT_BYTES {
                return Err(AppError::Other("文档解析结果超过 64 MiB 上限".into()));
            }
        }
        Err(AppError::Other(
            "文档解析器在返回退出状态前关闭了输出通道".into(),
        ))
    };
    match tokio::time::timeout(DOCUMENT_PARSER_TIMEOUT, wait_for_exit).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => {
            let _ = child.kill();
            Err(error)
        }
        Err(_) => {
            let _ = child.kill();
            Err(AppError::Other("文档解析超过 180 秒，已终止".into()))
        }
    }
}

pub async fn parse_office_document(
    app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
) -> AppResult<ParsedDocument> {
    let path = std::fs::canonicalize(path)?;
    let metadata = tokio::fs::metadata(&path).await?;
    if !metadata.is_file() {
        return Err(AppError::Other("知识库导入路径必须是普通文件".into()));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(AppError::Other("文件超过 50 MiB 的 Office 导入上限".into()));
    }
    let source = tokio::fs::read(&path).await?;
    let expected_size =
        i64::try_from(source.len()).map_err(|_| AppError::Other("文档大小超过支持范围".into()))?;
    let expected_hash = format!("{:x}", Sha256::digest(&source));
    drop(source);
    let (status, stdout, stderr) = run_parser(app_handle, &path, title_hint, media_type).await?;
    if status != 0 {
        if let Ok(response) = serde_json::from_slice::<ParserErrorResponse>(&stdout) {
            return Err(AppError::Other(response.error));
        }
        let details = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(AppError::Other(if details.is_empty() {
            format!("文档解析器异常退出（状态码 {status}）")
        } else {
            format!("文档解析器异常退出：{details}")
        }));
    }
    let parsed = serde_json::from_slice::<ParsedDocument>(&stdout)
        .map_err(|error| AppError::Other(format!("无法读取文档解析结果：{error}")))?
        .validate()?;
    if parsed.media_type != media_type
        || parsed.size != expected_size
        || parsed.source_hash != expected_hash
    {
        return Err(AppError::Other(
            "文档解析结果与源文件或请求格式不一致".into(),
        ));
    }
    Ok(parsed)
}

#[cfg(debug_assertions)]
fn resolve_document_parser_dir() -> PathBuf {
    if Path::new("document-parser").exists() {
        PathBuf::from("document-parser")
    } else {
        PathBuf::from("../document-parser")
    }
}

#[cfg(test)]
mod tests {
    use super::ParsedDocument;

    #[test]
    fn rejects_unknown_parser_protocol_versions() {
        let parsed: ParsedDocument = serde_json::from_value(serde_json::json!({
            "schema_version": 2,
            "title": "Report",
            "media_type": "application/test",
            "source_hash": "a".repeat(64),
            "size": 10,
            "parser_profile": {
                "id": "parser-v1",
                "name": "parser",
                "version": "1",
                "options_hash": "a".repeat(64)
            },
            "chunks": [{
                "content": "content",
                "page": null,
                "section_path": null,
                "token_count": 1,
                "metadata": {}
            }]
        }))
        .unwrap();
        assert!(parsed.validate().is_err());
    }
}
