import { type CSSProperties, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { Minimap } from "@/components/Minimap";
import { ProblemsDock } from "@/components/ProblemsDock";
import { ProblemExportDialog } from "@/components/ProblemExportDialog";
import { Toast, type ToastState } from "@/components/Toast";
import {
  getConfig,
  nextBookmark,
  onExportProgress,
  onFilterDone,
  onIndexProgress,
  onSearchProgress,
  onStreamAppend,
  onStreamControl,
  onStreamError,
  saveAppConfig,
  setFilter as setFilterCommand,
} from "@/lib/ipc";
import { createSessionTableScopeController } from "@/lib/sessionTableScope";
import { createStreamAppendBatcher } from "@/lib/streamAppend";
import { useProblemsLive } from "@/hooks/useProblemsLive";
import { useProblems } from "@/store/problems";
import { useSession } from "@/store/session";
import type { AnalysisToken, ProblemOccurrence } from "@/types";

interface OpenProblemExport {
  occurrence: ProblemOccurrence;
  analysisToken: AnalysisToken;
  returnFocus: HTMLElement | null;
}

function restoreProblemOccurrenceFocus(eventId: number): void {
  const option = document.getElementById(`lf-problem-event-${eventId}`);
  const listbox = option?.closest<HTMLElement>('[role="listbox"]');
  if (listbox) {
    listbox.focus();
    return;
  }
  document.querySelector<HTMLButtonElement>(".lf-problems-toggle")?.focus();
}

export default function App() {
  const [configReady, setConfigReady] = useState(false);
  const [toast, setToast] = useState<ToastState | null>(null);
  const [problemExport, setProblemExport] = useState<OpenProblemExport | null>(null);
  const toastSeqRef = useRef(0);
  const problemContextReturnEventRef = useRef<number | null>(null);
  const dismissToast = useCallback(() => setToast(null), []);
  const setStatus = useSession((s) => s.setStatus);
  const filter = useSession((s) => s.filter);
  const setFilterState = useSession((s) => s.setFilter);
  const filterRevision = useSession((s) => s.filterRevision);
  const sessionId = useSession((s) => s.sessionId);
  const hasFile = useSession((s) => s.status.totalBytes > 0);
  const applyFilterDone = useSession((s) => s.applyFilterDone);
  const setSearchResult = useSession((s) => s.setSearchResult);
  const bookmarkRevision = useSession((s) => s.bookmarkRevision);
  const selectedLine = useSession((s) => s.selectedLine);
  const appConfig = useSession((s) => s.appConfig);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const setLogcatBuffers = useSession((s) => s.setLogcatBuffers);
  const theme = useSession((s) => s.theme);
  const problemsBindings = useProblemsLive();
  const bookmarkSensitiveRevision = filter.markedOnly ? bookmarkRevision : 0;
  const appConfigRef = useRef(appConfig);
  const tableController = useMemo(
    () =>
      createSessionTableScopeController((error) => {
        if (error instanceof Error && error.message === "filter-result-wait-cancelled") return;
        console.error("table scope navigation failed", error);
      }),
    [],
  );

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
      applyFilterDone(done);
    });
    return () => {
      un.then((f) => f());
    };
  }, [applyFilterDone]);

  useEffect(() => {
    const un = onSearchProgress((progress) => {
      const state = useSession.getState();
      if (
        !progress.done ||
        progress.generation !== state.status.generation ||
        progress.requestId !== state.searchRevision
      ) {
        return;
      }
      setSearchResult(progress.matches, progress.firstLine);
      if (progress.firstLine != null) {
        void tableController.navigateToSourceLine(progress.firstLine, "search");
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [setSearchResult, tableController]);

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
        if (state.sourceMode === "adb" && state.tailFollowing && append.status.stableLines > 0) {
          void tableController.navigateToSourceLine(append.status.stableLines, "tail");
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
  }, [setStatus, tableController]);

  useEffect(() => {
    const controlUnlisten = onStreamControl((control) => {
      useSession.getState().setStreamControl(control);
      if (control.error) {
        toastSeqRef.current += 1;
        setToast({
          id: toastSeqRef.current,
          message: `日志抓取状态异常：${control.error}`,
          tone: "error",
        });
      }
    });
    const errorUnlisten = onStreamError((error) => {
      toastSeqRef.current += 1;
      setToast({
        id: toastSeqRef.current,
        message: `日志抓取失败：${error}`,
        tone: "error",
      });
    });
    return () => {
      controlUnlisten.then((unlisten) => unlisten());
      errorUnlisten.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    if (!hasFile) return;
    const timer = window.setTimeout(() => {
      void setFilterCommand(filter, filterRevision).catch((err) => {
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
          void tableController.navigateToSourceLine(target.lineNo, "bookmark");
        }
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedLine, tableController]);

  const locateProblem = useCallback(
    async (occurrence: Parameters<typeof tableController.locateProblem>[0]) => {
      const outcome = await tableController.locateProblem(occurrence);
      problemContextReturnEventRef.current =
        outcome === "context-opened" ? occurrence.eventId : null;
    },
    [tableController],
  );

  const openProblemContext = useCallback(
    (occurrence: Parameters<typeof tableController.openProblemContext>[0]) => {
      problemContextReturnEventRef.current = occurrence.eventId;
      tableController.openProblemContext(occurrence);
    },
    [tableController],
  );

  const returnToResults = useCallback(async () => {
    const currentScope = useSession.getState().tableScope;
    const eventId =
      problemContextReturnEventRef.current ??
      (currentScope.kind === "problem-context" ? currentScope.occurrence.eventId : null);
    await tableController.returnToResults();
    if (useSession.getState().tableScope.kind !== "results" || eventId == null) return;
    problemContextReturnEventRef.current = null;
    restoreProblemOccurrenceFocus(eventId);
  }, [tableController]);

  const exportProblem = useCallback((occurrence: ProblemOccurrence) => {
    const analysisToken = useProblems.getState().analysisToken;
    if (!analysisToken) return;
    setProblemExport({
      occurrence,
      analysisToken,
      returnFocus:
        document.activeElement instanceof HTMLElement ? document.activeElement : null,
    });
  }, []);

  const appStyle = {
    "--lf-font-size": `${appConfig.fontSize}px`,
    "--lf-row-height": `${appConfig.rowHeight}px`,
  } as CSSProperties;

  return (
    <div className={`lf-app lf-theme-${theme}`} style={appStyle}>
      <Toolbar tableController={tableController} />
      <div className="lf-workbench">
        <div className="lf-main">
          <Minimap />
          <LogTable
            onReturnToResults={() => void returnToResults()}
            onFollowLatest={(lineNo) =>
              void tableController.navigateToSourceLine(lineNo, "tail")
            }
          />
        </div>
        <ProblemsDock
          {...problemsBindings}
          onLocateOccurrence={(occurrence) => void locateProblem(occurrence)}
          onOpenContext={openProblemContext}
          onExportOccurrence={exportProblem}
        />
      </div>
      <StatusBar />
      <Toast toast={toast} onDismiss={dismissToast} />
      {problemExport ? (
        <ProblemExportDialog
          occurrence={problemExport.occurrence}
          analysisToken={problemExport.analysisToken}
          returnFocus={problemExport.returnFocus}
          onClose={() => setProblemExport(null)}
        />
      ) : null}
    </div>
  );
}
