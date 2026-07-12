import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type PointerEvent,
} from "react";
import { getMinimap } from "@/lib/ipc";
import {
  bucketRanges,
  indexToViewportTopPx,
  MINIMAP_BUCKETS,
  pointerToResultIndex,
  rangeStyle,
  viewportHeightPx,
} from "@/lib/minimap";
import { useSession } from "@/store/session";
import type { MinimapData } from "@/types";

export function Minimap() {
  const status = useSession((s) => s.status);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const markedOnly = useSession((s) => s.filter.markedOnly);
  const rowHeight = useSession((s) => s.appConfig.rowHeight);
  const viewportResultIndex = useSession((s) => s.viewportResultIndex);
  const navigateToResultIndex = useSession((s) => s.navigateToResultIndex);
  const pauseTailFollowing = useSession((s) => s.pauseTailFollowing);
  const [data, setData] = useState<MinimapData>({ bookmarks: [], errors: [] });
  const [dragging, setDragging] = useState(false);
  const [trackHeight, setTrackHeight] = useState(0);
  const trackRef = useRef<HTMLButtonElement>(null);
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
          pauseTailFollowing("minimap");
          navigateToResultIndex(next, { align: "start", reason: "minimap" });
        }
      });
    },
    [navigateToResultIndex, pauseTailFollowing],
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
    getMinimap(MINIMAP_BUCKETS)
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

  useLayoutEffect(() => {
    const element = trackRef.current;
    if (!element) return;
    const updateHeight = () => setTrackHeight(element.getBoundingClientRect().height);
    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const viewportTop = resultCount
    ? Math.min(92, Math.max(0, (viewportResultIndex / Math.max(1, resultCount - 1)) * 92))
    : 0;
  const markedOnlyContinuous = markedOnly && resultCount > 0;
  const contentHeightPercent = trackHeight
    ? Math.min(100, Math.max(0.7, ((resultCount * rowHeight) / trackHeight) * 100))
    : 100;

  return (
    <button
      ref={trackRef}
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
      {markedOnlyContinuous ? (
        <span
          className="lf-minimap-segment lf-minimap-bookmark lf-minimap-continuous"
          style={{ top: "0%", height: `${contentHeightPercent}%` }}
        />
      ) : (
        bucketRanges(data.bookmarks).map((range) => (
          <span
            className="lf-minimap-segment lf-minimap-bookmark"
            key={`b-${range.start}-${range.end}`}
            style={rangeStyle(range)}
          />
        ))
      )}
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
