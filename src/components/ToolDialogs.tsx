import { useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Save, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SelectField } from "@/components/ui/dropdown";
import {
  exportLogs,
  onExportProgress,
  onSplitProgress,
  saveAppConfig,
  splitLogFile,
} from "@/lib/ipc";
import { useSession } from "@/store/session";
import type { AppConfig, ExportProgress, RowsView, SplitProgress } from "@/types";

interface DialogProps {
  onClose: () => void;
}

const VIEW_LABELS: Record<RowsView, string> = {
  all: "全部日志",
  filtered: "当前结果",
  bookmarks: "已标记",
  errors: "错误/Fatal",
};

function DialogShell({
  title,
  onClose,
  children,
}: DialogProps & { title: string; children: ReactNode }) {
  return (
    <div className="lf-modal-backdrop" role="presentation">
      <div className="lf-dialog" role="dialog" aria-modal="true" aria-label={title}>
        <div className="lf-dialog-header">
          <h2>{title}</h2>
          <button className="lf-icon-button" type="button" title="关闭" onClick={onClose}>
            <X />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

function StatusLine({ text, tone }: { text: string; tone?: "error" | "ok" }) {
  if (!text) return null;
  return (
    <div className="lf-dialog-status" data-tone={tone ?? "ok"}>
      {text}
    </div>
  );
}

export function ExportDialog({ onClose }: DialogProps) {
  const status = useSession((s) => s.status);
  const selectedLine = useSession((s) => s.selectedLine);
  const [mode, setMode] = useState<"view" | "range">("view");
  const [exportView, setExportView] = useState<RowsView>("filtered");
  const [startLine, setStartLine] = useState(selectedLine ?? 1);
  const [endLine, setEndLine] = useState(selectedLine ?? Math.max(1, status.totalLines));
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [tone, setTone] = useState<"error" | "ok">("ok");
  const [progress, setProgress] = useState<ExportProgress | null>(null);

  const disabled = busy || !status.totalBytes || !path.trim();

  useEffect(() => {
    const un = onExportProgress(setProgress);
    return () => {
      un.then((f) => f());
    };
  }, []);

  const chooseOutput = async () => {
    const picked = await save({
      defaultPath: "logfilter-export.log",
      filters: [{ name: "Log files", extensions: ["log", "txt"] }],
    });
    if (picked) setPath(picked);
  };

  const runExport = async () => {
    setBusy(true);
    setMessage("");
    setProgress(null);
    try {
      const result = await exportLogs(
        mode === "range" ? { mode, startLine, endLine, path } : { mode, view: exportView, path },
      );
      setTone("ok");
      setMessage(
        `已导出 ${result.writtenLines.toLocaleString()} 行 · ${result.writtenBytes.toLocaleString()} bytes`,
      );
    } catch (err) {
      setTone("error");
      setMessage(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <DialogShell title="导出日志" onClose={onClose}>
      <div className="lf-dialog-body">
        <div className="lf-segmented">
          <button data-active={mode === "view"} type="button" onClick={() => setMode("view")}>
            当前结果
          </button>
          <button data-active={mode === "range"} type="button" onClick={() => setMode("range")}>
            行号范围
          </button>
        </div>

        {mode === "view" ? (
          <SelectField
            label="导出内容"
            value={exportView}
            options={(Object.keys(VIEW_LABELS) as RowsView[]).map((key) => ({
              value: key,
              label: VIEW_LABELS[key],
            }))}
            onChange={setExportView}
          />
        ) : (
          <div className="lf-form-grid-two">
            <label className="lf-form-field">
              <span>开始行</span>
              <input
                min={1}
                type="number"
                value={startLine}
                onChange={(e) => setStartLine(Number(e.target.value))}
              />
            </label>
            <label className="lf-form-field">
              <span>结束行</span>
              <input
                min={1}
                type="number"
                value={endLine}
                onChange={(e) => setEndLine(Number(e.target.value))}
              />
            </label>
          </div>
        )}

        <label className="lf-form-field">
          <span>输出文件</span>
          <div className="lf-path-row">
            <input value={path} onChange={(e) => setPath(e.target.value)} />
            <Button size="icon-sm" variant="ghost" title="选择文件" onClick={chooseOutput}>
              <FolderOpen />
            </Button>
          </div>
        </label>

        <StatusLine text={message} tone={tone} />
      </div>
      <div className="lf-dialog-footer">
        <Button variant="ghost" onClick={onClose}>
          关闭
        </Button>
        <Button className="lf-dialog-primary" disabled={disabled} onClick={runExport}>
          <Save />
          {busy
            ? progress
              ? `导出中 · 已写入 ${progress.writtenLines.toLocaleString()} 行`
              : "导出中"
            : "导出"}
        </Button>
      </div>
    </DialogShell>
  );
}

export function SplitDialog({ onClose }: DialogProps) {
  const sourcePath = useSession((s) => s.sourcePath);
  const config = useSession((s) => s.appConfig);
  const [path, setPath] = useState(sourcePath ?? "");
  const [outDir, setOutDir] = useState(config.storageDir ?? "");
  const [mode, setMode] = useState<"lines" | "bytes">("lines");
  const [value, setValue] = useState(100_000);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [tone, setTone] = useState<"error" | "ok">("ok");
  const [progress, setProgress] = useState<SplitProgress | null>(null);
  const disabled = busy || !path.trim() || !outDir.trim() || value <= 0;

  useEffect(() => {
    const un = onSplitProgress(setProgress);
    return () => {
      un.then((f) => f());
    };
  }, []);

  const chooseSource = async () => {
    const picked = await open({ multiple: false, directory: false });
    if (typeof picked === "string") setPath(picked);
  };

  const chooseOutDir = async () => {
    const picked = await open({ multiple: false, directory: true });
    if (typeof picked === "string") setOutDir(picked);
  };

  const runSplit = async () => {
    setBusy(true);
    setMessage("");
    setProgress(null);
    try {
      const result = await splitLogFile({ path, outDir, mode, value });
      setTone("ok");
      setMessage(
        `已生成 ${result.parts.length.toLocaleString()} 个文件 · ${result.totalBytes.toLocaleString()} bytes`,
      );
    } catch (err) {
      setTone("error");
      setMessage(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <DialogShell title="切分日志" onClose={onClose}>
      <div className="lf-dialog-body">
        <label className="lf-form-field">
          <span>源文件</span>
          <div className="lf-path-row">
            <input value={path} onChange={(e) => setPath(e.target.value)} />
            <Button size="icon-sm" variant="ghost" title="选择源文件" onClick={chooseSource}>
              <FolderOpen />
            </Button>
          </div>
        </label>
        <label className="lf-form-field">
          <span>输出目录</span>
          <div className="lf-path-row">
            <input value={outDir} onChange={(e) => setOutDir(e.target.value)} />
            <Button size="icon-sm" variant="ghost" title="选择目录" onClick={chooseOutDir}>
              <FolderOpen />
            </Button>
          </div>
        </label>
        <div className="lf-form-grid-two">
          <SelectField
            label="方式"
            value={mode}
            options={[
              { value: "lines", label: "按行数" },
              { value: "bytes", label: "按字节" },
            ]}
            onChange={setMode}
          />
          <label className="lf-form-field">
            <span>{mode === "lines" ? "每份行数" : "每份字节"}</span>
            <input
              min={1}
              type="number"
              value={value}
              onChange={(e) => setValue(Number(e.target.value))}
            />
          </label>
        </div>
        <StatusLine text={message} tone={tone} />
      </div>
      <div className="lf-dialog-footer">
        <Button variant="ghost" onClick={onClose}>
          关闭
        </Button>
        <Button className="lf-dialog-primary" disabled={disabled} onClick={runSplit}>
          <Save />
          {busy ? (progress ? `切分中 · 已生成 ${progress.parts} 份` : "切分中") : "切分"}
        </Button>
      </div>
    </DialogShell>
  );
}

export function SettingsDialog({ onClose }: DialogProps) {
  const config = useSession((s) => s.appConfig);
  const setAppConfig = useSession((s) => s.setAppConfig);
  const [draft, setDraft] = useState<AppConfig>(config);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [tone, setTone] = useState<"error" | "ok">("ok");

  const patch = (next: Partial<AppConfig>) => setDraft((current) => ({ ...current, ...next }));

  const chooseAdb = async () => {
    const picked = await open({ multiple: false, directory: false });
    if (typeof picked === "string") patch({ adbPath: picked });
  };

  const chooseStorage = async () => {
    const picked = await open({ multiple: false, directory: true });
    if (typeof picked === "string") patch({ storageDir: picked });
  };

  const saveSettings = async () => {
    setBusy(true);
    setMessage("");
    try {
      const saved = await saveAppConfig(draft);
      setAppConfig(saved);
      setTone("ok");
      setMessage("设置已保存");
    } catch (err) {
      setTone("error");
      setMessage(String(err));
    } finally {
      setBusy(false);
    }
  };

  const configName = useMemo(() => draft.configPath || "默认配置路径", [draft.configPath]);

  return (
    <DialogShell title="设置" onClose={onClose}>
      <div className="lf-dialog-body">
        <div className="lf-segmented">
          <button
            data-active={draft.theme === "light"}
            type="button"
            onClick={() => patch({ theme: "light" })}
          >
            浅色
          </button>
          <button
            data-active={draft.theme === "dark"}
            type="button"
            onClick={() => patch({ theme: "dark" })}
          >
            深色
          </button>
        </div>
        <label className="lf-form-field">
          <span>配置文件</span>
          <input readOnly value={configName} />
        </label>
        <SelectField
          label="编码"
          value={draft.encoding}
          options={[
            { value: "UTF-8", label: "UTF-8" },
            { value: "Local", label: "本地" },
          ]}
          onChange={(encoding) => patch({ encoding })}
        />
        <label className="lf-form-field">
          <span>存储位置</span>
          <div className="lf-path-row">
            <input
              value={draft.storageDir ?? ""}
              onChange={(e) => patch({ storageDir: e.target.value || null })}
            />
            <Button size="icon-sm" variant="ghost" title="选择目录" onClick={chooseStorage}>
              <FolderOpen />
            </Button>
          </div>
        </label>
        <label className="lf-form-field">
          <span>ADB 路径</span>
          <div className="lf-path-row">
            <input
              value={draft.adbPath ?? ""}
              onChange={(e) => patch({ adbPath: e.target.value || null })}
            />
            <Button size="icon-sm" variant="ghost" title="选择 adb" onClick={chooseAdb}>
              <FolderOpen />
            </Button>
          </div>
        </label>
        <div className="lf-form-grid-two">
          <label className="lf-form-field">
            <span>字体</span>
            <input
              min={10}
              max={20}
              type="number"
              value={draft.fontSize}
              onChange={(e) => patch({ fontSize: Number(e.target.value) })}
            />
          </label>
          <label className="lf-form-field">
            <span>行高</span>
            <input
              min={16}
              max={32}
              type="number"
              value={draft.rowHeight}
              onChange={(e) => patch({ rowHeight: Number(e.target.value) })}
            />
          </label>
        </div>
        <StatusLine text={message} tone={tone} />
      </div>
      <div className="lf-dialog-footer">
        <Button variant="ghost" onClick={onClose}>
          关闭
        </Button>
        <Button className="lf-dialog-primary" disabled={busy} onClick={saveSettings}>
          <Save />
          {busy ? "保存中" : "保存"}
        </Button>
      </div>
    </DialogShell>
  );
}
