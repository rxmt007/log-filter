import { beforeEach, describe, expect, it, vi } from "vitest";
import { createSessionTableScopeController } from "@/lib/sessionTableScope";
import type { LineMappingResponse } from "@/lib/tableScopeController";
import { useSession } from "@/store/session";
import type { Status } from "@/types";

const mocks = vi.hoisted(() => ({
  mapSourceLine: vi.fn(),
}));

vi.mock("@/lib/ipc", () => ({
  mapSourceLine: mocks.mapSourceLine,
}));

const status: Status = {
  totalLines: 1_000,
  stableLines: 1_000,
  filteredLines: 100,
  bookmarkLines: 0,
  errorLines: 0,
  indexedBytes: 10_000,
  totalBytes: 10_000,
  indexing: false,
  generation: 7,
  analysisGeneration: 3,
  filterInputRevision: 4,
  appliedFilterInputRevision: 4,
  filterResultRevision: 9,
  decodeRevision: 0,
  sourceDataRevision: 1,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("session table scope controller", () => {
  beforeEach(() => {
    mocks.mapSourceLine.mockReset();
    useSession.setState({
      status,
      tableScope: { kind: "results", view: "filtered" },
      filterRevision: 4,
      appliedFilterInputRevision: 4,
      filterResultRevision: 9,
      selectedLine: null,
      selectedResultIndex: null,
      viewportLine: 1,
      viewportResultIndex: 0,
      scrollRequest: null,
      tailFollowing: false,
    });
  });

  it("waits for the backend-reported filter result before retrying a stale mapping", async () => {
    const firstMapping = deferred<LineMappingResponse>();
    const secondMapping = deferred<LineMappingResponse>();
    mocks.mapSourceLine
      .mockReturnValueOnce(firstMapping.promise)
      .mockReturnValueOnce(secondMapping.promise);
    const controller = createSessionTableScopeController();

    const navigating = controller.navigateToSourceLine(490, "problem-anchor");
    await vi.waitFor(() => expect(mocks.mapSourceLine).toHaveBeenCalledTimes(1));
    firstMapping.resolve({
      status: "stale-filter-result",
      analysisToken: { sessionGeneration: 7, analysisGeneration: 3 },
      actualFilterResultRevision: 10,
    });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));

    expect(mocks.mapSourceLine).toHaveBeenCalledTimes(1);

    useSession.setState({
      status: { ...status, filterResultRevision: 10 },
      filterResultRevision: 10,
    });
    await vi.waitFor(() => expect(mocks.mapSourceLine).toHaveBeenCalledTimes(2));
    expect(mocks.mapSourceLine.mock.calls[1][0]).toMatchObject({
      expectedFilterResultRevision: 10,
    });
    secondMapping.resolve({
      status: "ok",
      analysisToken: { sessionGeneration: 7, analysisGeneration: 3 },
      filterResultRevision: 10,
      target: { lineNo: 490, resultIndex: 21 },
    });

    await navigating;

    expect(mocks.mapSourceLine).toHaveBeenCalledTimes(2);
    expect(useSession.getState()).toMatchObject({
      selectedLine: 490,
      selectedResultIndex: 21,
      viewportLine: 490,
      viewportResultIndex: 21,
    });
  });
});
