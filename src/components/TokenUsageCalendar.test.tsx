import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { getTokenIntensity, TokenUsageCalendar } from "./TokenUsageCalendar";

describe("TokenUsageCalendar", () => {
  it("renders the current 53-week activity window with exact usage labels", () => {
    const html = renderToStaticMarkup(
      <TokenUsageCalendar
        today={new Date(2026, 6, 22, 12)}
        days={[
          {
            date: "2026-07-20",
            input_tokens: 240,
            cached_tokens: 120,
            output_tokens: 60,
            total_tokens: 300,
          },
          {
            date: "2026-07-22",
            input_tokens: 900,
            cached_tokens: 400,
            output_tokens: 300,
            total_tokens: 1_200,
          },
          {
            date: "2026-07-23",
            input_tokens: 10_000,
            cached_tokens: 0,
            output_tokens: 1_000,
            total_tokens: 11_000,
          },
        ]}
      />,
    );

    expect(html).toContain("2 个活跃日 · 1,500 Token");
    expect(html).toContain("2026年7月22日：1,200 Token");
    expect(html).toContain("输入 900 · 缓存 400 · 输出 300");
    expect(html.match(/role="gridcell"/g)).toHaveLength(368);
  });

  it("uses a logarithmic four-level scale", () => {
    expect(getTokenIntensity(0, 100_000)).toBe(0);
    expect(getTokenIntensity(1, 100_000)).toBe(1);
    expect(getTokenIntensity(100_000, 100_000)).toBe(4);
    expect(getTokenIntensity(200_000, 100_000)).toBe(4);
  });
});
