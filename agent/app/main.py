"""Sidecar 入口：作为 WS Client 连接 Rust（Rust 为 WS Server），完成 hello/ready 握手。

V0.1 仅打通通道与握手；V0.2 起在此之上接 LangGraph，按 run_request 的 ContextSnapshot
做 prompt 拼装、模型推理、工具决策与记忆抽取建议。
"""
from __future__ import annotations

import asyncio
import json
import os
import sys

import websockets

from .protocol import Envelope, MsgType, make


async def main() -> None:
    url = os.environ.get("AGENT_WS_URL")
    token = os.environ.get("AGENT_PROTOCOL_TOKEN")
    if not url or not token:
        print("[sidecar] 缺少 AGENT_WS_URL / AGENT_PROTOCOL_TOKEN，退出", file=sys.stderr)
        sys.exit(1)

    ws_url = f"{url}?token={token}"
    print(f"[sidecar] 连接 Rust WS: {url}", flush=True)

    async with websockets.connect(ws_url) as ws:
        # 握手：发送 hello（payload 带 token，Rust 校验）
        await ws.send(make(MsgType.HELLO, payload={"token": token}).model_dump_json())

        async for raw in ws:
            msg = json.loads(raw)
            mtype = msg.get("type")
            if mtype == MsgType.READY:
                print("[sidecar] 握手成功，进入待命（V0.1 仅骨架）", flush=True)
            elif mtype == MsgType.PING:
                await ws.send(make(MsgType.PONG).model_dump_json())
            else:
                print(f"[sidecar] recv: {mtype}", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
