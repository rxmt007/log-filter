import { describe, expect, it } from "vitest";
import {
  formatRowForClipboard,
  normalizeColumns,
  resolveTableDataset,
} from "@/lib/table";
import type { Row, Status, TableColumnConfig, TableScope } from "@/types";

const row: Row = {
  lineNo: 42,
  date: "04-20",
  time: "12:06:02.125",
  level: "E",
  pid: "300",
  tid: "330",
  tag: "Payment",
  message: "SocketTimeoutException",
  marked: true,
};

describe("table helpers", () => {
  it("normalizes column widths and keeps message visible", () => {
    const config: TableColumnConfig[] = [
      { id: "lineNo", width: 1, visible: true },
      { id: "message", width: 5000, visible: false },
    ];

    const columns = normalizeColumns(config);

    expect(columns.find((column) => column.id === "lineNo")?.width).toBe(52);
    expect(columns.find((column) => column.id === "message")).toMatchObject({
      width: 1200,
      visible: true,
    });
  });

  it("formats visible row cells for clipboard without bookmark glyphs", () => {
    const columns = normalizeColumns([
      { id: "bookmark", width: 24, visible: true },
      { id: "lineNo", width: 58, visible: true },
      { id: "tag", width: 154, visible: true },
      { id: "message", width: 360, visible: true },
      { id: "pid", width: 54, visible: false },
    ]).filter((column) => column.visible);

    expect(formatRowForClipboard(row, columns)).toBe(
      "42  04-20  12:06:02.125  E  330  Payment  SocketTimeoutException",
    );
  });
});

const tableStatus: Status = {
  totalLines: 1_000,
  stableLines: 999,
  filteredLines: 20,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 10_000,
  totalBytes: 20_000,
  indexing: true,
  generation: 7,
};

describe("table scope datasets", () => {
  it("keeps the existing result dataset semantics", () => {
    expect(
      resolveTableDataset(
        { kind: "results", view: "filtered" },
        tableStatus,
        7,
        3,
        5,
        11,
      ),
    ).toEqual({
      rowsView: "filtered",
      rowCount: 20,
      cacheKey: "results:7:3:5",
      minimapVisible: true,
      sourceDataRevision: 11,
    });
  });

  it("uses only stable all rows for temporary problem context", () => {
    const scope: TableScope = {
      kind: "problem-context",
      occurrence: {
        eventId: 4,
        groupId: 2,
        startLine: 480,
        endLine: 510,
        anchorLine: 490,
      },
      eventRange: { startLine: 480, endLine: 510 },
      contextRange: { startLine: 430, endLine: 560 },
      returnPoint: {
        viewportLine: 120,
        selectedLine: 125,
        filterInputRevision: 4,
      },
    };

    expect(resolveTableDataset(scope, tableStatus, 7, 3, 5, 11)).toEqual({
      rowsView: "all",
      rowCount: 999,
      cacheKey: "all:7:3",
      minimapVisible: false,
      sourceDataRevision: 11,
    });
  });

  it("does not invalidate decoded all-row history for filter or append revisions", () => {
    const scope: TableScope = {
      kind: "problem-context",
      occurrence: {
        eventId: 4,
        groupId: 2,
        startLine: 480,
        endLine: 510,
        anchorLine: 490,
      },
      eventRange: { startLine: 480, endLine: 510 },
      contextRange: { startLine: 430, endLine: 560 },
      returnPoint: {
        viewportLine: 120,
        selectedLine: null,
        filterInputRevision: 4,
      },
    };
    const before = resolveTableDataset(scope, tableStatus, 7, 3, 5, 11);
    const after = resolveTableDataset(
      scope,
      { ...tableStatus, stableLines: 1_050 },
      7,
      3,
      99,
      12,
    );
    expect(after.cacheKey).toBe(before.cacheKey);
    expect(after.rowCount).toBe(1_050);
  });
});
