from pathlib import Path

from docx import Document
from openpyxl import Workbook
from pptx import Presentation

from app.parser import PARSER_PROFILE, parse_document


def test_docx_preserves_heading_and_table_metadata(tmp_path: Path) -> None:
    path = tmp_path / "report.docx"
    document = Document()
    document.add_heading("实验结果", level=1)
    document.add_paragraph("苹果样本表现稳定。")
    table = document.add_table(rows=2, cols=2)
    table.cell(0, 0).text = "名称"
    table.cell(0, 1).text = "数量"
    table.cell(1, 0).text = "苹果"
    table.cell(1, 1).text = "3"
    document.save(path)

    result = parse_document(str(path))

    assert result["parser_profile"] == PARSER_PROFILE
    assert result["media_type"].endswith("wordprocessingml.document")
    assert any(chunk["section_path"] == "实验结果" for chunk in result["chunks"])
    table_chunk = next(chunk for chunk in result["chunks"] if chunk["metadata"]["kind"] == "table")
    assert "苹果" in table_chunk["content"]
    assert table_chunk["page"] is None


def test_office_media_type_hint_supports_extensionless_staged_files(tmp_path: Path) -> None:
    path = tmp_path / "staged-source"
    document = Document()
    document.add_paragraph("网盘暂存文件仍可解析。")
    document.save(path)

    result = parse_document(
        str(path),
        "remote.docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    )

    assert result["title"] == "remote.docx"
    assert "网盘暂存文件" in result["chunks"][0]["content"]


def test_pptx_uses_slide_number_and_title(tmp_path: Path) -> None:
    path = tmp_path / "slides.pptx"
    presentation = Presentation()
    slide = presentation.slides.add_slide(presentation.slide_layouts[1])
    slide.shapes.title.text = "系统架构"
    slide.placeholders[1].text = "桌面端\n同步服务"
    presentation.save(path)

    result = parse_document(str(path), "演示文稿")

    chunk = next(chunk for chunk in result["chunks"] if chunk["metadata"]["kind"] == "slide")
    assert result["title"] == "演示文稿"
    assert chunk["page"] == 1
    assert chunk["section_path"] == "系统架构"
    assert "桌面端" in chunk["content"]


def test_xlsx_preserves_sheet_and_splits_large_tables(tmp_path: Path) -> None:
    path = tmp_path / "sales.xlsx"
    workbook = Workbook()
    sheet = workbook.active
    sheet.title = "销售数据"
    sheet.append(["产品", "销量"])
    for index in range(90):
        sheet.append([f"产品-{index}", index])
    workbook.save(path)

    result = parse_document(str(path))

    table_chunks = [chunk for chunk in result["chunks"] if chunk["metadata"]["kind"] == "table"]
    assert len(table_chunks) >= 3
    assert all(chunk["page"] == 1 for chunk in table_chunks)
    assert all(chunk["section_path"] == "销售数据" for chunk in table_chunks)
    assert all(chunk["metadata"]["sheet"] == "销售数据" for chunk in table_chunks)
    assert all("产品" in chunk["content"] for chunk in table_chunks)
