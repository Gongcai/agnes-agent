"""Stable interfaces between Voice Session orchestration and cloud providers."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import AsyncIterator, Protocol


class AudioFormat(str, Enum):
    WAV = "wav"
    PCM = "pcm"


@dataclass(frozen=True)
class AsrResult:
    text: str
    request_id: str | None = None
    raw: dict | None = None


@dataclass(frozen=True)
class AudioChunk:
    data: bytes
    content_type: str
    sequence: int
    is_final: bool = False


class AsrProvider(Protocol):
    async def transcribe(
        self,
        audio: bytes,
        *,
        audio_format: AudioFormat = AudioFormat.WAV,
        sample_rate: int = 16000,
    ) -> AsrResult: ...


class TtsProvider(Protocol):
    streaming: bool

    async def synthesize(self, text: str) -> bytes: ...

    def synthesize_stream(self, text: str) -> AsyncIterator[AudioChunk]: ...

    async def cancel(self) -> None: ...
