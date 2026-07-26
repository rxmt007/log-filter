import { beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_FILTER, LEVEL_BITS, useSession } from "@/store/session";

const status = {
  totalLines: 100,
  stableLines: 100,
  filteredLines: 100,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 1000,
  totalBytes: 1000,
  indexing: false,
  generation: 1,
  analysisGeneration: 1,
  filterInputRevision: 0,
  appliedFilterInputRevision: 0,
  filterResultRevision: 0,
  decodeRevision: 0,
  sourceDataRevision: 0,
};

describe("session store", () => {
  beforeEach(() => {
    useSession.setState({
      status,
      filter: DEFAULT_FILTER,
      filterRevision: 0,
      appliedFilterInputRevision: 0,
      filterResultRevision: 0,
      searchRevision: 0,
      selectedResultIndex: 50,
      viewportLine: 76,
      viewportResultIndex: 75,
      scrollRequest: null,
      tableScope: { kind: "results", view: "filtered" },
      contextRows: null,
      sourceMode: "file",
      selectedDeviceSerial: null,
      devices: [],
      streamRunning: false,
      streamPaused: false,
      streamLifecycle: "stopped",
      streamError: null,
    });
  });

  it("toggles level bits and bumps the filter revision", () => {
    useSession.getState().toggleLevel(LEVEL_BITS.E);

    expect(useSession.getState().filter.levels & LEVEL_BITS.E).toBe(0);
    expect(useSession.getState().filterRevision).toBe(1);
  });

  it("clamps selected and viewport result indexes after result count changes", () => {
    useSession.getState().setFilteredLines(20);

    expect(useSession.getState().selectedResultIndex).toBeNull();
    expect(useSession.getState().viewportResultIndex).toBe(19);
    expect(useSession.getState().status.filteredLines).toBe(20);
    expect(useSession.getState().filterResultRevision).toBe(1);
  });

  it("updates appended filter counts without invalidating loaded row windows", () => {
    useSession.getState().setFilteredLines(120, { invalidateRows: false });

    expect(useSession.getState().selectedResultIndex).toBe(50);
    expect(useSession.getState().viewportResultIndex).toBe(75);
    expect(useSession.getState().status.filteredLines).toBe(120);
    expect(useSession.getState().filterResultRevision).toBe(0);
  });

  it("accepts only the exact requested filter completion and publishes its dataset identity", () => {
    useSession.setState({
      filterRevision: 5,
      appliedFilterInputRevision: 4,
      filterResultRevision: 8,
      status: {
        ...status,
        filterInputRevision: 5,
        appliedFilterInputRevision: 4,
        filterResultRevision: 8,
      },
    });

    useSession.getState().applyFilterDone({
      generation: 1,
      filteredLines: 12,
      filterInputRevision: 4,
      filterResultRevision: 9,
    });
    expect(useSession.getState().status.filteredLines).toBe(100);

    useSession.getState().applyFilterDone({
      generation: 1,
      filteredLines: 12,
      filterInputRevision: 5,
      filterResultRevision: 9,
    });
    expect(useSession.getState()).toMatchObject({
      appliedFilterInputRevision: 5,
      filterResultRevision: 9,
      status: {
        filteredLines: 12,
        appliedFilterInputRevision: 5,
        filterResultRevision: 9,
      },
    });
  });

  it("publishes a filter completion without clamping an active unfiltered context", () => {
    useSession.setState({
      tableScope: {
        kind: "problem-context",
        occurrence: { eventId: 3, groupId: 2, startLine: 40, endLine: 44, anchorLine: 42 },
        eventRange: { startLine: 40, endLine: 44 },
        contextRange: { startLine: 20, endLine: 90 },
        returnPoint: { viewportLine: 76, selectedLine: 82, filterInputRevision: 0 },
      },
      selectedLine: 82,
      selectedResultIndex: 81,
      viewportLine: 76,
      viewportResultIndex: 75,
    });

    useSession.getState().applyFilterDone({
      generation: 1,
      filteredLines: 2,
      filterInputRevision: 0,
      filterResultRevision: 1,
    });

    expect(useSession.getState()).toMatchObject({
      selectedLine: 82,
      selectedResultIndex: 81,
      viewportLine: 76,
      viewportResultIndex: 75,
      status: {
        filteredLines: 2,
        filterResultRevision: 1,
      },
    });
  });

  it("allocates a new filter request when marked-only membership changes", () => {
    useSession.setState({
      filter: { ...DEFAULT_FILTER, markedOnly: true },
      filterRevision: 7,
      status: { ...status, filterInputRevision: 7 },
    });

    useSession.getState().setBookmarks([12, 42]);
    expect(useSession.getState().filterRevision).toBe(8);
  });

  it("does not regress a dataset identity from delayed status events", () => {
    useSession.setState({
      status: {
        ...status,
        sourceDataRevision: 9,
        filterResultRevision: 4,
      },
      filterResultRevision: 4,
    });

    useSession.getState().setStatus({
      ...status,
      stableLines: 120,
      sourceDataRevision: 8,
      filterResultRevision: 5,
    });

    expect(useSession.getState().status).toMatchObject({
      stableLines: 100,
      sourceDataRevision: 9,
      filterResultRevision: 4,
    });
  });

  it("commits context navigation atomically only for the guarded dataset", () => {
    const token = { sessionGeneration: 1, analysisGeneration: 1 };
    const scope = {
      kind: "problem-context" as const,
      occurrence: { eventId: 3, groupId: 2, startLine: 40, endLine: 44, anchorLine: 42 },
      eventRange: { startLine: 40, endLine: 44 },
      contextRange: { startLine: 20, endLine: 64 },
      returnPoint: { viewportLine: 76, selectedLine: null, filterInputRevision: 0 },
    };
    const update = {
      scope,
      selectedLine: 42,
      selectedResultIndex: 41,
      viewportLine: 42,
      viewportResultIndex: 41,
      scrollRequest: null,
      tailFollowing: false,
    };
    const guard = {
      expectedScopeKind: "results" as const,
      expectedSessionGeneration: 1,
      expectedAnalysisToken: token,
      expectedFilterInputRevision: 0,
      expectedAppliedFilterInputRevision: 0,
      expectedFilterResultRevision: 0,
    };

    expect(useSession.getState().commitTableNavigation(update, guard)).toBe(true);
    expect(useSession.getState().tableScope).toEqual(scope);
    expect(useSession.getState().commitTableNavigation(update, guard)).toBe(false);
  });

  it("selects the first online adb device and applies stream control state", () => {
    useSession.getState().setDevices([
      { serial: "offline", state: "offline", model: null, product: null, online: false },
      { serial: "usb", state: "device", model: "Pixel", product: null, online: true },
    ]);

    expect(useSession.getState().selectedDeviceSerial).toBe("usb");

    useSession.getState().setStreamControl({
      status: { ...status, totalLines: 2, filteredLines: 2 },
      running: true,
      paused: false,
      lifecycle: "running",
      error: null,
      deviceSerial: "usb",
      sessionPath: "/tmp/logcat.log",
    });

    expect(useSession.getState()).toMatchObject({
      sourceMode: "adb",
      streamRunning: true,
      streamPaused: false,
      sourcePath: "/tmp/logcat.log",
      selectedDeviceSerial: "usb",
    });
  });

  it("pauses and restores tail following through explicit actions", () => {
    useSession.setState({
      sourceMode: "adb",
      tailFollowing: true,
      scrollRequest: {
        index: 99,
        align: "end",
        reason: "tail",
        nonce: 3,
      },
    });
    useSession.getState().pauseTailFollowing("row");
    expect(useSession.getState().tailFollowing).toBe(false);
    expect(useSession.getState().scrollRequest).toBeNull();

    useSession.getState().setTailFollowingFromViewport(false, "program");
    expect(useSession.getState().tailFollowing).toBe(false);

    useSession.getState().setTailFollowingFromViewport(true, "user");
    expect(useSession.getState().tailFollowing).toBe(true);
  });

  it("requests tail viewport scrolling without changing the selected row", () => {
    useSession.setState({
      sourceMode: "adb",
      tailFollowing: true,
      selectedLine: 42,
      selectedResultIndex: 41,
      viewportResultIndex: 41,
      scrollRequest: null,
    });

    useSession.getState().requestTailFollow(99);

    expect(useSession.getState().selectedLine).toBe(42);
    expect(useSession.getState().selectedResultIndex).toBe(41);
    expect(useSession.getState().viewportResultIndex).toBe(41);
    expect(useSession.getState().scrollRequest).toMatchObject({
      index: 99,
      align: "end",
      reason: "tail",
    });
  });

  it("consumes only the matching one-shot scroll request", () => {
    useSession.setState({
      scrollRequest: {
        index: 99,
        align: "end",
        reason: "tail",
        nonce: 3,
      },
    });

    useSession.getState().acknowledgeScrollRequest(2);
    expect(useSession.getState().scrollRequest?.nonce).toBe(3);

    useSession.getState().acknowledgeScrollRequest(3);
    expect(useSession.getState().scrollRequest).toBeNull();
  });

  it("keeps the selected row when empty search results are refreshed during streaming", () => {
    useSession.setState({
      sourceMode: "adb",
      selectedLine: 42,
      selectedResultIndex: 41,
      currentSearchLine: null,
      searchCount: 0,
    });

    useSession.getState().setSearchResult(0, null);

    expect(useSession.getState().selectedLine).toBe(42);
    expect(useSession.getState().selectedResultIndex).toBe(41);
    expect(useSession.getState().currentSearchLine).toBeNull();
    expect(useSession.getState().searchCount).toBe(0);
  });

  it("pauses tail following when filter conditions change", () => {
    const tailRequest = {
      index: 99,
      align: "end" as const,
      reason: "tail" as const,
      nonce: 1,
    };
    useSession.setState({
      sourceMode: "adb",
      tailFollowing: true,
      scrollRequest: tailRequest,
    });
    useSession.getState().setFilter({ markedOnly: true });
    expect(useSession.getState().tailFollowing).toBe(false);
    expect(useSession.getState().scrollRequest).toBeNull();

    useSession.setState({
      sourceMode: "adb",
      tailFollowing: true,
      scrollRequest: tailRequest,
    });
    useSession.getState().setFilterField("tagInclude", { pattern: "ActivityManager" });
    expect(useSession.getState().tailFollowing).toBe(false);
    expect(useSession.getState().scrollRequest).toBeNull();

    useSession.setState({
      sourceMode: "adb",
      tailFollowing: true,
      scrollRequest: tailRequest,
    });
    useSession.getState().toggleLevel(LEVEL_BITS.E);
    expect(useSession.getState().tailFollowing).toBe(false);
    expect(useSession.getState().scrollRequest).toBeNull();
  });

  it("initializes adb sessions with tail following enabled and file sessions disabled", () => {
    useSession.getState().beginSession(status, "/tmp/logcat.log", "adb");
    expect(useSession.getState().tailFollowing).toBe(true);

    useSession.getState().beginSession(status, "/tmp/file.log", "file");
    expect(useSession.getState().tailFollowing).toBe(false);
  });

  it("defaults to adb mode before any file is opened", () => {
    expect(useSession.getInitialState().sourceMode).toBe("adb");
  });
});
