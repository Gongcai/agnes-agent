import { describe, expect, it } from "vitest";

import { computeThumbLayout } from "./ChatScrollNav";

describe("computeThumbLayout", () => {
  it("fills the track when content does not overflow", () => {
    expect(computeThumbLayout(400, 500, 500, 0)).toEqual({ height: 400, top: 0 });
    expect(computeThumbLayout(400, 600, 500, 0)).toEqual({ height: 400, top: 0 });
  });

  it("scales thumb height to the visible ratio", () => {
    const { height, top } = computeThumbLayout(400, 200, 1000, 0);
    expect(height).toBe(80);
    expect(top).toBe(0);
  });

  it("enforces a minimum thumb height for very long content", () => {
    const { height } = computeThumbLayout(400, 100, 100000, 0);
    expect(height).toBe(44);
  });

  it("maps scroll position onto the remaining track", () => {
    // thumb = 200 (half the track), maxTop = 200, maxScroll = 400
    const mid = computeThumbLayout(400, 400, 800, 200);
    expect(mid).toEqual({ height: 200, top: 100 });
    const bottom = computeThumbLayout(400, 400, 800, 400);
    expect(bottom).toEqual({ height: 200, top: 200 });
  });

  it("clamps the thumb inside the track at the scroll bounds", () => {
    const layout = computeThumbLayout(400, 400, 800, 9999);
    expect(layout.top).toBe(200);
    expect(layout.top + layout.height).toBeLessThanOrEqual(400);
  });

  it("returns zero height for an unmeasurable track", () => {
    expect(computeThumbLayout(0, 400, 800, 0)).toEqual({ height: 0, top: 0 });
  });
});
