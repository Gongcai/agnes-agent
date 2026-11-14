import {
  createHighlighterCoreSync,
  type HighlighterCore,
  type ThemeRegistrationRaw,
  type ThemedToken,
} from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { bundledLanguages, type BundledLanguage } from "shiki/langs";

export type { ThemedToken };

export const AGNES_CODE_THEME_NAME = "agnes-warm-dark";

/**
 * Warm editorial dark syntax theme harmonized with Agnes's Claude-inspired
 * terracotta, warm stone, and parchment design tokens. Comments intentionally
 * stay upright (non-italic) so CJK notes and ASCII box-drawing trees remain sharp.
 */
const AGNES_WARM_DARK_THEME: ThemeRegistrationRaw = {
  name: AGNES_CODE_THEME_NAME,
  type: "dark",
  colors: {
    "editor.background": "#141412",
    "editor.foreground": "#E5DFD5",
  },
  settings: [
    {
      settings: {
        background: "#141412",
        foreground: "#E5DFD5",
      },
    },
    {
      scope: ["comment", "punctuation.definition.comment", "string.comment"],
      settings: { foreground: "#868076" },
    },
    {
      scope: [
        "delimiter.bracket",
        "delimiter",
        "keyword.operator.rest",
        "keyword.operator.spread",
        "keyword.operator.type.annotation",
        "keyword.operator.relational",
        "keyword.operator.assignment",
        "keyword.operator.type",
        "meta.brace",
        "meta.type.annotation",
        "storage.type.function.arrow",
        "punctuation",
      ],
      settings: { foreground: "#969086" },
    },
    {
      scope: [
        "keyword",
        "keyword.control",
        "keyword.control.directive",
        "punctuation.definition.directive",
        "punctuation.definition.template-expression",
        "storage.type.class.jsdoc",
      ],
      settings: { foreground: "#E38368" },
    },
    {
      scope: [
        "storage",
        "storage.type",
        "storage.modifier",
        "keyword.operator",
        "keyword.operator.new",
        "keyword.operator.delete",
        "keyword.operator.expression",
        "keyword.other.unit",
      ],
      settings: { foreground: "#DC7A7A" },
    },
    {
      scope: [
        "entity.name.type",
        "entity.name.class",
        "entity.other.inherited-class",
        "support.type",
        "support.class",
        "support.type.primitive",
        "support.type.builtin",
        "type.identifier",
        "storage.type.integral",
        "storage.type.primitive",
        "storage.type.built-in",
      ],
      settings: { foreground: "#75C0B7" },
    },
    {
      scope: [
        "entity.name.namespace",
        "entity.name.type.namespace",
        "entity.name.type.module",
        "entity.name.scope-resolution",
        "support.module",
        "namespace",
      ],
      settings: { foreground: "#8CB8CB" },
    },
    {
      scope: [
        "entity.name.function",
        "support.function",
        "entity.name.function.preprocessor",
        "meta.function-call.generic",
      ],
      settings: { foreground: "#A6C77B" },
    },
    {
      scope: [
        "string",
        "string punctuation.section.embedded source",
        "attribute.value",
      ],
      settings: { foreground: "#E1C07E" },
    },
    {
      scope: ["punctuation.definition.string"],
      settings: { foreground: "#E1C07EB8" },
    },
    {
      scope: ["constant.character.escape", "string.regexp", "source.regexp"],
      settings: { foreground: "#E7A06E" },
    },
    {
      scope: [
        "constant",
        "constant.numeric",
        "number",
        "constant.language",
        "constant.language.boolean",
        "constant.language.null",
        "constant.language.undefined",
        "entity.name.constant",
        "support.constant",
        "variable.language",
      ],
      settings: { foreground: "#D498B6" },
    },
    {
      scope: [
        "variable.other.enummember",
        "variable.other.constant",
        "readonly",
      ],
      settings: { foreground: "#D8A675" },
    },
    {
      scope: [
        "property",
        "meta.property-name",
        "meta.object-literal.key",
        "support.type.property-name",
        "variable.other.property",
        "variable.other.member",
        "variable.other.object.property",
        "entity.name.tag.yaml",
        "keyword.key.toml",
        "attribute.name",
        "entity.other.attribute-name",
      ],
      settings: { foreground: "#CEB27E" },
    },
    {
      scope: ["entity.name.tag", "tag.html", "support.class.component"],
      settings: { foreground: "#E38368" },
    },
    {
      scope: ["variable", "identifier", "variable.parameter"],
      settings: { foreground: "#E5DFD5" },
    },
    {
      scope: ["markup.heading", "markup.heading entity.name"],
      settings: { fontStyle: "bold", foreground: "#E38368" },
    },
    {
      scope: ["markup.bold"],
      settings: { fontStyle: "bold", foreground: "#F2EEE6" },
    },
    {
      scope: ["markup.italic"],
      settings: { fontStyle: "italic", foreground: "#E5DFD5" },
    },
    {
      scope: ["markup.inserted", "punctuation.definition.inserted"],
      settings: { foreground: "#98CA89" },
    },
    {
      scope: ["markup.deleted", "punctuation.definition.deleted"],
      settings: { foreground: "#E67E7E" },
    },
    {
      scope: ["meta.diff.range", "meta.diff.header"],
      settings: { foreground: "#8CB8CB" },
    },
  ],
};

const MAX_HIGHLIGHT_CHARS = 80_000;

const PLAIN_TEXT_ALIASES = new Set([
  "",
  "text",
  "txt",
  "plain",
  "plaintext",
  "none",
  "raw",
  "output",
]);

const LANGUAGE_GRAMMAR_ALIASES: Record<string, BundledLanguage> = {
  "c++": "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  hxx: "cpp",
  "h++": "cpp",
  h: "c",
  rs: "rust",
  py: "python",
  py3: "python",
  python3: "python",
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  sh: "shellscript",
  bash: "shellscript",
  zsh: "shellscript",
  shell: "shellscript",
  tmux: "shellscript",
  env: "dotenv",
  console: "shellsession",
  terminal: "shellsession",
  ps1: "powershell",
  pwsh: "powershell",
  yml: "yaml",
  md: "markdown",
  cs: "csharp",
  "c#": "csharp",
  golang: "go",
  kt: "kotlin",
  kts: "kotlin",
  rb: "ruby",
  objc: "objective-c",
  objcpp: "objective-cpp",
  patch: "diff",
  udiff: "diff",
  docker: "dockerfile",
  make: "makefile",
  mk: "makefile",
  conf: "ini",
  cfg: "ini",
  svg: "xml",
  gql: "graphql",
  hs: "haskell",
  tex: "latex",
  x86asm: "asm",
  nasm: "asm",
};

const LANGUAGE_LABELS: Record<string, string> = {
  c: "C",
  cpp: "C++",
  "c++": "C++",
  cc: "C++",
  cxx: "C++",
  hpp: "C++",
  h: "C / C++ Header",
  rust: "Rust",
  rs: "Rust",
  python: "Python",
  py: "Python",
  typescript: "TypeScript",
  ts: "TypeScript",
  tsx: "TSX",
  javascript: "JavaScript",
  js: "JavaScript",
  jsx: "JSX",
  shellscript: "Shell",
  bash: "Bash",
  sh: "Shell",
  zsh: "Zsh",
  shell: "Shell",
  shellsession: "Terminal",
  console: "Terminal",
  fish: "Fish",
  powershell: "PowerShell",
  pwsh: "PowerShell",
  ps1: "PowerShell",
  tmux: "tmux",
  json: "JSON",
  jsonc: "JSONC",
  json5: "JSON5",
  toml: "TOML",
  yaml: "YAML",
  yml: "YAML",
  cmake: "CMake",
  makefile: "Makefile",
  make: "Makefile",
  dockerfile: "Dockerfile",
  docker: "Dockerfile",
  glsl: "GLSL",
  hlsl: "HLSL",
  wgsl: "WGSL",
  gdscript: "GDScript",
  gdshader: "GDShader",
  csharp: "C#",
  cs: "C#",
  "c#": "C#",
  go: "Go",
  golang: "Go",
  java: "Java",
  kotlin: "Kotlin",
  kt: "Kotlin",
  swift: "Swift",
  zig: "Zig",
  lua: "Lua",
  luau: "Luau",
  sql: "SQL",
  graphql: "GraphQL",
  gql: "GraphQL",
  html: "HTML",
  xml: "XML",
  svg: "SVG",
  css: "CSS",
  scss: "SCSS",
  less: "Less",
  markdown: "Markdown",
  md: "Markdown",
  mdx: "MDX",
  latex: "LaTeX",
  tex: "LaTeX",
  diff: "Diff",
  patch: "Diff",
  ini: "INI",
  dotenv: "Dotenv",
  asm: "Assembly",
  llvm: "LLVM IR",
  wasm: "WebAssembly",
  mermaid: "Mermaid",
  text: "Text",
  plaintext: "Text",
  txt: "Text",
};

const PREWARM_LANGUAGES: BundledLanguage[] = [
  "cpp",
  "c",
  "rust",
  "python",
  "typescript",
  "tsx",
  "javascript",
  "shellscript",
  "json",
  "toml",
  "yaml",
  "cmake",
  "diff",
];

let highlighterInstance: HighlighterCore | null = null;
const loadedGrammars = new Set<string>();
const pendingGrammarLoads = new Map<BundledLanguage, Promise<boolean>>();
const failedGrammars = new Set<BundledLanguage>();
const grammarListeners = new Set<() => void>();

function getHighlighter(): HighlighterCore {
  if (!highlighterInstance) {
    highlighterInstance = createHighlighterCoreSync({
      themes: [AGNES_WARM_DARK_THEME],
      langs: [],
      engine: createJavaScriptRegexEngine({ forgiving: true }),
    });
  }
  return highlighterInstance;
}

function normalizeRawLang(raw: string): string {
  return raw.trim().toLowerCase().replace(/^language-/, "");
}

/** Format a fenced block info tag into a clean human-readable badge label. */
export function formatCodeLanguageLabel(rawLang: string): string {
  const normalized = normalizeRawLang(rawLang);
  if (!normalized) return "Text";
  return LANGUAGE_LABELS[normalized] ?? rawLang.trim();
}

/** Resolve a fenced code block language tag to a Shiki bundled language key. */
export function resolveShikiLanguage(rawLang: string): BundledLanguage | null {
  const normalized = normalizeRawLang(rawLang);
  if (PLAIN_TEXT_ALIASES.has(normalized)) return null;
  const aliased = LANGUAGE_GRAMMAR_ALIASES[normalized];
  if (aliased) return aliased;
  if (normalized in bundledLanguages) {
    return normalized as BundledLanguage;
  }
  return null;
}

/** Strip a single trailing fence newline so code blocks do not render an empty last row. */
export function stripTrailingFenceNewline(code: string): string {
  return code.replace(/\r?\n$/, "");
}

export function isCodeLanguageLoaded(lang: BundledLanguage): boolean {
  if (loadedGrammars.has(lang)) return true;
  const loaded = getHighlighter().getLoadedLanguages().includes(lang);
  if (loaded) loadedGrammars.add(lang);
  return loaded;
}

function notifyGrammarListeners(): void {
  grammarListeners.forEach((listener) => listener());
}

/** Subscribe to newly loaded Shiki grammars. */
export function subscribeCodeLanguageLoaded(listener: () => void): () => void {
  grammarListeners.add(listener);
  return () => {
    grammarListeners.delete(listener);
  };
}

/** Lazily load a Shiki grammar once and notify subscribed CodeBlocks on completion. */
export function ensureCodeLanguageLoaded(rawLang: string): Promise<boolean> {
  const resolved = resolveShikiLanguage(rawLang);
  if (!resolved || failedGrammars.has(resolved)) {
    return Promise.resolve(false);
  }
  if (isCodeLanguageLoaded(resolved)) {
    return Promise.resolve(true);
  }
  const existing = pendingGrammarLoads.get(resolved);
  if (existing) return existing;

  const loader = bundledLanguages[resolved];
  if (!loader) {
    failedGrammars.add(resolved);
    return Promise.resolve(false);
  }

  const task = loader()
    .then((mod) => {
      const highlighter = getHighlighter();
      highlighter.loadLanguageSync(mod.default);
      loadedGrammars.add(resolved);
      for (const loaded of highlighter.getLoadedLanguages()) {
        loadedGrammars.add(loaded);
      }
      pendingGrammarLoads.delete(resolved);
      notifyGrammarListeners();
      return true;
    })
    .catch(() => {
      pendingGrammarLoads.delete(resolved);
      failedGrammars.add(resolved);
      return false;
    });

  pendingGrammarLoads.set(resolved, task);
  return task;
}

/**
 * Synchronously tokenize code if its grammar is already in memory.
 * During streaming, once a grammar has been loaded once, every tick renders
 * highlighted tokens in-place with zero async flash.
 */
export function tokenizeCodeSync(code: string, rawLang: string): ThemedToken[][] | null {
  const cleaned = stripTrailingFenceNewline(code);
  if (!cleaned || cleaned.length > MAX_HIGHLIGHT_CHARS) return null;

  const resolved = resolveShikiLanguage(rawLang);
  if (!resolved || !isCodeLanguageLoaded(resolved)) return null;

  try {
    return getHighlighter().codeToTokensBase(cleaned, {
      lang: resolved,
      theme: AGNES_CODE_THEME_NAME,
    });
  } catch {
    return null;
  }
}

// Pre-warm everyday languages during browser idle time so the first code block renders colored immediately.
if (
  typeof window !== "undefined"
  && !(import.meta as unknown as { env?: { VITEST?: boolean } }).env?.VITEST
) {
  const schedule =
    typeof window.requestIdleCallback === "function"
      ? (cb: () => void) => window.requestIdleCallback(cb, { timeout: 1500 })
      : (cb: () => void) => window.setTimeout(cb, 120);

  schedule(() => {
    for (const lang of PREWARM_LANGUAGES) {
      void ensureCodeLanguageLoaded(lang);
    }
  });
}
