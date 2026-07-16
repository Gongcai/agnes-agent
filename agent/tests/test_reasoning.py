"""Unit tests for the Python reasoning sidecar (prompt, graph, models, memory_extract)."""
from __future__ import annotations
import json
import pytest
from app.prompt import assemble_prompt, group_protocol_messages, translate_messages, count_tokens
from app.graph import build_graph, get_available_tools
from app.memory_extract import extract_memories
from app.main import resolve_task_llm
from app.models import LlmConfig

def test_count_tokens():
    assert count_tokens("Hello world") > 0
    assert count_tokens("") == 0

def test_translate_messages():
    raw_recent = [
        {
            "role": "user",
            "parts": [
                {"kind": "text", "content": "How's the weather?"}
            ]
        },
        {
            "role": "assistant",
            "parts": [
                {"kind": "thought", "content": "Checking weather API..."},
                {
                    "kind": "tool_call",
                    "content": "Calling git...",
                    "toolCall": {
                        "id": "tc-1",
                        "tool": "git",
                        "args": "[\"status\"]"
                    }
                }
            ]
        },
        {
            "role": "assistant",
            "parts": [
                {"kind": "tool_result", "content": "On branch main", "toolCall": {"id": "tc-1", "tool": "git"}}
            ]
        }
    ]
    
    translated = translate_messages(raw_recent)
    
    assert len(translated) == 3
    assert translated[0]["role"] == "user"
    assert "How's the weather?" in translated[0]["content"]
    
    assert translated[1]["role"] == "assistant"
    assert "<thought>" in translated[1]["content"]
    assert len(translated[1]["tool_calls"]) == 1
    assert translated[1]["tool_calls"][0]["function"]["name"] == "git"
    
    assert translated[2]["role"] == "tool"
    assert translated[2]["tool_call_id"] == "tc-1"
    assert translated[2]["content"] == "On branch main"


def test_translate_interleaved_tool_history_from_one_assistant_message():
    raw_recent = [
        {
            "role": "user",
            "parts": [{"kind": "text", "content": "Inspect the repository"}],
        },
        {
            "role": "assistant",
            "parts": [
                {"kind": "reasoning", "content": "I should list the files."},
                {
                    "kind": "tool_call",
                    "content": "Calling list_files...",
                    "toolCall": {
                        "id": "tc-1",
                        "tool": "list_files",
                        "args": "{\"path\":\".\"}",
                    },
                },
                {
                    "kind": "tool_result",
                    "content": "README.md",
                    "toolCall": {"id": "tc-1", "tool": "list_files"},
                },
                {"kind": "text", "content": "The repository contains a README."},
            ],
        },
    ]

    translated = translate_messages(raw_recent)

    assert [message["role"] for message in translated] == [
        "user",
        "assistant",
        "tool",
        "assistant",
    ]
    assert translated[1]["tool_calls"][0]["id"] == "tc-1"
    assert translated[2]["tool_call_id"] == "tc-1"
    assert translated[3]["content"] == "The repository contains a README."


def test_tool_exchange_is_one_context_budget_group():
    messages = [
        {"role": "user", "content": "Inspect the repository"},
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "tc-1",
                    "type": "function",
                    "function": {"name": "list_files", "arguments": "{}"},
                }
            ],
        },
        {"role": "tool", "tool_call_id": "tc-1", "content": "README.md"},
        {"role": "assistant", "content": "Done"},
    ]

    groups = group_protocol_messages(messages)

    assert [len(group) for group in groups] == [1, 2, 1]
    assert groups[1][0]["role"] == "assistant"
    assert groups[1][1]["role"] == "tool"

def test_assemble_prompt_and_budgeting():
    snapshot = {
        "input": "Latest user question",
        "context": {
            "agent": {
                "persona": "Agnes persona text",
                "systemPrompt": "System instructions text",
                "model": "gpt-4o",
                "toolPolicy": {
                    "shell": {"enabled": True, "approval": "always"}
                }
            },
            "settings": {
                "user_context_limit": 2000  # set a small budget for testing
            },
            "recentMessages": [
                {
                    "role": "user",
                    "parts": [{"kind": "text", "content": "Message A " * 100}]  # around 100 tokens
                },
                {
                    "role": "assistant",
                    "parts": [{"kind": "text", "content": "Message B " * 100}]  # around 100 tokens
                }
            ],
            "explicitMemories": {
                "user_md": "User likes Python",
                "memory_md": "Remember that database is SQLite"
            },
            "summary": "Rolling summary text"
        }
    }
    
    # Run with small budget to force compression / message discarding
    system_prompt, messages, discarded = assemble_prompt(snapshot, reserved_tokens=1500)
    
    assert "Agnes persona text" in system_prompt
    assert "User likes Python" in system_prompt
    assert "Remember that database is SQLite" in system_prompt
    
    # Budget check
    assert len(messages) > 0
    assert messages[-1]["role"] == "user"
    assert messages[-1]["content"] == "Latest user question"

def test_graph_compiles():
    graph = build_graph()
    assert graph is not None
    # Verify nodes exist
    node_names = graph.nodes.keys()
    assert "call_llm" in node_names
    assert "execute_tools" in node_names

def test_get_available_tools():
    policy = {
        "shell": {"enabled": True},
        "file": {"enabled": False},
        "git": {"enabled": True}
    }
    tools = get_available_tools(policy)
    tool_names = [t["function"]["name"] for t in tools]
    
    assert "shell" in tool_names
    assert "file_read" not in tool_names
    assert "file_write" not in tool_names
    assert "file_edit" not in tool_names
    assert "list_files" not in tool_names
    assert "grep" not in tool_names
    assert "apply_patch" not in tool_names
    assert "git" in tool_names
    assert "memory_search" in tool_names
    assert "memory_create" in tool_names
    assert "memory_update" in tool_names
    assert "memory_md_view" in tool_names
    assert "memory_md_edit" in tool_names

    all_tool_names = [
        tool["function"]["name"] for tool in get_available_tools({})
    ]
    assert all_tool_names == [
        "shell",
        "file_read",
        "file_write",
        "file_edit",
        "list_files",
        "grep",
        "apply_patch",
        "git",
        "memory_search",
        "memory_create",
        "memory_update",
        "memory_md_view",
        "memory_md_edit",
    ]

    memory_disabled_names = [
        tool["function"]["name"]
        for tool in get_available_tools({"memory": {"enabled": False}})
    ]
    assert "memory_search" not in memory_disabled_names
    assert "memory_create" not in memory_disabled_names
    assert "memory_update" not in memory_disabled_names
    assert "memory_md_view" not in memory_disabled_names
    assert "memory_md_edit" not in memory_disabled_names


def test_task_model_routing_and_fallback():
    fallback = LlmConfig(model="main", litellm_model="main")
    model, config = resolve_task_llm({}, "summary", "main", fallback)
    assert model == "main"
    assert config is fallback

    model, config = resolve_task_llm(
        {
            "summary": {
                "provider": "openai_compatible",
                "model": "cheap-summary",
                "litellmModel": "openai/cheap-summary",
            }
        },
        "summary",
        "main",
        fallback,
    )
    assert model == "openai/cheap-summary"
    assert config is not None
    assert config.model == "cheap-summary"


def test_memory_extractor_normalizes_new_fields(monkeypatch):
    class Message:
        content = json.dumps({
            "memories": [
                {
                    "name": " Preferred package manager ",
                    "keywords": ["pnpm", " pnpm ", "frontend", ""],
                    "content": " User uses pnpm for frontend dependencies. ",
                    "type": "Preference",
                    "confidence": 0.95,
                    "source": "Use pnpm",
                }
            ]
        })

    class Choice:
        message = Message()

    class Response:
        choices = [Choice()]

    monkeypatch.setattr("app.memory_extract.completion", lambda **_: Response())
    memories = extract_memories([
        {"role": "user", "content": "Use pnpm"},
        {"role": "assistant", "content": "Understood"},
    ])

    assert memories == [{
        "name": "Preferred package manager",
        "keywords": ["pnpm", "frontend"],
        "content": "User uses pnpm for frontend dependencies.",
        "type": "Preference",
        "confidence": 0.95,
        "source": "Use pnpm",
    }]
