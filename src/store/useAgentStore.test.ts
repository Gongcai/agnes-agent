import { describe, expect, it } from "vitest";

import {
  buildOptimisticEditBranch,
  buildOptimisticRegenerationBranch,
  preserveLatestRunRenderKeys,
  type Message,
  useAgentStore,
} from "./useAgentStore";

const user: Message = {
  id: "user-1",
  session_id: "session-1",
  role: "user",
  seq: 0,
  status: "complete",
  parts: [
    { id: "text-1", kind: "text", content: "旧问题" },
    {
      id: "attachment-1",
      kind: "attachment",
      content: "",
      metadata: { attachmentKind: "local_file", id: "file-1", name: "资料.txt" },
    },
  ],
  created_at: "10:00:00",
  parent_id: null,
  version_index: 0,
  version_count: 1,
  is_leaf: false,
  input_tokens: 0,
  cached_tokens: 0,
  output_tokens: 0,
  context_tokens: 0,
};

const assistant: Message = {
  ...user,
  id: "assistant-1",
  role: "assistant",
  seq: 1,
  parts: [{ id: "answer-1", kind: "text", content: "旧回答" }],
  parent_id: user.id,
  is_leaf: true,
};

describe("optimistic conversation branches", () => {
  it("switches an edited user message to a visible pending branch immediately", () => {
    const branch = buildOptimisticEditBranch([user, assistant], user.id, "新问题", 42);

    expect(branch).not.toBeNull();
    expect(branch).toHaveLength(2);
    expect(branch?.[0]).toMatchObject({
      id: "temp_edit_user_42",
      role: "user",
      version_index: 1,
      version_count: 2,
    });
    expect(branch?.[0].parts.map((part) => part.kind)).toEqual(["text", "attachment"]);
    expect(branch?.[0].parts[0].content).toBe("新问题");
    expect(branch?.[1]).toMatchObject({
      id: "temp_edit_assistant_42",
      role: "assistant",
      status: "pending",
      parent_id: "temp_edit_user_42",
    });
  });

  it("switches regeneration to a visible pending assistant sibling immediately", () => {
    const branch = buildOptimisticRegenerationBranch([user, assistant], assistant.id, 84);

    expect(branch).not.toBeNull();
    expect(branch).toHaveLength(2);
    expect(branch?.[1]).toMatchObject({
      id: "temp_regenerate_assistant_84",
      role: "assistant",
      status: "pending",
      parent_id: user.id,
      version_index: 1,
      version_count: 2,
    });
  });

  it("keeps an early tool approval card when the optimistic branch is hydrated", () => {
    const optimistic = buildOptimisticEditBranch([user, assistant], user.id, "新问题", 42)!;
    optimistic[1] = {
      ...optimistic[1],
      status: "streaming",
      parts: [
        {
          id: "local-tool-part",
          kind: "tool_call",
          content: "Calling shell",
          tool_call: {
            id: "tool-1",
            tool: "shell",
            args: "{}",
            risk: "High",
            status: "pending_approval",
          },
        },
      ],
    };
    const persisted: Message[] = [
      { ...optimistic[0], id: "edited-user-db" },
      {
        ...optimistic[1],
        id: "pending-assistant-db",
        parent_id: "edited-user-db",
        status: "pending",
        parts: [],
      },
    ];

    const reconciled = preserveLatestRunRenderKeys(optimistic, persisted, "session-1");

    expect(reconciled[1]).toMatchObject({
      id: "pending-assistant-db",
      status: "streaming",
    });
    expect(reconciled[1].parts[0].tool_call).toMatchObject({
      id: "tool-1",
      status: "pending_approval",
    });
  });
});

describe("typed assistant deltas", () => {
  it("keeps reasoning out of text after an in-flight message is reloaded", () => {
    const reloadedAssistant: Message = {
      ...assistant,
      id: "assistant-streaming",
      status: "streaming",
      parts: [],
      _streamingInThought: undefined,
    };
    useAgentStore.setState({
      activeSessionId: "session-1",
      isStreaming: true,
      messages: [user, reloadedAssistant],
    });

    useAgentStore.getState().appendStreamingDelta(
      "<thought>private reasoning",
      [{ kind: "thought", content: "private reasoning" }],
      true,
    );
    useAgentStore.getState().appendStreamingDelta(
      " continues</thought>public answer",
      [
        { kind: "thought", content: " continues" },
        { kind: "text", content: "public answer" },
      ],
      false,
    );

    const streamed = useAgentStore.getState().messages.at(-1)!;
    expect(streamed.parts).toMatchObject([
      { kind: "thought", content: "private reasoning continues" },
      { kind: "text", content: "public answer" },
    ]);
    expect(streamed._streamingInThought).toBe(false);

    useAgentStore.setState({ activeSessionId: null, isStreaming: false, messages: [] });
  });
});
