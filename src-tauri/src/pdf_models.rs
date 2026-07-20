use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::error::{AppError, AppResult};

const PACKAGE_ID: &str = "docling-pdf-local";
const PACKAGE_VERSION: &str = "1";
const PACKAGE_SCHEMA_VERSION: u32 = 1;
const PACKAGE_MANIFEST: &str = "agnes-pdf-model-package.json";
const MAX_PACKAGE_ARCHIVE_BYTES: u64 = 3 * 1024 * 1024 * 1024;
const MAX_PACKAGE_UNPACKED_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MAX_PACKAGE_FILES: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfModelPackageStatus {
    pub supported: bool,
    pub installed: bool,
    pub package_version: Option<String>,
    pub docling_version: Option<String>,
    pub size_bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PdfParserRuntime {
    pub executable: PathBuf,
    pub artifacts_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PdfPackageFile {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PdfPackageManifest {
    schema_version: u32,
    package_id: String,
    package_version: String,
    docling_version: String,
    target: String,
    parser: String,
    models_dir: String,
    files: Vec<PdfPackageFile>,
}

#[derive(Debug)]
pub struct PdfModelPackageManager {
    root: PathBuf,
}

impl PdfModelPackageManager {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("model-packages").join(PACKAGE_ID),
        }
    }

    pub fn status(&self) -> PdfModelPackageStatus {
        if !target_supported() {
            return PdfModelPackageStatus {
                supported: false,
                installed: false,
                package_version: None,
                docling_version: None,
                size_bytes: 0,
                error: Some("当前平台暂不支持本地 PDF 模型包".into()),
            };
        }
        match self.load_installed_manifest() {
            Ok(Some((manifest, _))) => PdfModelPackageStatus {
                supported: true,
                installed: true,
                package_version: Some(manifest.package_version),
                docling_version: Some(manifest.docling_version),
                size_bytes: manifest.files.iter().map(|file| file.size).sum(),
                error: None,
            },
            Ok(None) => PdfModelPackageStatus {
                supported: true,
                installed: false,
                package_version: None,
                docling_version: None,
                size_bytes: 0,
                error: None,
            },
            Err(error) => PdfModelPackageStatus {
                supported: true,
                installed: false,
                package_version: None,
                docling_version: None,
                size_bytes: 0,
                error: Some(error.to_string()),
            },
        }
    }

    pub fn runtime(&self) -> AppResult<PdfParserRuntime> {
        let (manifest, directory) = self
            .load_installed_manifest()?
            .ok_or_else(|| AppError::Other("PDF 解析模型包尚未安装，请先在设置中安装".into()))?;
        let executable = safe_join(&directory, &manifest.parser)?;
        let artifacts_path = safe_join(&directory, &manifest.models_dir)?;
        if !executable.is_file() || !artifacts_path.is_dir() {
            return Err(AppError::Other("PDF 解析模型包不完整，请重新安装".into()));
        }
        Ok(PdfParserRuntime {
            executable,
            artifacts_path,
        })
    }

    pub fn install_archive(&self, archive_path: &Path) -> AppResult<PdfModelPackageStatus> {
        if !target_supported() {
            return Err(AppError::Other("当前平台暂不支持本地 PDF 模型包".into()));
        }
        let metadata = fs::metadata(archive_path)?;
        if !metadata.is_file() {
            return Err(AppError::Other("PDF 模型包路径必须是普通文件".into()));
        }
        if metadata.len() > MAX_PACKAGE_ARCHIVE_BYTES {
            return Err(AppError::Other("PDF 模型包超过 3 GiB 上限".into()));
        }
        fs::create_dir_all(&self.root)?;
        let staging = self.root.join(format!(".install-{}", uuid::Uuid::new_v4()));
        let destination = self.root.join(PACKAGE_VERSION);
        let backup = self.root.join(format!(".backup-{}", uuid::Uuid::new_v4()));
        let result = (|| -> AppResult<()> {
            let manifest = extract_and_verify_archive(archive_path, &staging)?;
            validate_manifest(&manifest)?;
            let executable = safe_join(&staging, &manifest.parser)?;
            let artifacts_path = safe_join(&staging, &manifest.models_dir)?;
            if !executable.is_file()
                || !artifacts_path.is_dir()
                || !artifacts_path.join("agnes-models.json").is_file()
            {
                return Err(AppError::Other("PDF 模型包缺少解析器或模型文件".into()));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = fs::metadata(&executable)?.permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&executable, permissions)?;
            }
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
            Ok(())
        })();
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        Ok(self.status())
    }

    pub fn remove(&self) -> AppResult<()> {
        let destination = self.root.join(PACKAGE_VERSION);
        if destination.exists() {
            fs::remove_dir_all(destination)?;
        }
        Ok(())
    }

    fn load_installed_manifest(&self) -> AppResult<Option<(PdfPackageManifest, PathBuf)>> {
        let directory = self.root.join(PACKAGE_VERSION);
        let manifest_path = directory.join(PACKAGE_MANIFEST);
        if !manifest_path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&manifest_path)?;
        if bytes.len() > 1024 * 1024 {
            return Err(AppError::Other("PDF 模型包清单超过 1 MiB 上限".into()));
        }
        let manifest = serde_json::from_slice::<PdfPackageManifest>(&bytes)?;
        validate_manifest(&manifest)?;
        Ok(Some((manifest, directory)))
    }
}

fn validate_manifest(manifest: &PdfPackageManifest) -> AppResult<()> {
    if manifest.schema_version != PACKAGE_SCHEMA_VERSION
        || manifest.package_id != PACKAGE_ID
        || manifest.package_version != PACKAGE_VERSION
        || manifest.target != current_target()
        || manifest.docling_version != "2.113.0"
        || manifest.files.is_empty()
        || manifest.files.len() > MAX_PACKAGE_FILES
    {
        return Err(AppError::Other("PDF 模型包清单不兼容".into()));
    }
    safe_relative_path(&manifest.parser)?;
    safe_relative_path(&manifest.models_dir)?;
    let mut paths = HashSet::new();
    let mut total = 0u64;
    for file in &manifest.files {
        safe_relative_path(&file.path)?;
        if file.size == 0
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !paths.insert(file.path.as_str())
        {
            return Err(AppError::Other("PDF 模型包文件清单无效".into()));
        }
        total = total
            .checked_add(file.size)
            .ok_or_else(|| AppError::Other("PDF 模型包解压大小超过上限".into()))?;
    }
    if total > MAX_PACKAGE_UNPACKED_BYTES {
        return Err(AppError::Other("PDF 模型包解压后超过 5 GiB 上限".into()));
    }
    Ok(())
}

fn extract_and_verify_archive(
    archive_path: &Path,
    destination: &Path,
) -> AppResult<PdfPackageManifest> {
    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|error| AppError::Other(format!("PDF 模型包不是有效 ZIP：{error}")))?;
    if archive.len() > MAX_PACKAGE_FILES + 256 {
        return Err(AppError::Other("PDF 模型包包含过多文件".into()));
    }
    let manifest = {
        let mut entry = archive
            .by_name(PACKAGE_MANIFEST)
            .map_err(|_| AppError::Other("PDF 模型包缺少清单".into()))?;
        if entry.size() > 1024 * 1024 {
            return Err(AppError::Other("PDF 模型包清单超过 1 MiB 上限".into()));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        serde_json::from_slice::<PdfPackageManifest>(&bytes)?
    };
    validate_manifest(&manifest)?;
    let expected = manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    fs::create_dir_all(destination)?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AppError::Other(format!("无法读取 PDF 模型包：{error}")))?;
        if entry.encrypted() {
            return Err(AppError::Other("PDF 模型包不能包含加密文件".into()));
        }
        let path = entry
            .enclosed_name()
            .ok_or_else(|| AppError::Other("PDF 模型包包含不安全路径".into()))?;
        let path_string = path
            .to_str()
            .ok_or_else(|| AppError::Other("PDF 模型包路径不是有效 UTF-8".into()))?
            .replace('\\', "/");
        if path_string == PACKAGE_MANIFEST {
            let output = destination.join(PACKAGE_MANIFEST);
            fs::write(output, serde_json::to_vec_pretty(&manifest)?)?;
            continue;
        }
        if entry.is_dir() {
            fs::create_dir_all(destination.join(path))?;
            continue;
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(AppError::Other("PDF 模型包不能包含符号链接".into()));
        }
        let expected_file = expected
            .get(path_string.as_str())
            .ok_or_else(|| AppError::Other("PDF 模型包包含清单外文件".into()))?;
        if entry.size() != expected_file.size || !seen.insert(path_string.clone()) {
            return Err(AppError::Other("PDF 模型包文件大小或路径无效".into()));
        }
        let output_path = destination.join(path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = fs::File::create(output_path)?;
        let mut hasher = Sha256::new();
        let mut remaining = expected_file.size;
        let mut buffer = [0u8; 128 * 1024];
        while remaining > 0 {
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                return Err(AppError::Other("PDF 模型包文件被截断".into()));
            }
            remaining = remaining.saturating_sub(count as u64);
            output.write_all(&buffer[..count])?;
            hasher.update(&buffer[..count]);
        }
        if format!("{:x}", hasher.finalize()) != expected_file.sha256 {
            return Err(AppError::Other("PDF 模型包文件校验失败".into()));
        }
    }
    if seen.len() != expected.len() {
        return Err(AppError::Other("PDF 模型包缺少清单中的文件".into()));
    }
    Ok(manifest)
}

fn safe_relative_path(value: &str) -> AppResult<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || value.contains('\0')
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(AppError::Other("PDF 模型包包含不安全路径".into()));
    }
    Ok(path.to_path_buf())
}

fn safe_join(root: &Path, value: &str) -> AppResult<PathBuf> {
    Ok(root.join(safe_relative_path(value)?))
}

pub fn current_target() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else {
        "unsupported"
    }
}

fn target_supported() -> bool {
    current_target() != "unsupported"
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use super::{current_target, PdfModelPackageManager, PdfPackageFile, PdfPackageManifest};

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn write_package(directory: &TempDir, parser_hash: String) -> std::path::PathBuf {
        let path = directory.path().join("package.zip");
        let parser = b"parser-binary";
        let models = b"{}";
        let manifest = PdfPackageManifest {
            schema_version: 1,
            package_id: "docling-pdf-local".into(),
            package_version: "1".into(),
            docling_version: "2.113.0".into(),
            target: current_target().into(),
            parser: "bin/docling-pdf-parserd".into(),
            models_dir: "models".into(),
            files: vec![
                PdfPackageFile {
                    path: "bin/docling-pdf-parserd".into(),
                    size: parser.len() as u64,
                    sha256: parser_hash,
                },
                PdfPackageFile {
                    path: "models/agnes-models.json".into(),
                    size: models.len() as u64,
                    sha256: hash(models),
                },
            ],
        };
        let file = std::fs::File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive
            .start_file("agnes-pdf-model-package.json", options)
            .unwrap();
        archive
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        archive
            .start_file("bin/docling-pdf-parserd", options)
            .unwrap();
        archive.write_all(parser).unwrap();
        archive
            .start_file("models/agnes-models.json", options)
            .unwrap();
        archive.write_all(models).unwrap();
        archive.finish().unwrap();
        path
    }

    #[test]
    fn installs_validated_pdf_model_packages_atomically() {
        let directory = TempDir::new().unwrap();
        let archive = write_package(&directory, hash(b"parser-binary"));
        let manager = PdfModelPackageManager::new(directory.path());

        let status = manager.install_archive(&archive).unwrap();

        assert!(status.installed);
        assert_eq!(status.docling_version.as_deref(), Some("2.113.0"));
        assert!(manager.runtime().unwrap().executable.is_file());
        manager.remove().unwrap();
        assert!(!manager.status().installed);
    }

    #[test]
    fn rejects_pdf_packages_with_tampered_files() {
        let directory = TempDir::new().unwrap();
        let archive = write_package(&directory, "a".repeat(64));
        let manager = PdfModelPackageManager::new(directory.path());

        assert!(manager.install_archive(&archive).is_err());
        assert!(!manager.status().installed);
    }

    #[test]
    #[ignore = "requires a package built with pnpm build:pdf-package"]
    fn installs_a_built_pdf_package() {
        let archive = std::env::var("AGNES_PDF_PACKAGE")
            .expect("AGNES_PDF_PACKAGE must point to the built ZIP");
        let directory = TempDir::new().unwrap();
        let manager = PdfModelPackageManager::new(directory.path());

        let status = manager
            .install_archive(std::path::Path::new(&archive))
            .unwrap();

        assert!(status.installed);
        assert_eq!(status.docling_version.as_deref(), Some("2.113.0"));
        let runtime = manager.runtime().unwrap();
        assert!(runtime.executable.is_file());
        assert!(runtime.artifacts_path.join("agnes-models.json").is_file());
    }
}
