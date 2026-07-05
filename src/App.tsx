import { type CSSProperties, useEffect } from "react";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { Minimap } from "@/components/Minimap";
import { getConfig, nextBookmark, onIndexProgress, setFilter } from "@/lib/ipc";
import { ALL_LEVELS, useSession } from "@/store/session";
import type { FilterSpec } from "@/types";

function isFilterSpecActive(filter: FilterSpec) {
  const fieldActive = (field: FilterSpec["pid"]) =>
    field.enabled && field.pattern.trim().length > 0;
  return (
    filter.levels !== ALL_LEVELS ||
    fieldActive(filter.pid) ||
    fieldActive(filter.tid) ||
    fieldActive(filter.tagInclude) ||
    fieldActive(filter.tagExclude) ||
    fieldActive(filter.wordInclude) ||
    fieldActive(filter.wordExclude)
  );
}

export default function App() {
  const setStatus = useSession((s) => s.setStatus);
  const filter = useSession((s) => s.filter);
  const filterRevision = useSession((s) => s.filterRevision);
  const sessionId = useSession((s) => s.sessionId);
  const hasFile = useSession((s) => s.status.totalBytes > 0);
  const setFilteredLines = useSession((s) => s.setFilteredLines);
  const selectedLine = useSession((s) => s.selectedLine);
  const setSelectedLine = useSession((s) => s.setSelectedLine);
  const setView = useSession((s) => s.setView);
  const appConfig = useSession((s) => s.appConfig);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const theme = useSession((s) => s.theme);

  useEffect(() => {
    getConfig()
      .then(setAppConfig)
      .catch((err) => console.error("get_config failed", err));
  }, [setAppConfig]);

  useEffect(() => {
    const un = onIndexProgress(setStatus);
    return () => {
      un.then((f) => f());
    };
  }, [setStatus]);

  useEffect(() => {
    if (!hasFile) return;
    const timer = window.setTimeout(() => {
      const requestedRevision = filterRevision;
      const filterActive = isFilterSpecActive(filter);
      setFilter(filter)
        .then((count) => {
          if (useSession.getState().filterRevision !== requestedRevision) return;
          setFilteredLines(count);
          const currentView = useSession.getState().view;
          if (filterActive && currentView !== "bookmarks" && currentView !== "errors") {
            setView("filtered");
          } else if (!filterActive && currentView === "filtered") {
            setView("all");
          }
        })
        .catch((err) => {
          console.error("set_filter failed", err);
        });
    }, 220);
    return () => window.clearTimeout(timer);
  }, [filter, filterRevision, hasFile, sessionId, setFilteredLines, setView]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "F2" && event.key !== "F3") return;
      event.preventDefault();
      const direction = event.key === "F2" ? "previous" : "next";
      nextBookmark(selectedLine ?? 1, direction).then((line) => {
        if (line) {
          setView("all");
          setSelectedLine(line);
        }
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedLine, setSelectedLine, setView]);

  const appStyle = {
    "--lf-font-size": `${appConfig.fontSize}px`,
    "--lf-row-height": `${appConfig.rowHeight}px`,
  } as CSSProperties;

  return (
    <div className={`lf-app lf-theme-${theme}`} style={appStyle}>
      <Toolbar />
      <div className="lf-main">
        <Minimap />
        <LogTable />
      </div>
      <StatusBar />
    </div>
  );
}
