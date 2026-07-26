import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
import { FilterBar } from "@/components/FilterBar";
import { Button } from "@/components/ui/button";
import { DropdownMenu, type DropdownGroup } from "@/components/ui/dropdown";
import { ExportDialog, SettingsDialog, SplitDialog } from "@/components/ToolDialogs";
import {
  clearLogcat,
  listDevices,
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
import {
  DEFAULT_LOGCAT_COMMANDS,
  normalizeCommandPresets,
  parseLogcatCommand,
} from "@/lib/logcatCommand";
import { LOG_LEVELS } from "@/lib/filterDefinitions";
import type { TableScopeController } from "@/lib/tableScopeController";
import { ALL_LEVELS, useSession } from "@/store/session";
import type { ThemeMode } from "@/types";

interface ToolbarProps {
  tableController: TableScopeController;
}

export function Toolbar({ tableController }: ToolbarProps) {
  const [dialog, setDialog] = useState<"export" | "split" | "settings" | null>(null);
  const [jumpLine, setJumpLine] = useState("");
  const searchInputRef = useRef<HTMLInputElement>(null);
  const jumpInputRef = useRef<HTMLInputElement>(null);
  const beginSession = useSession((s) => s.beginSession);
  const status = useSession((s) => s.status);
  const sourceMode = useSession((s) => s.sourceMode);
  const devices = useSession((s) => s.devices);
  const setDevices = useSession((s) => s.setDevices);
  const selectedDeviceSerial = useSession((s) => s.selectedDeviceSerial);
  const setSelectedDeviceSerial = useSession((s) => s.setSelectedDeviceSerial);
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
  const search = useSession((s) => s.search);
  const searchRevision = useSession((s) => s.searchRevision);
  const setSearch = useSession((s) => s.setSearch);
  const searchCount = useSession((s) => s.searchCount);
  const currentSearchLine = useSession((s) => s.currentSearchLine);
  const setSearchResult = useSession((s) => s.setSearchResult);
  const setCurrentSearchLine = useSession((s) => s.setCurrentSearchLine);
  const pauseTailFollowing = useSession((s) => s.pauseTailFollowing);
  const [commandDraft, setCommandDraft] = useState(
    appConfig.currentCommand || DEFAULT_LOGCAT_COMMANDS[0],
  );
  const [commandError, setCommandError] = useState("");

  const commandPresets = useMemo(
    () => normalizeCommandPresets(appConfig.commandPresets ?? []),
    [appConfig.commandPresets],
  );
  const selectedDevice = devices.find((device) => device.serial === selectedDeviceSerial);
  const selectedDeviceLabel = selectedDevice
    ? `${selectedDevice.model ?? selectedDevice.serial} · ${selectedDevice.state}`
    : devices.length
      ? "选择设备"
      : "无在线设备";
  const deviceGroups: Array<DropdownGroup<string>> = useMemo(
    () => [
      {
        items: devices.map((device) => ({
          value: device.serial,
          label: `${device.model ?? device.serial} · ${device.state}`,
          checked: device.serial === selectedDeviceSerial,
          disabled: !device.online,
          leading: <span className="lf-device-dot" data-online={device.online || undefined} />,
          shortcut: device.serial,
        })),
      },
    ],
    [devices, selectedDeviceSerial],
  );
  const commandGroups: Array<DropdownGroup<string>> = useMemo(() => {
    const parsedDraft = parseLogcatCommand(commandDraft);
    const selectedCommand = parsedDraft.ok ? parsedDraft.normalized : commandDraft;
    return [
      {
        label: "预设命令",
        items: commandPresets.map((command) => {
          const parsed = parseLogcatCommand(command);
          const normalized = parsed.ok ? parsed.normalized : command;
          return {
            value: command,
            label: command,
            checked: normalized === selectedCommand,
          };
        }),
      },
    ];
  }, [commandDraft, commandPresets]);

  useEffect(() => {
    setCommandDraft(appConfig.currentCommand || DEFAULT_LOGCAT_COMMANDS[0]);
  }, [appConfig.currentCommand]);

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

  const refreshInflightRef = useRef(false);
  const refreshDevices = useCallback(async () => {
    if (refreshInflightRef.current) return;
    refreshInflightRef.current = true;
    try {
      const result = await listDevices();
      setDevices(result.devices);
    } catch (err) {
      console.error("list_devices failed", err);
      setDevices([]);
    } finally {
      refreshInflightRef.current = false;
    }
  }, [setDevices]);

  useEffect(() => {
    void refreshDevices();
    const timer = window.setInterval(() => {
      void refreshDevices();
    }, 4000);
    return () => window.clearInterval(timer);
  }, [refreshDevices]);

  const persistCommand = useCallback(
    async (command: string, presets = appConfig.commandPresets) => {
      const parsed = parseLogcatCommand(command);
      if (!parsed.ok) {
        setCommandError(parsed.error);
        return null;
      }
      const commandPresets = normalizeCommandPresets([...presets, parsed.normalized]);
      const nextConfig = {
        ...appConfig,
        commandBuffers: parsed.buffers,
        currentCommand: parsed.normalized,
        commandPresets,
      };
      const saved = await saveAppConfig(nextConfig);
      setCommandDraft(parsed.normalized);
      setLogcatBuffers(parsed.buffers);
      setAppConfig(saved);
      setCommandError("");
      return saved;
    },
    [appConfig, setAppConfig, setLogcatBuffers],
  );

  const commitCommandDraft = useCallback(async () => {
    const command = commandDraft.trim();
    if (!command) {
      setCommandError("命令不能为空");
      return null;
    }
    return persistCommand(command);
  }, [commandDraft, persistCommand]);

  const runCapture = useCallback(async () => {
    try {
      if (streamPaused) {
        const control = await resumeLogcat();
        setStreamControl(control);
        return;
      }
      const parsed = parseLogcatCommand(commandDraft);
      if (!parsed.ok) {
        setCommandError(parsed.error);
        return;
      }
      const control = await startLogcat({
        deviceSerial: selectedDeviceSerial,
        command: parsed.normalized,
        buffers: parsed.buffers,
      });
      beginSession(control.status, control.sessionPath, "adb");
      setStreamControl(control);
      await persistCommand(parsed.normalized);
    } catch (err) {
      console.error("start/resume logcat failed", err);
    }
  }, [
    beginSession,
    commandDraft,
    persistCommand,
    selectedDeviceSerial,
    setStreamControl,
    streamPaused,
  ]);

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
      beginSession(control.status, control.sessionPath, "adb");
      setStreamControl(control);
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
      void searchLogs(search, searchRevision).catch((err) => {
        console.error("clear search failed", err);
      });
      return;
    }
    const timer = window.setTimeout(() => {
      void searchLogs(search, searchRevision).catch((err) => {
        console.error("search failed", err);
        setSearchResult(0, null);
      });
    }, 180);
    return () => window.clearTimeout(timer);
  }, [search, searchRevision, status.totalBytes, setSearchResult]);

  const jumpSearch = useCallback(
    async (direction: "next" | "previous") => {
      if (!searchCount) return;
      const from = currentSearchLine ?? 0;
      const line = await searchNext(from, direction);
      setCurrentSearchLine(line);
      if (line != null) {
        await tableController.navigateToSourceLine(line, "search");
      }
    },
    [currentSearchLine, searchCount, setCurrentSearchLine, tableController],
  );

  const jumpToLine = useCallback(async () => {
    const lineNo = Number(jumpLine);
    if (!Number.isFinite(lineNo) || lineNo < 1) return;
    await tableController.navigateToSourceLine(lineNo, "jump");
  }, [jumpLine, tableController]);

  useEffect(() => {
    const openListener = () => void onOpen();
    const startCapture = () => void runCapture();
    const focusSearch = () => {
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
    };
    const focusJump = () => {
      jumpInputRef.current?.focus();
      jumpInputRef.current?.select();
    };
    const focusDevice = () => {
      void refreshDevices();
    };
    window.addEventListener("lf:open-file", openListener);
    window.addEventListener("lf:start-capture", startCapture);
    window.addEventListener("lf:focus-search", focusSearch);
    window.addEventListener("lf:focus-jump", focusJump);
    window.addEventListener("lf:focus-device", focusDevice);
    return () => {
      window.removeEventListener("lf:open-file", openListener);
      window.removeEventListener("lf:start-capture", startCapture);
      window.removeEventListener("lf:focus-search", focusSearch);
      window.removeEventListener("lf:focus-jump", focusJump);
      window.removeEventListener("lf:focus-device", focusDevice);
    };
  }, [onOpen, refreshDevices, runCapture]);

  const countLabel = search.query ? `${currentSearchLine ?? "-"} / ${searchCount}` : "0 / 0";

  return (
    <>
      <div className="lf-toolbar">
        <div className="lf-toolbar-row lf-toolbar-row-top">
          <DropdownMenu
            className="lf-top-dropdown lf-device"
            disabled={devices.length === 0}
            emptyText="无在线设备"
            groups={deviceGroups}
            menuLabel="当前设备"
            onSelect={(value) => setSelectedDeviceSerial(value)}
            trigger={() => (
              <>
                <span className="lf-device-dot" data-online={selectedDevice?.online || undefined} />
                <span>{selectedDeviceLabel}</span>
                <ChevronDown />
              </>
            )}
          />
          <div
            className="lf-command-combobox"
            data-invalid={commandError ? true : undefined}
            title={commandError || commandDraft}
          >
            <span>命令</span>
            <input
              aria-label="Logcat command"
              value={commandDraft}
              onBlur={() => void commitCommandDraft()}
              onChange={(event) => {
                setCommandDraft(event.target.value);
                setCommandError("");
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void commitCommandDraft();
                }
                if (event.key === "Escape") {
                  setCommandDraft(appConfig.currentCommand || DEFAULT_LOGCAT_COMMANDS[0]);
                  setCommandError("");
                }
              }}
            />
            <DropdownMenu
              className="lf-command-preset"
              groups={commandGroups}
              menuLabel="命令预设"
              onSelect={(value) => {
                setCommandDraft(value);
                setCommandError("");
                void persistCommand(value);
              }}
              trigger={() => <ChevronDown />}
            />
          </div>
        </div>

        <div className="lf-toolbar-row lf-toolbar-row-actions">
          <Button
            aria-label={streamPaused ? "Resume" : "Start"}
            className="lf-run-button"
            data-tooltip={streamPaused ? "Resume" : "Start"}
            disabled={streamRunning && !streamPaused}
            size="icon-sm"
            onClick={runCapture}
          >
            <Play />
          </Button>
          <Button
            aria-label="Pause"
            data-tooltip="Pause"
            disabled={!streamRunning || streamPaused}
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
          <DropdownMenu
            className="lf-toolbar-menu"
            groups={[
              {
                items: [{ value: "open", label: "打开文件..." }],
              },
              ...(appConfig.recentFiles.length
                ? [
                    {
                      label: "最近文件",
                      items: appConfig.recentFiles.map((path) => ({
                        value: `recent:${path}`,
                        label: (
                          <span className="lf-recent-file" title={path}>
                            {fileNameFromPath(path)}
                          </span>
                        ),
                        shortcut: path.replace(/^\/(?:Users|home)\/[^/]+/, "~"),
                      })),
                    },
                  ]
                : []),
            ]}
            menuLabel="打开文件"
            triggerTooltip="Open file"
            onSelect={(value) => {
              if (value === "open") {
                void onOpen();
                return;
              }
              if (value.startsWith("recent:")) {
                void openPath(value.slice("recent:".length));
              }
            }}
            trigger={() => <FolderOpen />}
          />
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
              aria-pressed={filter.levels === ALL_LEVELS}
              className="lf-level-chip lf-level-all"
              data-active={filter.levels === ALL_LEVELS}
              data-tooltip="All levels"
              type="button"
              onClick={() => setFilter({ levels: ALL_LEVELS })}
            >
              <b>全部</b>
            </button>
            {LOG_LEVELS.map(({ label: level, bit, tooltip }) => {
              const on = (filter.levels & bit) !== 0;
              return (
                <button
                  aria-label={tooltip}
                  aria-pressed={on}
                  key={level}
                  className="lf-level-chip"
                  data-level={level}
                  data-active={on}
                  data-tooltip={tooltip}
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
              aria-pressed={filter.markedOnly}
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
              onChange={(e) => {
                pauseTailFollowing("search");
                setSearch({ query: e.target.value });
              }}
              placeholder="查找日志…"
            />
            <span className="lf-search-count">{countLabel}</span>
            <button
              aria-label="Case sensitive"
              className="lf-mini-toggle"
              data-active={search.caseSensitive}
              data-tooltip="Case sensitive"
              type="button"
              onClick={() => {
                pauseTailFollowing("search");
                setSearch({ caseSensitive: !search.caseSensitive });
              }}
            >
              Aa
            </button>
            <button
              aria-label="Regex search"
              className="lf-mini-toggle"
              data-active={search.regex}
              data-tooltip="Regex search"
              type="button"
              onClick={() => {
                pauseTailFollowing("search");
                setSearch({ regex: !search.regex });
              }}
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

        <FilterBar />
      </div>
      {dialog === "export" && <ExportDialog onClose={() => setDialog(null)} />}
      {dialog === "split" && <SplitDialog onClose={() => setDialog(null)} />}
      {dialog === "settings" && <SettingsDialog onClose={() => setDialog(null)} />}
    </>
  );
}
