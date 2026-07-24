export interface RectLike {
  left: number;
  top: number;
  width: number;
  height: number;
}

export interface LocalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function viewportRectToLocalRect(
  containerRect: RectLike,
  targetRect: RectLike,
  containerLayoutWidth: number,
): LocalRect {
  const measuredScale = containerLayoutWidth > 0
    ? containerRect.width / containerLayoutWidth
    : 1;
  const scale = Number.isFinite(measuredScale) && measuredScale > 0 ? measuredScale : 1;

  return {
    x: (targetRect.left - containerRect.left) / scale,
    y: (targetRect.top - containerRect.top) / scale,
    width: targetRect.width / scale,
    height: targetRect.height / scale,
  };
}
