import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Message } from "../store/useAgentStore";

export interface ThumbLayout {
  height: number;
  top: number;
}

const MIN_THUMB_HEIGHT = 32;
const IDLE_HIDE_DELAY = 1200;
const FLASH_DURATION = 1200;
const PREVIEW_LENGTH = 30;

export function computeThumbLayout(
  trackHeight: number,
  clientHeight: number,
  scrollHeight: number,
  scrollTop: number,
): ThumbLayout {
  if (trackHeight <= 0) return { height: 0, top: 0 };
  if (scrollHeight <= clientHeight) return { height: trackHeight, top: 0 };
  const rawHeight = (clientHeight / scrollHeight) * trackHeight;
  const height = Math.min(trackHeight, Math.max(MIN_THUMB_HEIGHT, rawHeight));
  const maxTop = trackHeight - height;
  const maxScroll = scrollHeight - clientHeight;
  const top = maxScroll > 0 ? (scrollTop / maxScroll) * maxTop : 0;
  return { height, top: Math.min(maxTop, Math.max(0, top)) };
}

interface ScrollMetrics {
  clientHeight: number;
  scrollHeight: number;
  scrollTop: number;
  trackHeight: number;
}

interface AnchorMarker {
  id: string;
  role: "user" | "assistant";
  ratio: number;
  label: string;
}

interface TooltipState {
  top: number;
  text: string;
}

interface ChatScrollNavProps {
  containerRef: React.RefObject<HTMLDivElement | null>;
  messages: Message[];
  visible: boolean;
}

function messageLabel(message: Message): string {
  const text = message.parts
    .filter((part) => part.kind === "text")
    .map((part) => part.content)
    .join("")
    .trim();
  const preview = text.length > PREVIEW_LENGTH ? `${text.slice(0, PREVIEW_LENGTH)}…` : text;
  const time = message.created_at || "";
  return preview ? `${time}  ${preview}` : time || "(空消息)";
}

export function ChatScrollNav({ containerRef, messages, visible }: ChatScrollNavProps) {
  const trackRef = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState<ScrollMetrics | null>(null);
  const [markers, setMarkers] = useState<AnchorMarker[]>([]);
  const [active, setActive] = useState(false);
  const [hovered, setHovered] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  const rafRef = useRef(0);
  const hideTimerRef = useRef(0);
  const dragStateRef = useRef({ startY: 0, startScrollTop: 0 });

  const messageMeta = useMemo(() => {
    const map = new Map<string, { role: "user" | "assistant"; label: string }>();
    for (const message of messages) {
      map.set(message.id, { role: message.role, label: messageLabel(message) });
    }
    return map;
  }, [messages]);

  const pulseActive = useCallback(() => {
    setActive(true);
    window.clearTimeout(hideTimerRef.current);
    hideTimerRef.current = window.setTimeout(() => setActive(false), IDLE_HIDE_DELAY);
  }, []);

  const syncFromContainer = useCallback(() => {
    const container = containerRef.current;
    const track = trackRef.current;
    if (!container || !track) return;
    const { clientHeight, scrollHeight, scrollTop } = container;
    setMetrics({ clientHeight, scrollHeight, scrollTop, trackHeight: track.clientHeight });
    if (scrollHeight <= 0) {
      setMarkers([]);
      return;
    }
    const rows = container.querySelectorAll<HTMLElement>("[data-mid]");
    const next: AnchorMarker[] = [];
    rows.forEach((row) => {
      const id = row.dataset.mid;
      if (!id) return;
      const meta = messageMeta.get(id);
      if (!meta) return;
      next.push({ id, role: meta.role, ratio: row.offsetTop / scrollHeight, label: meta.label });
    });
    setMarkers(next);
  }, [containerRef, messageMeta]);

  const scheduleSync = useCallback(() => {
    cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(syncFromContainer);
  }, [syncFromContainer]);

  // Scroll + resize keep thumb/markers in sync; scroll also pulses visibility.
  useEffect(() => {
    const container = containerRef.current;
    if (!container || !visible) return;
    const handleScroll = () => {
      pulseActive();
      scheduleSync();
    };
    container.addEventListener("scroll", handleScroll, { passive: true });
    const observer = new ResizeObserver(scheduleSync);
    observer.observe(container);
    scheduleSync();
    return () => {
      container.removeEventListener("scroll", handleScroll);
      observer.disconnect();
      cancelAnimationFrame(rafRef.current);
    };
  }, [containerRef, visible, pulseActive, scheduleSync]);

  // Content growth (streaming, new messages) changes scrollHeight without resizing the container.
  useEffect(() => {
    if (visible) scheduleSync();
  }, [messages, visible, scheduleSync]);

  useEffect(() => () => window.clearTimeout(hideTimerRef.current), []);

  const jumpToRatio = useCallback(
    (ratio: number) => {
      const container = containerRef.current;
      if (!container) return;
      const maxScroll = container.scrollHeight - container.clientHeight;
      container.scrollTop = Math.max(0, Math.min(maxScroll, ratio * maxScroll));
    },
    [containerRef],
  );

  const handleTrackPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.target !== event.currentTarget) return;
    const track = trackRef.current;
    if (!track || !metrics) return;
    const rect = track.getBoundingClientRect();
    const thumb = computeThumbLayout(metrics.trackHeight, metrics.clientHeight, metrics.scrollHeight, metrics.scrollTop);
    const ratio = (event.clientY - rect.top - thumb.height / 2) / Math.max(1, metrics.trackHeight - thumb.height);
    jumpToRatio(ratio);
  };

  const handleThumbPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const container = containerRef.current;
    if (!container) return;
    dragStateRef.current = { startY: event.clientY, startScrollTop: container.scrollTop };
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
    setActive(true);
  };

  const handleThumbPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    const container = containerRef.current;
    if (!container || !metrics) return;
    const thumb = computeThumbLayout(metrics.trackHeight, metrics.clientHeight, metrics.scrollHeight, metrics.scrollTop);
    const maxTop = Math.max(1, metrics.trackHeight - thumb.height);
    const maxScroll = container.scrollHeight - container.clientHeight;
    const delta = ((event.clientY - dragStateRef.current.startY) / maxTop) * maxScroll;
    container.scrollTop = dragStateRef.current.startScrollTop + delta;
  };

  const handleThumbPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    setDragging(false);
    setTooltip(null);
    pulseActive();
  };

  // Drag preview: nearest anchor at/above the current viewport top.
  useEffect(() => {
    if (!dragging || !metrics || markers.length === 0) return;
    const scrollRatio = metrics.scrollTop / Math.max(1, metrics.scrollHeight);
    let current: AnchorMarker | null = null;
    for (const marker of markers) {
      if (marker.ratio <= scrollRatio + 0.001) current = marker;
      else break;
    }
    const fallback = markers[0];
    const shown = current ?? fallback;
    if (!shown) return;
    const thumb = computeThumbLayout(metrics.trackHeight, metrics.clientHeight, metrics.scrollHeight, metrics.scrollTop);
    setTooltip({ top: thumb.top + thumb.height / 2, text: shown.label });
  }, [dragging, metrics, markers]);

  const flashMessage = useCallback(
    (id: string) => {
      const container = containerRef.current;
      const row = container?.querySelector<HTMLElement>(`[data-mid="${CSS.escape(id)}"]`);
      if (!row) return;
      row.scrollIntoView({ behavior: "smooth", block: "center" });
      row.classList.add("agnes-message-row--flash");
      window.setTimeout(() => row.classList.remove("agnes-message-row--flash"), FLASH_DURATION);
    },
    [containerRef],
  );

  if (!visible) return null;
  const overflowing = metrics ? metrics.scrollHeight > metrics.clientHeight + 1 : false;
  if (metrics && !overflowing) return null;

  const thumb = metrics
    ? computeThumbLayout(metrics.trackHeight, metrics.clientHeight, metrics.scrollHeight, metrics.scrollTop)
    : { height: MIN_THUMB_HEIGHT, top: 0 };

  const shown = active || hovered || dragging;
  const rootClass = [
    "agnes-scroll-nav",
    shown ? "agnes-scroll-nav--shown" : "",
    dragging ? "agnes-scroll-nav--dragging" : "",
  ].filter(Boolean).join(" ");

  return (
    <div
      className={rootClass}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      aria-hidden={!shown}
    >
      <div ref={trackRef} className="agnes-scroll-nav-track" onPointerDown={handleTrackPointerDown}>
        {markers.map((marker) => (
          <button
            key={marker.id}
            type="button"
            className={`agnes-scroll-nav-marker agnes-scroll-nav-marker--${marker.role}`}
            style={{ top: `${marker.ratio * 100}%` }}
            onClick={() => flashMessage(marker.id)}
            onMouseEnter={(event) => {
              if (dragging) return;
              const rect = event.currentTarget.getBoundingClientRect();
              const trackRect = trackRef.current?.getBoundingClientRect();
              setTooltip({
                top: rect.top - (trackRect?.top ?? 0) + rect.height / 2,
                text: marker.label,
              });
            }}
            onMouseLeave={() => {
              if (!dragging) setTooltip(null);
            }}
            tabIndex={-1}
            aria-label={marker.label}
          />
        ))}
        <div
          className="agnes-scroll-nav-thumb"
          style={{ height: `${thumb.height}px`, top: `${thumb.top}px` }}
          onPointerDown={handleThumbPointerDown}
          onPointerMove={handleThumbPointerMove}
          onPointerUp={handleThumbPointerUp}
          onPointerCancel={handleThumbPointerUp}
        />
      </div>
      {tooltip && (
        <div className="agnes-scroll-nav-tooltip" style={{ top: `${tooltip.top}px` }}>
          {tooltip.text}
        </div>
      )}
    </div>
  );
}
