import { describe, expect, it } from "vitest";
import { formatRowForClipboard, normalizeColumns } from "@/lib/table";
import type { Row, TableColumnConfig } from "@/types";

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
