import { useEffect } from "react";
import { Toolbar } from "@/components/Toolbar";
import { StatusBar } from "@/components/StatusBar";
import { LogTable } from "@/components/LogTable";
import { Minimap } from "@/components/Minimap";
import { nextBookmark, onIndexProgress, setFilter } from "@/lib/ipc";
import { useSession } from "@/store/session";

export default function App() {
  const setStatus = useSession((s) => s.setStatus);
  const filter = useSession((s) => s.filter);
  const filterRevision = useSession((s) => s.filterRevision);
  const sessionId = useSession((s) => s.sessionId);
  const hasFile = useSession((s) => s.status.totalBytes > 0);
  const setFilteredLines = useSession((s) => s.setFilteredLines);
  const selectedLine = useSession((s) => s.selectedLine);
  const setSelectedLine = useSession((s) => s.setSelectedLine);
  const setView = useSession((s) => s.setView);

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

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "F2" && event.key !== "F3") return;
      event.preventDefault();
      const direction = event.key === "F2" ? "previous" : "next";
      nextBookmark(selectedLine ?? 1, direction).then((line) => {
        if (line) {
          setView("all");
          setSelectedLine(line);
        }
      });
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedLine, setSelectedLine, setView]);

  return (
    <div className="lf-app">
      <Toolbar />
      <div className="lf-main">
        <Minimap />
        <LogTable />
      </div>
      <StatusBar />
    </div>
  );
}
