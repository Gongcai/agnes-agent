import { describe, expect, it } from "vitest";
import type { ModelProvider } from "../store/useAgentStore";
import { filterModelProviders } from "./modelPicker";

const provider = (id: string, name: string, models: string[]): ModelProvider => ({
  id, name, kind: "openai", api_base: null, is_default: false, has_api_key: true,
  created_at: "", updated_at: "",
  models: models.map((model) => ({
    id: model, context_window: null,
    capabilities: { input_modalities: ["text"], output_modalities: ["text"], embedding: false },
  })),
});

describe("filterModelProviders", () => {
  const providers = [
    provider("first", "DeepSeek 官方", ["deepseek-v4-pro", "deepseek-v4-flash"]),
    provider("second", "中转服务", ["deepseek-v4-pro", "gemini/3.1-pro"]),
    provider("empty", "未配置", []),
  ];

  it("matches provider and model names together without case sensitivity", () => {
    const result = filterModelProviders(providers, "  DEEPSEEK 官方 Pro  ");
    expect(result.map((item) => [item.id, item.models.map((model) => model.id)]))
      .toEqual([["first", ["deepseek-v4-pro"]]]);
  });

  it("distinguishes identical model names from different providers", () => {
    const result = filterModelProviders(providers, "deepseek-v4-pro", "second");
    expect(result.map((item) => item.id)).toEqual(["second"]);
    expect(result[0].models[0].id).toBe("deepseek-v4-pro");
  });

  it("preserves full model IDs containing slashes and handles empty results", () => {
    expect(filterModelProviders(providers, "gemini/3.1")[0].models[0].id).toBe("gemini/3.1-pro");
    expect(filterModelProviders(providers, "不存在的模型")).toEqual([]);
    expect(filterModelProviders(providers, "", "empty")).toEqual([]);
  });

  it("keeps provider order and does not modify the original catalog", () => {
    expect(filterModelProviders(providers, " ").map((item) => item.id)).toEqual(["first", "second"]);
    filterModelProviders(providers, "flash");
    expect(providers[0].models).toHaveLength(2);
  });
});
