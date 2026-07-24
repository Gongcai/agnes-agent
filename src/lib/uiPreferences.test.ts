import { describe, expect, it } from "vitest";

import {
  DEFAULT_MAX_OUTPUT_TOKENS,
  fontScaleFactor,
  normalizeBooleanPreference,
  normalizeColorScheme,
  normalizeFontScale,
  normalizeMaxOutputTokens,
  resolveColorScheme,
} from "./uiPreferences";

describe("UI preference normalization", () => {
  it("accepts supported color schemes and resolves the system preference", () => {
    expect(normalizeColorScheme("dark")).toBe("dark");
    expect(normalizeColorScheme("light")).toBe("light");
    expect(normalizeColorScheme("system")).toBe("system");
    expect(normalizeColorScheme(null)).toBe("light");
    expect(resolveColorScheme("system", true)).toBe("dark");
    expect(resolveColorScheme("system", false)).toBe("light");
    expect(resolveColorScheme("light", true)).toBe("light");
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

  it("normalizes the font scale and maps each step to a zoom factor", () => {
    expect(normalizeFontScale("small")).toBe("small");
    expect(normalizeFontScale("large")).toBe("large");
    expect(normalizeFontScale("xlarge")).toBe("xlarge");
    expect(normalizeFontScale("standard")).toBe("standard");
    expect(normalizeFontScale(null)).toBe("standard");
    expect(normalizeFontScale("bogus")).toBe("standard");
    expect(fontScaleFactor("standard")).toBe(1);
    expect(fontScaleFactor("small")).toBeLessThan(1);
    expect(fontScaleFactor("large")).toBeGreaterThan(1);
    expect(fontScaleFactor("xlarge")).toBeGreaterThan(fontScaleFactor("large"));
  });
});
