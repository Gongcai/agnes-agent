"""Memory extraction from conversation history."""
from __future__ import annotations
import json
from typing import Any, Dict, List, Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from .models import LlmConfig

from .models import completion

def extract_memories(
    messages: List[Dict[str, Any]],
    model: str = "gpt-4o",
    llm_config: Optional["LlmConfig"] = None,
) -> List[Dict[str, Any]]:
    """Extract new user facts, preferences, or project details from recent chat history.
    
    Returns:
        A list of dicts, each representing an extracted memory:
        {
            "content": "...",
            "type": "Preference" | "Fact" | "Context" | "Codebase",
            "confidence": 0.9,
            "source": "..."
        }
    """
    if len(messages) < 2:
        return []

    # System instruction for extraction
    system_prompt = (
        "You are an expert memory extraction agent. Your job is to analyze the conversation history "
        "and extract any permanent facts, user preferences, codebase settings, or workspace context "
        "that are worth remembering long-term. Only extract high-confidence, explicit statements. "
        "Do not extract temporary greeting, chat filler, or generic code snippets. "
        "Provide your output strictly as a JSON object with a single 'memories' key containing a list of objects. "
        "Each memory object must have:\n"
        "- 'content': The detailed factual statement to remember (e.g., 'User prefers standard CSS instead of TailwindCSS').\n"
        "- 'type': One of: 'Preference', 'Fact', 'Context', 'Codebase'.\n"
        "- 'confidence': A float between 0.0 and 1.0 representing your certainty.\n"
        "- 'source': A short snippet of the dialogue that proves this fact.\n\n"
        "If no permanent facts were introduced, return an empty list."
    )

    # Compile messages for extraction (system prompt + past history)
    extraction_messages = [
        {"role": "system", "content": system_prompt}
    ]
    
    # We only need the last few turns (e.g., 4 messages) to detect recent changes
    recent_history = messages[-6:] if len(messages) > 6 else messages
    
    # Clean messages for standard format (remove thoughts or non-text parts)
    for msg in recent_history:
        role = msg.get("role")
        content = msg.get("content", "")
        if role in ("user", "assistant") and content:
            # Strip thought tags from assistant messages to focus on actual content
            if role == "assistant" and "<thought>" in content:
                import re
                content = re.sub(r"<thought>.*?</thought>", "", content, flags=re.DOTALL).strip()
            extraction_messages.append({"role": role, "content": content})

    try:
        response = completion(
            model=model,
            messages=extraction_messages,
            llm_config=llm_config,
            response_format={"type": "json_object"},
            temperature=0.1,
        )
        content_str = response.choices[0].message.content
        data = json.loads(content_str)
        memories = data.get("memories", [])
        
        # Validate schema
        valid_memories = []
        for mem in memories:
            if isinstance(mem, dict) and "content" in mem and "type" in mem:
                # Ensure type is one of the permitted types
                m_type = mem.get("type", "Fact")
                if m_type not in ("Preference", "Fact", "Context", "Codebase"):
                    m_type = "Fact"
                    
                valid_memories.append({
                    "content": str(mem["content"]),
                    "type": m_type,
                    "confidence": float(mem.get("confidence", 0.8)),
                    "source": str(mem.get("source", ""))
                })
        return valid_memories
    except Exception as e:
        # Graceful fallback on API or parsing failures
        print(f"[sidecar][memory_extract] Failed to extract memories: {e}", flush=True)
        return []
