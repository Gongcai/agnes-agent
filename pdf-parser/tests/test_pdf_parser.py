import os
from pathlib import Path

import pytest
from reportlab.pdfgen import canvas

from app.parser import DocumentParseError, parse_document


def test_pdf_requires_an_installed_model_package(tmp_path: Path) -> None:
    path = tmp_path / "report.pdf"
    document = canvas.Canvas(str(path))
    document.drawString(72, 720, "Agnes PDF parser")
    document.save()

    with pytest.raises(DocumentParseError, match="模型包尚未安装"):
        parse_document(str(path))


@pytest.mark.skipif(
    not os.environ.get("AGNES_PDF_MODELS"),
    reason="requires downloaded PDF model artifacts",
)
def test_pdf_parser_extracts_page_text_with_local_models(tmp_path: Path) -> None:
    path = tmp_path / "offline.pdf"
    document = canvas.Canvas(str(path))
    document.setFont("Helvetica", 16)
    document.drawString(72, 720, "Offline PDF parsing")
    document.setFont("Helvetica", 11)
    document.drawString(72, 690, "No remote services are required.")
    document.save()

    result = parse_document(
        str(path),
        artifacts_path=os.environ["AGNES_PDF_MODELS"],
    )

    assert result["media_type"] == "application/pdf"
    assert result["parser_profile"]["name"] == "docling_pdf_local"
    assert any("Offline PDF parsing" in chunk["content"] for chunk in result["chunks"])
    assert any(chunk["page"] == 1 for chunk in result["chunks"])
