use std::collections::HashMap;
use std::path::Path;
#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Runtime};
#[cfg(not(debug_assertions))]
use tauri_plugin_shell::{process::CommandEvent, ShellExt};
use tokio::sync::watch;

use crate::db::repo::knowledge::{DocumentParserProfile, NewDocumentChunk};
use crate::error::{AppError, AppResult};

const DOCUMENT_PARSER_TIMEOUT: Duration = Duration::from_secs(180);
const PDF_PARSER_TIMEOUT: Duration = Duration::from_secs(600);
const DOCUMENT_PARSER_SCHEMA_VERSION: u32 = 1;
const MAX_SOURCE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_PARSER_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOCUMENT_CHUNKS: usize = 100_000;
const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;
const MAX_PARSER_STDERR_BYTES: u64 = 1024 * 1024;

#[derive(Default)]
pub struct DocumentParserManager {
    jobs: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl DocumentParserManager {
    pub fn register(&self, job_id: &str) -> AppResult<watch::Receiver<bool>> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| AppError::Other("文档解析任务状态不可用".into()))?;
        if jobs.contains_key(job_id) {
            return Err(AppError::Other("文档导入任务 ID 已在使用".into()));
        }
        let (sender, receiver) = watch::channel(false);
        jobs.insert(job_id.to_string(), sender);
        Ok(receiver)
    }

    pub fn finish(&self, job_id: &str) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.remove(job_id);
        }
    }

    pub fn cancel(&self, job_id: &str) -> AppResult<()> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| AppError::Other("文档解析任务状态不可用".into()))?;
        if let Some(sender) = jobs.get(job_id) {
            let _ = sender.send(true);
        }
        Ok(())
    }
}

pub fn detached_cancellation() -> watch::Receiver<bool> {
    static SENDER: OnceLock<watch::Sender<bool>> = OnceLock::new();
    SENDER.get_or_init(|| watch::channel(false).0).subscribe()
}

#[derive(Clone)]
pub struct DocumentImportContext {
    pub job_id: String,
    pub file_name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentImportProgress {
    job_id: String,
    file_name: String,
    stage: String,
    percent: u8,
    message: String,
}

pub fn emit_import_progress<R: Runtime>(
    app_handle: &AppHandle<R>,
    context: Option<&DocumentImportContext>,
    stage: &str,
    percent: u8,
    message: &str,
) {
    let Some(context) = context else {
        return;
    };
    let _ = app_handle.emit(
        "knowledge://import-progress",
        DocumentImportProgress {
            job_id: context.job_id.clone(),
            file_name: context.file_name.clone(),
            stage: stage.to_string(),
            percent,
            message: message.to_string(),
        },
    );
}

pub fn ensure_not_cancelled(cancellation: &watch::Receiver<bool>) -> AppResult<()> {
    if *cancellation.borrow() {
        Err(AppError::Other("文档导入已取消".into()))
    } else {
        Ok(())
    }
}

fn safe_profile_value(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ParserMessage {
    Progress {
        stage: String,
        percent: u8,
        message: String,
    },
    Result {
        payload: ParsedDocument,
    },
    Error {
        error: String,
    },
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

fn parser_arguments(
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
    artifacts_path: Option<&Path>,
) -> Vec<String> {
    let mut arguments = vec!["--path".into(), path.to_string_lossy().into_owned()];
    arguments.push("--media-type".into());
    arguments.push(media_type.into());
    if let Some(title) = title_hint.map(str::trim).filter(|title| !title.is_empty()) {
        arguments.push("--title".into());
        arguments.push(title.into());
    }
    if let Some(artifacts_path) = artifacts_path {
        arguments.push("--artifacts-path".into());
        arguments.push(artifacts_path.to_string_lossy().into_owned());
    }
    arguments
}

#[derive(Default)]
struct ParserOutput {
    pending: Vec<u8>,
    total_bytes: usize,
    result: Option<ParsedDocument>,
    error: Option<String>,
}

impl ParserOutput {
    fn push<R: Runtime>(
        &mut self,
        bytes: &[u8],
        app_handle: &AppHandle<R>,
        context: Option<&DocumentImportContext>,
    ) -> AppResult<()> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| AppError::Other("文档解析结果超过 64 MiB 上限".into()))?;
        if self.total_bytes > MAX_PARSER_OUTPUT_BYTES {
            return Err(AppError::Other("文档解析结果超过 64 MiB 上限".into()));
        }
        self.pending.extend_from_slice(bytes);
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=position).collect::<Vec<_>>();
            self.process_line(&line[..line.len() - 1], app_handle, context)?;
        }
        Ok(())
    }

    fn finish<R: Runtime>(
        &mut self,
        app_handle: &AppHandle<R>,
        context: Option<&DocumentImportContext>,
    ) -> AppResult<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.process_line(&line, app_handle, context)?;
        }
        Ok(())
    }

    fn process_line<R: Runtime>(
        &mut self,
        line: &[u8],
        app_handle: &AppHandle<R>,
        context: Option<&DocumentImportContext>,
    ) -> AppResult<()> {
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let message = serde_json::from_slice::<ParserMessage>(line)
            .map_err(|error| AppError::Other(format!("无法读取文档解析事件：{error}")))?;
        match message {
            ParserMessage::Progress {
                stage,
                percent,
                message,
            } => {
                if stage.is_empty()
                    || stage.len() > 64
                    || !stage
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                    || percent > 100
                    || message.is_empty()
                    || message.len() > 512
                {
                    return Err(AppError::Other("文档解析器返回了无效进度".into()));
                }
                emit_import_progress(app_handle, context, &stage, percent, &message);
            }
            ParserMessage::Result { payload } => {
                if self.result.replace(payload).is_some() {
                    return Err(AppError::Other("文档解析器返回了多个结果".into()));
                }
            }
            ParserMessage::Error { error } => {
                if error.trim().is_empty() || error.len() > 4_096 {
                    return Err(AppError::Other("文档解析器返回了无效错误".into()));
                }
                self.error = Some(error);
            }
        }
        Ok(())
    }
}

async fn run_tokio_parser(
    app_handle: &AppHandle,
    mut command: tokio::process::Command,
    timeout_duration: Duration,
    startup_error_hint: &str,
    context: Option<&DocumentImportContext>,
    cancellation: &mut watch::Receiver<bool>,
) -> AppResult<(i32, ParserOutput, Vec<u8>)> {
    use tokio::io::{AsyncReadExt, BufReader};
    ensure_not_cancelled(cancellation)?;
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| {
        AppError::Other(format!(
            "启动文档解析器失败：{error}（{startup_error_hint}）"
        ))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Other("无法读取文档解析器输出".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Other("无法读取文档解析器错误输出".into()))?;
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(MAX_PARSER_STDERR_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map(|_| bytes)
    });
    let mut stdout = BufReader::new(stdout);
    let mut output = ParserOutput::default();
    let mut buffer = [0_u8; 16 * 1024];
    let mut stdout_open = true;
    let timeout = tokio::time::sleep(timeout_duration);
    tokio::pin!(timeout);
    let status = loop {
        tokio::select! {
            read = stdout.read(&mut buffer), if stdout_open => {
                let count = read.map_err(|error| AppError::Other(format!("读取文档解析器输出失败：{error}")))?;
                if count == 0 {
                    stdout_open = false;
                } else if let Err(error) = output.push(&buffer[..count], app_handle, context) {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(error);
                }
            }
            status = child.wait() => {
                let status = status.map_err(|error| AppError::Other(format!("等待文档解析器退出失败：{error}")))?;
                if stdout_open {
                    let mut remaining = Vec::new();
                    stdout.read_to_end(&mut remaining).await.map_err(|error| AppError::Other(format!("读取文档解析器输出失败：{error}")))?;
                    output.push(&remaining, app_handle, context)?;
                }
                break status.code().unwrap_or(-1);
            }
            changed = cancellation.changed() => {
                if changed.is_ok() && *cancellation.borrow() {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(AppError::Other("文档导入已取消".into()));
                }
            }
            _ = &mut timeout => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(AppError::Other(format!(
                    "文档解析超过 {} 秒，已终止",
                    timeout_duration.as_secs()
                )));
            }
        }
    };
    output.finish(app_handle, context)?;
    let stderr = stderr_task
        .await
        .map_err(|error| AppError::Other(format!("文档解析器错误输出任务异常中止：{error}")))??;
    Ok((status, output, stderr))
}

#[cfg(debug_assertions)]
fn development_pdf_paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let project_root = if Path::new("pdf-parser").exists() {
        PathBuf::from(".")
    } else if Path::new("../pdf-parser").exists() {
        PathBuf::from("..")
    } else {
        return None;
    };
    let pdf_project = project_root.join("pdf-parser");
    let parser_entry = project_root
        .join("document-parser")
        .join("document_parserd.py");
    let models = pdf_project.join(".models");
    if parser_entry.is_file() && models.is_dir() {
        Some((pdf_project, parser_entry, models))
    } else {
        None
    }
}

async fn run_pdf_parser(
    app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
    runtime: Option<&crate::pdf_models::PdfParserRuntime>,
    context: Option<&DocumentImportContext>,
    cancellation: &mut watch::Receiver<bool>,
) -> AppResult<(i32, ParserOutput, Vec<u8>)> {
    if let Some(runtime) = runtime {
        let mut command = tokio::process::Command::new(&runtime.executable);
        command.args(parser_arguments(
            path,
            title_hint,
            media_type,
            Some(&runtime.artifacts_path),
        ));
        return run_tokio_parser(
            app_handle,
            command,
            PDF_PARSER_TIMEOUT,
            "请重新安装 PDF 模型包",
            context,
            cancellation,
        )
        .await;
    }
    #[cfg(debug_assertions)]
    if let Some((pdf_project, parser_entry, models)) = development_pdf_paths() {
        let mut command = tokio::process::Command::new("uv");
        command
            .args(["run", "--project"])
            .arg(&pdf_project)
            .arg("python")
            .arg(&parser_entry)
            .args(parser_arguments(
                path,
                title_hint,
                media_type,
                Some(&models),
            ));
        return run_tokio_parser(
            app_handle,
            command,
            PDF_PARSER_TIMEOUT,
            "确认 uv、pdf-parser/ 和本地模型存在",
            context,
            cancellation,
        )
        .await;
    }
    Err(AppError::Other(
        "PDF 解析模型包尚未安装，请先在设置中安装".into(),
    ))
}

#[cfg(debug_assertions)]
async fn run_office_parser(
    app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
    context: Option<&DocumentImportContext>,
    cancellation: &mut watch::Receiver<bool>,
) -> AppResult<(i32, ParserOutput, Vec<u8>)> {
    let mut command = tokio::process::Command::new("uv");
    command
        .args(["run", "python", "document_parserd.py"])
        .args(parser_arguments(path, title_hint, media_type, None))
        .current_dir(resolve_document_parser_dir());
    run_tokio_parser(
        app_handle,
        command,
        DOCUMENT_PARSER_TIMEOUT,
        "确认 uv 在 PATH 且 document-parser/ 存在",
        context,
        cancellation,
    )
    .await
}

#[cfg(not(debug_assertions))]
async fn run_office_parser(
    app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
    context: Option<&DocumentImportContext>,
    cancellation: &mut watch::Receiver<bool>,
) -> AppResult<(i32, ParserOutput, Vec<u8>)> {
    ensure_not_cancelled(cancellation)?;
    let command = app_handle
        .shell()
        .sidecar("document-parserd")
        .map_err(|error| AppError::Other(format!("解析内置文档解析器失败：{error}")))?
        .args(parser_arguments(path, title_hint, media_type, None));
    let (mut events, child) = command
        .spawn()
        .map_err(|error| AppError::Other(format!("启动内置文档解析器失败：{error}")))?;
    let mut child = Some(child);
    let mut output = ParserOutput::default();
    let wait_for_exit = async {
        let mut stderr = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                CommandEvent::Stdout(bytes) => output.push(&bytes, app_handle, context)?,
                CommandEvent::Stderr(bytes) => {
                    if stderr.len() < MAX_PARSER_STDERR_BYTES as usize {
                        let remaining = MAX_PARSER_STDERR_BYTES as usize - stderr.len();
                        stderr.extend(bytes.into_iter().take(remaining));
                    }
                }
                CommandEvent::Error(error) => {
                    if stderr.len() < MAX_PARSER_STDERR_BYTES as usize {
                        stderr.extend_from_slice(error.as_bytes());
                        stderr.push(b'\n');
                    }
                }
                CommandEvent::Terminated(payload) => {
                    output.finish(app_handle, context)?;
                    return Ok::<_, AppError>((payload.code.unwrap_or(-1), output, stderr));
                }
                _ => {}
            }
        }
        Err(AppError::Other(
            "文档解析器在返回退出状态前关闭了输出通道".into(),
        ))
    };
    tokio::select! {
        result = wait_for_exit => {
            if result.is_err() {
                if let Some(child) = child.take() {
                    let _ = child.kill();
                }
            }
            result
        }
        changed = cancellation.changed() => {
            if changed.is_ok() && *cancellation.borrow() {
                if let Some(child) = child.take() {
                    let _ = child.kill();
                }
                Err(AppError::Other("文档导入已取消".into()))
            } else {
                Err(AppError::Other("文档解析取消通道异常关闭".into()))
            }
        }
        _ = tokio::time::sleep(DOCUMENT_PARSER_TIMEOUT) => {
            if let Some(child) = child.take() {
                let _ = child.kill();
            }
            Err(AppError::Other("文档解析超过 180 秒，已终止".into()))
        }
    }
}

pub async fn parse_structured_document(
    app_handle: &AppHandle,
    path: &Path,
    title_hint: Option<&str>,
    media_type: &str,
    pdf_runtime: Option<&crate::pdf_models::PdfParserRuntime>,
    context: Option<&DocumentImportContext>,
    mut cancellation: watch::Receiver<bool>,
) -> AppResult<ParsedDocument> {
    ensure_not_cancelled(&cancellation)?;
    let path = std::fs::canonicalize(path)?;
    let metadata = tokio::fs::metadata(&path).await?;
    if !metadata.is_file() {
        return Err(AppError::Other("知识库导入路径必须是普通文件".into()));
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(AppError::Other("文件超过 50 MiB 的文档导入上限".into()));
    }
    let source = tokio::fs::read(&path).await?;
    let expected_size =
        i64::try_from(source.len()).map_err(|_| AppError::Other("文档大小超过支持范围".into()))?;
    let expected_hash = format!("{:x}", Sha256::digest(&source));
    drop(source);
    let (status, mut output, stderr) = if media_type == "application/pdf" {
        run_pdf_parser(
            app_handle,
            &path,
            title_hint,
            media_type,
            pdf_runtime,
            context,
            &mut cancellation,
        )
        .await?
    } else {
        run_office_parser(
            app_handle,
            &path,
            title_hint,
            media_type,
            context,
            &mut cancellation,
        )
        .await?
    };
    if status != 0 {
        if let Some(error) = output.error {
            return Err(AppError::Other(error));
        }
        let details = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(AppError::Other(if details.is_empty() {
            format!("文档解析器异常退出（状态码 {status}）")
        } else {
            format!("文档解析器异常退出：{details}")
        }));
    }
    ensure_not_cancelled(&cancellation)?;
    let parsed = output
        .result
        .take()
        .ok_or_else(|| AppError::Other("文档解析器未返回结果".into()))?
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
    use super::{DocumentParserManager, ParsedDocument, ParserOutput};

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

    #[test]
    fn manager_delivers_cancellation_and_releases_job_ids() {
        let manager = DocumentParserManager::default();
        let mut cancellation = manager.register("job-1").unwrap();
        assert!(manager.register("job-1").is_err());

        manager.cancel("job-1").unwrap();
        assert!(cancellation.has_changed().unwrap());
        assert!(*cancellation.borrow_and_update());

        manager.finish("job-1");
        assert!(manager.register("job-1").is_ok());
    }

    #[test]
    fn parser_output_accepts_fragmented_jsonl_messages() {
        let app = tauri::test::mock_app();
        let mut output = ParserOutput::default();
        let messages = concat!(
            "{\"type\":\"progress\",\"stage\":\"validating\",\"percent\":10,\"message\":\"正在检查文档\"}\n",
            "{\"type\":\"result\",\"payload\":{\"schema_version\":1,\"title\":\"Report\",\"media_type\":\"application/test\",\"source_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"size\":10,\"parser_profile\":{\"id\":\"parser-v1\",\"name\":\"parser\",\"version\":\"1\",\"options_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"chunks\":[{\"content\":\"content\",\"page\":null,\"section_path\":null,\"token_count\":1,\"metadata\":{}}]}}\n"
        );
        let split = messages.len() / 3;

        output
            .push(&messages.as_bytes()[..split], app.handle(), None)
            .unwrap();
        output
            .push(&messages.as_bytes()[split..], app.handle(), None)
            .unwrap();
        output.finish(app.handle(), None).unwrap();

        assert!(output.error.is_none());
        assert!(output.result.take().unwrap().validate().is_ok());
    }
}
