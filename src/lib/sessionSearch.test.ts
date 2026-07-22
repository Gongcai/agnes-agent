import { describe, expect, it } from "vitest";
import { searchSessionsByTitle } from "./sessionSearch";

const sessions = [
  { id: "session-1", agent_id: "agent-1", title: "项目发布计划" },
  { id: "session-2", agent_id: "agent-1", title: "API Design Review" },
  { id: "session-3", agent_id: "agent-2", title: "项目复盘" },
];

describe("session title search", () => {
  it("only searches sessions owned by the active agent", () => {
    expect(searchSessionsByTitle(sessions, "agent-1", "项目").map((session) => session.id))
      .toEqual(["session-1"]);
  });

  it("matches trimmed titles without case sensitivity", () => {
    expect(searchSessionsByTitle(sessions, "agent-1", "  design  ").map((session) => session.id))
      .toEqual(["session-2"]);
  });

  it("returns all active-agent sessions for an empty query", () => {
    expect(searchSessionsByTitle(sessions, "agent-1", "").map((session) => session.id))
      .toEqual(["session-1", "session-2"]);
  });
});
