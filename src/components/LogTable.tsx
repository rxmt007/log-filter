import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ClipboardEvent as ReactClipboardEvent,
  PointerEvent as ReactPointerEvent,
  ReactNode,
  WheelEvent as ReactWheelEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Bookmark, Columns3 } from "lucide-react";
import { getRows, listBookmarks, saveAppConfig, toggleBookmark } from "@/lib/ipc";
import type { AppConfig, Row, SearchSpec, TableColumnConfig } from "@/types";
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

interface ResizeState {
  columnId: ColumnId;
  startX: number;
  startWidth: number;
}

interface FilledBlock {
  count: number;
  epoch: number;
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

function toConfigColumns(columns: ColumnState[]): TableColumnConfig[] {
  return TABLE_COLUMNS.map((definition) => {
    const column = columns.find((item) => item.id === definition.id);
    return {
      id: definition.id,
      width: column?.width ?? definition.width,
      visible: definition.id === "message" ? true : column?.visible ?? true,
    };
  });
}

function cellTextForClipboard(column: ColumnState, row: Row) {
  switch (column.id) {
    case "bookmark":
      return null;
    case "lineNo":
      return String(row.lineNo);
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
      return row.message;
  }
}

function formatRowForClipboard(row: Row, columns: ColumnState[]) {
  return columns
    .map((column) => cellTextForClipboard(column, row))
    .filter((value): value is string => value != null && value.length > 0)
    .join("  ");
}

function setsEqual(left: Set<number>, right: Set<number>) {
  if (left.size !== right.size) return false;
  for (const item of left) {
    if (!right.has(item)) return false;
  }
  return true;
}

function selectionIntersectsElement(selection: Selection, element: Element) {
  for (let index = 0; index < selection.rangeCount; index += 1) {
    if (selection.getRangeAt(index).intersectsNode(element)) return true;
  }
  return false;
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
  const setAppConfig = useSession((s) => s.setAppConfig);
  const setBookmarks = useSession((s) => s.setBookmarks);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef<Map<number, Row>>(new Map());
  const filledEpoch = useRef<Map<number, FilledBlock>>(new Map());
  const inflight = useRef<Set<number>>(new Set());
  const cacheEpoch = useRef(0);
  const appConfigRef = useRef(appConfig);
  const resizeRef = useRef<ResizeState | null>(null);
  const [, force] = useState(0);
  const [columnMenuOpen, setColumnMenuOpen] = useState(false);
  const [copySelectedRows, setCopySelectedRows] = useState<Set<number>>(() => new Set());
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
    appConfigRef.current = appConfig;
  }, [appConfig]);

  const applyColumnUpdate = useCallback(
    (updater: (columns: ColumnState[]) => ColumnState[]) => {
      const currentConfig = appConfigRef.current;
      const currentColumns = normalizeColumns(currentConfig.table.columns);
      const nextColumns = updater(currentColumns).map((column) =>
        column.id === "message" ? { ...column, visible: true } : column,
      );
      const nextConfig: AppConfig = {
        ...currentConfig,
        table: { columns: toConfigColumns(nextColumns) },
      };
      appConfigRef.current = nextConfig;
      setAppConfig(nextConfig);
      return nextConfig;
    },
    [setAppConfig],
  );

  const persistConfig = useCallback(
    async (config: AppConfig) => {
      try {
        const saved = await saveAppConfig(config);
        appConfigRef.current = saved;
        setAppConfig(saved);
      } catch (err) {
        console.error("save table columns failed", err);
      }
    },
    [setAppConfig],
  );

  const startColumnResize = useCallback(
    (event: ReactPointerEvent<HTMLElement>, columnId: ColumnId) => {
      const column = columns.find((item) => item.id === columnId);
      if (!column) return;
      event.preventDefault();
      event.stopPropagation();
      resizeRef.current = {
        columnId,
        startX: event.clientX,
        startWidth: column.width,
      };
      try {
        event.currentTarget.setPointerCapture(event.pointerId);
      } catch {
        // Pointer capture may fail if the pointer is canceled; window listeners still clean up.
      }

      const finishResize = () => {
        window.removeEventListener("pointermove", resizeColumn);
        window.removeEventListener("pointerup", finishResize);
        window.removeEventListener("pointercancel", finishResize);
        resizeRef.current = null;
        void persistConfig(appConfigRef.current);
      };

      const resizeColumn = (moveEvent: PointerEvent) => {
        const resize = resizeRef.current;
        if (!resize) return;
        const definition = TABLE_COLUMNS.find((item) => item.id === resize.columnId);
        if (!definition) return;
        const width = clamp(
          resize.startWidth + moveEvent.clientX - resize.startX,
          definition.min,
          definition.max,
        );
        applyColumnUpdate((currentColumns) =>
          currentColumns.map((item) =>
            item.id === resize.columnId ? { ...item, width } : item,
          ),
        );
      };

      window.addEventListener("pointermove", resizeColumn);
      window.addEventListener("pointerup", finishResize);
      window.addEventListener("pointercancel", finishResize);
    },
    [applyColumnUpdate, columns, persistConfig],
  );

  const toggleColumnVisibility = useCallback(
    (columnId: ColumnId) => {
      if (columnId === "message") return;
      const nextConfig = applyColumnUpdate((currentColumns) =>
        currentColumns.map((column) =>
          column.id === columnId ? { ...column, visible: !column.visible } : column,
        ),
      );
      void persistConfig(nextConfig);
    },
    [applyColumnUpdate, persistConfig],
  );

  const collectRowsFromSelection = useCallback(() => {
    const root = parentRef.current;
    const selection = window.getSelection();
    if (!root || !selection || selection.rangeCount === 0 || selection.isCollapsed) return [];

    const touchesTable =
      (selection.anchorNode && root.contains(selection.anchorNode)) ||
      (selection.focusNode && root.contains(selection.focusNode));
    if (!touchesTable) return [];

    const rows: Array<{ index: number; row: Row }> = [];
    root.querySelectorAll<HTMLElement>(".lf-table-row[data-result-index]").forEach((element) => {
      if (!selectionIntersectsElement(selection, element)) return;
      const index = Number(element.dataset.resultIndex);
      if (!Number.isFinite(index)) return;
      const row = cache.current.get(index);
      if (row) rows.push({ index, row });
    });
    return rows;
  }, []);

  const refreshCopySelection = useCallback(() => {
    const indices = collectRowsFromSelection().map((item) => item.index);
    const next = new Set(indices);
    setCopySelectedRows((current) => (setsEqual(current, next) ? current : next));
  }, [collectRowsFromSelection]);

  const handleTableCopy = useCallback(
    (event: ReactClipboardEvent<HTMLDivElement>) => {
      const rows = collectRowsFromSelection();
      if (rows.length === 0) return;
      event.preventDefault();
      event.clipboardData.setData(
        "text/plain",
        rows.map(({ row }) => formatRowForClipboard(row, visibleColumns)).join("\n"),
      );
    },
    [collectRowsFromSelection, visibleColumns],
  );

  const handleTableWheel = useCallback((event: ReactWheelEvent<HTMLDivElement>) => {
    const element = event.currentTarget;
    const deltaY = event.deltaY;
    if (deltaY === 0) return;

    const maxScrollTop = element.scrollHeight - element.clientHeight;
    if (maxScrollTop <= 0) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }

    const atTop = element.scrollTop <= 0;
    const atBottom = element.scrollTop >= maxScrollTop - 1;
    if ((atTop && deltaY < 0) || (atBottom && deltaY > 0)) {
      event.preventDefault();
      event.stopPropagation();
    }
  }, []);

  useEffect(() => {
    cacheEpoch.current += 1;
    cache.current.clear();
    filledEpoch.current.clear();
    inflight.current.clear();
    parentRef.current?.scrollTo({ top: 0 });
    setViewportResultIndex(0);
    force((x) => x + 1);
  }, [sessionId, setViewportResultIndex]);

  useEffect(() => {
    cacheEpoch.current += 1;
    inflight.current.clear();
    force((x) => x + 1);
  }, [filterResultRevision]);

  useEffect(() => {
    document.addEventListener("selectionchange", refreshCopySelection);
    return () => document.removeEventListener("selectionchange", refreshCopySelection);
  }, [refreshCopySelection]);

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
      const filled = filledEpoch.current.get(block);
      if (filled?.epoch === cacheEpoch.current && filled.count >= want) return;
      if (inflight.current.has(block)) return;
      const epoch = cacheEpoch.current;
      inflight.current.add(block);
      try {
        const rows = await getRows("filtered", block, WINDOW);
        if (cacheEpoch.current !== epoch) return;
        rows.forEach((r, i) => cache.current.set(block + i, r));
        for (let i = rows.length; i < want; i += 1) {
          cache.current.delete(block + i);
        }
        filledEpoch.current.set(block, { count: rows.length, epoch });
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
  }, [filterResultRevision, items, ensureBlock, total]);

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
      <div className="lf-table-header-wrap">
        <div className="lf-table-header" style={{ gridTemplateColumns }}>
          {visibleColumns.map((column) => (
            <div className="lf-table-header-cell" key={column.id}>
              <span>{column.label}</span>
              <span
                aria-hidden="true"
                className="lf-column-resize-handle"
                onPointerDown={(event) => startColumnResize(event, column.id)}
              />
            </div>
          ))}
        </div>
        <button
          className="lf-column-menu-button"
          title="显示列"
          type="button"
          onClick={() => setColumnMenuOpen((open) => !open)}
        >
          <Columns3 />
        </button>
        {columnMenuOpen && (
          <div className="lf-column-menu">
            {TABLE_COLUMNS.map((definition) => {
              const column = columns.find((item) => item.id === definition.id);
              return (
                <label key={definition.id}>
                  <input
                    checked={column?.visible ?? true}
                    disabled={definition.id === "message"}
                    type="checkbox"
                    onChange={() => toggleColumnVisibility(definition.id)}
                  />
                  <span>{definition.label || "书签"}</span>
                </label>
              );
            })}
          </div>
        )}
      </div>
      <div
        ref={parentRef}
        className="lf-table-scroll"
        onCopy={handleTableCopy}
        onWheelCapture={handleTableWheel}
      >
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
                  data-copy-selected={copySelectedRows.has(vi.index) || undefined}
                  data-level={row?.level || ""}
                  data-marked={row?.marked || undefined}
                  data-result-index={vi.index}
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
