use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::browser::{
    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams,
};
use chromiumoxide::cdp::browser_protocol::network::{ErrorReason, ResourceType};
use futures_util::StreamExt;
use reqwest::Url;
use serde::Serialize;

use crate::error::{AppError, AppResult};

const MAX_MAIN_DOCUMENT_REQUESTS: usize = 6;
const DOM_SETTLE_DELAY: Duration = Duration::from_millis(900);
const RENDERED_TEXT_EXPRESSION: &str = r#"(() => {
    const candidates = Array.from(document.querySelectorAll('article, main, [role="main"]'));
    const root = candidates
        .filter((element) => (element.innerText || '').trim().length >= 200)
        .sort((left, right) => (right.innerText || '').length - (left.innerText || '').length)[0]
        || document.body;
    if (!root) return '';
    const clone = root.cloneNode(true);
    clone.querySelectorAll('script, style, noscript, nav, header, footer, aside, form')
        .forEach((element) => element.remove());
    return clone.innerText || clone.textContent || '';
})()"#;

#[derive(Debug, Clone, Serialize)]
pub struct BrowserReadResponse {
    pub final_url: String,
    pub title: Option<String>,
    pub content: String,
    pub truncated: bool,
    pub browser: String,
}

struct TempProfile {
    path: PathBuf,
}

impl TempProfile {
    fn create() -> AppResult<Self> {
        let path =
            std::env::temp_dir().join(format!("agnes-browser-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&path)?;
        Ok(Self { path })
    }
}

impl Drop for TempProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestDecision {
    ValidatePublicUrl,
    AllowLocalUrl,
    Block(&'static str),
}

pub async fn open_public_page(
    url: &str,
    max_chars: usize,
    timeout_sec: u32,
) -> AppResult<BrowserReadResponse> {
    let parsed =
        Url::parse(url).map_err(|error| AppError::Other(format!("Invalid URL: {error}")))?;
    crate::web::validate_public_url(&parsed).await?;

    let timeout = Duration::from_secs(timeout_sec.clamp(5, 60) as u64);
    tokio::time::timeout(timeout, open_public_page_inner(parsed, max_chars, timeout))
        .await
        .map_err(|_| AppError::Other("Browser page load timed out".into()))?
}

async fn open_public_page_inner(
    url: Url,
    max_chars: usize,
    timeout: Duration,
) -> AppResult<BrowserReadResponse> {
    let executable = find_browser_executable()?;
    let browser_name = executable
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("chromium")
        .to_string();
    let profile = TempProfile::create()?;
    let config = BrowserConfig::builder()
        .chrome_executable(&executable)
        .user_data_dir(&profile.path)
        .incognito()
        .new_headless_mode()
        .respect_https_errors()
        .enable_request_intercept()
        .disable_cache()
        .launch_timeout(timeout.min(Duration::from_secs(15)))
        .request_timeout(timeout)
        .args([
            "disable-notifications",
            "disable-remote-fonts",
            "disable-speech-api",
            "disable-file-system",
        ])
        .build()
        .map_err(|error| AppError::Other(format!("Unable to configure browser: {error}")))?;

    let (mut browser, mut handler) = Browser::launch(config).await.map_err(|error| {
        AppError::Other(format!(
            "Unable to launch `{}` for browser reading: {error}",
            executable.display()
        ))
    })?;
    let handler_task = tokio::spawn(async move {
        while let Some(result) = handler.next().await {
            if result.is_err() {
                break;
            }
        }
    });

    let operation = async {
        browser
            .execute(SetDownloadBehaviorParams::new(
                SetDownloadBehaviorBehavior::Deny,
            ))
            .await
            .map_err(browser_error("Unable to disable browser downloads"))?;
        let page = Arc::new(
            browser
                .new_page("about:blank")
                .await
                .map_err(browser_error("Unable to create browser page"))?,
        );
        let request_events = page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(browser_error("Unable to enable browser request filtering"))?;
        let (blocked_tx, mut blocked_rx) = tokio::sync::mpsc::unbounded_channel();
        let intercept_task =
            tokio::spawn(filter_requests(page.clone(), request_events, blocked_tx));

        let navigation = page.goto(url.as_str()).await;
        if let Err(error) = navigation {
            intercept_task.abort();
            if let Ok(reason) = blocked_rx.try_recv() {
                return Err(AppError::Other(format!(
                    "Browser navigation was blocked: {reason}"
                )));
            }
            return Err(AppError::Other(format!(
                "Browser navigation failed: {error}"
            )));
        }
        tokio::time::sleep(DOM_SETTLE_DELAY).await;

        let final_url = page
            .url()
            .await
            .map_err(browser_error("Unable to read final browser URL"))?
            .ok_or_else(|| AppError::Other("Browser page did not report a final URL".into()))?;
        let final_parsed = Url::parse(&final_url)
            .map_err(|error| AppError::Other(format!("Invalid final browser URL: {error}")))?;
        crate::web::validate_public_url(&final_parsed).await?;

        let html = page
            .content()
            .await
            .map_err(browser_error("Unable to read rendered page"))?;
        let (extracted_title, html_extracted) = crate::web::extract_html(&html);
        let rendered_text = match page.evaluate(RENDERED_TEXT_EXPRESSION).await {
            Ok(result) => result
                .into_value::<String>()
                .ok()
                .map(|text| crate::web::normalize_plain_text(&text))
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        let extracted = if rendered_text.trim().is_empty() {
            html_extracted
        } else {
            rendered_text
        };
        if extracted.trim().is_empty() {
            intercept_task.abort();
            return Err(AppError::Other(
                "The rendered page did not contain readable text".into(),
            ));
        }
        let title = page.get_title().await.ok().flatten().or(extracted_title);
        let (content, truncated) = crate::web::truncate_chars(&extracted, max_chars);
        intercept_task.abort();
        Ok(BrowserReadResponse {
            final_url,
            title,
            content,
            truncated,
            browser: browser_name,
        })
    }
    .await;

    let _ = browser.close().await;
    let _ = browser.wait().await;
    let _ = tokio::time::timeout(Duration::from_secs(2), handler_task).await;
    drop(profile);
    operation
}

async fn filter_requests(
    page: Arc<chromiumoxide::Page>,
    mut events: chromiumoxide::listeners::EventStream<EventRequestPaused>,
    blocked_tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    let mut main_frame = None;
    let mut main_document_requests = 0_usize;
    while let Some(event) = events.next().await {
        let is_main_document = if event.resource_type == ResourceType::Document {
            let frame = main_frame.get_or_insert_with(|| event.frame_id.clone());
            *frame == event.frame_id
        } else {
            false
        };
        if is_main_document {
            main_document_requests += 1;
        }

        let decision = if is_main_document && main_document_requests > MAX_MAIN_DOCUMENT_REQUESTS {
            RequestDecision::Block("redirect limit exceeded")
        } else {
            request_decision(
                &event.request.method,
                &event.request.url,
                &event.resource_type,
            )
        };
        let denial = match decision {
            RequestDecision::ValidatePublicUrl => match Url::parse(&event.request.url) {
                Ok(url) => crate::web::validate_public_url(&url)
                    .await
                    .err()
                    .map(|error| error.to_string()),
                Err(_) => Some("invalid request URL".into()),
            },
            RequestDecision::AllowLocalUrl => None,
            RequestDecision::Block(reason) => Some(reason.into()),
        };

        let result = if let Some(reason) = denial {
            if is_main_document {
                let _ = blocked_tx.send(reason);
            }
            page.execute(FailRequestParams::new(
                event.request_id.clone(),
                ErrorReason::BlockedByClient,
            ))
            .await
            .map(|_| ())
        } else {
            page.execute(ContinueRequestParams::new(event.request_id.clone()))
                .await
                .map(|_| ())
        };
        if result.is_err() {
            break;
        }
    }
}

fn request_decision(method: &str, url: &str, resource_type: &ResourceType) -> RequestDecision {
    if !matches!(method, "GET" | "HEAD") {
        return RequestDecision::Block("only GET and HEAD requests are allowed");
    }
    if !matches!(
        resource_type,
        ResourceType::Document
            | ResourceType::Stylesheet
            | ResourceType::Script
            | ResourceType::Xhr
            | ResourceType::Fetch
    ) {
        return RequestDecision::Block("resource type is disabled in read-only browser mode");
    }
    let scheme = url
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => RequestDecision::ValidatePublicUrl,
        "about" | "blob" | "data" => RequestDecision::AllowLocalUrl,
        _ => RequestDecision::Block("URL scheme is disabled in read-only browser mode"),
    }
}

fn browser_error(context: &'static str) -> impl FnOnce(chromiumoxide::error::CdpError) -> AppError {
    move |error| AppError::Other(format!("{context}: {error}"))
}

fn find_browser_executable() -> AppResult<PathBuf> {
    browser_executable_from(
        std::env::var_os("AGNES_BROWSER_PATH").as_deref(),
        std::env::var_os("PATH").as_deref(),
        &platform_browser_paths(),
    )
}

fn browser_executable_from(
    override_path: Option<&OsStr>,
    path_env: Option<&OsStr>,
    fixed_paths: &[PathBuf],
) -> AppResult<PathBuf> {
    if let Some(value) = override_path {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        return Err(AppError::Other(format!(
            "AGNES_BROWSER_PATH does not point to a browser executable: {}",
            path.display()
        )));
    }

    if let Some(path_env) = path_env {
        for directory in std::env::split_paths(path_env) {
            for name in browser_binary_names() {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    if let Some(path) = fixed_paths.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }
    Err(AppError::Other(
        "No supported Chromium browser was found. Install Microsoft Edge, Google Chrome, or Chromium, or set AGNES_BROWSER_PATH.".into(),
    ))
}

fn browser_binary_names() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &["msedge.exe", "chrome.exe", "chromium.exe"]
    }
    #[cfg(not(target_os = "windows"))]
    {
        &[
            "microsoft-edge-stable",
            "microsoft-edge",
            "google-chrome-stable",
            "google-chrome",
            "chromium",
            "chromium-browser",
        ]
    }
}

fn platform_browser_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        return [
            "/opt/microsoft/msedge/msedge",
            "/usr/bin/microsoft-edge-stable",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    }
    #[cfg(target_os = "macos")]
    {
        return [
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    }
    #[cfg(target_os = "windows")]
    {
        let mut paths = Vec::new();
        for root in [
            std::env::var_os("PROGRAMFILES"),
            std::env::var_os("PROGRAMFILES(X86)"),
            std::env::var_os("LOCALAPPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            paths.push(PathBuf::from(&root).join("Microsoft/Edge/Application/msedge.exe"));
            paths.push(PathBuf::from(root).join("Google/Chrome/Application/chrome.exe"));
        }
        return paths;
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_file(root: &std::path::Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"browser").unwrap();
        path
    }

    #[test]
    fn browser_override_has_priority_and_invalid_override_fails_closed() {
        let root =
            std::env::temp_dir().join(format!("agnes-browser-test-{}", uuid::Uuid::new_v4()));
        let override_path = fake_file(&root, "override-browser");
        let path_browser = fake_file(&root, browser_binary_names()[0]);
        let selected = browser_executable_from(
            Some(override_path.as_os_str()),
            Some(root.as_os_str()),
            &[path_browser],
        )
        .unwrap();
        assert_eq!(selected, override_path);
        assert!(browser_executable_from(
            Some(root.join("missing").as_os_str()),
            Some(root.as_os_str()),
            &[],
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_browser_precedes_fixed_paths() {
        let root =
            std::env::temp_dir().join(format!("agnes-browser-test-{}", uuid::Uuid::new_v4()));
        let path_dir = root.join("bin");
        let path_browser = fake_file(&path_dir, browser_binary_names()[0]);
        let fixed_browser = fake_file(&root, "fixed-browser");
        assert_eq!(
            browser_executable_from(None, Some(path_dir.as_os_str()), &[fixed_browser]).unwrap(),
            path_browser
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn request_filter_allows_only_read_only_rendering_resources() {
        assert_eq!(
            request_decision("GET", "https://example.com/", &ResourceType::Document),
            RequestDecision::ValidatePublicUrl
        );
        assert_eq!(
            request_decision("GET", "data:text/javascript,1", &ResourceType::Script),
            RequestDecision::AllowLocalUrl
        );
        assert!(matches!(
            request_decision("POST", "https://example.com/", &ResourceType::Fetch),
            RequestDecision::Block(_)
        ));
        for resource in [
            ResourceType::Image,
            ResourceType::Media,
            ResourceType::Font,
            ResourceType::WebSocket,
            ResourceType::EventSource,
            ResourceType::Ping,
        ] {
            assert!(matches!(
                request_decision("GET", "https://example.com/", &resource),
                RequestDecision::Block(_)
            ));
        }
        assert!(matches!(
            request_decision("GET", "file:///etc/passwd", &ResourceType::Document),
            RequestDecision::Block(_)
        ));
    }

    #[tokio::test]
    #[ignore = "requires an installed Chromium browser and public network access"]
    async fn renders_a_public_javascript_page() {
        let response = open_public_page("https://quotes.toscrape.com/js/", 5_000, 30)
            .await
            .unwrap();
        assert_eq!(response.final_url, "https://quotes.toscrape.com/js/");
        assert!(response.content.contains("Albert Einstein"));
        assert!(!response.browser.is_empty());
    }
}
