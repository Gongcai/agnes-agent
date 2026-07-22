import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ContextUsageRing, formatCompactTokens } from "./ContextUsageRing";

describe("ContextUsageRing", () => {
  it("formats token counts compactly", () => {
    expect(formatCompactTokens(999)).toBe("999");
    expect(formatCompactTokens(12_400)).toBe("12.4K");
    expect(formatCompactTokens(128_000)).toBe("128K");
    expect(formatCompactTokens(1_500_000)).toBe("1.5M");
  });

  it("exposes exact context usage to assistive technology", () => {
    const html = renderToStaticMarkup(
      <ContextUsageRing usedTokens={32_000} limitTokens={128_000} />,
    );

    expect(html).toContain("上下文 32K / 128K（25%）");
    expect(html).toContain('data-status="normal"');
  });

  it("uses warning and danger states near the context limit", () => {
    const warning = renderToStaticMarkup(
      <ContextUsageRing usedTokens={90} limitTokens={100} warningThreshold={0.85} />,
    );
    const danger = renderToStaticMarkup(
      <ContextUsageRing usedTokens={120} limitTokens={100} warningThreshold={0.85} />,
    );

    expect(warning).toContain('data-status="warning"');
    expect(danger).toContain('data-status="danger"');
  });
});
