import { describe, expect, it, vi } from "vitest";
import { createStreamAppendBatcher } from "@/lib/streamAppend";
import type { StreamAppend } from "@/types";

const status = {
  totalLines: 0,
  stableLines: 0,
  filteredLines: 0,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 0,
  totalBytes: 0,
  indexing: false,
  generation: 1,
  analysisGeneration: 1,
  filterInputRevision: 0,
  appliedFilterInputRevision: 0,
  filterResultRevision: 0,
  decodeRevision: 0,
  sourceDataRevision: 0,
};

function append(filteredLines: number, appendedBytes: number): StreamAppend {
  return {
    appendedBytes,
    deviceSerial: "usb",
    status: {
      ...status,
      totalLines: filteredLines,
      stableLines: filteredLines,
      filteredLines,
      indexedBytes: filteredLines * 10,
      totalBytes: filteredLines * 10,
    },
  };
}

describe("stream append batcher", () => {
  it("coalesces bursts into one flush with the latest status", () => {
    const callbacks: Array<() => void> = [];
    const flushed: StreamAppend[] = [];
    const batcher = createStreamAppendBatcher({
      delayMs: 75,
      onFlush: (payload) => flushed.push(payload),
      timer: {
        setTimeout: (callback) => {
          callbacks.push(callback);
          return callbacks.length;
        },
        clearTimeout: vi.fn(),
      },
    });

    batcher.push(append(10, 10));
    batcher.push(append(25, 15));
    batcher.push(append(40, 20));

    expect(flushed).toEqual([]);
    expect(callbacks).toHaveLength(1);

    callbacks[0]();

    expect(flushed).toHaveLength(1);
    expect(flushed[0]).toMatchObject({
      appendedBytes: 45,
      status: { filteredLines: 40, totalLines: 40 },
    });
  });

  it("drops pending work when disposed", () => {
    const clearTimeout = vi.fn();
    const callbacks: Array<() => void> = [];
    const flushed: StreamAppend[] = [];
    const batcher = createStreamAppendBatcher({
      onFlush: (payload) => flushed.push(payload),
      timer: {
        setTimeout: (callback) => {
          callbacks.push(callback);
          return 123;
        },
        clearTimeout,
      },
    });

    batcher.push(append(10, 10));
    batcher.dispose();
    callbacks[0]();

    expect(clearTimeout).toHaveBeenCalledWith(123);
    expect(flushed).toEqual([]);
  });
});
