"""Short session title generation for the quick task model."""
from __future__ import annotations

import re
from dataclasses import replace
from typing import Optional, TYPE_CHECKING

from .models import completion

if TYPE_CHECKING:
    from .models import LlmConfig


TITLE_MAX_CHARS = 40
TITLE_MAX_TOKENS = 512
TITLE_REQUEST_TIMEOUT_SECONDS = 45


def normalize_session_title(value: object, max_chars: int = TITLE_MAX_CHARS) -> Optional[str]:
    """Turn model output into a single short title, or return None when empty."""
    if not isinstance(value, str):
        return None
    text = re.sub(r"\s+", " ", value).strip()
    text = re.sub(r"^(?:标题|title)\s*[:：]\s*", "", text, flags=re.IGNORECASE)
    text = text.strip("`*_# \"'“”‘’")
    if not text:
        return None
    if len(text) > max_chars:
        text = text[:max_chars].rstrip() + "…"
    return text


def generate_session_title(
    source_text: str,
    model: str,
    llm_config: Optional["LlmConfig"] = None,
) -> Optional[str]:
    """Ask the configured quick model for a concise title without conversation context."""
    prompt = (
        "为下面这条用户消息生成一个简短的会话标题。\n"
        "要求：使用消息的主要语言；中文 4-12 个字，英文不超过 8 个单词；"
        "不要回答或求解消息中的问题，只概括主题；"
        "只返回标题本身，不要引号、Markdown、前缀或解释。\n\n"
        f"用户消息：\n{source_text}"
    )
    try:
        title_config = (
            replace(llm_config, thinking_mode="off", thinking_budget=0)
            if llm_config
            else None
        )
        title_kwargs = {}
        if title_config and title_config.provider in ("openai", "openai_compatible"):
            title_kwargs["extra_body"] = {"thinking": {"type": "disabled"}}
        request = {
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "llm_config": title_config,
            "max_tokens": TITLE_MAX_TOKENS,
            "temperature": 0.2,
            "timeout": TITLE_REQUEST_TIMEOUT_SECONDS,
        }
        try:
            response = completion(**request, **title_kwargs)
        except Exception:
            if not title_kwargs:
                raise
            response = completion(**request)
        content = getattr(response.choices[0].message, "content", None)
        title = normalize_session_title(content)
        if title is None:
            finish_reason = getattr(response.choices[0], "finish_reason", "unknown")
            print(
                f"[sidecar][title] Model returned no title (finish_reason={finish_reason})",
                flush=True,
            )
        return title
    except Exception as error:
        print(f"[sidecar][title] Failed to generate session title: {error}", flush=True)
        return None
