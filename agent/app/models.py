"""LiteLLM 接入占位（V0.2 实现：统一多模型厂商 + 模型注册表 max_context_tokens）。"""
from __future__ import annotations


def chat(*args, **kwargs):
    raise NotImplementedError("V0.2 落地 LiteLLM 模型调用")
