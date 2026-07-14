"""LiteLLM integration and Model Registry."""
from __future__ import annotations
import os
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional
import litellm

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
    """根据模型 provider 与思考强度，构造对应的思考参数。

    依据服务商文档（以 DeepSeek 为例，OpenAI 兼容格式）：
    - 思考开关：``{"thinking": {"type": "enabled"}}``（不再手动设置 token 预算，
      交由服务商默认参数决定）。
    - 思考强度（OpenAI / DeepSeek 格式）：``{"reasoning_effort": "low"|"medium"|"high"}``。
    - 思考强度（Anthropic 格式）：``{"output_config": {"effort": "low"|"medium"|"high"}}``。

    ``drop_params=True`` 已开启，因此即使误传给不支持的 provider，
    不被识别的参数也会被自动丢弃，不会引发调用失败。
    """
    if not mode or mode == "off":
        return {}

    model_l = (litellm_model or "").lower()
    effort_map = {"low": "low", "medium": "medium", "high": "high", "auto": "medium"}
    effort = effort_map.get(mode, "medium")

    # OpenAI / OpenAI 兼容（DeepSeek 等）/ o-series：thinking 开关 + reasoning_effort
    if provider in ("openai", "openai_compatible") or any(
        k in model_l for k in ("deepseek", "reasoner", "o1", "o3", "o4")
    ):
        return {
            "thinking": {"type": "enabled"},
            "reasoning_effort": effort,
        }

    # Anthropic：thinking 开关 + output_config.effort
    if provider == "anthropic" or "claude" in model_l:
        return {
            "thinking": {"type": "enabled"},
            "output_config": {"effort": effort},
        }

    # 其它 provider：不注入不支持的参数
    return {}


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
        **extra_kwargs,
        **kwargs,
    )
