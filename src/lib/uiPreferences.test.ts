import { describe, expect, it } from "vitest";

import {
  DEFAULT_MAX_OUTPUT_TOKENS,
  normalizeBooleanPreference,
  normalizeColorScheme,
  normalizeMaxOutputTokens,
} from "./uiPreferences";

describe("UI preference normalization", () => {
  it("accepts supported color schemes and falls back to light", () => {
    expect(normalizeColorScheme("dark")).toBe("dark");
    expect(normalizeColorScheme("light")).toBe("light");
    expect(normalizeColorScheme("system")).toBe("light");
    expect(normalizeColorScheme(null)).toBe("light");
  });

  it("normalizes persisted boolean values without changing the default", () => {
    expect(normalizeBooleanPreference("true", false)).toBe(true);
    expect(normalizeBooleanPreference("1", false)).toBe(true);
    expect(normalizeBooleanPreference("false", true)).toBe(false);
    expect(normalizeBooleanPreference("0", true)).toBe(false);
    expect(normalizeBooleanPreference(null, true)).toBe(true);
    expect(normalizeBooleanPreference("invalid", false)).toBe(false);
  });

  it("defaults max output tokens to 128K and clamps persisted values", () => {
    expect(normalizeMaxOutputTokens(null)).toBe(DEFAULT_MAX_OUTPUT_TOKENS);
    expect(normalizeMaxOutputTokens("")).toBe(DEFAULT_MAX_OUTPUT_TOKENS);
    expect(normalizeMaxOutputTokens("131072")).toBe(131_072);
    expect(normalizeMaxOutputTokens("64")).toBe(128);
    expect(normalizeMaxOutputTokens(2_000_000)).toBe(1_048_576);
  });
});
