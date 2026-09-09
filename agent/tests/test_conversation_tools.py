"""Tests for the historical conversation tool schemas."""
from app.graph import get_available_tools


def tool_names(policy):
    return [
        tool["function"]["name"]
        for tool in get_available_tools(policy)
    ]


def test_conversation_tools_are_enabled_by_default():
    names = tool_names({})
    assert "conversation_search" in names
    assert "conversation_read" in names


def test_conversation_tools_can_be_disabled_per_agent():
    names = tool_names({"conversation": {"enabled": False}})
    assert "conversation_search" not in names
    assert "conversation_read" not in names


def test_conversation_read_requires_a_session_id():
    tool = next(
        tool
        for tool in get_available_tools({})
        if tool["function"]["name"] == "conversation_read"
    )
    assert tool["function"]["parameters"]["required"] == ["session_id"]
