import { create } from "zustand";
import type {
  AdbDevice,
  AnalysisToken,
  AppConfig,
  FilterDone,
  FilterSpec,
  LogcatBuffer,
  Row,
  RowsView,
  ScrollRequest,
  SearchSpec,
  SourceMode,
  Status,
  StreamControl,
  StreamLifecycle,
  TableScope,
  ThemeMode,
} from "@/types";
import type {
  ContextRowsResponse,
  NavigationCommitGuard,
  TableNavigationCommit,
} from "@/lib/tableScopeController";

type FilterFieldKey = "pid" | "tid" | "tagInclude" | "tagExclude" | "wordInclude" | "wordExclude";

export interface ContextRowsWindow {
  analysisToken: AnalysisToken;
  requestNonce: number;
  rows: readonly Row[];
}

export interface SessionState {
  status: Status;
  sessionId: number; // 每打开一个新文件自增,供表格清缓存
  sourcePath: string | null;
  sourceMode: SourceMode;
  devices: AdbDevice[];
  selectedDeviceSerial: string | null;
  logcatBuffers: LogcatBuffer[];
  streamRunning: boolean;
  streamPaused: boolean;
  streamLifecycle: StreamLifecycle;
  streamError: string | null;
  tailFollowing: boolean;
  view: RowsView;
  tableScope: TableScope;
  contextRows: ContextRowsWindow | null;
  appConfig: AppConfig;
  theme: ThemeMode;
  filter: FilterSpec;
  filterRevision: number;
  appliedFilterInputRevision: number;
  filterResultRevision: number;
  search: SearchSpec;
  searchRevision: number;
  searchCount: number;
  currentSearchLine: number | null;
  selectedLine: number | null;
  selectedResultIndex: number | null;
  viewportLine: number;
  viewportResultIndex: number;
  scrollRequest: ScrollRequest | null;
  bookmarks: number[];
  bookmarkRevision: number;
  setStatus: (s: Status) => void;
  beginSession: (s: Status, sourcePath?: string | null, sourceMode?: SourceMode) => void;
  setSourcePath: (sourcePath: string | null) => void;
  setSourceMode: (sourceMode: SourceMode) => void;
  setDevices: (devices: AdbDevice[]) => void;
  setSelectedDeviceSerial: (serial: string | null) => void;
  setLogcatBuffers: (buffers: LogcatBuffer[]) => void;
  setStreamControl: (control: StreamControl) => void;
  setTailFollowing: (tailFollowing: boolean) => void;
  setTailFollowingFromViewport: (isAtBottom: boolean, source: "user" | "program") => void;
  pauseTailFollowing: (
    reason: "row" | "search" | "bookmark" | "minimap" | "jump" | "scroll",
  ) => void;
  setAppConfig: (config: AppConfig) => void;
  setTheme: (theme: ThemeMode) => void;
  applyFilterDone: (done: FilterDone) => void;
  setFilteredLines: (count: number, options?: { invalidateRows?: boolean }) => void;
  setBookmarks: (bookmarks: number[]) => void;
  setView: (view: RowsView) => void;
  setFilter: (patch: Partial<FilterSpec>) => void;
  setFilterField: (key: FilterFieldKey, patch: Partial<FilterSpec["pid"]>) => void;
  setHighlightRule: (index: number, patch: Partial<FilterSpec["highlights"][number]>) => void;
  toggleLevel: (bit: number) => void;
  setSearch: (patch: Partial<SearchSpec>) => void;
  setSearchResult: (count: number, firstLine: number | null) => void;
  setCurrentSearchLine: (line: number | null) => void;
  setSelectedLine: (line: number | null) => void;
  setSelectedResultIndex: (index: number | null) => void;
  setViewportPosition: (index: number, lineNo?: number | null) => void;
  setViewportResultIndex: (index: number) => void;
  selectRow: (line: number | null, resultIndex: number | null) => void;
  navigateToResultIndex: (
    index: number,
    options?: {
      lineNo?: number | null;
      align?: ScrollRequest["align"];
      reason?: ScrollRequest["reason"];
    },
  ) => void;
  requestTailFollow: (index: number) => void;
  commitTableNavigation: (
    update: TableNavigationCommit,
    guard: NavigationCommitGuard,
  ) => boolean;
  acceptContextRows: (response: ContextRowsResponse) => void;
}

export const LEVEL_BITS = {
  V: 1 << 0,
  D: 1 << 1,
  I: 1 << 2,
  W: 1 << 3,
  E: 1 << 4,
  F: 1 << 5,
} as const;

export const ALL_LEVELS =
  LEVEL_BITS.V | LEVEL_BITS.D | LEVEL_BITS.I | LEVEL_BITS.W | LEVEL_BITS.E | LEVEL_BITS.F;

const field = (enabled = false, pattern = "", regex = false) => ({ enabled, pattern, regex });

export const DEFAULT_FILTER: FilterSpec = {
  levels: ALL_LEVELS,
  markedOnly: false,
  pid: field(),
  tid: field(),
  tagInclude: field(),
  tagExclude: field(),
  wordInclude: field(),
  wordExclude: field(),
  highlights: [
    { enabled: false, pattern: "", regex: false, caseSensitive: false, color: "yellow" },
    { enabled: false, pattern: "", regex: false, caseSensitive: false, color: "green" },
    { enabled: false, pattern: "", regex: false, caseSensitive: false, color: "blue" },
  ],
};

const EMPTY: Status = {
  totalLines: 0,
  stableLines: 0,
  filteredLines: 0,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 0,
  totalBytes: 0,
  indexing: false,
  generation: 0,
  analysisGeneration: 0,
  filterInputRevision: 0,
  appliedFilterInputRevision: 0,
  filterResultRevision: 0,
  decodeRevision: 0,
  sourceDataRevision: 0,
};

const DEFAULT_SCOPE: TableScope = { kind: "results", view: "filtered" };

function analysisTokenFor(status: Status): AnalysisToken {
  return {
    sessionGeneration: status.generation,
    analysisGeneration: status.analysisGeneration,
  };
}

function sameAnalysisToken(left: AnalysisToken, right: AnalysisToken) {
  return (
    left.sessionGeneration === right.sessionGeneration &&
    left.analysisGeneration === right.analysisGeneration
  );
}

function statusCanReplace(current: Status, next: Status) {
  if (next.generation !== current.generation) return next.generation > current.generation;
  if (next.analysisGeneration !== current.analysisGeneration) {
    return next.analysisGeneration > current.analysisGeneration;
  }
  return (
    next.filterInputRevision >= current.filterInputRevision &&
    next.appliedFilterInputRevision >= current.appliedFilterInputRevision &&
    next.filterResultRevision >= current.filterResultRevision &&
    next.decodeRevision >= current.decodeRevision &&
    next.sourceDataRevision >= current.sourceDataRevision
  );
}

function navigationGuardMatches(state: SessionState, guard: NavigationCommitGuard) {
  const token = analysisTokenFor(state.status);
  return (
    state.tableScope.kind === guard.expectedScopeKind &&
    state.status.generation === guard.expectedSessionGeneration &&
    sameAnalysisToken(token, guard.expectedAnalysisToken) &&
    (guard.expectedFilterInputRevision == null ||
      state.filterRevision === guard.expectedFilterInputRevision) &&
    (guard.expectedAppliedFilterInputRevision == null ||
      state.appliedFilterInputRevision === guard.expectedAppliedFilterInputRevision) &&
    (guard.expectedFilterResultRevision == null ||
      state.filterResultRevision === guard.expectedFilterResultRevision)
  );
}

export const DEFAULT_CONFIG: AppConfig = {
  theme: "light",
  adbPath: null,
  storageDir: null,
  encoding: "UTF-8",
  fontSize: 13,
  rowHeight: 20,
  table: {
    columns: [
      { id: "bookmark", width: 24, visible: true },
      { id: "lineNo", width: 58, visible: true },
      { id: "date", width: 50, visible: true },
      { id: "time", width: 98, visible: true },
      { id: "level", width: 40, visible: true },
      { id: "pid", width: 54, visible: true },
      { id: "tid", width: 54, visible: true },
      { id: "tag", width: 154, visible: true },
      { id: "message", width: 360, visible: true },
    ],
  },
  recentFiles: [],
  lastFilter: DEFAULT_FILTER,
  commandBuffers: ["main"],
  currentCommand: "logcat -v threadtime -b main",
  commandPresets: [
    "logcat -v threadtime -b main",
    "logcat -v threadtime -b system",
    "logcat -v threadtime -b radio",
    "logcat -v threadtime -b events",
    "logcat -v threadtime -b crash",
  ],
  window: {
    width: 1180,
    height: 720,
  },
  configPath: "",
};

export const useSession = create<SessionState>()((set) => ({
  status: EMPTY,
  sessionId: 0,
  sourcePath: null,
  sourceMode: "adb",
  devices: [],
  selectedDeviceSerial: null,
  logcatBuffers: ["main"],
  streamRunning: false,
  streamPaused: false,
  streamLifecycle: "stopped",
  streamError: null,
  tailFollowing: true,
  view: "all",
  tableScope: DEFAULT_SCOPE,
  contextRows: null,
  appConfig: DEFAULT_CONFIG,
  theme: DEFAULT_CONFIG.theme,
  filter: DEFAULT_FILTER,
  filterRevision: 0,
  appliedFilterInputRevision: 0,
  filterResultRevision: 0,
  search: { query: "", regex: false, caseSensitive: false },
  searchRevision: 0,
  searchCount: 0,
  currentSearchLine: null,
  selectedLine: null,
  selectedResultIndex: null,
  viewportLine: 1,
  viewportResultIndex: 0,
  scrollRequest: null,
  bookmarks: [],
  bookmarkRevision: 0,
  // Status carries the complete dataset identity. Never let a delayed event
  // regress one component of that identity.
  setStatus: (status) =>
    set((s) => {
      if (!statusCanReplace(s.status, status)) return {};
      const inputChanged =
        status.generation !== s.status.generation ||
        status.analysisGeneration !== s.status.analysisGeneration;
      return {
        status,
        appliedFilterInputRevision: status.appliedFilterInputRevision,
        filterResultRevision: status.filterResultRevision,
        ...(inputChanged
          ? {
              sessionId: s.sessionId + 1,
              tableScope: DEFAULT_SCOPE,
              contextRows: null,
              selectedLine: null,
              selectedResultIndex: null,
              viewportLine: 1,
              viewportResultIndex: 0,
              scrollRequest: null,
            }
          : {}),
      };
    }),
  // 打开新文件时用它:更新状态并自增 sessionId。
  beginSession: (status, sourcePath, sourceMode) =>
    set((s) => ({
      status,
      sessionId: s.sessionId + 1,
      sourcePath: sourcePath ?? s.sourcePath,
      sourceMode: sourceMode ?? s.sourceMode,
      streamRunning: sourceMode === "file" ? false : s.streamRunning,
      streamPaused: sourceMode === "file" ? false : s.streamPaused,
      streamLifecycle: sourceMode === "file" ? "stopped" : s.streamLifecycle,
      streamError: sourceMode === "file" ? null : s.streamError,
      tailFollowing: sourceMode === "adb",
      view: "all",
      tableScope: DEFAULT_SCOPE,
      contextRows: null,
      filterRevision: Math.max(s.filterRevision + 1, status.filterInputRevision + 1),
      appliedFilterInputRevision: status.appliedFilterInputRevision,
      filterResultRevision: status.filterResultRevision,
      searchCount: 0,
      currentSearchLine: null,
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: null,
      bookmarks: [],
      bookmarkRevision: s.bookmarkRevision + 1,
    })),
  setSourcePath: (sourcePath) => set({ sourcePath }),
  setSourceMode: (sourceMode) => set({ sourceMode }),
  setDevices: (devices) =>
    set((s) => ({
      devices,
      selectedDeviceSerial:
        s.selectedDeviceSerial && devices.some((device) => device.serial === s.selectedDeviceSerial)
          ? s.selectedDeviceSerial
          : (devices.find((device) => device.online)?.serial ?? null),
    })),
  setSelectedDeviceSerial: (selectedDeviceSerial) => set({ selectedDeviceSerial }),
  setLogcatBuffers: (logcatBuffers) =>
    set({ logcatBuffers: logcatBuffers.length > 0 ? logcatBuffers : ["main"] }),
  setStreamControl: (control) =>
    set((s) => {
      const replaceStatus = statusCanReplace(s.status, control.status);
      const inputChanged =
        replaceStatus &&
        (control.status.generation !== s.status.generation ||
          control.status.analysisGeneration !== s.status.analysisGeneration);
      return {
        ...(replaceStatus
          ? {
              status: control.status,
              appliedFilterInputRevision: control.status.appliedFilterInputRevision,
              filterResultRevision: control.status.filterResultRevision,
            }
          : {}),
        streamRunning: control.running,
        streamPaused: control.paused,
        streamLifecycle: control.lifecycle,
        streamError: control.error,
        selectedDeviceSerial: control.deviceSerial,
        sourcePath: control.sessionPath,
        sourceMode: "adb",
        ...(inputChanged
          ? {
              sessionId: s.sessionId + 1,
              tableScope: DEFAULT_SCOPE,
              contextRows: null,
              selectedLine: null,
              selectedResultIndex: null,
              viewportLine: 1,
              viewportResultIndex: 0,
              scrollRequest: null,
            }
          : {}),
      };
    }),
  setTailFollowing: (tailFollowing) => set({ tailFollowing }),
  setTailFollowingFromViewport: (isAtBottom, source) =>
    set((s) => (source === "user" && s.sourceMode === "adb" ? { tailFollowing: isAtBottom } : {})),
  pauseTailFollowing: () => set((s) => (s.sourceMode === "adb" ? { tailFollowing: false } : {})),
  setAppConfig: (appConfig) => set({ appConfig, theme: appConfig.theme }),
  setTheme: (theme) =>
    set((s) => ({
      theme,
      appConfig: { ...s.appConfig, theme },
    })),
  applyFilterDone: (done) =>
    set((s) => {
      if (
        done.generation !== s.status.generation ||
        done.filterInputRevision !== s.filterRevision ||
        done.filterInputRevision < s.appliedFilterInputRevision ||
        done.filterResultRevision < s.filterResultRevision
      ) {
        return {};
      }
      const count = done.filteredLines;
      return {
        selectedResultIndex:
          s.selectedResultIndex != null && s.selectedResultIndex < count
            ? s.selectedResultIndex
            : null,
        viewportResultIndex: count > 0 ? Math.min(s.viewportResultIndex, count - 1) : 0,
        status: {
          ...s.status,
          filteredLines: count,
          filterInputRevision: Math.max(
            s.status.filterInputRevision,
            done.filterInputRevision,
          ),
          appliedFilterInputRevision: done.filterInputRevision,
          filterResultRevision: done.filterResultRevision,
        },
        appliedFilterInputRevision: done.filterInputRevision,
        filterResultRevision: done.filterResultRevision,
      };
    }),
  setFilteredLines: (count, options) =>
    set((s) => ({
      selectedResultIndex:
        s.selectedResultIndex != null && s.selectedResultIndex < count
          ? s.selectedResultIndex
          : null,
      viewportResultIndex: count > 0 ? Math.min(s.viewportResultIndex, count - 1) : 0,
      status: { ...s.status, filteredLines: count },
      filterResultRevision:
        options?.invalidateRows === false ? s.filterResultRevision : s.filterResultRevision + 1,
    })),
  setBookmarks: (bookmarks) =>
    set((s) => ({
      bookmarks,
      bookmarkRevision: s.bookmarkRevision + 1,
      filterRevision: s.filter.markedOnly
        ? Math.max(s.filterRevision, s.status.filterInputRevision) + 1
        : s.filterRevision,
      status: { ...s.status, bookmarkLines: bookmarks.length },
    })),
  setView: (view) => set({ view }),
  setFilter: (patch) =>
    set((s) => {
      const filterRevision = Math.max(s.filterRevision, s.status.filterInputRevision) + 1;
      return {
        filter: { ...s.filter, ...patch },
        filterRevision,
        tailFollowing: s.sourceMode === "adb" ? false : s.tailFollowing,
      };
    }),
  setFilterField: (key, patch) =>
    set((s) => {
      const filterRevision = Math.max(s.filterRevision, s.status.filterInputRevision) + 1;
      return {
        filter: {
          ...s.filter,
          [key]: { ...s.filter[key], ...patch },
        },
        filterRevision,
        tailFollowing: s.sourceMode === "adb" ? false : s.tailFollowing,
      };
    }),
  setHighlightRule: (index, patch) =>
    set((s) => ({
      filter: {
        ...s.filter,
        highlights: s.filter.highlights.map((rule, ruleIndex) =>
          ruleIndex === index ? { ...rule, ...patch } : rule,
        ),
      },
      filterRevision: Math.max(s.filterRevision, s.status.filterInputRevision) + 1,
    })),
  toggleLevel: (bit) =>
    set((s) => ({
      filter: { ...s.filter, levels: s.filter.levels ^ bit },
      filterRevision: Math.max(s.filterRevision, s.status.filterInputRevision) + 1,
      tailFollowing: s.sourceMode === "adb" ? false : s.tailFollowing,
    })),
  setSearch: (patch) =>
    set((s) => ({
      search: { ...s.search, ...patch },
      searchRevision: s.searchRevision + 1,
    })),
  setSearchResult: (count, firstLine) =>
    set({
      searchCount: count,
      currentSearchLine: firstLine,
    }),
  setCurrentSearchLine: (line) =>
    set({ currentSearchLine: line, selectedLine: line, selectedResultIndex: null }),
  setSelectedLine: (line) => set({ selectedLine: line }),
  setSelectedResultIndex: (selectedResultIndex) => set({ selectedResultIndex }),
  setViewportPosition: (index, lineNo) =>
    set((s) => {
      const rowCount =
        s.tableScope.kind === "problem-context" ? s.status.stableLines : s.status.filteredLines;
      const viewportResultIndex =
        rowCount > 0 ? Math.min(Math.max(0, index), rowCount - 1) : 0;
      return {
        viewportResultIndex,
        ...(lineNo == null ? {} : { viewportLine: lineNo }),
      };
    }),
  setViewportResultIndex: (index) =>
    set((s) => {
      const rowCount =
        s.tableScope.kind === "problem-context" ? s.status.stableLines : s.status.filteredLines;
      const viewportResultIndex = rowCount > 0 ? Math.min(Math.max(0, index), rowCount - 1) : 0;
      return viewportResultIndex === s.viewportResultIndex ? {} : { viewportResultIndex };
    }),
  selectRow: (selectedLine, selectedResultIndex) => set({ selectedLine, selectedResultIndex }),
  navigateToResultIndex: (index, options) =>
    set((s) => {
      const safeIndex =
        s.status.filteredLines > 0
          ? Math.min(Math.max(0, index), s.status.filteredLines - 1)
          : Math.max(0, index);
      const nonce = (s.scrollRequest?.nonce ?? 0) + 1;
      return {
        selectedLine: options?.lineNo ?? s.selectedLine,
        selectedResultIndex: safeIndex,
        viewportLine: options?.lineNo ?? s.viewportLine,
        viewportResultIndex: safeIndex,
        scrollRequest: {
          index: safeIndex,
          align: options?.align ?? "center",
          reason: options?.reason ?? "bookmark",
          nonce,
        },
      };
    }),
  requestTailFollow: (index) =>
    set((s) => {
      const safeIndex =
        s.status.filteredLines > 0
          ? Math.min(Math.max(0, index), s.status.filteredLines - 1)
          : Math.max(0, index);
      const nonce = (s.scrollRequest?.nonce ?? 0) + 1;
      return {
        viewportResultIndex: safeIndex,
        scrollRequest: {
          index: safeIndex,
          align: "end",
          reason: "tail",
          nonce,
        },
      };
    }),
  commitTableNavigation: (update, guard) => {
    let accepted = false;
    set((state) => {
      if (!navigationGuardMatches(state, guard)) return {};
      accepted = true;
      return {
        tableScope: update.scope,
        contextRows: update.scope.kind === "problem-context" ? state.contextRows : null,
        selectedLine: update.selectedLine,
        selectedResultIndex: update.selectedResultIndex,
        viewportLine: update.viewportLine,
        viewportResultIndex: update.viewportResultIndex,
        scrollRequest: update.scrollRequest,
        tailFollowing: update.tailFollowing,
      };
    });
    return accepted;
  },
  acceptContextRows: (response) =>
    set((state) => {
      if (
        state.tableScope.kind !== "problem-context" ||
        !sameAnalysisToken(response.analysisToken, analysisTokenFor(state.status))
      ) {
        return {};
      }
      return {
        contextRows: {
          analysisToken: response.analysisToken,
          requestNonce: response.requestNonce,
          rows: response.rows as readonly Row[],
        },
      };
    }),
}));
