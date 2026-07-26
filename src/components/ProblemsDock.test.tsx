import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProblemsDock } from "@/components/ProblemsDock";
import { useProblems } from "@/store/problems";
import type {
  AnalysisToken,
  ProblemDetail,
  ProblemGroup,
  ProblemOccurrence,
  ProblemsStatus,
} from "@/types";

const token: AnalysisToken = { sessionGeneration: 3, analysisGeneration: 1 };
const status: ProblemsStatus = {
  analysisToken: token,
  scannedLines: 900,
  stableLines: 1_000,
  scanning: true,
  finished: false,
  stats: {
    observedOccurrenceCount: 7,
    storedOccurrenceCount: 5,
    droppedOccurrenceCount: 2,
    storedGroupCount: 1,
    ungroupedDroppedOccurrenceCount: 1,
    droppedRecentObservationCount: 3,
    revision: 4,
    limited: true,
    correlationLimited: true,
  },
};

const group: ProblemGroup = {
  id: 2,
  kind: "java-crash",
  observedOccurrenceCount: 5,
  storedOccurrenceCount: 4,
  droppedOccurrenceCount: 1,
  firstLine: 120,
  lastLine: 920,
  representativeEventId: 8,
};

const occurrence: ProblemOccurrence = {
  eventId: 8,
  groupId: 2,
  kind: "java-crash",
  startLine: 910,
  endLine: 930,
  anchorLine: 912,
  pid: 42,
  timestamp: "07-26 12:00:00.000",
  outcomeFlags: [],
  boundaryFlags: ["observation-refs-truncated"],
};

const detail: ProblemDetail = {
  analysisToken: token,
  revision: 4,
  occurrence,
  observationTotal: 9,
  facts: [
    { code: "java-uncaught-exception", sourceLine: 912, ruleId: "aosp.java-uncaught.v1" },
    { code: "exception-type-recorded", sourceLine: 914, ruleId: "aosp.java-uncaught.v1" },
  ],
};

function seedOpenDock() {
  const state = useProblems.getState();
  state.acceptStatus(status);
  state.setPanelOpen(true);
  state.replaceGroupPage({
    querySnapshotId: 10,
    revision: 4,
    total: 1,
    items: [group],
    nextOffset: null,
  });
  state.selectGroup(group.id);
  state.replaceOccurrencePage({
    querySnapshotId: 11,
    revision: 4,
    total: 1,
    items: [occurrence],
    nextOffset: null,
  });
  state.selectEvent(occurrence.eventId);
  state.setDetail(detail);
}

describe("ProblemsDock", () => {
  beforeEach(() => useProblems.getState().resetForAnalysis(token));

  it("is folded by default, requests no list, and distinguishes detected from expandable", () => {
    useProblems.getState().acceptStatus(status);
    const onOpen = vi.fn();
    render(<ProblemsDock onOpen={onOpen} />);

    expect(screen.queryByRole("region", { name: "故障调查工作台" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Problems/ })).toHaveTextContent(
      "检出 7 · 可展开 5",
    );
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("requests the first group page only after the user expands it", async () => {
    useProblems.getState().acceptStatus(status);
    const onOpen = vi.fn();
    render(<ProblemsDock onOpen={onOpen} />);

    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));

    expect(screen.getByRole("region", { name: "故障调查工作台" })).toBeInTheDocument();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("separates located facts from static non-conclusive hints", async () => {
    seedOpenDock();
    const onLocateFact = vi.fn();
    render(<ProblemsDock onLocateFact={onLocateFact} />);

    expect(screen.getByRole("heading", { name: "检测到的事实" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "排查提示（非结论）" })).toBeInTheDocument();
    expect(screen.getByText("仅展示 2/9 条关键证据，可查看事件范围")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "定位到第 914 行" }));
    expect(onLocateFact).toHaveBeenCalledWith(914);
    expect(screen.queryByText(/根因是|确定为|已导致/)).not.toBeInTheDocument();
  });

  it("offers locate, temporary context, and raw-log export actions", async () => {
    seedOpenDock();
    const onLocateOccurrence = vi.fn();
    const onOpenContext = vi.fn();
    const onExportOccurrence = vi.fn();
    render(
      <ProblemsDock
        onLocateOccurrence={onLocateOccurrence}
        onOpenContext={onOpenContext}
        onExportOccurrence={onExportOccurrence}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "定位事件" }));
    await userEvent.click(screen.getByRole("button", { name: "查看上下文" }));
    await userEvent.click(screen.getByRole("button", { name: "导出原始日志" }));

    expect(onLocateOccurrence).toHaveBeenCalledWith(occurrence);
    expect(onOpenContext).toHaveBeenCalledWith(occurrence);
    expect(onExportOccurrence).toHaveBeenCalledWith(occurrence);
  });
});
