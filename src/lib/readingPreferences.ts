export interface ReadingPreferences {
  fontSize: number;
  lineHeight: number;
  contentWidth: number;
}

export const DEFAULT_READING_PREFERENCES: ReadingPreferences = {
  fontSize: 18,
  lineHeight: 1.8,
  contentWidth: 760,
};

function finiteNumber(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

export function normalizeReadingPreferences(value: unknown): ReadingPreferences {
  const source = value && typeof value === "object"
    ? value as Partial<ReadingPreferences>
    : {};
  return {
    fontSize: Math.round(Math.min(26, Math.max(14, finiteNumber(source.fontSize, 18)))),
    lineHeight: Math.round(
      Math.min(2.4, Math.max(1.4, finiteNumber(source.lineHeight, 1.8))) * 10,
    ) / 10,
    contentWidth: Math.round(
      Math.min(1_000, Math.max(560, finiteNumber(source.contentWidth, 760))) / 20,
    ) * 20,
  };
}

export function parseReadingPreferences(raw: string | null): ReadingPreferences {
  if (!raw) return DEFAULT_READING_PREFERENCES;
  try {
    return normalizeReadingPreferences(JSON.parse(raw));
  } catch {
    return DEFAULT_READING_PREFERENCES;
  }
}
