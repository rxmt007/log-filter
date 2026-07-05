import { create } from "zustand";
import type { AppConfig, FilterSpec, RowsView, SearchSpec, Status, ThemeMode } from "@/types";

interface SessionState {
  status: Status;
  sessionId: number; // 每打开一个新文件自增,供表格清缓存
  sourcePath: string | null;
  view: RowsView;
  appConfig: AppConfig;
  theme: ThemeMode;
  filter: FilterSpec;
  filterRevision: number;
  search: SearchSpec;
  searchCount: number;
  currentSearchLine: number | null;
  selectedLine: number | null;
  bookmarks: number[];
  bookmarkRevision: number;
  setStatus: (s: Status) => void;
  beginSession: (s: Status, sourcePath?: string) => void;
  setSourcePath: (sourcePath: string | null) => void;
  setAppConfig: (config: AppConfig) => void;
  setTheme: (theme: ThemeMode) => void;
  setFilteredLines: (count: number) => void;
  setBookmarks: (bookmarks: number[]) => void;
  setView: (view: RowsView) => void;
  setFilter: (patch: Partial<FilterSpec>) => void;
  setFilterField: (key: keyof Omit<FilterSpec, "levels">, patch: Partial<FilterSpec["pid"]>) => void;
  toggleLevel: (bit: number) => void;
  setSearch: (patch: Partial<SearchSpec>) => void;
  setSearchResult: (count: number, firstLine: number | null) => void;
  setCurrentSearchLine: (line: number | null) => void;
  setSelectedLine: (line: number | null) => void;
}

export const LEVEL_BITS = {
  V: 1 << 0,
  D: 1 << 1,
  I: 1 << 2,
  W: 1 << 3,
  E: 1 << 4,
  F: 1 << 5,
} as const;

export const ALL_LEVELS = LEVEL_BITS.V | LEVEL_BITS.D | LEVEL_BITS.I | LEVEL_BITS.W | LEVEL_BITS.E | LEVEL_BITS.F;

const field = (enabled = false, pattern = "", regex = false) => ({ enabled, pattern, regex });

export const DEFAULT_FILTER: FilterSpec = {
  levels: ALL_LEVELS,
  pid: field(),
  tid: field(),
  tagInclude: field(),
  tagExclude: field(),
  wordInclude: field(),
  wordExclude: field(),
};

const EMPTY: Status = {
  totalLines: 0,
  filteredLines: 0,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 0,
  totalBytes: 0,
  indexing: false,
  generation: 0,
};

export const DEFAULT_CONFIG: AppConfig = {
  theme: "light",
  adbPath: null,
  storageDir: null,
  encoding: "UTF-8",
  fontSize: 13,
  rowHeight: 20,
  configPath: "",
};

export const useSession = create<SessionState>()((set) => ({
  status: EMPTY,
  sessionId: 0,
  sourcePath: null,
  view: "all",
  appConfig: DEFAULT_CONFIG,
  theme: DEFAULT_CONFIG.theme,
  filter: DEFAULT_FILTER,
  filterRevision: 0,
  search: { query: "", regex: false, caseSensitive: false },
  searchCount: 0,
  currentSearchLine: null,
  selectedLine: null,
  bookmarks: [],
  bookmarkRevision: 0,
  // 索引进度事件用它更新状态(不换 session)。
  setStatus: (status) =>
    set((s) => (status.generation >= s.status.generation ? { status } : {})),
  // 打开新文件时用它:更新状态并自增 sessionId。
  beginSession: (status, sourcePath) =>
    set((s) => ({
      status,
      sessionId: s.sessionId + 1,
      sourcePath: sourcePath ?? s.sourcePath,
      view: "all",
      searchCount: 0,
      currentSearchLine: null,
      selectedLine: null,
      bookmarks: [],
      bookmarkRevision: s.bookmarkRevision + 1,
    })),
  setSourcePath: (sourcePath) => set({ sourcePath }),
  setAppConfig: (appConfig) => set({ appConfig, theme: appConfig.theme }),
  setTheme: (theme) =>
    set((s) => ({
      theme,
      appConfig: { ...s.appConfig, theme },
    })),
  setFilteredLines: (count) =>
    set((s) => ({ status: { ...s.status, filteredLines: count } })),
  setBookmarks: (bookmarks) =>
    set((s) => ({
      bookmarks,
      bookmarkRevision: s.bookmarkRevision + 1,
      status: { ...s.status, bookmarkLines: bookmarks.length },
    })),
  setView: (view) => set({ view }),
  setFilter: (patch) =>
    set((s) => ({
      filter: { ...s.filter, ...patch },
      filterRevision: s.filterRevision + 1,
    })),
  setFilterField: (key, patch) =>
    set((s) => ({
      filter: {
        ...s.filter,
        [key]: { ...s.filter[key], ...patch },
      },
      filterRevision: s.filterRevision + 1,
    })),
  toggleLevel: (bit) =>
    set((s) => ({
      filter: { ...s.filter, levels: s.filter.levels ^ bit },
      filterRevision: s.filterRevision + 1,
    })),
  setSearch: (patch) =>
    set((s) => ({ search: { ...s.search, ...patch } })),
  setSearchResult: (count, firstLine) =>
    set({ searchCount: count, currentSearchLine: firstLine, selectedLine: firstLine }),
  setCurrentSearchLine: (line) => set({ currentSearchLine: line, selectedLine: line }),
  setSelectedLine: (line) => set({ selectedLine: line }),
}));
