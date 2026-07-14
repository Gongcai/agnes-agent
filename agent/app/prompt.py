"""Prompt 拼装占位（V0.2 实现：用 Rust 下发的 ContextSnapshot 做会话预算/压缩与角色卡注入）。"""
from __future__ import annotations


def assemble_prompt(context: dict) -> str:
    raise NotImplementedError("V0.2 落地 prompt 拼装 + 会话预算/压缩")
