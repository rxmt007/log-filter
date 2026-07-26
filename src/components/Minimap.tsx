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
  errorTickStyle,
  indexToViewportTopPx,
  maxViewportStartIndex,
  MINIMAP_BUCKETS,
  pointerToResultIndex,
  rangeStyle,
  viewportHeightPx,
} from "@/lib/minimap";
import { useSession } from "@/store/session";
import type { MinimapData } from "@/types";

export function Minimap() {
  const status = useSession((s) => s.status);
  const tableScope = useSession((s) => s.tableScope);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const markedOnly = useSession((s) => s.filter.markedOnly);
  const rowHeight = useSession((s) => s.appConfig.rowHeight);
  const tableViewportHeight = useSession((s) => s.tableViewportHeight);
  const viewportResultIndex = useSession((s) => s.viewportResultIndex);
  const navigateToResultIndex = useSession((s) => s.navigateToResultIndex);
  const pauseTailFollowing = useSession((s) => s.pauseTailFollowing);
  const [data, setData] = useState<MinimapData>({
    bucketCount: 0,
    bookmarks: [],
    errors: [],
  });
  const [dragging, setDragging] = useState(false);
  const [trackHeight, setTrackHeight] = useState(0);
  const trackRef = useRef<HTMLButtonElement>(null);
  const draggingRef = useRef(false);
  const grabOffsetRef = useRef<number | null>(null);
  const frameRef = useRef<number | null>(null);
  const pendingIndexRef = useRef<number | null>(null);
  const throttleRef = useRef<number | null>(null);
  const requestInFlightRef = useRef(false);
  const refreshDirtyRef = useRef(false);
  const requestEligibleRef = useRef(false);
  const requestContextEpochRef = useRef(0);
  const lastRequestStartedAtRef = useRef<number | null>(null);
  const requestContext = [
    sessionId,
    status.generation,
    status.analysisGeneration,
    status.appliedFilterInputRevision,
    status.decodeRevision,
    bookmarkRevision,
    tableScope.kind,
  ].join(":");
  const requestContextRef = useRef(requestContext);
  const resultCount = status.filteredLines;
  const visibleRows =
    tableViewportHeight > 0 && rowHeight > 0
      ? Math.max(1, Math.ceil(tableViewportHeight / rowHeight))
      : 1;
  const viewportScrollable = resultCount > visibleRows;
  const maxViewportIndex = maxViewportStartIndex(resultCount, visibleRows);
  const currentViewportIndex = Math.min(maxViewportIndex, Math.max(0, viewportResultIndex));

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
      if (!resultCount || !viewportScrollable) return;
      const rect = event.currentTarget.getBoundingClientRect();
      const resultIndex = pointerToResultIndex(
        event.clientY,
        rect,
        resultCount,
        visibleRows,
        grabOffsetRef.current ?? 0,
      );
      if (resultIndex != null) {
        scheduleResultIndex(resultIndex);
      }
    },
    [resultCount, scheduleResultIndex, viewportScrollable, visibleRows],
  );

  const endDrag = useCallback(() => {
    draggingRef.current = false;
    grabOffsetRef.current = null;
    setDragging(false);
  }, []);

  const scheduleMinimapRefresh = useCallback(function scheduleRefresh() {
    if (
      !requestEligibleRef.current ||
      !refreshDirtyRef.current ||
      requestInFlightRef.current ||
      throttleRef.current != null
    ) {
      return;
    }
    const now = Date.now();
    const delay =
      lastRequestStartedAtRef.current == null
        ? 250
        : Math.max(0, 250 - (now - lastRequestStartedAtRef.current));
    throttleRef.current = window.setTimeout(() => {
      throttleRef.current = null;
      if (!requestEligibleRef.current || !refreshDirtyRef.current || requestInFlightRef.current) {
        return;
      }
      refreshDirtyRef.current = false;
      requestInFlightRef.current = true;
      lastRequestStartedAtRef.current = Date.now();
      const activeContextEpoch = requestContextEpochRef.current;
      getMinimap(MINIMAP_BUCKETS)
        .then((next) => {
          if (requestEligibleRef.current && requestContextEpochRef.current === activeContextEpoch) {
            setData(next);
          }
        })
        .catch(() => {
          if (requestEligibleRef.current && requestContextEpochRef.current === activeContextEpoch) {
            setData({ bucketCount: 0, bookmarks: [], errors: [] });
          }
        })
        .finally(() => {
          requestInFlightRef.current = false;
          scheduleRefresh();
        });
    }, delay);
  }, []);

  useEffect(() => {
    const eligible = tableScope.kind === "results" && status.totalBytes > 0;
    if (requestContextRef.current !== requestContext || requestEligibleRef.current !== eligible) {
      requestContextRef.current = requestContext;
      requestContextEpochRef.current += 1;
    }
    requestEligibleRef.current = eligible;
    if (!eligible) {
      refreshDirtyRef.current = false;
      if (throttleRef.current != null) {
        window.clearTimeout(throttleRef.current);
        throttleRef.current = null;
      }
      setData({ bucketCount: 0, bookmarks: [], errors: [] });
      return;
    }
    refreshDirtyRef.current = true;
    scheduleMinimapRefresh();
  }, [
    scheduleMinimapRefresh,
    status.totalBytes,
    status.filteredLines,
    status.errorLines,
    filterResultRevision,
    requestContext,
    tableScope.kind,
  ]);

  useEffect(
    () => () => {
      requestEligibleRef.current = false;
      refreshDirtyRef.current = false;
      requestContextEpochRef.current += 1;
      if (throttleRef.current != null) window.clearTimeout(throttleRef.current);
    },
    [],
  );

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

  const trackRect = { top: 0, height: trackHeight };
  const viewportTop = indexToViewportTopPx(
    viewportResultIndex,
    trackRect,
    resultCount,
    visibleRows,
  );
  const viewportHeight = viewportHeightPx(trackRect, resultCount, visibleRows);
  const markedOnlyContinuous = markedOnly && resultCount > 0;
  const contentHeightPercent = trackHeight
    ? Math.min(100, Math.max(0.7, ((resultCount * rowHeight) / trackHeight) * 100))
    : 100;

  if (tableScope.kind !== "results") return null;

  return (
    <button
      ref={trackRef}
      className="lf-minimap"
      data-dragging={dragging || undefined}
      type="button"
      role="scrollbar"
      aria-label="日志小地图"
      aria-controls="lf-log-table-scroll"
      aria-orientation="vertical"
      aria-valuemax={maxViewportIndex}
      aria-valuemin={0}
      aria-valuenow={currentViewportIndex}
      onKeyDown={(event) => {
        if (!viewportScrollable) return;
        let nextIndex: number | null = null;
        switch (event.key) {
          case "ArrowUp":
            nextIndex = currentViewportIndex - 1;
            break;
          case "ArrowDown":
            nextIndex = currentViewportIndex + 1;
            break;
          case "PageUp":
            nextIndex = currentViewportIndex - visibleRows;
            break;
          case "PageDown":
            nextIndex = currentViewportIndex + visibleRows;
            break;
          case "Home":
            nextIndex = 0;
            break;
          case "End":
            nextIndex = maxViewportIndex;
            break;
        }
        if (nextIndex == null) return;
        event.preventDefault();
        scheduleResultIndex(Math.min(maxViewportIndex, Math.max(0, nextIndex)));
      }}
      onPointerDown={(event) => {
        if (!resultCount || !viewportScrollable) return;
        event.preventDefault();
        const rect = event.currentTarget.getBoundingClientRect();
        const pointerY = event.clientY - rect.top;
        const viewportTopPx = indexToViewportTopPx(
          viewportResultIndex,
          rect,
          resultCount,
          visibleRows,
        );
        const insideViewport =
          pointerY >= viewportTopPx &&
          pointerY <= viewportTopPx + viewportHeightPx(rect, resultCount, visibleRows);
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
            style={rangeStyle(range, data.bucketCount)}
          />
        ))
      )}
      {data.errors.map((entry) => (
        <span
          className="lf-minimap-segment lf-minimap-error"
          key={`e-${entry.bucket}`}
          style={{ ...errorTickStyle(entry, resultCount, data.bucketCount) }}
        />
      ))}
      {resultCount > 0 && trackHeight > 0 ? (
        <span
          className="lf-minimap-viewport"
          style={{ height: `${viewportHeight}px`, top: `${viewportTop}px` }}
        />
      ) : null}
    </button>
  );
}
