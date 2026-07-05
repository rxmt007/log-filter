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

export interface MinimapData {
  bookmarks: number[];
  errors: number[];
}

export type RowsView = "all" | "filtered" | "bookmarks" | "errors";
