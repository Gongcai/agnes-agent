"""Create an Agnes voice with the CosyVoice Voice Design API.

The generated voice is model-specific. This script deliberately targets
cosyvoice-v3.5-flash instead of reusing a v3 preset voice.

Usage:
  DASHSCOPE_API_KEY=... uv run python -m app.voice.design_voice

For a workspace endpoint, set DASHSCOPE_VOICE_DESIGN_ENDPOINT or
DASHSCOPE_WORKSPACE_ID. The script writes the preview to the current directory
and prints the voice_id; it never prints the API key.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


TARGET_MODEL = "cosyvoice-v3.5-flash"
DEFAULT_ENDPOINT = "https://dashscope.aliyuncs.com/api/v1/services/audio/tts/customization"
DEFAULT_PREFIX = "agnes"
DEFAULT_PREVIEW_TEXT = (
    "你好，我是 Agnes。今天我们一起把事情理清楚，我会用清晰、自然的语气陪你完成接下来的工作。"
)
DEFAULT_VOICE_PROMPT = (
    "一位三十岁左右的女性语音助手，中音偏低，音色温暖清澈，细腻而有质感；"
    "语速中等偏慢，吐字清晰自然，停顿短而从容；情绪稳定、耐心、亲切，"
    "带有轻微的微笑感，但不过分活泼或夸张；适合长时间的中文对话、解释复杂问题、"
    "提醒和陪伴式交互。避免播音腔、广告腔、尖锐高音和过度戏剧化。"
)


def endpoint_from_env() -> str:
    explicit = os.environ.get("DASHSCOPE_VOICE_DESIGN_ENDPOINT")
    if explicit:
        return explicit
    workspace_id = os.environ.get("DASHSCOPE_WORKSPACE_ID")
    if workspace_id:
        return (
            f"https://{workspace_id}.cn-beijing.maas.aliyuncs.com"
            "/api/v1/services/audio/tts/customization"
        )
    return DEFAULT_ENDPOINT


def decode_preview(value: object) -> bytes | None:
    if isinstance(value, str):
        raw = value.split(",", 1)[-1] if value.startswith("data:") else value
        return base64.b64decode(raw)
    if isinstance(value, dict):
        return decode_preview(value.get("data"))
    return None


def create_voice(
    api_key: str,
    endpoint: str,
    voice_prompt: str,
    preview_text: str,
    prefix: str,
) -> tuple[str, bytes | None, str | None, dict]:
    if len(voice_prompt) > 500:
        raise ValueError("CosyVoice voice_prompt must be at most 500 characters")
    if not re.fullmatch(r"[a-z0-9]{1,10}", prefix):
        raise ValueError("prefix must contain 1-10 lowercase letters or digits")
    payload = {
        "model": "voice-enrollment",
        "input": {
            "action": "create_voice",
            "target_model": TARGET_MODEL,
            "voice_prompt": voice_prompt,
            "preview_text": preview_text,
            "prefix": prefix,
        },
        "parameters": {"sample_rate": 24000, "response_format": "wav"},
    }
    request = Request(
        endpoint,
        data=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urlopen(request, timeout=60) as response:
            raw = json.loads(response.read().decode("utf-8"))
    except (HTTPError, URLError, TimeoutError, json.JSONDecodeError) as exc:
        detail = getattr(exc, "read", lambda: b"")()
        raise RuntimeError(f"CosyVoice voice design request failed: {exc}; {detail!r}") from exc
    output = raw.get("output") or {}
    voice_id = output.get("voice_id") or output.get("voice")
    if not voice_id:
        raise RuntimeError(f"Voice design response has no voice id: {raw}")
    preview = decode_preview(output.get("preview_audio"))
    return str(voice_id), preview, raw.get("request_id"), raw


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--voice-prompt", default=DEFAULT_VOICE_PROMPT)
    parser.add_argument("--preview-text", default=DEFAULT_PREVIEW_TEXT)
    parser.add_argument("--prefix", default=DEFAULT_PREFIX)
    parser.add_argument("--preview-output", type=Path, default=Path("agnes-voice-preview.wav"))
    args = parser.parse_args()
    api_key = os.environ.get("DASHSCOPE_API_KEY")
    if not api_key:
        raise SystemExit("DASHSCOPE_API_KEY is required")

    voice_id, preview, request_id, raw = create_voice(
        api_key,
        endpoint_from_env(),
        args.voice_prompt,
        args.preview_text,
        args.prefix,
    )
    if preview:
        args.preview_output.write_bytes(preview)
        print(f"preview={args.preview_output} bytes={len(preview)}")
    print(f"request_id={request_id or '-'}")
    print(f"target_model={TARGET_MODEL}")
    print(f"voice_id={voice_id}")
    print("export DASHSCOPE_TTS_VOICE=" + voice_id)
    # Keep the full response out of normal output because it may contain a
    # large preview payload. It is available for debugging when requested.
    if os.environ.get("DASHSCOPE_VOICE_DESIGN_DEBUG") == "1":
        print(json.dumps(raw, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
