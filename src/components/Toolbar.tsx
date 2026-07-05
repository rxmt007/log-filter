import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
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
  const toggleLevel = useSession((s) => s.toggleLevel);
  const setFilterField = useSession((s) => s.setFilterField);
  const view = useSession((s) => s.view);
  const setView = useSession((s) => s.setView);
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
    if (!status.totalBytes || !search.query.trim()) {
      setSearchResult(0, null);
      return;
    }
    const timer = window.setTimeout(() => {
      searchLogs(search)
        .then((result) => setSearchResult(result.count, result.firstLine))
        .catch((err) => {
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
          <button className="lf-select-button" type="button">
            <FolderOpen />
            <span>来源:文件</span>
            <ChevronDown />
          </button>
          <button className="lf-select-button lf-device" type="button">
            <span className="lf-device-dot" />
            <span>{status.totalBytes ? "本地日志文件" : "无设备"}</span>
            <ChevronDown />
          </button>
          <button className="lf-select-button lf-command" type="button">
            <span>命令</span>
            <code>logcat -v threadtime</code>
            <ChevronDown />
          </button>
        </div>

      <div className="lf-toolbar-row lf-toolbar-row-actions">
        <Button size="icon-sm" className="lf-run-button" title="运行">
          <Play />
        </Button>
        <Button size="icon-sm" variant="ghost" title="暂停">
          <Pause />
        </Button>
        <Button size="icon-sm" variant="ghost" title="停止">
          <Square />
        </Button>
        <Button size="icon-sm" variant="ghost" title="清空">
          <Trash2 />
        </Button>
        <span className="lf-separator" />
        <Button size="icon-sm" variant="ghost" title="打开" onClick={onOpen}>
          <FolderOpen />
        </Button>
        <Button size="icon-sm" variant="ghost" title="导出" onClick={() => setDialog("export")}>
          <Download />
        </Button>
        <Button size="icon-sm" variant="ghost" title="切分" onClick={() => setDialog("split")}>
          <Split />
        </Button>
        <Button size="icon-sm" variant="ghost" title="设置" onClick={() => setDialog("settings")}>
          <Settings />
        </Button>
        <Button size="icon-sm" variant="ghost" title="主题" onClick={toggleTheme}>
          {theme === "dark" ? <Sun /> : <Moon />}
        </Button>
        <span className="lf-separator" />
        <span className="lf-level-label">级别</span>
        <div className="lf-level-chips">
          {LEVELS.map(([level, bit]) => {
            const on = (filter.levels & bit) !== 0;
            return (
              <button
                key={level}
                className="lf-level-chip"
                data-level={level}
                data-active={on}
                type="button"
                onClick={() => toggleLevel(bit)}
              >
                <span />
                <b>{level}</b>
              </button>
            );
          })}
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
            className="lf-mini-toggle"
            data-active={search.caseSensitive}
            type="button"
            title="区分大小写"
            onClick={() => setSearch({ caseSensitive: !search.caseSensitive })}
          >
            Aa
          </button>
          <button
            className="lf-mini-toggle"
            data-active={search.regex}
            type="button"
            title="正则匹配"
            onClick={() => setSearch({ regex: !search.regex })}
          >
            .*
          </button>
          <span className="lf-highlight-swatch" title="高亮颜色" />
          <span className="lf-search-divider" />
          <button type="button" title="上一处" onClick={() => jumpSearch("previous")}>
            <ChevronUp />
          </button>
          <button type="button" title="下一处" onClick={() => jumpSearch("next")}>
            <ChevronDown />
          </button>
        </div>
      </div>

      <div className="lf-filter-bar">
        <div className="lf-filter-title">
          <span>过滤条件</span>
          <button
            className="lf-view-toggle"
            data-active={view === "all"}
            type="button"
            onClick={() => setView("all")}
          >
            全部
          </button>
          <button
            className="lf-view-toggle"
            data-active={view === "filtered"}
            type="button"
            onClick={() => setView("filtered")}
          >
            过滤
          </button>
          <button
            className="lf-view-toggle"
            data-active={view === "bookmarks"}
            type="button"
            onClick={() => setView("bookmarks")}
          >
            书签
          </button>
          <button
            className="lf-view-toggle"
            data-active={view === "errors"}
            type="button"
            onClick={() => setView("errors")}
          >
            错误
          </button>
          <button
            className="lf-view-toggle"
            data-active={filter.levels === ALL_LEVELS}
            type="button"
            onClick={() => useSession.getState().setFilter({ levels: ALL_LEVELS })}
          >
            全级别
          </button>
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
                    className="lf-mini-toggle"
                    data-active={value.regex}
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
