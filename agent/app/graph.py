"""LangGraph Agent State Machine."""
from __future__ import annotations
import json
from typing import Any, Dict, List, Optional, TypedDict
from langgraph.graph import StateGraph, END
from langchain_core.runnables import RunnableConfig

from .models import LlmConfig, completion

class AgentState(TypedDict):
    messages: List[Dict[str, Any]]
    system_prompt: str
    model: str
    tool_policy: Dict[str, Any]
    pending_tool_calls: List[Dict[str, Any]]
    finished: bool
    llm_config: Optional[Dict[str, Any]]  # Raw dict from ContextSnapshot, parsed in nodes

def get_available_tools(tool_policy: Dict[str, Any]) -> List[Dict[str, Any]]:
    """Determine tools to expose to the LLM based on permissions policy."""
    tools = []
    
    # 1. Shell Tool
    shell_enabled = tool_policy.get("shell", {}).get("enabled", True)
    if shell_enabled:
        tools.append({
            "type": "function",
            "function": {
                "name": "shell",
                "description": (
                    "Execute a command in the bash shell. Only run allowed commands in allowed working directories. "
                    "Make sure to explain your reasoning before running commands."
                ),
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "The command to run."},
                        "cwd": {"type": "string", "description": "Optional working directory. Defaults to current workspace."}
                    },
                    "required": ["command"]
                }
            }
        })
        
    # 2. File Tools
    file_enabled = tool_policy.get("file", {}).get("enabled", True)
    if file_enabled:
        tools.append({
            "type": "function",
            "function": {
                "name": "file_read",
                "description": "Read the text content of a file from the local filesystem.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute or relative path to the file."}
                    },
                    "required": ["path"]
                }
            }
        })
        tools.append({
            "type": "function",
            "function": {
                "name": "file_write",
                "description": "Write text content to a file on the local filesystem. Overwrites the file if it exists.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute or relative path to the target file."},
                        "content": {"type": "string", "description": "Text content to write."}
                    },
                    "required": ["path", "content"]
                }
            }
        })
        tools.append({
            "type": "function",
            "function": {
                "name": "file_edit",
                "description": "Replace an exact string in a UTF-8 file. The old string must be unique unless replace_all is true.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Absolute path or a path relative to the workspace."},
                        "old_string": {"type": "string", "description": "Exact text to replace."},
                        "new_string": {"type": "string", "description": "Replacement text."},
                        "replace_all": {"type": "boolean", "description": "Replace all matches instead of requiring one unique match."}
                    },
                    "required": ["path", "old_string", "new_string"]
                }
            }
        })
        tools.append({
            "type": "function",
            "function": {
                "name": "list_files",
                "description": "List files and directories below a workspace path using a glob pattern.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Directory to list; defaults to the workspace."},
                        "pattern": {"type": "string", "description": "Glob relative to path, such as **/*.py."},
                        "max_results": {"type": "integer", "minimum": 1, "maximum": 5000}
                    }
                }
            }
        })
        tools.append({
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Search UTF-8 files recursively with a regular expression.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Rust-compatible regular expression."},
                        "path": {"type": "string", "description": "File or directory to search; defaults to the workspace."},
                        "glob": {"type": "string", "description": "Optional file glob relative to path."},
                        "case_sensitive": {"type": "boolean", "description": "Defaults to true."},
                        "max_results": {"type": "integer", "minimum": 1, "maximum": 1000}
                    },
                    "required": ["pattern"]
                }
            }
        })
        tools.append({
            "type": "function",
            "function": {
                "name": "apply_patch",
                "description": "Apply a Codex-style multi-file patch within the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "patch": {"type": "string", "description": "Patch text from *** Begin Patch through *** End Patch."},
                        "cwd": {"type": "string", "description": "Optional base directory; defaults to the workspace."}
                    },
                    "required": ["patch"]
                }
            }
        })

    # 3. Git Tool
    git_enabled = tool_policy.get("git", {}).get("enabled", True)
    if git_enabled:
        tools.append({
            "type": "function",
            "function": {
                "name": "git",
                "description": "Execute a git operation inside the workspace.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "args": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Arguments to pass to git (e.g. ['status', '--short'])."
                        },
                        "cwd": {"type": "string", "description": "Working directory. Defaults to workspace."}
                    },
                    "required": ["args"]
                }
            }
        })

    # 4. Agent-scoped memory tools
    memory_enabled = tool_policy.get("memory", {}).get("enabled", True)
    if memory_enabled:
        tools.extend([
            {
                "type": "function",
                "function": {
                    "name": "memory_search",
                    "description": "Search this agent's structured long-term memories by name, keywords, and content.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"},
                            "limit": {"type": "integer", "minimum": 1, "maximum": 50},
                        },
                        "required": ["query"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "memory_create",
                    "description": "Create one structured long-term memory for this agent. The system assigns its ID, agent, creator, and timestamps.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "keywords": {
                                "type": "array",
                                "items": {"type": "string"},
                            },
                            "content": {"type": "string"},
                        },
                        "required": ["name", "content"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "memory_update",
                    "description": "Update selected fields of one structured long-term memory belonging to this agent. The creator and creation time are preserved.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "memory_id": {"type": "string"},
                            "name": {"type": "string"},
                            "keywords": {
                                "type": "array",
                                "items": {"type": "string"},
                            },
                            "content": {"type": "string"},
                        },
                        "required": ["memory_id"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "memory_md_view",
                    "description": "Read the complete MEMORY.md for this agent. Use after editing to verify final content.",
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "memory_md_edit",
                    "description": "Append to MEMORY.md or replace one uniquely matching exact block. This cannot modify USER.md or arbitrary files.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "action": {"type": "string", "enum": ["append", "replace"]},
                            "content": {"type": "string"},
                            "old_text": {"type": "string"},
                            "new_text": {"type": "string"},
                        },
                        "required": ["action"],
                        "additionalProperties": False,
                    },
                },
            },
        ])

    # 5. User-level calendar and task tools
    planner_enabled = tool_policy.get("planner", {}).get("enabled", True)
    if planner_enabled:
        tools.extend([
            {
                "type": "function",
                "function": {
                    "name": "calendar_list",
                    "description": "List local calendars first to obtain calendar IDs. Then call again with calendar_id plus range_start and range_end to read event occurrences; do not conclude that a calendar has no events from the first call alone. Range values must be RFC 3339 / ISO 8601 instants including Z or a numeric UTC offset, for example 2026-07-18T00:00:00+08:00. In each recurring result, id is the event_id used for updates; occurrence_id and original_occurrence identify that occurrence.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "calendar_id": {"type": "string", "description": "Calendar ID returned by calendar_list."},
                            "range_start": {"type": "string", "description": "Required with calendar_id. RFC 3339 / ISO 8601 range start with Z or an explicit offset, for example 2026-07-18T00:00:00+08:00; never omit the timezone."},
                            "range_end": {"type": "string", "description": "Required with calendar_id. RFC 3339 / ISO 8601 exclusive range end with Z or an explicit offset, for example 2026-07-19T00:00:00+08:00; never omit the timezone."},
                        },
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "calendar_create",
                    "description": "Create a local calendar. This write always requires approval outside Full Access.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "Human-readable calendar name."},
                            "timezone": {"type": "string", "description": "IANA timezone, for example Asia/Shanghai."},
                            "color": {"type": ["string", "null"], "description": "Optional CSS color such as #4f8a6f."},
                        },
                        "required": ["name", "timezone"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "calendar_event_create",
                    "description": "Create an event in an existing local calendar. starts_at and ends_at must be RFC 3339 / ISO 8601 instants with Z or an explicit UTC offset, never a timezone-less datetime. For an all-day event, use local midnight for starts_at and the next local midnight for ends_at in the supplied timezone. This write always requires approval outside Full Access.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "calendar_id": {"type": "string", "description": "Calendar ID returned by calendar_list."},
                            "title": {"type": "string", "description": "Event title."},
                            "starts_at": {"type": "string", "description": "RFC 3339 / ISO 8601 start instant with timezone: use Z or an explicit UTC offset, for example 2026-07-18T09:00:00+08:00."},
                            "ends_at": {"type": "string", "description": "RFC 3339 / ISO 8601 end instant after starts_at with timezone: use Z or an explicit UTC offset, for example 2026-07-18T10:00:00+08:00."},
                            "timezone": {"type": "string", "description": "IANA timezone used for wall-clock and recurrence semantics, for example Asia/Shanghai."},
                            "all_day": {"type": "boolean", "description": "True for an all-day range whose instants are local midnights."},
                            "recurrence_rule": {"type": ["string", "null"], "description": "Optional RFC 5545 rule prefixed with RRULE:, for example RRULE:FREQ=WEEKLY."},
                        },
                        "required": ["calendar_id", "title", "starts_at", "ends_at", "timezone"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "calendar_update",
                    "description": "Update a recurring series or one occurrence. starts_at and ends_at, when supplied, must be RFC 3339 / ISO 8601 instants with Z or an explicit UTC offset. Omit original_occurrence to update the series; include it to update one occurrence. With original_occurrence, cancelled=true cancels that occurrence and cancelled=false restores the original occurrence. Do not combine cancelled with edit fields. This write always requires approval outside Full Access.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "event_id": {"type": "string", "description": "Stable event ID returned by calendar_list."},
                            "title": {"type": "string", "description": "Replacement title."},
                            "starts_at": {"type": "string", "description": "RFC 3339 / ISO 8601 start instant with timezone: use Z or an explicit UTC offset, for example 2026-07-18T09:00:00+08:00."},
                            "ends_at": {"type": "string", "description": "RFC 3339 / ISO 8601 end instant after starts_at with timezone: use Z or an explicit UTC offset."},
                            "timezone": {"type": "string", "description": "IANA timezone, for example Asia/Shanghai."},
                            "all_day": {"type": "boolean", "description": "Whether the resulting event is all-day."},
                            "recurrence_rule": {"type": ["string", "null"], "description": "RFC 5545 rule prefixed with RRULE:, or null to clear series recurrence."},
                            "original_occurrence": {"type": "string", "description": "Exact ISO 8601 instant returned by calendar_list. When present, edit only that occurrence."},
                            "cancelled": {"type": "boolean", "description": "With original_occurrence only: true cancels and false restores the occurrence."},
                        },
                        "required": ["event_id"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "task_list",
                    "description": "List local task lists, or tasks in one list when task_list_id is provided.",
                    "parameters": {
                        "type": "object",
                        "properties": {"task_list_id": {"type": "string"}},
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "task_create",
                    "description": "Create a local task in an existing task list. This write always requires approval outside Full Access.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "task_list_id": {"type": "string"},
                            "title": {"type": "string"},
                            "description": {"type": ["string", "null"]},
                            "due_date": {"type": ["string", "null"]},
                            "due_at": {"type": ["string", "null"]},
                            "due_timezone": {"type": ["string", "null"]},
                            "is_important": {"type": "boolean"},
                            "my_day_date": {"type": ["string", "null"]},
                            "recurrence_rule": {"type": ["string", "null"]},
                            "priority": {"type": "integer", "minimum": 0, "maximum": 4},
                        },
                        "required": ["task_list_id", "title"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "task_update",
                    "description": "Update selected fields of an existing local task. This write always requires approval outside Full Access.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type": "string"},
                            "title": {"type": "string"},
                            "description": {"type": ["string", "null"]},
                            "priority": {"type": "integer", "minimum": 0, "maximum": 4},
                            "due_date": {"type": ["string", "null"]},
                            "due_at": {"type": ["string", "null"]},
                            "due_timezone": {"type": ["string", "null"]},
                            "is_important": {"type": "boolean"},
                            "my_day_date": {"type": ["string", "null"]},
                            "recurrence_rule": {"type": ["string", "null"]},
                        },
                        "required": ["task_id"],
                        "additionalProperties": False,
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "task_complete",
                    "description": "Mark a local task completed or reopen it. This write always requires approval outside Full Access.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "task_id": {"type": "string"},
                            "completed": {"type": "boolean"},
                        },
                        "required": ["task_id", "completed"],
                        "additionalProperties": False,
                    },
                },
            },
        ])

    return tools

async def call_llm_node(state: AgentState, config: RunnableConfig) -> Dict[str, Any]:
    """Node that handles LiteLLM completion and output streaming."""
    # Retrieve communication callbacks from LangGraph config
    configurable = config.get("configurable", {})
    send_delta_fn = configurable.get("send_delta_fn")
    
    # Consolidate messages for LLM call
    messages = []
    if state.get("system_prompt"):
        messages.append({
            "role": "system",
            "content": state["system_prompt"]
        })
    messages.extend(state["messages"])
    
    # Check what tools are permitted
    available_tools = get_available_tools(state.get("tool_policy", {}))
    tools_arg = available_tools if available_tools else None
    
    # Build LlmConfig from state
    raw_llm_config = state.get("llm_config")
    llm_config = LlmConfig.from_dict(raw_llm_config) if raw_llm_config else None

    # Invoke LiteLLM
    response = completion(
        model=state["model"],
        messages=messages,
        tools=tools_arg,
        stream=True,
        llm_config=llm_config,
    )
    
    full_text = ""
    full_reasoning = ""
    tool_calls_accumulator = {}
    reasoning_started = False
    reasoning_closed = False

    async def emit(content: str) -> None:
        if send_delta_fn:
            await send_delta_fn(content)

    # Stream tokens and accumulate tool calls
    for chunk in response:
        if not chunk.choices:
            continue
        delta = chunk.choices[0].delta

        # 思维链内容（DeepSeek reasoning_content / OpenAI o-series reasoning）
        # 用 <thought>...</thought> 包裹，Rust 与前端据此分流到 thought 片段
        reasoning_text = getattr(delta, "reasoning_content", None) or getattr(delta, "reasoning", None)
        if reasoning_text:
            if not reasoning_started:
                reasoning_started = True
                await emit("<thought>")
            full_reasoning += reasoning_text
            await emit(reasoning_text)

        # 从思维链切换到正文：闭合 <thought> 标签
        if delta.content and reasoning_started and not reasoning_closed:
            reasoning_closed = True
            await emit("</thought>")

        # Stream text delta if callback is present
        if delta.content:
            full_text += delta.content
            await emit(delta.content)

        # Accumulate streaming tool call args
        if delta.tool_calls:
            for tc in delta.tool_calls:
                index = tc.index
                if index not in tool_calls_accumulator:
                    tool_calls_accumulator[index] = {
                        "id": tc.id or "",
                        "type": "function",
                        "function": {
                            "name": tc.function.name or "",
                            "arguments": tc.function.arguments or ""
                        }
                    }
                else:
                    if tc.id:
                        tool_calls_accumulator[index]["id"] = tc.id
                    if tc.function.name:
                        tool_calls_accumulator[index]["function"]["name"] += tc.function.name
                    if tc.function.arguments:
                        tool_calls_accumulator[index]["function"]["arguments"] += tc.function.arguments

    # 流结束时若仍处于思维链中（无正文跟进），闭合标签
    if reasoning_started and not reasoning_closed:
        reasoning_closed = True
        await emit("</thought>")
                        
    # Format accumulated tool calls list
    pending_tool_calls = list(tool_calls_accumulator.values())
    
    # Clean up empty IDs in tool calls (sometimes LLM streams id only in first chunk)
    for tc in pending_tool_calls:
        if not tc["id"]:
            import uuid
            tc["id"] = f"tc_{uuid.uuid4().hex[:8]}"
            
    # Append assistant's response message to history
    assistant_msg: Dict[str, Any] = {
        "role": "assistant",
        "content": full_text
    }
    if pending_tool_calls:
        assistant_msg["tool_calls"] = pending_tool_calls
        
    new_messages = list(state["messages"])
    new_messages.append(assistant_msg)
    
    return {
        "messages": new_messages,
        "pending_tool_calls": pending_tool_calls,
        "finished": len(pending_tool_calls) == 0
    }

async def execute_tools_node(state: AgentState, config: RunnableConfig) -> Dict[str, Any]:
    """Node that handles routing tool calls to Rust and waiting for results."""
    configurable = config.get("configurable", {})
    execute_tool_fn = configurable.get("execute_tool_fn")
    
    new_messages = list(state["messages"])
    
    if not execute_tool_fn:
        # Fallback if no executor callback (e.g. testing)
        for tc in state["pending_tool_calls"]:
            new_messages.append({
                "role": "tool",
                "tool_call_id": tc["id"],
                "name": tc["function"]["name"],
                "content": json.dumps({"error": "No execution handler available"})
            })
        return {
            "messages": new_messages,
            "pending_tool_calls": []
        }
        
    for tc in state["pending_tool_calls"]:
        tool_name = tc["function"]["name"]
        args_str = tc["function"]["arguments"]
        
        # Parse arguments safely
        try:
            arguments = json.loads(args_str) if args_str else {}
        except Exception as e:
            arguments = {"raw_args": args_str, "parse_error": str(e)}
            
        # Call Rust (async wait for stdout/result)
        result_content = await execute_tool_fn(tc["id"], tool_name, arguments)
        
        new_messages.append({
            "role": "tool",
            "tool_call_id": tc["id"],
            "name": tool_name,
            "content": result_content
        })
        
    return {
        "messages": new_messages,
        "pending_tool_calls": []
    }

def build_graph() -> StateGraph:
    """Compile the Agent reasoning graph workflow."""
    workflow = StateGraph(AgentState)
    
    # Register Nodes
    workflow.add_node("call_llm", call_llm_node)
    workflow.add_node("execute_tools", execute_tools_node)
    
    # Establish Entry
    workflow.set_entry_point("call_llm")
    
    # Define conditional branching
    def decide_next_step(state: AgentState) -> str:
        if state.get("finished", False):
            return "end"
        return "execute_tools"
        
    workflow.add_conditional_edges(
        "call_llm",
        decide_next_step,
        {
            "end": END,
            "execute_tools": "execute_tools"
        }
    )
    
    # Loop back to LLM after tools
    workflow.add_edge("execute_tools", "call_llm")
    
    return workflow.compile()
