import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Bookmark,
  ChevronDown,
  ChevronUp,
  Download,
  FolderOpen,
  Pause,
  Play,
  Search,
  Settings,
  Split,
  Moon,
  Sun,
  Square,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ExportDialog, SettingsDialog, SplitDialog } from "@/components/ToolDialogs";
import { openFile, saveAppConfig, searchLogs, searchNext } from "@/lib/ipc";
import { ALL_LEVELS, LEVEL_BITS, useSession } from "@/store/session";
import type { FilterSpec, ThemeMode } from "@/types";

const LEVELS = [
  ["V", LEVEL_BITS.V],
  ["D", LEVEL_BITS.D],
  ["I", LEVEL_BITS.I],
  ["W", LEVEL_BITS.W],
  ["E", LEVEL_BITS.E],
  ["F", LEVEL_BITS.F],
] as const;

const LEVEL_TOOLTIPS = {
  V: "Verbose",
  D: "Debug",
  I: "Info",
  W: "Warning",
  E: "Error",
  F: "Fatal",
} as const;

const FILTER_FIELDS: Array<{
  key: keyof Omit<FilterSpec, "levels" | "markedOnly">;
  label: string;
  badge?: "+" | "-";
  placeholder: string;
}> = [
  { key: "tagInclude", label: "Tag 包含", badge: "+", placeholder: "*Manager" },
  { key: "tagExclude", label: "Tag 屏蔽", badge: "-", placeholder: "chatty|GC" },
  { key: "pid", label: "PID", placeholder: "12043|146" },
  { key: "tid", label: "TID", placeholder: "179|12095" },
  { key: "wordInclude", label: "内容包含", badge: "+", placeholder: "network|支付" },
  { key: "wordExclude", label: "内容屏蔽", badge: "-", placeholder: "heartbeat" },
];

export function Toolbar() {
  const [dialog, setDialog] = useState<"export" | "split" | "settings" | null>(null);
  const beginSession = useSession((s) => s.beginSession);
  const status = useSession((s) => s.status);
  const appConfig = useSession((s) => s.appConfig);
  const theme = useSession((s) => s.theme);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const setTheme = useSession((s) => s.setTheme);
  const filter = useSession((s) => s.filter);
  const setFilter = useSession((s) => s.setFilter);
  const toggleLevel = useSession((s) => s.toggleLevel);
  const setFilterField = useSession((s) => s.setFilterField);
  const search = useSession((s) => s.search);
  const setSearch = useSession((s) => s.setSearch);
  const searchCount = useSession((s) => s.searchCount);
  const currentSearchLine = useSession((s) => s.currentSearchLine);
  const setSearchResult = useSession((s) => s.setSearchResult);
  const setCurrentSearchLine = useSession((s) => s.setCurrentSearchLine);

  const onOpen = async () => {
    const path = await open({ multiple: false, directory: false });
    if (typeof path === "string") {
      const st = await openFile(path);
      beginSession(st, path);
    }
  };

  const toggleTheme = async () => {
    const nextTheme: ThemeMode = theme === "dark" ? "light" : "dark";
    const nextConfig = { ...appConfig, theme: nextTheme };
    setTheme(nextTheme);
    try {
      const saved = await saveAppConfig(nextConfig);
      setAppConfig(saved);
    } catch (err) {
      console.error("save theme failed", err);
    }
  };

  useEffect(() => {
    if (!status.totalBytes) {
      setSearchResult(0, null);
      return;
    }
    if (!search.query.trim()) {
      setSearchResult(0, null);
      void searchLogs(search).catch((err) => {
        console.error("clear search failed", err);
      });
      return;
    }
    const timer = window.setTimeout(() => {
      void searchLogs(search).catch((err) => {
        console.error("search failed", err);
        setSearchResult(0, null);
      });
    }, 180);
    return () => window.clearTimeout(timer);
  }, [search, status.totalBytes, setSearchResult]);

  const jumpSearch = useCallback(
    async (direction: "next" | "previous") => {
      if (!searchCount) return;
      const from = currentSearchLine ?? 0;
      const line = await searchNext(from, direction);
      setCurrentSearchLine(line);
    },
    [currentSearchLine, searchCount, setCurrentSearchLine],
  );

  const countLabel = search.query ? `${currentSearchLine ?? "-"} / ${searchCount}` : "0 / 0";

  return (
    <>
      <div className="lf-toolbar">
        <div className="lf-toolbar-row lf-toolbar-row-top">
          <button
            aria-label="Source file"
            className="lf-select-button"
            data-tooltip="Source file"
            data-tooltip-placement="bottom"
            type="button"
          >
            <FolderOpen />
            <span>来源:文件</span>
            <ChevronDown />
          </button>
          <button
            aria-label="Current source"
            className="lf-select-button lf-device"
            data-tooltip="Current source"
            data-tooltip-placement="bottom"
            type="button"
          >
            <span className="lf-device-dot" />
            <span>{status.totalBytes ? "本地日志文件" : "无设备"}</span>
            <ChevronDown />
          </button>
          <button
            aria-label="Logcat command"
            className="lf-select-button lf-command"
            data-tooltip="Logcat command"
            data-tooltip-placement="bottom"
            type="button"
          >
            <span>命令</span>
            <code>logcat -v threadtime</code>
            <ChevronDown />
          </button>
        </div>

        <div className="lf-toolbar-row lf-toolbar-row-actions">
          <Button aria-label="Start" className="lf-run-button" data-tooltip="Start" size="icon-sm">
            <Play />
          </Button>
          <Button aria-label="Pause" data-tooltip="Pause" size="icon-sm" variant="ghost">
            <Pause />
          </Button>
          <Button aria-label="Stop" data-tooltip="Stop" size="icon-sm" variant="ghost">
            <Square />
          </Button>
          <Button aria-label="Clear" data-tooltip="Clear" size="icon-sm" variant="ghost">
            <Trash2 />
          </Button>
          <span className="lf-separator" />
          <Button
            aria-label="Open file"
            data-tooltip="Open file"
            size="icon-sm"
            variant="ghost"
            onClick={onOpen}
          >
            <FolderOpen />
          </Button>
          <Button
            aria-label="Export"
            data-tooltip="Export"
            size="icon-sm"
            variant="ghost"
            onClick={() => setDialog("export")}
          >
            <Download />
          </Button>
          <Button
            aria-label="Split file"
            data-tooltip="Split file"
            size="icon-sm"
            variant="ghost"
            onClick={() => setDialog("split")}
          >
            <Split />
          </Button>
          <Button
            aria-label="Settings"
            data-tooltip="Settings"
            size="icon-sm"
            variant="ghost"
            onClick={() => setDialog("settings")}
          >
            <Settings />
          </Button>
          <Button
            aria-label="Theme"
            data-tooltip="Theme"
            size="icon-sm"
            variant="ghost"
            onClick={toggleTheme}
          >
            {theme === "dark" ? <Sun /> : <Moon />}
          </Button>
          <span className="lf-separator" />
          <span className="lf-level-label">级别</span>
          <div className="lf-level-chips">
            <button
              aria-label="All levels"
              className="lf-level-chip lf-level-all"
              data-active={filter.levels === ALL_LEVELS}
              data-tooltip="All levels"
              type="button"
              onClick={() => setFilter({ levels: ALL_LEVELS })}
            >
              <b>全部</b>
            </button>
            {LEVELS.map(([level, bit]) => {
              const on = (filter.levels & bit) !== 0;
              return (
                <button
                  aria-label={LEVEL_TOOLTIPS[level]}
                  key={level}
                  className="lf-level-chip"
                  data-level={level}
                  data-active={on}
                  data-tooltip={LEVEL_TOOLTIPS[level]}
                  type="button"
                  onClick={() => toggleLevel(bit)}
                >
                  <span />
                  <b>{level}</b>
                </button>
              );
            })}
            <button
              aria-label="Marked only"
              className="lf-level-chip lf-marked-only-chip"
              data-active={filter.markedOnly}
              data-tooltip="Marked only"
              type="button"
              onClick={() => setFilter({ markedOnly: !filter.markedOnly })}
            >
              <Bookmark />
              <b>仅标记</b>
            </button>
          </div>
          <div className="lf-spacer" />
          <div className="lf-search-box">
            <Search />
            <input
              value={search.query}
              onChange={(e) => setSearch({ query: e.target.value })}
              placeholder="查找日志…"
            />
            <span className="lf-search-count">{countLabel}</span>
            <button
              aria-label="Case sensitive"
              className="lf-mini-toggle"
              data-active={search.caseSensitive}
              data-tooltip="Case sensitive"
              type="button"
              onClick={() => setSearch({ caseSensitive: !search.caseSensitive })}
            >
              Aa
            </button>
            <button
              aria-label="Regex search"
              className="lf-mini-toggle"
              data-active={search.regex}
              data-tooltip="Regex search"
              type="button"
              onClick={() => setSearch({ regex: !search.regex })}
            >
              .*
            </button>
            <span className="lf-highlight-swatch" data-tooltip="Highlight color" />
            <span className="lf-search-divider" />
            <button
              aria-label="Previous match"
              data-tooltip="Previous match"
              type="button"
              onClick={() => jumpSearch("previous")}
            >
              <ChevronUp />
            </button>
            <button
              aria-label="Next match"
              data-tooltip="Next match"
              type="button"
              onClick={() => jumpSearch("next")}
            >
              <ChevronDown />
            </button>
          </div>
        </div>

        <div className="lf-filter-bar">
          <div className="lf-filter-title">
            <span>过滤条件</span>
          </div>
          <div className="lf-filter-fields">
            {FILTER_FIELDS.map((field) => {
              const value = filter[field.key];
              return (
                <label className="lf-filter-field" data-enabled={value.enabled} key={field.key}>
                  <button
                    className="lf-switch"
                    data-active={value.enabled}
                    type="button"
                    onClick={(e) => {
                      e.preventDefault();
                      setFilterField(field.key, { enabled: !value.enabled });
                    }}
                  >
                    <span />
                  </button>
                  <span className={field.badge === "-" ? "lf-badge lf-badge-exclude" : "lf-badge"}>
                    {field.badge ?? ""}
                  </span>
                  <span className="lf-filter-label">{field.label}</span>
                  <input
                    value={value.pattern}
                    placeholder={field.placeholder}
                    onChange={(e) => setFilterField(field.key, { pattern: e.target.value })}
                  />
                  {(field.key === "tagInclude" ||
                    field.key === "tagExclude" ||
                    field.key === "wordInclude" ||
                    field.key === "wordExclude") && (
                    <button
                      aria-label={`${field.label} 正则`}
                      className="lf-mini-toggle"
                      data-active={value.regex}
                      data-tooltip="Regex filter"
                      type="button"
                      onClick={(e) => {
                        e.preventDefault();
                        setFilterField(field.key, { regex: !value.regex });
                      }}
                    >
                      .*
                    </button>
                  )}
                </label>
              );
            })}
          </div>
        </div>
      </div>
      {dialog === "export" && <ExportDialog onClose={() => setDialog(null)} />}
      {dialog === "split" && <SplitDialog onClose={() => setDialog(null)} />}
      {dialog === "settings" && <SettingsDialog onClose={() => setDialog(null)} />}
    </>
  );
}
