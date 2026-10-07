"""CosyVoice TTS adapter backed by the DashScope Python SDK."""

from __future__ import annotations

import asyncio
import contextlib
import os
from typing import AsyncIterator

from .base import AudioChunk


def resolve_websocket_url(websocket_url: str | None, workspace: str | None) -> str | None:
    """Resolve a DashScope Beijing WebSocket endpoint from safe inputs."""
    if websocket_url:
        value = websocket_url.strip().rstrip("/")
        if (
            value.count(".cn-beijing.maas.aliyuncs.com") > 1
            or ".maas.aliyuncs.com.cn-beijing." in value
        ):
            raise ValueError(
                "DASHSCOPE_TTS_WEBSOCKET_URL contains a duplicated Beijing domain; "
                "use wss://<WorkspaceId>.cn-beijing.maas.aliyuncs.com/api-ws/v1/inference"
            )
        if not value.startswith(("ws://", "wss://")):
            raise ValueError("DASHSCOPE_TTS_WEBSOCKET_URL must start with ws:// or wss://")
        return value
    if not workspace:
        return None
    workspace_id = workspace.strip()
    if ".cn-beijing.maas.aliyuncs.com" in workspace_id or "/" in workspace_id:
        raise ValueError(
            "DASHSCOPE_WORKSPACE_ID must be the Workspace ID only, not a hostname or URL"
        )
    return f"wss://{workspace_id}.cn-beijing.maas.aliyuncs.com/api-ws/v1/inference"


class CosyVoiceTtsProvider:
    """CosyVoice v3.5 Flash adapter.

    The checked-in official example uses ``SpeechSynthesizer.call`` and returns
    one complete byte string. ``streaming`` remains false until the SDK's
    callback/iterator API is verified against the target account.
    """

    model = "cosyvoice-v3.5-flash"
    streaming = False

    def __init__(
        self,
        api_key: str | None = None,
        voice: str | None = None,
        websocket_url: str | None = None,
        workspace: str | None = None,
    ) -> None:
        self.api_key = api_key or os.environ.get("DASHSCOPE_API_KEY")
        self.voice = voice or os.environ.get("DASHSCOPE_TTS_VOICE")
        self.workspace = workspace or os.environ.get("DASHSCOPE_WORKSPACE_ID")
        self.websocket_url = resolve_websocket_url(
            websocket_url or os.environ.get("DASHSCOPE_TTS_WEBSOCKET_URL"),
            self.workspace,
        )
        self._active_task: asyncio.Task[bytes] | None = None
        if not self.api_key:
            raise ValueError("DASHSCOPE_API_KEY is required for CosyVoice")
        if not self.voice:
            raise ValueError(
                "DASHSCOPE_TTS_VOICE is required for cosyvoice-v3.5-flash; "
                "create or select a voice supported by this model, then set the returned voice_id"
            )

    @property
    def voice_id(self) -> str:
        """Return the provider voice id used by the current model."""
        return self.voice

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
            from dashscope.audio.tts_v2 import AudioFormat, SpeechSynthesizer
        except ImportError as exc:
            raise RuntimeError(
                "CosyVoice requires the dashscope Python package; run uv sync"
            ) from exc
        dashscope.api_key = self.api_key
        if self.websocket_url:
            dashscope.base_websocket_api_url = self.websocket_url
        synthesizer = SpeechSynthesizer(
            model=self.model,
            voice=self.voice,
            format=AudioFormat.MP3_22050HZ_MONO_256KBPS,
            workspace=self.workspace,
        )
        audio = synthesizer.call(text)
        if not audio:
            raise RuntimeError("CosyVoice returned no audio")
        return bytes(audio)
