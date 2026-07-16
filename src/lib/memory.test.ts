import { describe, expect, it } from "vitest";

import {
  embeddingModelName,
  formatMemoryTime,
  memoryEmbeddingProgress,
  memoryMatchesQuery,
  parseMemoryKeywords,
} from "./memory";

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

  it("formats embedding coverage and model references", () => {
    expect(memoryEmbeddingProgress(3, 4)).toBe(75);
    expect(memoryEmbeddingProgress(1, 0)).toBe(0);
    expect(memoryEmbeddingProgress(8, 4)).toBe(100);
    expect(embeddingModelName("provider-id/Qwen3-Embedding-8B")).toBe("Qwen3-Embedding-8B");
    expect(embeddingModelName(null)).toBe("未配置嵌入模型");
  });
});
