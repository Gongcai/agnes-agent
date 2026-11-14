import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Message } from "../store/useAgentStore";
import { subscribeUIPreferenceChanges } from "../lib/uiPreferences";

export interface ThumbLayout {
  height: number;
  top: number;
}

interface OverflowSnapshot {
  clientHeight: number;
  scrollHeight: number;
}

// Overflow state is derived from the latest snapshot instead of being latched
// into component state, so the nav can never get stuck "not overflowing".
export function isOverflowing(snapshot: OverflowSnapshot | null): boolean {
  if (!snapshot) return false;
  return snapshot.scrollHeight > snapshot.clientHeight + 1;
}

const MIN_THUMB_HEIGHT = 44;
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
  // Latest scroll geometry read from the container. Kept in a ref so overflow
  // can be re-evaluated when the track resizes without waiting for a sync that
  // needs the track to be mounted.
  const scrollMetricsRef = useRef<OverflowSnapshot | null>(null);
  const [markers, setMarkers] = useState<AnchorMarker[]>([]);
  const [dragging, setDragging] = useState(false);
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);
  const rafRef = useRef(0);
  const dragStateRef = useRef({ startY: 0, startScrollTop: 0 });

  const messageMeta = useMemo(() => {
    const map = new Map<string, { role: "user" | "assistant"; label: string }>();
    for (const message of messages) {
      map.set(message.id, { role: message.role, label: messageLabel(message) });
    }
    return map;
  }, [messages]);

  const syncFromContainer = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const { clientHeight, scrollHeight, scrollTop } = container;
    scrollMetricsRef.current = { clientHeight, scrollHeight };
    // A missing track only means "we cannot render a thumb this frame"; it must
    // never abort the sync, otherwise the nav can latch onto a stale snapshot.
    const nextTrackHeight = trackRef.current?.clientHeight ?? 0;
    if (nextTrackHeight <= 0) return;
    setMetrics((previous) => {
      if (
        previous
        && previous.clientHeight === clientHeight
        && previous.scrollHeight === scrollHeight
        && previous.scrollTop === scrollTop
        && previous.trackHeight === nextTrackHeight
      ) {
        return previous;
      }
      return { clientHeight, scrollHeight, scrollTop, trackHeight: nextTrackHeight };
    });
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

  // Scroll keeps the thumb/markers in sync.
  useEffect(() => {
    const container = containerRef.current;
    if (!container || !visible) return;
    container.addEventListener("scroll", scheduleSync, { passive: true });
    scheduleSync();
    return () => {
      container.removeEventListener("scroll", scheduleSync);
      cancelAnimationFrame(rafRef.current);
    };
  }, [containerRef, visible, scheduleSync]);

  // The track is always mounted now, so its own height can be observed directly.
  // It changes when the composer grows/shrinks (--agnes-composer-h) or on window
  // resize, and every change must feed a fresh scroll-geometry read.
  useEffect(() => {
    const track = trackRef.current;
    if (!track) return;
    scheduleSync();
    const observer = new ResizeObserver(scheduleSync);
    observer.observe(track);
    return () => observer.disconnect();
  }, [scheduleSync]);

  // The container can change size (sidebar toggle, composer growth, window
  // resize) without any scroll event. Re-read geometry so overflow is re-judged
  // as soon as the viewport changes.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const observer = new ResizeObserver(() => scheduleSync());
    observer.observe(container);
    return () => observer.disconnect();
  }, [containerRef, scheduleSync]);

  // Content growth (streaming, new messages) changes scrollHeight without resizing the container.
  useEffect(() => {
    if (visible) scheduleSync();
  }, [messages, visible, scheduleSync]);

  // Re-measure scrollHeight and anchor positions when column width or font scale changes.
  useEffect(() => subscribeUIPreferenceChanges((change) => {
    if (visible && (change.chatMaxWidth !== undefined || change.fontScale !== undefined)) {
      scheduleSync();
    }
  }), [visible, scheduleSync]);

  // Drop the cached geometry when the nav is hidden so a remount can never
  // start from a stale "fits in viewport" snapshot.
  useEffect(() => {
    if (visible) return;
    scrollMetricsRef.current = null;
    setMetrics(null);
    setMarkers([]);
    setTooltip(null);
  }, [visible]);

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
  // Overflow decides whether the nav is offered at all; the mount itself is
  // kept stable so the track can always be measured.
  const overflowing = isOverflowing(metrics ?? scrollMetricsRef.current);
  const laidOut = metrics && metrics.trackHeight > 0 ? metrics : null;
  const thumb = laidOut
    ? computeThumbLayout(laidOut.trackHeight, laidOut.clientHeight, laidOut.scrollHeight, laidOut.scrollTop)
    : { height: MIN_THUMB_HEIGHT, top: 0 };

  // A long chat should be discoverable without hovering: the nav is visible
  // whenever the content overflows, and stays visible while being dragged.
  const shown = overflowing || dragging;
  const rootClass = [
    "agnes-scroll-nav",
    shown ? "agnes-scroll-nav--shown" : "",
    dragging ? "agnes-scroll-nav--dragging" : "",
    shown ? "" : "agnes-scroll-nav--hidden",
  ].filter(Boolean).join(" ");

  return (
    <div
      className={rootClass}
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
