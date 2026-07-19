"""Protocol smoke test for a frozen Agnes sidecar executable."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path
import sys

import websockets


async def smoke_test(binary: Path) -> None:
    token = "frozen-sidecar-smoke-token"
    hello_received = asyncio.get_running_loop().create_future()

    async def handler(websocket: websockets.ServerConnection) -> None:
        message = json.loads(await asyncio.wait_for(websocket.recv(), timeout=20))
        if message.get("type") != "hello":
            raise RuntimeError(f"Unexpected handshake message: {message}")
        if message.get("payload", {}).get("token") != token:
            raise RuntimeError("Frozen sidecar sent an invalid protocol token")
        await websocket.send(
            json.dumps(
                {
                    "protocol_version": 1,
                    "id": "ready",
                    "run_id": "",
                    "session_id": "",
                    "type": "ready",
                    "created_at": "",
                    "payload": {},
                }
            )
        )
        hello_received.set_result(None)
        await websocket.wait_closed()

    async with websockets.serve(handler, "127.0.0.1", 0) as server:
        port = server.sockets[0].getsockname()[1]
        environment = os.environ.copy()
        environment["AGENT_WS_URL"] = f"ws://127.0.0.1:{port}/agent"
        environment["AGENT_PROTOCOL_TOKEN"] = token
        process = await asyncio.create_subprocess_exec(
            str(binary),
            env=environment,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        terminated_by_test = False
        try:
            await asyncio.wait_for(hello_received, timeout=30)
            await asyncio.sleep(0.25)
            if process.returncode is not None:
                raise RuntimeError(f"Frozen sidecar exited early with {process.returncode}")
        finally:
            if process.returncode is None:
                terminated_by_test = True
                process.terminate()
            stdout, stderr = await asyncio.wait_for(process.communicate(), timeout=10)

        if not terminated_by_test and process.returncode != 0:
            raise RuntimeError(
                f"Frozen sidecar exited with {process.returncode}: "
                f"{stderr.decode(errors='replace')}"
            )
        if b"127.0.0.1" not in stdout:
            raise RuntimeError("Frozen sidecar did not reach the WebSocket connection stage")


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: smoke_sidecar.py <agentd-path>", file=sys.stderr)
        return 2
    binary = Path(sys.argv[1]).resolve()
    if not binary.is_file():
        print(f"sidecar binary does not exist: {binary}", file=sys.stderr)
        return 2
    asyncio.run(smoke_test(binary))
    print(f"Frozen sidecar handshake passed: {binary}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
