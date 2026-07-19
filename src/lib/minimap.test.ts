import { describe, expect, it } from "vitest";
import {
  bucketRanges,
  errorTickStyle,
  MINIMAP_BUCKETS,
  pointerToResultIndex,
  rangeStyle,
} from "@/lib/minimap";

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

describe("errorTickStyle", () => {
  // 62k 行 / 180 桶 ≈ 345 行每桶:用户报告的真实密度。
  const SPARSE_ROWS = 62_000;

  it("makes opacity strictly increase with error count in a bucket", () => {
    const one = errorTickStyle({ bucket: 10, count: 1 }, SPARSE_ROWS).opacity;
    const five = errorTickStyle({ bucket: 10, count: 5 }, SPARSE_ROWS).opacity;
    const fifty = errorTickStyle({ bucket: 10, count: 50 }, SPARSE_ROWS).opacity;
    expect(one).toBeLessThan(five);
    expect(five).toBeLessThan(fifty);
  });

  it("renders a single sparse error faint (< 0.2) in a ~345-row bucket", () => {
    const { opacity } = errorTickStyle({ bucket: 3, count: 1 }, SPARSE_ROWS);
    expect(opacity).toBeLessThan(0.2);
    expect(opacity).toBeGreaterThan(0.16);
  });

  it("saturates to opacity 1 once a whole bucket is errors", () => {
    const rowsPerBucket = SPARSE_ROWS / MINIMAP_BUCKETS;
    const { opacity } = errorTickStyle(
      { bucket: 0, count: Math.round(rowsPerBucket) },
      SPARSE_ROWS,
    );
    expect(opacity).toBe(1);
  });

  it("clamps opacity within [0.16, 1] even for absurd counts", () => {
    const { opacity } = errorTickStyle({ bucket: 0, count: 1_000_000 }, SPARSE_ROWS);
    expect(opacity).toBeGreaterThanOrEqual(0.16);
    expect(opacity).toBeLessThanOrEqual(1);
    expect(opacity).toBe(1);
  });

  it("positions the tick from the bucket index and gives a floor height", () => {
    const style = errorTickStyle({ bucket: 90, count: 1 }, SPARSE_ROWS, 180);
    expect(style.top).toBe("50%");
    // 100 / 180 ≈ 0.5556% ⇒ 大于 0.55 的下限。
    expect(style.height).toBe("0.5556%");
  });

  it("treats a zero-row file defensively without dividing by zero", () => {
    const { opacity } = errorTickStyle({ bucket: 0, count: 1 }, 0);
    expect(opacity).toBe(1); // rowsPerBucket 兜底为 1 ⇒ density 1 ⇒ 饱和
  });
});
