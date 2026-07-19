import { describe, expect, it } from "vitest";

import { normalizeBooleanPreference, normalizeColorScheme } from "./uiPreferences";

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
});
