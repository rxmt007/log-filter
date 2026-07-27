import { sameAnalysisToken } from "@/lib/analysisToken";
import type { AnalysisToken, ProblemOccurrenceRef, ScrollRequest, TableScope } from "@/types";

export type MappingBias = "exact" | "nearest";

export interface TableScopeControllerState {
  scope: TableScope;
  sessionGeneration: number;
  analysisToken: AnalysisToken;
  stableLines: number;
  filterInputRevision: number;
  appliedFilterInputRevision: number;
  filterResultRevision: number;
  selectedLine: number | null;
  selectedResultIndex: number | null;
  viewportLine: number;
  viewportResultIndex: number;
}

export interface LineMappingRequest {
  lineNo: number;
  bias: MappingBias;
  expectedAnalysisToken: AnalysisToken;
  expectedFilterResultRevision: number;
  requestNonce: number;
}

export type LineMappingResponse =
  | {
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: { lineNo: number; resultIndex: number } | null;
    }
  | {
      status: "stale-filter-result";
      analysisToken: AnalysisToken;
      actualFilterResultRevision: number;
    };

export interface FilterResultWaitRequest {
  filterInputRevision: number;
  minimumFilterResultRevision: number;
  expectedAnalysisToken: AnalysisToken;
  requestNonce: number;
}

export interface FilterResultWaitResponse {
  analysisToken: AnalysisToken;
  filterInputRevision: number;
  filterResultRevision: number;
}

export interface TableNavigationCommit {
  scope: TableScope;
  selectedLine: number | null;
  selectedResultIndex: number | null;
  viewportLine: number;
  viewportResultIndex: number;
  scrollRequest: ScrollRequest | null;
  tailFollowing: boolean;
}

export interface NavigationCommitGuard {
  expectedScopeKind: TableScope["kind"];
  expectedSessionGeneration: number;
  expectedAnalysisToken: AnalysisToken;
  expectedFilterInputRevision?: number;
  expectedAppliedFilterInputRevision?: number;
  expectedFilterResultRevision?: number;
}

export interface ContextRowsRequest {
  view: "all";
  start: number;
  count: number;
  expectedAnalysisToken: AnalysisToken;
  requestNonce: number;
}

export interface ContextRowsResponse {
  analysisToken: AnalysisToken;
  requestNonce: number;
  rows: readonly unknown[];
}

export interface TableScopeControllerDependencies {
  getState(): TableScopeControllerState;
  /**
   * Compare `guard` and apply the entire update in one synchronous state transaction.
   * Return false without mutating state when the guarded dataset is no longer current.
   */
  commit(update: TableNavigationCommit, guard: NavigationCommitGuard): boolean;
  mapSourceLine(request: LineMappingRequest): Promise<LineMappingResponse>;
  waitForFilterResult(request: FilterResultWaitRequest): Promise<FilterResultWaitResponse>;
  loadContextRows?(request: ContextRowsRequest): Promise<ContextRowsResponse>;
  acceptContextRows?(response: ContextRowsResponse): void;
  subscribeStateChanges(listener: () => void): () => void;
  reportError?(error: unknown): void;
}

export type NavigationReason = ScrollRequest["reason"] | "problem-anchor" | "return-viewport";

export interface TableScopeController {
  navigateToSourceLine(lineNo: number, reason: NavigationReason): Promise<void>;
  locateProblem(
    occurrence: ProblemOccurrenceRef,
  ): Promise<"located" | "context-opened" | "cancelled">;
  openProblemContext(occurrence: ProblemOccurrenceRef, radius?: number): void;
  returnToResults(): Promise<void>;
}

const DEFAULT_CONTEXT_RADIUS = 50;
export const MAX_CONTEXT_ROWS = 512;

function clamp(value: number, minimum: number, maximum: number) {
  return Math.min(maximum, Math.max(minimum, value));
}

function contextRangeFor(occurrence: ProblemOccurrenceRef, radius: number, stableLines: number) {
  const maximumLine = Math.max(1, Math.floor(stableLines));
  const eventStart = clamp(Math.floor(occurrence.startLine), 1, maximumLine);
  const eventEnd = clamp(Math.max(eventStart, Math.floor(occurrence.endLine)), 1, maximumLine);
  const anchor = clamp(Math.floor(occurrence.anchorLine), eventStart, eventEnd);
  const safeRadius = Math.max(0, Math.floor(Number.isFinite(radius) ? radius : 0));
  const desiredStart = Math.max(1, eventStart - safeRadius);
  const desiredEnd = Math.min(maximumLine, eventEnd + safeRadius);
  if (desiredEnd - desiredStart + 1 <= MAX_CONTEXT_ROWS) {
    return {
      anchor,
      eventRange: { startLine: eventStart, endLine: eventEnd },
      contextRange: { startLine: desiredStart, endLine: desiredEnd },
    };
  }

  const lastStart = desiredEnd - MAX_CONTEXT_ROWS + 1;
  const eventLength = eventEnd - eventStart + 1;
  const minimumStart =
    eventLength <= MAX_CONTEXT_ROWS
      ? Math.max(desiredStart, eventEnd - MAX_CONTEXT_ROWS + 1)
      : desiredStart;
  const maximumStart =
    eventLength <= MAX_CONTEXT_ROWS ? Math.min(eventStart, lastStart) : lastStart;
  const centeredStart = anchor - Math.floor(MAX_CONTEXT_ROWS / 2);
  const contextStart = clamp(centeredStart, minimumStart, maximumStart);
  return {
    anchor,
    eventRange: { startLine: eventStart, endLine: eventEnd },
    contextRange: {
      startLine: contextStart,
      endLine: contextStart + MAX_CONTEXT_ROWS - 1,
    },
  };
}

export function createTableScopeController(
  dependencies: TableScopeControllerDependencies,
): TableScopeController {
  interface ReturnMetadata {
    sessionGeneration: number;
    analysisToken: AnalysisToken;
    filterInputRevision: number;
    filterResultRevision: number;
    viewportLine: number;
    viewportResultIndex: number;
    selectedLine: number | null;
    selectedResultIndex: number | null;
  }

  interface ResultDatasetIdentity {
    sessionGeneration: number;
    analysisToken: AnalysisToken;
    filterInputRevision: number;
    appliedFilterInputRevision: number;
    filterResultRevision: number;
  }

  type DatasetOutcome =
    | { status: "ready"; state: TableScopeControllerState }
    | { status: "aborted" }
    | { status: "failed" };

  type MappingOutcome =
    | {
        status: "ok";
        targets: Array<{ lineNo: number; resultIndex: number } | null>;
        dataset: ResultDatasetIdentity;
      }
    | { status: "aborted" }
    | { status: "failed" };

  let operationNonce = 0;
  let requestNonce = 0;
  let scrollNonce = 0;
  let returnMetadata: ReturnMetadata | null = null;
  const wakePendingWaits = new Set<() => void>();

  const failedProtocolOutcome = (message: string): { status: "failed" } => {
    dependencies.reportError?.(new Error(message));
    return { status: "failed" };
  };

  const clearReturnMetadata = (metadata: ReturnMetadata) => {
    if (returnMetadata === metadata) returnMetadata = null;
  };

  const beginOperation = () => {
    operationNonce += 1;
    for (const wake of wakePendingWaits) wake();
    return operationNonce;
  };

  const operationIsCurrent = (operation: number, token: AnalysisToken) => {
    const current = dependencies.getState();
    return (
      operation === operationNonce &&
      current.sessionGeneration === token.sessionGeneration &&
      sameAnalysisToken(current.analysisToken, token)
    );
  };

  const datasetIdentity = (state: TableScopeControllerState): ResultDatasetIdentity => ({
    sessionGeneration: state.sessionGeneration,
    analysisToken: state.analysisToken,
    filterInputRevision: state.filterInputRevision,
    appliedFilterInputRevision: state.appliedFilterInputRevision,
    filterResultRevision: state.filterResultRevision,
  });

  const datasetIsCurrent = (operation: number, dataset: ResultDatasetIdentity) => {
    if (!operationIsCurrent(operation, dataset.analysisToken)) return false;
    const current = dependencies.getState();
    return (
      current.scope.kind === "results" &&
      current.sessionGeneration === dataset.sessionGeneration &&
      sameAnalysisToken(current.analysisToken, dataset.analysisToken) &&
      current.filterInputRevision === dataset.filterInputRevision &&
      current.appliedFilterInputRevision === dataset.appliedFilterInputRevision &&
      current.filterResultRevision === dataset.filterResultRevision
    );
  };

  const guardForState = (
    state: TableScopeControllerState,
    includeFilterDataset = true,
  ): NavigationCommitGuard => ({
    expectedScopeKind: state.scope.kind,
    expectedSessionGeneration: state.sessionGeneration,
    expectedAnalysisToken: state.analysisToken,
    ...(includeFilterDataset
      ? {
          expectedFilterInputRevision: state.filterInputRevision,
          expectedAppliedFilterInputRevision: state.appliedFilterInputRevision,
          expectedFilterResultRevision: state.filterResultRevision,
        }
      : {}),
  });

  const guardForDataset = (dataset: ResultDatasetIdentity): NavigationCommitGuard => ({
    expectedScopeKind: "results",
    expectedSessionGeneration: dataset.sessionGeneration,
    expectedAnalysisToken: dataset.analysisToken,
    expectedFilterInputRevision: dataset.filterInputRevision,
    expectedAppliedFilterInputRevision: dataset.appliedFilterInputRevision,
    expectedFilterResultRevision: dataset.filterResultRevision,
  });

  const stateMatchesGuard = (state: TableScopeControllerState, guard: NavigationCommitGuard) =>
    state.scope.kind === guard.expectedScopeKind &&
    state.sessionGeneration === guard.expectedSessionGeneration &&
    sameAnalysisToken(state.analysisToken, guard.expectedAnalysisToken) &&
    (guard.expectedFilterInputRevision == null ||
      state.filterInputRevision === guard.expectedFilterInputRevision) &&
    (guard.expectedAppliedFilterInputRevision == null ||
      state.appliedFilterInputRevision === guard.expectedAppliedFilterInputRevision) &&
    (guard.expectedFilterResultRevision == null ||
      state.filterResultRevision === guard.expectedFilterResultRevision);

  const guardedCommit = (update: TableNavigationCommit, guard: NavigationCommitGuard) => {
    if (!stateMatchesGuard(dependencies.getState(), guard)) return false;
    return dependencies.commit(update, guard);
  };

  const currentResultDataset = async (
    operation: number,
    token: AnalysisToken,
    forceWait: boolean,
    minimumFilterResultRevision: number,
  ): Promise<DatasetOutcome> => {
    let mustWait = forceWait;
    while (operationIsCurrent(operation, token)) {
      const state = dependencies.getState();
      if (state.scope.kind !== "results") return { status: "aborted" };
      if (!mustWait && state.appliedFilterInputRevision === state.filterInputRevision) {
        return { status: "ready", state };
      }
      mustWait = false;
      const expectedInputRevision = state.filterInputRevision;
      const minimumResultRevision = Math.max(
        state.filterResultRevision,
        minimumFilterResultRevision,
      );
      const nonce = ++requestNonce;
      let wakeWait: (() => void) | undefined;
      let unsubscribe: (() => void) | undefined;
      const stateChanged = new Promise<{ kind: "changed" }>((resolve) => {
        wakeWait = () => resolve({ kind: "changed" });
        wakePendingWaits.add(wakeWait);
        unsubscribe = dependencies.subscribeStateChanges(() => {
          const current = dependencies.getState();
          if (
            current.scope.kind !== "results" ||
            current.filterInputRevision !== expectedInputRevision ||
            !sameAnalysisToken(current.analysisToken, token) ||
            current.sessionGeneration !== token.sessionGeneration
          ) {
            resolve({ kind: "changed" });
          }
        });
      });
      const waited = dependencies
        .waitForFilterResult({
          filterInputRevision: expectedInputRevision,
          minimumFilterResultRevision: minimumResultRevision,
          expectedAnalysisToken: token,
          requestNonce: nonce,
        })
        .then(
          (response) => ({ kind: "result" as const, response }),
          (error) => ({ kind: "error" as const, error }),
        );
      const outcome = await Promise.race([waited, stateChanged]);
      if (wakeWait) wakePendingWaits.delete(wakeWait);
      unsubscribe?.();
      if (outcome.kind === "changed") {
        if (!operationIsCurrent(operation, token)) return { status: "aborted" };
        if (dependencies.getState().scope.kind !== "results") return { status: "aborted" };
        continue;
      }
      if (outcome.kind === "error") {
        dependencies.reportError?.(outcome.error);
        return { status: "failed" };
      }
      const response = outcome.response;
      if (!operationIsCurrent(operation, token) || nonce !== requestNonce) {
        return { status: "aborted" };
      }
      if (!sameAnalysisToken(response.analysisToken, token)) {
        return failedProtocolOutcome("Filter result wait returned a mismatched analysis token");
      }
      const current = dependencies.getState();
      if (current.scope.kind !== "results") return { status: "aborted" };
      if (current.filterInputRevision !== expectedInputRevision) {
        continue;
      }
      if (response.filterInputRevision !== expectedInputRevision) {
        return failedProtocolOutcome("Filter result wait returned a mismatched input revision");
      }
      if (
        current.appliedFilterInputRevision === response.filterInputRevision &&
        current.filterResultRevision === response.filterResultRevision &&
        response.filterResultRevision >= minimumResultRevision
      ) {
        return { status: "ready", state: current };
      }
      mustWait = true;
    }
    return { status: "aborted" };
  };

  const mapLatestResults = async (
    operation: number,
    token: AnalysisToken,
    requests: ReadonlyArray<{ lineNo: number; bias: MappingBias }>,
  ): Promise<MappingOutcome> => {
    let forceWait = false;
    let minimumFilterResultRevision = 0;
    while (operationIsCurrent(operation, token)) {
      const datasetOutcome = await currentResultDataset(
        operation,
        token,
        forceWait,
        minimumFilterResultRevision,
      );
      if (datasetOutcome.status !== "ready") return datasetOutcome;
      const dataset = datasetOutcome.state;
      const identity = datasetIdentity(dataset);
      forceWait = false;
      minimumFilterResultRevision = 0;
      const nonce = ++requestNonce;
      let wakeMapping: (() => void) | undefined;
      let unsubscribe: (() => void) | undefined;
      const stateChanged = new Promise<{ kind: "changed" }>((resolve) => {
        wakeMapping = () => resolve({ kind: "changed" });
        wakePendingWaits.add(wakeMapping);
        unsubscribe = dependencies.subscribeStateChanges(() => {
          const current = dependencies.getState();
          if (
            current.scope.kind !== "results" ||
            current.filterInputRevision !== dataset.filterInputRevision ||
            current.appliedFilterInputRevision !== dataset.appliedFilterInputRevision ||
            current.filterResultRevision !== dataset.filterResultRevision ||
            current.sessionGeneration !== token.sessionGeneration ||
            !sameAnalysisToken(current.analysisToken, token)
          ) {
            resolve({ kind: "changed" });
          }
        });
      });
      const mapped = Promise.all(
        requests.map((request) =>
          dependencies.mapSourceLine({
            ...request,
            expectedAnalysisToken: token,
            expectedFilterResultRevision: dataset.filterResultRevision,
            requestNonce: nonce,
          }),
        ),
      ).then(
        (responses) => ({ kind: "result" as const, responses }),
        (error) => ({ kind: "error" as const, error }),
      );
      const outcome = await Promise.race([mapped, stateChanged]);
      if (wakeMapping) wakePendingWaits.delete(wakeMapping);
      unsubscribe?.();
      if (outcome.kind === "changed") {
        if (!operationIsCurrent(operation, token)) return { status: "aborted" };
        if (dependencies.getState().scope.kind !== "results") return { status: "aborted" };
        continue;
      }
      if (outcome.kind === "error") {
        dependencies.reportError?.(outcome.error);
        return { status: "failed" };
      }
      const responses = outcome.responses;
      if (!operationIsCurrent(operation, token) || nonce !== requestNonce) {
        return { status: "aborted" };
      }
      const current = dependencies.getState();
      if (
        current.scope.kind !== "results" ||
        current.filterInputRevision !== dataset.filterInputRevision ||
        current.appliedFilterInputRevision !== dataset.appliedFilterInputRevision ||
        current.filterResultRevision !== dataset.filterResultRevision
      ) {
        continue;
      }
      if (responses.some((response) => !sameAnalysisToken(response.analysisToken, token))) {
        return failedProtocolOutcome("Line mapping returned a mismatched analysis token");
      }
      if (responses.some((response) => response.status === "stale-filter-result")) {
        forceWait = true;
        minimumFilterResultRevision = Math.max(
          ...responses
            .filter(
              (
                response,
              ): response is Extract<LineMappingResponse, { status: "stale-filter-result" }> =>
                response.status === "stale-filter-result",
            )
            .map((response) => response.actualFilterResultRevision),
        );
        continue;
      }
      const successful = responses.filter(
        (response): response is Extract<LineMappingResponse, { status: "ok" }> =>
          response.status === "ok",
      );
      if (
        successful.length !== responses.length ||
        successful.some(
          (response) => response.filterResultRevision !== dataset.filterResultRevision,
        )
      ) {
        return failedProtocolOutcome("Line mapping returned a mismatched dataset identity");
      }
      if (
        successful.some((response, index) => {
          const target = response.target;
          return (
            requests[index].bias === "exact" &&
            target != null &&
            target.lineNo !== requests[index].lineNo
          );
        })
      ) {
        return failedProtocolOutcome("Exact line mapping returned a different source line");
      }
      return {
        status: "ok",
        dataset: identity,
        targets: successful.map((response) => response.target),
      };
    }
    return { status: "aborted" };
  };

  const commitLocated = (
    operation: number,
    dataset: ResultDatasetIdentity,
    lineNo: number,
    resultIndex: number,
    reason: ScrollRequest["reason"] = "jump",
    align: ScrollRequest["align"] = "center",
  ) => {
    if (!datasetIsCurrent(operation, dataset)) return false;
    scrollNonce += 1;
    return guardedCommit(
      {
        scope: { kind: "results", view: "filtered" },
        selectedLine: lineNo,
        selectedResultIndex: resultIndex,
        viewportLine: lineNo,
        viewportResultIndex: resultIndex,
        scrollRequest: {
          index: resultIndex,
          align,
          reason,
          nonce: scrollNonce,
        },
        tailFollowing: reason === "tail",
      },
      guardForDataset(dataset),
    );
  };

  const commitResultRestore = (
    viewportLine: number,
    viewportResultIndex: number,
    selectedLine: number | null,
    selectedResultIndex: number | null,
    guard: NavigationCommitGuard,
  ) => {
    scrollNonce += 1;
    return guardedCommit(
      {
        scope: { kind: "results", view: "filtered" },
        selectedLine,
        selectedResultIndex,
        viewportLine,
        viewportResultIndex,
        scrollRequest: {
          index: viewportResultIndex,
          align: "start",
          reason: "jump",
          nonce: scrollNonce,
        },
        tailFollowing: false,
      },
      guard,
    );
  };

  const commitSafeResults = (guard: NavigationCommitGuard) =>
    guardedCommit(
      {
        scope: { kind: "results", view: "filtered" },
        selectedLine: null,
        selectedResultIndex: null,
        viewportLine: 1,
        viewportResultIndex: 0,
        scrollRequest: null,
        tailFollowing: false,
      },
      guard,
    );

  const commitSafeTailResults = (operation: number, token: AnalysisToken) => {
    while (operationIsCurrent(operation, token)) {
      const current = dependencies.getState();
      if (current.scope.kind !== "results") return false;
      const committed = guardedCommit(
        {
          scope: { kind: "results", view: "filtered" },
          selectedLine: null,
          selectedResultIndex: null,
          viewportLine: 1,
          viewportResultIndex: 0,
          scrollRequest: null,
          tailFollowing: true,
        },
        guardForState(current),
      );
      if (committed) return true;
    }
    return false;
  };

  const openContext = (occurrence: ProblemOccurrenceRef, radius: number) => {
    const operation = beginOperation();
    const state = dependencies.getState();
    const { anchor, eventRange, contextRange } = contextRangeFor(
      occurrence,
      radius,
      state.stableLines,
    );
    const returnPoint =
      state.scope.kind === "problem-context"
        ? state.scope.returnPoint
        : {
            viewportLine: state.viewportLine,
            selectedLine: state.selectedLine,
            filterInputRevision: state.filterInputRevision,
          };
    const pendingReturnMetadata =
      returnMetadata &&
      returnMetadata.sessionGeneration === state.sessionGeneration &&
      sameAnalysisToken(returnMetadata.analysisToken, state.analysisToken)
        ? returnMetadata
        : null;
    const nextReturnMetadata =
      state.scope.kind === "results"
        ? (pendingReturnMetadata ?? {
            sessionGeneration: state.sessionGeneration,
            analysisToken: state.analysisToken,
            filterInputRevision: state.filterInputRevision,
            filterResultRevision: state.filterResultRevision,
            viewportLine: state.viewportLine,
            viewportResultIndex: state.viewportResultIndex,
            selectedLine: state.selectedLine,
            selectedResultIndex: state.selectedResultIndex,
          })
        : returnMetadata;
    const scope: TableScope = {
      kind: "problem-context",
      occurrence,
      eventRange,
      contextRange,
      returnPoint,
    };
    scrollNonce += 1;
    const committed = guardedCommit(
      {
        scope,
        selectedLine: anchor,
        selectedResultIndex: anchor - 1,
        viewportLine: anchor,
        viewportResultIndex: anchor - 1,
        scrollRequest: {
          index: anchor - 1,
          align: "center",
          reason: "jump",
          nonce: scrollNonce,
        },
        tailFollowing: false,
      },
      guardForState(state),
    );
    if (!committed) return false;
    returnMetadata = nextReturnMetadata;

    if (!dependencies.loadContextRows || state.stableLines <= 0) return true;
    const nonce = ++requestNonce;
    const expectedToken = state.analysisToken;
    const count = Math.min(MAX_CONTEXT_ROWS, contextRange.endLine - contextRange.startLine + 1);
    void dependencies
      .loadContextRows({
        view: "all",
        start: contextRange.startLine - 1,
        count,
        expectedAnalysisToken: expectedToken,
        requestNonce: nonce,
      })
      .then((response) => {
        const current = dependencies.getState();
        if (
          operationIsCurrent(operation, expectedToken) &&
          nonce === requestNonce &&
          response.requestNonce === nonce &&
          sameAnalysisToken(response.analysisToken, expectedToken) &&
          current.scope.kind === "problem-context" &&
          current.scope.occurrence.eventId === occurrence.eventId
        ) {
          dependencies.acceptContextRows?.(response);
        }
      })
      .catch((error) => {
        if (operationIsCurrent(operation, expectedToken) && nonce === requestNonce) {
          dependencies.reportError?.(error);
        }
      });
    return true;
  };

  const recoverFailedReturn = (operation: number, metadata: ReturnMetadata) => {
    while (returnMetadata === metadata && operationIsCurrent(operation, metadata.analysisToken)) {
      const current = dependencies.getState();
      if (current.scope.kind !== "results") return;
      const guard = guardForState(current);
      const committed =
        current.filterResultRevision === metadata.filterResultRevision
          ? commitResultRestore(
              metadata.viewportLine,
              metadata.viewportResultIndex,
              metadata.selectedLine,
              metadata.selectedResultIndex,
              guard,
            )
          : commitSafeResults(guard);
      if (committed) {
        clearReturnMetadata(metadata);
        return;
      }
    }
  };

  return {
    async navigateToSourceLine(lineNo, reason) {
      const operation = beginOperation();
      const state = dependencies.getState();
      if (state.scope.kind === "problem-context" && reason !== "tail") {
        const safeLine = clamp(Math.floor(lineNo), 1, Math.max(1, state.stableLines));
        scrollNonce += 1;
        guardedCommit(
          {
            scope: state.scope,
            selectedLine: safeLine,
            selectedResultIndex: safeLine - 1,
            viewportLine: safeLine,
            viewportResultIndex: safeLine - 1,
            scrollRequest: {
              index: safeLine - 1,
              align: "center",
              reason: reason === "problem-anchor" || reason === "return-viewport" ? "jump" : reason,
              nonce: scrollNonce,
            },
            tailFollowing: false,
          },
          guardForState(state),
        );
        return;
      }
      if (state.scope.kind === "problem-context") {
        const enteredResults = guardedCommit(
          {
            scope: { kind: "results", view: "filtered" },
            selectedLine: null,
            selectedResultIndex: null,
            viewportLine: clamp(Math.floor(lineNo), 1, Math.max(1, state.stableLines)),
            viewportResultIndex: 0,
            scrollRequest: null,
            tailFollowing: false,
          },
          guardForState(state),
        );
        if (!enteredResults) return;
        returnMetadata = null;
      } else {
        returnMetadata = null;
      }
      while (operationIsCurrent(operation, state.analysisToken)) {
        const outcome = await mapLatestResults(operation, state.analysisToken, [
          {
            lineNo,
            bias: reason === "return-viewport" || reason === "tail" ? "nearest" : "exact",
          },
        ]);
        if (outcome.status === "aborted") return;
        if (outcome.status === "failed") {
          if (reason === "tail") commitSafeTailResults(operation, state.analysisToken);
          return;
        }
        const target = outcome.targets[0];
        if (!target) {
          if (reason !== "tail") return;
          if (!datasetIsCurrent(operation, outcome.dataset)) continue;
          const committed = guardedCommit(
            {
              scope: { kind: "results", view: "filtered" },
              selectedLine: null,
              selectedResultIndex: null,
              viewportLine: 1,
              viewportResultIndex: 0,
              scrollRequest: null,
              tailFollowing: true,
            },
            guardForDataset(outcome.dataset),
          );
          if (committed) return;
          continue;
        }
        const scrollReason =
          reason === "problem-anchor" || reason === "return-viewport" ? "jump" : reason;
        if (
          commitLocated(
            operation,
            outcome.dataset,
            target.lineNo,
            target.resultIndex,
            scrollReason,
            reason === "tail" ? "end" : "center",
          )
        ) {
          return;
        }
        if (dependencies.getState().scope.kind !== "results") return;
      }
    },

    async locateProblem(problem) {
      const state = dependencies.getState();
      if (state.scope.kind === "problem-context") {
        return openContext(problem, DEFAULT_CONTEXT_RADIUS) ? "context-opened" : "cancelled";
      }
      const operation = beginOperation();
      while (operationIsCurrent(operation, state.analysisToken)) {
        const outcome = await mapLatestResults(operation, state.analysisToken, [
          { lineNo: problem.anchorLine, bias: "exact" },
        ]);
        if (outcome.status !== "ok") return "cancelled";
        const target = outcome.targets[0];
        if (target) {
          if (commitLocated(operation, outcome.dataset, problem.anchorLine, target.resultIndex)) {
            return "located";
          }
          if (dependencies.getState().scope.kind !== "results") return "cancelled";
          continue;
        }
        if (!datasetIsCurrent(operation, outcome.dataset)) continue;
        return openContext(problem, DEFAULT_CONTEXT_RADIUS) ? "context-opened" : "cancelled";
      }
      return "cancelled";
    },

    openProblemContext(problem, radius = DEFAULT_CONTEXT_RADIUS) {
      openContext(problem, radius);
    },

    async returnToResults() {
      const operation = beginOperation();
      const state = dependencies.getState();
      if (state.scope.kind !== "problem-context") return;
      const metadata = returnMetadata;
      if (
        !metadata ||
        metadata.sessionGeneration !== state.sessionGeneration ||
        !sameAnalysisToken(metadata.analysisToken, state.analysisToken)
      ) {
        returnMetadata = null;
        commitSafeResults(guardForState(state));
        return;
      }

      if (
        metadata.filterInputRevision === state.filterInputRevision &&
        metadata.filterResultRevision === state.filterResultRevision &&
        state.appliedFilterInputRevision === state.filterInputRevision
      ) {
        const committed = commitResultRestore(
          metadata.viewportLine,
          metadata.viewportResultIndex,
          metadata.selectedLine,
          metadata.selectedResultIndex,
          guardForState(state),
        );
        if (committed) clearReturnMetadata(metadata);
        return;
      }

      const enteredResults = guardedCommit(
        {
          scope: { kind: "results", view: "filtered" },
          selectedLine: metadata.selectedLine,
          selectedResultIndex: null,
          viewportLine: metadata.viewportLine,
          viewportResultIndex: 0,
          scrollRequest: null,
          tailFollowing: false,
        },
        guardForState(state),
      );
      if (!enteredResults) return;
      const requests: Array<{ lineNo: number; bias: MappingBias }> = [
        { lineNo: metadata.viewportLine, bias: "nearest" },
      ];
      if (metadata.selectedLine != null) {
        requests.push({ lineNo: metadata.selectedLine, bias: "exact" });
      }
      while (returnMetadata === metadata && operationIsCurrent(operation, metadata.analysisToken)) {
        const outcome = await mapLatestResults(operation, metadata.analysisToken, requests);
        if (outcome.status === "aborted") return;
        if (outcome.status === "failed") {
          recoverFailedReturn(operation, metadata);
          return;
        }
        if (!datasetIsCurrent(operation, outcome.dataset)) continue;
        const viewport = outcome.targets[0];
        const committed = viewport
          ? commitResultRestore(
              viewport.lineNo,
              viewport.resultIndex,
              (metadata.selectedLine == null ? null : outcome.targets[1])?.lineNo ?? null,
              (metadata.selectedLine == null ? null : outcome.targets[1])?.resultIndex ?? null,
              guardForDataset(outcome.dataset),
            )
          : commitSafeResults(guardForDataset(outcome.dataset));
        if (committed) {
          clearReturnMetadata(metadata);
          return;
        }
      }
    },
  };
}
