import { describe, expect, it } from "vitest";
import {
  appendSnapshotPage,
  problemFactLabel,
  problemKindLabel,
  PROBLEM_FACT_CODES,
} from "@/lib/problems";
import type { ProblemFactCode, ProblemGroup, ProblemPage } from "@/types";

const group = (id: number): ProblemGroup => ({
  id,
  kind: "java-crash",
  observedOccurrenceCount: 1,
  storedOccurrenceCount: 1,
  droppedOccurrenceCount: 0,
  firstLine: id * 10,
  lastLine: id * 10,
  representativeEventId: id,
});

const page = (
  snapshot: number,
  offsetItems: ProblemGroup[],
  nextOffset: number | null,
): ProblemPage<ProblemGroup> => ({
  querySnapshotId: snapshot,
  revision: 4,
  total: 4,
  items: offsetItems,
  nextOffset,
});

describe("problems pagination", () => {
  it("appends one frozen snapshot without duplicate ids", () => {
    const first = page(9, [group(1), group(2)], 2);
    const second = page(9, [group(2), group(3)], 4);

    expect(appendSnapshotPage(first, second, (item) => item.id)).toEqual({
      ...second,
      items: [group(1), group(2), group(3)],
    });
  });

  it("rejects pages from another snapshot or revision", () => {
    const current = page(9, [group(1)], 1);
    expect(() =>
      appendSnapshotPage(current, page(10, [group(2)], 2), (item) => item.id),
    ).toThrow(
      "snapshot-mismatch",
    );
    expect(() =>
      appendSnapshotPage(
        current,
        { ...page(9, [group(2)], 2), revision: 5 },
        (item) => item.id,
      ),
    ).toThrow("snapshot-revision-mismatch");
  });
});

describe("problem facts and labels", () => {
  it("has an exhaustive reader-facing label for every fact code", () => {
    const seen = new Set<string>();
    for (const code of PROBLEM_FACT_CODES) {
      const label = problemFactLabel(code);
      expect(label.trim()).not.toBe("");
      seen.add(label);
    }
    expect(seen.size).toBe(PROBLEM_FACT_CODES.length);
  });

  it("keeps labels factual rather than causal", () => {
    const prohibited = ["根因", "导致", "内存泄漏", "代码缺陷"];
    for (const code of PROBLEM_FACT_CODES) {
      const label = problemFactLabel(code);
      for (const phrase of prohibited) expect(label).not.toContain(phrase);
    }
  });

  it("covers every kind with a short category label", () => {
    expect(problemKindLabel("java-crash")).toBe("Java/Kotlin 崩溃");
    expect(problemKindLabel("kernel-oom-kill")).toBe("Kernel OOM Kill");
  });

  it("keeps the fact-code list assignable to the public union", () => {
    const values: readonly ProblemFactCode[] = PROBLEM_FACT_CODES;
    expect(values).toHaveLength(22);
  });
});
