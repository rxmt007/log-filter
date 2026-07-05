import { useSession } from "@/store/session";

export function StatusBar() {
  const status = useSession((s) => s.status);
  const selectedLine = useSession((s) => s.selectedLine);
  const markedOnly = useSession((s) => s.filter.markedOnly);
  const hasFile = status.totalBytes > 0;
  const pct = hasFile ? Math.round((status.indexedBytes / status.totalBytes) * 100) : 0;

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
          <span>UTF-8</span>
          <span>logcat · threadtime</span>
          <span className="lf-status-fill" />
          <span className="lf-status-device">
            <i />
            当前结果
          </span>
        </>
      ) : (
        <>
          <span>未打开文件 · 就绪</span>
          <span className="lf-status-fill" />
          <span className="lf-status-device lf-status-muted">
            <i />
            无设备
          </span>
        </>
      )}
    </div>
  );
}
