import { useEffect } from "react";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { onIndexProgress } from "@/lib/ipc";
import { useSession } from "@/store/session";

export default function App() {
  const setStatus = useSession((s) => s.setStatus);

  useEffect(() => {
    const un = onIndexProgress(setStatus);
    return () => {
      un.then((f) => f());
    };
  }, [setStatus]);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        background: "var(--background, #ffffff)",
        color: "var(--foreground, #111111)",
      }}
    >
      <Toolbar />
      <div style={{ flex: 1, minHeight: 0 }}>
        <LogTable />
      </div>
      <StatusBar />
    </div>
  );
}
