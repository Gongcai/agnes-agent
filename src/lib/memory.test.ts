import { describe, expect, it } from "vitest";

import { formatMemoryTime, memoryMatchesQuery, parseMemoryKeywords } from "./memory";

describe("structured memory helpers", () => {
  it("normalizes optional keywords", () => {
    expect(parseMemoryKeywords(" Rust, workspace，Rust,  SQLite ")).toEqual([
      "Rust",
      "workspace",
      "SQLite",
    ]);
    expect(parseMemoryKeywords("  ")).toEqual([]);
  });

  it("matches names, keywords, and content", () => {
    const memory = {
      name: "Rust execution core",
      keywords: ["workspace", "SQLite"],
      content: "The desktop app owns tool execution.",
    };
    expect(memoryMatchesQuery(memory, "RUST")).toBe(true);
    expect(memoryMatchesQuery(memory, "sqlite")).toBe(true);
    expect(memoryMatchesQuery(memory, "tool execution")).toBe(true);
    expect(memoryMatchesQuery(memory, "android")).toBe(false);
  });

  it("keeps invalid timestamps readable", () => {
    expect(formatMemoryTime("not-a-time")).toBe("not-a-time");
  });
});
