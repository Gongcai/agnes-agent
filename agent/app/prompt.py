"""Prompt compiler and context budget manager."""
from __future__ import annotations
import json
from typing import Any, Dict, List, Tuple, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from .models import LlmConfig
import tiktoken

from .models import get_max_context_tokens, completion

def count_tokens(text: str, model: str = "gpt-4") -> int:
    """Accurately count tokens in a string using tiktoken."""
    try:
        encoding = tiktoken.encoding_for_model(model)
    except Exception:
        try:
            encoding = tiktoken.get_encoding("cl100k_base")
        except Exception:
            # Fallback approximation: 1 token ~= 4 characters for English
            return len(text) // 4
    return len(encoding.encode(text))

def translate_messages(recent_messages: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Translate multi-part messages from the database schema to standard OpenAI format."""
    translated: List[Dict[str, Any]] = []
    
    for msg in recent_messages:
        role = msg.get("role")
        parts = msg.get("parts", [])
        
        content_parts: List[str] = []
        tool_calls: List[Dict[str, Any]] = []
        has_tool_result = False
        
        for part in parts:
            kind = part.get("kind") or part.get("type")
            content = part.get("content", "")
            
            if kind == "text":
                content_parts.append(content)
            elif kind == "thought":
                # Encapsulate assistant's internal thinking process
                content_parts.append(f"<thought>\n{content}\n</thought>")
            elif kind == "tool_call":
                tc_info = part.get("toolCall")
                if tc_info:
                    tool_calls.append({
                        "id": tc_info.get("id"),
                        "type": "function",
                        "function": {
                            "name": tc_info.get("tool"),
                            "arguments": tc_info.get("args") or "{}",
                        }
                    })
                if content:
                    content_parts.append(content)
            elif kind == "tool_result":
                has_tool_result = True
                tc_info = part.get("toolCall")
                tc_id = tc_info.get("id") if tc_info else ""
                tool_name = tc_info.get("tool") if tc_info else ""
                
                translated.append({
                    "role": "tool",
                    "tool_call_id": tc_id,
                    "name": tool_name,
                    "content": content,
                })
        
        if role in ("user", "assistant"):
            # Only append the assistant message if it contains actual content/tool_calls,
            # or if it does not contain a tool_result. (Avoids empty assistant placeholder messages).
            if content_parts or tool_calls or not has_tool_result:
                message_dict: Dict[str, Any] = {
                    "role": role,
                    "content": "\n".join(content_parts)
                }
                if tool_calls:
                    message_dict["tool_calls"] = tool_calls
                translated.append(message_dict)
            
    return translated

def summarize_history(
    messages_to_compress: List[Dict[str, Any]],
    old_summary: Optional[str],
    model: str,
    llm_config: Optional["LlmConfig"] = None,
) -> str:
    """Run LiteLLM to compress message history into a rolling summary."""
    if not messages_to_compress:
        return old_summary or ""
        
    history_text = ""
    for msg in messages_to_compress:
        role = msg.get("role")
        content = msg.get("content", "")
        # Ignore thought tags in summarizer context
        if "<thought>" in content:
            import re
            content = re.sub(r"<thought>.*?</thought>", "", content, flags=re.DOTALL).strip()
        history_text += f"{role}: {content}\n"
        
    prompt = (
        f"You are a rolling summarizer. Your task is to update the summary of the conversation.\n\n"
        f"Previous Summary:\n{old_summary or 'None'}\n\n"
        f"New segment of conversation to summarize:\n{history_text}\n\n"
        f"Consolidate both into a single cohesive, highly concise summary of the key facts discussed. "
        f"Respond ONLY with the new updated summary text."
    )
    
    try:
        response = completion(
            model=model,
            messages=[{"role": "user", "content": prompt}],
            llm_config=llm_config,
            temperature=0.3,
        )
        return response.choices[0].message.content.strip()
    except Exception as e:
        print(f"[sidecar][summary] Failed to summarize history: {e}", flush=True)
        return old_summary or ""

def assemble_prompt(
    snapshot: Dict[str, Any],
    reserved_tokens: int = 4000
) -> Tuple[str, List[Dict[str, Any]], List[Dict[str, Any]]]:
    """Assemble system prompt and filter message history under context window budget.
    
    Returns:
        A tuple of (system_prompt, messages_list, discarded_messages)
    """
    context = snapshot.get("context", {})
    agent = context.get("agent", {})
    settings = context.get("settings", {})
    
    # 1. Base System Prompt
    system_parts = []
    
    persona = agent.get("persona")
    sys_prompt = agent.get("systemPrompt")
    if sys_prompt:
        system_parts.append(f"# System Instructions\n{sys_prompt}")
    if persona:
        system_parts.append(f"# Character Persona\n{persona}")
        
    # 2. Tool policies description
    tool_policy = agent.get("toolPolicy")
    if tool_policy:
        system_parts.append(
            f"# Tool Policies & Permissions\n"
            f"You have permissions configured as follows:\n{json.dumps(tool_policy, indent=2)}\n"
            f"Always explain your rationale briefly before calling tools."
        )
        
    # 3. Explicit Memory files (USER.md / MEMORY.md)
    explicit_mem = context.get("explicitMemories", {})
    user_md = explicit_mem.get("user_md")
    memory_md = explicit_mem.get("memory_md")
    
    if user_md:
        system_parts.append(f"# User Profile (USER.md - Read Only)\n{user_md}")
    if memory_md:
        system_parts.append(f"# Key Memories & Facts (MEMORY.md - Read/Write)\n{memory_md}")
        
    # 4. Project context & workspace files
    project_context = context.get("projectContext", [])
    if project_context:
        system_parts.append("# Current Workspace Context")
        for item in project_context:
            system_parts.append(f"File: {item.get('path')}\n```\n{item.get('content')}\n```")
            
    # 5. Retrieved memories (Vector Search)
    retrieved_memories = context.get("retrievedMemories", [])
    if retrieved_memories:
        system_parts.append("# Retrieved Information (Memory Store)")
        for item in retrieved_memories:
            system_parts.append(f"- {item}")

    system_prompt = "\n\n".join(system_parts)
    
    # 6. Calculate token budgets
    model_name = agent.get("model") or "gpt-4o"
    model_limit = get_max_context_tokens(model_name)
    user_limit = settings.get("user_context_limit")
    
    context_limit = min(model_limit, user_limit) if user_limit else model_limit
    usable_budget = context_limit - reserved_tokens - count_tokens(system_prompt, model_name)
    
    # 7. Add conversation summary if present
    messages: List[Dict[str, Any]] = []
    summary = context.get("summary")
    if summary:
        messages.append({
            "role": "system",
            "content": f"Below is a summary of the preceding conversation history:\n{summary}"
        })
        usable_budget -= count_tokens(messages[0]["content"], model_name)
        
    # 8. Translate and filter recent messages
    raw_recent = context.get("recentMessages", [])
    translated_recent = translate_messages(raw_recent)
    
    # Iterate backwards to add messages within budget
    filtered_recent: List[Dict[str, Any]] = []
    accumulated_tokens = 0
    
    for msg in reversed(translated_recent):
        msg_str = msg.get("content", "")
        # Add tool_calls structure to approximation if present
        if "tool_calls" in msg:
            msg_str += json.dumps(msg["tool_calls"])
            
        msg_tokens = count_tokens(msg_str, model_name)
        if accumulated_tokens + msg_tokens > usable_budget:
            break
        filtered_recent.append(msg)
        accumulated_tokens += msg_tokens
        
    filtered_recent.reverse()
    messages.extend(filtered_recent)
    
    # Append the user's latest input to the messages list
    user_input = snapshot.get("input")
    if user_input:
        messages.append({
            "role": "user",
            "content": user_input
        })
        
    # Compute discarded messages for rolling summary
    discarded_messages = translated_recent[:-len(filtered_recent)] if filtered_recent else translated_recent
    
    return system_prompt, messages, discarded_messages
