import { useSession } from "@/store/session";

export function StatusBar() {
  const status = useSession((s) => s.status);
  const hasFile = status.totalBytes > 0;
  const pct = hasFile ? Math.round((status.indexedBytes / status.totalBytes) * 100) : 0;

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "4px 12px",
        borderTop: "1px solid var(--border, #e5e5e5)",
        fontSize: 12,
        color: "var(--muted-foreground, #888)",
      }}
    >
      {hasFile ? (
        <>
          <span>已加载 {status.totalLines.toLocaleString()} 行</span>
          <span>
            索引 {pct}%{status.indexing ? "(进行中)" : ""}
          </span>
        </>
      ) : (
        <span>未打开文件</span>
      )}
    </div>
  );
}
