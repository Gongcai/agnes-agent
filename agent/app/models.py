"""LiteLLM integration and Model Registry."""
from __future__ import annotations
import os
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
    **kwargs: Any
) -> Any:
    """Wrapper around litellm.completion for uniform calling and logging."""
    # Inject API Keys if they exist in environment variables (passed by Rust)
    # LiteLLM automatically picks up standard env variables like OPENAI_API_KEY, etc.
    return litellm.completion(
        model=model,
        messages=messages,
        tools=tools,
        stream=stream,
        **kwargs
    )
