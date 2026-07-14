"""Sidecar WS Client and agent routing loop."""
from __future__ import annotations

import asyncio
import json
import os
import sys
import traceback
import litellm
from typing import Any, Dict, Optional

import websockets

from .protocol import MsgType, make
from .prompt import assemble_prompt, summarize_history
from .graph import build_graph
from .models import LlmConfig
from .memory_extract import extract_memories


async def run_agent_graph(
    ws: websockets.WebSocketClientProtocol,
    envelope: Dict[str, Any],
    pending_futures: Dict[str, asyncio.Future],
) -> None:
    """Task that compiles prompt, executes LangGraph reasoning, and handles LLM output."""
    session_id = envelope.get("session_id", "")
    run_id = envelope.get("run_id", "")
    payload = envelope.get("payload", {})
    
    # 1. Compile prompt & budget calculations
    try:
        system_prompt, messages, discarded_messages = assemble_prompt(payload)
    except Exception as e:
        traceback.print_exc()
        err_envelope = make(
            MsgType.RUN_ERROR,
            session_id=session_id,
            run_id=run_id,
            payload={"message": f"Prompt assembly failed: {e}"}
        )
        await ws.send(err_envelope.model_dump_json())
        return

    # 2. Async Callbacks for LangGraph config
    async def send_delta(content: str) -> None:
        """Send streaming assistant text delta back to Rust."""
        delta_envelope = make(
            MsgType.ASSISTANT_DELTA,
            session_id=session_id,
            run_id=run_id,
            payload={"content": content}
        )
        await ws.send(delta_envelope.model_dump_json())

    async def execute_tool(tool_call_id: str, tool_name: str, arguments: Dict[str, Any]) -> str:
        """Send tool request to Rust and wait for the execution result."""
        print(f"[sidecar][tool] Requesting `{tool_name}` (id: {tool_call_id})", flush=True)
        tool_req = make(
            MsgType.TOOL_CALL_REQUEST,
            session_id=session_id,
            run_id=run_id,
            payload={
                "id": tool_call_id,
                "tool": tool_name,
                "arguments": arguments
            }
        )
        await ws.send(tool_req.model_dump_json())

        # Create future and register it to wait for the WS loop to resolve it
        future = asyncio.get_running_loop().create_future()
        pending_futures[tool_call_id] = future
        
        try:
            result_payload = await future
            print(f"[sidecar][tool] Result received for `{tool_name}` (id: {tool_call_id})", flush=True)
            # Rust returns the tool result inside "result" field or stdout/stderr logs
            # Let's extract and serialize/format the result
            exit_code = result_payload.get("exit_code", 0)
            stdout = result_payload.get("stdout", "")
            stderr = result_payload.get("stderr", "")
            
            if exit_code != 0:
                return f"Error (Exit Code {exit_code}):\n{stderr or stdout}"
            return stdout or result_payload.get("result") or "Success"
        except asyncio.CancelledError:
            print(f"[sidecar][tool] Tool execution cancelled for (id: {tool_call_id})", flush=True)
            raise
        finally:
            pending_futures.pop(tool_call_id, None)

    # 3. Setup Graph and Inputs
    llm_config_raw: Optional[Dict[str, Any]] = payload.get("context", {}).get("llmConfig")
    # Prefer litellm model from llmConfig, fallback to agent model field
    _cfg = llm_config_raw or {}
    agent_model = _cfg.get("litellmModel") or _cfg.get("model") or payload.get("context", {}).get("agent", {}).get("model") or "gpt-4o"
    llm_config = LlmConfig.from_dict(llm_config_raw) if llm_config_raw else None

    graph_inputs = {
        "messages": messages,
        "system_prompt": system_prompt,
        "model": agent_model,
        "tool_policy": payload.get("context", {}).get("agent", {}).get("toolPolicy") or {},
        "pending_tool_calls": [],
        "finished": False,
        "llm_config": llm_config_raw,  # Raw dict, parsed in graph nodes
    }
    
    config = {
        "configurable": {
            "send_delta_fn": send_delta,
            "execute_tool_fn": execute_tool
        }
    }

    # 4. Invoke LangGraph workflow
    try:
        graph = build_graph()
        output_state = await graph.ainvoke(graph_inputs, config=config)
        
        # 5. Extract rolling conversation summary (if budget was exceeded)
        new_summary = payload.get("context", {}).get("summary") or ""
        if discarded_messages:
            print(f"[sidecar][summary] History exceeded budget, compiling new rolling summary...", flush=True)
            # Summarize the discarded messages + current summary
            new_summary = summarize_history(discarded_messages, new_summary, agent_model, llm_config=llm_config)
            
        # 6. Extract memories from the latest turns (asynchronously) and compute vectors
        extracted_memories = []
        try:
            latest_messages = output_state.get("messages", [])
            extracted_memories = extract_memories(latest_messages, agent_model, llm_config=llm_config)
            if extracted_memories:
                print(f"[sidecar][memory] Extracted {len(extracted_memories)} new memories: {extracted_memories}", flush=True)
                # Compute embedding vectors using LiteLLM
                for mem in extracted_memories:
                    try:
                        content = mem.get("content", "")
                        # Fallback to standard text-embedding-3-small
                        embed_res = litellm.embedding(
                            model="text-embedding-3-small",
                            input=[content]
                        )
                        vector = embed_res.data[0]["embedding"]
                        mem["vector"] = vector
                        print(f"[sidecar][memory] Computed vector of length {len(vector)} for extracted memory", flush=True)
                    except Exception as embed_ex:
                        print(f"[sidecar][memory] Failed to compute embedding: {embed_ex}", flush=True)
        except Exception as mem_ex:
            print(f"[sidecar][memory] Failed to extract memories: {mem_ex}", flush=True)

        # 7. Run completed successfully -> send RUN_FINISHED
        finished_envelope = make(
            MsgType.RUN_FINISHED,
            session_id=session_id,
            run_id=run_id,
            payload={
                "summary": new_summary,
                "memories": extracted_memories
            }
        )
        await ws.send(finished_envelope.model_dump_json())
        print(f"[sidecar][run] Run finished successfully (id: {run_id})", flush=True)
        
    except asyncio.CancelledError:
        print(f"[sidecar][run] Run task cancelled (id: {run_id})", flush=True)
    except Exception as e:
        traceback.print_exc()
        err_envelope = make(
            MsgType.RUN_ERROR,
            session_id=session_id,
            run_id=run_id,
            payload={"message": f"Execution failed: {e}"}
        )
        await ws.send(err_envelope.model_dump_json())


async def main() -> None:
    url = os.environ.get("AGENT_WS_URL")
    token = os.environ.get("AGENT_PROTOCOL_TOKEN")
    if not url or not token:
        print("[sidecar] 缺少 AGENT_WS_URL / AGENT_PROTOCOL_TOKEN，退出", file=sys.stderr)
        sys.exit(1)

    ws_url = f"{url}?token={token}"
    print(f"[sidecar] 连接 Rust WS: {url}", flush=True)

    pending_futures: Dict[str, asyncio.Future] = {}
    run_tasks: Dict[str, asyncio.Task] = {}

    try:
        async with websockets.connect(ws_url) as ws:
            # Send HELLO handshake
            await ws.send(
                make(MsgType.HELLO, payload={"token": token}).model_dump_json()
            )

            async for raw in ws:
                msg = json.loads(raw)
                mtype = msg.get("type")
                session_id = msg.get("session_id", "")
                run_id = msg.get("run_id", "")
                payload = msg.get("payload", {})

                if mtype == MsgType.READY:
                    print("[sidecar] 握手成功，进入待命状态", flush=True)
                    
                elif mtype == MsgType.PING:
                    await ws.send(make(MsgType.PONG).model_dump_json())
                    
                elif mtype == MsgType.RUN_REQUEST:
                    print(f"[sidecar][run] Received run request (id: {run_id})", flush=True)
                    # Spawn task to run compiled graph concurrently
                    task = asyncio.create_task(run_agent_graph(ws, msg, pending_futures))
                    run_tasks[run_id] = task
                    # Clean task reference when done
                    task.add_done_callback(lambda t, rid=run_id: run_tasks.pop(rid, None))
                    
                elif mtype == MsgType.TOOL_RESULT:
                    payload = msg.get("payload", {})
                    tc_id = payload.get("id")
                    if tc_id in pending_futures:
                        pending_futures[tc_id].set_result(payload)
                        
                elif mtype == MsgType.RUN_CANCEL:
                    print(f"[sidecar][run] Cancelling run task (id: {run_id})", flush=True)
                    if run_id in run_tasks:
                        run_tasks[run_id].cancel()

                elif mtype == MsgType.DEBUG_PROMPT:
                    # 仅拼装提示词并返回，不调用 LLM，用于前端调试面板
                    print(f"[sidecar][debug] Assembling prompt for debug panel (id: {msg.get('id')})", flush=True)
                    try:
                        system_prompt, messages, discarded = assemble_prompt(payload)
                        result = make(
                            MsgType.DEBUG_PROMPT_RESULT,
                            session_id=session_id,
                            run_id=run_id,
                            payload={
                                "id": msg.get("id"),
                                "system_prompt": system_prompt,
                                "messages": messages,
                                "discarded_messages": discarded,
                            },
                        )
                        await ws.send(result.model_dump_json())
                    except Exception as e:
                        traceback.print_exc()
                        err = make(
                            MsgType.DEBUG_PROMPT_RESULT,
                            session_id=session_id,
                            run_id=run_id,
                            payload={"id": msg.get("id"), "error": f"Prompt assembly failed: {e}"},
                        )
                        await ws.send(err.model_dump_json())

                else:
                    print(f"[sidecar] 未知消息类型: {mtype}", flush=True)
                    
    except websockets.exceptions.ConnectionClosed as exc:
        print(f"[sidecar] 与 Rust 的连接已断开 (code={exc.code})，退出", flush=True)
        sys.exit(0)
    except Exception as e:
        print(f"[sidecar] 发生未预期错误: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
