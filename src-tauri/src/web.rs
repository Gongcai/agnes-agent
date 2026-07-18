use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use encoding_rs::{Encoding, UTF_8};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

use crate::error::{AppError, AppResult};

const SEARCH_BODY_LIMIT: usize = 2 * 1024 * 1024;
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

pub async fn search(
    provider: &str,
    query: &str,
    count: usize,
    language: Option<&str>,
    freshness: Option<&str>,
    timeout_sec: u32,
) -> AppResult<WebSearchResponse> {
    let language = normalize_language(language)?;
    match provider {
        "duckduckgo" => {
            search_duckduckgo(query, count, language.as_deref(), freshness, timeout_sec).await
        }
        "bing" => search_bing(query, count, language.as_deref(), freshness, timeout_sec).await,
        "auto" | "" => {
            let duckduckgo =
                search_duckduckgo(query, count, language.as_deref(), freshness, timeout_sec).await;
            if let Ok(response) = &duckduckgo {
                if !response.results.is_empty() {
                    return Ok(response.clone());
                }
            }

            let bing = search_bing(query, count, language.as_deref(), freshness, timeout_sec).await;
            match bing {
                Ok(response) => Ok(response),
                Err(bing_error) => match duckduckgo {
                    Ok(response) => Ok(response),
                    Err(duckduckgo_error) => Err(AppError::Other(format!(
                        "Web search failed with DuckDuckGo ({duckduckgo_error}) and Bing ({bing_error})"
                    ))),
                },
            }
        }
        other => Err(AppError::Other(format!(
            "Unsupported web search provider `{other}`"
        ))),
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
        let search = search(
            "duckduckgo",
            "Rust programming language official website",
            3,
            Some("en-US"),
            None,
            15,
        )
        .await
        .unwrap();
        assert!(!search.results.is_empty());

        let page = fetch("https://www.rust-lang.org/", 5_000, 15)
            .await
            .unwrap();
        assert!(page.content.to_ascii_lowercase().contains("rust"));
        assert!(page.final_url.starts_with("https://"));
    }
}
