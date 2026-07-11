import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Bookmark } from "lucide-react";
import { getRows, listBookmarks, toggleBookmark } from "@/lib/ipc";
import type { Row, SearchSpec, TableColumnConfig } from "@/types";
import { ALL_LEVELS, useSession } from "@/store/session";

const WINDOW = 200;

type ColumnId =
  | "bookmark"
  | "lineNo"
  | "date"
  | "time"
  | "level"
  | "pid"
  | "tid"
  | "tag"
  | "message";

interface ColumnDefinition {
  id: ColumnId;
  label: string;
  className: string;
  width: number;
  min: number;
  max: number;
}

interface ColumnState extends ColumnDefinition {
  visible: boolean;
}

const TABLE_COLUMNS: ColumnDefinition[] = [
  { id: "bookmark", label: "", className: "lf-bookmark-cell", width: 24, min: 22, max: 36 },
  { id: "lineNo", label: "行号", className: "lf-num", width: 58, min: 52, max: 120 },
  { id: "date", label: "日期", className: "lf-meta", width: 50, min: 48, max: 90 },
  { id: "time", label: "时间", className: "lf-meta", width: 98, min: 82, max: 160 },
  { id: "level", label: "级别", className: "lf-level", width: 40, min: 36, max: 60 },
  { id: "pid", label: "PID", className: "lf-num", width: 54, min: 48, max: 100 },
  { id: "tid", label: "TID", className: "lf-num", width: 54, min: 48, max: 100 },
  { id: "tag", label: "Tag", className: "lf-tag", width: 154, min: 110, max: 260 },
  { id: "message", label: "消息", className: "lf-message", width: 360, min: 220, max: 1200 },
];

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function normalizeColumns(config: TableColumnConfig[]): ColumnState[] {
  const configured = new Map(config.map((column) => [column.id, column]));
  return TABLE_COLUMNS.map((definition) => {
    const column = configured.get(definition.id);
    return {
      ...definition,
      width: clamp(column?.width ?? definition.width, definition.min, definition.max),
      visible: definition.id === "message" ? true : column?.visible ?? true,
    };
  });
}

function gridTemplateFor(columns: ColumnState[]) {
  return columns
    .map((column) =>
      column.id === "message" ? `minmax(${column.width}px, 1fr)` : `${column.width}px`,
    )
    .join(" ");
}

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

function fieldActive(field: { enabled: boolean; pattern: string }) {
  return field.enabled && field.pattern.trim().length > 0;
}

function renderCell(column: ColumnState, row: Row, search: SearchSpec) {
  switch (column.id) {
    case "bookmark":
      return row.marked ? <Bookmark /> : null;
    case "lineNo":
      return row.lineNo;
    case "date":
      return row.date;
    case "time":
      return row.time;
    case "level":
      return row.level;
    case "pid":
      return row.pid;
    case "tid":
      return row.tid;
    case "tag":
      return row.tag;
    case "message":
      return highlightText(row.message, search.query, search.regex, search.caseSensitive);
  }
}

export function LogTable() {
  const status = useSession((s) => s.status);
  const total = useSession((s) => s.status.filteredLines);
  const filter = useSession((s) => s.filter);
  const sessionId = useSession((s) => s.sessionId);
  const filterResultRevision = useSession((s) => s.filterResultRevision);
  const appConfig = useSession((s) => s.appConfig);
  const search = useSession((s) => s.search);
  const currentSearchLine = useSession((s) => s.currentSearchLine);
  const selectedLine = useSession((s) => s.selectedLine);
  const selectedResultIndex = useSession((s) => s.selectedResultIndex);
  const scrollRequest = useSession((s) => s.scrollRequest);
  const selectRow = useSession((s) => s.selectRow);
  const setViewportResultIndex = useSession((s) => s.setViewportResultIndex);
  const setBookmarks = useSession((s) => s.setBookmarks);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef<Map<number, Row>>(new Map());
  const filled = useRef<Map<number, number>>(new Map());
  const inflight = useRef<Set<number>>(new Set());
  const cacheEpoch = useRef(0);
  const [, force] = useState(0);
  const rowHeight = appConfig.rowHeight;
  const defaultResultOrder =
    filter.levels === ALL_LEVELS &&
    !filter.markedOnly &&
    !fieldActive(filter.pid) &&
    !fieldActive(filter.tid) &&
    !fieldActive(filter.tagInclude) &&
    !fieldActive(filter.tagExclude) &&
    !fieldActive(filter.wordInclude) &&
    !fieldActive(filter.wordExclude);
  const columns = useMemo(() => normalizeColumns(appConfig.table.columns), [appConfig.table.columns]);
  const visibleColumns = useMemo(() => columns.filter((column) => column.visible), [columns]);
  const gridTemplateColumns = useMemo(() => gridTemplateFor(visibleColumns), [visibleColumns]);

  useEffect(() => {
    cacheEpoch.current += 1;
    cache.current.clear();
    filled.current.clear();
    inflight.current.clear();
    parentRef.current?.scrollTo({ top: 0 });
    setViewportResultIndex(0);
    force((x) => x + 1);
  }, [sessionId, setViewportResultIndex]);

  useEffect(() => {
    cacheEpoch.current += 1;
    cache.current.clear();
    filled.current.clear();
    inflight.current.clear();
    force((x) => x + 1);
  }, [filterResultRevision]);

  const rv = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 24,
  });

  useEffect(() => {
    if (!currentSearchLine || !defaultResultOrder) return;
    rv.scrollToIndex(Math.max(0, currentSearchLine - 1), { align: "center" });
  }, [currentSearchLine, defaultResultOrder, rv]);

  useEffect(() => {
    if (!scrollRequest || !total) return;
    rv.scrollToIndex(Math.max(0, scrollRequest.index), { align: scrollRequest.align });
  }, [rv, scrollRequest, total]);

  const items = rv.getVirtualItems();
  const firstVisibleIndex = items[0]?.index ?? null;

  useEffect(() => {
    if (firstVisibleIndex == null) return;
    setViewportResultIndex(firstVisibleIndex);
  }, [firstVisibleIndex, setViewportResultIndex]);

  const ensureBlock = useCallback(
    async (block: number, totalNow: number) => {
      const want = Math.min(WINDOW, totalNow - block);
      if (want <= 0) return;
      if ((filled.current.get(block) ?? 0) >= want) return;
      if (inflight.current.has(block)) return;
      const epoch = cacheEpoch.current;
      inflight.current.add(block);
      try {
        const rows = await getRows("filtered", block, WINDOW);
        if (cacheEpoch.current !== epoch) return;
        rows.forEach((r, i) => cache.current.set(block + i, r));
        filled.current.set(block, rows.length);
        force((x) => x + 1);
      } finally {
        inflight.current.delete(block);
      }
    },
    [],
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
    return "当前结果没有命中行";
  }, [status.totalBytes]);

  const toggleRowBookmark = useCallback(
    async (row: Row) => {
      const marked = await toggleBookmark(row.lineNo);
      cache.current.forEach((cached, index) => {
        if (cached.lineNo === row.lineNo) {
          cache.current.set(index, { ...cached, marked });
        }
      });
      const bookmarks = await listBookmarks();
      setBookmarks(bookmarks);
      force((x) => x + 1);
    },
    [setBookmarks],
  );

  return (
    <div className="lf-table-shell">
      <div className="lf-table-header" style={{ gridTemplateColumns }}>
        {visibleColumns.map((column) => (
          <div className="lf-table-header-cell" key={column.id}>
            {column.label}
          </div>
        ))}
      </div>
      <div ref={parentRef} className="lf-table-scroll">
        {total === 0 ? (
          <div className="lf-empty-state">{emptyText}</div>
        ) : (
          <div style={{ height: rv.getTotalSize(), position: "relative" }}>
            {items.map((vi) => {
              const row = cache.current.get(vi.index);
              const selected =
                vi.index === selectedResultIndex ||
                row?.lineNo === selectedLine ||
                row?.lineNo === currentSearchLine;
              return (
                <div
                  className="lf-table-row"
                  data-level={row?.level || ""}
                  data-selected={selected || undefined}
                  key={vi.key}
                  onClick={() => {
                    if (!row) return;
                    selectRow(row.lineNo, vi.index);
                  }}
                  onDoubleClick={() => row && toggleRowBookmark(row)}
                  style={{
                    gridTemplateColumns,
                    height: rowHeight,
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  {row ? (
                    visibleColumns.map((column) => (
                      <span
                        className={column.className}
                        key={column.id}
                        title={column.id === "tag" ? row.tag : undefined}
                      >
                        {renderCell(column, row, search)}
                      </span>
                    ))
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
