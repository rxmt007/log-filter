import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import { getMinimap } from "@/lib/ipc";
import { useSession } from "@/store/session";
import type { MinimapData } from "@/types";

const BUCKETS = 180;
const VIEWPORT_HEIGHT_RATIO = 0.08;
const VIEWPORT_MIN_HEIGHT = 22;

function bucketRanges(buckets: number[]) {
  const sorted = [...new Set(buckets)].sort((a, b) => a - b);
  const ranges: Array<{ start: number; end: number }> = [];
  for (const bucket of sorted) {
    const last = ranges[ranges.length - 1];
    if (last && bucket <= last.end + 1) {
      last.end = bucket;
    } else {
      ranges.push({ start: bucket, end: bucket });
    }
  }
  return ranges;
}

function rangeStyle(range: { start: number; end: number }) {
  const start = (range.start / BUCKETS) * 100;
  const end = ((range.end + 1) / BUCKETS) * 100;
  return {
    top: `${start}%`,
    height: `${Math.max(0.7, end - start)}%`,
  };
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function viewportHeightPx(rect: DOMRect) {
  return Math.max(VIEWPORT_MIN_HEIGHT, rect.height * VIEWPORT_HEIGHT_RATIO);
}

function maxViewportTopPx(rect: DOMRect) {
  return Math.max(0, rect.height - viewportHeightPx(rect));
}

function indexToViewportTopPx(index: number, rect: DOMRect, resultCount: number) {
  if (resultCount <= 1) return 0;
  return clamp((index / (resultCount - 1)) * maxViewportTopPx(rect), 0, maxViewportTopPx(rect));
}

function viewportTopPxToResultIndex(topPx: number, rect: DOMRect, resultCount: number) {
  if (resultCount <= 0 || rect.height <= 0) return null;
  if (resultCount === 1) return 0;
  const maxTop = maxViewportTopPx(rect);
  const frac = maxTop > 0 ? clamp(topPx / maxTop, 0, 1) : 0;
  return clamp(Math.round(frac * (resultCount - 1)), 0, resultCount - 1);
}

function pointerToResultIndex(clientY: number, rect: DOMRect, resultCount: number, grabOffset: number) {
  const topPx = clientY - rect.top - grabOffset;
  return viewportTopPxToResultIndex(topPx, rect, resultCount);
}

export function Minimap() {
  const status = useSession((s) => s.status);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const viewportResultIndex = useSession((s) => s.viewportResultIndex);
  const navigateToResultIndex = useSession((s) => s.navigateToResultIndex);
  const [data, setData] = useState<MinimapData>({ bookmarks: [], errors: [] });
  const [dragging, setDragging] = useState(false);
  const draggingRef = useRef(false);
  const grabOffsetRef = useRef<number | null>(null);
  const frameRef = useRef<number | null>(null);
  const pendingIndexRef = useRef<number | null>(null);
  const resultCount = status.filteredLines;

  const scheduleResultIndex = useCallback(
    (index: number) => {
      pendingIndexRef.current = index;
      if (frameRef.current != null) return;
      frameRef.current = window.requestAnimationFrame(() => {
        frameRef.current = null;
        const next = pendingIndexRef.current;
        pendingIndexRef.current = null;
        if (next != null) {
          navigateToResultIndex(next, { align: "start", reason: "minimap" });
        }
      });
    },
    [navigateToResultIndex],
  );

  const updateFromPointer = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      if (!resultCount) return;
      const rect = event.currentTarget.getBoundingClientRect();
      const resultIndex = pointerToResultIndex(
        event.clientY,
        rect,
        resultCount,
        grabOffsetRef.current ?? 0,
      );
      if (resultIndex != null) {
        scheduleResultIndex(resultIndex);
      }
    },
    [resultCount, scheduleResultIndex],
  );

  const endDrag = useCallback(() => {
    draggingRef.current = false;
    grabOffsetRef.current = null;
    setDragging(false);
  }, []);

  useEffect(() => {
    if (!status.totalBytes) {
      setData({ bookmarks: [], errors: [] });
      return;
    }
    getMinimap(BUCKETS)
      .then(setData)
      .catch(() => setData({ bookmarks: [], errors: [] }));
  }, [
    status.totalBytes,
    status.filteredLines,
    status.errorLines,
    sessionId,
    bookmarkRevision,
    filterResultRevision,
  ]);

  useEffect(() => {
    return () => {
      if (frameRef.current != null) {
        window.cancelAnimationFrame(frameRef.current);
      }
    };
  }, []);

  const viewportTop = resultCount
    ? Math.min(92, Math.max(0, (viewportResultIndex / Math.max(1, resultCount - 1)) * 92))
    : 0;

  return (
    <button
      className="lf-minimap"
      data-dragging={dragging || undefined}
      type="button"
      aria-label="日志小地图"
      onPointerDown={(event) => {
        if (!resultCount) return;
        event.preventDefault();
        const rect = event.currentTarget.getBoundingClientRect();
        const pointerY = event.clientY - rect.top;
        const viewportTopPx = indexToViewportTopPx(viewportResultIndex, rect, resultCount);
        const insideViewport =
          pointerY >= viewportTopPx && pointerY <= viewportTopPx + viewportHeightPx(rect);
        grabOffsetRef.current = insideViewport ? pointerY - viewportTopPx : 0;
        draggingRef.current = true;
        setDragging(true);
        try {
          event.currentTarget.setPointerCapture(event.pointerId);
        } catch {
          // Pointer capture can fail if the pointer was already canceled; dragging still ends safely.
        }
        if (!insideViewport) {
          updateFromPointer(event);
        }
      }}
      onPointerMove={(event) => {
        if (!draggingRef.current) return;
        event.preventDefault();
        updateFromPointer(event);
      }}
      onPointerUp={(event) => {
        event.preventDefault();
        endDrag();
      }}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
    >
      {bucketRanges(data.bookmarks).map((range) => (
        <span
          className="lf-minimap-segment lf-minimap-bookmark"
          key={`b-${range.start}-${range.end}`}
          style={rangeStyle(range)}
        />
      ))}
      {bucketRanges(data.errors).map((range) => (
        <span
          className="lf-minimap-segment lf-minimap-error"
          key={`e-${range.start}-${range.end}`}
          style={rangeStyle(range)}
        />
      ))}
      <span className="lf-minimap-viewport" style={{ top: `${viewportTop}%` }} />
    </button>
  );
}
