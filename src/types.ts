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

export interface AdbDevice {
  serial: string;
  state: string;
  model: string | null;
  product: string | null;
  online: boolean;
}

export interface DeviceList {
  adbPath: string | null;
  devices: AdbDevice[];
}

export type LogcatBuffer = "main" | "system" | "radio" | "events" | "crash";

export interface StartLogcatRequest {
  deviceSerial?: string | null;
  command?: string | null;
  buffers: LogcatBuffer[];
}

export interface StreamAppend {
  appendedBytes: number;
  status: Status;
  deviceSerial: string;
}

export interface StreamControl {
  status: Status;
  running: boolean;
  paused: boolean;
  deviceSerial: string | null;
  sessionPath: string | null;
}

export interface FilterField {
  enabled: boolean;
  pattern: string;
  regex: boolean;
}

export interface HighlightRule {
  enabled: boolean;
  pattern: string;
  regex: boolean;
  caseSensitive: boolean;
  color: string;
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
  highlights: HighlightRule[];
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
  align: "auto" | "center" | "start" | "end";
  reason: "tail" | "minimap" | "bookmark" | "search" | "jump";
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

export type SourceMode = "file" | "adb";

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
  recentFiles: string[];
  lastFilter: FilterSpec | null;
  commandBuffers: LogcatBuffer[];
  currentCommand: string;
  commandPresets: string[];
  window: {
    width: number;
    height: number;
  };
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

export interface SplitProgress {
  parts: number;
  bytesProcessed: number;
}
