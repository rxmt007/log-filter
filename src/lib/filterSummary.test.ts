import { describe, expect, it } from "vitest";
import { summarizeActiveFilters } from "@/lib/filterSummary";
import { DEFAULT_FILTER, LEVEL_BITS } from "@/store/session";

function filterFixture() {
  return structuredClone(DEFAULT_FILTER);
}

describe("summarizeActiveFilters", () => {
  it("omits defaults and enabled fields without an effective pattern", () => {
    const filter = filterFixture();
    filter.tagInclude = { enabled: true, pattern: "   ", regex: true };
    filter.pid = { enabled: false, pattern: "12043", regex: false };

    expect(summarizeActiveFilters(filter)).toEqual([]);
  });

  it("summarizes every effective filter in stable UI order", () => {
    const filter = filterFixture();
    filter.levels = LEVEL_BITS.W | LEVEL_BITS.E;
    filter.markedOnly = true;
    filter.tagInclude = { enabled: true, pattern: " ActivityManager ", regex: false };
    filter.tagExclude = { enabled: true, pattern: "chatty|GC", regex: true };
    filter.highlights[0] = {
      enabled: true,
      pattern: " timeout ",
      regex: true,
      caseSensitive: true,
      color: "yellow",
    };

    expect(summarizeActiveFilters(filter)).toEqual([
      "级别：W / E",
      "仅标记",
      "Tag 包含：ActivityManager",
      "Tag 屏蔽：chatty|GC（正则）",
      "高亮 1：timeout（正则、区分大小写）",
    ]);
  });

  it("makes an empty level mask explicit", () => {
    const filter = filterFixture();
    filter.levels = 0;

    expect(summarizeActiveFilters(filter)).toEqual(["级别：无"]);
  });
});
