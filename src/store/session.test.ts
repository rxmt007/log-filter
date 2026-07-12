import { beforeEach, describe, expect, it } from "vitest";
import { DEFAULT_FILTER, LEVEL_BITS, useSession } from "@/store/session";

const status = {
  totalLines: 100,
  filteredLines: 100,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 1000,
  totalBytes: 1000,
  indexing: false,
  generation: 1,
};

describe("session store", () => {
  beforeEach(() => {
    useSession.setState({
      status,
      filter: DEFAULT_FILTER,
      filterRevision: 0,
      filterResultRevision: 0,
      selectedResultIndex: 50,
      viewportResultIndex: 75,
      scrollRequest: null,
      sourceMode: "file",
      selectedDeviceSerial: null,
      devices: [],
      streamRunning: false,
      streamPaused: false,
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
    useSession.setState({ sourceMode: "adb", tailFollowing: true });
    useSession.getState().pauseTailFollowing("row");
    expect(useSession.getState().tailFollowing).toBe(false);

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
    expect(useSession.getState().viewportResultIndex).toBe(99);
    expect(useSession.getState().scrollRequest).toMatchObject({
      index: 99,
      align: "end",
      reason: "tail",
    });
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
    useSession.setState({ sourceMode: "adb", tailFollowing: true });
    useSession.getState().setFilter({ markedOnly: true });
    expect(useSession.getState().tailFollowing).toBe(false);

    useSession.setState({ sourceMode: "adb", tailFollowing: true });
    useSession.getState().setFilterField("tagInclude", { pattern: "ActivityManager" });
    expect(useSession.getState().tailFollowing).toBe(false);

    useSession.setState({ sourceMode: "adb", tailFollowing: true });
    useSession.getState().toggleLevel(LEVEL_BITS.E);
    expect(useSession.getState().tailFollowing).toBe(false);
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
