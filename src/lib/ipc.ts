import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  ExportRequest,
  ExportSummary,
  FilterSpec,
  MinimapData,
  Row,
  RowsView,
  SearchResult,
  SearchSpec,
  SplitRequest,
  SplitSummary,
  Status,
} from "@/types";

export const openFile = (path: string) => invoke<Status>("open_file", { path });

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
  invoke<number | null>("next_bookmark", { fromLineNo, direction });

export const getMinimap = (buckets: number) => invoke<MinimapData>("get_minimap", { buckets });

export const exportLogs = (request: ExportRequest) =>
  invoke<ExportSummary>("export_logs", { request });

export const splitLogFile = (request: SplitRequest) =>
  invoke<SplitSummary>("split_log_file", { request });

export const getConfig = () => invoke<AppConfig>("get_config");

export const saveAppConfig = (config: AppConfig) => invoke<AppConfig>("set_config", { config });

export const onIndexProgress = (cb: (s: Status) => void): Promise<UnlistenFn> =>
  listen<Status>("index:progress", (e) => cb(e.payload));
