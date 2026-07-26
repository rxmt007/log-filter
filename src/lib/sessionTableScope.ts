import { sameAnalysisToken } from "@/lib/analysisToken";
import { mapSourceLine } from "@/lib/ipc";
import {
  createTableScopeController,
  type FilterResultWaitRequest,
  type FilterResultWaitResponse,
  type TableScopeController,
  type TableScopeControllerState,
} from "@/lib/tableScopeController";
import { useSession } from "@/store/session";

function controllerState(): TableScopeControllerState {
  const state = useSession.getState();
  return {
    scope: state.tableScope,
    sessionGeneration: state.status.generation,
    analysisToken: {
      sessionGeneration: state.status.generation,
      analysisGeneration: state.status.analysisGeneration,
    },
    stableLines: state.status.stableLines,
    filterInputRevision: state.filterRevision,
    appliedFilterInputRevision: state.appliedFilterInputRevision,
    filterResultRevision: state.filterResultRevision,
    selectedLine: state.selectedLine,
    selectedResultIndex: state.selectedResultIndex,
    viewportLine: state.viewportLine,
    viewportResultIndex: state.viewportResultIndex,
  };
}

function waitForFilterResult(
  request: FilterResultWaitRequest,
): Promise<FilterResultWaitResponse> {
  return new Promise((resolve, reject) => {
    let unsubscribe = () => {};
    const inspect = () => {
      const state = controllerState();
      if (
        !sameAnalysisToken(state.analysisToken, request.expectedAnalysisToken) ||
        state.sessionGeneration !== request.expectedAnalysisToken.sessionGeneration ||
        state.scope.kind !== "results" ||
        state.filterInputRevision !== request.filterInputRevision
      ) {
        unsubscribe();
        reject(new Error("filter-result-wait-cancelled"));
        return;
      }
      if (state.appliedFilterInputRevision !== request.filterInputRevision) return;
      unsubscribe();
      resolve({
        analysisToken: state.analysisToken,
        filterInputRevision: state.appliedFilterInputRevision,
        filterResultRevision: state.filterResultRevision,
      });
    };
    unsubscribe = useSession.subscribe(inspect);
    inspect();
  });
}

export function createSessionTableScopeController(
  reportError?: (error: unknown) => void,
): TableScopeController {
  return createTableScopeController({
    getState: controllerState,
    commit: (update, guard) => useSession.getState().commitTableNavigation(update, guard),
    mapSourceLine,
    waitForFilterResult,
    subscribeStateChanges: (listener) => useSession.subscribe(listener),
    reportError,
  });
}
