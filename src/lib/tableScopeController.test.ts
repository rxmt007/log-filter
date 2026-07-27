import { describe, expect, it, vi } from "vitest";
import {
  createTableScopeController,
  type NavigationCommitGuard,
  type TableNavigationCommit,
  type TableScopeControllerState,
} from "@/lib/tableScopeController";
import type { AnalysisToken, ProblemOccurrenceRef } from "@/types";

const token: AnalysisToken = {
  sessionGeneration: 7,
  analysisGeneration: 3,
};

const occurrence: ProblemOccurrenceRef = {
  eventId: 44,
  groupId: 8,
  startLine: 480,
  endLine: 510,
  anchorLine: 490,
};

function resultsState(): TableScopeControllerState {
  return {
    scope: { kind: "results", view: "filtered" },
    sessionGeneration: 7,
    analysisToken: token,
    stableLines: 1_000,
    filterInputRevision: 4,
    appliedFilterInputRevision: 4,
    filterResultRevision: 9,
    selectedLine: 125,
    selectedResultIndex: 12,
    viewportLine: 120,
    viewportResultIndex: 10,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

const noStateSubscription = () => () => {};

describe("table scope controller", () => {
  it("keeps Results scope and centers a visible problem anchor", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const guards: NavigationCommitGuard[] = [];
    const mapSourceLine = vi.fn().mockResolvedValue({
      status: "ok",
      analysisToken: token,
      filterResultRevision: 9,
      target: { lineNo: 490, resultIndex: 21 },
    });
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit, guard) => {
        commits.push(commit);
        guards.push(guard);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });

    await expect(controller.locateProblem(occurrence)).resolves.toBe("located");

    expect(mapSourceLine).toHaveBeenCalledWith({
      lineNo: 490,
      bias: "exact",
      expectedAnalysisToken: token,
      expectedFilterResultRevision: 9,
      requestNonce: 1,
    });
    expect(commits).toEqual([
      expect.objectContaining({
        scope: { kind: "results", view: "filtered" },
        selectedLine: 490,
        selectedResultIndex: 21,
        viewportLine: 490,
        viewportResultIndex: 21,
        tailFollowing: false,
        scrollRequest: expect.objectContaining({
          index: 21,
          align: "center",
          reason: "jump",
        }),
      }),
    ]);
    expect(guards).toEqual([
      {
        expectedScopeKind: "results",
        expectedSessionGeneration: 7,
        expectedAnalysisToken: token,
        expectedFilterInputRevision: 4,
        expectedAppliedFilterInputRevision: 4,
        expectedFilterResultRevision: 9,
      },
    ]);
  });

  it("opens bounded all-row context when the anchor is filtered out", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const loadContextRows = vi.fn().mockResolvedValue({
      analysisToken: token,
      requestNonce: 2,
      rows: [],
    });
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn().mockResolvedValue({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 9,
        target: null,
      }),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
      loadContextRows,
      acceptContextRows: vi.fn(),
    });

    await expect(controller.locateProblem(occurrence)).resolves.toBe("context-opened");

    expect(commits).toHaveLength(1);
    expect(commits[0]).toMatchObject({
      scope: {
        kind: "problem-context",
        occurrence,
        eventRange: { startLine: 480, endLine: 510 },
        contextRange: { startLine: 430, endLine: 560 },
        returnPoint: {
          viewportLine: 120,
          selectedLine: 125,
          filterInputRevision: 4,
        },
      },
      selectedLine: 490,
      selectedResultIndex: 489,
      viewportLine: 490,
      viewportResultIndex: 489,
      tailFollowing: false,
    });
    expect(loadContextRows).toHaveBeenCalledWith({
      view: "all",
      start: 429,
      count: 131,
      expectedAnalysisToken: token,
      requestNonce: 2,
    });
    expect(loadContextRows.mock.calls[0][0].count).toBeLessThanOrEqual(512);
  });

  it("explicit context always opens and switching occurrences preserves the first return point", () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const mapSourceLine = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });

    controller.openProblemContext(occurrence);
    state = { ...state, viewportLine: 492, selectedLine: 492 };
    controller.openProblemContext({
      eventId: 45,
      groupId: 8,
      startLine: 600,
      endLine: 605,
      anchorLine: 602,
    });

    expect(mapSourceLine).not.toHaveBeenCalled();
    expect(commits[1].scope).toMatchObject({
      kind: "problem-context",
      occurrence: { eventId: 45 },
      returnPoint: {
        viewportLine: 120,
        selectedLine: 125,
        filterInputRevision: 4,
      },
    });
  });

  it("returns directly to the original result indices when the filter dataset is unchanged", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const mapSourceLine = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });
    controller.openProblemContext(occurrence);

    await controller.returnToResults();

    expect(mapSourceLine).not.toHaveBeenCalled();
    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      selectedLine: 125,
      selectedResultIndex: 12,
      viewportLine: 120,
      viewportResultIndex: 10,
      scrollRequest: {
        index: 10,
        align: "start",
        reason: "jump",
      },
      tailFollowing: false,
    });
  });

  it("drops R1 mappings, waits for R2, then restores viewport with Nearest and selection with Exact", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const listeners = new Set<() => void>();
    const r1Viewport = deferred<{
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: { lineNo: number; resultIndex: number };
    }>();
    const r1Selection = deferred<{
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: { lineNo: number; resultIndex: number };
    }>();
    const r2Viewport = deferred<{
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: { lineNo: number; resultIndex: number };
    }>();
    const r2Selection = deferred<{
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: null;
    }>();
    const mappingResponses = [
      r1Viewport.promise,
      r1Selection.promise,
      r2Viewport.promise,
      r2Selection.promise,
    ];
    const mapSourceLine = vi.fn().mockImplementation(() => mappingResponses.shift());
    const r2Applied = deferred<{
      analysisToken: AnalysisToken;
      filterInputRevision: number;
      filterResultRevision: number;
    }>();
    const waitForFilterResult = vi.fn().mockReturnValue(r2Applied.promise);
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult,
      subscribeStateChanges: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    });
    controller.openProblemContext(occurrence);
    state = {
      ...state,
      filterInputRevision: 5,
      appliedFilterInputRevision: 5,
      filterResultRevision: 10,
    };

    const returning = controller.returnToResults();
    await vi.waitFor(() => expect(mapSourceLine).toHaveBeenCalledTimes(2));
    state = {
      ...state,
      filterInputRevision: 6,
      appliedFilterInputRevision: 5,
    };
    for (const listener of listeners) listener();
    await vi.waitFor(() => expect(waitForFilterResult).toHaveBeenCalledTimes(1));
    expect(waitForFilterResult).toHaveBeenCalledWith({
      filterInputRevision: 6,
      minimumFilterResultRevision: 10,
      expectedAnalysisToken: token,
      requestNonce: 2,
    });
    state = {
      ...state,
      appliedFilterInputRevision: 6,
      filterResultRevision: 11,
    };
    r2Applied.resolve({
      analysisToken: token,
      filterInputRevision: 6,
      filterResultRevision: 11,
    });
    await vi.waitFor(() => expect(mapSourceLine).toHaveBeenCalledTimes(4));
    expect(mapSourceLine.mock.calls.slice(2).map(([request]) => request)).toEqual([
      {
        lineNo: 120,
        bias: "nearest",
        expectedAnalysisToken: token,
        expectedFilterResultRevision: 11,
        requestNonce: 3,
      },
      {
        lineNo: 125,
        bias: "exact",
        expectedAnalysisToken: token,
        expectedFilterResultRevision: 11,
        requestNonce: 3,
      },
    ]);
    r2Viewport.resolve({
      status: "ok",
      analysisToken: token,
      filterResultRevision: 11,
      target: { lineNo: 118, resultIndex: 8 },
    });
    r2Selection.resolve({
      status: "ok",
      analysisToken: token,
      filterResultRevision: 11,
      target: null,
    });
    await returning;

    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 118,
      viewportResultIndex: 8,
      scrollRequest: { index: 8, align: "start" },
    });
    expect(commits.some((commit) => commit.viewportResultIndex === 99)).toBe(false);
  });

  it("discards a late mapping after the session and analysis token change", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const mapping = deferred<{
      status: "ok";
      analysisToken: AnalysisToken;
      filterResultRevision: number;
      target: { lineNo: number; resultIndex: number };
    }>();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn().mockReturnValue(mapping.promise),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });

    const locating = controller.locateProblem(occurrence);
    state = {
      ...state,
      sessionGeneration: 8,
      analysisToken: { sessionGeneration: 8, analysisGeneration: 1 },
    };
    mapping.resolve({
      status: "ok",
      analysisToken: token,
      filterResultRevision: 9,
      target: { lineNo: 490, resultIndex: 21 },
    });
    await expect(locating).resolves.toBe("cancelled");

    expect(commits).toEqual([]);
  });

  it("caps context row requests at 512 and drops rows from a superseded occurrence", async () => {
    let state = resultsState();
    const firstRows = deferred<{
      analysisToken: AnalysisToken;
      requestNonce: number;
      rows: unknown[];
    }>();
    const secondRows = deferred<{
      analysisToken: AnalysisToken;
      requestNonce: number;
      rows: unknown[];
    }>();
    const loadContextRows = vi
      .fn()
      .mockReturnValueOnce(firstRows.promise)
      .mockReturnValueOnce(secondRows.promise);
    const acceptContextRows = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn(),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
      loadContextRows,
      acceptContextRows,
    });

    controller.openProblemContext(
      { ...occurrence, startLine: 100, endLine: 900, anchorLine: 490 },
      10_000,
    );
    controller.openProblemContext({ ...occurrence, eventId: 99 });
    expect(loadContextRows.mock.calls[0][0].count).toBe(512);
    firstRows.resolve({
      analysisToken: token,
      requestNonce: 1,
      rows: [{ lineNo: 490 }],
    });
    await Promise.resolve();
    await Promise.resolve();

    expect(acceptContextRows).not.toHaveBeenCalled();
  });

  it("waits for the latest filter result before ordinary Results navigation", async () => {
    let state = {
      ...resultsState(),
      filterInputRevision: 5,
      appliedFilterInputRevision: 4,
    };
    const applied = deferred<{
      analysisToken: AnalysisToken;
      filterInputRevision: number;
      filterResultRevision: number;
    }>();
    const mapSourceLine = vi.fn().mockResolvedValue({
      status: "ok",
      analysisToken: token,
      filterResultRevision: 10,
      target: { lineNo: 700, resultIndex: 31 },
    });
    const commits: TableNavigationCommit[] = [];
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn().mockReturnValue(applied.promise),
      subscribeStateChanges: noStateSubscription,
    });

    const navigating = controller.navigateToSourceLine(700, "search");
    await Promise.resolve();
    expect(mapSourceLine).not.toHaveBeenCalled();
    state = {
      ...state,
      appliedFilterInputRevision: 5,
      filterResultRevision: 10,
    };
    applied.resolve({
      analysisToken: token,
      filterInputRevision: 5,
      filterResultRevision: 10,
    });
    await navigating;

    expect(mapSourceLine).toHaveBeenCalledWith({
      lineNo: 700,
      bias: "exact",
      expectedAnalysisToken: token,
      expectedFilterResultRevision: 10,
      requestNonce: 2,
    });
    expect(commits[commits.length - 1]).toMatchObject({
      selectedLine: 700,
      selectedResultIndex: 31,
      scrollRequest: { index: 31, align: "center", reason: "search" },
    });
  });

  it("navigates directly in all-row context without consuming a filtered mapping", async () => {
    let state = resultsState();
    const mapSourceLine = vi.fn();
    const commits: TableNavigationCommit[] = [];
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });
    controller.openProblemContext(occurrence);

    await controller.navigateToSourceLine(505, "bookmark");

    expect(mapSourceLine).not.toHaveBeenCalled();
    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "problem-context" },
      selectedLine: 505,
      selectedResultIndex: 504,
      viewportLine: 505,
      viewportResultIndex: 504,
      scrollRequest: { index: 504, reason: "bookmark" },
    });
  });

  it("starts waiting for R2 without requiring the superseded R1 wait to finish", async () => {
    let state = resultsState();
    const listeners = new Set<() => void>();
    const r1 = deferred<{
      analysisToken: AnalysisToken;
      filterInputRevision: number;
      filterResultRevision: number;
    }>();
    const r2 = deferred<{
      analysisToken: AnalysisToken;
      filterInputRevision: number;
      filterResultRevision: number;
    }>();
    const waitForFilterResult = vi
      .fn()
      .mockReturnValueOnce(r1.promise)
      .mockReturnValueOnce(r2.promise);
    const mapSourceLine = vi
      .fn()
      .mockResolvedValueOnce({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 11,
        target: { lineNo: 120, resultIndex: 7 },
      })
      .mockResolvedValueOnce({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 11,
        target: { lineNo: 125, resultIndex: 8 },
      });
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult,
      subscribeStateChanges: (listener) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    });
    controller.openProblemContext(occurrence);
    state = {
      ...state,
      filterInputRevision: 5,
      appliedFilterInputRevision: 4,
    };

    const returning = controller.returnToResults();
    await vi.waitFor(() => expect(waitForFilterResult).toHaveBeenCalledTimes(1));
    state = { ...state, filterInputRevision: 6 };
    for (const listener of listeners) listener();
    await vi.waitFor(() => expect(waitForFilterResult).toHaveBeenCalledTimes(2));
    expect(waitForFilterResult.mock.calls[1][0]).toEqual({
      filterInputRevision: 6,
      minimumFilterResultRevision: 9,
      expectedAnalysisToken: token,
      requestNonce: 2,
    });
    state = {
      ...state,
      appliedFilterInputRevision: 6,
      filterResultRevision: 11,
    };
    r2.resolve({
      analysisToken: token,
      filterInputRevision: 6,
      filterResultRevision: 11,
    });
    await returning;

    expect(mapSourceLine).toHaveBeenCalledTimes(2);
    expect(mapSourceLine.mock.calls[0][0].requestNonce).toBe(3);
  });

  it("does not treat an invalid mapping response as a filtered-out anchor", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn().mockResolvedValue({
        status: "ok",
        analysisToken: { sessionGeneration: 7, analysisGeneration: 99 },
        filterResultRevision: 9,
        target: null,
      }),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });

    await expect(controller.locateProblem(occurrence)).resolves.toBe("cancelled");

    expect(commits).toEqual([]);
    expect(state.scope).toEqual({ kind: "results", view: "filtered" });
  });

  it("lets the adapter atomically reject a mapping commit after the dataset changes", async () => {
    let state = resultsState();
    const commit = vi.fn((_update: TableNavigationCommit, guard: NavigationCommitGuard) => {
      expect(guard.expectedFilterResultRevision).toBe(9);
      state = { ...state, filterResultRevision: 10 };
      return false;
    });
    const controller = createTableScopeController({
      getState: () => state,
      commit,
      mapSourceLine: vi.fn().mockResolvedValue({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 9,
        target: { lineNo: 490, resultIndex: 21 },
      }),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });

    await expect(controller.locateProblem(occurrence)).resolves.toBe("cancelled");

    expect(commit).toHaveBeenCalledTimes(1);
    expect(state.scope).toEqual({ kind: "results", view: "filtered" });
  });

  it("preserves a newer context return owner when an older return is superseded", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const pendingViewport = deferred<never>();
    const pendingSelection = deferred<never>();
    const mapSourceLine = vi
      .fn()
      .mockReturnValueOnce(pendingViewport.promise)
      .mockReturnValueOnce(pendingSelection.promise)
      .mockResolvedValueOnce({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 10,
        target: { lineNo: 120, resultIndex: 8 },
      })
      .mockResolvedValueOnce({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 10,
        target: { lineNo: 125, resultIndex: 9 },
      });
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });
    controller.openProblemContext(occurrence);
    state = {
      ...state,
      filterInputRevision: 5,
      appliedFilterInputRevision: 5,
      filterResultRevision: 10,
    };
    const olderReturn = controller.returnToResults();
    await vi.waitFor(() => expect(mapSourceLine).toHaveBeenCalledTimes(2));

    controller.openProblemContext({
      ...occurrence,
      eventId: 99,
      startLine: 600,
      endLine: 605,
      anchorLine: 602,
    });
    await olderReturn;
    await controller.returnToResults();

    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      viewportLine: 120,
      viewportResultIndex: 8,
      selectedLine: 125,
      selectedResultIndex: 9,
    });
  });

  it("remaps against R3 when the adapter rejects an R2 return commit", async () => {
    let state = resultsState();
    let rejectR2Commit = true;
    const commits: TableNavigationCommit[] = [];
    const mapSourceLine = vi
      .fn()
      .mockImplementation(
        ({
          lineNo,
          expectedFilterResultRevision,
        }: {
          lineNo: number;
          expectedFilterResultRevision: number;
        }) =>
          Promise.resolve({
            status: "ok",
            analysisToken: token,
            filterResultRevision: expectedFilterResultRevision,
            target: {
              lineNo,
              resultIndex:
                expectedFilterResultRevision === 10
                  ? lineNo === 120
                    ? 8
                    : 9
                  : lineNo === 120
                    ? 18
                    : 19,
            },
          }),
      );
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        if (rejectR2Commit && commit.scrollRequest?.index === 8) {
          rejectR2Commit = false;
          state = {
            ...state,
            filterInputRevision: 6,
            appliedFilterInputRevision: 6,
            filterResultRevision: 11,
          };
          return false;
        }
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });
    controller.openProblemContext(occurrence);
    state = {
      ...state,
      filterInputRevision: 5,
      appliedFilterInputRevision: 5,
      filterResultRevision: 10,
    };

    await controller.returnToResults();

    expect(
      mapSourceLine.mock.calls.map(([request]) => request.expectedFilterResultRevision),
    ).toEqual([10, 10, 11, 11]);
    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      viewportLine: 120,
      viewportResultIndex: 18,
      selectedLine: 125,
      selectedResultIndex: 19,
    });
  });

  it("treats an Exact response for another source line as invalid", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const reportError = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn().mockResolvedValue({
        status: "ok",
        analysisToken: token,
        filterResultRevision: 9,
        target: { lineNo: 489, resultIndex: 21 },
      }),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
      reportError,
    });

    await expect(controller.locateProblem(occurrence)).resolves.toBe("cancelled");

    expect(commits).toEqual([]);
    expect(reportError).toHaveBeenCalledWith(
      expect.objectContaining({ message: "Exact line mapping returned a different source line" }),
    );
  });

  it("exits context and resumes Results tail-following for an explicit tail navigation", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const mapSourceLine = vi.fn().mockResolvedValue({
      status: "ok",
      analysisToken: token,
      filterResultRevision: 9,
      target: { lineNo: 990, resultIndex: 44 },
    });
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });
    controller.openProblemContext(occurrence);

    await controller.navigateToSourceLine(1_000, "tail");

    expect(mapSourceLine).toHaveBeenCalledWith({
      lineNo: 1_000,
      bias: "nearest",
      expectedAnalysisToken: token,
      expectedFilterResultRevision: 9,
      requestNonce: 1,
    });
    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      selectedLine: 990,
      selectedResultIndex: 44,
      viewportLine: 990,
      viewportResultIndex: 44,
      scrollRequest: { index: 44, align: "end", reason: "tail" },
      tailFollowing: true,
    });
  });

  it("keeps the context return owner when the tail transition is atomically rejected", async () => {
    let state = resultsState();
    let rejectTailTransition = true;
    const commits: TableNavigationCommit[] = [];
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        if (
          rejectTailTransition &&
          state.scope.kind === "problem-context" &&
          commit.scope.kind === "results"
        ) {
          rejectTailTransition = false;
          return false;
        }
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn(),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });
    controller.openProblemContext(occurrence);

    await controller.navigateToSourceLine(1_000, "tail");
    expect(state.scope.kind).toBe("problem-context");
    await controller.returnToResults();

    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      viewportLine: 120,
      viewportResultIndex: 10,
      selectedLine: 125,
      selectedResultIndex: 12,
    });
  });

  it("safe-clears Results and keeps tail-following when tail mapping fails", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const reportError = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn().mockRejectedValue(new Error("tail mapping failed")),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
      reportError,
    });
    controller.openProblemContext(occurrence);

    await controller.navigateToSourceLine(1_000, "tail");

    expect(reportError).toHaveBeenCalledWith(
      expect.objectContaining({ message: "tail mapping failed" }),
    );
    expect(commits[commits.length - 1]).toEqual({
      scope: { kind: "results", view: "filtered" },
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: null,
      tailFollowing: true,
    });
  });

  it("safe-clears return state when a controller is recreated inside context", async () => {
    const source = resultsState();
    let state: TableScopeControllerState = {
      ...source,
      scope: {
        kind: "problem-context",
        occurrence,
        eventRange: { startLine: 480, endLine: 510 },
        contextRange: { startLine: 430, endLine: 560 },
        returnPoint: {
          viewportLine: 120,
          selectedLine: 125,
          filterInputRevision: 4,
        },
      },
      selectedLine: 490,
      selectedResultIndex: 489,
      viewportLine: 490,
      viewportResultIndex: 489,
    };
    const commits: TableNavigationCommit[] = [];
    const mapSourceLine = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine,
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
    });

    await controller.returnToResults();

    expect(mapSourceLine).not.toHaveBeenCalled();
    expect(commits).toEqual([
      {
        scope: { kind: "results", view: "filtered" },
        selectedLine: null,
        selectedResultIndex: null,
        viewportLine: 1,
        viewportResultIndex: 0,
        scrollRequest: null,
        tailFollowing: false,
      },
    ]);
  });

  it("restores the old result indices when filter waiting fails before a new dataset applies", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const reportError = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn(),
      waitForFilterResult: vi.fn().mockRejectedValue(new Error("filter failed")),
      subscribeStateChanges: noStateSubscription,
      reportError,
    });
    controller.openProblemContext(occurrence);
    state = {
      ...state,
      filterInputRevision: 5,
      appliedFilterInputRevision: 4,
    };

    await controller.returnToResults();

    expect(reportError).toHaveBeenCalledWith(expect.objectContaining({ message: "filter failed" }));
    expect(commits[commits.length - 1]).toMatchObject({
      scope: { kind: "results", view: "filtered" },
      selectedLine: 125,
      selectedResultIndex: 12,
      viewportLine: 120,
      viewportResultIndex: 10,
      scrollRequest: { index: 10, align: "start" },
    });
  });

  it("safe-clears the placeholder Results state when remapping fails on a new dataset", async () => {
    let state = resultsState();
    const commits: TableNavigationCommit[] = [];
    const reportError = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        commits.push(commit);
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn().mockRejectedValue(new Error("mapping failed")),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
      reportError,
    });
    controller.openProblemContext(occurrence);
    state = {
      ...state,
      filterInputRevision: 5,
      appliedFilterInputRevision: 5,
      filterResultRevision: 10,
    };

    await controller.returnToResults();

    expect(reportError).toHaveBeenCalledWith(
      expect.objectContaining({ message: "mapping failed" }),
    );
    expect(commits[commits.length - 1]).toEqual({
      scope: { kind: "results", view: "filtered" },
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: null,
      tailFollowing: false,
    });
  });

  it("reports a current context row load failure without an unhandled rejection", async () => {
    let state = resultsState();
    const reportError = vi.fn();
    const controller = createTableScopeController({
      getState: () => state,
      commit: (commit) => {
        state = { ...state, ...commit };
        return true;
      },
      mapSourceLine: vi.fn(),
      waitForFilterResult: vi.fn(),
      subscribeStateChanges: noStateSubscription,
      loadContextRows: vi.fn().mockRejectedValue(new Error("rows failed")),
      reportError,
    });

    controller.openProblemContext(occurrence);

    await vi.waitFor(() =>
      expect(reportError).toHaveBeenCalledWith(expect.objectContaining({ message: "rows failed" })),
    );
  });
});
