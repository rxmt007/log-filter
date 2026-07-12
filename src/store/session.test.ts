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
});
