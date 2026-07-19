import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ClipboardEvent as ReactClipboardEvent,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  WheelEvent as ReactWheelEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Bookmark, ChevronsDown, Columns3 } from "lucide-react";
import { splitHighlightTokens } from "@/lib/highlight";
import { getRows, listBookmarks, saveAppConfig, toggleBookmark } from "@/lib/ipc";
import { RowBlockCache } from "@/lib/rowCache";
import {
  clamp,
  formatRowForClipboard,
  gridTemplateFor,
  normalizeColumns,
  TABLE_COLUMNS,
  toConfigColumns,
  type ColumnId,
  type ColumnState,
} from "@/lib/table";
import type { AppConfig, FilterSpec, Row, SearchSpec } from "@/types";
import { ALL_LEVELS, useSession } from "@/store/session";

const WINDOW = 200;

interface ResizeState {
  columnId: ColumnId;
  startX: number;
  startWidth: number;
}

interface SelectionRange {
  start: number;
  end: number;
}

interface BookmarkMenuState {
  x: number;
  y: number;
  range: SelectionRange;
}

function normalizeSelectionRange(start: number, end: number): SelectionRange {
  return start <= end ? { start, end } : { start: end, end: start };
}

function selectionRangeEqual(left: SelectionRange | null, right: SelectionRange | null) {
  if (left === right) return true;
  if (!left || !right) return false;
  return left.start === right.start && left.end === right.end;
}

function selectionIntersectsElement(selection: Selection, element: Element) {
  for (let index = 0; index < selection.rangeCount; index += 1) {
    if (selection.getRangeAt(index).intersectsNode(element)) return true;
  }
  return false;
}

function highlightText(
  text: string,
  query: string,
  regex: boolean,
  caseSensitive: boolean,
  highlights: FilterSpec["highlights"],
) {
  return splitHighlightTokens(text, {
    highlights: highlights
      .filter((rule) => rule.enabled && rule.pattern.trim())
      .map((rule) => ({
        query: rule.pattern,
        regex: rule.regex,
        caseSensitive: rule.caseSensitive,
        color: rule.color,
      })),
    search: { query, regex, caseSensitive },
  }).map((token, index) => {
    if (token.kind === "search") {
      return (
        <mark className="lf-hit" key={`${index}-${token.text}`}>
          {token.text}
        </mark>
      );
    }
    if (token.kind === "highlight") {
      return (
        <mark className="lf-keyword-hit" data-color={token.color} key={`${index}-${token.text}`}>
          {token.text}
        </mark>
      );
    }
    return token.text;
  });
}

function fieldActive(field: { enabled: boolean; pattern: string }) {
  return field.enabled && field.pattern.trim().length > 0;
}

function renderCell(
  column: ColumnState,
  row: Row,
  search: SearchSpec,
  highlights: FilterSpec["highlights"],
) {
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
      return highlightText(
        row.message,
        search.query,
        search.regex,
        search.caseSensitive,
        highlights,
      );
  }
}

export function LogTable() {
  const status = useSession((s) => s.status);
  const total = useSession((s) => s.status.filteredLines);
  const sourceMode = useSession((s) => s.sourceMode);
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
  const setTailFollowingFromViewport = useSession((s) => s.setTailFollowingFromViewport);
  const pauseTailFollowing = useSession((s) => s.pauseTailFollowing);
  const streamRunning = useSession((s) => s.streamRunning);
  const tailFollowing = useSession((s) => s.tailFollowing);
  const setTailFollowing = useSession((s) => s.setTailFollowing);
  const requestTailFollow = useSession((s) => s.requestTailFollow);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const setBookmarks = useSession((s) => s.setBookmarks);
  const parentRef = useRef<HTMLDivElement>(null);
  const programmaticScrollRef = useRef(false);
  const programmaticScrollTimerRef = useRef<number | null>(null);
  const cache = useRef(new RowBlockCache(64));
  const inflight = useRef<Map<number, Promise<void>>>(new Map());
  const cacheEpoch = useRef(0);
  const appConfigRef = useRef(appConfig);
  const resizeRef = useRef<ResizeState | null>(null);
  const [, force] = useState(0);
  const [columnMenuOpen, setColumnMenuOpen] = useState(false);
  const [selectionRange, setSelectionRange] = useState<SelectionRange | null>(null);
  const [bookmarkMenu, setBookmarkMenu] = useState<BookmarkMenuState | null>(null);
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
  const columns = useMemo(
    () => normalizeColumns(appConfig.table.columns),
    [appConfig.table.columns],
  );
  const visibleColumns = useMemo(() => columns.filter((column) => column.visible), [columns]);
  const gridTemplateColumns = useMemo(() => gridTemplateFor(visibleColumns), [visibleColumns]);

  useEffect(() => {
    appConfigRef.current = appConfig;
  }, [appConfig]);

  useEffect(
    () => () => {
      if (programmaticScrollTimerRef.current != null) {
        window.clearTimeout(programmaticScrollTimerRef.current);
      }
    },
    [],
  );

  const markProgrammaticScroll = useCallback(() => {
    programmaticScrollRef.current = true;
    if (programmaticScrollTimerRef.current != null) {
      window.clearTimeout(programmaticScrollTimerRef.current);
    }
    programmaticScrollTimerRef.current = window.setTimeout(() => {
      programmaticScrollRef.current = false;
      programmaticScrollTimerRef.current = null;
    }, 160);
  }, []);

  const clearProgrammaticScroll = useCallback(() => {
    programmaticScrollRef.current = false;
    if (programmaticScrollTimerRef.current != null) {
      window.clearTimeout(programmaticScrollTimerRef.current);
      programmaticScrollTimerRef.current = null;
    }
  }, []);

  const syncTailFollowingFromScroll = useCallback(() => {
    const element = parentRef.current;
    if (!element) return;
    const maxScrollTop = Math.max(0, element.scrollHeight - element.clientHeight);
    const isAtBottom = maxScrollTop <= 1 || element.scrollTop >= maxScrollTop - 2;
    setTailFollowingFromViewport(isAtBottom, programmaticScrollRef.current ? "program" : "user");
  }, [setTailFollowingFromViewport]);

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
          currentColumns.map((item) => (item.id === resize.columnId ? { ...item, width } : item)),
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
      const row = cache.current.get(index, WINDOW);
      if (row) rows.push({ index, row });
    });
    return rows;
  }, []);

  const collectRowsInRange = useCallback(
    (range: SelectionRange | null = selectionRange) => {
      if (!range) return [];
      const rows: Array<{ index: number; row: Row }> = [];
      for (let index = range.start; index <= range.end; index += 1) {
        const row = cache.current.get(index, WINDOW);
        if (row) rows.push({ index, row });
      }
      return rows;
    },
    [selectionRange],
  );

  const refreshCopySelection = useCallback(() => {
    const indices = collectRowsFromSelection().map((item) => item.index);
    if (indices.length === 0) {
      setSelectionRange((current) => (current == null ? current : null));
      return;
    }
    const next = normalizeSelectionRange(Math.min(...indices), Math.max(...indices));
    setSelectionRange((current) => (selectionRangeEqual(current, next) ? current : next));
  }, [collectRowsFromSelection]);

  const handleTableCopy = useCallback(
    (event: ReactClipboardEvent<HTMLDivElement>) => {
      const rows = collectRowsInRange();
      if (rows.length === 0) return;
      event.preventDefault();
      event.clipboardData.setData(
        "text/plain",
        rows.map(({ row }) => formatRowForClipboard(row, visibleColumns)).join("\n"),
      );
    },
    [collectRowsInRange, visibleColumns],
  );

  const handleTableWheel = useCallback(
    (event: ReactWheelEvent<HTMLDivElement>) => {
      const element = event.currentTarget;
      const deltaY = event.deltaY;
      if (deltaY === 0) return;

      clearProgrammaticScroll();
      if (deltaY < 0) {
        pauseTailFollowing("scroll");
      }

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
    },
    [clearProgrammaticScroll, pauseTailFollowing],
  );

  useEffect(() => {
    cacheEpoch.current += 1;
    cache.current.clear();
    inflight.current.clear();
    parentRef.current?.scrollTo({ top: 0 });
    setBookmarkMenu(null);
    setSelectionRange(null);
    setViewportResultIndex(0);
    force((x) => x + 1);
  }, [sessionId, setViewportResultIndex]);

  useEffect(() => {
    cacheEpoch.current += 1;
    inflight.current.clear();
    setBookmarkMenu(null);
    setSelectionRange(null);
    force((x) => x + 1);
  }, [filterResultRevision]);

  useEffect(() => {
    document.addEventListener("selectionchange", refreshCopySelection);
    return () => document.removeEventListener("selectionchange", refreshCopySelection);
  }, [refreshCopySelection]);

  useEffect(() => {
    if (!bookmarkMenu) return;
    const closeMenu = () => setBookmarkMenu(null);
    const closeMenuOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenu();
    };
    window.addEventListener("click", closeMenu);
    window.addEventListener("keydown", closeMenuOnEscape);
    return () => {
      window.removeEventListener("click", closeMenu);
      window.removeEventListener("keydown", closeMenuOnEscape);
    };
  }, [bookmarkMenu]);

  const ensureBlock = useCallback(async (block: number, totalNow: number) => {
    const want = Math.min(WINDOW, totalNow - block);
    if (want <= 0) return;
    if (cache.current.isFresh(block, want, cacheEpoch.current)) return;
    const existing = inflight.current.get(block);
    if (existing) return existing;
    const epoch = cacheEpoch.current;
    const load = (async () => {
      try {
        const rows = await getRows("filtered", block, WINDOW);
        if (cacheEpoch.current !== epoch) return;
        cache.current.fill(block, rows, epoch);
        force((x) => x + 1);
      } finally {
        inflight.current.delete(block);
      }
    })();
    inflight.current.set(block, load);
    return load;
  }, []);

  const rv = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 24,
  });

  useEffect(() => {
    if (!currentSearchLine || !defaultResultOrder) return;
    markProgrammaticScroll();
    rv.scrollToIndex(Math.max(0, currentSearchLine - 1), { align: "center" });
  }, [currentSearchLine, defaultResultOrder, markProgrammaticScroll, rv]);

  useEffect(() => {
    if (!scrollRequest || !total) return;
    let canceled = false;
    const scroll = () => {
      if (canceled) return;
      const current = useSession.getState().scrollRequest;
      if (current?.nonce !== scrollRequest.nonce) return;
      markProgrammaticScroll();
      rv.scrollToIndex(Math.max(0, scrollRequest.index), { align: scrollRequest.align });
    };
    if (scrollRequest.reason === "tail") {
      const block = Math.floor(scrollRequest.index / WINDOW) * WINDOW;
      void ensureBlock(block, total).finally(scroll);
    } else {
      scroll();
    }
    return () => {
      canceled = true;
    };
  }, [ensureBlock, markProgrammaticScroll, rv, scrollRequest, total]);

  const items = rv.getVirtualItems();
  const firstVisibleIndex = items[0]?.index ?? null;

  useEffect(() => {
    if (firstVisibleIndex == null) return;
    setViewportResultIndex(firstVisibleIndex);
  }, [firstVisibleIndex, setViewportResultIndex]);

  useEffect(() => {
    if (items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    ensureBlock(Math.floor(first / WINDOW) * WINDOW, total);
    ensureBlock(Math.floor(last / WINDOW) * WINDOW, total);
  }, [filterResultRevision, items, ensureBlock, total]);

  const emptyText = useMemo(() => {
    if (!status.totalBytes) return "从设备抓取或打开 logcat 文件后开始浏览";
    return "当前结果没有命中行";
  }, [status.totalBytes]);

  const resumeTailFollow = useCallback(() => {
    const count = useSession.getState().status.filteredLines;
    if (count <= 0) return;
    setTailFollowing(true);
    requestTailFollow(count - 1);
  }, [requestTailFollow, setTailFollowing]);

  const toggleRowBookmark = useCallback(
    async (row: Row) => {
      const marked = await toggleBookmark(row.lineNo);
      cache.current.updateRows((cached) =>
        cached.lineNo === row.lineNo ? { ...cached, marked } : cached,
      );
      const bookmarks = await listBookmarks();
      setBookmarks(bookmarks);
      force((x) => x + 1);
    },
    [setBookmarks],
  );

  const openBookmarkMenu = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>, index: number) => {
      const row = cache.current.get(index, WINDOW);
      if (!row) return;
      event.preventDefault();
      const range =
        selectionRange && index >= selectionRange.start && index <= selectionRange.end
          ? selectionRange
          : { start: index, end: index };
      setSelectionRange((current) => (selectionRangeEqual(current, range) ? current : range));
      setBookmarkMenu({ x: event.clientX, y: event.clientY, range });
      pauseTailFollowing("row");
      selectRow(row.lineNo, index);
    },
    [pauseTailFollowing, selectRow, selectionRange],
  );

  const applyBookmarkRange = useCallback(
    async (targetMarked: boolean) => {
      if (!bookmarkMenu) return;
      const rows = collectRowsInRange(bookmarkMenu.range);
      const touchedLines = new Set<number>();
      for (const { row } of rows) {
        if (touchedLines.has(row.lineNo) || row.marked === targetMarked) continue;
        touchedLines.add(row.lineNo);
        const marked = await toggleBookmark(row.lineNo);
        cache.current.updateRows((cached) =>
          cached.lineNo === row.lineNo ? { ...cached, marked } : cached,
        );
      }
      const bookmarks = await listBookmarks();
      setBookmarks(bookmarks);
      setBookmarkMenu(null);
      force((x) => x + 1);
    },
    [bookmarkMenu, collectRowsInRange, setBookmarks],
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
          aria-label="Show columns"
          className="lf-column-menu-button"
          data-tooltip="Show columns"
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
        onPointerDownCapture={clearProgrammaticScroll}
        onScroll={syncTailFollowingFromScroll}
        onWheelCapture={handleTableWheel}
      >
        {total === 0 ? (
          <div className="lf-empty-state">
            <div className="lf-empty-icon">
              <Columns3 />
            </div>
            <div className="lf-empty-title">
              {status.totalBytes
                ? "当前结果没有命中行"
                : sourceMode === "adb"
                  ? "开始抓取 logcat"
                  : "打开 logcat 日志文件"}
            </div>
            <div className="lf-empty-copy">{emptyText}</div>
            {!status.totalBytes && (
              <div className="lf-empty-actions">
                <button
                  type="button"
                  onClick={() => window.dispatchEvent(new Event("lf:start-capture"))}
                >
                  从设备抓取
                </button>
                <button
                  type="button"
                  onClick={() => window.dispatchEvent(new Event("lf:open-file"))}
                >
                  打开文件
                </button>
              </div>
            )}
          </div>
        ) : (
          <div style={{ height: rv.getTotalSize(), position: "relative" }}>
            {items.map((vi) => {
              const row = cache.current.get(vi.index, WINDOW);
              const selected =
                vi.index === selectedResultIndex ||
                row?.lineNo === selectedLine ||
                row?.lineNo === currentSearchLine;
              return (
                <div
                  className="lf-table-row"
                  data-copy-selected={
                    selectionRange &&
                    vi.index >= selectionRange.start &&
                    vi.index <= selectionRange.end
                      ? true
                      : undefined
                  }
                  data-level={row?.level || ""}
                  data-marked={row?.marked || undefined}
                  data-result-index={vi.index}
                  data-selected={selected || undefined}
                  key={vi.key}
                  onClick={() => {
                    if (!row) return;
                    pauseTailFollowing("row");
                    selectRow(row.lineNo, vi.index);
                  }}
                  onDoubleClick={() => row && toggleRowBookmark(row)}
                  onContextMenu={(event) => openBookmarkMenu(event, vi.index)}
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
                        {renderCell(column, row, search, filter.highlights)}
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
      {sourceMode === "adb" && streamRunning && !tailFollowing && total > 0 && (
        <button
          className="lf-follow-tail"
          type="button"
          title="回到底部并继续追踪最新日志"
          onClick={resumeTailFollow}
        >
          <ChevronsDown />
          追最新
        </button>
      )}
      {bookmarkMenu && (
        <div
          className="lf-table-context-menu"
          style={{ left: bookmarkMenu.x, top: bookmarkMenu.y }}
          onClick={(event) => event.stopPropagation()}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <button type="button" onClick={() => applyBookmarkRange(true)}>
            标记选中行
          </button>
          <button type="button" onClick={() => applyBookmarkRange(false)}>
            取消标记
          </button>
        </div>
      )}
    </div>
  );
}
