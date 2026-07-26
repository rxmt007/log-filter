import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProblemsDock } from "@/components/ProblemsDock";
import { useProblemsLive, type ProblemsLiveClient } from "@/hooks/useProblemsLive";
import { useProblems } from "@/store/problems";
import { useSession } from "@/store/session";
import type {
  AnalysisToken,
  ProblemDetail,
  ProblemGroup,
  ProblemOccurrence,
  ProblemPage,
  ProblemsProgress,
  ProblemsStatus,
} from "@/types";

const token: AnalysisToken = { sessionGeneration: 7, analysisGeneration: 2 };

const status = (revision = 4): ProblemsStatus => ({
  analysisToken: token,
  scannedLines: 900,
  stableLines: 1_000,
  scanning: true,
  finished: false,
  coverage: {
    origin: "static-file",
    requestedBuffers: null,
    rangeCompleteness: "bounded",
  },
  stats: {
    observedOccurrenceCount: 2,
    storedOccurrenceCount: 2,
    droppedOccurrenceCount: 0,
    provisionalOccurrenceCount: 0,
    storedGroupCount: 2,
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
  signatureSummary: `IllegalStateException · frame${id}`,
  signatureSummaryTruncated: false,
  fingerprint: `java-crash:${id}`,
  observedOccurrenceCount: 1,
  storedOccurrenceCount: 1,
  droppedOccurrenceCount: 0,
  firstLine: id * 100,
  firstTimestamp: `07-26 10:0${id}:00.000`,
  lastLine: id * 100 + 10,
  lastTimestamp: `07-26 10:0${id}:01.000`,
  firstEventId: id * 10,
  lastEventId: id * 10,
  representativeEventId: id * 10,
});

const occurrence = (eventId: number, groupId = 1): ProblemOccurrence => ({
  eventId,
  groupId,
  kind: "java-crash",
  startLine: 100,
  endLine: 110,
  anchorLine: 102,
  pid: 42,
  timestamp: "07-26 10:01:00.000",
  processInstanceId: 3,
  evidenceFlags: ["primary"],
  outcomeFlags: [],
  boundaryFlags: [],
});

const page = <T,>(
  items: T[],
  querySnapshotId: number,
  nextOffset: number | null,
): ProblemPage<T> => ({
  analysisToken: token,
  snapshotHandle: `snapshot-${querySnapshotId}`,
  revision: 4,
  total: 2,
  items,
  nextCursor:
    nextOffset == null ? null : `cursor-${querySnapshotId}-${nextOffset}`,
});

const detail = (eventId: number): ProblemDetail => ({
  analysisToken: token,
  revision: 4,
  occurrence: occurrence(eventId),
  facts: [
    {
      code: "java-uncaught-exception",
      sourceLine: 102,
      ruleId: "aosp.java-uncaught.v1",
      role: "primary",
      evidenceFormat: "aosp",
      provenance: "main",
    },
  ],
  factsTruncated: false,
  observationTotal: 1,
});

function createClient() {
  let progressListener: ((progress: ProblemsProgress) => void) | null = null;
  const client: ProblemsLiveClient = {
    getStatus: vi.fn().mockResolvedValue(status()),
    getGroups: vi.fn().mockResolvedValue(page([group(1)], 91, 1)),
    getOccurrences: vi.fn().mockResolvedValue(page([occurrence(10)], 92, null)),
    getDetail: vi.fn().mockImplementation(({ eventId }) => Promise.resolve(detail(eventId))),
    releaseSnapshot: vi.fn().mockResolvedValue(true),
    onProgress: vi.fn().mockImplementation((listener) => {
      progressListener = listener;
      return Promise.resolve(() => undefined);
    }),
  };
  return {
    client,
    emitProgress(progress: ProblemsProgress) {
      if (!progressListener) throw new Error("progress listener not installed");
      act(() => progressListener?.(progress));
    },
  };
}

function progress(revision: number): ProblemsProgress {
  return {
    scannedLines: 950,
    stableLines: 1_000,
    coverage: {
      origin: "static-file",
      requestedBuffers: null,
      rangeCompleteness: "bounded",
    },
    observedOccurrenceCount: 3,
    storedOccurrenceCount: 3,
    droppedOccurrenceCount: 0,
    provisionalOccurrenceCount: 0,
    storedGroupCount: 3,
    ungroupedDroppedOccurrenceCount: 0,
    droppedRecentObservationCount: 0,
    correlationLimited: false,
    revision,
    done: false,
    limited: false,
    sessionGeneration: token.sessionGeneration,
    analysisGeneration: token.analysisGeneration,
  };
}

function Harness({ client }: { client: ProblemsLiveClient }) {
  const bindings = useProblemsLive(client);
  return <ProblemsDock {...bindings} />;
}

describe("useProblemsLive", () => {
  beforeEach(() => {
    useProblems.setState(useProblems.getInitialState(), true);
    useSession.setState({
      status: {
        totalLines: 1_000,
        stableLines: 1_000,
        filteredLines: 900,
        bookmarkLines: 0,
        errorLines: 0,
        indexedBytes: 100,
        totalBytes: 100,
        indexing: false,
        generation: token.sessionGeneration,
        analysisGeneration: token.analysisGeneration,
        filterInputRevision: 0,
        appliedFilterInputRevision: 0,
        filterResultRevision: 0,
        decodeRevision: 0,
        sourceDataRevision: 0,
      },
      sessionId: 1,
    });
  });

  it("subscribes to summary while folded and waits for expansion before querying groups", async () => {
    const { client } = createClient();
    render(<Harness client={client} />);

    await waitFor(() => expect(client.getStatus).toHaveBeenCalledTimes(1));
    expect(client.getGroups).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));

    await waitFor(() =>
      expect(client.getGroups).toHaveBeenCalledWith({
        expectedAnalysisToken: token,
        cursor: null,
        kind: null,
        sort: "last-seen-desc",
        limit: 100,
      }),
    );
    expect(await screen.findByText("IllegalStateException · frame1")).toBeInTheDocument();
  });

  it("surfaces progress-subscription failures without losing the status snapshot", async () => {
    const { client } = createClient();
    vi.mocked(client.onProgress)
      .mockRejectedValueOnce("progress unavailable")
      .mockResolvedValueOnce(() => undefined);
    render(<Harness client={client} />);

    await waitFor(() =>
      expect(useProblems.getState()).toMatchObject({
        statusError: "progress unavailable",
        status: { analysisToken: token },
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await userEvent.click(screen.getByRole("button", { name: "重试故障分析状态" }));
    await waitFor(() => expect(client.onProgress).toHaveBeenCalledTimes(2));
  });

  it("reuses the frozen page when the user folds and reopens the dock", async () => {
    const { client } = createClient();
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await screen.findByText("IllegalStateException · frame1");

    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));

    expect(client.getGroups).toHaveBeenCalledTimes(1);
    expect(useProblems.getState().groupPage?.snapshotHandle).toBe("snapshot-91");
  });

  it("loads occurrences and detail from the selected frozen analysis", async () => {
    const { client } = createClient();
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await screen.findByText("IllegalStateException · frame1");

    await userEvent.click(screen.getByRole("option", { name: /Java\/Kotlin 崩溃/ }));
    await waitFor(() =>
      expect(client.getOccurrences).toHaveBeenCalledWith({
        expectedAnalysisToken: token,
        cursor: null,
        groupId: 1,
        limit: 100,
      }),
    );

    await userEvent.click(await screen.findByRole("option", { name: /锚点 第 102 行/ }));
    await waitFor(() =>
      expect(client.getDetail).toHaveBeenCalledWith({
        eventId: 10,
        expectedAnalysisToken: token,
      }),
    );
    expect(await screen.findByRole("heading", { name: "检测到的事实" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "排查提示（非结论）" })).toBeInTheDocument();
  });

  it("releases the prior occurrence snapshot before switching groups", async () => {
    const { client } = createClient();
    vi.mocked(client.getGroups).mockResolvedValue(page([group(1), group(2)], 91, null));
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));

    const options = await screen.findAllByRole("option", {
      name: /Java\/Kotlin 崩溃/,
    });
    await userEvent.click(options[0]);
    await screen.findByRole("option", { name: /锚点 第 102 行/ });
    await userEvent.click(options[1]);

    await waitFor(() =>
      expect(client.releaseSnapshot).toHaveBeenCalledWith({
        snapshotHandle: "snapshot-92",
        expectedAnalysisToken: token,
      }),
    );
    await waitFor(() =>
      expect(client.getOccurrences).toHaveBeenLastCalledWith({
        expectedAnalysisToken: token,
        cursor: null,
        groupId: 2,
        limit: 100,
      }),
    );
  });

  it("keeps pagination on its frozen snapshot when live progress advances", async () => {
    const { client, emitProgress } = createClient();
    vi.mocked(client.getGroups)
      .mockResolvedValueOnce(page([group(1)], 91, 1))
      .mockResolvedValueOnce(page([group(2)], 91, null));
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await screen.findByText("IllegalStateException · frame1");

    emitProgress(progress(5));
    expect(await screen.findByRole("button", { name: /有新结果/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "加载更多分组" }));

    await waitFor(() =>
      expect(client.getGroups).toHaveBeenLastCalledWith({
        expectedAnalysisToken: token,
        cursor: "cursor-91-1",
        limit: 100,
      }),
    );
    expect(useProblems.getState().groupPage?.items.map((item) => item.id)).toEqual([1, 2]);
    expect(useProblems.getState().groupPage?.revision).toBe(4);
  });

  it("preserves rendered items and exposes snapshot expiry instead of mixing pages", async () => {
    const { client } = createClient();
    vi.mocked(client.getGroups)
      .mockResolvedValueOnce(page([group(1)], 91, 1))
      .mockRejectedValueOnce("snapshot-expired");
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await screen.findByText("IllegalStateException · frame1");

    await userEvent.click(screen.getByRole("button", { name: "加载更多分组" }));

    expect(
      await screen.findByText("结果快照已过期；当前内容已保留，请手动刷新。"),
    ).toBeInTheDocument();
    expect(useProblems.getState().groupPage?.items.map((item) => item.id)).toEqual([1]);
  });

  it("rejects a page whose echoed analysis token does not match the request", async () => {
    const { client } = createClient();
    vi.mocked(client.getGroups).mockResolvedValue({
      ...page([group(1)], 91, null),
      analysisToken: {
        ...token,
        analysisGeneration: token.analysisGeneration - 1,
      },
    });
    render(<Harness client={client} />);

    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));

    expect(await screen.findByText("读取故障分组失败：stale-analysis-token")).toBeInTheDocument();
    expect(useProblems.getState().groupPage).toBeNull();
    expect(useProblems.getState().groupLoading).toBe(false);
    await waitFor(() => expect(client.getStatus).toHaveBeenCalledTimes(2));
  });

  it("releases both frozen snapshots before refreshing the group query", async () => {
    const { client, emitProgress } = createClient();
    vi.mocked(client.getGroups)
      .mockResolvedValueOnce(page([group(1)], 91, null))
      .mockResolvedValueOnce(page([group(2)], 93, null));
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await userEvent.click(await screen.findByRole("option", { name: /Java\/Kotlin 崩溃/ }));
    await screen.findByRole("option", { name: /锚点 第 102 行/ });

    emitProgress(progress(5));
    await userEvent.click(screen.getByRole("button", { name: /有新结果/ }));

    await waitFor(() => expect(client.releaseSnapshot).toHaveBeenCalledTimes(2));
    expect(client.releaseSnapshot).toHaveBeenCalledWith({
      snapshotHandle: "snapshot-91",
      expectedAnalysisToken: token,
    });
    expect(client.releaseSnapshot).toHaveBeenCalledWith({
      snapshotHandle: "snapshot-92",
      expectedAnalysisToken: token,
    });
    await waitFor(() => expect(client.getGroups).toHaveBeenCalledTimes(2));
    expect(useProblems.getState().groupPage?.snapshotHandle).toBe("snapshot-93");
  });

  it("releases frozen pages before changing the category query", async () => {
    const { client } = createClient();
    vi.mocked(client.getGroups)
      .mockResolvedValueOnce(page([group(1)], 91, null))
      .mockResolvedValueOnce(page([group(2)], 94, null));
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await userEvent.click(await screen.findByRole("option", { name: /Java\/Kotlin 崩溃/ }));
    await screen.findByRole("option", { name: /锚点 第 102 行/ });

    await userEvent.click(screen.getByRole("button", { name: "仅看 ANR" }));

    await waitFor(() => expect(client.releaseSnapshot).toHaveBeenCalledTimes(2));
    expect(client.releaseSnapshot).toHaveBeenCalledWith({
      snapshotHandle: "snapshot-91",
      expectedAnalysisToken: token,
    });
    expect(client.releaseSnapshot).toHaveBeenCalledWith({
      snapshotHandle: "snapshot-92",
      expectedAnalysisToken: token,
    });
    await waitFor(() =>
      expect(client.getGroups).toHaveBeenLastCalledWith({
        expectedAnalysisToken: token,
        cursor: null,
        kind: "anr",
        sort: "last-seen-desc",
        limit: 100,
      }),
    );
    const releaseOrder = vi.mocked(client.releaseSnapshot).mock.invocationCallOrder;
    const groupQueryOrder = vi.mocked(client.getGroups).mock.invocationCallOrder[1];
    expect(Math.max(...releaseOrder)).toBeLessThan(groupQueryOrder);
  });

  it("starts a new frozen query when the group ordering changes", async () => {
    const { client } = createClient();
    vi.mocked(client.getGroups)
      .mockResolvedValueOnce(page([group(1)], 91, null))
      .mockResolvedValueOnce(page([group(2)], 95, null));
    render(<Harness client={client} />);
    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));
    await screen.findByText("IllegalStateException · frame1");

    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "分组排序" }),
      "count-desc",
    );

    await waitFor(() =>
      expect(client.getGroups).toHaveBeenLastCalledWith({
        expectedAnalysisToken: token,
        cursor: null,
        kind: null,
        sort: "count-desc",
        limit: 100,
      }),
    );
    expect(useProblems.getState().sort).toBe("count-desc");
  });
});
