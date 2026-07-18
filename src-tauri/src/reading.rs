//! EPUB parsing for Read With AI indexing. Rendering remains in EPUB.js so CSS,
//! images, navigation, and CFI anchors stay faithful to the source publication.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};

use percent_encoding::percent_decode_str;
use roxmltree::{Document, Node};
use zip::ZipArchive;

use crate::error::{AppError, AppResult};

pub const MAX_EPUB_ARCHIVE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_EPUB_ENTRY_BYTES: u64 = 8 * 1024 * 1024;
const MAX_EPUB_TEXT_BYTES: usize = 24 * 1024 * 1024;
const MAX_EPUB_CHAPTERS: usize = 2_000;

#[derive(Debug, Clone)]
pub struct ParsedEpub {
    pub title: String,
    pub author: Option<String>,
    pub chapters: Vec<ParsedEpubChapter>,
}

#[derive(Debug, Clone)]
pub struct ParsedEpubChapter {
    pub title: String,
    pub text: String,
}

pub fn parse_epub_bytes(bytes: &[u8]) -> AppResult<ParsedEpub> {
    if bytes.len() as u64 > MAX_EPUB_ARCHIVE_BYTES {
        return Err(AppError::Other(format!(
            "EPUB exceeds the {} MiB import limit",
            MAX_EPUB_ARCHIVE_BYTES / 1024 / 1024
        )));
    }
    parse_epub_archive(Cursor::new(bytes))
}

fn parse_epub_archive<R: Read + Seek>(reader: R) -> AppResult<ParsedEpub> {
    let mut archive = ZipArchive::new(reader)
        .map_err(|error| AppError::Other(format!("Invalid EPUB archive: {error}")))?;
    let container = read_archive_text(&mut archive, "META-INF/container.xml")?;
    let container_document = parse_xml(&container, "EPUB container")?;
    let package_path = container_document
        .descendants()
        .find(|node| is_element_named(*node, "rootfile"))
        .and_then(|node| node.attribute("full-path"))
        .filter(|path| !path.trim().is_empty())
        .ok_or_else(|| {
            AppError::Other("EPUB container does not declare a package document".into())
        })?;
    let package_path = normalize_archive_path("", package_path)?;
    let package = read_archive_text(&mut archive, &package_path)?;
    let package_document = parse_xml(&package, "EPUB package")?;
    let package_dir = parent_archive_path(&package_path);

    let title = package_document
        .descendants()
        .find(|node| is_element_named(*node, "title"))
        .and_then(node_text)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Untitled EPUB".into());
    let author = package_document
        .descendants()
        .find(|node| is_element_named(*node, "creator"))
        .and_then(node_text)
        .filter(|text| !text.is_empty());

    let manifest = package_document
        .descendants()
        .filter(|node| is_element_named(*node, "item"))
        .filter_map(|node| {
            let id = node.attribute("id")?.trim();
            let href = node.attribute("href")?.trim();
            if id.is_empty() || href.is_empty() {
                return None;
            }
            Some((id.to_string(), href.to_string()))
        })
        .collect::<HashMap<_, _>>();
    let spine = package_document
        .descendants()
        .filter(|node| is_element_named(*node, "itemref"))
        .filter_map(|node| node.attribute("idref"))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if spine.is_empty() {
        return Err(AppError::Other("EPUB package has no readable spine".into()));
    }
    if spine.len() > MAX_EPUB_CHAPTERS {
        return Err(AppError::Other(format!(
            "EPUB contains more than {MAX_EPUB_CHAPTERS} spine items"
        )));
    }

    let mut chapters = Vec::new();
    let mut total_text_bytes = 0usize;
    for item_id in &spine {
        let Some(href) = manifest.get(*item_id) else {
            continue;
        };
        let locator = normalize_archive_path(&package_dir, href)?;
        let markup = read_archive_text(&mut archive, &locator)?;
        let (chapter_title, text) = extract_chapter_text(&markup, &locator)?;
        if text.is_empty() {
            continue;
        }
        total_text_bytes = total_text_bytes.saturating_add(text.len());
        if total_text_bytes > MAX_EPUB_TEXT_BYTES {
            return Err(AppError::Other(format!(
                "EPUB contains more than {} MiB of readable text",
                MAX_EPUB_TEXT_BYTES / 1024 / 1024
            )));
        }
        chapters.push(ParsedEpubChapter {
            title: chapter_title,
            text,
        });
    }

    if chapters.is_empty() {
        return Err(AppError::Other("EPUB has no indexable chapter text".into()));
    }
    Ok(ParsedEpub {
        title,
        author,
        chapters,
    })
}

fn read_archive_text<R: Read + Seek>(archive: &mut ZipArchive<R>, name: &str) -> AppResult<String> {
    let mut entry = archive
        .by_name(name)
        .map_err(|_| AppError::Other(format!("EPUB is missing `{name}`")))?;
    if entry.size() > MAX_EPUB_ENTRY_BYTES {
        return Err(AppError::Other(format!(
            "EPUB entry `{name}` exceeds the {} MiB limit",
            MAX_EPUB_ENTRY_BYTES / 1024 / 1024
        )));
    }
    let mut bytes = Vec::with_capacity(entry.size().min(MAX_EPUB_ENTRY_BYTES) as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::Other(format!("Unable to read EPUB entry `{name}`: {error}")))?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::Other(format!("EPUB entry `{name}` is not UTF-8 XML/XHTML")))
}

fn parse_xml<'a>(source: &'a str, label: &str) -> AppResult<Document<'a>> {
    Document::parse(source).map_err(|error| AppError::Other(format!("Invalid {label}: {error}")))
}

fn is_element_named(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

fn parent_archive_path(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_default()
}

fn normalize_archive_path(base: &str, href: &str) -> AppResult<String> {
    let href = href.split(['#', '?']).next().unwrap_or_default().trim();
    if href.is_empty() || href.starts_with('/') || href.contains('\0') {
        return Err(AppError::Other(
            "EPUB contains an invalid archive path".into(),
        ));
    }
    let decoded = percent_decode_str(href)
        .decode_utf8()
        .map_err(|_| AppError::Other("EPUB path is not valid UTF-8".into()))?;
    let mut components = Vec::new();
    for component in base.split('/').chain(decoded.split('/')) {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(AppError::Other("EPUB path escapes its archive root".into()));
                }
            }
            component => components.push(component),
        }
    }
    if components.is_empty() {
        return Err(AppError::Other(
            "EPUB contains an empty archive path".into(),
        ));
    }
    Ok(components.join("/"))
}

fn extract_chapter_text(markup: &str, locator: &str) -> AppResult<(String, String)> {
    let document = match Document::parse(markup) {
        Ok(document) => document,
        Err(_) => return Ok((fallback_chapter_title(locator), strip_markup(markup))),
    };
    let title = document
        .descendants()
        .find(|node| is_element_named(*node, "title"))
        .and_then(node_text)
        .or_else(|| {
            document
                .descendants()
                .find(|node| is_element_named(*node, "h1"))
                .and_then(node_text)
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| fallback_chapter_title(locator));
    let body = document
        .descendants()
        .find(|node| is_element_named(*node, "body"))
        .unwrap_or_else(|| document.root_element());
    let mut text = String::new();
    collect_readable_text(body, &mut text);
    Ok((title, normalize_readable_text(&text)))
}

fn node_text(node: Node<'_, '_>) -> Option<String> {
    let mut text = String::new();
    for descendant in node.descendants().filter(|item| item.is_text()) {
        if let Some(value) = descendant.text() {
            append_normalized_text(&mut text, value);
        }
    }
    let text = normalize_readable_text(&text);
    (!text.is_empty()).then_some(text)
}

fn collect_readable_text(node: Node<'_, '_>, output: &mut String) {
    if node.is_text() {
        if let Some(value) = node.text() {
            append_normalized_text(output, value);
        }
        return;
    }
    if node.is_element() {
        let name = node.tag_name().name();
        if matches!(name, "head" | "script" | "style" | "svg") {
            return;
        }
        if is_block_element(name) && !output.trim().is_empty() {
            append_paragraph_break(output);
        }
    }
    for child in node.children() {
        collect_readable_text(child, output);
    }
    if node.is_element() && is_block_element(node.tag_name().name()) {
        append_paragraph_break(output);
    }
}

fn is_block_element(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "figcaption"
            | "figure"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "li"
            | "main"
            | "p"
            | "section"
    )
}

fn append_normalized_text(output: &mut String, value: &str) {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.is_empty() {
        return;
    }
    if !output.is_empty() && !output.chars().last().is_some_and(char::is_whitespace) {
        output.push(' ');
    }
    output.push_str(&value);
}

fn append_paragraph_break(output: &mut String) {
    while output.chars().last().is_some_and(char::is_whitespace) {
        output.pop();
    }
    if !output.is_empty() && !output.ends_with("\n\n") {
        output.push_str("\n\n");
    }
}

fn normalize_readable_text(value: &str) -> String {
    value
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn fallback_chapter_title(locator: &str) -> String {
    locator
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.').map(|(stem, _)| stem))
        .filter(|name| !name.is_empty())
        .unwrap_or("Chapter")
        .to_string()
}

fn strip_markup(value: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => {
                in_tag = true;
                append_paragraph_break(&mut output);
            }
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    normalize_readable_text(&output)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;

    use super::*;

    fn fixture_epub() -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default();
        for (path, content) in [
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#,
            ),
            (
                "OPS/book.opf",
                r#"<?xml version="1.0"?><package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>Example Book</dc:title><dc:creator>Reader</dc:creator></metadata><manifest><item id="one" href="text/chapter-1.xhtml"/></manifest><spine><itemref idref="one"/></spine></package>"#,
            ),
            (
                "OPS/text/chapter-1.xhtml",
                r#"<?xml version="1.0"?><html><head><title>Chapter One</title></head><body><h1>Chapter One</h1><p>First paragraph.</p><p>Second paragraph.</p></body></html>"#,
            ),
        ] {
            archive.start_file(path, options).unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn parses_spine_metadata_and_readable_text() {
        let parsed = parse_epub_bytes(&fixture_epub()).unwrap();
        assert_eq!(parsed.title, "Example Book");
        assert_eq!(parsed.author.as_deref(), Some("Reader"));
        assert_eq!(parsed.chapters.len(), 1);
        assert_eq!(parsed.chapters[0].title, "Chapter One");
        assert_eq!(
            parsed.chapters[0].text,
            "Chapter One\n\nFirst paragraph.\n\nSecond paragraph."
        );
    }

    #[test]
    fn prevents_archive_root_escape() {
        assert!(normalize_archive_path("OPS", "../../outside.xhtml").is_err());
    }
}
