import { describe, expect, it } from "vitest";
import { PROBLEM_KINDS, problemHints } from "@/lib/problemHints";

describe("problem hints", () => {
  it("covers every backend problem kind with investigation prompts", () => {
    for (const kind of PROBLEM_KINDS) {
      const hints = problemHints(kind);
      expect(hints.length).toBeGreaterThan(0);
      expect(hints.every((hint) => hint.trim().length > 0)).toBe(true);
    }
  });

  it("keeps prompts explicitly non-conclusive", () => {
    const prohibited = ["根因是", "确定为", "已导致", "内存泄漏", "代码缺陷"];
    for (const kind of PROBLEM_KINDS) {
      const text = problemHints(kind).join("\n");
      for (const phrase of prohibited) {
        expect(text).not.toContain(phrase);
      }
    }
  });

  it("returns immutable catalog entries rather than accepting detected facts", () => {
    const first = problemHints("anr");
    const second = problemHints("anr");
    expect(first).toBe(second);
    expect(Object.isFrozen(first)).toBe(true);
  });
});
