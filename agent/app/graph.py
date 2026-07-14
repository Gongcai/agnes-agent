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
    tool_calls_accumulator = {}
    
    # Stream tokens and accumulate tool calls
    for chunk in response:
        if not chunk.choices:
            continue
        delta = chunk.choices[0].delta
        
        # Stream text delta if callback is present
        if delta.content:
            full_text += delta.content
            if send_delta_fn:
                await send_delta_fn(delta.content)
                
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
