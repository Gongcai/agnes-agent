export const UI_COLOR_SCHEME_KEY = "ui:color_scheme";
export const UI_AUTO_EXPAND_THOUGHTS_KEY = "ui:auto_expand_thoughts";
export const UI_AUTO_FOLLOW_STREAMING_KEY = "ui:auto_follow_streaming";
export const UI_DEFAULT_MAX_OUTPUT_TOKENS_KEY = "ui:default_max_output_tokens";
export const DEFAULT_MAX_OUTPUT_TOKENS = 131_072;
export const MIN_MAX_OUTPUT_TOKENS = 128;
export const MAX_MAX_OUTPUT_TOKENS = 1_048_576;

export type ColorScheme = "light" | "dark" | "system";
export type ResolvedColorScheme = Exclude<ColorScheme, "system">;

export interface UIPreferenceChange {
  colorScheme?: ColorScheme;
  resolvedColorScheme?: ResolvedColorScheme;
  autoExpandThoughts?: boolean;
  autoFollowStreaming?: boolean;
}

const COLOR_SCHEME_CACHE_KEY = "agnes.ui.color_scheme";
const AUTO_EXPAND_THOUGHTS_CACHE_KEY = "agnes.ui.auto_expand_thoughts";
const AUTO_FOLLOW_STREAMING_CACHE_KEY = "agnes.ui.auto_follow_streaming";
const UI_PREFERENCE_EVENT = "agnes-ui-preference-change";
let systemColorQuery: MediaQueryList | null = null;
let systemColorListener: ((event: MediaQueryListEvent) => void) | null = null;

export function normalizeColorScheme(value: string | null | undefined): ColorScheme {
  return value === "dark" || value === "system" ? value : "light";
}

function systemPrefersDark(): boolean {
  return typeof window !== "undefined"
    && typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function resolveColorScheme(
  scheme: ColorScheme,
  prefersDark = systemPrefersDark(),
): ResolvedColorScheme {
  const normalized = normalizeColorScheme(scheme);
  return normalized === "system" ? (prefersDark ? "dark" : "light") : normalized;
}

export function normalizeBooleanPreference(
  value: string | boolean | null | undefined,
  fallback: boolean,
): boolean {
  if (value === true || value === "true" || value === "1") return true;
  if (value === false || value === "false" || value === "0") return false;
  return fallback;
}

export function normalizeMaxOutputTokens(value: string | number | null | undefined): number {
  if (value === null || value === undefined) return DEFAULT_MAX_OUTPUT_TOKENS;
  if (typeof value === "string" && value.trim() === "") return DEFAULT_MAX_OUTPUT_TOKENS;
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed)) return DEFAULT_MAX_OUTPUT_TOKENS;
  return Math.min(
    MAX_MAX_OUTPUT_TOKENS,
    Math.max(MIN_MAX_OUTPUT_TOKENS, Math.round(parsed)),
  );
}

function readCache(key: string): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeCache(key: string, value: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // Preferences remain persisted in the application database when storage is unavailable.
  }
}

export function getCachedColorScheme(): ColorScheme {
  return normalizeColorScheme(readCache(COLOR_SCHEME_CACHE_KEY));
}

export function getResolvedColorScheme(
  scheme: ColorScheme = getCachedColorScheme(),
): ResolvedColorScheme {
  return resolveColorScheme(scheme);
}

export function getCachedAutoExpandThoughts(): boolean {
  return normalizeBooleanPreference(readCache(AUTO_EXPAND_THOUGHTS_CACHE_KEY), true);
}

export function getCachedAutoFollowStreaming(): boolean {
  return normalizeBooleanPreference(readCache(AUTO_FOLLOW_STREAMING_CACHE_KEY), true);
}

function clearSystemColorListener(): void {
  if (!systemColorQuery || !systemColorListener) return;
  if (typeof systemColorQuery.removeEventListener === "function") {
    systemColorQuery.removeEventListener("change", systemColorListener);
  } else {
    systemColorQuery.removeListener(systemColorListener);
  }
  systemColorQuery = null;
  systemColorListener = null;
}

function applyResolvedColorScheme(scheme: ResolvedColorScheme): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.dataset.agnesTheme = scheme;
  root.style.colorScheme = scheme;
  announceUIPreferenceChange({ resolvedColorScheme: scheme });
}

export function applyColorScheme(scheme: ColorScheme): ResolvedColorScheme {
  const normalized = normalizeColorScheme(scheme);
  clearSystemColorListener();

  let resolved = resolveColorScheme(normalized);
  if (
    normalized === "system"
    && typeof window !== "undefined"
    && typeof window.matchMedia === "function"
  ) {
    systemColorQuery = window.matchMedia("(prefers-color-scheme: dark)");
    resolved = systemColorQuery.matches ? "dark" : "light";
    systemColorListener = () => {
      if (!systemColorQuery) return;
      applyResolvedColorScheme(systemColorQuery.matches ? "dark" : "light");
    };
    if (typeof systemColorQuery.addEventListener === "function") {
      systemColorQuery.addEventListener("change", systemColorListener);
    } else {
      systemColorQuery.addListener(systemColorListener);
    }
  }

  writeCache(COLOR_SCHEME_CACHE_KEY, normalized);
  applyResolvedColorScheme(resolved);
  return resolved;
}

export function setAutoExpandThoughts(value: boolean): void {
  writeCache(AUTO_EXPAND_THOUGHTS_CACHE_KEY, String(value));
}

export function setAutoFollowStreaming(value: boolean): void {
  writeCache(AUTO_FOLLOW_STREAMING_CACHE_KEY, String(value));
}

export function announceUIPreferenceChange(change: UIPreferenceChange): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<UIPreferenceChange>(UI_PREFERENCE_EVENT, { detail: change }));
}

export function subscribeUIPreferenceChanges(listener: (change: UIPreferenceChange) => void): () => void {
  if (typeof window === "undefined") return () => undefined;
  const handleChange = (event: Event) => {
    listener((event as CustomEvent<UIPreferenceChange>).detail ?? {});
  };
  window.addEventListener(UI_PREFERENCE_EVENT, handleChange);
  return () => window.removeEventListener(UI_PREFERENCE_EVENT, handleChange);
}
