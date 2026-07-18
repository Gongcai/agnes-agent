"""Agent Protocol 消息（pydantic 镜像 Rust 真相源，见 src-tauri/src/agent/protocol.rs）。"""
from __future__ import annotations

import time
import uuid
from enum import Enum

from pydantic import BaseModel

PROTOCOL_VERSION = 1


class MsgType(str, Enum):
    HELLO = "hello"
    READY = "ready"
    PING = "ping"
    PONG = "pong"
    RUN_REQUEST = "run_request"
    TOOL_CALL_REQUEST = "tool_call_request"
    TOOL_RESULT = "tool_result"
    APPROVAL_RESULT = "approval_result"
    RUN_CANCEL = "run_cancel"
    RUN_FINISHED = "run_finished"
    RUN_ERROR = "run_error"
    ASSISTANT_DELTA = "assistant_delta"
    MODEL_FALLBACK = "model_fallback"
    MEMORY_QUERY_REQUEST = "memory_query_request"
    USER_MESSAGE = "user_message"
    DEBUG_PROMPT = "debug_prompt"
    DEBUG_PROMPT_RESULT = "debug_prompt_result"
    EMBEDDING_REQUEST = "embedding_request"
    EMBEDDING_RESULT = "embedding_result"


class Envelope(BaseModel):
    protocol_version: int = PROTOCOL_VERSION
    id: str = ""
    run_id: str = ""
    session_id: str = ""
    type: str
    created_at: str = ""
    payload: dict = {}


def make(
    msg_type: str,
    session_id: str = "",
    run_id: str = "",
    payload: dict | None = None,
) -> Envelope:
    return Envelope(
        id=uuid.uuid4().hex,
        run_id=run_id,
        session_id=session_id,
        type=msg_type,
        created_at=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        payload=payload or {},
    )
