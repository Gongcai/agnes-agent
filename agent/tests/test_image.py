"""Image attachment fallback and multimodal prompt tests."""
from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import pytest

from app import image as image_module
from app.image import attachment_data_url, process_image_attachments
from app.prompt import translate_messages


def _snapshot(root: Path, *, supports_image: bool = False) -> dict:
    cached = root / ".agnes" / "cache" / "attachments" / "image-1" / "sample.png"
    cached.parent.mkdir(parents=True)
    cached.write_bytes(b"fake image bytes")
    metadata = {
        "attachmentKind": "local_file",
        "id": "image-1",
        "name": "sample.png",
        "path": ".agnes/cache/attachments/image-1/sample.png",
        "mediaType": "image/png",
    }
    return {
        "context": {
            "attachmentRoot": str(root),
            "llmConfig": {"supportsImageInput": supports_image},
            "recentMessages": [{
                "role": "user",
                "parts": [
                    {"kind": "text", "content": "请看这张图"},
                    {"kind": "attachment", "content": "", "metadata": metadata},
                ],
            }],
            "attachmentsContext": [{"kind": "local_file", **metadata, "content": ""}],
        }
    }


def test_attachment_data_url_rejects_paths_outside_workspace(tmp_path: Path):
    outside = tmp_path.parent / "outside.png"
    outside.write_bytes(b"not allowed")
    with pytest.raises(ValueError, match="超出"):
        attachment_data_url(str(tmp_path), "../outside.png", "image/png")


def test_process_image_attachments_uses_ocr_prompt_and_records_result(tmp_path, monkeypatch):
    snapshot = _snapshot(tmp_path)
    calls = []

    def fake_completion(model, messages, llm_config, temperature):
        calls.append((model, messages, llm_config, temperature))
        return SimpleNamespace(
            choices=[SimpleNamespace(message=SimpleNamespace(content="识别到的文字"))]
        )

    monkeypatch.setattr(image_module, "completion", fake_completion)
    prepared = process_image_attachments(
        snapshot,
        {
            "modelRef": "openai/ocr-model",
            "model": "ocr-model",
            "litellmModel": "openai/ocr-model",
            "supportsImageInput": True,
            "imageProcessingMode": "ocr",
        },
    )

    assert len(calls) == 1
    assert "OCR" in calls[0][1][0]["content"][0]["text"]
    assert calls[0][1][0]["content"][1]["image_url"]["url"].startswith("data:image/png;base64,")
    part_metadata = prepared["context"]["recentMessages"][0]["parts"][1]["metadata"]
    assert part_metadata["processedText"] == "识别到的文字"
    assert part_metadata["processedMode"] == "ocr"
    assert prepared["context"]["attachmentsContext"][0]["processedModel"] == "openai/ocr-model"

    process_image_attachments(
        snapshot,
        {
            "modelRef": "openai/ocr-model",
            "model": "ocr-model",
            "litellmModel": "openai/ocr-model",
            "supportsImageInput": True,
            "imageProcessingMode": "ocr",
        },
    )
    assert len(calls) == 1


def test_process_image_attachments_skips_images_for_vision_main_model(tmp_path, monkeypatch):
    snapshot = _snapshot(tmp_path, supports_image=True)
    monkeypatch.setattr(image_module, "completion", lambda **_: pytest.fail("不应调用图片处理模型"))
    assert process_image_attachments(snapshot, None) is snapshot


def test_translate_messages_sends_cached_image_to_vision_model(tmp_path: Path):
    snapshot = _snapshot(tmp_path, supports_image=True)
    translated = translate_messages(
        snapshot["context"]["recentMessages"],
        snapshot["context"]["attachmentRoot"],
        True,
    )
    content = translated[0]["content"]
    assert content[0]["type"] == "text"
    assert content[1]["type"] == "image_url"
    assert "base64" in content[1]["image_url"]["url"]


def test_process_image_attachments_requires_config_for_text_only_main(tmp_path: Path):
    with pytest.raises(ValueError, match="配置.*图片处理模型"):
        process_image_attachments(_snapshot(tmp_path), None)
