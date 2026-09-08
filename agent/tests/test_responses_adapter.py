"""Unit tests for the OpenAI Responses API adapter in app.models."""
from __future__ import annotations
import asyncio
from types import SimpleNamespace

import pytest

from app.models import (
    LlmConfig,
    RESPONSES_API_PROVIDER,
    _chat_messages_to_responses_input,
    _chat_tools_to_responses_tools,
    _iter_responses_events_as_chat_chunks,
    _responses_final_to_chat_response,
    _responses_reasoning,
    _responses_usage_to_chat,
    classify_llm_error,
    completion,
    responses_completion,
    supports_stream_usage_options,
)


def _config(**overrides) -> LlmConfig:
    base = {
        "provider": RESPONSES_API_PROVIDER,
        "model": "gpt-5",
        "litellm_model": "openai/gpt-5",
        "api_base": "https://api.openai.com/v1",
        "api_key": "sk-test",
    }
    base.update(overrides)
    return LlmConfig(**base)


def _event_stream(*events):
    return iter(events)


def test_system_prompt_becomes_instructions_and_later_system_messages_stay_items():
    instructions, items = _chat_messages_to_responses_input([
        {"role": "system", "content": "You are Agnes."},
        {"role": "user", "content": "Hello"},
        {"role": "system", "content": "Answer in Chinese."},
    ])

    assert instructions == "You are Agnes."
    assert items == [
        {"role": "user", "content": "Hello"},
        {"role": "system", "content": "Answer in Chinese."},
    ]


def test_tool_exchange_becomes_function_call_and_output_items():
    messages = [
        {"role": "user", "content": "Inspect the repository"},
        {
            "role": "assistant",
            "content": "Checking.",
            "tool_calls": [{
                "id": "call-1",
                "type": "function",
                "function": {"name": "list_files", "arguments": '{"path":"."}'},
            }],
        },
        {"role": "tool", "tool_call_id": "call-1", "name": "list_files", "content": "README.md"},
    ]

    instructions, items = _chat_messages_to_responses_input(messages)

    assert instructions is None
    assert items == [
        {"role": "user", "content": "Inspect the repository"},
        {"role": "assistant", "content": "Checking."},
        {
            "type": "function_call",
            "call_id": "call-1",
            "name": "list_files",
            "arguments": '{"path":"."}',
        },
        {
            "type": "function_call_output",
            "call_id": "call-1",
            "output": "README.md",
        },
    ]


def test_assistant_reasoning_is_not_replayed_as_an_input_item():
    _, items = _chat_messages_to_responses_input([
        {"role": "user", "content": "Question"},
        {
            "role": "assistant",
            "content": "Answer",
            "reasoning_content": "Internal chain.",
        },
    ])

    assert items == [
        {"role": "user", "content": "Question"},
        {"role": "assistant", "content": "Answer"},
    ]


def test_vision_content_parts_are_converted_to_responses_input_parts():
    _, items = _chat_messages_to_responses_input([{
        "role": "user",
        "content": [
            {"type": "text", "text": "Describe this image"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA", "detail": "high"}},
            {"type": "unsupported", "text": "ignored"},
        ],
    }])

    assert items == [{
        "role": "user",
        "content": [
            {"type": "input_text", "text": "Describe this image"},
            {"type": "input_image", "image_url": "data:image/png;base64,AAA", "detail": "high"},
        ],
    }]


def test_chat_tools_are_flattened_into_responses_function_tools():
    tools = _chat_tools_to_responses_tools([{
        "type": "function",
        "function": {
            "name": "get_current_time",
            "description": "Return the current time.",
            "parameters": {"type": "object", "properties": {}},
        },
    }])

    assert tools == [{
        "type": "function",
        "name": "get_current_time",
        "description": "Return the current time.",
        "parameters": {"type": "object", "properties": {}},
    }]
    assert _chat_tools_to_responses_tools(None) is None
    assert _chat_tools_to_responses_tools([{"type": "function", "function": {}}]) is None


def test_reasoning_requests_summaries_and_pins_effort_only_when_chosen():
    # Summaries must always be on: without them reasoning models stream no
    # thinking deltas and the UI shows nothing.
    assert _responses_reasoning("off") == {"summary": "auto"}
    assert _responses_reasoning("") == {"summary": "auto"}
    assert _responses_reasoning("low") == {"effort": "low", "summary": "auto"}
    assert _responses_reasoning("high") == {"effort": "high", "summary": "auto"}
    assert _responses_reasoning("auto") == {"effort": "medium", "summary": "auto"}
    assert _responses_reasoning("unknown") == {"summary": "auto"}


def test_stream_events_become_chat_chunks_with_text_reasoning_and_tool_calls():
    events = [
        SimpleNamespace(type="response.output_text.delta", delta="Hello "),
        SimpleNamespace(type="response.output_text.delta", delta="world"),
        SimpleNamespace(type="response.reasoning_summary_text.delta", delta="Thinking"),
        SimpleNamespace(
            type="response.output_item.added",
            output_index=1,
            item=SimpleNamespace(type="function_call", call_id="call-1", name="shell"),
        ),
        SimpleNamespace(
            type="response.function_call_arguments.delta",
            output_index=1,
            delta='{"command":"ls',
        ),
        SimpleNamespace(
            type="response.function_call_arguments.delta",
            output_index=1,
            delta='"}',
        ),
        SimpleNamespace(type="response.output_text.delta", delta=""),
    ]

    chunks = list(_iter_responses_events_as_chat_chunks(_event_stream(*events)))

    # The trailing empty text delta is discarded instead of emitting an empty chunk.
    assert [chunk.choices[0].delta.content for chunk in chunks] == [
        "Hello ", "world", None, None, None, None,
    ]
    reasoning = [chunk.choices[0].delta.reasoning_content for chunk in chunks]
    assert [value for value in reasoning if value] == ["Thinking"]

    tool_deltas = [
        delta
        for chunk in chunks
        if chunk.choices[0].delta.tool_calls
        for delta in chunk.choices[0].delta.tool_calls
    ]
    assert tool_deltas[0].index == 1
    assert tool_deltas[0].id == "call-1"
    assert tool_deltas[0].function.name == "shell"
    assert tool_deltas[0].function.arguments is None
    assert "".join(
        delta.function.arguments for delta in tool_deltas[1:] if delta.function.arguments
    ) == '{"command":"ls"}'


def test_stream_completion_event_carries_normalized_usage():
    response = SimpleNamespace(usage=SimpleNamespace(
        input_tokens=120,
        output_tokens=30,
        input_tokens_details=SimpleNamespace(cached_tokens=80),
    ))
    events = [
        SimpleNamespace(type="response.output_text.delta", delta="done"),
        SimpleNamespace(type="response.completed", response=response),
    ]

    chunks = list(_iter_responses_events_as_chat_chunks(_event_stream(*events)))

    assert chunks[0].usage is None
    assert chunks[1].usage.prompt_tokens == 120
    assert chunks[1].usage.completion_tokens == 30
    assert chunks[1].usage.cached_tokens == 80
    assert chunks[1].usage.prompt_tokens_details.cached_tokens == 80


def test_usage_normalization_accepts_responses_and_chat_field_names():
    responses_usage = SimpleNamespace(
        input_tokens=10,
        output_tokens=5,
        input_tokens_details=SimpleNamespace(cached_tokens=4),
    )
    chat_usage = {
        "prompt_tokens": 7,
        "completion_tokens": 3,
        "prompt_tokens_details": {"cached_tokens": 2},
    }

    assert _responses_usage_to_chat(responses_usage).cached_tokens == 4
    assert _responses_usage_to_chat(chat_usage).prompt_tokens == 7
    assert _responses_usage_to_chat(None) is None


def test_stream_failure_events_raise_a_classifiable_error():
    failed = SimpleNamespace(
        type="response.failed",
        response=SimpleNamespace(
            error=SimpleNamespace(code="rate_limit_exceeded", message="Rate limit reached"),
        ),
    )

    with pytest.raises(RuntimeError) as raised:
        list(_iter_responses_events_as_chat_chunks(_event_stream(failed)))

    failure = classify_llm_error(raised.value)
    assert failure.category == "rate_limit"
    assert failure.retryable_with_fallback


def test_non_streaming_response_is_converted_to_chat_shape():
    response = SimpleNamespace(
        output=[
            SimpleNamespace(type="message", content=[
                SimpleNamespace(type="output_text", text="The time is 10:00."),
            ]),
            SimpleNamespace(
                type="function_call",
                call_id="call-9",
                name="get_current_time",
                arguments="{}",
            ),
        ],
        usage=SimpleNamespace(input_tokens=10, output_tokens=4, input_tokens_details=None),
    )

    converted = _responses_final_to_chat_response(response)

    assert converted.choices[0].message.content == "The time is 10:00."
    assert converted.choices[0].finish_reason == "tool_calls"
    assert converted.choices[0].message.tool_calls[0]["id"] == "call-9"
    assert converted.usage.prompt_tokens == 10


def test_responses_completion_builds_a_responses_api_request(monkeypatch):
    captured = {}

    def fake_responses(**kwargs):
        captured.update(kwargs)
        return SimpleNamespace(output=[], usage=None)

    monkeypatch.setattr("app.models.litellm.responses", fake_responses)
    messages = [
        {"role": "system", "content": "System prompt"},
        {"role": "user", "content": "Hi"},
    ]
    tools = [{
        "type": "function",
        "function": {"name": "shell", "parameters": {"type": "object"}},
    }]

    responses_completion(
        "openai/gpt-5",
        messages,
        tools=tools,
        llm_config=_config(thinking_mode="high"),
        timeout=30,
        stream_options={"include_usage": True},
    )

    assert captured["model"] == "openai/gpt-5"
    assert captured["instructions"] == "System prompt"
    assert captured["input"] == [{"role": "user", "content": "Hi"}]
    assert captured["tools"] == [{
        "type": "function",
        "name": "shell",
        "parameters": {"type": "object"},
    }]
    assert captured["reasoning"] == {"effort": "high", "summary": "auto"}
    assert captured["timeout"] == 30
    assert captured["api_base"] == "https://api.openai.com/v1"
    assert captured["api_key"] == "sk-test"
    assert captured["stream"] is False
    assert "stream_options" not in captured


def test_responses_completion_maps_max_tokens_and_drops_chat_thinking(monkeypatch):
    captured = {}

    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **kwargs: captured.update(kwargs) or SimpleNamespace(output=[], usage=None),
    )

    responses_completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        llm_config=_config(max_tokens=512),
        max_tokens=256,
        extra_body={"thinking": {"type": "disabled"}},
    )

    assert captured["max_output_tokens"] == 256
    assert "max_tokens" not in captured
    assert "extra_body" not in captured


def test_responses_completion_keeps_unrelated_extra_body(monkeypatch):
    captured = {}

    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **kwargs: captured.update(kwargs) or SimpleNamespace(output=[], usage=None),
    )

    responses_completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        extra_body={"custom": "value"},
    )

    assert captured["extra_body"] == {"custom": "value"}


def test_streaming_responses_completion_returns_a_chunk_generator(monkeypatch):
    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **_kwargs: _event_stream(
            SimpleNamespace(type="response.output_text.delta", delta="hi"),
        ),
    )

    stream = responses_completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        stream=True,
        llm_config=_config(),
    )

    chunks = list(stream)
    assert chunks[0].choices[0].delta.content == "hi"


def test_completion_dispatches_responses_providers(monkeypatch):
    captured = {}

    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **kwargs: captured.update(kwargs) or SimpleNamespace(output=[], usage=None),
    )
    monkeypatch.setattr("app.models.litellm.completion", lambda **_kwargs: pytest.fail(
        "chat completions must not be used for Responses providers"
    ))

    completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        llm_config=_config(),
    )

    assert captured["model"] == "openai/gpt-5"
    assert captured["input"] == [{"role": "user", "content": "Hi"}]


def test_stream_usage_options_are_disabled_for_responses_providers():
    assert not supports_stream_usage_options(_config())
    assert supports_stream_usage_options(LlmConfig(provider="openai"))


def test_streaming_toggle_downgrades_to_single_shot_for_responses(monkeypatch):
    captured = {}

    def fake_responses(**kwargs):
        captured.update(kwargs)
        return SimpleNamespace(
            output=[SimpleNamespace(type="message", content=[
                SimpleNamespace(type="output_text", text="整段返回"),
            ])],
            usage=SimpleNamespace(input_tokens=5, output_tokens=2, input_tokens_details=None),
        )

    monkeypatch.setattr("app.models.litellm.responses", fake_responses)

    chunks = list(completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        stream=True,
        llm_config=_config(streaming=False),
    ))

    assert captured["stream"] is False
    assert chunks[0].choices[0].delta.content == "整段返回"
    assert chunks[1].usage.prompt_tokens == 5


def test_streaming_toggle_keeps_streaming_when_enabled(monkeypatch):
    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **_kwargs: _event_stream(
            SimpleNamespace(type="response.output_text.delta", delta="hi"),
        ),
    )

    stream = completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        stream=True,
        llm_config=_config(streaming=True),
    )

    assert list(stream)[0].choices[0].delta.content == "hi"


def test_streaming_registers_unknown_models_as_native_streaming(monkeypatch):
    """litellm must not fake-stream models missing from its cost map.

    A streaming Responses request for an unknown model name registers it as
    native-streaming so litellm keeps ``stream=True`` on the wire instead of
    silently downgrading to a single-shot request.
    """
    import litellm
    from litellm.utils import supports_native_streaming
    from app.models import _registered_native_streaming_models

    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **_kwargs: _event_stream(
            SimpleNamespace(type="response.output_text.delta", delta="hi"),
        ),
    )
    model_name = "agnes-native-streaming-test"

    stream = responses_completion(
        f"openai/{model_name}",
        [{"role": "user", "content": "Hi"}],
        stream=True,
        llm_config=_config(model=model_name, litellm_model=f"openai/{model_name}"),
    )

    assert list(stream)[0].choices[0].delta.content == "hi"
    assert supports_native_streaming(model_name, "openai")
    assert supports_native_streaming(f"openai/{model_name}", None)

    litellm.model_cost.pop(model_name, None)
    _registered_native_streaming_models.discard(model_name)
    _registered_native_streaming_models.discard(f"openai/{model_name}")


def test_graph_consumes_responses_stream_end_to_end(monkeypatch):
    """The synthesized chunks must drive the real graph loop, tools included."""
    from app import graph as graph_module

    calls = []

    def fake_responses(**kwargs):
        calls.append(kwargs)
        if len(calls) == 1:
            return _event_stream(
                SimpleNamespace(type="response.reasoning_summary_text.delta", delta="Listing files."),
                SimpleNamespace(type="response.output_text.delta", delta="Checking now. "),
                SimpleNamespace(
                    type="response.output_item.added",
                    output_index=1,
                    item=SimpleNamespace(
                        type="function_call", call_id="call-1", name="list_files",
                    ),
                ),
                SimpleNamespace(
                    type="response.function_call_arguments.delta",
                    output_index=1,
                    delta='{"path":".',
                ),
                SimpleNamespace(
                    type="response.function_call_arguments.delta",
                    output_index=1,
                    delta='"}',
                ),
                SimpleNamespace(type="response.completed", response=SimpleNamespace(
                    usage=SimpleNamespace(
                        input_tokens=100,
                        output_tokens=10,
                        input_tokens_details=SimpleNamespace(cached_tokens=40),
                    ),
                )),
            )
        return _event_stream(
            SimpleNamespace(type="response.output_text.delta", delta="README.md exists."),
            SimpleNamespace(type="response.completed", response=SimpleNamespace(
                usage=SimpleNamespace(
                    input_tokens=150,
                    output_tokens=20,
                    input_tokens_details=SimpleNamespace(cached_tokens=60),
                ),
            )),
        )

    monkeypatch.setattr("app.models.litellm.responses", fake_responses)
    emitted = []

    async def emit(value):
        emitted.append(value)

    async def execute_tool(_tool_call_id, _tool_name, _arguments):
        return "README.md"

    state = {
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "system_prompt": "You are Agnes.",
        "model": "openai/gpt-5",
        "tool_policy": {"shell": {"enabled": True}},
        "dynamic_tools": [{
            "type": "function",
            "function": {"name": "list_files", "parameters": {"type": "object"}},
        }],
        "pending_tool_calls": [],
        "finished": False,
        "llm_config": {
            "provider": RESPONSES_API_PROVIDER,
            "model": "gpt-5",
            "litellmModel": "openai/gpt-5",
            "apiBase": "https://api.openai.com/v1",
            "apiKey": "sk-test",
            "thinking": {"mode": "high", "budget": 0},
            "maxTokens": 2048,
        },
        "fallback_configs": [],
        "active_llm_index": 0,
        "active_llm_config": None,
        "active_model": "openai/gpt-5",
        "fallback_locked": False,
        "token_usage": {},
    }

    def run():
        graph = graph_module.build_graph()
        return asyncio.run(
            graph.ainvoke(state, config={"configurable": {
                "send_delta_fn": emit,
                "execute_tool_fn": execute_tool,
            }})
        )

    result = run()

    # Reasoning summaries stream as thought markers, text as answer deltas.
    assert emitted[:3] == ["<thought>", "Listing files.", "</thought>"]
    assert "Checking now. " in emitted
    assert "README.md exists." in result["messages"][-1]["content"]
    assert calls[0]["instructions"] == "You are Agnes."
    assert calls[0]["input"][0]["role"] == "user"
    assert calls[1]["input"][-1] == {
        "type": "function_call_output",
        "call_id": "call-1",
        "output": "README.md",
    }
    # Usage accumulates across turns; context_tokens keeps the latest turn size.
    assert result["token_usage"] == {
        "input_tokens": 250,
        "cached_tokens": 100,
        "output_tokens": 30,
        "context_tokens": 170,
    }


def test_graph_replays_reasoning_items_across_the_tool_loop(monkeypatch):
    """The graph must capture reasoning items and replay them on the next call."""
    from app import graph as graph_module

    calls = []

    def fake_responses(**kwargs):
        calls.append(kwargs)
        if len(calls) == 1:
            return _event_stream(
                SimpleNamespace(
                    type="response.output_item.added",
                    output_index=0,
                    item=SimpleNamespace(type="reasoning", id="rs-1", encrypted_content="enc-1"),
                ),
                SimpleNamespace(type="response.reasoning_summary_text.delta", delta="Listing files."),
                SimpleNamespace(type="response.output_text.delta", delta="Checking now. "),
                SimpleNamespace(
                    type="response.output_item.added",
                    output_index=2,
                    item=SimpleNamespace(type="function_call", call_id="call-1", name="list_files"),
                ),
                SimpleNamespace(
                    type="response.function_call_arguments.delta",
                    output_index=2,
                    delta='{"path":"."}',
                ),
                SimpleNamespace(type="response.completed", response=SimpleNamespace(usage=None)),
            )
        return _event_stream(
            SimpleNamespace(type="response.output_text.delta", delta="README.md exists."),
            SimpleNamespace(type="response.completed", response=SimpleNamespace(usage=None)),
        )

    monkeypatch.setattr("app.models.litellm.responses", fake_responses)
    emitted = []

    async def emit(value):
        emitted.append(value)

    async def execute_tool(_tool_call_id, _tool_name, _arguments):
        return "README.md"

    state = {
        "messages": [{"role": "user", "content": "Inspect the repository"}],
        "system_prompt": "You are Agnes.",
        "model": "openai/gpt-5",
        "tool_policy": {"shell": {"enabled": True}},
        "dynamic_tools": [{
            "type": "function",
            "function": {"name": "list_files", "parameters": {"type": "object"}},
        }],
        "pending_tool_calls": [],
        "finished": False,
        "llm_config": {
            "provider": RESPONSES_API_PROVIDER,
            "model": "gpt-5",
            "litellmModel": "openai/gpt-5",
            "apiBase": "https://api.openai.com/v1",
            "apiKey": "sk-test",
            "thinking": {"mode": "high", "budget": 0},
            "maxTokens": 2048,
        },
        "fallback_configs": [],
        "active_llm_index": 0,
        "active_llm_config": None,
        "active_model": "openai/gpt-5",
        "fallback_locked": False,
        "token_usage": {},
    }

    result = asyncio.run(
        graph_module.build_graph().ainvoke(state, config={"configurable": {
            "send_delta_fn": emit,
            "execute_tool_fn": execute_tool,
        }})
    )

    assert result["messages"][-1]["content"] == "README.md exists."
    # The reasoning item captured on turn 1 must be replayed on turn 2, before
    # the function_call_output it produced.
    second_input = calls[1]["input"]
    replayed = [item for item in second_input if item.get("type") == "reasoning"]
    assert replayed == [{"type": "reasoning", "id": "rs-1", "encrypted_content": "enc-1"}]
    assert second_input.index(replayed[0]) < second_input.index({
        "type": "function_call_output",
        "call_id": "call-1",
        "output": "README.md",
    })


def test_reasoning_items_are_captured_from_stream_events():
    events = [
        SimpleNamespace(
            type="response.output_item.added",
            output_index=0,
            item=SimpleNamespace(type="reasoning", id="rs-1", encrypted_content="enc-1"),
        ),
        SimpleNamespace(type="response.reasoning_summary_text.delta", delta="Thinking"),
        SimpleNamespace(type="response.output_text.delta", delta="Answer"),
        SimpleNamespace(
            type="response.output_item.done",
            output_index=0,
            item=SimpleNamespace(type="reasoning", id="rs-1", encrypted_content="enc-1"),
        ),
    ]

    chunks = list(_iter_responses_events_as_chat_chunks(_event_stream(*events), {}))

    assert [c.choices[0].delta.content for c in chunks if c.choices[0].delta.content] == ["Answer"]


def test_responses_stream_exposes_captured_reasoning_items():
    from app.models import ResponsesStream

    events = [
        SimpleNamespace(
            type="response.output_item.added",
            output_index=0,
            item=SimpleNamespace(type="reasoning", id="rs-1", encrypted_content="enc-1"),
        ),
        SimpleNamespace(type="response.output_text.delta", delta="Answer"),
    ]
    stream = ResponsesStream(lambda: _event_stream(*events))

    chunks = list(stream)

    assert chunks[0].choices[0].delta.content == "Answer"
    assert stream.reasoning_items == [{
        "type": "reasoning",
        "id": "rs-1",
        "encrypted_content": "enc-1",
    }]


def test_assistant_reasoning_items_are_replayed_before_the_turn():
    messages = [
        {"role": "user", "content": "Run the check"},
        {
            "role": "assistant",
            "content": "Running.",
            "provider_specific_fields": {
                "responses_reasoning": [
                    {"type": "reasoning", "id": "rs-1", "encrypted_content": "enc-1"},
                ],
            },
            "tool_calls": [{
                "id": "call-1",
                "type": "function",
                "function": {"name": "shell", "arguments": '{"cmd":"ls"}'},
            }],
        },
        {"role": "tool", "tool_call_id": "call-1", "content": "ok"},
    ]

    _, items = _chat_messages_to_responses_input(messages)

    assert items[0] == {"role": "user", "content": "Run the check"}
    assert items[1] == {"type": "reasoning", "id": "rs-1", "encrypted_content": "enc-1"}
    assert items[2] == {"role": "assistant", "content": "Running."}
    assert items[3]["type"] == "function_call"
    assert items[4] == {
        "type": "function_call_output",
        "call_id": "call-1",
        "output": "ok",
    }


def test_responses_completion_requests_reasoning_replay_flags(monkeypatch):
    captured = {}

    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **kwargs: captured.update(kwargs) or SimpleNamespace(output=[], usage=None),
    )

    responses_completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        llm_config=_config(),
    )

    assert captured["include"] == ["reasoning.encrypted_content"]
    assert captured["store"] is False
    assert captured["input"] == [{"role": "user", "content": "Hi"}]


def test_non_streaming_response_exposes_reasoning_items(monkeypatch):
    monkeypatch.setattr(
        "app.models.litellm.responses",
        lambda **_kwargs: SimpleNamespace(
            output=[
                SimpleNamespace(
                    type="reasoning",
                    id="rs-1",
                    encrypted_content="enc-1",
                    summary=[SimpleNamespace(type="summary_text", text="Thinking")],
                ),
                SimpleNamespace(type="message", content=[
                    SimpleNamespace(type="output_text", text="Answer"),
                ]),
            ],
            usage=None,
        ),
    )

    response = responses_completion(
        "openai/gpt-5",
        [{"role": "user", "content": "Hi"}],
        llm_config=_config(),
    )

    assert response.choices[0].message.content == "Answer"
    assert response.choices[0].message.reasoning_content == "Thinking"
    assert response.reasoning_items == [{
        "type": "reasoning",
        "id": "rs-1",
        "encrypted_content": "enc-1",
    }]


def test_streaming_reasoning_replay_retries_without_reasoning_on_invalid_request(monkeypatch):
    calls = []

    def fake_responses(**kwargs):
        calls.append(kwargs)
        if kwargs.get("include"):
            raise ValueError("400 invalid_encrypted_content: could not be verified")
        return _event_stream(
            SimpleNamespace(type="response.output_text.delta", delta="fallback"),
        )

    monkeypatch.setattr("app.models.litellm.responses", fake_responses)

    stream = responses_completion(
        "openai/gpt-5",
        [
            {"role": "user", "content": "Hi"},
            {
                "role": "assistant",
                "content": "",
                "provider_specific_fields": {
                    "responses_reasoning": [
                        {"type": "reasoning", "id": "rs-1", "encrypted_content": "enc-1"},
                    ],
                },
            },
        ],
        stream=True,
        llm_config=_config(),
    )

    chunks = list(stream)

    assert chunks[0].choices[0].delta.content == "fallback"
    assert len(calls) == 2
    assert calls[0]["include"] == ["reasoning.encrypted_content"]
    assert calls[1].get("include") is None
    # The retry stripped the replayed reasoning item from the input.
    assert all(
        item.get("type") != "reasoning"
        for item in calls[1]["input"]
        if isinstance(item, dict)
    )
