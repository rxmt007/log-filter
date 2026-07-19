import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  DeviceList,
  ExportProgress,
  ExportRequest,
  ExportSummary,
  FilterDone,
  FilterSpec,
  MinimapData,
  NavigationTarget,
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

export const getRows = (view: RowsView, start: number, count: number) =>
  invoke<Row[]>("get_rows", { view, start, count });

export const setFilter = (filter: FilterSpec) => invoke<number>("set_filter", { filter });

export const getFilteredCount = () => invoke<number>("get_filtered_count");

export const searchLogs = (spec: SearchSpec) => invoke<SearchResult>("search", { spec });

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

export const onSplitProgress = (cb: (progress: SplitProgress) => void): Promise<UnlistenFn> =>
  listen<SplitProgress>("split:progress", (e) => cb(e.payload));

export const onExportProgress = (cb: (progress: ExportProgress) => void): Promise<UnlistenFn> =>
  listen<ExportProgress>("export:progress", (e) => cb(e.payload));
