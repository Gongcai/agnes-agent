"""Phase 0 provider smoke check.

Usage: ``DASHSCOPE_API_KEY=... uv run python -m app.voice.spike input.wav``.
It intentionally writes output only when an explicit output path is supplied.
"""

from __future__ import annotations

import argparse
import asyncio
from pathlib import Path

from .providers import CosyVoiceTtsProvider, DashScopeAsrProvider


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("audio", type=Path)
    parser.add_argument("--text", default="语音服务连接测试。")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    asr = DashScopeAsrProvider()
    result = await asr.transcribe(args.audio.read_bytes())
    print(f"ASR request={result.request_id or '-'} text={result.text!r}")

    tts = CosyVoiceTtsProvider()
    audio = await tts.synthesize(args.text)
    print(f"TTS bytes={len(audio)} streaming={tts.streaming}")
    if args.output:
        args.output.write_bytes(audio)
        print(f"TTS output={args.output}")


if __name__ == "__main__":
    asyncio.run(main())
