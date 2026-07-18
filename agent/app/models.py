"""LiteLLM integration and Model Registry."""
from __future__ import annotations
import os
import math
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional
import litellm


MAX_EMBEDDING_DIMS = 8192

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

DEFAULT_CONTEXT_LIMIT = 8192
MODEL_REQUEST_TIMEOUT_SECONDS = 90
DEFAULT_MAX_OUTPUT_TOKENS = 2048

# Ensure litellm throws errors immediately rather than trying fallback options invisibly
litellm.drop_params = True


@dataclass
class LlmConfig:
    """LLM connection configuration resolved by Rust from model_providers."""
    provider: str = "openai"           # openai / anthropic / ollama / openai_compatible / google
    api_base: Optional[str] = None     # Custom API endpoint URL
    api_key: Optional[str] = None      # API key (injected by Rust, never persisted in Python)
    model: str = "gpt-4o"              # Raw model name
    litellm_model: str = "gpt-4o"      # Pre-formatted model string for LiteLLM
    model_ref: Optional[str] = None     # Stable provider/model reference used by local indexes
    thinking_mode: str = "off"         # off | auto | low | medium | high
    thinking_budget: int = 0           # 思考预算(token)：Claude 的 budget_tokens，0 = 按强度预设

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
        )


def get_max_context_tokens(model_name: str) -> int:
    """Get the maximum context window size for the given model name."""
    model_lower = model_name.lower()
    for key, limit in MODEL_CONTEXT_LIMITS.items():
        if key in model_lower:
            return limit
    return DEFAULT_CONTEXT_LIMIT

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


def completion(
    model: str,
    messages: List[Dict[str, Any]],
    tools: Optional[List[Dict[str, Any]]] = None,
    stream: bool = False,
    llm_config: Optional[LlmConfig] = None,
    **kwargs: Any
) -> Any:
    """Wrapper around litellm.completion with provider config support."""
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

    return litellm.completion(
        model=call_model,
        messages=messages,
        tools=tools,
        stream=stream,
        timeout=MODEL_REQUEST_TIMEOUT_SECONDS,
        max_tokens=DEFAULT_MAX_OUTPUT_TOKENS,
        **extra_kwargs,
        **kwargs,
    )


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

    response = litellm.embedding(model=call_model, input=inputs, **extra_kwargs)
    data = getattr(response, "data", None)
    if data is None and isinstance(response, dict):
        data = response.get("data")
    if not isinstance(data, list):
        raise ValueError("Embedding provider returned no data array")

    ordered = sorted(
        data,
        key=lambda item: (
            item.get("index", 0) if isinstance(item, dict) else getattr(item, "index", 0)
        ),
    )
    vectors: List[List[float]] = []
    expected_dims: Optional[int] = None
    for item in ordered:
        raw_vector = item.get("embedding") if isinstance(item, dict) else getattr(item, "embedding", None)
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
