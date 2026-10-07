"""DashScope Qwen Audio ASR adapter.

The supplied qwen-audio-3.1-asr-flash example is an HTTP utterance API, so this
adapter intentionally exposes a complete-utterance operation. It does not
claim partial transcript support.
"""

from __future__ import annotations

import base64
import json
import os
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from .base import AudioFormat, AsrResult


DEFAULT_ASR_ENDPOINT = (
    "https://llm-srukcv1xajun2e5g.cn-beijing.maas.aliyuncs.com"
    "/api/v1/services/aigc/multimodal-generation/generation"
)


class DashScopeAsrError(RuntimeError):
    """A normalized ASR provider error."""


class DashScopeAsrProvider:
    model = "qwen-audio-3.1-asr-flash"
    streaming = False

    def __init__(
        self,
        api_key: str | None = None,
        endpoint: str | None = None,
        model: str | None = None,
    ) -> None:
        self.api_key = api_key or os.environ.get("DASHSCOPE_API_KEY")
        self.endpoint = endpoint or os.environ.get("DASHSCOPE_ASR_ENDPOINT", DEFAULT_ASR_ENDPOINT)
        self.model = model or type(self).model
        if not self.api_key:
            raise ValueError("DASHSCOPE_API_KEY is required for Qwen Audio ASR")

    async def transcribe(
        self,
        audio: bytes,
        *,
        audio_format: AudioFormat = AudioFormat.WAV,
        sample_rate: int = 16000,
    ) -> AsrResult:
        import asyncio

        return await asyncio.to_thread(
            self._transcribe_sync,
            audio,
            audio_format,
            sample_rate,
        )

    def _transcribe_sync(self, audio: bytes, audio_format: AudioFormat, sample_rate: int) -> AsrResult:
        if not audio:
            return AsrResult(text="")
        # The official example accepts an audio URL. DashScope also accepts a
        # data URL, which keeps microphone data out of public object storage.
        mime = "audio/wav" if audio_format == AudioFormat.WAV else "audio/pcm"
        audio_data = f"data:{mime};base64,{base64.b64encode(audio).decode('ascii')}"
        payload = {
            "model": self.model,
            "input": {
                "messages": [
                    {
                        "role": "user",
                        "content": [{"type": "input_audio", "input_audio": {"data": audio_data}}],
                    }
                ]
            },
            "parameters": {"format": audio_format.value, "sample_rate": sample_rate},
        }
        request = Request(
            self.endpoint,
            data=json.dumps(payload).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
                "X-DashScope-SSE": "disable",
            },
            method="POST",
        )
        try:
            with urlopen(request, timeout=30) as response:
                raw = json.loads(response.read().decode("utf-8"))
        except (HTTPError, URLError, TimeoutError, json.JSONDecodeError) as exc:
            raise DashScopeAsrError(f"DashScope ASR request failed: {exc}") from exc
        text = self._extract_text(raw)
        return AsrResult(text=text, request_id=raw.get("request_id"), raw=raw)

    @staticmethod
    def _extract_text(raw: dict) -> str:
        output = raw.get("output") or {}
        choices = output.get("choices") or []
        if choices:
            message = choices[0].get("message") or {}
            content = message.get("content") or []
            for item in content:
                if isinstance(item, dict) and item.get("text"):
                    return str(item["text"]).strip()
            if isinstance(content, str):
                return content.strip()
        for key in ("text", "transcript"):
            if raw.get(key):
                return str(raw[key]).strip()
        return ""
