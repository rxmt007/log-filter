import { useEffect, useState } from "react";
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

export function Minimap() {
  const status = useSession((s) => s.status);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const selectedResultIndex = useSession((s) => s.selectedResultIndex);
  const setSelectedResultIndex = useSession((s) => s.setSelectedResultIndex);
  const [data, setData] = useState<MinimapData>({ bookmarks: [], errors: [] });
  const resultCount = status.filteredLines;

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

  const viewportTop = resultCount
    ? Math.min(92, Math.max(0, ((selectedResultIndex ?? 0) / resultCount) * 100))
    : 0;

  return (
    <button
      className="lf-minimap"
      type="button"
      aria-label="日志小地图"
      onClick={(event) => {
        if (!resultCount) return;
        const rect = event.currentTarget.getBoundingClientRect();
        const frac = (event.clientY - rect.top) / rect.height;
        const resultIndex = Math.min(
          resultCount - 1,
          Math.max(0, Math.floor(frac * resultCount)),
        );
        setSelectedResultIndex(resultIndex);
      }}
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
