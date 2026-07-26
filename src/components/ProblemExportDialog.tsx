import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Download } from "lucide-react";
import { Button } from "@/components/ui/button";
import { exportProblemLogs } from "@/lib/ipc";
import type { AnalysisToken, ProblemOccurrence } from "@/types";

interface ProblemExportDialogProps {
  analysisToken: AnalysisToken;
  occurrence: ProblemOccurrence;
  onClose: () => void;
  returnFocus?: HTMLElement | null;
}

type ProblemExportMode = "event-range" | "context";

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function ProblemExportDialog({
  analysisToken,
  occurrence,
  onClose,
  returnFocus = null,
}: ProblemExportDialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const initialFocusRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef(returnFocus);
  const [mode, setMode] = useState<ProblemExportMode>("event-range");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    initialFocusRef.current?.focus();
    return () => {
      const preferred = restoreFocusRef.current;
      const fallback = document.querySelector<HTMLElement>(".lf-problems-toggle");
      const target = preferred?.isConnected ? preferred : fallback;
      queueMicrotask(() => target?.focus());
    };
  }, []);

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;

    const focusable = Array.from(
      dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [],
    ).filter((element) => !element.hasAttribute("disabled"));
    if (focusable.length === 0) {
      event.preventDefault();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  const runExport = async () => {
    setError(null);
    const path = await save({
      title: "导出故障事件原始日志",
      defaultPath: `problem-${occurrence.eventId}.log`,
      filters: [{ name: "Log files", extensions: ["log", "txt"] }],
    });
    if (typeof path !== "string") return;

    setBusy(true);
    try {
      await exportProblemLogs({
        eventId: occurrence.eventId,
        expectedAnalysisToken: analysisToken,
        mode,
        radius: mode === "context" ? 50 : undefined,
        path,
      });
      onClose();
    } catch (cause) {
      setError(String(cause));
      setBusy(false);
    }
  };

  return (
    <div className="lf-modal-backdrop" role="presentation">
      <div
        ref={dialogRef}
        className="lf-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="导出故障事件原始日志"
        onKeyDown={handleKeyDown}
      >
        <div className="lf-dialog-header">
          <div>
            <h2>导出故障事件原始日志</h2>
            <p>
              事件范围：第 {occurrence.startLine.toLocaleString()}–
              {occurrence.endLine.toLocaleString()} 行
            </p>
          </div>
        </div>

        <div className="lf-dialog-body">
          <div className="lf-segmented" aria-label="导出范围">
            <button
              ref={initialFocusRef}
              type="button"
              aria-pressed={mode === "event-range"}
              data-active={mode === "event-range"}
              onClick={() => setMode("event-range")}
            >
              事件范围
            </button>
            <button
              type="button"
              aria-pressed={mode === "context"}
              data-active={mode === "context"}
              onClick={() => setMode("context")}
            >
              ±50 行上下文
            </button>
          </div>

          <div className="lf-problem-export-option" data-active={mode === "event-range"}>
            <strong>事件范围</strong>
            <span>
              仅导出检测器记录的第 {occurrence.startLine.toLocaleString()}–
              {occurrence.endLine.toLocaleString()} 行。
            </span>
          </div>
          <div className="lf-problem-export-option" data-active={mode === "context"}>
            <strong>上下文：事件范围外各增加 50 行</strong>
            <span>范围会在日志实际边界内钳制，便于离线复核前后事件。</span>
          </div>

          <p className="lf-dialog-note">
            导出内容是源文件中的原始日志，不加入检测事实、排查提示或根因判断。
          </p>
          {error ? (
            <div className="lf-dialog-status" data-tone="error" role="alert">
              导出失败：{error}
            </div>
          ) : null}
        </div>

        <div className="lf-dialog-footer">
          <Button type="button" variant="ghost" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button
            type="button"
            className="lf-dialog-primary"
            disabled={busy}
            onClick={() => void runExport()}
          >
            <Download aria-hidden="true" />
            {busy ? "正在导出…" : "选择位置并导出"}
          </Button>
        </div>
      </div>
    </div>
  );
}
