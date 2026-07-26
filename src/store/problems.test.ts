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
  coverage: {
    origin: "static-file",
    requestedBuffers: null,
    rangeCompleteness: "bounded",
  },
  stats: {
    observedOccurrenceCount: 3,
    storedOccurrenceCount: 3,
    droppedOccurrenceCount: 0,
    provisionalOccurrenceCount: 0,
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
  fingerprintVersion: 1,
  signatureQuality: "stack",
  identityQuality: "known-process",
  processSummary: "com.example.app",
  processSummaryTruncated: false,
  signatureSummary: "IllegalStateException",
  signatureSummaryTruncated: false,
  fingerprint: `java-crash:${id}`,
  observedOccurrenceCount: 1,
  storedOccurrenceCount: 1,
  droppedOccurrenceCount: 0,
  firstLine: id,
  firstTimestamp: null,
  lastLine: id,
  lastTimestamp: null,
  firstEventId: id,
  lastEventId: id,
  representativeEventId: id,
});

const page = (items: ProblemGroup[], nextCursor: string | null): ProblemPage<ProblemGroup> => ({
  analysisToken: token,
  snapshotHandle: "snapshot-11",
  revision: 4,
  total: 3,
  items,
  nextCursor,
});

describe("problems store", () => {
  beforeEach(() => {
    useProblems.setState(useProblems.getInitialState(), true);
    useProblems.getState().resetForAnalysis(token);
  });

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

  it("does not regress coverage or revision within the same analysis", () => {
    useProblems.getState().acceptStatus(status(5));
    useProblems.getState().acceptStatus({
      ...status(4),
      scannedLines: 300,
    });

    expect(useProblems.getState().status?.scannedLines).toBe(400);
    expect(useProblems.getState().status?.stats.revision).toBe(5);
  });

  it("keeps a frozen page and marks newer revisions for explicit refresh", () => {
    useProblems.getState().replaceGroupPage(page([group(1)], "cursor-1"));
    useProblems.getState().acceptStatus(status(5));
    expect(useProblems.getState().groupPage?.items).toEqual([group(1)]);
    expect(useProblems.getState().hasNewResults).toBe(true);
  });

  it("deduplicates appended pages from the same snapshot", () => {
    useProblems.getState().replaceGroupPage(page([group(1), group(2)], "cursor-2"));
    useProblems.getState().appendGroupPage(page([group(2), group(3)], null));
    expect(useProblems.getState().groupPage?.items.map((item) => item.id)).toEqual([1, 2, 3]);
  });

  it("preserves rendered content when a snapshot expires", () => {
    useProblems.getState().replaceGroupPage(page([group(1)], "cursor-1"));
    useProblems.getState().markSnapshotExpired("groups");
    expect(useProblems.getState().groupPage?.items).toEqual([group(1)]);
    expect(useProblems.getState().groupPageError).toBe("snapshot-expired");
  });

  it("ignores pages and details from an obsolete analysis token", () => {
    useProblems.getState().replaceGroupPage(page([group(1)], null));

    useProblems.getState().resetForAnalysis({
      sessionGeneration: token.sessionGeneration,
      analysisGeneration: token.analysisGeneration + 1,
    });
    useProblems.getState().replaceGroupPage(page([group(2)], null));

    expect(useProblems.getState().groupPage).toBeNull();
  });

  it("preserves the user's folded preference across analysis replacement", () => {
    useProblems.getState().setPanelOpen(true);
    useProblems.getState().setPanelHeight(312);

    useProblems.getState().resetForAnalysis({
      sessionGeneration: token.sessionGeneration + 1,
      analysisGeneration: 0,
    });

    expect(useProblems.getState().panelOpen).toBe(true);
    expect(useProblems.getState().panelHeight).toBe(312);
    expect(useProblems.getState().selectedGroupId).toBeNull();
    expect(useProblems.getState().groupPage).toBeNull();
  });
});
