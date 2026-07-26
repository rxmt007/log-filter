import { beforeEach, describe, expect, it } from "vitest";
import { useProblems } from "@/store/problems";
import type { AnalysisToken, ProblemGroup, ProblemPage, ProblemsStatus } from "@/types";

const token: AnalysisToken = { sessionGeneration: 7, analysisGeneration: 2 };
const status = (revision: number): ProblemsStatus => ({
  analysisToken: token,
  scannedLines: 400,
  stableLines: 1_000,
  scanning: true,
  finished: false,
  stats: {
    observedOccurrenceCount: 3,
    storedOccurrenceCount: 3,
    droppedOccurrenceCount: 0,
    storedGroupCount: 3,
    ungroupedDroppedOccurrenceCount: 0,
    droppedRecentObservationCount: 0,
    revision,
    limited: false,
    correlationLimited: false,
  },
});

const group = (id: number): ProblemGroup => ({
  id,
  kind: "java-crash",
  observedOccurrenceCount: 1,
  storedOccurrenceCount: 1,
  droppedOccurrenceCount: 0,
  firstLine: id,
  lastLine: id,
  representativeEventId: id,
});

const page = (items: ProblemGroup[], nextOffset: number | null): ProblemPage<ProblemGroup> => ({
  querySnapshotId: 11,
  revision: 4,
  total: 3,
  items,
  nextOffset,
});

describe("problems store", () => {
  beforeEach(() => useProblems.getState().resetForAnalysis(token));

  it("starts folded and does not materialize list state", () => {
    const state = useProblems.getState();
    expect(state.panelOpen).toBe(false);
    expect(state.groupPage).toBeNull();
    expect(state.occurrencePage).toBeNull();
  });

  it("ignores progress from a stale analysis token", () => {
    useProblems.getState().acceptStatus(status(1));
    useProblems.getState().acceptStatus({
      ...status(9),
      analysisToken: { ...token, analysisGeneration: 1 },
    });
    expect(useProblems.getState().status?.stats.revision).toBe(1);
  });

  it("keeps a frozen page and marks newer revisions for explicit refresh", () => {
    useProblems.getState().replaceGroupPage(page([group(1)], 1));
    useProblems.getState().acceptStatus(status(5));
    expect(useProblems.getState().groupPage?.items).toEqual([group(1)]);
    expect(useProblems.getState().hasNewResults).toBe(true);
  });

  it("deduplicates appended pages from the same snapshot", () => {
    useProblems.getState().replaceGroupPage(page([group(1), group(2)], 2));
    useProblems.getState().appendGroupPage(page([group(2), group(3)], null));
    expect(useProblems.getState().groupPage?.items.map((item) => item.id)).toEqual([1, 2, 3]);
  });

  it("preserves rendered content when a snapshot expires", () => {
    useProblems.getState().replaceGroupPage(page([group(1)], 1));
    useProblems.getState().markSnapshotExpired("groups");
    expect(useProblems.getState().groupPage?.items).toEqual([group(1)]);
    expect(useProblems.getState().groupPageError).toBe("snapshot-expired");
  });
});
