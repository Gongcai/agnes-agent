//! read_image tool: read a local image for a vision-capable model.
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

pub(crate) fn detect_image_media_type(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some("image/tiff");
    }
    if bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && [&b"avif"[..], &b"avis"[..], &b"heic"[..], &b"heix"[..]]
            .iter()
            .any(|brand| bytes[8..12] == **brand)
    {
        return Some(if &bytes[8..12] == b"avif" || &bytes[8..12] == b"avis" {
            "image/avif"
        } else {
            "image/heic"
        });
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "avif" => Some("image/avif"),
        "heic" | "heif" => Some("image/heic"),
        _ => None,
    }
}

pub struct ReadImageTool;

#[async_trait]
impl BuiltinTool for ReadImageTool {
    fn name(&self) -> &'static str {
        "read_image"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name(),
                "description": "Read a local image and return it as a multimodal tool result for a vision-capable model. Use this when you need to inspect an image instead of treating it as text. The image is read-only and must be inside the effective file-read boundary.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path or a path relative to the workspace."},
                        "detail": {"type": "string", "enum": ["low", "auto", "high"], "description": "Optional provider hint for image detail; defaults to auto."}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn risk(&self, args: &Value) -> Risk {
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        const SENSITIVE: &[&str] = &[".ssh", "/etc", "id_rsa", ".env", ".aws", ".gnupg"];
        if SENSITIVE.iter().any(|marker| path.contains(marker)) {
            Risk::Medium
        } else {
            Risk::Low
        }
    }

    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let path_arg = match ctx.args.get("path").and_then(Value::as_str) {
            Some(path) if !path.trim().is_empty() => path,
            _ => {
                let error = "Missing path argument";
                ctx.record_failure(error).await?;
                return Err(AppError::Other(error.to_string()));
            }
        };
        let path = crate::tools::builtin::normalize_path(&crate::tools::builtin::resolve_path(
            ctx, path_arg,
        ));
        if let Err(error) = ctx.policy.check_file_read(&path.to_string_lossy()) {
            ctx.record_failure(&error).await?;
            return Err(AppError::Other(error));
        }
        if let Err(error) = ctx.sandbox.check_read(&path) {
            ctx.record_failure(&error).await?;
            return Err(AppError::Other(error));
        }

        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let message = format!("无法读取图片: {error}");
                ctx.record_failure(&message).await?;
                return Err(AppError::Other(message));
            }
        };
        let size = bytes.len() as u64;
        if size == 0 {
            let message = "图片文件为空";
            ctx.record_failure(message).await?;
            return Err(AppError::Other(message.to_string()));
        }
        if size > MAX_IMAGE_BYTES {
            let message = format!("图片超过 {} MiB 上限", MAX_IMAGE_BYTES / 1024 / 1024);
            ctx.record_failure(&message).await?;
            return Err(AppError::Other(message));
        }
        let Some(media_type) = detect_image_media_type(&path, &bytes) else {
            let message =
                "文件不是支持的图片格式（支持 PNG、JPEG、GIF、WebP、BMP、TIFF、AVIF、HEIC）";
            ctx.record_failure(message).await?;
            return Err(AppError::Other(message.to_string()));
        };
        let detail = match ctx.args.get("detail").and_then(Value::as_str) {
            Some(value @ ("low" | "auto" | "high")) => value,
            Some(_) => "auto",
            None => "auto",
        };
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
        let summary = format!("已读取图片 {name} ({media_type}, {size} bytes)");
        ctx.update_complete("done", Some("image"), Some(0), Some(summary.clone()), None)
            .await?;
        Ok(json!({
            "result_kind": "image",
            "stdout": summary,
            "content": summary,
            "image": {
                "media_type": media_type,
                "data_base64": BASE64.encode(bytes),
                "detail": detail,
                "name": name,
                "path": path_arg,
                "size_bytes": size
            }
        }))
    }
}
