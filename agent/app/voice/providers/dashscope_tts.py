"""CosyVoice TTS adapter backed by the DashScope Python SDK."""

from __future__ import annotations

import asyncio
import concurrent.futures
import contextlib
import os
import threading
import time
from dataclasses import dataclass, field
from typing import Any
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

    The SDK is synchronous and delivers streaming PCM through callbacks. The
    adapter runs that SDK call in a worker thread and bridges bounded callback
    events to the asyncio loop, while retaining ``synthesize`` for callers that
    need one complete MP3 byte string.
    """

    model = "cosyvoice-v3.5-flash"
    streaming = True
    MAX_PENDING_CHUNKS = 3

    def __init__(
        self,
        api_key: str | None = None,
        voice: str | None = None,
        websocket_url: str | None = None,
        workspace: str | None = None,
        model: str | None = None,
    ) -> None:
        self.api_key = api_key or os.environ.get("DASHSCOPE_API_KEY")
        self.voice = voice or os.environ.get("DASHSCOPE_TTS_VOICE")
        self.workspace = workspace or os.environ.get("DASHSCOPE_WORKSPACE_ID")
        self.model = model or type(self).model
        self.websocket_url = resolve_websocket_url(
            websocket_url or os.environ.get("DASHSCOPE_TTS_WEBSOCKET_URL"),
            self.workspace,
        )
        self._active_task: asyncio.Task[bytes] | None = None
        self._active_stream: _StreamState | None = None
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
        async def stream() -> AsyncIterator[AudioChunk]:
            if not text.strip():
                return
            await self._cancel_stream()
            state = _StreamState(
                loop=asyncio.get_running_loop(),
                events=asyncio.Queue(maxsize=self.MAX_PENDING_CHUNKS),
            )
            self._active_stream = state
            state.worker = asyncio.create_task(self._run_stream(text, state))
            sequence = 0
            try:
                while True:
                    event = await state.events.get()
                    if isinstance(event, _AudioEvent):
                        if state.cancelled:
                            continue
                        yield AudioChunk(
                            data=event.data,
                            content_type="audio/pcm;rate=22050;channels=1;format=s16le",
                            sequence=sequence,
                        )
                        sequence += 1
                    elif isinstance(event, _StreamError):
                        raise event.error
                    elif isinstance(event, _StreamComplete):
                        break
            finally:
                if self._active_stream is state:
                    await self._close_stream(state)
                    self._active_stream = None

        return stream()

    async def cancel(self) -> None:
        await self._cancel_stream()
        task = self._active_task
        if task and not task.done():
            task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await task

    async def _cancel_stream(self) -> None:
        state = self._active_stream
        if state is None:
            return
        state.cancelled = True
        synthesizer = state.get_synthesizer()
        if synthesizer is not None:
            self._start_cancel_thread(synthesizer)
        worker = state.worker
        if worker and not worker.done():
            worker.cancel()
        if self._active_stream is state:
            self._active_stream = None

    async def _close_stream(self, state: "_StreamState") -> None:
        if state.cancelled or not state.is_terminal():
            state.cancelled = True
            # The generator may be running in a cancellation context. Waiting
            # for another executor task here can prevent the cancellation from
            # completing, so let the SDK cancellation finish independently.
            asyncio.create_task(self._cancel_stream_state(state))
            return
        worker = state.worker
        if worker and not worker.done():
            with contextlib.suppress(asyncio.CancelledError):
                await worker

    async def _cancel_stream_state(self, state: "_StreamState") -> None:
        synthesizer = state.get_synthesizer()
        if synthesizer is not None:
            self._start_cancel_thread(synthesizer)
        worker = state.worker
        if worker and not worker.done():
            worker.cancel()

    @staticmethod
    def _start_cancel_thread(synthesizer: Any) -> None:
        def cancel() -> None:
            with contextlib.suppress(Exception):
                synthesizer.streaming_cancel()

        threading.Thread(target=cancel, name="cosyvoice-cancel", daemon=True).start()

    async def _run_stream(self, text: str, state: "_StreamState") -> None:
        thread = threading.Thread(
            target=self._stream_sync,
            args=(text, state),
            name="cosyvoice-stream",
            daemon=True,
        )
        thread.start()
        try:
            await state.done.wait()
        except asyncio.CancelledError:
            raise

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

    def _stream_sync(self, text: str, state: "_StreamState") -> None:
        try:
            self._stream_sync_impl(text, state)
        except Exception as exc:
            state.emit_error(exc)
        finally:
            state.emit_complete()
            state.loop.call_soon_threadsafe(state.done.set)

    def _stream_sync_impl(self, text: str, state: "_StreamState") -> None:
        try:
            import dashscope
            from dashscope.audio.tts_v2 import (
                AudioFormat,
                ResultCallback,
                SpeechSynthesizer,
            )
        except ImportError as exc:
            raise RuntimeError(
                "CosyVoice requires the dashscope Python package; run uv sync"
            ) from exc

        dashscope.api_key = self.api_key
        if self.websocket_url:
            dashscope.base_websocket_api_url = self.websocket_url

        provider = self

        class Callback(ResultCallback):
            def on_data(self, data: bytes) -> None:
                state.emit_audio(bytes(data))

            def on_complete(self) -> None:
                state.emit_complete()

            def on_error(self, message: Any) -> None:
                if isinstance(message, BaseException):
                    error = message
                else:
                    error = RuntimeError(str(message))
                state.emit_error(error)

        last_timeout: TimeoutError | None = None
        for attempt in range(2):
            synthesizer = SpeechSynthesizer(
                model=provider.model,
                voice=provider.voice,
                format=AudioFormat.PCM_22050HZ_MONO_16BIT,
                workspace=provider.workspace,
                callback=Callback(),
            )
            state.set_synthesizer(synthesizer)
            if state.cancelled:
                with contextlib.suppress(Exception):
                    synthesizer.streaming_cancel()
                return
            try:
                synthesizer.streaming_call(text)
                if state.cancelled:
                    with contextlib.suppress(Exception):
                        synthesizer.streaming_cancel()
                    return
                synthesizer.streaming_complete()
                return
            except TimeoutError as exc:
                last_timeout = exc
                with contextlib.suppress(Exception):
                    synthesizer.streaming_cancel()
                if attempt == 0 and not state.cancelled:
                    time.sleep(0.6)
        endpoint = provider.websocket_url or "wss://dashscope.aliyuncs.com/api-ws/v1/inference"
        raise RuntimeError(
            f"CosyVoice WebSocket 建连超时（endpoint={endpoint}）。"
            "请检查网络、Workspace ID 与北京地域 endpoint。"
        ) from last_timeout


@dataclass(frozen=True)
class _AudioEvent:
    data: bytes


@dataclass(frozen=True)
class _StreamError:
    error: BaseException


@dataclass(frozen=True)
class _StreamComplete:
    pass


@dataclass
class _StreamState:
    loop: asyncio.AbstractEventLoop
    events: asyncio.Queue[object] = field(
        default_factory=lambda: asyncio.Queue(maxsize=3)
    )
    done: asyncio.Event = field(default_factory=asyncio.Event)
    worker: asyncio.Task[None] | None = None
    cancelled: bool = False
    _synthesizer: Any = None
    _lock: threading.Lock = field(default_factory=threading.Lock)
    _terminal: bool = False

    def set_synthesizer(self, synthesizer: Any) -> None:
        with self._lock:
            self._synthesizer = synthesizer

    def get_synthesizer(self) -> Any:
        with self._lock:
            return self._synthesizer

    def is_terminal(self) -> bool:
        with self._lock:
            return self._terminal

    def _emit(self, event: object) -> None:
        if self.cancelled and isinstance(event, (_AudioEvent, _StreamError)):
            return
        try:
            future = asyncio.run_coroutine_threadsafe(self.events.put(event), self.loop)
            while not self.cancelled:
                try:
                    future.result(timeout=0.1)
                    return
                except concurrent.futures.TimeoutError:
                    continue
                except RuntimeError:
                    return
            future.cancel()
        except (RuntimeError, concurrent.futures.CancelledError):
            return

    def emit_audio(self, data: bytes) -> None:
        if data:
            self._emit(_AudioEvent(data))

    def emit_error(self, error: BaseException) -> None:
        with self._lock:
            if self._terminal:
                return
        self._emit(_StreamError(error))

    def emit_complete(self) -> None:
        with self._lock:
            if self._terminal:
                return
            self._terminal = True
        self._emit(_StreamComplete())
