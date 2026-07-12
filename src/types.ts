export interface Row {
  lineNo: number;
  date: string;
  time: string;
  level: string;
  pid: string;
  tid: string;
  tag: string;
  message: string;
  marked: boolean;
}

export interface Status {
  totalLines: number;
  filteredLines: number;
  bookmarkLines: number;
  errorLines: number;
  indexedBytes: number;
  totalBytes: number;
  indexing: boolean;
  generation: number;
}

export interface FilterField {
  enabled: boolean;
  pattern: string;
  regex: boolean;
}

export interface FilterSpec {
  levels: number;
  markedOnly: boolean;
  pid: FilterField;
  tid: FilterField;
  tagInclude: FilterField;
  tagExclude: FilterField;
  wordInclude: FilterField;
  wordExclude: FilterField;
}

export interface SearchSpec {
  query: string;
  regex: boolean;
  caseSensitive: boolean;
}

export interface SearchResult {
  count: number;
  firstLine: number | null;
}

export interface FilterDone {
  filteredLines: number;
  generation: number;
}

export interface SearchProgress {
  scanned: number;
  matches: number;
  firstLine: number | null;
  done: boolean;
  generation: number;
}

export interface ScrollRequest {
  index: number;
  align: "auto" | "center" | "start";
  reason: "minimap" | "bookmark" | "search";
  nonce: number;
}

export interface MinimapData {
  bookmarks: number[];
  errors: number[];
}

export interface NavigationTarget {
  lineNo: number;
  resultIndex: number;
}

export type RowsView = "all" | "filtered" | "bookmarks" | "errors";

export type ThemeMode = "light" | "dark";

export interface TableColumnConfig {
  id: string;
  width: number;
  visible: boolean;
}

export interface TableConfig {
  columns: TableColumnConfig[];
}

export interface AppConfig {
  theme: ThemeMode;
  adbPath: string | null;
  storageDir: string | null;
  encoding: string;
  fontSize: number;
  rowHeight: number;
  table: TableConfig;
  configPath: string;
}

export interface ExportRequest {
  mode: "view" | "range";
  view?: RowsView;
  startLine?: number;
  endLine?: number;
  path: string;
}

export interface ExportSummary {
  writtenLines: number;
  writtenBytes: number;
}

export interface SplitRequest {
  path: string;
  outDir: string;
  mode: "bytes" | "lines";
  value: number;
}

export interface SplitSummary {
  parts: string[];
  totalBytes: number;
}
