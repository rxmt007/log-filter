import { useSession } from "@/store/session";

export function StatusBar() {
  const status = useSession((s) => s.status);
  const selectedLine = useSession((s) => s.selectedLine);
  const markedOnly = useSession((s) => s.filter.markedOnly);
  const appConfig = useSession((s) => s.appConfig);
  const sourceMode = useSession((s) => s.sourceMode);
  const selectedDeviceSerial = useSession((s) => s.selectedDeviceSerial);
  const streamRunning = useSession((s) => s.streamRunning);
  const streamPaused = useSession((s) => s.streamPaused);
  const hasFile = status.totalBytes > 0;
  const pct = hasFile ? Math.round((status.indexedBytes / status.totalBytes) * 100) : 0;
  const sourceLabel = sourceMode === "adb" ? "adb logcat · threadtime" : "本地文件";
  const deviceLabel =
    sourceMode === "adb"
      ? selectedDeviceSerial
        ? `${selectedDeviceSerial}${streamRunning ? " · 运行中" : streamPaused ? " · 已暂停" : ""}`
        : "无设备"
      : "当前结果";

  return (
    <div className="lf-statusbar">
      {hasFile ? (
        <>
          <span>已加载 {status.totalLines.toLocaleString()} 行</span>
          <span className="lf-status-accent">
            当前结果 {status.filteredLines.toLocaleString()} 行{markedOnly ? " · 仅标记" : ""}
          </span>
          <span>
            索引 {pct}%{status.indexing ? " · 进行中" : ""}
          </span>
          <span>当前 第 {selectedLine ?? 1} 行</span>
          <span>{appConfig.encoding}</span>
          <span>{sourceLabel}</span>
          <span className="lf-status-fill" />
          <span className="lf-status-device">
            <i />
            {deviceLabel}
          </span>
        </>
      ) : (
        <>
          <span>{sourceMode === "adb" ? "logcat 就绪" : "未打开文件 · 就绪"}</span>
          <span className="lf-status-fill" />
          <span className="lf-status-device lf-status-muted">
            <i />
            {deviceLabel}
          </span>
        </>
      )}
    </div>
  );
}
