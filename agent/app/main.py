"""Sidecar WS Client and agent routing loop."""
from __future__ import annotations

import asyncio
import json
import os
import sys
import traceback
from typing import Any, Dict, Optional

import websockets

from .protocol import MsgType, make
from .prompt import assemble_prompt, summarize_history
from .graph import build_graph, get_available_tools
from .models import LlmConfig, embed_texts
from .memory_extract import extract_memories
from .title import generate_session_title

EMBEDDING_REQUEST_TIMEOUT_SECONDS = 30
TITLE_RESULT_WAIT_SECONDS = 5


def resolve_task_llm(
    task_configs: Dict[str, Dict[str, Any]],
    role: str,
    fallback_model: str,
    fallback_config: Optional[LlmConfig],
) -> tuple[str, Optional[LlmConfig]]:
    """Resolve one task-specific model, falling back to the active main model."""
    raw = task_configs.get(role)
    if not raw:
        return fallback_model, fallback_config
    model = raw.get("litellmModel") or raw.get("model") or fallback_model
    return model, LlmConfig.from_dict(raw)


def build_debug_prompt_payload(payload: Dict[str, Any]) -> Dict[str, Any]:
    """Build the same prompt components and tool schemas used by an LLM call."""
    system_prompt, messages, discarded = assemble_prompt(payload)
    tool_policy = payload.get("context", {}).get("agent", {}).get("toolPolicy") or {}
    dynamic_tools = payload.get("context", {}).get("mcpTools") or []
    return {
        "system_prompt": system_prompt,
        "messages": messages,
        "tools": get_available_tools(tool_policy, dynamic_tools),
        "discarded_messages": discarded,
    }


INTERNAL_EMBEDDING_KEY = "__agnes_embedding"


def prepare_memory_tool_arguments(
    tool_name: str,
    arguments: Dict[str, Any],
    embedding_model: Optional[str],
    embedding_config: Optional[LlmConfig],
) -> Dict[str, Any]:
    """Attach a trusted embedding without exposing it in the model's tool schema."""
    prepared = dict(arguments)
    prepared.pop(INTERNAL_EMBEDDING_KEY, None)
    if not embedding_model or not embedding_config:
        return prepared

    field = "query" if tool_name == "memory_search" else "content"
    if tool_name not in {"memory_search", "memory_create", "memory_update"}:
        return prepared
    text = prepared.get(field)
    if not isinstance(text, str) or not text.strip():
        return prepared

    vector = embed_texts(embedding_model, [text.strip()], embedding_config)[0]
    prepared[INTERNAL_EMBEDDING_KEY] = {
        "model": embedding_config.model_ref or embedding_model,
        "vector": vector,
    }
    return prepared


def attach_extracted_memory_embeddings(
    memories: list[Dict[str, Any]],
    embedding_model: Optional[str],
    embedding_config: Optional[LlmConfig],
) -> list[Dict[str, Any]]:
    """Batch-index extracted memories while preserving extractor output fields."""
    if not memories or not embedding_model or not embedding_config:
        return memories
    vectors = embed_texts(
        embedding_model,
        [str(memory.get("content", "")).strip() for memory in memories],
        embedding_config,
    )
    model_ref = embedding_config.model_ref or embedding_model
    return [
        {
            **memory,
            "embedding": {"model": model_ref, "vector": vector},
        }
        for memory, vector in zip(memories, vectors)
    ]


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

    llm_config_raw: Optional[Dict[str, Any]] = payload.get("context", {}).get("llmConfig")
    fallback_llm_configs = payload.get("context", {}).get("fallbackLlmConfigs") or []
    if not isinstance(fallback_llm_configs, list):
        fallback_llm_configs = []
    task_llm_configs: Dict[str, Dict[str, Any]] = payload.get("context", {}).get("taskLlmConfigs") or {}
    title_request = payload.get("context", {}).get("sessionTitleRequest")
    title_task: Optional[asyncio.Task[Optional[str]]] = None
    embedding_config_raw = task_llm_configs.get("embedding")
    embedding_model = None
    embedding_llm_config = None
    if embedding_config_raw:
        embedding_model = embedding_config_raw.get("litellmModel") or embedding_config_raw.get("model")
        embedding_llm_config = LlmConfig.from_dict(embedding_config_raw)

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

    async def notify_model_fallback(details: Dict[str, Any]) -> None:
        """Record a zero-output failover without adding it to model conversation history."""
        fallback_envelope = make(
            MsgType.MODEL_FALLBACK,
            session_id=session_id,
            run_id=run_id,
            payload=details,
        )
        await ws.send(fallback_envelope.model_dump_json())

    async def execute_tool(tool_call_id: str, tool_name: str, arguments: Dict[str, Any]) -> str:
        """Send tool request to Rust and wait for the execution result."""
        print(f"[sidecar][tool] Requesting `{tool_name}` (id: {tool_call_id})", flush=True)
        try:
            arguments = await asyncio.to_thread(
                prepare_memory_tool_arguments,
                tool_name,
                arguments,
                embedding_model,
                embedding_llm_config,
            )
        except Exception as embedding_error:
            arguments = dict(arguments)
            arguments.pop(INTERNAL_EMBEDDING_KEY, None)
            print(
                f"[sidecar][embedding] Tool embedding failed; using text fallback: {embedding_error}",
                flush=True,
            )
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
    # Prefer litellm model from llmConfig, fallback to agent model field
    _cfg = llm_config_raw or {}
    agent_model = _cfg.get("litellmModel") or _cfg.get("model") or payload.get("context", {}).get("agent", {}).get("model") or "gpt-4o"
    llm_config = LlmConfig.from_dict(llm_config_raw) if llm_config_raw else None
    summary_model, summary_llm_config = resolve_task_llm(
        task_llm_configs,
        "summary",
        agent_model,
        llm_config,
    )
    memory_model, memory_llm_config = resolve_task_llm(
        task_llm_configs,
        "memory",
        agent_model,
        llm_config,
    )

    # Title generation is deliberately opt-in: never silently spend the main model
    # or include the title request in the conversation context.
    if isinstance(title_request, dict):
        source_text = title_request.get("sourceText")
        quick_raw = task_llm_configs.get("quick")
        if isinstance(source_text, str) and source_text.strip() and isinstance(quick_raw, dict):
            quick_model = quick_raw.get("litellmModel") or quick_raw.get("model")
            if isinstance(quick_model, str) and quick_model:
                title_task = asyncio.create_task(
                    asyncio.to_thread(
                        generate_session_title,
                        source_text.strip(),
                        quick_model,
                        LlmConfig.from_dict(quick_raw),
                    )
                )

    graph_inputs = {
        "messages": messages,
        "system_prompt": system_prompt,
        "model": agent_model,
        "tool_policy": payload.get("context", {}).get("agent", {}).get("toolPolicy") or {},
        "dynamic_tools": payload.get("context", {}).get("mcpTools") or [],
        "pending_tool_calls": [],
        "finished": False,
        "llm_config": llm_config_raw,  # Raw dict, parsed in graph nodes
        "fallback_configs": fallback_llm_configs,
        "active_llm_index": 0,
        "active_llm_config": llm_config_raw,
        "active_model": agent_model,
        "fallback_locked": False,
    }
    
    config = {
        "configurable": {
            "send_delta_fn": send_delta,
            "execute_tool_fn": execute_tool,
            "notify_fallback_fn": notify_model_fallback,
        }
    }

    # 4. Invoke LangGraph workflow
    try:
        graph = build_graph()
        output_state = await graph.ainvoke(graph_inputs, config=config)

        active_config_raw = output_state.get("active_llm_config") or llm_config_raw
        active_model = output_state.get("active_model") or agent_model
        active_llm_config = (
            LlmConfig.from_dict(active_config_raw) if active_config_raw else llm_config
        )
        if not task_llm_configs.get("summary"):
            summary_model = active_model
            summary_llm_config = active_llm_config
        if not task_llm_configs.get("memory"):
            memory_model = active_model
            memory_llm_config = active_llm_config
        
        # 5. Extract rolling conversation summary (if budget was exceeded)
        new_summary = payload.get("context", {}).get("summary") or ""
        if discarded_messages:
            print(f"[sidecar][summary] History exceeded budget, compiling new rolling summary...", flush=True)
            # Summarize the discarded messages + current summary
            new_summary = summarize_history(
                discarded_messages,
                new_summary,
                summary_model,
                llm_config=summary_llm_config,
            )
            
        # 6. Extract structured memories and attach local vectors when configured.
        extracted_memories = []
        try:
            latest_messages = output_state.get("messages", [])
            extracted_memories = extract_memories(
                latest_messages,
                memory_model,
                llm_config=memory_llm_config,
            )
            extracted_memories = await asyncio.to_thread(
                attach_extracted_memory_embeddings,
                extracted_memories,
                embedding_model,
                embedding_llm_config,
            )
            if extracted_memories:
                print(
                    f"[sidecar][memory] Extracted {len(extracted_memories)} new memories",
                    flush=True,
                )
        except Exception as mem_ex:
            print(f"[sidecar][memory] Failed to extract memories: {mem_ex}", flush=True)

        generated_title = None
        if title_task is not None:
            try:
                generated_title = await asyncio.wait_for(title_task, timeout=TITLE_RESULT_WAIT_SECONDS)
            except asyncio.TimeoutError:
                title_task.cancel()
                print("[sidecar][title] Timed out waiting for quick model; keeping fallback title", flush=True)
            except Exception as title_error:
                print(f"[sidecar][title] Title task failed: {title_error}", flush=True)

        # 7. Run completed successfully -> send RUN_FINISHED
        title_payload = None
        if generated_title and isinstance(title_request, dict):
            title_payload = {
                "value": generated_title,
                "fallbackTitle": title_request.get("fallbackTitle"),
            }
        finished_envelope = make(
            MsgType.RUN_FINISHED,
            session_id=session_id,
            run_id=run_id,
            payload={
                "summary": new_summary,
                "memories": extracted_memories,
                "title": title_payload,
            }
        )
        await ws.send(finished_envelope.model_dump_json())
        print(f"[sidecar][run] Run finished successfully (id: {run_id})", flush=True)
        
    except asyncio.CancelledError:
        if title_task is not None and not title_task.done():
            title_task.cancel()
        print(f"[sidecar][run] Run task cancelled (id: {run_id})", flush=True)
        # 通知 Rust 取消，让其走 RUN_ERROR 清理路径（保存已累积内容、置状态、清映射）
        try:
            err_envelope = make(
                MsgType.RUN_ERROR,
                session_id=session_id,
                run_id=run_id,
                payload={"message": "已取消"}
            )
            await ws.send(err_envelope.model_dump_json())
        except Exception:
            pass
        raise
    except Exception as e:
        if title_task is not None and not title_task.done():
            title_task.cancel()
        traceback.print_exc()
        err_envelope = make(
            MsgType.RUN_ERROR,
            session_id=session_id,
            run_id=run_id,
            payload={"message": f"Execution failed: {e}"}
        )
        await ws.send(err_envelope.model_dump_json())


async def handle_embedding_request(
    ws: websockets.WebSocketClientProtocol,
    envelope: Dict[str, Any],
) -> None:
    """Handle an embedding request without blocking the main WS receive loop."""
    request_id = envelope.get("id", "")
    payload = envelope.get("payload", {})
    try:
        config_raw = payload.get("config") or {}
        config = LlmConfig.from_dict(config_raw)
        inputs = payload.get("inputs") or []
        model = config.litellm_model or config.model
        vectors = await asyncio.wait_for(
            asyncio.to_thread(embed_texts, model, inputs, config),
            timeout=EMBEDDING_REQUEST_TIMEOUT_SECONDS,
        )
        result_payload = {"id": request_id, "vectors": vectors}
    except Exception as embedding_error:
        result_payload = {"id": request_id, "error": str(embedding_error)}
    try:
        result = make(
            MsgType.EMBEDDING_RESULT,
            payload=result_payload,
        )
        await ws.send(result.model_dump_json())
    except websockets.exceptions.ConnectionClosed:
        return


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
    background_tasks: set[asyncio.Task] = set()

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

                elif mtype == MsgType.EMBEDDING_REQUEST:
                    task = asyncio.create_task(handle_embedding_request(ws, msg))
                    background_tasks.add(task)
                    task.add_done_callback(background_tasks.discard)

                elif mtype == MsgType.DEBUG_PROMPT:
                    # 仅拼装提示词并返回，不调用 LLM，用于前端调试面板
                    print(f"[sidecar][debug] Assembling prompt for debug panel (id: {msg.get('id')})", flush=True)
                    try:
                        debug_payload = build_debug_prompt_payload(payload)
                        result = make(
                            MsgType.DEBUG_PROMPT_RESULT,
                            session_id=session_id,
                            run_id=run_id,
                            payload={
                                "id": msg.get("id"),
                                **debug_payload,
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
