import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { useSession } from "@/store/session";
import type {
  CheckedRowsRequest,
  LineMappingRequest,
  SearchProgress,
  Status,
  StreamAppend,
} from "@/types";

const mocks = vi.hoisted(() => ({
  getConfig: vi.fn(),
  getRowsChecked: vi.fn(),
  mapSourceLine: vi.fn(),
  nextBookmark: vi.fn(),
  onExportProgress: vi.fn(),
  onFilterDone: vi.fn(),
  onIndexProgress: vi.fn(),
  onSearchProgress: vi.fn(),
  onStreamAppend: vi.fn(),
  onStreamControl: vi.fn(),
  onStreamError: vi.fn(),
  saveAppConfig: vi.fn(),
  setFilter: vi.fn(),
  searchProgressListener: null as ((progress: SearchProgress) => void) | null,
  streamAppendListener: null as ((append: StreamAppend) => void) | null,
  scrollToIndex: vi.fn(),
}));

vi.mock("@/lib/ipc", () => ({
  getConfig: mocks.getConfig,
  getRowsChecked: mocks.getRowsChecked,
  listBookmarks: vi.fn(async () => []),
  mapSourceLine: mocks.mapSourceLine,
  nextBookmark: mocks.nextBookmark,
  onExportProgress: mocks.onExportProgress,
  onFilterDone: mocks.onFilterDone,
  onIndexProgress: mocks.onIndexProgress,
  onSearchProgress: mocks.onSearchProgress,
  onStreamAppend: mocks.onStreamAppend,
  onStreamControl: mocks.onStreamControl,
  onStreamError: mocks.onStreamError,
  saveAppConfig: mocks.saveAppConfig,
  setFilter: mocks.setFilter,
  toggleBookmark: vi.fn(async () => false),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ setSize: vi.fn(async () => {}) }),
  LogicalSize: class {
    constructor(
      readonly width: number,
      readonly height: number,
    ) {}
  },
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn(async () => {}),
}));

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: () => ({
    getTotalSize: () => 0,
    getVirtualItems: () => [],
    scrollOffset: 0,
    scrollToIndex: mocks.scrollToIndex,
  }),
}));

vi.mock("@/hooks/useProblemsLive", () => ({
  useProblemsLive: () => ({}),
}));

vi.mock("@/components/Toolbar", () => ({ Toolbar: () => null }));
vi.mock("@/components/Minimap", () => ({ Minimap: () => null }));
vi.mock("@/components/ProblemsDock", () => ({ ProblemsDock: () => null }));
vi.mock("@/components/ProblemExportDialog", () => ({ ProblemExportDialog: () => null }));
vi.mock("@/components/StatusBar", () => ({ StatusBar: () => null }));
vi.mock("@/components/Toast", () => ({ Toast: () => null }));

const status: Status = {
  totalLines: 100,
  stableLines: 100,
  filteredLines: 37,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 1_000,
  totalBytes: 1_000,
  indexing: false,
  generation: 7,
  analysisGeneration: 3,
  filterInputRevision: 4,
  appliedFilterInputRevision: 4,
  filterResultRevision: 9,
  decodeRevision: 0,
  sourceDataRevision: 1,
};

const originalRequestTailFollow = useSession.getState().requestTailFollow;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

const subscription = (assign: (listener: never) => void) =>
  vi.fn((listener: never) => {
    assign(listener);
    return Promise.resolve(vi.fn());
  });

describe("App TableScope navigation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.streamAppendListener = null;
    mocks.searchProgressListener = null;
    mocks.getConfig.mockResolvedValue({
      ...structuredClone(useSession.getState().appConfig),
      lastFilter: null,
    });
    mocks.getRowsChecked.mockImplementation(async (request: CheckedRowsRequest) => ({
      status: "ok",
      analysisToken: request.expectedAnalysisToken,
      rows: [],
      requestNonce: request.requestNonce,
      decodeRevision: 0,
      sourceDataRevision: 2,
      filterResultRevision: request.expectedFilterResultRevision,
    }));
    mocks.mapSourceLine.mockImplementation(async (request: LineMappingRequest) => ({
      status: "ok",
      analysisToken: request.expectedAnalysisToken,
      filterResultRevision: request.expectedFilterResultRevision,
      target: { lineNo: request.lineNo, resultIndex: 39 },
    }));
    mocks.saveAppConfig.mockImplementation(async (config) => config);
    mocks.setFilter.mockResolvedValue(undefined);
    mocks.nextBookmark.mockResolvedValue(null);
    mocks.onExportProgress.mockReturnValue(Promise.resolve(vi.fn()));
    mocks.onFilterDone.mockReturnValue(Promise.resolve(vi.fn()));
    mocks.onIndexProgress.mockReturnValue(Promise.resolve(vi.fn()));
    mocks.onSearchProgress.mockImplementation(
      subscription((listener) => {
        mocks.searchProgressListener = listener as (progress: SearchProgress) => void;
      }),
    );
    mocks.onStreamAppend.mockImplementation(
      subscription((listener) => {
        mocks.streamAppendListener = listener as (append: StreamAppend) => void;
      }),
    );
    mocks.onStreamControl.mockReturnValue(Promise.resolve(vi.fn()));
    mocks.onStreamError.mockReturnValue(Promise.resolve(vi.fn()));
    useSession.setState({
      status,
      sourceMode: "adb",
      streamRunning: true,
      tailFollowing: true,
      tableScope: { kind: "results", view: "filtered" },
      filterRevision: 4,
      appliedFilterInputRevision: 4,
      filterResultRevision: 9,
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: null,
      currentSearchLine: null,
      searchRevision: 12,
      requestTailFollow: originalRequestTailFollow,
    });
  });

  it("maps a stream append's latest stable source line through the table controller", async () => {
    const legacyTailFollow = vi.fn();
    useSession.setState({ requestTailFollow: legacyTailFollow });
    render(<App />);
    await waitFor(() => expect(mocks.streamAppendListener).not.toBeNull());

    act(() => {
      mocks.streamAppendListener?.({
        appendedBytes: 200,
        deviceSerial: "tv-box",
        status: {
          ...status,
          totalLines: 120,
          stableLines: 120,
          filteredLines: 40,
          indexedBytes: 1_200,
          totalBytes: 1_200,
          sourceDataRevision: 2,
        },
      });
    });

    await waitFor(() =>
      expect(mocks.mapSourceLine).toHaveBeenCalledWith(
        expect.objectContaining({
          lineNo: 120,
          bias: "nearest",
          expectedFilterResultRevision: 9,
        }),
      ),
    );
    expect(legacyTailFollow).not.toHaveBeenCalled();
  });

  it("leaves problem context before following the latest stable source line", async () => {
    const mapping = deferred<{
      status: "ok";
      analysisToken: { sessionGeneration: number; analysisGeneration: number };
      filterResultRevision: number;
      target: { lineNo: number; resultIndex: number };
    }>();
    mocks.mapSourceLine.mockReturnValue(mapping.promise);
    useSession.setState({
      tailFollowing: false,
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
          filterInputRevision: 4,
        },
      },
    });
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "Follow latest" }));
    await waitFor(() =>
      expect(mocks.mapSourceLine).toHaveBeenCalledWith(
        expect.objectContaining({
          lineNo: 100,
          bias: "nearest",
          expectedFilterResultRevision: 9,
        }),
      ),
    );
    expect(useSession.getState()).toMatchObject({
      tableScope: { kind: "results", view: "filtered" },
      tailFollowing: false,
    });

    mapping.resolve({
      status: "ok",
      analysisToken: { sessionGeneration: 7, analysisGeneration: 3 },
      filterResultRevision: 9,
      target: { lineNo: 100, resultIndex: 36 },
    });

    await waitFor(() =>
      expect(useSession.getState()).toMatchObject({
        tableScope: { kind: "results", view: "filtered" },
        tailFollowing: true,
        selectedLine: 100,
        selectedResultIndex: 36,
        scrollRequest: expect.objectContaining({ reason: "tail", index: 36 }),
      }),
    );
  });

  it("locates a completed search through the controller scroll request only", async () => {
    render(<App />);
    await waitFor(() => expect(mocks.searchProgressListener).not.toBeNull());

    act(() => {
      mocks.searchProgressListener?.({
        scanned: 100,
        matches: 3,
        firstLine: 84,
        done: true,
        generation: 7,
        requestId: 12,
      });
    });

    await waitFor(() =>
      expect(mocks.mapSourceLine).toHaveBeenCalledWith(
        expect.objectContaining({
          lineNo: 84,
          bias: "exact",
          expectedFilterResultRevision: 9,
        }),
      ),
    );
    await waitFor(() => expect(mocks.scrollToIndex).toHaveBeenCalledWith(39, { align: "center" }));
    expect(mocks.scrollToIndex).not.toHaveBeenCalledWith(83, { align: "center" });
  });
});
