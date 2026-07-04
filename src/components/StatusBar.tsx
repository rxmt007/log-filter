import { useSession } from "@/store/session";

export function StatusBar() {
  const status = useSession((s) => s.status);
  const selectedLine = useSession((s) => s.selectedLine);
  const view = useSession((s) => s.view);
  const hasFile = status.totalBytes > 0;
  const pct = hasFile ? Math.round((status.indexedBytes / status.totalBytes) * 100) : 0;

  return (
    <div className="lf-statusbar">
      {hasFile ? (
        <>
          <span>已加载 {status.totalLines.toLocaleString()} 行</span>
          <span className="lf-status-accent">
            过滤后 {status.filteredLines.toLocaleString()} 行
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
            {view === "filtered" ? "过滤视图" : "全部视图"}
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
