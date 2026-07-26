import { compactSourcePath } from "@/lib/sourceDisplay";
import { useSession } from "@/store/session";

export function StatusBar() {
  const status = useSession((s) => s.status);
  const selectedLine = useSession((s) => s.selectedLine);
  const tableScope = useSession((s) => s.tableScope);
  const markedOnly = useSession((s) => s.filter.markedOnly);
  const appConfig = useSession((s) => s.appConfig);
  const sourceMode = useSession((s) => s.sourceMode);
  const sourcePath = useSession((s) => s.sourcePath);
  const selectedDeviceSerial = useSession((s) => s.selectedDeviceSerial);
  const streamRunning = useSession((s) => s.streamRunning);
  const streamPaused = useSession((s) => s.streamPaused);
  const streamError = useSession((s) => s.streamError);
  const hasFile = status.totalBytes > 0;
  const pct = hasFile ? Math.round((status.indexedBytes / status.totalBytes) * 100) : 0;
  const sourceDetail =
    sourceMode === "adb"
      ? {
          label: `adb · ${selectedDeviceSerial ?? "无设备"}`,
          title: streamRunning
            ? "adb logcat 运行中"
            : streamPaused
              ? "adb logcat 已暂停"
              : "adb logcat",
        }
      : sourcePath
        ? compactSourcePath(sourcePath, { maxLength: 54 })
        : { label: "file · 未打开", title: "未打开文件" };

  return (
    <div className="lf-statusbar">
      {hasFile ? (
        <>
          <span>已加载 {status.totalLines.toLocaleString()} 行</span>
          {tableScope.kind === "problem-context" ? (
            <span className="lf-status-accent">
              临时原始上下文 · 事件第 {tableScope.eventRange.startLine.toLocaleString()}–
              {tableScope.eventRange.endLine.toLocaleString()} 行
            </span>
          ) : (
            <span className="lf-status-accent">
              当前结果 {status.filteredLines.toLocaleString()} 行{markedOnly ? " · 仅标记" : ""}
            </span>
          )}
          <span>
            索引 {pct}%{status.indexing ? " · 进行中" : ""}
          </span>
          <span>当前 第 {selectedLine ?? 1} 行</span>
          <span>{appConfig.encoding}</span>
          {streamError ? <span title={streamError}>抓取异常</span> : null}
          <span className="lf-status-fill" />
          <span className="lf-status-source" title={sourceDetail.title}>
            <i />
            {sourceDetail.label}
          </span>
        </>
      ) : (
        <>
          <span>{sourceMode === "adb" ? "logcat 就绪" : "未打开文件 · 就绪"}</span>
          <span className="lf-status-fill" />
          <span className="lf-status-source lf-status-muted" title={sourceDetail.title}>
            <i />
            {sourceDetail.label}
          </span>
        </>
      )}
    </div>
  );
}
