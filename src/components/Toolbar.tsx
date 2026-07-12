import { useCallback, useEffect, useRef, useState } from "react";
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
import {
  clearLogcat,
  listDevices,
  lineToResultIndex,
  openFile,
  pauseLogcat,
  resumeLogcat,
  saveAppConfig,
  searchLogs,
  searchNext,
  startLogcat,
  stopLogcat,
} from "@/lib/ipc";
import { fileNameFromPath, rememberRecentFile } from "@/lib/recent";
import { ALL_LEVELS, LEVEL_BITS, useSession } from "@/store/session";
import type { FilterSpec, LogcatBuffer, SourceMode, ThemeMode } from "@/types";

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
  key: keyof Omit<FilterSpec, "levels" | "markedOnly" | "highlights">;
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

const LOGCAT_BUFFERS: Array<{ value: LogcatBuffer; label: string }> = [
  { value: "main", label: "main" },
  { value: "system", label: "system" },
  { value: "radio", label: "radio" },
  { value: "events", label: "events" },
  { value: "crash", label: "crash" },
];

const HIGHLIGHT_COLORS = ["yellow", "green", "blue", "purple"] as const;

export function Toolbar() {
  const [dialog, setDialog] = useState<"export" | "split" | "settings" | null>(null);
  const [jumpLine, setJumpLine] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const jumpInputRef = useRef<HTMLInputElement>(null);
  const beginSession = useSession((s) => s.beginSession);
  const status = useSession((s) => s.status);
  const sourceMode = useSession((s) => s.sourceMode);
  const setSourceMode = useSession((s) => s.setSourceMode);
  const devices = useSession((s) => s.devices);
  const setDevices = useSession((s) => s.setDevices);
  const selectedDeviceSerial = useSession((s) => s.selectedDeviceSerial);
  const setSelectedDeviceSerial = useSession((s) => s.setSelectedDeviceSerial);
  const logcatBuffers = useSession((s) => s.logcatBuffers);
  const setLogcatBuffers = useSession((s) => s.setLogcatBuffers);
  const streamRunning = useSession((s) => s.streamRunning);
  const streamPaused = useSession((s) => s.streamPaused);
  const setStreamControl = useSession((s) => s.setStreamControl);
  const appConfig = useSession((s) => s.appConfig);
  const theme = useSession((s) => s.theme);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const setTheme = useSession((s) => s.setTheme);
  const filter = useSession((s) => s.filter);
  const setFilter = useSession((s) => s.setFilter);
  const toggleLevel = useSession((s) => s.toggleLevel);
  const setFilterField = useSession((s) => s.setFilterField);
  const setHighlightRule = useSession((s) => s.setHighlightRule);
  const search = useSession((s) => s.search);
  const setSearch = useSession((s) => s.setSearch);
  const searchCount = useSession((s) => s.searchCount);
  const currentSearchLine = useSession((s) => s.currentSearchLine);
  const setSearchResult = useSession((s) => s.setSearchResult);
  const setCurrentSearchLine = useSession((s) => s.setCurrentSearchLine);
  const navigateToResultIndex = useSession((s) => s.navigateToResultIndex);

  const rememberFile = useCallback(
    async (path: string) => {
      const nextConfig = {
        ...appConfig,
        recentFiles: rememberRecentFile(appConfig.recentFiles, path),
      };
      const saved = await saveAppConfig(nextConfig);
      setAppConfig(saved);
    },
    [appConfig, setAppConfig],
  );

  const openPath = useCallback(
    async (path: string) => {
      const st = await openFile(path);
      beginSession(st, path, "file");
      await rememberFile(path);
    },
    [beginSession, rememberFile],
  );

  const onOpen = useCallback(async () => {
    const path = await open({ multiple: false, directory: false });
    if (typeof path === "string") {
      await openPath(path);
    }
  }, [openPath]);

  const refreshDevices = useCallback(async () => {
    try {
      const result = await listDevices();
      setDevices(result.devices);
    } catch (err) {
      console.error("list_devices failed", err);
      setDevices([]);
    }
  }, [setDevices]);

  useEffect(() => {
    if (sourceMode !== "adb") return;
    void refreshDevices();
    const timer = window.setInterval(() => {
      void refreshDevices();
    }, 4000);
    return () => window.clearInterval(timer);
  }, [refreshDevices, sourceMode]);

  const onSourceChange = (next: string) => {
    if (next.startsWith("recent:")) {
      void openPath(next.slice("recent:".length)).catch((err) => {
        console.error("open recent file failed", err);
      });
      return;
    }
    const source = next as SourceMode;
    setSourceMode(source);
    if (source === "adb") void refreshDevices();
  };

  const runCapture = async () => {
    setSourceMode("adb");
    try {
      if (streamPaused) {
        const control = await resumeLogcat();
        setStreamControl(control);
        return;
      }
      const control = await startLogcat({
        deviceSerial: selectedDeviceSerial,
        buffers: logcatBuffers,
      });
      setStreamControl(control);
      beginSession(control.status, control.sessionPath, "adb");
      const saved = await saveAppConfig({
        ...appConfig,
        commandBuffers: logcatBuffers,
      });
      setAppConfig(saved);
    } catch (err) {
      console.error("start/resume logcat failed", err);
    }
  };

  const pauseCapture = async () => {
    try {
      setStreamControl(await pauseLogcat());
    } catch (err) {
      console.error("pause logcat failed", err);
    }
  };

  const stopCapture = async () => {
    try {
      setStreamControl(await stopLogcat());
    } catch (err) {
      console.error("stop logcat failed", err);
    }
  };

  const clearCapture = async () => {
    try {
      const control = await clearLogcat();
      setStreamControl(control);
      beginSession(control.status, control.sessionPath, "adb");
    } catch (err) {
      console.error("clear logcat failed", err);
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

  const jumpToLine = useCallback(async () => {
    const lineNo = Number(jumpLine);
    if (!Number.isFinite(lineNo) || lineNo < 1) return;
    const target = await lineToResultIndex(lineNo);
    if (!target) return;
    navigateToResultIndex(target.resultIndex, {
      lineNo: target.lineNo,
      align: "center",
      reason: "jump",
    });
  }, [jumpLine, navigateToResultIndex]);

  useEffect(() => {
    const openListener = () => void onOpen();
    const focusSearch = () => {
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
    };
    const focusJump = () => {
      jumpInputRef.current?.focus();
      jumpInputRef.current?.select();
    };
    const focusDevice = () => {
      setSourceMode("adb");
      void refreshDevices();
    };
    window.addEventListener("lf:open-file", openListener);
    window.addEventListener("lf:focus-search", focusSearch);
    window.addEventListener("lf:focus-jump", focusJump);
    window.addEventListener("lf:focus-device", focusDevice);
    return () => {
      window.removeEventListener("lf:open-file", openListener);
      window.removeEventListener("lf:focus-search", focusSearch);
      window.removeEventListener("lf:focus-jump", focusJump);
      window.removeEventListener("lf:focus-device", focusDevice);
    };
  }, [onOpen, refreshDevices, setSourceMode]);

  const countLabel = search.query ? `${currentSearchLine ?? "-"} / ${searchCount}` : "0 / 0";
  const selectedBuffer = logcatBuffers[0] ?? "main";

  return (
    <>
      <div className="lf-toolbar">
        <div className="lf-toolbar-row lf-toolbar-row-top">
          <label
            aria-label="Source"
            className="lf-select-button lf-select-control"
            data-tooltip="Source file"
            data-tooltip-placement="bottom"
          >
            <FolderOpen />
            <select value={sourceMode} onChange={(event) => onSourceChange(event.target.value)}>
              <option value="file">来源: 文件</option>
              <option value="adb">来源: ADB</option>
              {appConfig.recentFiles.length > 0 && (
                <optgroup label="最近文件">
                  {appConfig.recentFiles.map((path) => (
                    <option key={path} value={`recent:${path}`}>
                      {fileNameFromPath(path)}
                    </option>
                  ))}
                </optgroup>
              )}
            </select>
            <ChevronDown />
          </label>
          <label
            aria-label="Current device"
            className="lf-select-button lf-select-control lf-device"
            data-tooltip="Current source"
            data-tooltip-placement="bottom"
          >
            <span className="lf-device-dot" />
            <select
              value={selectedDeviceSerial ?? ""}
              disabled={sourceMode !== "adb"}
              onChange={(event) => setSelectedDeviceSerial(event.target.value || null)}
            >
              {sourceMode !== "adb" ? (
                <option value="">本地日志文件</option>
              ) : devices.length ? (
                devices.map((device) => (
                  <option disabled={!device.online} key={device.serial} value={device.serial}>
                    {device.model ?? device.serial} · {device.state}
                  </option>
                ))
              ) : (
                <option value="">无在线设备</option>
              )}
            </select>
            <ChevronDown />
          </label>
          <label
            aria-label="Logcat command"
            className="lf-select-button lf-select-control lf-command"
            data-tooltip="Logcat command"
            data-tooltip-placement="bottom"
          >
            <span>命令</span>
            <select
              value={selectedBuffer}
              onChange={(event) => {
                const buffers = [event.target.value as LogcatBuffer];
                setLogcatBuffers(buffers);
                void saveAppConfig({ ...appConfig, commandBuffers: buffers })
                  .then(setAppConfig)
                  .catch((err) => console.error("save command buffers failed", err));
              }}
            >
              {LOGCAT_BUFFERS.map((buffer) => (
                <option key={buffer.value} value={buffer.value}>
                  logcat -v threadtime -b {buffer.label}
                </option>
              ))}
            </select>
            <ChevronDown />
          </label>
        </div>

        <div className="lf-toolbar-row lf-toolbar-row-actions">
          <Button
            aria-label={streamPaused ? "Resume" : "Start"}
            className="lf-run-button"
            data-tooltip={streamPaused ? "Resume" : "Start"}
            disabled={streamRunning}
            size="icon-sm"
            onClick={runCapture}
          >
            <Play />
          </Button>
          <Button
            aria-label="Pause"
            data-tooltip="Pause"
            disabled={!streamRunning}
            size="icon-sm"
            variant="ghost"
            onClick={pauseCapture}
          >
            <Pause />
          </Button>
          <Button
            aria-label="Stop"
            data-tooltip="Stop"
            disabled={!streamRunning && !streamPaused}
            size="icon-sm"
            variant="ghost"
            onClick={stopCapture}
          >
            <Square />
          </Button>
          <Button
            aria-label="Clear"
            data-tooltip="Clear"
            disabled={
              sourceMode !== "adb" || (!streamRunning && !streamPaused && !status.totalBytes)
            }
            size="icon-sm"
            variant="ghost"
            onClick={clearCapture}
          >
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
              ref={searchInputRef}
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
          <div className="lf-jump-box">
            <input
              ref={jumpInputRef}
              min={1}
              type="number"
              value={jumpLine}
              placeholder="行号"
              onChange={(event) => setJumpLine(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void jumpToLine();
              }}
            />
            <button type="button" onClick={() => void jumpToLine()}>
              跳转
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
          <div className="lf-highlight-fields">
            {filter.highlights.map((rule, index) => (
              <label
                className="lf-filter-field lf-highlight-field"
                data-enabled={rule.enabled}
                key={index}
              >
                <button
                  className="lf-switch"
                  data-active={rule.enabled}
                  type="button"
                  onClick={(event) => {
                    event.preventDefault();
                    setHighlightRule(index, { enabled: !rule.enabled });
                  }}
                >
                  <span />
                </button>
                <span className="lf-filter-label">高亮 {index + 1}</span>
                <input
                  value={rule.pattern}
                  placeholder="keyword"
                  onChange={(event) => setHighlightRule(index, { pattern: event.target.value })}
                />
                <button
                  aria-label={`高亮 ${index + 1} 正则`}
                  className="lf-mini-toggle"
                  data-active={rule.regex}
                  data-tooltip="Regex highlight"
                  type="button"
                  onClick={(event) => {
                    event.preventDefault();
                    setHighlightRule(index, { regex: !rule.regex });
                  }}
                >
                  .*
                </button>
                <button
                  aria-label={`高亮 ${index + 1} 大小写`}
                  className="lf-mini-toggle"
                  data-active={rule.caseSensitive}
                  data-tooltip="Case sensitive"
                  type="button"
                  onClick={(event) => {
                    event.preventDefault();
                    setHighlightRule(index, { caseSensitive: !rule.caseSensitive });
                  }}
                >
                  Aa
                </button>
                <select
                  className="lf-color-select"
                  value={rule.color}
                  onChange={(event) => setHighlightRule(index, { color: event.target.value })}
                >
                  {HIGHLIGHT_COLORS.map((color) => (
                    <option key={color} value={color}>
                      {color}
                    </option>
                  ))}
                </select>
                <span className="lf-highlight-color" data-color={rule.color} />
              </label>
            ))}
          </div>
        </div>
      </div>
      {dialog === "export" && <ExportDialog onClose={() => setDialog(null)} />}
      {dialog === "split" && <SplitDialog onClose={() => setDialog(null)} />}
      {dialog === "settings" && <SettingsDialog onClose={() => setDialog(null)} />}
    </>
  );
}
