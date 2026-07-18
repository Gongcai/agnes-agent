"""Short session title generation for the quick task model."""
from __future__ import annotations

import re
from typing import Optional, TYPE_CHECKING

from .models import completion

if TYPE_CHECKING:
    from .models import LlmConfig


TITLE_MAX_CHARS = 40


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
        "只返回标题本身，不要引号、Markdown、前缀或解释。\n\n"
        f"用户消息：\n{source_text}"
    )
    try:
        response = completion(
            model=model,
            messages=[{"role": "user", "content": prompt}],
            llm_config=llm_config,
            max_tokens=64,
            temperature=0.2,
            timeout=15,
        )
        content = getattr(response.choices[0].message, "content", None)
        return normalize_session_title(content)
    except Exception as error:
        print(f"[sidecar][title] Failed to generate session title: {error}", flush=True)
        return None
