import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AnalysisToken,
  AppConfig,
  CheckedRowsRequest,
  CheckedRowsResponse,
  DeviceList,
  ExportProgress,
  ExportRequest,
  ExportSummary,
  FilterDone,
  FilterSpec,
  LineMappingRequest,
  LineMappingResponse,
  MinimapData,
  NavigationTarget,
  ProblemDetail,
  ProblemDetailRequest,
  ProblemExportRequest,
  ProblemGroup,
  ProblemGroupQueryRequest,
  ProblemOccurrence,
  ProblemOccurrenceQueryRequest,
  ProblemPage,
  ProblemReleaseSnapshotRequest,
  ProblemsProgress,
  ProblemsStatus,
  Row,
  RowsView,
  SearchProgress,
  SearchResult,
  SearchSpec,
  SplitProgress,
  SplitRequest,
  SplitSummary,
  StartLogcatRequest,
  Status,
  StreamAppend,
  StreamControl,
} from "@/types";

export const openFile = (path: string) => invoke<Status>("open_file", { path });

export const listDevices = () => invoke<DeviceList>("list_devices");

export const startLogcat = (request: StartLogcatRequest) =>
  invoke<StreamControl>("start_logcat", { request });

export const pauseLogcat = () => invoke<StreamControl>("pause_logcat");

export const resumeLogcat = () => invoke<StreamControl>("resume_logcat");

export const stopLogcat = () => invoke<StreamControl>("stop_logcat");

export const clearLogcat = () => invoke<StreamControl>("clear_logcat");

export const getStatus = () => invoke<Status>("get_status");

export const getRows = (view: RowsView, start: number, count: number) => {
  if (!Number.isInteger(count) || count <= 0 || count > 512) {
    return Promise.reject(new Error("row window count must be between 1 and 512"));
  }
  return invoke<Row[]>("get_rows", { view, start, count });
};

export const getRowsChecked = async (
  request: CheckedRowsRequest,
): Promise<CheckedRowsResponse> => {
  if (!Number.isInteger(request.count) || request.count <= 0 || request.count > 512) {
    throw new Error("row window count must be between 1 and 512");
  }
  const response = await invoke<CheckedRowsResponse>("get_rows_checked", { request });
  if (
    response.requestNonce !== request.requestNonce ||
    !sameAnalysisToken(response.analysisToken, request.expectedAnalysisToken)
  ) {
    throw new Error("stale-checked-rows-response");
  }
  if (response.status === "stale-filter-result") {
    return response;
  }
  if (response.status !== "ok") {
    throw new Error("unknown-checked-rows-status");
  }
  if (
    request.view === "filtered" &&
    response.filterResultRevision !== request.expectedFilterResultRevision
  ) {
    throw new Error("checked-rows-filter-revision-mismatch");
  }
  if (response.rows.length > request.count) {
    throw new Error("checked-rows-window-overflow");
  }
  return response;
};

interface RawLineMappingResponse {
  status: "ok" | "stale-filter-result";
  analysisToken: AnalysisToken;
  filterResultRevision: number;
  requestNonce: number;
  target: NavigationTarget | null;
}

function sameAnalysisToken(left: AnalysisToken, right: AnalysisToken) {
  return (
    left.sessionGeneration === right.sessionGeneration &&
    left.analysisGeneration === right.analysisGeneration
  );
}

export const mapSourceLine = async (
  request: LineMappingRequest,
): Promise<LineMappingResponse> => {
  const response = await invoke<RawLineMappingResponse>("map_source_line", { request });
  if (
    response.requestNonce !== request.requestNonce ||
    !sameAnalysisToken(response.analysisToken, request.expectedAnalysisToken)
  ) {
    throw new Error("stale-line-mapping-response");
  }
  if (response.status === "stale-filter-result") {
    return {
      status: response.status,
      analysisToken: response.analysisToken,
      actualFilterResultRevision: response.filterResultRevision,
    };
  }
  if (response.status !== "ok") {
    throw new Error("unknown-line-mapping-status");
  }
  if (response.filterResultRevision !== request.expectedFilterResultRevision) {
    throw new Error("line-mapping-filter-revision-mismatch");
  }
  return {
    status: response.status,
    analysisToken: response.analysisToken,
    filterResultRevision: response.filterResultRevision,
    target: response.target,
  };
};

export const getProblemsStatus = () => invoke<ProblemsStatus>("get_problems_status");

export const getProblemGroups = (request: ProblemGroupQueryRequest) =>
  invoke<ProblemPage<ProblemGroup>>("get_problem_groups", { request });

export const getProblemOccurrences = (request: ProblemOccurrenceQueryRequest) =>
  invoke<ProblemPage<ProblemOccurrence>>("get_problem_occurrences", { request });

export const getProblemDetail = (request: ProblemDetailRequest) =>
  invoke<ProblemDetail>("get_problem_detail", { request });

export const releaseProblemSnapshot = (request: ProblemReleaseSnapshotRequest) =>
  invoke<boolean>("release_problem_snapshot", { request });

export const exportProblemLogs = (request: ProblemExportRequest) =>
  invoke<ExportSummary>("export_problem_logs", { request });

export const setFilter = (filter: FilterSpec, filterInputRevision: number) =>
  invoke<number>("set_filter", { filter, filterInputRevision });

export const getFilteredCount = () => invoke<number>("get_filtered_count");

export const searchLogs = (spec: SearchSpec, requestId: number) =>
  invoke<SearchResult>("search", { spec, requestId });

export const searchNext = (fromLineNo: number, direction: "next" | "previous") =>
  invoke<number | null>("search_next", { fromLineNo, direction });

export const toggleBookmark = (lineNo: number) => invoke<boolean>("toggle_bookmark", { lineNo });

export const listBookmarks = () => invoke<number[]>("list_bookmarks");

export const nextBookmark = (fromLineNo: number, direction: "next" | "previous") =>
  invoke<NavigationTarget | null>("next_bookmark", { fromLineNo, direction });

export const lineToResultIndex = (lineNo: number) =>
  invoke<NavigationTarget | null>("line_to_result_index", { lineNo });

export const getMinimap = (buckets: number) => invoke<MinimapData>("get_minimap", { buckets });

export const exportLogs = (request: ExportRequest) =>
  invoke<ExportSummary>("export_logs", { request });

export const cancelExport = () => invoke<void>("cancel_export");

export const splitLogFile = (request: SplitRequest) =>
  invoke<SplitSummary>("split_log_file", { request });

export const getConfig = () => invoke<AppConfig>("get_config");

export const saveAppConfig = (config: AppConfig) => invoke<AppConfig>("set_config", { config });

export const onIndexProgress = (cb: (s: Status) => void): Promise<UnlistenFn> =>
  listen<Status>("index:progress", (e) => cb(e.payload));

export const onFilterDone = (cb: (done: FilterDone) => void): Promise<UnlistenFn> =>
  listen<FilterDone>("filter:done", (e) => cb(e.payload));

export const onSearchProgress = (cb: (progress: SearchProgress) => void): Promise<UnlistenFn> =>
  listen<SearchProgress>("search:progress", (e) => cb(e.payload));

export const onStreamAppend = (cb: (append: StreamAppend) => void): Promise<UnlistenFn> =>
  listen<StreamAppend>("stream:append", (e) => cb(e.payload));

export const onStreamControl = (cb: (control: StreamControl) => void): Promise<UnlistenFn> =>
  listen<StreamControl>("stream:control", (e) => cb(e.payload));

export const onStreamError = (cb: (error: string) => void): Promise<UnlistenFn> =>
  listen<string>("stream:error", (e) => cb(e.payload));

export const onProblemsProgress = (
  cb: (progress: ProblemsProgress) => void,
): Promise<UnlistenFn> =>
  listen<ProblemsProgress>("problems:progress", (e) => cb(e.payload));

export const onSplitProgress = (cb: (progress: SplitProgress) => void): Promise<UnlistenFn> =>
  listen<SplitProgress>("split:progress", (e) => cb(e.payload));

export const onExportProgress = (cb: (progress: ExportProgress) => void): Promise<UnlistenFn> =>
  listen<ExportProgress>("export:progress", (e) => cb(e.payload));
