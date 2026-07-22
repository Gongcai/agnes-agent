import { describe, expect, it } from "vitest";
import {
  DEFAULT_READING_PREFERENCES,
  normalizeReadingPreferences,
  parseReadingPreferences,
} from "./readingPreferences";

describe("reading preferences", () => {
  it("uses defaults for missing or malformed settings", () => {
    expect(parseReadingPreferences(null)).toEqual(DEFAULT_READING_PREFERENCES);
    expect(parseReadingPreferences("not-json")).toEqual(DEFAULT_READING_PREFERENCES);
  });

  it("clamps typography values to reader-safe ranges", () => {
    expect(normalizeReadingPreferences({
      fontSize: 40,
      lineHeight: 0.5,
      contentWidth: 777,
    })).toEqual({
      fontSize: 26,
      lineHeight: 1.4,
      contentWidth: 780,
    });
  });
});
