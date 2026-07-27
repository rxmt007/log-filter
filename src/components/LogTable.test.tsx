import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { LogTable } from "@/components/LogTable";
import { DEFAULT_FILTER, useSession } from "@/store/session";
import type { CheckedRowsRequest, CheckedRowsResponse, Status } from "@/types";

const mocks = vi.hoisted(() => {
  const scrollToIndex = vi.fn();
  return {
    getRowsChecked: vi.fn(),
    scrollToIndex,
    virtualizer: {
      getTotalSize: () => 2_000,
      getVirtualItems: () => [],
      scrollOffset: 0,
      scrollToIndex,
    },
  };
});

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: () => mocks.virtualizer,
}));

vi.mock("@/lib/ipc", () => ({
  getRowsChecked: mocks.getRowsChecked,
  listBookmarks: vi.fn(async () => []),
  saveAppConfig: vi.fn(async (config) => config),
  toggleBookmark: vi.fn(async () => false),
}));

const status: Status = {
  totalLines: 100,
  stableLines: 100,
  filteredLines: 100,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 1_000,
  totalBytes: 1_000,
  indexing: false,
  generation: 1,
  analysisGeneration: 1,
  filterInputRevision: 0,
  appliedFilterInputRevision: 0,
  filterResultRevision: 0,
  decodeRevision: 0,
  sourceDataRevision: 1,
};

describe("LogTable live tail following", () => {
  beforeEach(() => {
    mocks.getRowsChecked.mockReset();
    mocks.scrollToIndex.mockReset();
    useSession.setState({
      status,
      sourceMode: "adb",
      streamRunning: true,
      tailFollowing: true,
      filter: structuredClone(DEFAULT_FILTER),
      tableScope: { kind: "results", view: "filtered" },
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: {
        index: 99,
        align: "end",
        reason: "tail",
        nonce: 7,
      },
    });
  });

  it.each([
    ["without an active filter", false],
    ["with an active filter", true],
  ])("cancels an in-flight tail scroll %s", async (_label, activeFilter) => {
    if (activeFilter) {
      useSession.getState().setFilterField("tagInclude", {
        enabled: true,
        pattern: "Activity",
      });
    }
    useSession.setState({
      tailFollowing: true,
      selectedLine: 42,
      selectedResultIndex: 41,
    });
    useSession.getState().requestTailFollow(99);
    let resolveRows: ((response: CheckedRowsResponse) => void) | null = null;
    mocks.getRowsChecked.mockReturnValue(
      new Promise<CheckedRowsResponse>((resolve) => {
        resolveRows = resolve;
      }),
    );
    const { container } = render(<LogTable />);
    const scroller = container.querySelector<HTMLElement>(".lf-table-scroll");
    expect(scroller).not.toBeNull();
    expect(mocks.getRowsChecked).toHaveBeenCalledTimes(1);

    fireEvent.wheel(scroller!, { deltaY: -120 });

    expect(useSession.getState().tailFollowing).toBe(false);
    expect(useSession.getState().scrollRequest).toBeNull();
    expect(useSession.getState().selectedLine).toBe(42);
    expect(useSession.getState().selectedResultIndex).toBe(41);

    await act(async () => {
      resolveRows?.({
        status: "ok",
        analysisToken: {
          sessionGeneration: 1,
          analysisGeneration: 1,
        },
        rows: [],
        requestNonce: 1,
        decodeRevision: 0,
        sourceDataRevision: 1,
        filterResultRevision: 0,
      });
      await Promise.resolve();
    });

    expect(mocks.scrollToIndex).not.toHaveBeenCalled();

    act(() => {
      useSession.getState().setStatus({
        ...status,
        totalLines: 101,
        stableLines: 101,
        filteredLines: 101,
        sourceDataRevision: 2,
      });
    });

    expect(mocks.scrollToIndex).not.toHaveBeenCalled();
  });

  it("consumes a completed tail request so later appends cannot replay it", async () => {
    mocks.getRowsChecked.mockImplementation(async (request: CheckedRowsRequest) => ({
      status: "ok",
      analysisToken: request.expectedAnalysisToken,
      rows: [],
      requestNonce: request.requestNonce,
      decodeRevision: 0,
      sourceDataRevision: 1,
      filterResultRevision: 0,
    }));
    render(<LogTable />);

    await waitFor(() => expect(mocks.scrollToIndex).toHaveBeenCalledTimes(1));
    expect(useSession.getState().scrollRequest).toBeNull();

    act(() => {
      useSession.getState().setStatus({
        ...status,
        totalLines: 101,
        stableLines: 101,
        filteredLines: 101,
        sourceDataRevision: 2,
      });
    });

    expect(mocks.scrollToIndex).toHaveBeenCalledTimes(1);
  });

  it("consumes a selected-row navigation so appends cannot jump back to it", async () => {
    useSession.setState({
      selectedLine: 42,
      selectedResultIndex: 41,
      scrollRequest: {
        index: 41,
        align: "center",
        reason: "bookmark",
        nonce: 9,
      },
    });
    render(<LogTable />);

    await waitFor(() => expect(mocks.scrollToIndex).toHaveBeenCalledWith(41, { align: "center" }));
    expect(useSession.getState().scrollRequest).toBeNull();

    act(() => {
      useSession.getState().setStatus({
        ...status,
        totalLines: 101,
        stableLines: 101,
        filteredLines: 101,
        sourceDataRevision: 2,
      });
    });

    expect(mocks.scrollToIndex).toHaveBeenCalledTimes(1);
  });

  it("states that filters are preserved but temporarily not applied in problem context", () => {
    useSession.setState({
      sourceMode: "file",
      streamRunning: false,
      scrollRequest: null,
      tableScope: {
        kind: "problem-context",
        occurrence: {
          eventId: 8,
          groupId: 2,
          startLine: 50,
          endLine: 55,
          anchorLine: 52,
        },
        eventRange: { startLine: 50, endLine: 55 },
        contextRange: { startLine: 20, endLine: 80 },
        returnPoint: {
          viewportLine: 12,
          selectedLine: 14,
          filterInputRevision: 3,
        },
      },
    });

    render(<LogTable />);

    const banner = screen.getByRole("status");
    expect(banner).toHaveTextContent("当前过滤保持，但暂不应用于此上下文");
    expect(banner).not.toHaveTextContent("这里显示未经过当前筛选的原始日志窗口");
  });

  it("announces the return to filtered results and moves focus into the main table", async () => {
    useSession.setState({
      sourceMode: "file",
      streamRunning: false,
      scrollRequest: null,
      tableScope: {
        kind: "problem-context",
        occurrence: {
          eventId: 8,
          groupId: 2,
          startLine: 50,
          endLine: 55,
          anchorLine: 52,
        },
        eventRange: { startLine: 50, endLine: 55 },
        contextRange: { startLine: 20, endLine: 80 },
        returnPoint: {
          viewportLine: 12,
          selectedLine: 14,
          filterInputRevision: 3,
        },
      },
    });
    const onReturnToResults = vi.fn(() => {
      useSession.setState({ tableScope: { kind: "results", view: "filtered" } });
    });
    render(<LogTable onReturnToResults={onReturnToResults} />);
    const returnButton = screen.getByRole("button", { name: "返回筛选结果" });
    returnButton.focus();

    await userEvent.click(returnButton);

    expect(onReturnToResults).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("status", { name: "日志表格状态" })).toHaveTextContent(
      "已返回筛选结果",
    );
    expect(screen.getByRole("region", { name: "日志表格" })).toHaveFocus();
  });
});
