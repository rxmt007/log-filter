import { describe, expect, it } from "vitest";
import { bucketRanges, pointerToResultIndex, rangeStyle } from "@/lib/minimap";

describe("minimap helpers", () => {
  it("merges adjacent bucket hits but keeps sparse hits separate", () => {
    expect(bucketRanges([5, 1, 2, 9, 8, 2])).toEqual([
      { start: 1, end: 2 },
      { start: 5, end: 5 },
      { start: 8, end: 9 },
    ]);
  });

  it("converts bucket ranges into stable percentage styles", () => {
    expect(rangeStyle({ start: 9, end: 10 }, 100)).toEqual({
      top: "9%",
      height: "2%",
    });
  });

  it("clamps pointer positions into the current result set", () => {
    const rect = { top: 100, height: 200 };

    expect(pointerToResultIndex(50, rect, 100, 0)).toBe(0);
    expect(pointerToResultIndex(300, rect, 100, 0)).toBe(99);
    expect(pointerToResultIndex(200, rect, 100, 0)).toBe(56);
  });
});
