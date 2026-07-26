import { describe, expect, it } from "vitest";

import { createMemoryFileTemplate, memoryFileMatchesQuery } from "./memory";

describe("memory filesystem helpers", () => {
  it("matches paths, descriptions, and aliases", () => {
    const file = {
      path: "/areas/agnes-agent.md",
      description: "Agnes 项目的持续决策",
      aliases: ["agnes-repo"],
    };
    expect(memoryFileMatchesQuery(file, "AGNES-AGENT")).toBe(true);
    expect(memoryFileMatchesQuery(file, "持续决策")).toBe(true);
    expect(memoryFileMatchesQuery(file, "agnes-repo")).toBe(true);
    expect(memoryFileMatchesQuery(file, "旅行")).toBe(false);
  });

  it("creates a valid user-surface Markdown template", () => {
    expect(createMemoryFileTemplate("/topics/package-manager.md")).toContain(
      "name: package-manager\n",
    );
    expect(createMemoryFileTemplate("/topics/package-manager.md")).toContain("sources: [user]");
    expect(createMemoryFileTemplate("topics/package-manager.md")).toBe("");
    expect(createMemoryFileTemplate("/other/file.md")).toBe("");
  });
});
