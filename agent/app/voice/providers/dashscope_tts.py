"""CosyVoice TTS adapter backed by the DashScope Python SDK."""

from __future__ import annotations

import asyncio
import contextlib
import os
from typing import AsyncIterator

from .base import AudioChunk


class CosyVoiceTtsProvider:
    """CosyVoice v3.5 Flash adapter.

    The checked-in official example uses ``SpeechSynthesizer.call`` and returns
    one complete byte string. ``streaming`` remains false until the SDK's
    callback/iterator API is verified against the target account.
    """

    model = "cosyvoice-v3.5-flash"
    streaming = False

    def __init__(self, api_key: str | None = None, voice: str | None = None) -> None:
        self.api_key = api_key or os.environ.get("DASHSCOPE_API_KEY")
        self.voice = voice
        self._active_task: asyncio.Task[bytes] | None = None
        if not self.api_key:
            raise ValueError("DASHSCOPE_API_KEY is required for CosyVoice")

    async def synthesize(self, text: str) -> bytes:
        if not text.strip():
            return b""
        self._active_task = asyncio.create_task(asyncio.to_thread(self._call_sync, text))
        try:
            return await self._active_task
        finally:
            self._active_task = None

    def synthesize_stream(self, text: str) -> AsyncIterator[AudioChunk]:
        async def one_chunk() -> AsyncIterator[AudioChunk]:
            data = await self.synthesize(text)
            if data:
                yield AudioChunk(data=data, content_type="audio/mpeg", sequence=0, is_final=True)

        return one_chunk()

    async def cancel(self) -> None:
        task = self._active_task
        if task and not task.done():
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    def _call_sync(self, text: str) -> bytes:
        try:
            import dashscope
            from dashscope.audio.tts_v2 import SpeechSynthesizer
        except ImportError as exc:
            raise RuntimeError(
                "CosyVoice requires the dashscope Python package; run uv sync"
            ) from exc
        dashscope.api_key = self.api_key
        synthesizer = SpeechSynthesizer(model=self.model, voice=self.voice)
        audio = synthesizer.call(text)
        if not audio:
            raise RuntimeError("CosyVoice returned no audio")
        return bytes(audio)
