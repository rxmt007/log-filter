import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Bookmark } from "lucide-react";
import { getRows, listBookmarks, toggleBookmark } from "@/lib/ipc";
import type { Row } from "@/types";
import { useSession } from "@/store/session";

const WINDOW = 200;
const COLS = "22px 58px 50px 98px 40px 54px 54px 154px minmax(0,1fr)";
const HEADERS = ["", "行号", "日期", "时间", "级别", "PID", "TID", "Tag", "消息"];

function highlightText(text: string, query: string, regex: boolean, caseSensitive: boolean): ReactNode {
  if (!query) return text;
  try {
    if (regex) {
      const re = new RegExp(query, caseSensitive ? "g" : "gi");
      const out: ReactNode[] = [];
      let last = 0;
      for (const match of text.matchAll(re)) {
        const index = match.index ?? 0;
        const hit = match[0];
        if (!hit) continue;
        if (index > last) out.push(text.slice(last, index));
        out.push(
          <mark className="lf-hit" key={`${index}-${hit}`}>
            {hit}
          </mark>,
        );
        last = index + hit.length;
      }
      if (last === 0) return text;
      if (last < text.length) out.push(text.slice(last));
      return out;
    }
    const haystack = caseSensitive ? text : text.toLowerCase();
    const needle = caseSensitive ? query : query.toLowerCase();
    const index = haystack.indexOf(needle);
    if (index < 0) return text;
    return (
      <>
        {text.slice(0, index)}
        <mark className="lf-hit">{text.slice(index, index + query.length)}</mark>
        {text.slice(index + query.length)}
      </>
    );
  } catch {
    return text;
  }
}

export function LogTable() {
  const status = useSession((s) => s.status);
  const total = useSession((s) => {
    if (s.view === "filtered") return s.status.filteredLines;
    if (s.view === "bookmarks") return s.status.bookmarkLines;
    if (s.view === "errors") return s.status.errorLines;
    return s.status.totalLines;
  });
  const view = useSession((s) => s.view);
  const sessionId = useSession((s) => s.sessionId);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const rowHeight = useSession((s) => s.appConfig.rowHeight);
  const search = useSession((s) => s.search);
  const currentSearchLine = useSession((s) => s.currentSearchLine);
  const selectedLine = useSession((s) => s.selectedLine);
  const setSelectedLine = useSession((s) => s.setSelectedLine);
  const setBookmarks = useSession((s) => s.setBookmarks);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef<Map<number, Row>>(new Map());
  const filled = useRef<Map<number, number>>(new Map());
  const inflight = useRef<Set<number>>(new Set());
  const [, force] = useState(0);

  useEffect(() => {
    cache.current.clear();
    filled.current.clear();
    inflight.current.clear();
    parentRef.current?.scrollTo({ top: 0 });
    force((x) => x + 1);
  }, [sessionId, view, bookmarkRevision, filterResultRevision]);

  const rv = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 24,
  });

  useEffect(() => {
    if (!currentSearchLine || view !== "all") return;
    rv.scrollToIndex(Math.max(0, currentSearchLine - 1), { align: "center" });
  }, [currentSearchLine, rv, view]);

  useEffect(() => {
    if (!selectedLine || view !== "all") return;
    rv.scrollToIndex(Math.max(0, selectedLine - 1), { align: "center" });
  }, [selectedLine, rv, view]);

  const items = rv.getVirtualItems();

  const ensureBlock = useCallback(
    async (block: number, totalNow: number) => {
      const want = Math.min(WINDOW, totalNow - block);
      if (want <= 0) return;
      if ((filled.current.get(block) ?? 0) >= want) return;
      if (inflight.current.has(block)) return;
      inflight.current.add(block);
      try {
        const rows = await getRows(view, block, WINDOW);
        rows.forEach((r, i) => cache.current.set(block + i, r));
        filled.current.set(block, rows.length);
        force((x) => x + 1);
      } finally {
        inflight.current.delete(block);
      }
    },
    [view],
  );

  useEffect(() => {
    if (items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    ensureBlock(Math.floor(first / WINDOW) * WINDOW, total);
    ensureBlock(Math.floor(last / WINDOW) * WINDOW, total);
  }, [items, ensureBlock, total]);

  const emptyText = useMemo(() => {
    if (!status.totalBytes) return "打开或拖入 logcat 文件后开始浏览";
    if (view === "filtered") return "当前过滤条件没有命中行";
    if (view === "bookmarks") return "还没有书签";
    if (view === "errors") return "当前日志没有错误或致命行";
    return "正在等待索引行";
  }, [status.totalBytes, view]);

  const toggleRowBookmark = useCallback(
    async (row: Row) => {
      await toggleBookmark(row.lineNo);
      const bookmarks = await listBookmarks();
      setBookmarks(bookmarks);
      force((x) => x + 1);
    },
    [setBookmarks],
  );

  return (
    <div className="lf-table-shell">
      <div className="lf-table-header" style={{ gridTemplateColumns: COLS }}>
        {HEADERS.map((h, i) => (
          <div key={`${h}-${i}`}>{h}</div>
        ))}
      </div>
      <div ref={parentRef} className="lf-table-scroll">
        {total === 0 ? (
          <div className="lf-empty-state">{emptyText}</div>
        ) : (
          <div style={{ height: rv.getTotalSize(), position: "relative" }}>
            {items.map((vi) => {
              const row = cache.current.get(vi.index);
              const selected = row?.lineNo === selectedLine || row?.lineNo === currentSearchLine;
              return (
                <div
                  className="lf-table-row"
                  data-level={row?.level || ""}
                  data-selected={selected || undefined}
                  key={vi.key}
                  onClick={() => row && setSelectedLine(row.lineNo)}
                  onDoubleClick={() => row && toggleRowBookmark(row)}
                  style={{
                    gridTemplateColumns: COLS,
                    height: rowHeight,
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  {row ? (
                    <>
                      <span className="lf-bookmark-cell">
                        {row.marked && <Bookmark />}
                      </span>
                      <span className="lf-num">{row.lineNo}</span>
                      <span className="lf-meta">{row.date}</span>
                      <span className="lf-meta">{row.time}</span>
                      <span className="lf-level">{row.level}</span>
                      <span className="lf-num">{row.pid}</span>
                      <span className="lf-num">{row.tid}</span>
                      <span className="lf-tag" title={row.tag}>
                        {row.tag}
                      </span>
                      <span className="lf-message">
                        {highlightText(row.message, search.query, search.regex, search.caseSensitive)}
                      </span>
                    </>
                  ) : (
                    <span className="lf-loading-row">...</span>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
