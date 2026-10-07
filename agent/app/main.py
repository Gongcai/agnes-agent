"""Sidecar WS Client and agent routing loop."""
from __future__ import annotations

import asyncio
import base64
import json
import os
import sys
import traceback
from typing import Any, Dict, Optional

import websockets

from .protocol import MsgType, make
from .image import process_image_attachments
from .prompt import assemble_prompt, summarize_history
from .graph import build_graph, get_available_tools
from .models import LlmConfig, embed_texts
from .title import generate_session_title
from .voice.providers import AudioFormat, CosyVoiceTtsProvider, DashScopeAsrProvider

EMBEDDING_REQUEST_TIMEOUT_SECONDS = 30


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
    context = payload.get("context", {})
    task_configs = context.get("taskLlmConfigs") or {}
    prepared = process_image_attachments(payload, task_configs.get("image"))
    system_prompt, messages, discarded = assemble_prompt(prepared)
    tool_policy = payload.get("context", {}).get("agent", {}).get("toolPolicy") or {}
    dynamic_tools = payload.get("context", {}).get("mcpTools") or []
    return {
        "system_prompt": system_prompt,
        "messages": messages,
        "tools": get_available_tools(tool_policy, dynamic_tools),
        "discarded_messages": discarded,
    }


async def run_agent_graph(
    ws: websockets.WebSocketClientProtocol,
    envelope: Dict[str, Any],
    pending_futures: Dict[str, asyncio.Future],
) -> None:
    """Task that compiles prompt, executes LangGraph reasoning, and handles LLM output."""
    session_id = envelope.get("session_id", "")
    run_id = envelope.get("run_id", "")
    payload = envelope.get("payload", {})

    context = payload.get("context", {})
    llm_config_raw: Optional[Dict[str, Any]] = context.get("llmConfig")
    task_llm_configs: Dict[str, Dict[str, Any]] = context.get("taskLlmConfigs") or {}

    # 1. Prepare image fallbacks, then compile prompt and budget calculations.
    try:
        payload = await asyncio.to_thread(
            process_image_attachments,
            payload,
            task_llm_configs.get("image"),
        )
        system_prompt, messages, discarded_messages = assemble_prompt(payload)
    except Exception as e:
        traceback.print_exc()
        err_envelope = make(
            MsgType.RUN_ERROR,
            session_id=session_id,
            run_id=run_id,
            payload={"message": f"图片附件或提示词处理失败：{e}"}
        )
        await ws.send(err_envelope.model_dump_json())
        return

    fallback_llm_configs = payload.get("context", {}).get("fallbackLlmConfigs") or []
    if not isinstance(fallback_llm_configs, list):
        fallback_llm_configs = []
    title_request = payload.get("context", {}).get("sessionTitleRequest")
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

    async def execute_tool(tool_call_id: str, tool_name: str, arguments: Dict[str, Any]) -> Any:
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
        # Register before sending: a fast local tool can return before the
        # event loop resumes this coroutine after ws.send().
        future = asyncio.get_running_loop().create_future()
        pending_futures[tool_call_id] = future

        try:
            await ws.send(tool_req.model_dump_json())
            result_payload = await future
            print(f"[sidecar][tool] Result received for `{tool_name}` (id: {tool_call_id})", flush=True)
            # Rust returns the tool result inside "result" field or stdout/stderr logs
            # Let's extract and serialize/format the result
            exit_code = result_payload.get("exit_code", 0)
            stdout = result_payload.get("stdout", "")
            stderr = result_payload.get("stderr", "")
            
            if exit_code is None:
                return f"Tool error:\n{stderr or stdout}"
            if exit_code != 0:
                return f"Error (Exit Code {exit_code}):\n{stderr or stdout}"
            image = result_payload.get("image")
            if isinstance(image, dict):
                return {
                    "text": stdout or result_payload.get("result") or "Image read successfully",
                    "image": image,
                }
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
        "token_usage": {
            "input_tokens": 0,
            "cached_tokens": 0,
            "output_tokens": 0,
            "context_tokens": 0,
        },
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
            
        # 6. Run completed successfully -> send RUN_FINISHED
        finished_envelope = make(
            MsgType.RUN_FINISHED,
            session_id=session_id,
            run_id=run_id,
            payload={
                "summary": new_summary,
                "usage": output_state.get("token_usage") or {},
            }
        )
        await ws.send(finished_envelope.model_dump_json())
        print(f"[sidecar][run] Run finished successfully (id: {run_id})", flush=True)

        # Generate the title after the main run so the same provider is not hit
        # concurrently and a slow title never keeps the response spinner active.
        try:
            if isinstance(title_request, dict):
                source_text = title_request.get("sourceText")
                fallback_title = title_request.get("fallbackTitle")
                quick_raw = task_llm_configs.get("quick")
                if (
                    isinstance(source_text, str)
                    and source_text.strip()
                    and isinstance(fallback_title, str)
                    and isinstance(quick_raw, dict)
                ):
                    quick_model = quick_raw.get("litellmModel") or quick_raw.get("model")
                    if isinstance(quick_model, str) and quick_model:
                        generated_title = await asyncio.to_thread(
                            generate_session_title,
                            source_text.strip(),
                            quick_model,
                            LlmConfig.from_dict(quick_raw),
                        )
                        if generated_title:
                            title_envelope = make(
                                MsgType.SESSION_TITLE_UPDATE,
                                session_id=session_id,
                                run_id=run_id,
                                payload={
                                    "value": generated_title,
                                    "fallbackTitle": fallback_title,
                                },
                            )
                            await ws.send(title_envelope.model_dump_json())
                            print(f"[sidecar][title] Session title updated (id: {run_id})", flush=True)
        except asyncio.CancelledError:
            print(f"[sidecar][title] Title update cancelled (id: {run_id})", flush=True)
            return
        except Exception as title_error:
            print(f"[sidecar][title] Title update failed: {title_error}", flush=True)
        
    except asyncio.CancelledError:
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


VOICE_AUDIO_MAX_BYTES = 4 * 1024 * 1024


async def handle_voice_transcribe_request(
    ws: websockets.WebSocketClientProtocol,
    envelope: Dict[str, Any],
) -> None:
    """Recognize one VAD-completed WAV utterance through the sidecar boundary."""
    request_id = envelope.get("id", "")
    payload = envelope.get("payload", {})
    try:
        encoded = payload.get("audio_base64")
        if not isinstance(encoded, str) or not encoded:
            raise ValueError("voice audio is required")
        audio = base64.b64decode(encoded, validate=True)
        if not audio or len(audio) > VOICE_AUDIO_MAX_BYTES:
            raise ValueError("voice audio is empty or exceeds the 4 MiB limit")
        format_name = str(payload.get("format") or "wav").lower()
        audio_format = AudioFormat(format_name)
        sample_rate = int(payload.get("sample_rate") or 16000)
        if sample_rate not in {8000, 16000, 22050, 24000, 44100, 48000}:
            raise ValueError("unsupported voice sample rate")
        provider = DashScopeAsrProvider()
        result = await provider.transcribe(
            audio,
            audio_format=audio_format,
            sample_rate=sample_rate,
        )
        result_payload = {
            "id": request_id,
            "text": result.text,
            "request_id": result.request_id,
        }
    except Exception as error:
        result_payload = {"id": request_id, "error": str(error)}
    try:
        result = make(
            MsgType.VOICE_TRANSCRIBE_RESULT,
            session_id=envelope.get("session_id", ""),
            payload=result_payload,
        )
        await ws.send(result.model_dump_json())
    except websockets.exceptions.ConnectionClosed:
        return


VOICE_SYNTHESIS_MAX_TEXT_CHARS = 4000
VOICE_SYNTHESIS_MAX_BYTES = 12 * 1024 * 1024


async def handle_voice_synthesize_request(
    ws: websockets.WebSocketClientProtocol,
    envelope: Dict[str, Any],
) -> None:
    """Synthesize one assistant sentence and return its bounded audio payload."""
    request_id = envelope.get("id", "")
    payload = envelope.get("payload", {})
    try:
        text = payload.get("text")
        if not isinstance(text, str) or not text.strip():
            raise ValueError("voice synthesis text is required")
        if len(text) > VOICE_SYNTHESIS_MAX_TEXT_CHARS:
            raise ValueError("voice synthesis text exceeds the 4000 character limit")
        provider = CosyVoiceTtsProvider(
            api_key=payload.get("api_key"),
            voice=payload.get("voice_id"),
            workspace=payload.get("workspace_id"),
        )
        audio = await provider.synthesize(text.strip())
        if not audio or len(audio) > VOICE_SYNTHESIS_MAX_BYTES:
            raise ValueError("voice synthesis audio is empty or exceeds the 12 MiB limit")
        result_payload = {
            "id": request_id,
            "audio_base64": base64.b64encode(audio).decode("ascii"),
            "content_type": "audio/mpeg",
        }
    except Exception as error:
        result_payload = {"id": request_id, "error": str(error)}
    try:
        result = make(
            MsgType.VOICE_SYNTHESIZE_RESULT,
            session_id=envelope.get("session_id", ""),
            payload=result_payload,
        )
        await ws.send(result.model_dump_json())
    except websockets.exceptions.ConnectionClosed:
        return


async def handle_voice_synthesize_stream_request(
    ws: websockets.WebSocketClientProtocol,
    envelope: Dict[str, Any],
    pending_voice_acks: Dict[tuple[str, int], asyncio.Future],
) -> None:
    """Forward bounded PCM chunks and wait for renderer playback credit."""
    request_id = envelope.get("id", "")
    payload = envelope.get("payload", {})
    sequence = 0
    try:
        text = payload.get("text")
        if not isinstance(text, str) or not text.strip():
            raise ValueError("voice synthesis text is required")
        if len(text) > VOICE_SYNTHESIS_MAX_TEXT_CHARS:
            raise ValueError("voice synthesis text exceeds the 4000 character limit")
        provider = CosyVoiceTtsProvider(
            api_key=payload.get("api_key"),
            voice=payload.get("voice_id"),
            workspace=payload.get("workspace_id"),
        )
        async for chunk in provider.synthesize_stream(text.strip()):
            if len(chunk.data) > 256 * 1024:
                raise ValueError("CosyVoice audio chunk exceeds the 256 KiB limit")
            if not chunk.data or len(chunk.data) % 2 != 0:
                raise ValueError("CosyVoice returned an invalid PCM16 chunk")
            ack = asyncio.get_running_loop().create_future()
            pending_voice_acks[(request_id, sequence)] = ack
            await ws.send(make(
                MsgType.VOICE_SYNTHESIZE_AUDIO_CHUNK,
                session_id=envelope.get("session_id", ""),
                payload={
                    "id": request_id,
                    "sequence": sequence,
                    "audio_base64": base64.b64encode(chunk.data).decode("ascii"),
                    "sample_rate": 22050,
                    "content_type": "audio/pcm;rate=22050;channels=1;format=s16le",
                },
            ).model_dump_json())
            await asyncio.wait_for(ack, timeout=8)
            pending_voice_acks.pop((request_id, sequence), None)
            sequence += 1
        result_payload = {"id": request_id, "complete": True, "chunks": sequence}
    except asyncio.CancelledError:
        raise
    except Exception as error:
        result_payload = {"id": request_id, "error": str(error)}
    finally:
        for key in [key for key in pending_voice_acks if key[0] == request_id]:
            future = pending_voice_acks.pop(key)
            if not future.done():
                future.cancel()
    try:
        result = make(
            MsgType.VOICE_SYNTHESIZE_RESULT,
            session_id=envelope.get("session_id", ""),
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
    pending_voice_acks: Dict[tuple[str, int], asyncio.Future] = {}
    voice_synthesis_tasks: Dict[str, asyncio.Task] = {}
    run_tasks: Dict[str, asyncio.Task] = {}
    background_tasks: set[asyncio.Task] = set()

    try:
        # Rust sends complete prompt snapshots over this authenticated loopback
        # channel. Valid histories can exceed websockets' 1 MiB default before
        # Python gets a chance to apply the model context budget.
        async with websockets.connect(ws_url, max_size=None) as ws:
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

                elif mtype == MsgType.VOICE_TRANSCRIBE_REQUEST:
                    task = asyncio.create_task(handle_voice_transcribe_request(ws, msg))
                    background_tasks.add(task)
                    task.add_done_callback(background_tasks.discard)

                elif mtype == MsgType.VOICE_SYNTHESIZE_REQUEST:
                    task = asyncio.create_task(handle_voice_synthesize_request(ws, msg))
                    background_tasks.add(task)
                    task.add_done_callback(background_tasks.discard)

                elif mtype == MsgType.VOICE_SYNTHESIZE_STREAM_REQUEST:
                    request_id = msg.get("id", "")
                    task = asyncio.create_task(
                        handle_voice_synthesize_stream_request(ws, msg, pending_voice_acks)
                    )
                    voice_synthesis_tasks[request_id] = task
                    background_tasks.add(task)
                    task.add_done_callback(background_tasks.discard)
                    task.add_done_callback(lambda _task, rid=request_id: voice_synthesis_tasks.pop(rid, None))

                elif mtype == MsgType.VOICE_SYNTHESIZE_ACK:
                    payload = msg.get("payload", {})
                    key = (payload.get("id", ""), payload.get("sequence", -1))
                    ack = pending_voice_acks.get(key)
                    if ack and not ack.done():
                        ack.set_result(True)

                elif mtype == MsgType.VOICE_SYNTHESIZE_CANCEL:
                    request_id = msg.get("payload", {}).get("id", "")
                    task = voice_synthesis_tasks.get(request_id)
                    if task and not task.done():
                        task.cancel()

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
        print(f"[sidecar] 与 Rust 的连接异常断开：{exc}，退出", flush=True)
        sys.exit(0)
    except Exception as e:
        print(f"[sidecar] 发生未预期错误: {e}", file=sys.stderr)
        sys.exit(1)


def run_sidecar() -> int:
    """Run the sidecar CLI and treat an interactive interrupt as a clean exit."""
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("[sidecar] 收到中断，退出", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(run_sidecar())
