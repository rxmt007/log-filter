import type { AnalysisToken } from "@/types";

export function sameAnalysisToken(
  left: AnalysisToken | null | undefined,
  right: AnalysisToken | null | undefined,
): boolean {
  return (
    left != null &&
    right != null &&
    left.sessionGeneration === right.sessionGeneration &&
    left.analysisGeneration === right.analysisGeneration
  );
}
