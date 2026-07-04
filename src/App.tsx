import { useEffect } from "react";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { onIndexProgress, setFilter } from "@/lib/ipc";
import { useSession } from "@/store/session";

export default function App() {
  const setStatus = useSession((s) => s.setStatus);
  const filter = useSession((s) => s.filter);
  const filterRevision = useSession((s) => s.filterRevision);
  const sessionId = useSession((s) => s.sessionId);
  const hasFile = useSession((s) => s.status.totalBytes > 0);
  const setFilteredLines = useSession((s) => s.setFilteredLines);

  useEffect(() => {
    const un = onIndexProgress(setStatus);
    return () => {
      un.then((f) => f());
    };
  }, [setStatus]);

  useEffect(() => {
    if (!hasFile) return;
    const timer = window.setTimeout(() => {
      setFilter(filter)
        .then(setFilteredLines)
        .catch((err) => {
          console.error("set_filter failed", err);
        });
    }, 220);
    return () => window.clearTimeout(timer);
  }, [filter, filterRevision, hasFile, sessionId, setFilteredLines]);

  return (
    <div className="lf-app">
      <Toolbar />
      <div className="lf-main">
        <LogTable />
      </div>
      <StatusBar />
    </div>
  );
}
