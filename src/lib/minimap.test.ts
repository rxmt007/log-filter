import { describe, expect, it } from "vitest";
import {
  bucketRanges,
  errorTickStyle,
  indexToViewportTopPx,
  maxViewportStartIndex,
  MINIMAP_BUCKETS,
  pointerToResultIndex,
  rangeStyle,
  viewportHeightPx,
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

    expect(pointerToResultIndex(50, rect, 100, 10, 0)).toBe(0);
    expect(pointerToResultIndex(300, rect, 100, 10, 0)).toBe(90);
    expect(pointerToResultIndex(200, rect, 100, 10, 0)).toBe(51);
  });

  it("sizes and maps the viewport from visible rows in the filtered result set", () => {
    const rect = { top: 0, height: 500 };

    expect(viewportHeightPx(rect, 40, 20)).toBe(250);
    expect(indexToViewportTopPx(20, rect, 40, 20)).toBe(250);
    expect(pointerToResultIndex(500, rect, 40, 20, 0)).toBe(20);
  });

  it("fills the track and disables its range when every result is visible", () => {
    const rect = { top: 0, height: 500 };

    expect(viewportHeightPx(rect, 10, 20)).toBe(500);
    expect(indexToViewportTopPx(8, rect, 10, 20)).toBe(0);
    expect(pointerToResultIndex(400, rect, 10, 20, 0)).toBe(0);
  });

  it("rounds a partially visible final row into the same integer viewport model", () => {
    const rect = { top: 0, height: 500 };

    expect(maxViewportStartIndex(11, 10.5)).toBe(0);
    expect(viewportHeightPx(rect, 11, 10.5)).toBe(500);
    expect(pointerToResultIndex(500, rect, 11, 10.5, 0)).toBe(0);
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

  it("covers a short all-error result set continuously with its effective buckets", () => {
    const styles = [0, 1, 2, 3].map((bucket) => errorTickStyle({ bucket, count: 1 }, 4, 4));

    expect(styles).toEqual([
      { top: "0%", height: "25%", opacity: 1 },
      { top: "25%", height: "25%", opacity: 1 },
      { top: "50%", height: "25%", opacity: 1 },
      { top: "75%", height: "25%", opacity: 1 },
    ]);
  });
});
