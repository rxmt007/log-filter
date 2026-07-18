import { describe, expect, it } from "vitest";
import { RowBlockCache } from "@/lib/rowCache";
import type { Row } from "@/types";

function makeRows(start: number, count: number): Row[] {
  return Array.from({ length: count }, (_, i) => ({
    lineNo: start + i,
    date: "",
    time: "",
    level: "",
    pid: "",
    tid: "",
    tag: "",
    message: `m${start + i}`,
    marked: false,
  }));
}

const BLOCK = 200;

describe("RowBlockCache", () => {
  it("returns filled rows by index and reports isFresh for the filled block", () => {
    const cache = new RowBlockCache(64);
    const rows = makeRows(0, BLOCK);
    cache.fill(0, rows, 1);

    for (let i = 0; i < BLOCK; i += 1) {
      expect(cache.get(i, BLOCK)?.message).toBe(`m${i}`);
    }
    expect(cache.isFresh(0, rows.length, 1)).toBe(true);
  });

  it("reports not fresh when want exceeds the filled count (growing tail block)", () => {
    const cache = new RowBlockCache(64);
    cache.fill(0, makeRows(0, 50), 1);

    expect(cache.isFresh(0, 50, 1)).toBe(true);
    expect(cache.isFresh(0, 80, 1)).toBe(false);
  });

  it("reports not fresh for a stale epoch but still returns the stale rows (anti-flicker)", () => {
    const cache = new RowBlockCache(64);
    cache.fill(0, makeRows(0, BLOCK), 1);

    expect(cache.isFresh(0, BLOCK, 2)).toBe(false);
    expect(cache.get(5, BLOCK)?.message).toBe("m5");
  });

  it("evicts the least-recently-used block when exceeding maxBlocks", () => {
    const cache = new RowBlockCache(3);
    cache.fill(0, makeRows(0, BLOCK), 1); // block A
    cache.fill(BLOCK, makeRows(BLOCK, BLOCK), 1); // block B
    cache.fill(2 * BLOCK, makeRows(2 * BLOCK, BLOCK), 1); // block C

    // touch A so it becomes most-recently-used
    expect(cache.get(0, BLOCK)?.message).toBe("m0");

    cache.fill(3 * BLOCK, makeRows(3 * BLOCK, BLOCK), 1); // block D evicts B (LRU)

    expect(cache.blockCount()).toBe(3);
    expect(cache.get(BLOCK, BLOCK)).toBeUndefined(); // B evicted
    expect(cache.get(0, BLOCK)?.message).toBe("m0"); // A survives
    expect(cache.get(2 * BLOCK, BLOCK)?.message).toBe(`m${2 * BLOCK}`); // C survives
    expect(cache.get(3 * BLOCK, BLOCK)?.message).toBe(`m${3 * BLOCK}`); // D present
  });

  it("updateRows rewrites the marked field on resident rows", () => {
    const cache = new RowBlockCache(64);
    cache.fill(0, makeRows(0, 3), 1);

    cache.updateRows((row) => (row.lineNo === 1 ? { ...row, marked: true } : row));

    expect(cache.get(0, BLOCK)?.marked).toBe(false);
    expect(cache.get(1, BLOCK)?.marked).toBe(true);
    expect(cache.get(2, BLOCK)?.marked).toBe(false);
  });

  it("clear removes every block and resets blockCount", () => {
    const cache = new RowBlockCache(64);
    cache.fill(0, makeRows(0, BLOCK), 1);
    cache.fill(BLOCK, makeRows(BLOCK, BLOCK), 1);

    cache.clear();

    expect(cache.blockCount()).toBe(0);
    expect(cache.get(0, BLOCK)).toBeUndefined();
    expect(cache.get(BLOCK, BLOCK)).toBeUndefined();
  });
});
