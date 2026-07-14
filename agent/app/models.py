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

    @classmethod
    def from_dict(cls, d: dict) -> "LlmConfig":
        """Parse from ContextSnapshot's llmConfig JSON object."""
        if not d:
            return cls()
        return cls(
            provider=d.get("provider", "openai"),
            api_base=d.get("apiBase"),
            api_key=d.get("apiKey"),
            model=d.get("model", "gpt-4o"),
            litellm_model=d.get("litellmModel", d.get("model", "gpt-4o")),
        )


def get_max_context_tokens(model_name: str) -> int:
    """Get the maximum context window size for the given model name."""
    model_lower = model_name.lower()
    for key, limit in MODEL_CONTEXT_LIMITS.items():
        if key in model_lower:
            return limit
    return DEFAULT_CONTEXT_LIMIT

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

    return litellm.completion(
        model=call_model,
        messages=messages,
        tools=tools,
        stream=stream,
        **extra_kwargs,
        **kwargs,
    )
