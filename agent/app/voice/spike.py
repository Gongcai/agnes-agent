"""Phase 0 provider smoke check.

Usage: ``DASHSCOPE_API_KEY=... DASHSCOPE_TTS_VOICE=... uv run python -m app.voice.spike input.wav``.
Set ``DASHSCOPE_TTS_WEBSOCKET_URL`` when the voice belongs to a workspace endpoint.
It intentionally writes output only when an explicit output path is supplied.
"""

from __future__ import annotations

import argparse
import asyncio
import os
from pathlib import Path

from .providers import CosyVoiceTtsProvider, DashScopeAsrProvider


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("audio", type=Path, nargs="?")
    parser.add_argument("--text", default="语音服务连接测试。")
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--stream",
        action="store_true",
        help="consume CosyVoice callback audio chunks instead of one complete result",
    )
    parser.add_argument(
        "--voice",
        default=os.environ.get("DASHSCOPE_TTS_VOICE"),
        help="CosyVoice voice_id; also accepted through DASHSCOPE_TTS_VOICE",
    )
    args = parser.parse_args()

    if args.audio:
        asr = DashScopeAsrProvider()
        result = await asr.transcribe(args.audio.read_bytes())
        print(f"ASR request={result.request_id or '-'} text={result.text!r}")
    else:
        print("ASR skipped: pass an audio file to run recognition.")

    try:
        tts = CosyVoiceTtsProvider(voice=args.voice)
        if args.stream:
            chunks = []
            first_chunk = None
            async for chunk in tts.synthesize_stream(args.text):
                if first_chunk is None:
                    first_chunk = chunk
                chunks.append(chunk.data)
            audio = b"".join(chunks)
            print(
                f"TTS chunks={len(chunks)} first_chunk_bytes="
                f"{len(first_chunk.data) if first_chunk else 0} streaming={tts.streaming}"
            )
        else:
            audio = await tts.synthesize(args.text)
            print(f"TTS bytes={len(audio)} streaming={tts.streaming}")
    except Exception as exc:
        raise RuntimeError(
            "CosyVoice synthesis failed. Verify that --voice/DASHSCOPE_TTS_VOICE "
            "is a voice_id enabled for cosyvoice-v3.5-flash. "
            f"Original error: {exc}"
        ) from exc
    if args.output:
        args.output.write_bytes(audio)
        print(f"TTS output={args.output}")


if __name__ == "__main__":
    asyncio.run(main())
