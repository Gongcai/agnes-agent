use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use encoding_rs::{Encoding, UTF_8};
use futures_util::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_LENGTH, CONTENT_TYPE, LOCATION,
};
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

pub const SEARCH_PROVIDER_SETTINGS_KEY: &str = "web:search_providers:v1";
pub const BRAVE_SEARCH_API_KEY_SECRET_ID: &str = "web:search:brave:api_key";
pub const SEARCH_PROVIDER_IDS: [&str; 4] = ["duckduckgo", "bing", "searxng", "brave"];

const SEARCH_BODY_LIMIT: usize = 2 * 1024 * 1024;
const SEARCH_API_BODY_LIMIT: usize = 2 * 1024 * 1024;
const FETCH_BODY_LIMIT: usize = 3 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 AgnesAgent/0.1 WebResearch";

#[derive(Debug, Clone, Serialize)]
pub struct WebSearchResult {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebSearchResponse {
    pub query: String,
    pub provider: String,
    pub results: Vec<WebSearchResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback_attempts: Vec<SearchFallbackAttempt>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchFallbackAttempt {
    pub provider: String,
    pub category: SearchFailureCategory,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchFailureCategory {
    Authentication,
    RateLimit,
    Timeout,
    Network,
    ServiceUnavailable,
    InvalidConfig,
    InvalidResponse,
    EmptyResults,
}

impl SearchFailureCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::ServiceUnavailable => "service_unavailable",
            Self::InvalidConfig => "invalid_config",
            Self::InvalidResponse => "invalid_response",
            Self::EmptyResults => "empty_results",
        }
    }

    pub fn display_message(self) -> &'static str {
        match self {
            Self::Authentication => "认证失败",
            Self::RateLimit => "请求受到限流",
            Self::Timeout => "请求超时",
            Self::Network => "网络连接失败",
            Self::ServiceUnavailable => "服务暂时不可用",
            Self::InvalidConfig => "配置不完整或无效",
            Self::InvalidResponse => "服务返回了无法解析的响应",
            Self::EmptyResults => "没有返回搜索结果",
        }
    }
}

#[derive(Debug, Clone)]
struct SearchProviderFailure {
    category: SearchFailureCategory,
}

impl SearchProviderFailure {
    fn new(category: SearchFailureCategory) -> Self {
        Self { category }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchProviderSettings {
    pub fallback_order: Vec<String>,
    pub searxng_base_url: Option<String>,
}

impl Default for SearchProviderSettings {
    fn default() -> Self {
        Self {
            fallback_order: vec!["duckduckgo".into(), "bing".into()],
            searxng_base_url: None,
        }
    }
}

impl SearchProviderSettings {
    pub fn normalize(&mut self) -> AppResult<()> {
        if self.fallback_order.len() > SEARCH_PROVIDER_IDS.len() {
            return Err(AppError::Other(format!(
                "搜索回退链最多包含 {} 个 Provider",
                SEARCH_PROVIDER_IDS.len()
            )));
        }
        let mut seen = HashSet::new();
        self.fallback_order = self
            .fallback_order
            .drain(..)
            .map(|provider| provider.trim().to_ascii_lowercase())
            .filter(|provider| !provider.is_empty() && seen.insert(provider.clone()))
            .collect();
        if self.fallback_order.is_empty() {
            return Err(AppError::Other("搜索回退链至少需要一个 Provider".into()));
        }
        if let Some(provider) = self
            .fallback_order
            .iter()
            .find(|provider| !SEARCH_PROVIDER_IDS.contains(&provider.as_str()))
        {
            return Err(AppError::Other(format!(
                "不支持的搜索 Provider `{provider}`"
            )));
        }
        self.searxng_base_url = self
            .searxng_base_url
            .take()
            .map(|value| normalize_searxng_base_url(&value))
            .transpose()?;
        if self
            .fallback_order
            .iter()
            .any(|provider| provider == "searxng")
            && self.searxng_base_url.is_none()
        {
            return Err(AppError::Other(
                "回退链包含 SearXNG，但尚未配置服务地址".into(),
            ));
        }
        Ok(())
    }
}

pub async fn load_search_provider_settings(
    db: &DbActorHandle,
) -> AppResult<SearchProviderSettings> {
    let raw = db
        .get_setting(SEARCH_PROVIDER_SETTINGS_KEY.to_string())
        .await?;
    let mut settings: SearchProviderSettings = raw
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    settings.normalize()?;
    Ok(settings)
}

pub async fn save_search_provider_settings(
    db: &DbActorHandle,
    settings: &SearchProviderSettings,
) -> AppResult<()> {
    db.set_setting(
        SEARCH_PROVIDER_SETTINGS_KEY.to_string(),
        serde_json::to_string(settings)?,
    )
    .await
}

#[derive(Debug, Clone, Serialize)]
pub struct WebFetchResponse {
    pub final_url: String,
    pub title: Option<String>,
    pub content_type: String,
    pub content: String,
    pub truncated: bool,
}

struct DownloadedPage {
    final_url: Url,
    content_type: String,
    body: Vec<u8>,
    truncated: bool,
}

struct SearchRequest<'a> {
    query: &'a str,
    count: usize,
    language: Option<&'a str>,
    freshness: Option<&'a str>,
    timeout_sec: u32,
}

#[async_trait]
trait SearchProvider: Send + Sync {
    fn id(&self) -> &'static str;

    async fn search(
        &self,
        request: &SearchRequest<'_>,
    ) -> Result<WebSearchResponse, SearchProviderFailure>;
}

struct DuckDuckGoSearchProvider;
struct BingSearchProvider;
struct SearxngSearchProvider {
    base_url: String,
}
struct BraveSearchProvider {
    api_key: String,
}

#[async_trait]
impl SearchProvider for DuckDuckGoSearchProvider {
    fn id(&self) -> &'static str {
        "duckduckgo"
    }

    async fn search(
        &self,
        request: &SearchRequest<'_>,
    ) -> Result<WebSearchResponse, SearchProviderFailure> {
        search_duckduckgo(
            request.query,
            request.count,
            request.language,
            request.freshness,
            request.timeout_sec,
        )
        .await
        .map_err(|error| classify_search_error(self.id(), &error))
    }
}

#[async_trait]
impl SearchProvider for BingSearchProvider {
    fn id(&self) -> &'static str {
        "bing"
    }

    async fn search(
        &self,
        request: &SearchRequest<'_>,
    ) -> Result<WebSearchResponse, SearchProviderFailure> {
        search_bing(
            request.query,
            request.count,
            request.language,
            request.freshness,
            request.timeout_sec,
        )
        .await
        .map_err(|error| classify_search_error(self.id(), &error))
    }
}

#[async_trait]
impl SearchProvider for SearxngSearchProvider {
    fn id(&self) -> &'static str {
        "searxng"
    }

    async fn search(
        &self,
        request: &SearchRequest<'_>,
    ) -> Result<WebSearchResponse, SearchProviderFailure> {
        search_searxng(&self.base_url, request).await
    }
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    fn id(&self) -> &'static str {
        "brave"
    }

    async fn search(
        &self,
        request: &SearchRequest<'_>,
    ) -> Result<WebSearchResponse, SearchProviderFailure> {
        search_brave(&self.api_key, request).await
    }
}

pub async fn search(
    provider: &str,
    query: &str,
    count: usize,
    language: Option<&str>,
    freshness: Option<&str>,
    timeout_sec: u32,
    settings: &SearchProviderSettings,
    brave_api_key: Option<&str>,
) -> AppResult<WebSearchResponse> {
    let language = normalize_language(language)?;
    let request = SearchRequest {
        query,
        count,
        language: language.as_deref(),
        freshness,
        timeout_sec,
    };
    let provider = provider.trim().to_ascii_lowercase();
    let provider_ids = if provider.is_empty() || provider == "auto" {
        settings.fallback_order.clone()
    } else if SEARCH_PROVIDER_IDS.contains(&provider.as_str()) {
        vec![provider]
    } else {
        return Err(AppError::Other(format!(
            "Unsupported web search provider `{provider}`"
        )));
    };

    let mut attempts = Vec::new();
    let mut last_empty_response = None;
    for provider_id in provider_ids {
        let provider = match create_search_provider(&provider_id, settings, brave_api_key) {
            Ok(provider) => provider,
            Err(failure) => {
                attempts.push(SearchFallbackAttempt {
                    provider: provider_id,
                    category: failure.category,
                });
                continue;
            }
        };
        match provider.search(&request).await {
            Ok(mut response) if !response.results.is_empty() => {
                response.fallback_attempts = attempts;
                return Ok(response);
            }
            Ok(response) => {
                attempts.push(SearchFallbackAttempt {
                    provider: provider.id().into(),
                    category: SearchFailureCategory::EmptyResults,
                });
                last_empty_response = Some(response);
            }
            Err(failure) => attempts.push(SearchFallbackAttempt {
                provider: provider.id().into(),
                category: failure.category,
            }),
        }
    }

    if let Some(mut response) = last_empty_response {
        response.fallback_attempts = attempts;
        return Ok(response);
    }
    let summary = attempts
        .iter()
        .map(|attempt| format!("{}={}", attempt.provider, attempt.category.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    Err(AppError::Other(format!(
        "Web search providers failed: {summary}"
    )))
}

pub async fn probe_search_provider(
    provider: &str,
    settings: &SearchProviderSettings,
    brave_api_key: Option<&str>,
    timeout_sec: u32,
) -> Result<usize, SearchFailureCategory> {
    let provider = create_search_provider(provider, settings, brave_api_key)
        .map_err(|failure| failure.category)?;
    let request = SearchRequest {
        query: "OpenAI",
        count: 1,
        language: Some("en-US"),
        freshness: None,
        timeout_sec,
    };
    let response = provider
        .search(&request)
        .await
        .map_err(|failure| failure.category)?;
    if response.results.is_empty() {
        Err(SearchFailureCategory::EmptyResults)
    } else {
        Ok(response.results.len())
    }
}

pub async fn fetch(url: &str, max_chars: usize, timeout_sec: u32) -> AppResult<WebFetchResponse> {
    let url = Url::parse(url).map_err(|error| AppError::Other(format!("Invalid URL: {error}")))?;
    let page = download(url, FETCH_BODY_LIMIT, None, timeout_sec).await?;
    let decoded = decode_body(&page.body, &page.content_type);
    let media_type = page
        .content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    let (title, extracted) = if media_type == "text/html"
        || media_type == "application/xhtml+xml"
        || (media_type.is_empty() && looks_like_html(&decoded))
    {
        extract_html(&decoded)
    } else if media_type.starts_with("text/")
        || media_type == "application/json"
        || media_type.ends_with("+json")
        || media_type == "application/xml"
        || media_type.ends_with("+xml")
    {
        (None, normalize_plain_text(&decoded))
    } else {
        return Err(AppError::Other(format!(
            "Unsupported web content type `{}`; only HTML and textual resources can be fetched",
            if media_type.is_empty() {
                "unknown"
            } else {
                &media_type
            }
        )));
    };

    if extracted.trim().is_empty() {
        return Err(AppError::Other(
            "The page did not contain readable text".into(),
        ));
    }
    let (content, output_truncated) = truncate_chars(&extracted, max_chars);
    Ok(WebFetchResponse {
        final_url: page.final_url.to_string(),
        title,
        content_type: if page.content_type.is_empty() {
            "unknown".into()
        } else {
            page.content_type
        },
        content,
        truncated: page.truncated || output_truncated,
    })
}

async fn search_duckduckgo(
    query: &str,
    count: usize,
    language: Option<&str>,
    freshness: Option<&str>,
    timeout_sec: u32,
) -> AppResult<WebSearchResponse> {
    let mut url = Url::parse("https://html.duckduckgo.com/html/").expect("valid search URL");
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("q", query);
        if let Some(region) = language.and_then(duckduckgo_region) {
            params.append_pair("kl", region);
        }
        if let Some(value) = freshness.and_then(duckduckgo_freshness) {
            params.append_pair("df", value);
        }
    }
    let page = download(url, SEARCH_BODY_LIMIT, language, timeout_sec).await?;
    let html = decode_body(&page.body, &page.content_type);
    Ok(WebSearchResponse {
        query: query.to_string(),
        provider: "duckduckgo".into(),
        results: parse_duckduckgo_results(&html, count),
        fallback_attempts: Vec::new(),
    })
}

async fn search_bing(
    query: &str,
    count: usize,
    language: Option<&str>,
    freshness: Option<&str>,
    timeout_sec: u32,
) -> AppResult<WebSearchResponse> {
    let mut url = Url::parse("https://www.bing.com/search").expect("valid search URL");
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("q", query);
        params.append_pair("count", &count.to_string());
        if let Some(language) = language {
            params.append_pair("setlang", language);
            if let Some(country) = language.split('-').nth(1) {
                params.append_pair("cc", &country.to_ascii_lowercase());
            }
        }
        if let Some(value) = freshness.and_then(bing_freshness) {
            params.append_pair("filters", &value);
        }
    }
    let page = download(url, SEARCH_BODY_LIMIT, language, timeout_sec).await?;
    let html = decode_body(&page.body, &page.content_type);
    Ok(WebSearchResponse {
        query: query.to_string(),
        provider: "bing".into(),
        results: parse_bing_results(&html, count),
        fallback_attempts: Vec::new(),
    })
}

fn create_search_provider(
    provider: &str,
    settings: &SearchProviderSettings,
    brave_api_key: Option<&str>,
) -> Result<Box<dyn SearchProvider>, SearchProviderFailure> {
    match provider {
        "duckduckgo" => Ok(Box::new(DuckDuckGoSearchProvider)),
        "bing" => Ok(Box::new(BingSearchProvider)),
        "searxng" => settings
            .searxng_base_url
            .as_ref()
            .map(|base_url| {
                Box::new(SearxngSearchProvider {
                    base_url: base_url.clone(),
                }) as Box<dyn SearchProvider>
            })
            .ok_or_else(|| SearchProviderFailure::new(SearchFailureCategory::InvalidConfig)),
        "brave" => brave_api_key
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|api_key| {
                Box::new(BraveSearchProvider {
                    api_key: api_key.to_string(),
                }) as Box<dyn SearchProvider>
            })
            .ok_or_else(|| SearchProviderFailure::new(SearchFailureCategory::InvalidConfig)),
        _ => Err(SearchProviderFailure::new(
            SearchFailureCategory::InvalidConfig,
        )),
    }
}

fn normalize_searxng_base_url(value: &str) -> AppResult<String> {
    let mut url = Url::parse(value.trim())
        .map_err(|error| AppError::Other(format!("SearXNG 地址无效: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::Other("SearXNG 地址只能使用 HTTP 或 HTTPS".into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::Other("SearXNG 地址不能包含用户名或密码".into()));
    }
    if url.host_str().is_none() {
        return Err(AppError::Other("SearXNG 地址必须包含主机名".into()));
    }
    url.set_query(None);
    url.set_fragment(None);
    let normalized = url.as_str().trim_end_matches('/').to_string();
    if normalized.len() > 2048 {
        return Err(AppError::Other("SearXNG 地址过长".into()));
    }
    Ok(normalized)
}

fn classify_search_error(_provider: &str, error: &AppError) -> SearchProviderFailure {
    let message = error.to_string().to_ascii_lowercase();
    let category = if message.contains("429") || message.contains("rate limit") {
        SearchFailureCategory::RateLimit
    } else if message.contains("timed out") || message.contains("timeout") {
        SearchFailureCategory::Timeout
    } else if ["http 500", "http 502", "http 503", "http 504", "http 529"]
        .iter()
        .any(|marker| message.contains(marker))
    {
        SearchFailureCategory::ServiceUnavailable
    } else if message.contains("dns")
        || message.contains("request failed")
        || message.contains("connection")
        || message.contains("resolve")
    {
        SearchFailureCategory::Network
    } else {
        SearchFailureCategory::InvalidResponse
    };
    SearchProviderFailure::new(category)
}

fn status_failure(status: reqwest::StatusCode) -> SearchProviderFailure {
    let category = match status.as_u16() {
        401 | 403 => SearchFailureCategory::Authentication,
        429 => SearchFailureCategory::RateLimit,
        500..=599 => SearchFailureCategory::ServiceUnavailable,
        _ => SearchFailureCategory::InvalidResponse,
    };
    SearchProviderFailure::new(category)
}

fn request_failure(error: &reqwest::Error) -> SearchProviderFailure {
    let category = if error.is_timeout() {
        SearchFailureCategory::Timeout
    } else if error.is_connect() || error.is_request() {
        SearchFailureCategory::Network
    } else {
        SearchFailureCategory::InvalidResponse
    };
    SearchProviderFailure::new(category)
}

async fn read_search_api_json(response: reqwest::Response) -> Result<Value, SearchProviderFailure> {
    if !response.status().is_success() {
        return Err(status_failure(response.status()));
    }
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > SEARCH_API_BODY_LIMIT)
    {
        return Err(SearchProviderFailure::new(
            SearchFailureCategory::InvalidResponse,
        ));
    }
    let mut body = Vec::with_capacity(64 * 1024);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| request_failure(&error))?;
        if body.len().saturating_add(chunk.len()) > SEARCH_API_BODY_LIMIT {
            return Err(SearchProviderFailure::new(
                SearchFailureCategory::InvalidResponse,
            ));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|_| SearchProviderFailure::new(SearchFailureCategory::InvalidResponse))
}

async fn search_searxng(
    base_url: &str,
    request: &SearchRequest<'_>,
) -> Result<WebSearchResponse, SearchProviderFailure> {
    let base_url = base_url.trim_end_matches('/');
    let endpoint = if base_url.ends_with("/search") {
        base_url.to_string()
    } else {
        format!("{base_url}/search")
    };
    let mut url = Url::parse(&endpoint)
        .map_err(|_| SearchProviderFailure::new(SearchFailureCategory::InvalidConfig))?;
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("q", request.query);
        params.append_pair("format", "json");
        if let Some(language) = request.language {
            params.append_pair("language", language);
        }
        if let Some(freshness) = request.freshness {
            params.append_pair("time_range", freshness);
        }
    }
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(request.timeout_sec.clamp(3, 60) as u64))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|_| SearchProviderFailure::new(SearchFailureCategory::InvalidConfig))?;
    let response = client
        .get(url)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| request_failure(&error))?;
    let value = read_search_api_json(response).await?;
    Ok(WebSearchResponse {
        query: request.query.to_string(),
        provider: "searxng".into(),
        results: parse_searxng_results(&value, request.count),
        fallback_attempts: Vec::new(),
    })
}

async fn search_brave(
    api_key: &str,
    request: &SearchRequest<'_>,
) -> Result<WebSearchResponse, SearchProviderFailure> {
    let mut url = Url::parse("https://api.search.brave.com/res/v1/web/search")
        .expect("valid Brave Search endpoint");
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("q", request.query);
        params.append_pair("count", &request.count.to_string());
        if let Some(language) = request.language {
            let search_lang = language.split('-').next().unwrap_or(language);
            params.append_pair("search_lang", search_lang);
        }
        if let Some(freshness) = request.freshness.and_then(brave_freshness) {
            params.append_pair("freshness", freshness);
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        "X-Subscription-Token",
        HeaderValue::from_str(api_key)
            .map_err(|_| SearchProviderFailure::new(SearchFailureCategory::InvalidConfig))?,
    );
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(request.timeout_sec.clamp(3, 60) as u64))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(USER_AGENT)
        .default_headers(headers)
        .build()
        .map_err(|_| SearchProviderFailure::new(SearchFailureCategory::InvalidConfig))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| request_failure(&error))?;
    let value = read_search_api_json(response).await?;
    Ok(WebSearchResponse {
        query: request.query.to_string(),
        provider: "brave".into(),
        results: parse_brave_results(&value, request.count),
        fallback_attempts: Vec::new(),
    })
}

async fn download(
    url: Url,
    max_bytes: usize,
    language: Option<&str>,
    timeout_sec: u32,
) -> AppResult<DownloadedPage> {
    let timeout = Duration::from_secs(timeout_sec.clamp(3, 60) as u64);
    tokio::time::timeout(timeout, download_inner(url, max_bytes, language, timeout))
        .await
        .map_err(|_| AppError::Other("Web request timed out".into()))?
}

async fn download_inner(
    mut url: Url,
    max_bytes: usize,
    language: Option<&str>,
    timeout: Duration,
) -> AppResult<DownloadedPage> {
    for redirect_index in 0..=MAX_REDIRECTS {
        let (host, address) = validate_and_resolve(&url).await?;
        let mut builder = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .user_agent(USER_AGENT);
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve(&host, address);
        }
        let client = builder
            .build()
            .map_err(|error| AppError::Other(format!("Unable to create web client: {error}")))?;
        let mut request = client.get(url.clone()).header(
            ACCEPT,
            "text/html,application/xhtml+xml,application/json,text/plain;q=0.9,application/xml;q=0.8,*/*;q=0.1",
        );
        if let Some(language) = language {
            request = request.header(ACCEPT_LANGUAGE, language);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AppError::Other(format!("Web request failed: {error}")))?;
        let status = response.status();
        if status.is_redirection() {
            if redirect_index == MAX_REDIRECTS {
                return Err(AppError::Other(
                    "Web request exceeded redirect limit".into(),
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| AppError::Other("Web redirect omitted Location".into()))?;
            url = url
                .join(location)
                .map_err(|error| AppError::Other(format!("Invalid redirect URL: {error}")))?;
            continue;
        }
        if !status.is_success() {
            return Err(AppError::Other(format!(
                "Web request returned HTTP {}",
                status.as_u16()
            )));
        }
        if let Some(length) = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
        {
            if length > max_bytes.saturating_mul(4) {
                return Err(AppError::Other(format!(
                    "Web response is too large ({length} bytes)"
                )));
            }
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let mut body = Vec::with_capacity(max_bytes.min(64 * 1024));
        let mut stream = response.bytes_stream();
        let mut truncated = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                AppError::Other(format!("Unable to read web response: {error}"))
            })?;
            let remaining = max_bytes.saturating_sub(body.len());
            if chunk.len() > remaining {
                body.extend_from_slice(&chunk[..remaining]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
            if body.len() == max_bytes {
                truncated = true;
                break;
            }
        }
        return Ok(DownloadedPage {
            final_url: url,
            content_type,
            body,
            truncated,
        });
    }
    Err(AppError::Other(
        "Web request exceeded redirect limit".into(),
    ))
}

async fn validate_and_resolve(url: &Url) -> AppResult<(String, SocketAddr)> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(AppError::Other(
            "Only public HTTP and HTTPS URLs are allowed".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::Other(
            "URLs containing credentials are not allowed".into(),
        ));
    }
    let raw_host = url
        .host_str()
        .ok_or_else(|| AppError::Other("URL must include a host".into()))?;
    if raw_host.ends_with('.') {
        return Err(AppError::Other(
            "Hostnames with a trailing dot are not allowed".into(),
        ));
    }
    let host = raw_host.to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err(AppError::Other("Local network URLs are not allowed".into()));
    }
    let port = url
        .port_or_known_default()
        .ok_or_else(|| AppError::Other("URL has no usable port".into()))?;
    let literal_ip = host.parse::<IpAddr>().ok();
    let addresses = if let Some(ip) = literal_ip {
        vec![SocketAddr::new(ip, port)]
    } else {
        tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((host.as_str(), port)))
            .await
            .map_err(|_| AppError::Other(format!("DNS lookup for `{host}` timed out")))?
            .map_err(|error| AppError::Other(format!("Unable to resolve `{host}`: {error}")))?
            .collect::<Vec<_>>()
    };
    if addresses.is_empty() {
        return Err(AppError::Other(format!(
            "Host `{host}` did not resolve to an address"
        )));
    }
    let invalid_address = addresses.iter().any(|address| {
        !is_public_ip(address.ip())
            && !(literal_ip.is_none() && url.scheme() == "https" && is_tun_fake_ipv4(address.ip()))
    });
    if invalid_address {
        return Err(AppError::Other(
            "Local, private, and reserved network addresses are not allowed".into(),
        ));
    }
    Ok((host, addresses[0]))
}

fn is_tun_fake_ipv4(ip: IpAddr) -> bool {
    let IpAddr::V4(ip) = ip else {
        return false;
    };
    let [a, b, _, _] = ip.octets();
    a == 198 && (b == 18 || b == 19)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 240)
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(ipv4);
    }
    let segments = ip.segments();
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] == 0x0100 && segments[1..].iter().all(|segment| *segment == 0)))
}

fn normalize_language(language: Option<&str>) -> AppResult<Option<String>> {
    let Some(language) = language.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if language.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    if language.len() > 32
        || !language
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-')
    {
        return Err(AppError::Other(
            "`language` must be a BCP 47 tag such as zh-CN or en-US".into(),
        ));
    }
    Ok(Some(language.to_string()))
}

fn duckduckgo_region(language: &str) -> Option<&'static str> {
    match language.to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh-hans" => Some("cn-zh"),
        "en" | "en-us" => Some("us-en"),
        "en-gb" => Some("uk-en"),
        _ => None,
    }
}

fn duckduckgo_freshness(freshness: &str) -> Option<&'static str> {
    match freshness {
        "day" => Some("d"),
        "week" => Some("w"),
        "month" => Some("m"),
        "year" => Some("y"),
        _ => None,
    }
}

fn bing_freshness(freshness: &str) -> Option<String> {
    match freshness {
        "day" => Some("ex1:\"ez1\"".into()),
        "week" => Some("ex1:\"ez2\"".into()),
        "month" => Some("ex1:\"ez3\"".into()),
        "year" => {
            let today = chrono::Utc::now().timestamp().div_euclid(86_400);
            Some(format!("ex1:\"ez5_{}_{}\"", today - 365, today))
        }
        _ => None,
    }
}

fn brave_freshness(freshness: &str) -> Option<&'static str> {
    match freshness {
        "day" => Some("pd"),
        "week" => Some("pw"),
        "month" => Some("pm"),
        "year" => Some("py"),
        _ => None,
    }
}

fn parse_duckduckgo_results(html: &str, count: usize) -> Vec<WebSearchResult> {
    let document = Html::parse_document(html);
    let result_selector = Selector::parse(".result").expect("valid selector");
    let link_selector = Selector::parse(".result__a").expect("valid selector");
    let snippet_selector = Selector::parse(".result__snippet").expect("valid selector");
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for item in document.select(&result_selector) {
        let Some(link) = item.select(&link_selector).next() else {
            continue;
        };
        let title = element_text(link);
        let Some(url) = link.value().attr("href").and_then(normalize_duckduckgo_url) else {
            continue;
        };
        if title.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let snippet = item
            .select(&snippet_selector)
            .next()
            .map(element_text)
            .unwrap_or_default();
        results.push(WebSearchResult {
            rank: results.len() + 1,
            title,
            url,
            snippet,
        });
        if results.len() == count {
            break;
        }
    }
    results
}

fn parse_bing_results(html: &str, count: usize) -> Vec<WebSearchResult> {
    let document = Html::parse_document(html);
    let result_selector = Selector::parse("li.b_algo").expect("valid selector");
    let link_selector = Selector::parse("h2 a").expect("valid selector");
    let snippet_selector = Selector::parse(".b_caption p").expect("valid selector");
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for item in document.select(&result_selector) {
        let Some(link) = item.select(&link_selector).next() else {
            continue;
        };
        let title = element_text(link);
        let Some(url) = link.value().attr("href").and_then(normalize_bing_url) else {
            continue;
        };
        if title.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let snippet = item
            .select(&snippet_selector)
            .next()
            .map(element_text)
            .unwrap_or_default();
        results.push(WebSearchResult {
            rank: results.len() + 1,
            title,
            url,
            snippet,
        });
        if results.len() == count {
            break;
        }
    }
    results
}

fn parse_searxng_results(value: &Value, count: usize) -> Vec<WebSearchResult> {
    parse_json_results(
        value.get("results").and_then(Value::as_array),
        "title",
        "url",
        "content",
        count,
    )
}

fn parse_brave_results(value: &Value, count: usize) -> Vec<WebSearchResult> {
    parse_json_results(
        value
            .get("web")
            .and_then(|web| web.get("results"))
            .and_then(Value::as_array),
        "title",
        "url",
        "description",
        count,
    )
}

fn parse_json_results(
    items: Option<&Vec<Value>>,
    title_key: &str,
    url_key: &str,
    snippet_key: &str,
    count: usize,
) -> Vec<WebSearchResult> {
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for item in items.into_iter().flatten() {
        let title = item
            .get(title_key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let url = item
            .get(url_key)
            .and_then(Value::as_str)
            .and_then(normalized_public_url);
        let (Some(title), Some(url)) = (title, url) else {
            continue;
        };
        if url.len() > 4096 {
            continue;
        }
        if !seen.insert(url.clone()) {
            continue;
        }
        let title = truncate_chars(title, 500).0;
        let snippet = truncate_chars(
            &item
                .get(snippet_key)
                .and_then(Value::as_str)
                .map(normalize_plain_text)
                .unwrap_or_default(),
            2_000,
        )
        .0;
        results.push(WebSearchResult {
            rank: results.len() + 1,
            title,
            url,
            snippet,
        });
        if results.len() == count {
            break;
        }
    }
    results
}

fn normalize_duckduckgo_url(href: &str) -> Option<String> {
    let base = Url::parse("https://duckduckgo.com").ok()?;
    let parsed = base.join(href).ok()?;
    if parsed.path() == "/l/" {
        if let Some((_, target)) = parsed.query_pairs().find(|(key, _)| key == "uddg") {
            return normalized_public_url(&target);
        }
    }
    normalized_public_url(parsed.as_str())
}

fn normalize_bing_url(href: &str) -> Option<String> {
    let base = Url::parse("https://www.bing.com").ok()?;
    let parsed = base.join(href).ok()?;
    if parsed
        .host_str()
        .is_some_and(|host| host.ends_with("bing.com"))
        && parsed.path() == "/ck/a"
    {
        if let Some((_, value)) = parsed.query_pairs().find(|(key, _)| key == "u") {
            if let Some(encoded) = value.strip_prefix("a1") {
                if let Ok(decoded) = URL_SAFE_NO_PAD.decode(encoded.as_bytes()) {
                    if let Ok(target) = String::from_utf8(decoded) {
                        if let Some(url) = normalized_public_url(&target) {
                            return Some(url);
                        }
                    }
                }
            }
        }
    }
    normalized_public_url(parsed.as_str())
}

fn normalized_public_url(value: &str) -> Option<String> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.set_fragment(None);
    Some(url.to_string())
}

fn extract_html(html: &str) -> (Option<String>, String) {
    let document = Html::parse_document(html);
    let title_selector = Selector::parse("title").expect("valid selector");
    let candidate_selector =
        Selector::parse("article, main, [role=\"main\"]").expect("valid selector");
    let body_selector = Selector::parse("body").expect("valid selector");
    let title = document
        .select(&title_selector)
        .next()
        .map(element_text)
        .filter(|value| !value.is_empty());

    let best = document
        .select(&candidate_selector)
        .map(extract_blocks)
        .max_by_key(|text| text.len())
        .filter(|text| text.chars().count() >= 200)
        .or_else(|| document.select(&body_selector).next().map(extract_blocks))
        .unwrap_or_default();
    (title, best)
}

fn extract_blocks(root: ElementRef<'_>) -> String {
    let block_selector =
        Selector::parse("h1, h2, h3, h4, p, li, blockquote, pre, td").expect("valid selector");
    let mut lines = Vec::new();
    let mut previous = String::new();
    for element in root.select(&block_selector) {
        if has_ignored_ancestor(element) {
            continue;
        }
        let text = element_text(element);
        if text.is_empty() || text == previous {
            continue;
        }
        previous.clone_from(&text);
        lines.push(text);
    }
    if lines.is_empty() {
        normalize_plain_text(&root.text().collect::<Vec<_>>().join(" "))
    } else {
        lines.join("\n\n")
    }
}

fn has_ignored_ancestor(element: ElementRef<'_>) -> bool {
    element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .any(|ancestor| {
            matches!(
                ancestor.value().name(),
                "script" | "style" | "noscript" | "nav" | "header" | "footer" | "aside" | "form"
            )
        })
}

fn element_text(element: ElementRef<'_>) -> String {
    normalize_inline(&element.text().collect::<Vec<_>>().join(" "))
}

fn normalize_inline(value: &str) -> String {
    let mut output = String::new();
    for token in value.split_whitespace() {
        let first = token.chars().next();
        let previous = output.chars().next_back();
        let closes_punctuation = first.is_some_and(|value| {
            matches!(
                value,
                '.' | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | ')'
                    | ']'
                    | '}'
                    | '，'
                    | '。'
                    | '！'
                    | '？'
                    | '；'
                    | '：'
                    | '、'
                    | '》'
                    | '」'
                    | '』'
            )
        });
        let follows_opening = previous
            .is_some_and(|value| matches!(value, '(' | '[' | '{' | '“' | '‘' | '《' | '「' | '『'));
        if !output.is_empty() && !closes_punctuation && !follows_opening {
            output.push(' ');
        }
        output.push_str(token);
    }
    output
}

fn normalize_plain_text(value: &str) -> String {
    value
        .lines()
        .map(normalize_inline)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    match value.char_indices().nth(max_chars) {
        Some((end, _)) => (value[..end].trim_end().to_string(), true),
        None => (value.to_string(), false),
    }
}

fn decode_body(bytes: &[u8], content_type: &str) -> String {
    let encoding = content_type
        .split(';')
        .skip(1)
        .find_map(|part| part.trim().strip_prefix("charset="))
        .map(|label| label.trim_matches(['\'', '"']))
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .unwrap_or(UTF_8);
    encoding.decode(bytes).0.into_owned()
}

fn looks_like_html(value: &str) -> bool {
    let prefix = value.trim_start().chars().take(128).collect::<String>();
    let prefix = prefix.to_ascii_lowercase();
    prefix.starts_with("<!doctype html") || prefix.starts_with("<html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_reserved_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.2.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} must be rejected");
        }
        assert!(is_public_ip("1.1.1.1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(is_tun_fake_ipv4("198.18.1.10".parse().unwrap()));
        assert!(!is_tun_fake_ipv4("192.168.1.10".parse().unwrap()));
    }

    #[tokio::test]
    async fn rejects_unsafe_url_forms_before_requesting_them() {
        for value in [
            "http://localhost/admin",
            "https://user:secret@example.com/",
            "https://example.com./",
            "https://198.18.1.10/",
            "http://[::1]/",
        ] {
            let url = Url::parse(value).unwrap();
            assert!(
                validate_and_resolve(&url).await.is_err(),
                "{value} must be rejected"
            );
        }
    }

    #[test]
    fn parses_duckduckgo_results_and_unwraps_redirects() {
        let html = r#"
            <div class="result">
              <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fstory%3Fa%3D1">Example story</a>
              <a class="result__snippet">A useful <b>summary</b>.</a>
            </div>
        "#;
        let results = parse_duckduckgo_results(html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example story");
        assert_eq!(results[0].url, "https://example.com/story?a=1");
        assert_eq!(results[0].snippet, "A useful summary.");
    }

    #[test]
    fn parses_bing_results_and_unwraps_redirects() {
        let target = "https://example.org/article";
        let encoded = URL_SAFE_NO_PAD.encode(target.as_bytes());
        let html = format!(
            r#"<li class="b_algo"><h2><a href="https://www.bing.com/ck/a?u=a1{encoded}">Article</a></h2><div class="b_caption"><p>Summary</p></div></li>"#
        );
        let results = parse_bing_results(&html, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, target);
        assert_eq!(results[0].snippet, "Summary");
    }

    #[test]
    fn parses_searxng_and_brave_json_results() {
        let searxng = serde_json::json!({
            "results": [
                {"title": "Result A", "url": "https://example.com/a", "content": " First  summary. "},
                {"title": "Duplicate", "url": "https://example.com/a", "content": "ignored"},
                {"title": "Result B", "url": "https://example.com/b", "content": "Second summary."}
            ]
        });
        let searxng_results = parse_searxng_results(&searxng, 2);
        assert_eq!(searxng_results.len(), 2);
        assert_eq!(searxng_results[0].title, "Result A");
        assert_eq!(searxng_results[0].snippet, "First summary.");

        let brave = serde_json::json!({
            "web": {"results": [
                {"title": "Brave result", "url": "https://example.org/news", "description": "Current news"}
            ]}
        });
        let brave_results = parse_brave_results(&brave, 5);
        assert_eq!(brave_results.len(), 1);
        assert_eq!(brave_results[0].url, "https://example.org/news");
        assert_eq!(brave_results[0].snippet, "Current news");
    }

    #[test]
    fn search_settings_validate_order_and_searxng_endpoint() {
        let legacy: SearchProviderSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.fallback_order, vec!["duckduckgo", "bing"]);

        let mut settings = SearchProviderSettings {
            fallback_order: vec![
                " brave ".into(),
                "duckduckgo".into(),
                "brave".into(),
                "searxng".into(),
            ],
            searxng_base_url: Some(" http://127.0.0.1:8888/ ".into()),
        };
        settings.normalize().unwrap();
        assert_eq!(
            settings.fallback_order,
            vec!["brave", "duckduckgo", "searxng"]
        );
        assert_eq!(
            settings.searxng_base_url.as_deref(),
            Some("http://127.0.0.1:8888")
        );

        settings.fallback_order = vec!["unknown".into()];
        assert!(settings.normalize().is_err());
        settings.fallback_order = vec!["searxng".into()];
        settings.searxng_base_url = None;
        assert!(settings.normalize().is_err());
        assert!(normalize_searxng_base_url("ftp://example.com").is_err());
        assert!(normalize_searxng_base_url("https://user:secret@example.com").is_err());
        assert!(!serde_json::to_string(&settings)
            .unwrap()
            .contains("api_key"));
    }

    #[test]
    fn search_errors_are_reduced_to_stable_categories() {
        let timeout = classify_search_error(
            "duckduckgo",
            &AppError::Other("request timed out while sending secret details".into()),
        );
        assert_eq!(timeout.category, SearchFailureCategory::Timeout);
        let rate_limit = classify_search_error(
            "bing",
            &AppError::Other("Web request returned HTTP 429".into()),
        );
        assert_eq!(rate_limit.category, SearchFailureCategory::RateLimit);
        assert_eq!(
            status_failure(reqwest::StatusCode::UNAUTHORIZED).category,
            SearchFailureCategory::Authentication
        );
        assert_eq!(
            status_failure(reqwest::StatusCode::BAD_GATEWAY).category,
            SearchFailureCategory::ServiceUnavailable
        );
    }

    #[tokio::test]
    async fn searches_a_configured_local_searxng_instance() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /search?"));
            assert!(request.contains("q=local+test"));
            assert!(request.contains("format=json"));
            let body = r#"{"results":[{"title":"Local result","url":"https://example.com/result","content":"Local summary"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let settings = SearchProviderSettings {
            fallback_order: vec!["brave".into(), "searxng".into()],
            searxng_base_url: Some(format!("http://{address}")),
        };
        let response = search(
            "auto",
            "local test",
            3,
            Some("en-US"),
            None,
            5,
            &settings,
            None,
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(response.provider, "searxng");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].title, "Local result");
        assert_eq!(
            response.fallback_attempts,
            vec![SearchFallbackAttempt {
                provider: "brave".into(),
                category: SearchFailureCategory::InvalidConfig,
            }]
        );
    }

    #[test]
    fn extracts_main_content_without_navigation() {
        let html = r#"
          <html><head><title> Sample page </title></head><body>
            <nav><p>Navigation item</p></nav>
            <main><h1>Headline</h1><p>First paragraph with useful details.</p><p>Second paragraph.</p></main>
            <footer><p>Footer item</p></footer>
          </body></html>
        "#;
        let (title, content) = extract_html(html);
        assert_eq!(title.as_deref(), Some("Sample page"));
        assert!(content.contains("First paragraph"));
        assert!(!content.contains("Navigation item"));
        assert!(!content.contains("Footer item"));
    }

    #[test]
    fn validates_language_and_character_truncation() {
        assert_eq!(
            normalize_language(Some("zh-CN")).unwrap().as_deref(),
            Some("zh-CN")
        );
        assert!(normalize_language(Some("zh-CN\r\nX-Test")).is_err());
        assert_eq!(truncate_chars("中文abc", 3), ("中文a".into(), true));
    }

    #[tokio::test]
    #[ignore = "requires public network"]
    async fn searches_and_fetches_public_web() {
        let settings = SearchProviderSettings {
            fallback_order: vec!["brave".into(), "duckduckgo".into()],
            searxng_base_url: None,
        };
        let search = search(
            "auto",
            "Rust programming language official website",
            3,
            Some("en-US"),
            None,
            15,
            &settings,
            None,
        )
        .await
        .unwrap();
        assert!(!search.results.is_empty());
        assert_eq!(search.provider, "duckduckgo");
        assert_eq!(
            search.fallback_attempts,
            vec![SearchFallbackAttempt {
                provider: "brave".into(),
                category: SearchFailureCategory::InvalidConfig,
            }]
        );

        let page = fetch("https://www.rust-lang.org/", 5_000, 15)
            .await
            .unwrap();
        assert!(page.content.to_ascii_lowercase().contains("rust"));
        assert!(page.final_url.starts_with("https://"));
    }
}
