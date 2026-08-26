import { describe, expect, it } from "vitest";

import { calculateMenuPosition, viewportPointToLocalPoint, viewportRectToLocalRect } from "./zoomGeometry";

describe("viewportRectToLocalRect", () => {
  it("keeps viewport measurements unchanged at the standard scale", () => {
    expect(viewportRectToLocalRect(
      { left: 8, top: 32, width: 260, height: 720 },
      { left: 20, top: 138, width: 236, height: 36 },
      260,
    )).toEqual({ x: 12, y: 106, width: 236, height: 36 });
  });

  it.each([0.9, 1.15, 1.3])(
    "converts CSS zoom %s measurements back to container-local coordinates",
    (zoom) => {
      const result = viewportRectToLocalRect(
        { left: 8, top: 32, width: 260 * zoom, height: 720 * zoom },
        {
          left: 8 + 12 * zoom,
          top: 32 + 106 * zoom,
          width: 236 * zoom,
          height: 36 * zoom,
        },
        260,
      );

      expect(result.x).toBeCloseTo(12);
      expect(result.y).toBeCloseTo(106);
      expect(result.width).toBeCloseTo(236);
      expect(result.height).toBeCloseTo(36);
    },
  );

  it("falls back to an unscaled conversion when layout width is unavailable", () => {
    expect(viewportRectToLocalRect(
      { left: 8, top: 32, width: 260, height: 720 },
      { left: 20, top: 138, width: 236, height: 36 },
      0,
    )).toEqual({ x: 12, y: 106, width: 236, height: 36 });
  });
});

describe("viewportPointToLocalPoint and calculateMenuPosition", () => {
  it.each([1, 0.9, 1.15, 1.3])("correctly scales viewport points to local points for zoom factor %s", (scale) => {
    const { x, y } = viewportPointToLocalPoint(130, 260, scale);
    expect(x).toBeCloseTo(130 / scale);
    expect(y).toBeCloseTo(260 / scale);
  });

  it("calculates clamped menu position properly", () => {
    const pos = calculateMenuPosition(100, 200, 160, 100, 8, 1);
    expect(pos.left).toBe(100);
    expect(pos.top).toBe(200);
  });
});
