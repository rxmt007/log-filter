import { describe, expect, it } from "vitest";
import { splitHighlightTokens } from "@/lib/highlight";

describe("splitHighlightTokens", () => {
  it("splits keyword highlights separately from the active search hit", () => {
    const tokens = splitHighlightTokens("network payment network", {
      highlights: [{ query: "network", color: "yellow", regex: false, caseSensitive: false }],
      search: { query: "payment", regex: false, caseSensitive: true },
    });

    expect(tokens).toEqual([
      { text: "network", kind: "highlight", color: "yellow" },
      { text: " ", kind: "text" },
      { text: "payment", kind: "search" },
      { text: " ", kind: "text" },
      { text: "network", kind: "highlight", color: "yellow" },
    ]);
  });

  it("supports regex highlights without throwing on invalid user patterns", () => {
    expect(
      splitHighlightTokens("Network retry", {
        highlights: [{ query: "[", color: "blue", regex: true, caseSensitive: false }],
        search: { query: "retry", regex: false, caseSensitive: false },
      }),
    ).toEqual([
      { text: "Network ", kind: "text" },
      { text: "retry", kind: "search" },
    ]);
  });

  it("keeps search hits visible when they overlap a broader keyword highlight", () => {
    const tokens = splitHighlightTokens("warning error", {
      highlights: [{ query: "warning error", color: "yellow", regex: false, caseSensitive: true }],
      search: { query: "error", regex: false, caseSensitive: true },
    });

    expect(tokens).toEqual([
      { text: "warning ", kind: "highlight", color: "yellow" },
      { text: "error", kind: "search" },
    ]);
  });
});
