import { describe, expect, it } from "vitest";
import { sameAnalysisToken } from "@/lib/analysisToken";

describe("sameAnalysisToken", () => {
  it("matches only complete tokens with both generations equal", () => {
    const token = { sessionGeneration: 7, analysisGeneration: 11 };

    expect(sameAnalysisToken(token, { ...token })).toBe(true);
    expect(
      sameAnalysisToken(token, {
        sessionGeneration: token.sessionGeneration + 1,
        analysisGeneration: token.analysisGeneration,
      }),
    ).toBe(false);
    expect(
      sameAnalysisToken(token, {
        sessionGeneration: token.sessionGeneration,
        analysisGeneration: token.analysisGeneration + 1,
      }),
    ).toBe(false);
    expect(sameAnalysisToken(null, token)).toBe(false);
    expect(sameAnalysisToken(token, undefined)).toBe(false);
  });
});
