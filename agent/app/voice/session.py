"""Provider-neutral voice turn coordinator.

This module deliberately owns cancellation and turn freshness. Transport and
UI adapters can feed complete utterances now and switch to streaming frames
later without changing the stale-result rules.
"""

from __future__ import annotations

import asyncio
from dataclasses import dataclass
from enum import Enum
from typing import Awaitable, Callable

from .providers import AsrProvider, TtsProvider


class VoiceState(str, Enum):
    IDLE = "idle"
    LISTENING = "listening"
    RECOGNIZING = "recognizing"
    THINKING = "thinking"
    SPEAKING = "speaking"
    STOPPING = "stopping"
    ENDED = "ended"


@dataclass(frozen=True)
class VoiceTurn:
    turn_id: int
    transcript: str


class VoiceSession:
    """Coordinates ASR, text Agent, TTS, and barge-in for one chat session."""

    def __init__(
        self,
        asr: AsrProvider,
        tts: TtsProvider,
        run_agent: Callable[[str, int], Awaitable[str]],
        on_state: Callable[[VoiceState], None] | None = None,
        on_audio: Callable[[bytes, int], Awaitable[None]] | None = None,
    ) -> None:
        self.asr = asr
        self.tts = tts
        self.run_agent = run_agent
        self.on_state = on_state or (lambda _state: None)
        self.on_audio = on_audio or (lambda _data, _turn_id: _completed())
        self.state = VoiceState.IDLE
        self._turn_id = 0
        self._active_task: asyncio.Task[None] | None = None
        self._stopped = False

    async def start(self) -> None:
        self._stopped = False
        self._set_state(VoiceState.LISTENING)

    async def submit_utterance(self, audio: bytes) -> VoiceTurn | None:
        """Recognize one VAD-completed utterance and start a reply."""
        if self._stopped or not audio:
            return None
        await self._cancel_active_turn()
        self._turn_id += 1
        turn_id = self._turn_id
        self._set_state(VoiceState.RECOGNIZING)
        result = await self.asr.transcribe(audio)
        if self._stopped or turn_id != self._turn_id or not result.text.strip():
            return None
        turn = VoiceTurn(turn_id=turn_id, transcript=result.text.strip())
        self._active_task = asyncio.create_task(self._reply(turn))
        return turn

    async def interrupt(self) -> None:
        """Invalidate the current reply before cloud recognition completes."""
        self._turn_id += 1
        await self._cancel_active_turn()
        if not self._stopped:
            self._set_state(VoiceState.LISTENING)

    async def stop(self) -> None:
        self._stopped = True
        self._turn_id += 1
        self._set_state(VoiceState.STOPPING)
        await self._cancel_active_turn()
        self._set_state(VoiceState.ENDED)

    async def _reply(self, turn: VoiceTurn) -> None:
        try:
            self._set_state(VoiceState.THINKING)
            text = await self.run_agent(turn.transcript, turn.turn_id)
            if not self._is_current(turn.turn_id) or not text.strip():
                return
            self._set_state(VoiceState.SPEAKING)
            async for chunk in self.tts.synthesize_stream(text):
                if not self._is_current(turn.turn_id):
                    return
                await self.on_audio(chunk.data, turn.turn_id)
        except asyncio.CancelledError:
            raise
        finally:
            if self._is_current(turn.turn_id) and not self._stopped:
                self._set_state(VoiceState.LISTENING)

    async def _cancel_active_turn(self) -> None:
        task = self._active_task
        self._active_task = None
        await self.tts.cancel()
        if task and not task.done():
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

    def _is_current(self, turn_id: int) -> bool:
        return not self._stopped and turn_id == self._turn_id

    def _set_state(self, state: VoiceState) -> None:
        self.state = state
        self.on_state(state)


async def _completed() -> None:
    return None
