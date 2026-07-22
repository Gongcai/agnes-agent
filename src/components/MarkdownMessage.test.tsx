import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { MarkdownMessage, normalizeLatexDelimiters } from "./MarkdownMessage";

describe("MarkdownMessage LaTeX delimiters", () => {
  it("normalizes inline and display TeX delimiters before Markdown parsing", () => {
    const source = String.raw`行内 \(x^2\)

\[\frac{1}{2}\]`;

    expect(normalizeLatexDelimiters(source)).toBe(
      "行内 $x^2$\n\n$$\n\\frac{1}{2}\n$$",
    );
  });

  it("renders standard TeX delimiters as KaTeX", () => {
    const html = renderToStaticMarkup(
      <MarkdownMessage content={String.raw`行内 \(x^2\) 和 \[\frac{1}{2}\]`} />,
    );

    expect(html).toContain("katex");
    expect(html).toContain("katex-display");
    expect(html).not.toContain("\\[");
    expect(html).not.toContain("\\]");
  });

  it("leaves fenced and inline code examples untouched", () => {
    const source = [
      String.raw`\(not math\)`,
      "",
      "`" + String.raw`\(still code\)` + "`",
      "",
      "```tex",
      "```example",
      String.raw`\[also code\]`,
      "```",
    ].join("\n");
    const normalized = normalizeLatexDelimiters(source);

    expect(normalized).toBe(source.replace(String.raw`\(not math\)`, "$not math$"));
  });

  it("preserves an unfinished delimiter while content is streaming", () => {
    const source = String.raw`正在生成 \(x^2`;

    expect(normalizeLatexDelimiters(source)).toBe(source);
  });

  it("does not treat escaped TeX delimiters as math", () => {
    const source = String.raw`literal \\(x^2\\)`;

    expect(normalizeLatexDelimiters(source)).toBe(source);
  });
});
