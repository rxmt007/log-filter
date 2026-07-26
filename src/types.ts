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
  stableLines: number;
  filteredLines: number;
  bookmarkLines: number;
  errorLines: number;
  indexedBytes: number;
  totalBytes: number;
  indexing: boolean;
  generation: number;
  analysisGeneration: number;
  filterInputRevision: number;
  appliedFilterInputRevision: number;
  filterResultRevision: number;
  decodeRevision: number;
  sourceDataRevision: number;
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

export type StreamLifecycle =
  | "stopped"
  | "starting"
  | "running"
  | "pausing"
  | "paused"
  | "finishing"
  | "control-error";

export interface StreamControl {
  status: Status;
  lifecycle: StreamLifecycle;
  running: boolean;
  paused: boolean;
  error: string | null;
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
  requestId: number;
}

export interface FilterDone {
  filteredLines: number;
  generation: number;
  filterInputRevision: number;
  filterResultRevision: number;
}

export interface SearchProgress {
  scanned: number;
  matches: number;
  firstLine: number | null;
  done: boolean;
  generation: number;
  requestId: number;
}

export interface ScrollRequest {
  index: number;
  align: "auto" | "center" | "start" | "end";
  reason: "tail" | "minimap" | "bookmark" | "search" | "jump";
  nonce: number;
}

export interface MinimapData {
  bookmarks: number[];
  errors: Array<{ bucket: number; count: number }>;
}

export interface NavigationTarget {
  lineNo: number;
  resultIndex: number;
}

export type ProblemKind =
  | "java-crash"
  | "java-oom"
  | "anr"
  | "native-crash"
  | "process-restart"
  | "signal-exit"
  | "lmk-kill"
  | "kernel-oom-kill";

export interface AnalysisToken {
  sessionGeneration: number;
  analysisGeneration: number;
}

export interface ProblemStats {
  observedOccurrenceCount: number;
  storedOccurrenceCount: number;
  droppedOccurrenceCount: number;
  provisionalOccurrenceCount: number;
  storedGroupCount: number;
  ungroupedDroppedOccurrenceCount: number;
  droppedRecentObservationCount: number;
  revision: number;
  limited: boolean;
  correlationLimited: boolean;
}

export type CaptureOrigin = "static-file" | "adb-live";

export type RangeCompleteness = "unknown" | "bounded" | "start-truncated" | "end-truncated";

export type ProblemInputBuffer = "main" | "system" | "events" | "crash" | "radio" | "kernel";

export interface InputCoverage {
  origin: CaptureOrigin;
  requestedBuffers: ProblemInputBuffer[] | null;
  rangeCompleteness: RangeCompleteness;
}

export interface ProblemsStatus {
  analysisToken: AnalysisToken;
  scannedLines: number;
  stableLines: number;
  scanning: boolean;
  finished: boolean;
  coverage: InputCoverage;
  stats: ProblemStats;
}

export interface ProblemsProgress {
  scannedLines: number;
  stableLines: number;
  coverage: InputCoverage;
  observedOccurrenceCount: number;
  storedOccurrenceCount: number;
  droppedOccurrenceCount: number;
  provisionalOccurrenceCount: number;
  storedGroupCount: number;
  ungroupedDroppedOccurrenceCount: number;
  droppedRecentObservationCount: number;
  correlationLimited: boolean;
  revision: number;
  done: boolean;
  limited: boolean;
  sessionGeneration: number;
  analysisGeneration: number;
}

export interface ProblemGroup {
  id: number;
  kind: ProblemKind;
  fingerprintVersion: number;
  signatureQuality: string;
  identityQuality: string;
  processSummary: string;
  processSummaryTruncated: boolean;
  signatureSummary: string;
  signatureSummaryTruncated: boolean;
  fingerprint: string;
  observedOccurrenceCount: number;
  storedOccurrenceCount: number;
  droppedOccurrenceCount: number;
  firstLine: number;
  firstTimestamp: string | null;
  lastLine: number;
  lastTimestamp: string | null;
  firstEventId: number | null;
  lastEventId: number | null;
  representativeEventId: number | null;
}

export interface ProblemOccurrence extends ProblemOccurrenceRef {
  kind: ProblemKind;
  pid: number | null;
  timestamp: string | null;
  processInstanceId: number;
  evidenceFlags: ProblemEvidenceFlag[];
  outcomeFlags: ProblemOutcomeFlag[];
  boundaryFlags: ProblemBoundaryFlag[];
}

export type ProblemEvidenceFlag = "primary" | "structured" | "multiline" | "correlated";

export type ProblemOutcomeFlag =
  | "kill-requested"
  | "kill-issued"
  | "death-observed"
  | "start-after-death-observed"
  | "explicitly-recoverable"
  | "conflict";

export type ProblemBoundaryFlag =
  | "truncated-by-input"
  | "truncated-by-limit"
  | "observation-refs-truncated"
  | "observation-count-limited"
  | "line-index-overflow"
  | "correlation-limited";

export type ProblemObservationRole =
  | "primary"
  | "process-identity"
  | "exception-type"
  | "stack-frame"
  | "reason"
  | "signal"
  | "backtrace-frame"
  | "start"
  | "death"
  | "restart"
  | "kill-request"
  | "kill-issued"
  | "supporting"
  | "recovery";

export type ProblemEvidenceFormat =
  | "aosp-text"
  | "event-log-shaped-text"
  | "tombstone-shaped-text"
  | "kernel-shaped-text";

export type ProblemLineProvenance =
  | "unknown"
  | "inferred-main"
  | "inferred-system"
  | "inferred-events"
  | "inferred-crash"
  | "inferred-radio"
  | "inferred-kernel"
  | "known-main"
  | "known-system"
  | "known-events"
  | "known-crash"
  | "known-radio"
  | "known-kernel";

export interface ProblemPage<T> {
  analysisToken: AnalysisToken;
  snapshotHandle: string;
  revision: number;
  total: number;
  items: T[];
  nextCursor: string | null;
}

export type ProblemFactCode =
  | "java-uncaught-exception"
  | "java-out-of-memory-error"
  | "managed-crash-record"
  | "anr-detected"
  | "native-crash-detected"
  | "signal-exit-detected"
  | "process-started"
  | "process-died"
  | "process-restarted"
  | "lmk-kill-issued"
  | "kernel-oom-kill-issued"
  | "kill-requested"
  | "process-identity-recorded"
  | "exception-type-recorded"
  | "stack-frame-recorded"
  | "anr-reason-recorded"
  | "fatal-signal-recorded"
  | "native-frame-recorded"
  | "process-death-observed"
  | "start-after-death-observed"
  | "native-recovery-recorded"
  | "supporting-evidence-recorded";

export interface ProblemFact {
  code: ProblemFactCode;
  sourceLine: number;
  ruleId: string;
  role: ProblemObservationRole;
  evidenceFormat: ProblemEvidenceFormat;
  provenance: ProblemLineProvenance;
}

export interface ProblemDetail {
  analysisToken: AnalysisToken;
  revision: number;
  occurrence: ProblemOccurrence;
  facts: ProblemFact[];
  factsTruncated: boolean;
  observationTotal: number;
}

export type ProblemGroupQueryRequest =
  | {
      expectedAnalysisToken: AnalysisToken;
      cursor: null;
      kind: ProblemKind | null;
      sort: "last-seen-desc" | "count-desc";
      limit?: number;
    }
  | {
      expectedAnalysisToken: AnalysisToken;
      cursor: string;
      kind?: ProblemKind | null;
      sort?: "last-seen-desc" | "count-desc";
      limit?: number;
    };

export type ProblemOccurrenceQueryRequest =
  | {
      expectedAnalysisToken: AnalysisToken;
      cursor: null;
      groupId: number;
      limit?: number;
    }
  | {
      expectedAnalysisToken: AnalysisToken;
      cursor: string;
      groupId?: number;
      limit?: number;
    };

export interface ProblemDetailRequest {
  eventId: number;
  expectedAnalysisToken: AnalysisToken;
}

export interface ProblemReleaseSnapshotRequest {
  snapshotHandle: string;
  expectedAnalysisToken: AnalysisToken;
}

export interface ProblemExportRequest {
  eventId: number;
  expectedAnalysisToken: AnalysisToken;
  mode: "event-range" | "context";
  radius?: number;
  path: string;
}

interface CheckedRowsRequestBase {
  start: number;
  count: number;
  expectedAnalysisToken: AnalysisToken;
  requestNonce: number;
}

export type CheckedRowsRequest =
  | (CheckedRowsRequestBase & {
      view: "filtered";
      expectedFilterResultRevision: number;
    })
  | (CheckedRowsRequestBase & {
      view: Exclude<RowsView, "filtered">;
      expectedFilterResultRevision?: number | null;
    });

export type CheckedRowsResponse =
  | {
      status: "ok";
      analysisToken: AnalysisToken;
      requestNonce: number;
      decodeRevision: number;
      sourceDataRevision: number;
      filterResultRevision: number;
      rows: Row[];
    }
  | {
      status: "stale-filter-result";
      analysisToken: AnalysisToken;
      requestNonce: number;
      actualFilterResultRevision: number;
    };

export type LineMappingBias = "exact" | "nearest";

export interface LineMappingRequest {
  lineNo: number;
  bias: LineMappingBias;
  expectedAnalysisToken: AnalysisToken;
  expectedFilterResultRevision: number;
  requestNonce: number;
}

export type LineMappingResponse =
  | {
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: NavigationTarget | null;
    }
  | {
      status: "stale-filter-result";
      analysisToken: AnalysisToken;
      actualFilterResultRevision: number;
    };

export interface LineRange {
  startLine: number;
  endLine: number;
}

export interface ProblemOccurrenceRef extends LineRange {
  eventId: number;
  groupId: number;
  anchorLine: number;
}

export interface TableReturnPoint {
  viewportLine: number;
  selectedLine: number | null;
  filterInputRevision: number;
}

export type TableScope =
  | { kind: "results"; view: "filtered" }
  | {
      kind: "problem-context";
      occurrence: ProblemOccurrenceRef;
      eventRange: LineRange;
      contextRange: LineRange;
      returnPoint: TableReturnPoint;
    };

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
  cancelled: boolean;
}

export interface ExportProgress {
  writtenLines: number;
  writtenBytes: number;
  done: boolean;
  path: string | null;
  cancelled: boolean;
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
