import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "@/components/ui/button";
import { openFile } from "@/lib/ipc";
import { useSession } from "@/store/session";

export function Toolbar() {
  const beginSession = useSession((s) => s.beginSession);

  const onOpen = async () => {
    const path = await open({ multiple: false, directory: false });
    if (typeof path === "string") {
      const st = await openFile(path);
      beginSession(st); // 换新文件:更新状态并自增 sessionId(触发表格清缓存)
    }
  };

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: 8,
        borderBottom: "1px solid var(--border, #e5e5e5)",
      }}
    >
      <Button size="sm" onClick={onOpen}>
        打开
      </Button>
    </div>
  );
}
