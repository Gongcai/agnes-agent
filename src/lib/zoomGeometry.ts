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

export function getRootZoomScale(): number {
  if (typeof document === "undefined") return 1;
  const root = document.documentElement;
  const zoomStr = root.style.zoom;
  if (zoomStr) {
    const parsed = parseFloat(zoomStr);
    if (Number.isFinite(parsed) && parsed > 0) return parsed;
  }
  if (root.offsetWidth > 0) {
    const measured = root.getBoundingClientRect().width / root.offsetWidth;
    if (Number.isFinite(measured) && measured > 0) return measured;
  }
  return 1;
}

export function viewportPointToLocalPoint(
  clientX: number,
  clientY: number,
  scale = getRootZoomScale(),
): { x: number; y: number } {
  const safeScale = Number.isFinite(scale) && scale > 0 ? scale : 1;
  return {
    x: clientX / safeScale,
    y: clientY / safeScale,
  };
}

export function calculateMenuPosition(
  clientX: number,
  clientY: number,
  menuWidth = 160,
  menuHeight = 110,
  padding = 8,
  scale = getRootZoomScale(),
): { left: number; top: number } {
  const { x, y } = viewportPointToLocalPoint(clientX, clientY, scale);
  const layoutWidth = typeof document !== "undefined" && document.documentElement.clientWidth > 0
    ? document.documentElement.clientWidth
    : (typeof window !== "undefined" ? window.innerWidth / scale : 1000);
  const layoutHeight = typeof document !== "undefined" && document.documentElement.clientHeight > 0
    ? document.documentElement.clientHeight
    : (typeof window !== "undefined" ? window.innerHeight / scale : 800);

  const maxLeft = Math.max(padding, layoutWidth - menuWidth - padding);
  const maxTop = Math.max(padding, layoutHeight - menuHeight - padding);

  return {
    left: Math.max(padding, Math.min(x, maxLeft)),
    top: Math.max(padding, Math.min(y, maxTop)),
  };
}
