from __future__ import annotations

import hashlib
import json
import math
import mimetypes
import sys
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from docling.backend.msexcel_backend import MsExcelDocumentBackend
from docling.backend.mspowerpoint_backend import MsPowerpointDocumentBackend
from docling.backend.msword_backend import MsWordDocumentBackend
from docling.datamodel.backend_options import (
    MsExcelBackendOptions,
    MsPowerpointBackendOptions,
)
from docling.datamodel.base_models import InputFormat
from docling.datamodel.document import InputDocument
from docling_core.types.doc import DoclingDocument, GroupItem, TableItem


def _configure_frozen_docx_templates() -> None:
    if not getattr(sys, "frozen", False):
        return
    from docx.parts.comments import CommentsPart
    from docx.parts.hdrftr import FooterPart, HeaderPart

    header = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<w:hdr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        b"<w:p/></w:hdr>"
    )
    footer = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<w:ftr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        b"<w:p/></w:ftr>"
    )
    comments = (
        b'<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        b'<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>'
    )
    HeaderPart._default_header_xml = classmethod(lambda _cls: header)
    FooterPart._default_footer_xml = classmethod(lambda _cls: footer)
    CommentsPart._default_comments_xml = classmethod(lambda _cls: comments)


_configure_frozen_docx_templates()

SCHEMA_VERSION = 1
DOCLING_VERSION = "2.113.0"
MAX_FILE_BYTES = 50 * 1024 * 1024
MAX_UNCOMPRESSED_BYTES = 500 * 1024 * 1024
MAX_ZIP_MEMBERS = 20_000
MAX_CHUNK_CHARS = 1_200
CHUNK_OVERLAP_CHARS = 200
MAX_TABLE_ROWS_PER_CHUNK = 40

MEDIA_TYPES = {
    ".docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ".pptx": "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ".xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
}

PROFILE_OPTIONS = {
    "chunk_chars": MAX_CHUNK_CHARS,
    "chunk_overlap_chars": CHUNK_OVERLAP_CHARS,
    "max_table_rows": MAX_TABLE_ROWS_PER_CHUNK,
    "render_chart_images": False,
    "parse_charts": True,
}
PROFILE_OPTIONS_HASH = hashlib.sha256(
    json.dumps(PROFILE_OPTIONS, sort_keys=True, separators=(",", ":")).encode()
).hexdigest()
PARSER_PROFILE = {
    "id": f"docling-office-{DOCLING_VERSION}-structured-v1",
    "name": "docling_office",
    "version": DOCLING_VERSION,
    "options_hash": PROFILE_OPTIONS_HASH,
}


class DocumentParseError(ValueError):
    pass


@dataclass
class ParsedChunk:
    content: str
    page: int | None
    section_path: str | None
    metadata: dict[str, Any]

    def to_dict(self) -> dict[str, Any]:
        return {
            "content": self.content,
            "page": self.page,
            "section_path": self.section_path,
            "token_count": estimate_tokens(self.content),
            "metadata": self.metadata,
        }


def estimate_tokens(content: str) -> int:
    word_estimate = len(content.split())
    character_estimate = math.ceil(len(content) / 4)
    return max(1, word_estimate, character_estimate)


def _preflight_ooxml(path: Path) -> None:
    size = path.stat().st_size
    if size > MAX_FILE_BYTES:
        raise DocumentParseError("文件超过 50 MiB 的 Office 导入上限")
    if not zipfile.is_zipfile(path):
        raise DocumentParseError("Office 文件不是有效的 OOXML ZIP 容器")

    with zipfile.ZipFile(path) as archive:
        members = archive.infolist()
        if len(members) > MAX_ZIP_MEMBERS:
            raise DocumentParseError("Office 文件包含过多 ZIP 成员")
        uncompressed = sum(member.file_size for member in members)
        if uncompressed > MAX_UNCOMPRESSED_BYTES:
            raise DocumentParseError("Office 文件解压后超过 500 MiB 上限")
        for member in members:
            member_path = PurePosixPath(member.filename)
            if member_path.is_absolute() or ".." in member_path.parts:
                raise DocumentParseError("Office 文件包含不安全的 ZIP 路径")
            if member.flag_bits & 0x1:
                raise DocumentParseError("不支持密码加密的 Office 文件")
            if member.file_size > 0 and member.compress_size == 0:
                raise DocumentParseError("Office 文件包含异常压缩成员")
            if member.compress_size > 0 and member.file_size / member.compress_size > 2_000:
                raise DocumentParseError("Office 文件压缩比异常，已拒绝解析")


def _suffix_for(path: Path, media_type_hint: str | None) -> str:
    suffix = path.suffix.lower()
    if suffix in MEDIA_TYPES:
        return suffix
    normalized = (media_type_hint or "").split(";", 1)[0].strip().lower()
    for candidate, media_type in MEDIA_TYPES.items():
        if normalized == media_type:
            return candidate
    return suffix


def _backend_for(suffix: str):
    if suffix == ".docx":
        return InputFormat.DOCX, MsWordDocumentBackend, None
    if suffix == ".pptx":
        options = MsPowerpointBackendOptions(render_chart_images=False)
        return InputFormat.PPTX, MsPowerpointDocumentBackend, options
    if suffix == ".xlsx":
        options = MsExcelBackendOptions(
            treat_singleton_as_text=True,
            parse_charts=True,
            render_chart_images=False,
            gap_tolerance=0,
            sheet_names=None,
        )
        return InputFormat.XLSX, MsExcelDocumentBackend, options
    raise DocumentParseError("当前仅支持 DOCX、PPTX 和 XLSX 文件")


def _convert(path: Path, suffix: str) -> DoclingDocument:
    input_format, backend_type, options = _backend_for(suffix)
    input_document = InputDocument(
        path,
        format=input_format,
        backend=backend_type,
        backend_options=options,
    )
    backend = backend_type(input_document, path, options) if options else backend_type(input_document, path)
    if not backend.is_valid():
        raise DocumentParseError("Office 文件损坏、加密或无法解析")
    try:
        return backend.convert()
    finally:
        backend.unload()


def _page_number(item: Any) -> int | None:
    for provenance in getattr(item, "prov", []):
        page = getattr(provenance, "page_no", None)
        if isinstance(page, int) and page > 0:
            return page
    return None


def _label(item: Any) -> str:
    label = getattr(item, "label", None)
    return str(getattr(label, "value", label or "unspecified"))


def _markdown_table_parts(markdown: str) -> list[tuple[str, int, int]]:
    lines = [line.rstrip() for line in markdown.strip().splitlines() if line.strip()]
    if len(lines) <= 2:
        return [(markdown.strip(), 1, max(1, len(lines)))]
    header = lines[:2]
    rows = lines[2:]
    parts: list[tuple[str, int, int]] = []
    start = 0
    while start < len(rows):
        selected: list[str] = []
        while start + len(selected) < len(rows) and len(selected) < MAX_TABLE_ROWS_PER_CHUNK:
            candidate = selected + [rows[start + len(selected)]]
            rendered = "\n".join(header + candidate)
            if selected and len(rendered) > MAX_CHUNK_CHARS:
                break
            selected = candidate
        if not selected:
            selected = [rows[start]]
        row_start = start + 2
        row_end = row_start + len(selected) - 1
        parts.append(("\n".join(header + selected), row_start, row_end))
        start += len(selected)
    return parts


def _split_long_text(content: str) -> list[str]:
    if len(content) <= MAX_CHUNK_CHARS:
        return [content]
    parts: list[str] = []
    start = 0
    while start < len(content):
        end = min(len(content), start + MAX_CHUNK_CHARS)
        if end < len(content):
            boundary = max(content.rfind("\n", start, end), content.rfind("。", start, end))
            if boundary > start + MAX_CHUNK_CHARS // 2:
                end = boundary + 1
        part = content[start:end].strip()
        if part:
            parts.append(part)
        if end >= len(content):
            break
        start = max(start + 1, end - CHUNK_OVERLAP_CHARS)
    return parts


def _document_chunks(document: DoclingDocument, suffix: str) -> list[ParsedChunk]:
    chunks: list[ParsedChunk] = []
    buffer: list[str] = []
    buffer_page: int | None = None
    buffer_section: str | None = None
    headings: list[str] = []
    group_stack: list[tuple[int, str, str]] = []
    current_slide_title: dict[int, str] = {}

    def current_sheet() -> str | None:
        for _, label, name in reversed(group_stack):
            if label == "sheet" and name:
                return name
        return None

    def flush() -> None:
        nonlocal buffer, buffer_page, buffer_section
        content = "\n\n".join(part for part in buffer if part.strip()).strip()
        if content:
            metadata: dict[str, Any] = {"kind": "section", "format": suffix[1:]}
            if suffix == ".pptx" and buffer_page:
                metadata = {
                    "kind": "slide",
                    "format": "pptx",
                    "slide_number": buffer_page,
                }
            for part in _split_long_text(content):
                chunks.append(ParsedChunk(part, buffer_page, buffer_section, metadata.copy()))
        buffer = []
        buffer_page = None
        buffer_section = None

    for item, level in document.iterate_items(with_groups=True):
        group_stack[:] = [entry for entry in group_stack if entry[0] < level]
        if isinstance(item, GroupItem):
            group_label = _label(item)
            if suffix == ".pptx" and group_label == "chapter":
                flush()
                headings.clear()
            group_stack.append(
                (level, group_label, str(getattr(item, "name", "") or ""))
            )
            continue

        label = _label(item)
        page = _page_number(item)
        sheet = current_sheet()
        text = str(getattr(item, "text", "") or "").strip()

        if label in {"title", "section_header"} and text:
            flush()
            heading_level = int(getattr(item, "level", 1) or 1)
            heading_level = min(max(heading_level, 1), 6)
            headings[:] = headings[: heading_level - 1]
            headings.append(text)
            if page:
                current_slide_title[page] = text
            buffer = [f"{'#' * heading_level} {text}"]
            buffer_page = page
            buffer_section = " / ".join(headings)
            continue

        section = sheet or (current_slide_title.get(page) if page else None) or (
            " / ".join(headings) if headings else None
        )

        if isinstance(item, TableItem):
            flush()
            markdown = item.export_to_markdown(document).strip()
            for part, row_start, row_end in _markdown_table_parts(markdown):
                metadata: dict[str, Any] = {
                    "kind": "table",
                    "format": suffix[1:],
                    "row_start": row_start,
                    "row_end": row_end,
                }
                if sheet:
                    metadata.update({"sheet": sheet, "header_rows": 1})
                chunks.append(ParsedChunk(part, page, section, metadata))
            continue

        if not text:
            continue
        if label == "list_item":
            text = f"- {text}"

        if buffer and (buffer_page != page or buffer_section != section):
            flush()
        if not buffer:
            buffer_page = page
            buffer_section = section
        candidate = "\n\n".join(buffer + [text])
        if buffer and len(candidate) > MAX_CHUNK_CHARS:
            flush()
            buffer_page = page
            buffer_section = section
        buffer.append(text)

    flush()
    return chunks


def parse_document(
    path_value: str,
    title_hint: str | None = None,
    media_type_hint: str | None = None,
) -> dict[str, Any]:
    path = Path(path_value).resolve()
    if not path.is_file():
        raise DocumentParseError("知识库导入路径必须是普通文件")
    suffix = _suffix_for(path, media_type_hint)
    if suffix not in MEDIA_TYPES:
        raise DocumentParseError("当前仅支持 DOCX、PPTX 和 XLSX 文件")
    _preflight_ooxml(path)
    document = _convert(path, suffix)
    chunks = _document_chunks(document, suffix)
    if not chunks:
        raise DocumentParseError("文件没有可索引的文本内容")

    title = (title_hint or "").strip() or path.stem.strip() or "未命名文档"
    source_hash = hashlib.sha256(path.read_bytes()).hexdigest()
    return {
        "schema_version": SCHEMA_VERSION,
        "title": title,
        "media_type": MEDIA_TYPES.get(suffix) or mimetypes.guess_type(path.name)[0],
        "source_hash": source_hash,
        "size": path.stat().st_size,
        "parser_profile": PARSER_PROFILE,
        "chunks": [chunk.to_dict() for chunk in chunks],
    }
