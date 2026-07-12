import type { Row, TableColumnConfig } from "@/types";

export type ColumnId =
  "bookmark" | "lineNo" | "date" | "time" | "level" | "pid" | "tid" | "tag" | "message";

export interface ColumnDefinition {
  id: ColumnId;
  label: string;
  className: string;
  width: number;
  min: number;
  max: number;
}

export interface ColumnState extends ColumnDefinition {
  visible: boolean;
}

export const TABLE_COLUMNS: ColumnDefinition[] = [
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

export function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export function normalizeColumns(config: TableColumnConfig[]): ColumnState[] {
  const configured = new Map(config.map((column) => [column.id, column]));
  return TABLE_COLUMNS.map((definition) => {
    const column = configured.get(definition.id);
    return {
      ...definition,
      width: clamp(column?.width ?? definition.width, definition.min, definition.max),
      visible: definition.id === "message" ? true : (column?.visible ?? true),
    };
  });
}

export function gridTemplateFor(columns: ColumnState[]) {
  return columns
    .map((column) =>
      column.id === "message" ? `minmax(${column.width}px, 1fr)` : `${column.width}px`,
    )
    .join(" ");
}

export function toConfigColumns(columns: ColumnState[]): TableColumnConfig[] {
  return TABLE_COLUMNS.map((definition) => {
    const column = columns.find((item) => item.id === definition.id);
    return {
      id: definition.id,
      width: column?.width ?? definition.width,
      visible: definition.id === "message" ? true : (column?.visible ?? true),
    };
  });
}

export function cellTextForClipboard(column: ColumnState, row: Row) {
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

export function formatRowForClipboard(row: Row, columns: ColumnState[]) {
  return columns
    .map((column) => cellTextForClipboard(column, row))
    .filter((value): value is string => value != null && value.length > 0)
    .join("  ");
}
