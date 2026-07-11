import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import { getMinimap } from "@/lib/ipc";
import { useSession } from "@/store/session";
import type { MinimapData } from "@/types";

const BUCKETS = 180;

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

function pointerToResultIndex(clientY: number, rect: DOMRect, resultCount: number) {
  if (resultCount <= 0 || rect.height <= 0) return null;
  const frac = clamp((clientY - rect.top) / rect.height, 0, 1);
  return clamp(Math.floor(frac * resultCount), 0, resultCount - 1);
}

export function Minimap() {
  const status = useSession((s) => s.status);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const selectedResultIndex = useSession((s) => s.selectedResultIndex);
  const setSelectedResultIndex = useSession((s) => s.setSelectedResultIndex);
  const [data, setData] = useState<MinimapData>({ bookmarks: [], errors: [] });
  const [dragging, setDragging] = useState(false);
  const draggingRef = useRef(false);
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
          setSelectedResultIndex(next);
        }
      });
    },
    [setSelectedResultIndex],
  );

  const updateFromPointer = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      if (!resultCount) return;
      const rect = event.currentTarget.getBoundingClientRect();
      const resultIndex = pointerToResultIndex(event.clientY, rect, resultCount);
      if (resultIndex != null) {
        scheduleResultIndex(resultIndex);
      }
    },
    [resultCount, scheduleResultIndex],
  );

  const endDrag = useCallback(() => {
    draggingRef.current = false;
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
    ? Math.min(92, Math.max(0, ((selectedResultIndex ?? 0) / resultCount) * 100))
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
        draggingRef.current = true;
        setDragging(true);
        try {
          event.currentTarget.setPointerCapture(event.pointerId);
        } catch {
          // Pointer capture can fail if the pointer was already canceled; dragging still ends safely.
        }
        updateFromPointer(event);
      }}
      onPointerMove={(event) => {
        if (!draggingRef.current) return;
        event.preventDefault();
        updateFromPointer(event);
      }}
      onPointerUp={(event) => {
        updateFromPointer(event);
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
