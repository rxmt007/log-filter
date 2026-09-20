import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LogTable } from "@/components/LogTable";
import { DEFAULT_FILTER, useSession } from "@/store/session";
import type { CheckedRowsRequest, CheckedRowsResponse, Status } from "@/types";

const mocks = vi.hoisted(() => ({
  getRowsChecked: vi.fn(),
  virtualizer: {
    getTotalSize: () => 2_000,
    getVirtualItems: () =>
      [0, 1].map((index) => ({
        index,
        key: index,
        start: index * 24,
        end: (index + 1) * 24,
        size: 24,
        lane: 0,
      })),
    scrollOffset: 0,
    scrollToIndex: vi.fn(),
  },
}));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: () => mocks.virtualizer,
}));

vi.mock("@/lib/ipc", () => ({
  getRowsChecked: mocks.getRowsChecked,
  listBookmarks: vi.fn(async () => []),
  saveAppConfig: vi.fn(async (config) => config),
  toggleBookmark: vi.fn(async () => false),
}));

const initialStatus: Status = {
  totalLines: 100,
  stableLines: 100,
  filteredLines: 50,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 1_000,
  totalBytes: 1_000,
  indexing: false,
  generation: 1,
  analysisGeneration: 1,
  filterInputRevision: 1,
  appliedFilterInputRevision: 1,
  filterResultRevision: 1,
  decodeRevision: 0,
  sourceDataRevision: 1,
};

interface PendingRows {
  request: CheckedRowsRequest;
  resolve: (response: CheckedRowsResponse) => void;
}

describe("LogTable live filtered rows", () => {
  let pending: PendingRows[];

  beforeEach(() => {
    pending = [];
    mocks.getRowsChecked.mockReset();
    mocks.getRowsChecked.mockImplementation(
      (request: CheckedRowsRequest) =>
        new Promise<CheckedRowsResponse>((resolve) => pending.push({ request, resolve })),
    );
    const filter = structuredClone(DEFAULT_FILTER);
    filter.tagInclude = { ...filter.tagInclude, enabled: true, pattern: "Activity" };
    useSession.setState({
      status: initialStatus,
      sourceMode: "adb",
      streamRunning: true,
      tailFollowing: false,
      filter,
      filterRevision: 1,
      appliedFilterInputRevision: 1,
      filterResultRevision: 1,
      tableScope: { kind: "results", view: "filtered" },
      selectedLine: null,
      selectedResultIndex: null,
      currentSearchLine: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: null,
    });
  });

  async function complete(load: PendingRows, prefix = "history") {
    await act(async () => {
      const { request } = load;
      load.resolve({
        status: "ok",
        analysisToken: request.expectedAnalysisToken,
        requestNonce: request.requestNonce,
        decodeRevision: 0,
        sourceDataRevision: 1,
        filterResultRevision:
          request.view === "filtered" ? request.expectedFilterResultRevision : 0,
        rows: Array.from({ length: request.count }, (_, index) => ({
          lineNo: request.start + index + 1,
          date: "09-18",
          time: "12:00:00.000",
          level: "I",
          pid: "100",
          tid: "101",
          tag: "Activity",
          message: `${prefix}-${request.start + index + 1}`,
          marked: false,
        })),
      });
    });
  }

  function append(revision: number) {
    act(() => {
      useSession.getState().setStatus({
        ...initialStatus,
        totalLines: 100 + revision,
        stableLines: 100 + revision,
        filteredLines: 50 + revision,
        sourceDataRevision: revision,
        filterResultRevision: revision,
      });
    });
  }

  it("keeps visible history while successive filtered append windows are loading", async () => {
    const { container } = render(<LogTable />);
    await complete(pending[0]);
    expect(screen.getByText("history-1")).toBeVisible();

    for (let revision = 2; revision <= 5; revision += 1) {
      append(revision);
      expect(screen.getByText("history-1")).toBeVisible();
      expect(screen.getByText("history-2")).toBeVisible();
      expect(container.querySelector(".lf-loading-row")).toBeNull();
      expect(pending[pending.length - 1].request).toMatchObject({
        expectedFilterResultRevision: revision,
      });
      await complete(pending[pending.length - 1]);
    }
  });

  it("does not let an obsolete request remove the current in-flight request", async () => {
    render(<LogTable />);
    await complete(pending[0]);
    append(2);
    const obsolete = pending[1];
    append(3);
    const current = pending[2];

    await complete(obsolete, "obsolete");
    act(() => useSession.getState().selectRow(1, 0));

    expect(pending).toHaveLength(3);
    expect(screen.queryByText("obsolete-1")).not.toBeInTheDocument();
    await complete(current, "current");
    expect(screen.getByText("current-1")).toBeVisible();
  });

  it.each([
    ["session", { generation: 2, analysisGeneration: 2 }],
    ["analysis", { analysisGeneration: 2 }],
    ["decoding", { decodeRevision: 1 }],
    ["applied filter", { filterInputRevision: 2, appliedFilterInputRevision: 2 }],
  ])("does not reuse history after changing the %s", async (_label, change) => {
    render(<LogTable />);
    await complete(pending[0]);

    act(() => {
      useSession.getState().setStatus({
        ...initialStatus,
        ...change,
        filterResultRevision: 2,
      });
    });

    expect(screen.queryByText("history-1")).not.toBeInTheDocument();
    await complete(pending[pending.length - 1], "replacement");
    expect(screen.getByText("replacement-1")).toBeVisible();
  });
});
