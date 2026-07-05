import { useEffect, useState } from "react";
import { getMinimap } from "@/lib/ipc";
import { useSession } from "@/store/session";
import type { MinimapData } from "@/types";

const BUCKETS = 180;

export function Minimap() {
  const status = useSession((s) => s.status);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const selectedLine = useSession((s) => s.selectedLine);
  const setSelectedLine = useSession((s) => s.setSelectedLine);
  const setView = useSession((s) => s.setView);
  const [data, setData] = useState<MinimapData>({ bookmarks: [], errors: [] });

  useEffect(() => {
    if (!status.totalBytes) {
      setData({ bookmarks: [], errors: [] });
      return;
    }
    getMinimap(BUCKETS)
      .then(setData)
      .catch(() => setData({ bookmarks: [], errors: [] }));
  }, [status.totalBytes, status.totalLines, status.errorLines, sessionId, bookmarkRevision]);

  const viewportTop = status.totalLines
    ? Math.min(92, Math.max(0, (((selectedLine ?? 1) - 1) / status.totalLines) * 100))
    : 0;

  return (
    <button
      className="lf-minimap"
      type="button"
      aria-label="日志小地图"
      onClick={(event) => {
        if (!status.totalLines) return;
        const rect = event.currentTarget.getBoundingClientRect();
        const frac = (event.clientY - rect.top) / rect.height;
        const line = Math.min(status.totalLines, Math.max(1, Math.floor(frac * status.totalLines) + 1));
        setView("all");
        setSelectedLine(line);
      }}
    >
      {data.bookmarks.map((bucket) => (
        <span
          className="lf-minimap-tick lf-minimap-bookmark"
          key={`b-${bucket}`}
          style={{ top: `${(bucket / Math.max(1, BUCKETS - 1)) * 100}%` }}
        />
      ))}
      {data.errors.map((bucket) => (
        <span
          className="lf-minimap-tick lf-minimap-error"
          key={`e-${bucket}`}
          style={{ top: `${(bucket / Math.max(1, BUCKETS - 1)) * 100}%` }}
        />
      ))}
      <span className="lf-minimap-viewport" style={{ top: `${viewportTop}%` }} />
    </button>
  );
}
