"""LiteLLM integration and Model Registry."""
from __future__ import annotations
import os
import math
import warnings
from dataclasses import dataclass
from types import SimpleNamespace
from typing import Any, Dict, List, Optional
from urllib.parse import urlparse
import litellm

# litellm 1.92 replaces the Responses API usage object with a chat-style dict
# during its streaming logging path, which makes pydantic emit a noisy
# "Pydantic serializer warnings" UserWarning per completed stream. It is a
# litellm-internal logging artifact with no public flag to disable; suppress
# only that exact warning so our own pydantic models still surface theirs.
warnings.filterwarnings(
    "ignore",
    message=r"Pydantic serializer warnings",
    category=UserWarning,
)


MAX_EMBEDDING_DIMS = 8192
# Several OpenAI-compatible embedding endpoints accept at most five inputs per call.
MAX_EMBEDDING_BATCH_SIZE = 5

# Model Registry mapping model names to their maximum context limits (in tokens)
MODEL_CONTEXT_LIMITS: Dict[str, int] = {
    "gpt-4o": 128_000,
    "gpt-4": 8192,
    "claude-3-5-sonnet": 200_000,
    "deepseek-coder": 64_000,
    "deepseek": 64_000,
    "llama3": 8192,
    "gemma2": 8192,
    "qwen2": 32_000,
}

DEFAULT_CONTEXT_LIMIT = 128_000
MODEL_REQUEST_TIMEOUT_SECONDS = 90
DEFAULT_MAX_OUTPUT_TOKENS = 2048
MAX_OUTPUT_TOKENS = 1_048_576

# Ensure litellm throws errors immediately rather than trying fallback options invisibly
litellm.drop_params = True


@dataclass(frozen=True)
class LlmFailure:
    category: str
    retryable_with_fallback: bool
    display_reason: str


class EmptyModelResponseError(RuntimeError):
    """Raised when a provider completes without text, reasoning, or tool calls."""


def classify_llm_error(error: BaseException) -> LlmFailure:
    """Classify whether a zero-output failure may move to the next configured model."""
    class_name = error.__class__.__name__.lower()
    message = str(error).lower()

    if isinstance(error, EmptyModelResponseError):
        return LlmFailure("empty_response", True, "未返回任何内容")
    if "authentication" in class_name or "permissiondenied" in class_name:
        return LlmFailure("authentication", False, "认证或权限失败")
    if "contentpolicy" in class_name or "content policy" in message:
        return LlmFailure("content_policy", False, "内容策略拒绝")
    if "contextwindow" in class_name or any(
        marker in message
        for marker in ("context length", "context window", "maximum context", "too many tokens")
    ):
        return LlmFailure("context_limit", False, "上下文超过模型限制")
    if "ratelimit" in class_name or "rate limit" in message or "status code: 429" in message:
        return LlmFailure("rate_limit", True, "服务限流")
    if isinstance(error, TimeoutError) or "timeout" in class_name or "timed out" in message:
        return LlmFailure("timeout", True, "请求超时")
    if "badrequest" in class_name and any(
        marker in message
        for marker in (
            "does not support tool",
            "tools are not supported",
            "tool use is not supported",
            "function calling is not supported",
            "unsupported parameter: tools",
        )
    ):
        return LlmFailure("unsupported_capability", True, "模型不支持当前工具能力")
    if "notfound" in class_name or "model not found" in message:
        return LlmFailure("model_unavailable", True, "模型不可用")
    if isinstance(error, (ConnectionError, OSError)) or any(
        marker in class_name
        for marker in ("apiconnection", "serviceunavailable", "internalserver")
    ) or any(
        marker in message
        for marker in (
            "status code: 500",
            "status code: 502",
            "status code: 503",
            "status code: 504",
            "status code: 529",
        )
    ):
        return LlmFailure("service_unavailable", True, "服务暂时不可用")
    if "badrequest" in class_name:
        return LlmFailure("invalid_request", False, "请求参数不被模型接受")
    return LlmFailure("unknown", False, "未知模型错误")


@dataclass
class LlmConfig:
    """LLM connection configuration resolved by Rust from model_providers."""
    # openai / anthropic / ollama / openai_compatible / google / openai_responses
    provider: str = "openai"
    api_base: Optional[str] = None     # Custom API endpoint URL
    api_key: Optional[str] = None      # API key (injected by Rust, never persisted in Python)
    model: str = "gpt-4o"              # Raw model name
    litellm_model: str = "gpt-4o"      # Pre-formatted model string for LiteLLM
    model_ref: Optional[str] = None     # Stable provider/model reference used by local indexes
    thinking_mode: str = "off"         # off | auto | low | medium | high
    thinking_budget: int = 0           # 思考预算(token)：Claude 的 budget_tokens，0 = 按强度预设
    max_tokens: int = DEFAULT_MAX_OUTPUT_TOKENS
    streaming: bool = True             # False = 整段返回（部分端点不支持 SSE 流式）

    @classmethod
    def from_dict(cls, d: dict) -> "LlmConfig":
        """Parse from ContextSnapshot's llmConfig JSON object."""
        if not d:
            return cls()
        thinking = d.get("thinking") or {}
        return cls(
            provider=d.get("provider", "openai"),
            api_base=d.get("apiBase"),
            api_key=d.get("apiKey"),
            model=d.get("model", "gpt-4o"),
            litellm_model=d.get("litellmModel", d.get("model", "gpt-4o")),
            model_ref=d.get("modelRef"),
            thinking_mode=thinking.get("mode", d.get("thinkingMode", "off")) or "off",
            thinking_budget=int(thinking.get("budget", d.get("thinkingBudget", 0)) or 0),
            streaming=bool(d.get("streaming", True)),
            max_tokens=max(
                128,
                min(
                    MAX_OUTPUT_TOKENS,
                    int(d.get("maxTokens", DEFAULT_MAX_OUTPUT_TOKENS) or DEFAULT_MAX_OUTPUT_TOKENS),
                ),
            ),
        )


def get_max_context_tokens(model_name: str) -> int:
    """Get the maximum context window size for the given model name."""
    model_lower = model_name.lower()
    for key, limit in MODEL_CONTEXT_LIMITS.items():
        if key in model_lower:
            return limit
    return DEFAULT_CONTEXT_LIMIT


def supports_stream_usage_options(llm_config: Optional[LlmConfig]) -> bool:
    """Return whether the endpoint is expected to accept OpenAI stream_options."""
    if llm_config is None:
        return True

    # The Responses API has no stream_options parameter.
    if llm_config.provider.lower() == RESPONSES_API_PROVIDER:
        return False

    if llm_config.provider.lower() != "openai":
        return False

    if not llm_config.api_base:
        return True

    hostname = (urlparse(llm_config.api_base).hostname or "").lower()
    return hostname in {"api.openai.com", "api.chatgpt.com"}


def build_thinking_kwargs(
    mode: str,
    litellm_model: str,
    provider: str,
) -> Dict[str, Any]:
    """根据思考强度，构造对应 provider 的思考参数。

    关键：``thinking`` 字段必须通过 ``extra_body`` 透传——OpenAI SDK（及 litellm 的
    openai/openai_compatible 适配器）不原生识别 ``thinking``，若作为顶层参数传入，
    会在 ``drop_params=True`` 时被 litellm 当作未知参数丢弃，导致思考未开启。
    参考服务商官方示例：``reasoning_effort`` 为顶层参数，``thinking`` 走 extra_body。

    - OpenAI / OpenAI 兼容（DeepSeek 等）：``reasoning_effort`` + ``extra_body={"thinking":{"type":"enabled"}}``
    - Anthropic：litellm 原生 ``thinking`` 开关 + ``output_config.effort``
    - 不再手动设置 token 预算，交由服务商默认参数。
    """
    if not mode or mode == "off":
        return {}

    effort_map = {"low": "low", "medium": "medium", "high": "high", "auto": "medium"}
    effort = effort_map.get(mode, "medium")

    # OpenAI / OpenAI 兼容（DeepSeek 等）：统一 OpenAI 格式
    if provider in ("openai", "openai_compatible"):
        return {
            "reasoning_effort": effort,
            "extra_body": {"thinking": {"type": "enabled"}},
        }

    # Anthropic：原生 thinking 开关 + output_config.effort
    if provider == "anthropic":
        return {
            "thinking": {"type": "enabled"},
            "extra_body": {"output_config": {"effort": effort}},
        }

    # 其它 provider（ollama/google 等）：尝试 OpenAI 通用格式，drop_params 兜底
    return {
        "reasoning_effort": effort,
        "extra_body": {"thinking": {"type": "enabled"}},
    }


def _uses_deepseek_thinking_history(
    call_model: str,
    llm_config: Optional[LlmConfig],
) -> bool:
    """Return whether assistant reasoning must be replayed in DeepSeek's wire format."""
    if not llm_config or llm_config.thinking_mode == "off":
        return False

    provider = llm_config.provider.lower()
    model_names = (call_model, llm_config.model, llm_config.litellm_model)
    if provider == "deepseek" or any(
        str(model_name).lower().startswith("deepseek/")
        for model_name in model_names
    ):
        return True

    if llm_config.api_base:
        hostname = (urlparse(llm_config.api_base).hostname or "").lower()
        if hostname == "deepseek.com" or hostname.endswith(".deepseek.com"):
            return True
    return False


def _prepare_completion_messages(
    messages: List[Dict[str, Any]],
    call_model: str,
    llm_config: Optional[LlmConfig],
) -> List[Dict[str, Any]]:
    """Normalize provider-specific assistant reasoning without mutating graph state."""
    replay_reasoning = _uses_deepseek_thinking_history(call_model, llm_config)
    prepared: List[Dict[str, Any]] = []

    for message in messages:
        if message.get("role") != "assistant":
            prepared.append(message)
            continue

        normalized = dict(message)
        provider_fields = normalized.get("provider_specific_fields")
        stored_reasoning = (
            provider_fields.get("reasoning_content")
            if isinstance(provider_fields, dict)
            else None
        )
        if isinstance(provider_fields, dict) and "reasoning_content" in provider_fields:
            cleaned_provider_fields = dict(provider_fields)
            cleaned_provider_fields.pop("reasoning_content", None)
            if cleaned_provider_fields:
                normalized["provider_specific_fields"] = cleaned_provider_fields
            else:
                normalized.pop("provider_specific_fields", None)
        if replay_reasoning:
            # DeepSeek rejects thinking-mode history if any assistant turn omits
            # this field. A blank is appropriate only when no original chain exists
            # (for example a legacy turn created while thinking was disabled).
            normalized["reasoning_content"] = (
                normalized.get("reasoning_content") or stored_reasoning or " "
            )
        else:
            # reasoning_content is not part of the standard OpenAI message schema.
            # Do not leak a prior DeepSeek turn into an unrelated provider request.
            normalized.pop("reasoning_content", None)
            normalized.pop("provider_specific_fields", None)
        prepared.append(normalized)

    return prepared


RESPONSES_API_PROVIDER = "openai_responses"


def _responses_field(value: Any, name: str) -> Any:
    """Read a field from either a dict or an attribute-style object."""
    if value is None:
        return None
    if isinstance(value, dict):
        return value.get(name)
    return getattr(value, name, None)


def _responses_content_part(part: Any) -> Optional[Dict[str, Any]]:
    """Convert one chat-completions content part into a Responses input part."""
    part_type = part.get("type") if isinstance(part, dict) else getattr(part, "type", None)
    if part_type == "text":
        text = _responses_field(part, "text")
        if isinstance(text, str):
            return {"type": "input_text", "text": text}
        return None
    if part_type == "image_url":
        image = _responses_field(part, "image_url")
        url = image.get("url") if isinstance(image, dict) else None
        if isinstance(url, str) and url:
            converted: Dict[str, Any] = {"type": "input_image", "image_url": url}
            detail = image.get("detail") if isinstance(image, dict) else None
            if isinstance(detail, str) and detail:
                converted["detail"] = detail
            return converted
        return None
    return None


def _chat_messages_to_responses_input(
    messages: List[Dict[str, Any]],
) -> tuple[Optional[str], List[Dict[str, Any]]]:
    """Translate chat-completions messages into Responses API (instructions, input)."""
    instructions: Optional[str] = None
    items: List[Dict[str, Any]] = []

    for message in messages:
        role = message.get("role")
        content = message.get("content")

        if role == "system":
            text = content if isinstance(content, str) else ""
            if instructions is None:
                instructions = text
            elif text:
                items.append({"role": "system", "content": text})
            continue

        if role == "tool":
            output = content if isinstance(content, str) else str(content or "")
            items.append({
                "type": "function_call_output",
                "call_id": str(message.get("tool_call_id") or ""),
                "output": output,
            })
            continue

        if role == "assistant":
            # Replay opaque reasoning items so the model keeps its chain of
            # thought across tool loops (stateless reasoning replay).
            provider_fields = message.get("provider_specific_fields")
            reasoning_items = (
                provider_fields.get("responses_reasoning")
                if isinstance(provider_fields, dict)
                else None
            )
            if isinstance(reasoning_items, list):
                for reasoning_item in reasoning_items:
                    if isinstance(reasoning_item, dict) and reasoning_item.get("type") == "reasoning":
                        items.append(reasoning_item)
            text = content if isinstance(content, str) else None
            if text:
                items.append({"role": "assistant", "content": text})
            for tool_call in message.get("tool_calls") or []:
                function = tool_call.get("function") or {}
                items.append({
                    "type": "function_call",
                    "call_id": str(tool_call.get("id") or ""),
                    "name": str(function.get("name") or ""),
                    "arguments": str(function.get("arguments") or ""),
                })
            continue

        # user (default)
        if isinstance(content, str):
            items.append({"role": "user", "content": content})
        elif isinstance(content, list):
            converted = [
                part for part in (_responses_content_part(p) for p in content) if part
            ]
            if converted:
                items.append({"role": "user", "content": converted})
        else:
            items.append({"role": "user", "content": content})

    return instructions, items


def _chat_tools_to_responses_tools(
    tools: Optional[List[Dict[str, Any]]],
) -> Optional[List[Dict[str, Any]]]:
    """Flatten chat tool definitions into the Responses API function tool shape."""
    converted: List[Dict[str, Any]] = []
    for tool in tools or []:
        if not isinstance(tool, dict) or tool.get("type") != "function":
            continue
        function = tool.get("function") or {}
        name = function.get("name")
        if not isinstance(name, str) or not name:
            continue
        entry: Dict[str, Any] = {"type": "function", "name": name}
        if isinstance(function.get("description"), str) and function["description"]:
            entry["description"] = function["description"]
        if isinstance(function.get("parameters"), dict):
            entry["parameters"] = function["parameters"]
        converted.append(entry)
    return converted or None


def _responses_reasoning(mode: str) -> Dict[str, Any]:
    """Map the thinking-mode setting to the Responses API reasoning parameter.

    Reasoning summaries are always requested: without ``summary`` the endpoint
    never emits ``response.reasoning_summary_text.delta``, so the UI shows no
    thinking at all for models that always reason (gpt-5 and friends). The
    effort level is only pinned when the user picked a strength.
    """
    reasoning: Dict[str, Any] = {"summary": "auto"}
    effort = {"low": "low", "medium": "medium", "high": "high", "auto": "medium"}.get(mode or "")
    if effort:
        reasoning["effort"] = effort
    return reasoning


def _responses_usage_to_chat(usage: Any) -> Optional[SimpleNamespace]:
    """Normalize Responses usage into the chat-completions usage shape."""
    if usage is None:
        return None

    input_tokens = _responses_field(usage, "prompt_tokens") or _responses_field(
        usage, "input_tokens"
    )
    output_tokens = _responses_field(usage, "completion_tokens") or _responses_field(
        usage, "output_tokens"
    )
    details = _responses_field(usage, "prompt_tokens_details") or _responses_field(
        usage, "input_tokens_details"
    )
    cached = _responses_field(usage, "cache_read_input_tokens") or _responses_field(
        usage, "cached_tokens"
    )
    if not cached:
        cached = _responses_field(details, "cached_tokens")

    try:
        input_tokens = max(0, int(input_tokens or 0))
        output_tokens = max(0, int(output_tokens or 0))
        cached = max(0, int(cached or 0))
    except (TypeError, ValueError):
        input_tokens, output_tokens, cached = 0, 0, 0
    return SimpleNamespace(
        prompt_tokens=input_tokens,
        completion_tokens=output_tokens,
        cached_tokens=cached,
        prompt_tokens_details=SimpleNamespace(cached_tokens=cached),
        input_tokens=input_tokens,
        output_tokens=output_tokens,
    )


def _chat_stream_chunk(
    content: Optional[str] = None,
    reasoning: Optional[str] = None,
    tool_calls: Optional[List[SimpleNamespace]] = None,
    usage: Optional[SimpleNamespace] = None,
) -> SimpleNamespace:
    """Build a chat-completions-shaped chunk from Responses stream data."""
    delta = SimpleNamespace(
        content=content,
        reasoning_content=reasoning,
        tool_calls=tool_calls,
    )
    return SimpleNamespace(choices=[SimpleNamespace(delta=delta)], usage=usage)


def _responses_stream_error(event: Any) -> RuntimeError:
    """Turn a failure event into an exception classifiable by classify_llm_error."""
    response = _responses_field(event, "response")
    error = _responses_field(response, "error") or _responses_field(event, "error")
    code = _responses_field(error, "code")
    message = _responses_field(error, "message")
    if not message:
        message = _responses_field(event, "message")
    detail = f"Responses API stream failed: {message or 'unknown error'}"
    if code:
        detail = f"{detail} (code: {code})"
    return RuntimeError(detail)


def _reasoning_item_dict(item: Any) -> Optional[Dict[str, Any]]:
    """Extract the replayable fields of a Responses reasoning item."""
    item_id = getattr(item, "id", None)
    if not item_id:
        return None
    return {
        "type": "reasoning",
        "id": item_id,
        "encrypted_content": getattr(item, "encrypted_content", None),
    }


def _iter_responses_events_as_chat_chunks(
    event_stream: Any,
    reasoning_sink: Optional[Dict[str, Dict[str, Any]]] = None,
) -> Any:
    """Yield chat-completions chunks synthesized from Responses API stream events.

    When ``reasoning_sink`` is provided, reasoning items are recorded there (keyed
    by item id, in stream order) so the caller can replay them on later turns.
    """
    for event in event_stream:
        event_type = getattr(event, "type", None)

        if event_type == "response.output_text.delta":
            text = getattr(event, "delta", None)
            if text:
                yield _chat_stream_chunk(content=text)

        elif event_type == "response.reasoning_summary_text.delta":
            text = getattr(event, "delta", None)
            if text:
                yield _chat_stream_chunk(reasoning=text)

        elif event_type in ("response.output_item.added", "response.output_item.done"):
            item = getattr(event, "item", None)
            if getattr(item, "type", None) == "function_call":
                if event_type == "response.output_item.added":
                    yield _chat_stream_chunk(tool_calls=[SimpleNamespace(
                        index=getattr(event, "output_index", 0) or 0,
                        id=getattr(item, "call_id", None),
                        function=SimpleNamespace(
                            name=getattr(item, "name", None),
                            arguments=None,
                        ),
                    )])
            elif getattr(item, "type", None) == "reasoning" and reasoning_sink is not None:
                captured = _reasoning_item_dict(item)
                if captured:
                    reasoning_sink[captured["id"]] = captured

        elif event_type == "response.function_call_arguments.delta":
            text = getattr(event, "delta", None)
            if text:
                yield _chat_stream_chunk(tool_calls=[SimpleNamespace(
                    index=getattr(event, "output_index", 0) or 0,
                    id=None,
                    function=SimpleNamespace(name=None, arguments=text),
                )])

        elif event_type == "response.completed":
            response = getattr(event, "response", None)
            usage = _responses_usage_to_chat(getattr(response, "usage", None))
            if usage is not None:
                yield _chat_stream_chunk(usage=usage)

        elif event_type in ("response.failed", "error"):
            raise _responses_stream_error(event)


def _responses_final_to_chat_response(response: Any) -> SimpleNamespace:
    """Build a chat-completions-shaped response from a final Responses object."""
    text_parts: List[str] = []
    reasoning_parts: List[str] = []
    tool_calls: List[Dict[str, Any]] = []
    for item in getattr(response, "output", None) or []:
        item_type = getattr(item, "type", None)
        if item_type == "message":
            for part in getattr(item, "content", None) or []:
                if getattr(part, "type", None) == "output_text":
                    text = getattr(part, "text", None)
                    if isinstance(text, str):
                        text_parts.append(text)
        elif item_type == "function_call":
            tool_calls.append({
                "id": getattr(item, "call_id", None) or "",
                "type": "function",
                "function": {
                    "name": getattr(item, "name", None) or "",
                    "arguments": getattr(item, "arguments", None) or "",
                },
            })
        elif item_type == "reasoning":
            for part in getattr(item, "summary", None) or []:
                if getattr(part, "type", None) == "summary_text":
                    text = getattr(part, "text", None)
                    if isinstance(text, str):
                        reasoning_parts.append(text)
    message = SimpleNamespace(
        content="".join(text_parts) or None,
        reasoning_content="".join(reasoning_parts) or None,
        tool_calls=tool_calls or None,
    )
    return SimpleNamespace(
        choices=[SimpleNamespace(
            message=message,
            finish_reason="tool_calls" if tool_calls else "stop",
        )],
        usage=_responses_usage_to_chat(getattr(response, "usage", None)),
    )


def _chat_response_to_chunks(response: Any) -> Any:
    """Wrap a final (non-streaming) chat response as chunk-iterable for graph.py."""
    message = response.choices[0].message
    tool_calls = []
    for index, tc in enumerate(getattr(message, "tool_calls", None) or []):
        function = getattr(tc, "function", None)
        tool_calls.append(SimpleNamespace(
            index=getattr(tc, "index", index) or 0,
            id=getattr(tc, "id", None),
            function=SimpleNamespace(
                name=getattr(function, "name", None) if function else None,
                arguments=getattr(function, "arguments", None) if function else None,
            ),
        ))
    delta = SimpleNamespace(
        content=getattr(message, "content", None),
        reasoning_content=getattr(message, "reasoning_content", None),
        tool_calls=tool_calls or None,
    )
    yield SimpleNamespace(choices=[SimpleNamespace(delta=delta)], usage=None)
    usage = getattr(response, "usage", None)
    if usage is not None:
        yield SimpleNamespace(
            choices=[SimpleNamespace(delta=SimpleNamespace(
                content=None, reasoning_content=None, tool_calls=None,
            ))],
            usage=usage,
        )


def _strip_reasoning_items(input_items: Any) -> Any:
    """Drop replayed reasoning items from an input list (fallback requests)."""
    if not isinstance(input_items, list):
        return input_items
    return [
        item for item in input_items
        if not (isinstance(item, dict) and item.get("type") == "reasoning")
    ]


def _is_reasoning_retryable(error: BaseException) -> bool:
    """Return whether a request error is worth one retry without reasoning state."""
    message = str(error).lower()
    if any(marker in message for marker in (
        "invalid_encrypted_content",
        "encrypted content",
        "encrypted_content",
    )):
        return True
    return classify_llm_error(error).category == "invalid_request"


class ResponsesChunkStream:
    """Iterable of chat chunks carrying the reasoning items captured for replay."""

    def __init__(self, iterator: Any, reasoning_items: List[Dict[str, Any]]):
        self._iterator = iterator
        self.reasoning_items = reasoning_items

    def __iter__(self) -> Any:
        return iter(self._iterator)


class ResponsesStream:
    """Streaming variant that captures reasoning items as it is consumed.

    ``stream_factory`` returns the litellm stream (deferred so a synchronous
    request error can be retried). ``retry_factory`` re-issues the request
    without reasoning replay and without the ``include``/``store`` flags when
    the endpoint rejects them.
    """

    def __init__(self, stream_factory: Any, retry_factory: Optional[Any] = None):
        self._stream_factory = stream_factory
        self._retry_factory = retry_factory
        self.reasoning_items: List[Dict[str, Any]] = []

    def __iter__(self) -> Any:
        reasoning: Dict[str, Dict[str, Any]] = {}
        yielded = False
        try:
            stream = self._stream_factory()
        except Exception as error:
            if self._retry_factory is None or not _is_reasoning_retryable(error):
                raise
            stream = self._retry_factory()
        try:
            for chunk in _iter_responses_events_as_chat_chunks(stream, reasoning):
                yielded = True
                yield chunk
        except Exception as error:
            if not yielded and self._retry_factory is not None and _is_reasoning_retryable(error):
                for chunk in _iter_responses_events_as_chat_chunks(
                    self._retry_factory(), reasoning
                ):
                    yield chunk
                return
            raise
        finally:
            self.reasoning_items = list(reasoning.values())


def _extract_reasoning_items_from_output(response: Any) -> List[Dict[str, Any]]:
    """Collect reasoning items from a final (non-streaming) Responses object."""
    collected: Dict[str, Dict[str, Any]] = {}
    for item in getattr(response, "output", None) or []:
        if getattr(item, "type", None) == "reasoning":
            captured = _reasoning_item_dict(item)
            if captured:
                collected[captured["id"]] = captured
    return list(collected.values())


_registered_native_streaming_models: set = set()


def _ensure_responses_native_streaming(model: str) -> None:
    """Opt litellm-unknown models out of litellm's Responses fake-stream path.

    ``OpenAIResponsesAPIConfig.should_fake_stream`` treats any model missing
    from litellm's cost map as non-streamable (``supports_native_streaming``
    returns False when the model lookup raises) and then issues a single-shot
    request, replaying the finished response as one burst. Custom api_base
    endpoints use model names litellm has never heard of, so streaming was
    silently broken for them even with the streaming toggle enabled. Register
    such models as native-streaming; the ``LlmConfig.streaming`` toggle stays
    the only authority for downgrading to single-shot.
    """
    if model in _registered_native_streaming_models:
        return
    _registered_native_streaming_models.add(model)
    try:
        litellm.get_model_info(model=model)
    except Exception:
        try:
            litellm.register_model({model: {"supports_native_streaming": True}})
        except Exception:
            pass


def responses_completion(
    model: str,
    messages: List[Dict[str, Any]],
    tools: Optional[List[Dict[str, Any]]] = None,
    stream: bool = False,
    llm_config: Optional[LlmConfig] = None,
    **kwargs: Any
) -> Any:
    """Chat-completions-compatible wrapper around litellm.responses().

    Converts chat messages/tools into Responses API format and converts the
    streamed events (or final object) back into the chat-completions shapes
    consumed by graph.py, title.py, and image.py. Reasoning items are captured
    and replayed so the model keeps its chain of thought across tool loops.
    """
    instructions, input_items = _chat_messages_to_responses_input(messages)

    call_model = model
    request_kwargs: Dict[str, Any] = {}
    if llm_config:
        call_model = llm_config.litellm_model or model
        if llm_config.api_base:
            request_kwargs["api_base"] = llm_config.api_base
        if llm_config.api_key:
            request_kwargs["api_key"] = llm_config.api_key
        reasoning = _responses_reasoning(llm_config.thinking_mode)
        if reasoning:
            request_kwargs["reasoning"] = reasoning

    responses_tools = _chat_tools_to_responses_tools(tools)
    if responses_tools:
        request_kwargs["tools"] = responses_tools

    request_timeout = kwargs.pop("timeout", MODEL_REQUEST_TIMEOUT_SECONDS)
    max_tokens = kwargs.pop(
        "max_tokens",
        llm_config.max_tokens if llm_config else DEFAULT_MAX_OUTPUT_TOKENS,
    )
    kwargs.pop("stream_options", None)
    # The chat "thinking" toggle has no Responses API equivalent; keep other
    # extra_body entries (if any) transparent.
    extra_body = kwargs.pop("extra_body", None)
    if isinstance(extra_body, dict):
        extra_body = {key: value for key, value in extra_body.items() if key != "thinking"}
        if extra_body:
            request_kwargs["extra_body"] = extra_body

    def call(reasoning_replay: bool) -> Any:
        params: Dict[str, Any] = dict(
            model=call_model,
            instructions=instructions,
            stream=stream,
            timeout=request_timeout,
            max_output_tokens=max_tokens,
            **request_kwargs,
            **kwargs,
        )
        if reasoning_replay:
            params["input"] = input_items if input_items else ""
            params["include"] = ["reasoning.encrypted_content"]
            params["store"] = False
        else:
            params["input"] = _strip_reasoning_items(input_items) or ""
        return litellm.responses(**params)

    if stream:
        _ensure_responses_native_streaming(call_model)
        return ResponsesStream(
            stream_factory=lambda: call(reasoning_replay=True),
            retry_factory=lambda: call(False),
        )

    try:
        response = call(reasoning_replay=True)
    except Exception as error:
        if _is_reasoning_retryable(error):
            response = call(reasoning_replay=False)
        else:
            raise
    result = _responses_final_to_chat_response(response)
    result.reasoning_items = _extract_reasoning_items_from_output(response)
    return result


def completion(
    model: str,
    messages: List[Dict[str, Any]],
    tools: Optional[List[Dict[str, Any]]] = None,
    stream: bool = False,
    llm_config: Optional[LlmConfig] = None,
    **kwargs: Any
) -> Any:
    """Wrapper around litellm.completion with provider config support."""
    # A streaming request is downgraded to a single-shot request when the
    # agent's streaming toggle is off (some endpoints lack SSE streaming).
    effective_stream = bool(stream) and (llm_config is None or llm_config.streaming)

    # OpenAI Responses API providers go through the dedicated adapter.
    if llm_config and llm_config.provider.lower() == RESPONSES_API_PROVIDER:
        result = responses_completion(
            model=model,
            messages=messages,
            tools=tools,
            stream=effective_stream,
            llm_config=llm_config,
            **kwargs,
        )
        if effective_stream:
            return result
        if stream:
            # Streaming was requested but the toggle disabled it: wrap the
            # final response as chunks while preserving the captured reasoning.
            return ResponsesChunkStream(
                _chat_response_to_chunks(result),
                getattr(result, "reasoning_items", []) or [],
            )
        return result

    # Inject API Keys if they exist in environment variables (passed by Rust)
    # LiteLLM automatically picks up standard env variables like OPENAI_API_KEY, etc.
    call_model = model
    extra_kwargs: Dict[str, Any] = {}

    if llm_config:
        # Use the pre-formatted litellm model string
        call_model = llm_config.litellm_model or model
        if llm_config.api_base:
            extra_kwargs["api_base"] = llm_config.api_base
        if llm_config.api_key:
            extra_kwargs["api_key"] = llm_config.api_key
        # 思考模式：按 provider 注入对应参数（litellm 会丢弃不支持的参数）
        thinking_kwargs = build_thinking_kwargs(
            llm_config.thinking_mode,
            call_model,
            llm_config.provider,
        )
        extra_kwargs.update(thinking_kwargs)

    request_timeout = kwargs.pop("timeout", MODEL_REQUEST_TIMEOUT_SECONDS)
    max_tokens = kwargs.pop(
        "max_tokens",
        llm_config.max_tokens if llm_config else DEFAULT_MAX_OUTPUT_TOKENS,
    )
    prepared_messages = _prepare_completion_messages(messages, call_model, llm_config)
    if effective_stream:
        return litellm.completion(
            model=call_model,
            messages=prepared_messages,
            tools=tools,
            stream=True,
            timeout=request_timeout,
            max_tokens=max_tokens,
            **extra_kwargs,
            **kwargs,
        )
    result = litellm.completion(
        model=call_model,
        messages=prepared_messages,
        tools=tools,
        stream=False,
        timeout=request_timeout,
        max_tokens=max_tokens,
        **extra_kwargs,
        **kwargs,
    )
    return _chat_response_to_chunks(result) if stream else result


def embed_texts(
    model: str,
    inputs: List[str],
    llm_config: Optional[LlmConfig] = None,
) -> List[List[float]]:
    """Generate finite, consistently sized embeddings through LiteLLM."""
    if not inputs or any(not isinstance(value, str) or not value.strip() for value in inputs):
        raise ValueError("Embedding inputs must contain non-empty strings")

    call_model = model
    extra_kwargs: Dict[str, Any] = {}
    if llm_config:
        call_model = llm_config.litellm_model or model
        if llm_config.api_base:
            extra_kwargs["api_base"] = llm_config.api_base
        if llm_config.api_key:
            extra_kwargs["api_key"] = llm_config.api_key

    vectors: List[List[float]] = []
    expected_dims: Optional[int] = None

    for offset in range(0, len(inputs), MAX_EMBEDDING_BATCH_SIZE):
        batch = inputs[offset : offset + MAX_EMBEDDING_BATCH_SIZE]
        response = litellm.embedding(
            model=call_model,
            input=batch,
            timeout=MODEL_REQUEST_TIMEOUT_SECONDS,
            **extra_kwargs,
        )
        data = getattr(response, "data", None)
        if data is None and isinstance(response, dict):
            data = response.get("data")
        if not isinstance(data, list):
            raise ValueError("Embedding provider returned no data array")

        ordered = sorted(
            data,
            key=lambda item: (
                item.get("index", 0)
                if isinstance(item, dict)
                else getattr(item, "index", 0)
            ),
        )
        if len(ordered) != len(batch):
            raise ValueError("Embedding provider returned an unexpected vector count")
        for item in ordered:
            raw_vector = (
                item.get("embedding")
                if isinstance(item, dict)
                else getattr(item, "embedding", None)
            )
            if not isinstance(raw_vector, list) or not raw_vector:
                raise ValueError("Embedding provider returned an empty vector")
            vector = [float(value) for value in raw_vector]
            if len(vector) > MAX_EMBEDDING_DIMS:
                raise ValueError(
                    f"Embedding provider returned more than {MAX_EMBEDDING_DIMS} dimensions"
                )
            if any(not math.isfinite(value) for value in vector):
                raise ValueError("Embedding provider returned a non-finite vector")
            if expected_dims is None:
                expected_dims = len(vector)
            elif len(vector) != expected_dims:
                raise ValueError("Embedding provider returned inconsistent dimensions")
            vectors.append(vector)

    if len(vectors) != len(inputs):
        raise ValueError("Embedding provider returned an unexpected vector count")
    return vectors
