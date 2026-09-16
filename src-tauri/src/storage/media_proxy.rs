use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{stream, StreamExt};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell};

use crate::error::AppResult;

use super::domain::{
    DownloadFileRequest, MediaPlaybackLink, ProviderByteStream, ProviderError,
    ProviderErrorCategory,
};
use super::service::{StorageFileStreamMetadata, StorageService};

const PROXY_PATH_PREFIX: &str = "/storage/media/";
const TRANSCODE_PATH_PREFIX: &str = "/storage/transcode/";
const SESSION_TTL: Duration = Duration::from_secs(2 * 60 * 60);
const REQUEST_LIMIT: usize = 16 * 1024;
const UPSTREAM_PART_BYTES: u64 = 10 * 1024 * 1024;
const UPSTREAM_RETRIES: usize = 3;
const FFMPEG_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct PlaybackSession {
    account_id: String,
    file_id: String,
    expected_revision: Option<String>,
    metadata: StorageFileStreamMetadata,
    mode: PlaybackMode,
    expires_at: Instant,
}

#[derive(Clone)]
enum PlaybackMode {
    Direct,
    Transcode { source_token: String },
}

pub struct StorageMediaProxy {
    storage: Arc<StorageService>,
    address: OnceCell<Result<std::net::SocketAddr, String>>,
    sessions: Arc<Mutex<HashMap<String, PlaybackSession>>>,
}

impl StorageMediaProxy {
    pub fn new(storage: Arc<StorageService>) -> Self {
        Self {
            storage,
            address: OnceCell::new(),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn create_link(
        &self,
        account_id: String,
        file_id: String,
        file_name: Option<String>,
        file_media_type: Option<String>,
        file_size: Option<u64>,
    ) -> AppResult<MediaPlaybackLink> {
        let mut metadata = self
            .storage
            .get_file_stream_metadata(&account_id, &file_id)
            .await?;
        if let Some(file_name) = file_name.filter(|value| !value.trim().is_empty()) {
            metadata.name = file_name;
        }
        if let Some(file_media_type) = file_media_type.filter(|value| !value.trim().is_empty()) {
            metadata.media_type = Some(file_media_type);
        }
        if let Some(file_size) = file_size.filter(|value| *value > 0) {
            metadata.size = Some(file_size);
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        let source_token = uuid::Uuid::new_v4().simple().to_string();
        self.sessions.lock().await.insert(
            source_token.clone(),
            PlaybackSession {
                account_id,
                file_id,
                expected_revision: metadata.revision.clone(),
                metadata: metadata.clone(),
                mode: PlaybackMode::Direct,
                expires_at: Instant::now() + SESSION_TTL,
            },
        );
        let address = self
            .ensure_server()
            .await
            .map_err(crate::error::AppError::Other)?;
        if should_transcode(&metadata) {
            self.sessions.lock().await.insert(
                token.clone(),
                PlaybackSession {
                    account_id: String::new(),
                    file_id: String::new(),
                    expected_revision: None,
                    metadata: metadata.clone(),
                    mode: PlaybackMode::Transcode { source_token },
                    expires_at: Instant::now() + SESSION_TTL,
                },
            );
            return Ok(MediaPlaybackLink {
                url: format!("http://{address}{TRANSCODE_PATH_PREFIX}{token}"),
                media_type: Some("video/mp4".into()),
                protocol: "transcode".into(),
                resolution: None,
                variants: Vec::new(),
            });
        }
        Ok(MediaPlaybackLink {
            url: format!("http://{address}{PROXY_PATH_PREFIX}{source_token}"),
            media_type: metadata.media_type,
            protocol: "range".into(),
            resolution: None,
            variants: Vec::new(),
        })
    }

    async fn ensure_server(&self) -> Result<std::net::SocketAddr, String> {
        self.address
            .get_or_init(|| async {
                let listener = TcpListener::bind("127.0.0.1:0")
                    .await
                    .map_err(|error| format!("无法启动网盘媒体代理: {error}"))?;
                let address = listener
                    .local_addr()
                    .map_err(|error| format!("无法读取网盘媒体代理地址: {error}"))?;
                let storage = self.storage.clone();
                let sessions = self.sessions.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        let (stream, _) = match listener.accept().await {
                            Ok(connection) => connection,
                            Err(error) => {
                                eprintln!("[storage][media-proxy] accept failed: {error}");
                                continue;
                            }
                        };
                        let storage = storage.clone();
                        let sessions = sessions.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(error) =
                                handle_connection(stream, storage, sessions, address).await
                            {
                                if !error.is_empty() {
                                    eprintln!("[storage][media-proxy] {error}");
                                }
                            }
                        });
                    }
                });
                Ok(address)
            })
            .await
            .clone()
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    storage: Arc<StorageService>,
    sessions: Arc<Mutex<HashMap<String, PlaybackSession>>>,
    address: std::net::SocketAddr,
) -> Result<(), String> {
    let request = read_request(&mut stream).await?;
    let Some((method, target, headers)) = parse_request(&request) else {
        write_error(&mut stream, 400, "Bad Request").await?;
        return Ok(());
    };
    let request_path = target.split('?').next().unwrap_or("");
    let log_path = request_path
        .strip_prefix(PROXY_PATH_PREFIX)
        .map_or(request_path, |_| "<media-session>");
    eprintln!(
        "[storage][media-proxy] request method={method} path={log_path} range={}",
        headers.get("range").map(String::as_str).unwrap_or("none")
    );
    if method == "OPTIONS" {
        write_response(
            &mut stream,
            204,
            &[
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS"),
                ("Access-Control-Allow-Headers", "Range"),
                ("Access-Control-Max-Age", "600"),
            ],
        )
        .await?;
        return Ok(());
    }
    if method != "GET" && method != "HEAD" {
        write_error(&mut stream, 405, "Method Not Allowed").await?;
        return Ok(());
    }
    let path = target.split('?').next().unwrap_or_default();
    let (is_transcode, token) = if let Some(token) = path.strip_prefix(TRANSCODE_PATH_PREFIX) {
        (true, token)
    } else if let Some(token) = path.strip_prefix(PROXY_PATH_PREFIX) {
        (false, token)
    } else {
        write_error(&mut stream, 404, "Not Found").await?;
        return Ok(());
    };
    if token.is_empty() {
        write_error(&mut stream, 404, "Not Found").await?;
        return Ok(());
    }
    let session = {
        let mut values = sessions.lock().await;
        let now = Instant::now();
        values.retain(|_, value| value.expires_at > now);
        values.get(token).cloned()
    };
    let Some(session) = session else {
        write_error(&mut stream, 404, "Playback session expired").await?;
        return Ok(());
    };
    if is_transcode {
        return handle_transcoded_connection(&mut stream, &session, address, method == "HEAD")
            .await;
    }
    if !matches!(&session.mode, PlaybackMode::Direct) {
        write_error(&mut stream, 404, "Invalid playback session").await?;
        return Ok(());
    }
    let range = match parse_range(
        headers.get("range").map(String::as_str),
        session.metadata.size,
    ) {
        Ok(range) => range,
        Err(_) => {
            let total = session
                .metadata
                .size
                .map_or_else(|| "*".into(), |size| size.to_string());
            write_response(
                &mut stream,
                416,
                &[
                    ("Content-Range", format!("bytes */{total}")),
                    ("Access-Control-Allow-Origin", "*".into()),
                    ("Connection", "close".into()),
                ],
            )
            .await?;
            return Ok(());
        }
    };
    let range = resolve_open_ended_range(range, session.metadata.size);
    let (status, content_length, content_range) =
        response_range_headers(&range, session.metadata.size);
    let response_content_type = content_type(&session.metadata).to_owned();
    eprintln!(
        "[storage][media-proxy] response status={status} name={} type={response_content_type} size={} length={}",
        session.metadata.name,
        session
            .metadata
            .size
            .map_or_else(|| "unknown".into(), |value| value.to_string()),
        content_length.map_or_else(|| "unknown".into(), |value| value.to_string())
    );
    let initial_source = if method == "GET" && session.metadata.size != Some(0) {
        match open_initial_provider_stream(&storage, &session, range).await {
            Ok(source) => Some(source),
            Err(ForwardError::Upstream { error, .. }) => {
                eprintln!("[storage][media-proxy] upstream stream failed: {error}");
                write_error(&mut stream, 502, "Storage stream unavailable").await?;
                return Ok(());
            }
            Err(ForwardError::Client(error)) => return Err(error),
        }
    } else {
        None
    };
    let mut response_headers = vec![
        ("Accept-Ranges".to_owned(), "bytes".to_owned()),
        ("Access-Control-Allow-Origin".to_owned(), "*".to_owned()),
        ("Cache-Control".to_owned(), "no-store".to_owned()),
        ("Connection".to_owned(), "close".to_owned()),
        (
            "Content-Disposition".to_owned(),
            content_disposition(&session.metadata.name),
        ),
        ("Content-Type".to_owned(), response_content_type),
    ];
    if let Some(revision) = session.metadata.revision.as_deref() {
        response_headers.push(("ETag".to_owned(), format!("\"{revision}\"")));
    }
    if let Some(length) = content_length {
        response_headers.push(("Content-Length".to_owned(), length.to_string()));
    }
    if let Some(value) = content_range {
        response_headers.push(("Content-Range".to_owned(), value));
    }
    write_response_owned(&mut stream, status, &response_headers).await?;
    if method == "HEAD" {
        return Ok(());
    }
    if let Err(error) =
        forward_provider_ranges(&mut stream, &storage, &session, range, initial_source).await
    {
        match error {
            ForwardError::Upstream { error, .. } => {
                eprintln!("[storage][media-proxy] upstream stream failed: {error}");
            }
            ForwardError::Client(error) if !is_client_disconnect(&error) => {
                eprintln!("[storage][media-proxy] client stream failed: {error}");
            }
            ForwardError::Client(_) => {}
        }
    }
    Ok(())
}

async fn handle_transcoded_connection(
    stream: &mut TcpStream,
    session: &PlaybackSession,
    address: std::net::SocketAddr,
    head_only: bool,
) -> Result<(), String> {
    let PlaybackMode::Transcode { source_token } = &session.mode else {
        write_error(stream, 404, "Invalid transcoding session").await?;
        return Ok(());
    };
    let input_url = format!("http://{address}{PROXY_PATH_PREFIX}{source_token}");
    let headers = vec![
        ("Accept-Ranges".to_owned(), "none".to_owned()),
        ("Access-Control-Allow-Origin".to_owned(), "*".to_owned()),
        ("Cache-Control".to_owned(), "no-store".to_owned()),
        ("Connection".to_owned(), "close".to_owned()),
        (
            "Content-Disposition".to_owned(),
            content_disposition(&session.metadata.name),
        ),
        ("Content-Type".to_owned(), "video/mp4".to_owned()),
    ];
    if head_only {
        write_response_owned(stream, 200, &headers).await?;
        return Ok(());
    }
    let mode = select_ffmpeg_mode(&input_url).await;
    eprintln!(
        "[storage][media-proxy] ffmpeg mode={} name={}",
        mode.label(),
        session.metadata.name
    );
    let source = match spawn_ffmpeg_stream(&input_url, mode).await {
        Ok(source) => source,
        Err(error) => {
            eprintln!("[storage][media-proxy] ffmpeg startup failed: {error}");
            write_error(stream, 502, "FFmpeg transcoding unavailable").await?;
            return Ok(());
        }
    };
    write_response_owned(stream, 200, &headers).await?;
    if let Err(error) = forward_stream(stream, source, None).await {
        match error {
            ForwardError::Upstream { error, .. } => {
                eprintln!("[storage][media-proxy] ffmpeg stream failed: {error}");
            }
            ForwardError::Client(error) if !is_client_disconnect(&error) => {
                eprintln!("[storage][media-proxy] transcoded client stream failed: {error}");
            }
            ForwardError::Client(_) => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum FfmpegMode {
    Remux,
    Transcode,
}

impl FfmpegMode {
    fn label(self) -> &'static str {
        match self {
            Self::Remux => "remux",
            Self::Transcode => "transcode",
        }
    }
}

async fn select_ffmpeg_mode(input_url: &str) -> FfmpegMode {
    let mut probe = Command::new(ffprobe_program());
    probe
        .args(["-v", "error", "-show_streams", "-of", "json", input_url])
        .kill_on_drop(true);
    let output = tokio::time::timeout(FFMPEG_PROBE_TIMEOUT, probe.output())
        .await
        .ok()
        .and_then(Result::ok);
    let Some(output) = output.filter(|output| output.status.success()) else {
        return FfmpegMode::Transcode;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return FfmpegMode::Transcode;
    };
    if probe_supports_fragmented_mp4(&value) {
        FfmpegMode::Remux
    } else {
        FfmpegMode::Transcode
    }
}

fn probe_supports_fragmented_mp4(value: &Value) -> bool {
    let Some(streams) = value.get("streams").and_then(Value::as_array) else {
        return false;
    };
    let Some(video) = streams
        .iter()
        .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
    else {
        return false;
    };
    let video_codec_ok = video.get("codec_name").and_then(Value::as_str) == Some("h264")
        && matches!(
            video.get("pix_fmt").and_then(Value::as_str),
            Some("yuv420p" | "yuvj420p")
        );
    let audio_codec_ok = streams
        .iter()
        .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio"))
        .map_or(true, |stream| {
            stream.get("codec_name").and_then(Value::as_str) == Some("aac")
        });
    video_codec_ok && audio_codec_ok
}

async fn spawn_ffmpeg_stream(
    input_url: &str,
    mode: FfmpegMode,
) -> Result<ProviderByteStream, String> {
    let mut command = Command::new(ffmpeg_program());
    command
        .arg("-nostdin")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(input_url)
        .args(["-map", "0:v:0", "-map", "0:a:0?"])
        .args(["-sn", "-dn"])
        .args(match mode {
            FfmpegMode::Remux => &["-c", "copy"][..],
            FfmpegMode::Transcode => &["-c:v", "libx264"][..],
        })
        .args(match mode {
            FfmpegMode::Remux => &[][..],
            FfmpegMode::Transcode => &["-preset", "veryfast"][..],
        })
        .args(match mode {
            FfmpegMode::Remux => &[][..],
            FfmpegMode::Transcode => &["-tune", "zerolatency"][..],
        })
        .args(match mode {
            FfmpegMode::Remux => &[][..],
            FfmpegMode::Transcode => &["-crf", "23", "-pix_fmt", "yuv420p"][..],
        })
        .args(match mode {
            FfmpegMode::Remux => &[][..],
            FfmpegMode::Transcode => &["-c:a", "aac", "-b:a", "160k"][..],
        })
        .args([
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof",
            "-flush_packets",
            "1",
            "-f",
            "mp4",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("{}: {error}", ffmpeg_program()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "FFmpeg stdout was not piped".to_owned())?;
    if let Some(mut stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut output = Vec::new();
            let _ = stderr.read_to_end(&mut output).await;
            if !output.is_empty() {
                eprintln!(
                    "[storage][media-proxy][ffmpeg] {}",
                    String::from_utf8_lossy(&output).trim()
                );
            }
        });
    }
    Ok(Box::pin(stream::unfold(
        (child, stdout),
        |(mut child, mut stdout)| async move {
            let mut buffer = vec![0_u8; 64 * 1024];
            match stdout.read(&mut buffer).await {
                Ok(0) => {
                    let status = child.wait().await;
                    if status.as_ref().is_ok_and(|status| !status.success()) {
                        return Some((
                            Err(ProviderError::new(
                                ProviderErrorCategory::RemoteUnavailable,
                                "FFmpeg exited before producing the complete video stream",
                            )),
                            (child, stdout),
                        ));
                    }
                    None
                }
                Ok(length) => {
                    buffer.truncate(length);
                    Some((Ok(buffer), (child, stdout)))
                }
                Err(error) => Some((
                    Err(ProviderError::new(
                        ProviderErrorCategory::Network,
                        format!("读取 FFmpeg 输出失败: {error}"),
                    )),
                    (child, stdout),
                )),
            }
        },
    )))
}

fn ffmpeg_program() -> String {
    std::env::var("AGNES_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}

fn ffprobe_program() -> String {
    std::env::var("AGNES_FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".into())
}

fn should_transcode(metadata: &StorageFileStreamMetadata) -> bool {
    let media_type = metadata
        .media_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let extension = metadata
        .name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    media_type == "video/x-matroska"
        || matches!(
            extension.as_deref(),
            Some("mkv" | "avi" | "flv" | "ts" | "m2ts" | "mts" | "wmv" | "rm" | "rmvb")
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestRange {
    Full,
    Partial { start: u64, end: Option<u64> },
}

impl RequestRange {
    fn start(self) -> Option<u64> {
        match self {
            Self::Full => None,
            Self::Partial { start, .. } => Some(start),
        }
    }

    fn end(self) -> Option<u64> {
        match self {
            Self::Full => None,
            Self::Partial { end, .. } => end,
        }
    }
}

fn parse_range(value: Option<&str>, size: Option<u64>) -> Result<RequestRange, ()> {
    let Some(value) = value else {
        return Ok(RequestRange::Full);
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        let size = size.ok_or(())?;
        if suffix == 0 || size == 0 {
            return Err(());
        }
        return Ok(RequestRange::Partial {
            start: size.saturating_sub(suffix),
            end: Some(size - 1),
        });
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    let end = if end.is_empty() {
        None
    } else {
        Some(end.parse::<u64>().map_err(|_| ())?)
    };
    if end.is_some_and(|end| end < start) || size.is_some_and(|size| start >= size) {
        return Err(());
    }
    let end = match (end, size) {
        (Some(end), Some(size)) => Some(end.min(size.saturating_sub(1))),
        (Some(end), None) => Some(end),
        (None, _) => None,
    };
    Ok(RequestRange::Partial { start, end })
}

/// Resolve the range the proxy serves to the client.
///
/// An open-ended client range must not be shortened when the total size is known. WebKit's media
/// loader keeps a single response open and reads from it continuously; when the response covers
/// less than the requested `bytes=X-`, it aborts the connection and re-requests from inside the
/// window it just received, so playback advances only a few hundred kilobytes per round trip and
/// eventually stalls at a demuxer error. The upstream provider is still polled in
/// `UPSTREAM_PART_BYTES` chunks by `forward_provider_ranges`, so the first bytes reach the client
/// just as quickly as before.
fn resolve_open_ended_range(range: RequestRange, size: Option<u64>) -> RequestRange {
    match range {
        RequestRange::Partial { start, end: None } => {
            let end = size
                .and_then(|size| size.checked_sub(1))
                .filter(|end| *end >= start)
                .unwrap_or_else(|| start.saturating_add(UPSTREAM_PART_BYTES - 1));
            RequestRange::Partial {
                start,
                end: Some(end),
            }
        }
        range => range,
    }
}

fn response_range_headers(
    range: &RequestRange,
    size: Option<u64>,
) -> (u16, Option<u64>, Option<String>) {
    match range {
        RequestRange::Full => (200, size, None),
        RequestRange::Partial { start, end } => {
            let content_length = end.and_then(|end| {
                end.checked_sub(*start)
                    .and_then(|value| value.checked_add(1))
            });
            let content_range = end.map(|end| {
                format!(
                    "bytes {start}-{end}/{}",
                    size.map_or_else(|| "*".into(), |value| value.to_string())
                )
            });
            (206, content_length, content_range)
        }
    }
}

fn content_type(metadata: &StorageFileStreamMetadata) -> &str {
    if let Some(media_type) = metadata
        .media_type
        .as_deref()
        .filter(|value| is_video_or_audio_mime_type(value))
    {
        return media_type;
    }
    file_extension_media_type(&metadata.name).unwrap_or_else(|| {
        mime_guess::from_path(&metadata.name)
            .first_raw()
            .unwrap_or("application/octet-stream")
    })
}

fn file_extension_media_type(name: &str) -> Option<&'static str> {
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match extension.as_str() {
        "mp4" | "m4v" | "mov" | "mpv" => Some("video/mp4"),
        "mkv" => Some("video/x-matroska"),
        "webm" => Some("video/webm"),
        "avi" => Some("video/x-msvideo"),
        "mpeg" | "mpg" => Some("video/mpeg"),
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        _ => None,
    }
}

fn is_video_or_audio_mime_type(value: &str) -> bool {
    let media_type = value.split(';').next().map(str::trim).unwrap_or_default();
    (media_type.starts_with("video/") || media_type.starts_with("audio/"))
        && media_type.contains('/')
}

fn content_disposition(name: &str) -> String {
    let fallback = name
        .chars()
        .map(|value| match value {
            '"' | '\\' | '\r' | '\n' => '_',
            value if value.is_ascii() && !value.is_ascii_control() => value,
            _ => '_',
        })
        .collect::<String>();
    let fallback = if fallback.is_empty() {
        "media".to_owned()
    } else {
        fallback
    };
    let encoded = utf8_percent_encode(name, NON_ALPHANUMERIC);
    format!("inline; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

fn is_client_disconnect(error: &str) -> bool {
    [
        "broken pipe",
        "connection reset",
        "connection aborted",
        "os error 32",
        "os error 104",
        "断开的管道",
        "连接被对方重置",
    ]
    .iter()
    .any(|marker| error.to_ascii_lowercase().contains(marker))
}

async fn read_request(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err(String::new());
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
        if request.len() > REQUEST_LIMIT {
            return Err("media proxy request is too large".into());
        }
    }
}

fn parse_request(request: &[u8]) -> Option<(String, String, HashMap<String, String>)> {
    let request = std::str::from_utf8(request).ok()?;
    let head = request.split("\r\n\r\n").next()?;
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    if first.next()? != "HTTP/1.1" {
        return None;
    }
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Some((method, target, headers))
}

async fn forward_provider_ranges(
    stream: &mut TcpStream,
    storage: &StorageService,
    session: &PlaybackSession,
    range: RequestRange,
    mut initial_source: Option<ProviderByteStream>,
) -> Result<(), ForwardError> {
    let Some((mut start, end)) = stream_bounds(range, session.metadata.size) else {
        let source = match initial_source {
            Some(source) => source,
            None => open_provider_stream(storage, session, range.start(), range.end()).await?,
        };
        match forward_stream(stream, source, None).await {
            Ok(_) => {}
            Err(error) => return Err(error),
        }
        return Ok(());
    };

    while start <= end {
        let part_end = start.saturating_add(UPSTREAM_PART_BYTES - 1).min(end);
        let expected = part_end - start + 1;
        let mut delivered = 0_u64;
        let mut attempts = 0_usize;
        while delivered < expected {
            let request_start = start + delivered;
            let source = if delivered == 0 && start == initial_range_start(range) {
                if let Some(source) = initial_source.take() {
                    source
                } else {
                    open_provider_stream(storage, session, Some(request_start), Some(part_end))
                        .await?
                }
            } else {
                open_provider_stream(storage, session, Some(request_start), Some(part_end)).await?
            };
            match forward_stream(stream, source, Some(expected - delivered)).await {
                Ok(bytes) => {
                    delivered = delivered.saturating_add(bytes);
                    if bytes == 0 {
                        if attempts >= UPSTREAM_RETRIES {
                            return Err(ForwardError::Upstream {
                                bytes: delivered,
                                error: "上游流提前结束".into(),
                            });
                        }
                        attempts += 1;
                        retry_upstream(attempts).await;
                    }
                }
                Err(ForwardError::Client(error)) => return Err(ForwardError::Client(error)),
                Err(ForwardError::Upstream { bytes, error }) => {
                    delivered = delivered.saturating_add(bytes);
                    if attempts >= UPSTREAM_RETRIES {
                        return Err(ForwardError::Upstream {
                            bytes: delivered,
                            error: format!("上游流传输失败: {error}"),
                        });
                    }
                    attempts += 1;
                    retry_upstream(attempts).await;
                }
            }
        }
        start = part_end.saturating_add(1);
    }
    Ok(())
}

async fn open_initial_provider_stream(
    storage: &StorageService,
    session: &PlaybackSession,
    range: RequestRange,
) -> Result<ProviderByteStream, ForwardError> {
    match range {
        RequestRange::Full => {
            let end = session
                .metadata
                .size
                .and_then(|size| size.checked_sub(1))
                .map(|end| end.min(UPSTREAM_PART_BYTES - 1));
            open_provider_stream(storage, session, Some(0), end).await
        }
        RequestRange::Partial { start, end } => {
            let end = end.map(|end| end.min(start.saturating_add(UPSTREAM_PART_BYTES - 1)));
            open_provider_stream(storage, session, Some(start), end).await
        }
    }
}

async fn open_provider_stream(
    storage: &StorageService,
    session: &PlaybackSession,
    range_start: Option<u64>,
    range_end: Option<u64>,
) -> Result<ProviderByteStream, ForwardError> {
    storage
        .stream_file_for_proxy(
            DownloadFileRequest {
                file_id: session.file_id.clone(),
                range_start,
                range_end_inclusive: range_end,
                expected_revision: session.expected_revision.clone(),
            },
            &session.account_id,
        )
        .await
        .map_err(|error| ForwardError::Upstream {
            bytes: 0,
            error: format!("上游流获取失败: {error}"),
        })
}

fn initial_range_start(range: RequestRange) -> u64 {
    match range {
        RequestRange::Full => 0,
        RequestRange::Partial { start, .. } => start,
    }
}

fn stream_bounds(range: RequestRange, size: Option<u64>) -> Option<(u64, u64)> {
    match range {
        RequestRange::Full => size.and_then(|size| size.checked_sub(1).map(|end| (0, end))),
        RequestRange::Partial { start, end } => end.map(|end| (start, end)),
    }
}

async fn retry_upstream(attempt: usize) {
    tokio::time::sleep(Duration::from_millis(150 * attempt as u64)).await;
}

#[derive(Debug)]
enum ForwardError {
    Upstream { bytes: u64, error: String },
    Client(String),
}

async fn forward_stream(
    stream: &mut TcpStream,
    mut source: ProviderByteStream,
    max_bytes: Option<u64>,
) -> Result<u64, ForwardError> {
    let mut bytes = 0_u64;
    while let Some(chunk) = source.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                return Err(ForwardError::Upstream {
                    bytes,
                    error: error.to_string(),
                });
            }
        };
        let chunk = if let Some(max_bytes) = max_bytes {
            let remaining = max_bytes.saturating_sub(bytes);
            if remaining == 0 {
                break;
            }
            &chunk[..chunk
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX))]
        } else {
            &chunk[..]
        };
        bytes = bytes.saturating_add(chunk.len() as u64);
        stream
            .write_all(chunk)
            .await
            .map_err(|error| ForwardError::Client(error.to_string()))?;
        if max_bytes.is_some_and(|max_bytes| bytes >= max_bytes) {
            break;
        }
    }
    Ok(bytes)
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(impl AsRef<str>, impl AsRef<str>)],
) -> Result<(), String> {
    let headers = headers
        .iter()
        .map(|(name, value)| (name.as_ref().to_owned(), value.as_ref().to_owned()))
        .collect::<Vec<_>>();
    write_response_owned(stream, status, &headers).await
}

async fn write_response_owned(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
) -> Result<(), String> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        response.push_str(name.as_ref());
        response.push_str(": ");
        response.push_str(value.as_ref());
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|error| error.to_string())
}

async fn write_error(stream: &mut TcpStream, status: u16, message: &str) -> Result<(), String> {
    let body = message.as_bytes();
    let headers = vec![
        (
            "Content-Type".to_owned(),
            "text/plain; charset=utf-8".to_owned(),
        ),
        ("Content-Length".to_owned(), body.len().to_string()),
        ("Access-Control-Allow-Origin".to_owned(), "*".to_owned()),
        ("Connection".to_owned(), "close".to_owned()),
    ];
    write_response_owned(stream, status, &headers).await?;
    stream
        .write_all(body)
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_browser_ranges() {
        assert_eq!(parse_range(None, Some(100)), Ok(RequestRange::Full));
        assert_eq!(
            parse_range(Some("bytes=0-99"), Some(100)),
            Ok(RequestRange::Partial {
                start: 0,
                end: Some(99)
            })
        );
        assert_eq!(
            parse_range(Some("bytes=50-"), Some(100)),
            Ok(RequestRange::Partial {
                start: 50,
                end: None
            })
        );
        assert_eq!(
            parse_range(Some("bytes=-20"), Some(100)),
            Ok(RequestRange::Partial {
                start: 80,
                end: Some(99)
            })
        );
    }

    #[test]
    fn rejects_invalid_or_multi_ranges() {
        assert!(parse_range(Some("bytes=100-"), Some(100)).is_err());
        assert!(parse_range(Some("bytes=10-1"), Some(100)).is_err());
        assert!(parse_range(Some("bytes=0-1,4-5"), Some(100)).is_err());
    }

    #[test]
    fn serves_open_ended_ranges_up_to_the_end_of_the_file() {
        let size = 274_765_496;
        let start = 1_235_949;
        let range = resolve_open_ended_range(
            parse_range(Some("bytes=1235949-"), Some(size)).unwrap(),
            Some(size),
        );
        assert_eq!(
            range,
            RequestRange::Partial {
                start,
                end: Some(size - 1)
            }
        );
        assert_eq!(
            response_range_headers(&range, Some(size)),
            (
                206,
                Some(size - start),
                Some(format!("bytes {start}-{}/{}", size - 1, size))
            )
        );
        // The upstream provider is still polled in fixed-size chunks.
        assert_eq!(stream_bounds(range, Some(size)), Some((start, size - 1)));
    }

    #[test]
    fn caps_open_ended_ranges_only_when_the_size_is_unknown() {
        let start = 17 * 1024 * 1024 - 1;
        assert_eq!(
            resolve_open_ended_range(
                RequestRange::Partial { start, end: None },
                Some(17 * 1024 * 1024),
            ),
            RequestRange::Partial {
                start,
                end: Some(17 * 1024 * 1024 - 1),
            }
        );
        assert_eq!(
            resolve_open_ended_range(RequestRange::Partial { start, end: None }, None),
            RequestRange::Partial {
                start,
                end: Some(start + UPSTREAM_PART_BYTES - 1),
            }
        );
    }

    #[test]
    fn turns_known_size_requests_into_small_upstream_ranges() {
        assert_eq!(
            stream_bounds(RequestRange::Full, Some(25 * 1024 * 1024)),
            Some((0, 25 * 1024 * 1024 - 1))
        );
        assert_eq!(
            stream_bounds(
                RequestRange::Partial {
                    start: 10,
                    end: Some(99)
                },
                Some(100)
            ),
            Some((10, 99))
        );
        assert_eq!(stream_bounds(RequestRange::Full, None), None);
    }

    #[test]
    fn creates_safe_inline_content_disposition() {
        let header = content_disposition("视频 \"预览\".mpv");
        assert!(header.starts_with("inline; filename=\""));
        assert!(header.contains(".mpv\"; filename*=UTF-8''"));
        assert!(header.contains("%E8%A7%86%E9%A2%91"));
        assert!(!header.contains('\n'));
    }

    #[test]
    fn identifies_expected_client_disconnects() {
        assert!(is_client_disconnect("断开的管道 (os error 32)"));
        assert!(is_client_disconnect("connection reset by peer"));
        assert!(!is_client_disconnect("上游流提前结束"));
    }

    #[test]
    fn derives_media_type_when_provider_returns_a_format_name() {
        let mp4 = StorageFileStreamMetadata {
            name: "movie.mp4".into(),
            media_type: Some("mp4".into()),
            size: Some(1),
            revision: None,
        };
        assert_eq!(content_type(&mp4), "video/mp4");

        let mpv = StorageFileStreamMetadata {
            name: "movie.mpv".into(),
            media_type: Some("mpv".into()),
            size: Some(1),
            revision: None,
        };
        assert_eq!(content_type(&mpv), "video/mp4");
    }

    #[test]
    fn accepts_only_real_video_or_audio_mime_types() {
        assert!(is_video_or_audio_mime_type("video/mp4"));
        assert!(is_video_or_audio_mime_type("audio/mp4; codecs=mp4a.40.2"));
        assert!(!is_video_or_audio_mime_type("mp4"));
        assert!(!is_video_or_audio_mime_type("application/octet-stream"));
    }

    #[test]
    fn routes_container_formats_through_ffmpeg() {
        assert!(should_transcode(&StorageFileStreamMetadata {
            name: "movie.mkv".into(),
            media_type: Some("video/x-matroska".into()),
            size: Some(1),
            revision: None,
        }));
        assert!(should_transcode(&StorageFileStreamMetadata {
            name: "movie.avi".into(),
            media_type: None,
            size: Some(1),
            revision: None,
        }));
        assert!(!should_transcode(&StorageFileStreamMetadata {
            name: "movie.mp4".into(),
            media_type: Some("video/mp4".into()),
            size: Some(1),
            revision: None,
        }));
    }

    #[test]
    fn only_browser_safe_h264_aac_streams_are_remuxed() {
        let safe = serde_json::json!({
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "pix_fmt": "yuv420p"},
                {"codec_type": "audio", "codec_name": "aac"}
            ]
        });
        assert!(probe_supports_fragmented_mp4(&safe));

        let unsupported_video = serde_json::json!({
            "streams": [
                {"codec_type": "video", "codec_name": "hevc", "pix_fmt": "yuv420p"},
                {"codec_type": "audio", "codec_name": "aac"}
            ]
        });
        assert!(!probe_supports_fragmented_mp4(&unsupported_video));

        let unsupported_audio = serde_json::json!({
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "pix_fmt": "yuv420p"},
                {"codec_type": "audio", "codec_name": "dts"}
            ]
        });
        assert!(!probe_supports_fragmented_mp4(&unsupported_audio));
    }
}
