import { type CSSProperties, useEffect } from "react";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { Minimap } from "@/components/Minimap";
import {
  getConfig,
  nextBookmark,
  onFilterDone,
  onIndexProgress,
  onSearchProgress,
  setFilter,
} from "@/lib/ipc";
import { useSession } from "@/store/session";

export default function App() {
  const setStatus = useSession((s) => s.setStatus);
  const filter = useSession((s) => s.filter);
  const filterRevision = useSession((s) => s.filterRevision);
  const sessionId = useSession((s) => s.sessionId);
  const hasFile = useSession((s) => s.status.totalBytes > 0);
  const setFilteredLines = useSession((s) => s.setFilteredLines);
  const setSearchResult = useSession((s) => s.setSearchResult);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const selectedLine = useSession((s) => s.selectedLine);
  const navigateToResultIndex = useSession((s) => s.navigateToResultIndex);
  const appConfig = useSession((s) => s.appConfig);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const theme = useSession((s) => s.theme);
  const bookmarkSensitiveRevision = filter.markedOnly ? bookmarkRevision : 0;

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
    const un = onFilterDone((done) => {
      const state = useSession.getState();
      if (done.generation !== state.status.generation) return;
      setFilteredLines(done.filteredLines);
    });
    return () => {
      un.then((f) => f());
    };
  }, [setFilteredLines]);

  useEffect(() => {
    const un = onSearchProgress((progress) => {
      const state = useSession.getState();
      if (!progress.done || progress.generation !== state.status.generation) return;
      setSearchResult(progress.matches, progress.firstLine);
    });
    return () => {
      un.then((f) => f());
    };
  }, [setSearchResult]);

  useEffect(() => {
    if (!hasFile) return;
    const timer = window.setTimeout(() => {
      void setFilter(filter).catch((err) => {
        console.error("set_filter failed", err);
      });
    }, 220);
    return () => window.clearTimeout(timer);
  }, [filter, filterRevision, bookmarkSensitiveRevision, hasFile, sessionId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "F2" && event.key !== "F3") return;
      event.preventDefault();
      const direction = event.key === "F2" ? "previous" : "next";
      nextBookmark(selectedLine ?? 1, direction).then((target) => {
        if (target) {
          navigateToResultIndex(target.resultIndex, {
            lineNo: target.lineNo,
            align: "center",
            reason: "bookmark",
          });
        }
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [navigateToResultIndex, selectedLine]);

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
