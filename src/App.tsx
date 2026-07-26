import { type CSSProperties, useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { Minimap } from "@/components/Minimap";
import { ProblemsDock } from "@/components/ProblemsDock";
import { Toast, type ToastState } from "@/components/Toast";
import {
  getConfig,
  nextBookmark,
  onExportProgress,
  onFilterDone,
  onIndexProgress,
  onSearchProgress,
  onStreamAppend,
  saveAppConfig,
  setFilter as setFilterCommand,
} from "@/lib/ipc";
import { createStreamAppendBatcher } from "@/lib/streamAppend";
import { useSession } from "@/store/session";

export default function App() {
  const [configReady, setConfigReady] = useState(false);
  const [toast, setToast] = useState<ToastState | null>(null);
  const toastSeqRef = useRef(0);
  const dismissToast = useCallback(() => setToast(null), []);
  const setStatus = useSession((s) => s.setStatus);
  const filter = useSession((s) => s.filter);
  const setFilterState = useSession((s) => s.setFilter);
  const filterRevision = useSession((s) => s.filterRevision);
  const sessionId = useSession((s) => s.sessionId);
  const hasFile = useSession((s) => s.status.totalBytes > 0);
  const setFilteredLines = useSession((s) => s.setFilteredLines);
  const setSearchResult = useSession((s) => s.setSearchResult);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const selectedLine = useSession((s) => s.selectedLine);
  const navigateToResultIndex = useSession((s) => s.navigateToResultIndex);
  const requestTailFollow = useSession((s) => s.requestTailFollow);
  const pauseTailFollowing = useSession((s) => s.pauseTailFollowing);
  const appConfig = useSession((s) => s.appConfig);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const setLogcatBuffers = useSession((s) => s.setLogcatBuffers);
  const theme = useSession((s) => s.theme);
  const bookmarkSensitiveRevision = filter.markedOnly ? bookmarkRevision : 0;
  const appConfigRef = useRef(appConfig);
  const dispatchedFilterRequestRef = useRef(0);
  const appliedFilterRequestRef = useRef(0);

  useEffect(() => {
    appConfigRef.current = appConfig;
  }, [appConfig]);

  useEffect(() => {
    getConfig()
      .then((config) => {
        setAppConfig(config);
        if (config.lastFilter) setFilterState(config.lastFilter);
        setLogcatBuffers(config.commandBuffers);
        void getCurrentWindow().setSize(new LogicalSize(config.window.width, config.window.height));
        setConfigReady(true);
      })
      .catch((err) => {
        console.error("get_config failed", err);
        setConfigReady(true);
      });
  }, [setAppConfig, setFilterState, setLogcatBuffers]);

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
      const dispatchedRequest = dispatchedFilterRequestRef.current;
      const invalidateRows = dispatchedRequest !== appliedFilterRequestRef.current;
      setFilteredLines(done.filteredLines, { invalidateRows });
      appliedFilterRequestRef.current = dispatchedRequest;
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

  // 导出完成/取消的全局提示:即使导出对话框已关闭也能收到(对话框自身仍监听进度做内联显示)。
  useEffect(() => {
    const un = onExportProgress((progress) => {
      if (!progress.done) return;
      toastSeqRef.current += 1;
      if (progress.cancelled) {
        setToast({ id: toastSeqRef.current, message: "导出已取消", tone: "info" });
        return;
      }
      const path = progress.path;
      setToast({
        id: toastSeqRef.current,
        message: `已导出 ${progress.writtenLines.toLocaleString()} 行`,
        tone: "success",
        action: path
          ? {
              label: "打开所在目录",
              onClick: () => {
                void revealItemInDir(path).catch((err) =>
                  console.error("revealItemInDir failed", err),
                );
              },
            }
          : undefined,
      });
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const appendBatcher = createStreamAppendBatcher({
      onFlush: (append) => {
        const state = useSession.getState();
        if (append.status.generation < state.status.generation) return;
        setStatus(append.status);
        if (state.sourceMode === "adb" && state.tailFollowing && append.status.filteredLines > 0) {
          requestTailFollow(append.status.filteredLines - 1);
        }
      },
    });
    const un = onStreamAppend((append) => {
      appendBatcher.push(append);
    });
    return () => {
      appendBatcher.dispose();
      un.then((f) => f());
    };
  }, [requestTailFollow, setStatus]);

  useEffect(() => {
    if (!hasFile) return;
    const timer = window.setTimeout(() => {
      dispatchedFilterRequestRef.current += 1;
      void setFilterCommand(filter).catch((err) => {
        console.error("set_filter failed", err);
      });
    }, 220);
    return () => window.clearTimeout(timer);
  }, [filter, filterRevision, bookmarkSensitiveRevision, hasFile, sessionId]);

  useEffect(() => {
    if (!configReady) return;
    const timer = window.setTimeout(() => {
      const nextConfig = {
        ...appConfigRef.current,
        lastFilter: filter,
      };
      void saveAppConfig(nextConfig)
        .then(setAppConfig)
        .catch((err) => console.error("save last filter failed", err));
    }, 600);
    return () => window.clearTimeout(timer);
  }, [configReady, filter, filterRevision, setAppConfig]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.metaKey && !event.ctrlKey) return;
      const key = event.key.toLowerCase();
      if (key === "o" || key === "f" || key === "g") {
        event.preventDefault();
      }
      if (key === "o") window.dispatchEvent(new Event("lf:open-file"));
      if (key === "f") window.dispatchEvent(new Event("lf:focus-search"));
      if (key === "g") window.dispatchEvent(new Event("lf:focus-jump"));
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    const onResize = () => {
      if (!configReady) return;
      const nextConfig = {
        ...appConfigRef.current,
        window: {
          width: Math.round(window.innerWidth),
          height: Math.round(window.innerHeight),
        },
      };
      void saveAppConfig(nextConfig)
        .then(setAppConfig)
        .catch((err) => console.error("save window size failed", err));
    };
    const debounced = () => {
      window.clearTimeout((debounced as { timer?: number }).timer);
      (debounced as { timer?: number }).timer = window.setTimeout(onResize, 500);
    };
    window.addEventListener("resize", debounced);
    return () => window.removeEventListener("resize", debounced);
  }, [configReady, setAppConfig]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "F2" && event.key !== "F3") return;
      event.preventDefault();
      const direction = event.key === "F2" ? "previous" : "next";
      nextBookmark(selectedLine ?? 1, direction).then((target) => {
        if (target) {
          pauseTailFollowing("bookmark");
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
  }, [navigateToResultIndex, pauseTailFollowing, selectedLine]);

  const appStyle = {
    "--lf-font-size": `${appConfig.fontSize}px`,
    "--lf-row-height": `${appConfig.rowHeight}px`,
  } as CSSProperties;

  return (
    <div className={`lf-app lf-theme-${theme}`} style={appStyle}>
      <Toolbar />
      <div className="lf-workbench">
        <div className="lf-main">
          <Minimap />
          <LogTable />
        </div>
        <ProblemsDock />
      </div>
      <StatusBar />
      <Toast toast={toast} onDismiss={dismissToast} />
    </div>
  );
}
