import { useCallback, useEffect, useRef, useState } from "react";
import {
  getProblemDetail,
  getProblemGroups,
  getProblemOccurrences,
  getProblemsStatus,
  onProblemsProgress,
  releaseProblemSnapshot,
} from "@/lib/ipc";
import { sameAnalysisToken } from "@/lib/analysisToken";
import { problemsStatusFromProgress } from "@/lib/problems";
import { useProblems, type ProblemsSort } from "@/store/problems";
import { useSession } from "@/store/session";
import type {
  AnalysisToken,
  ProblemDetail,
  ProblemDetailRequest,
  ProblemGroup,
  ProblemGroupQueryRequest,
  ProblemKind,
  ProblemOccurrence,
  ProblemOccurrenceQueryRequest,
  ProblemPage,
  ProblemReleaseSnapshotRequest,
  ProblemsProgress,
  ProblemsStatus,
} from "@/types";

const PAGE_SIZE = 100;

export interface ProblemsLiveClient {
  getStatus: () => Promise<ProblemsStatus>;
  getGroups: (request: ProblemGroupQueryRequest) => Promise<ProblemPage<ProblemGroup>>;
  getOccurrences: (
    request: ProblemOccurrenceQueryRequest,
  ) => Promise<ProblemPage<ProblemOccurrence>>;
  getDetail: (request: ProblemDetailRequest) => Promise<ProblemDetail>;
  releaseSnapshot: (request: ProblemReleaseSnapshotRequest) => Promise<boolean>;
  onProgress: (listener: (progress: ProblemsProgress) => void) => Promise<() => void>;
}

const defaultClient: ProblemsLiveClient = {
  getStatus: getProblemsStatus,
  getGroups: getProblemGroups,
  getOccurrences: getProblemOccurrences,
  getDetail: getProblemDetail,
  releaseSnapshot: releaseProblemSnapshot,
  onProgress: onProblemsProgress,
};

function errorCode(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function isCurrentAnalysis(token: AnalysisToken): boolean {
  return sameAnalysisToken(useProblems.getState().analysisToken, token);
}

function acceptProblemsStatus(status: ProblemsStatus): void {
  const sessionGeneration = useSession.getState().status.generation;
  if (status.analysisToken.sessionGeneration !== sessionGeneration) return;

  const currentToken = useProblems.getState().analysisToken;
  if (
    currentToken?.sessionGeneration === status.analysisToken.sessionGeneration &&
    currentToken.analysisGeneration > status.analysisToken.analysisGeneration
  ) {
    return;
  }
  if (!sameAnalysisToken(currentToken, status.analysisToken)) {
    useProblems.getState().resetForAnalysis(status.analysisToken);
  }
  useProblems.getState().acceptStatus(status);
}

function snapshotRequest<T>(page: ProblemPage<T> | null): ProblemReleaseSnapshotRequest | null {
  if (!page) return null;
  return {
    snapshotHandle: page.snapshotHandle,
    expectedAnalysisToken: page.analysisToken,
  };
}

export interface ProblemsLiveBindings {
  onOpen: () => void;
  onRefresh: () => void;
  onSelectGroup: (groupId: number) => void;
  onSelectOccurrence: (eventId: number) => void;
  onLoadMoreGroups: () => void;
  onLoadMoreOccurrences: () => void;
  onRetryStatus: () => void;
  onRetryGroups: () => void;
  onRetryOccurrences: () => void;
  onRetryDetail: () => void;
  onSetKindFilter: (kind: ProblemKind | null) => void;
  onSetSort: (sort: ProblemsSort) => void;
}

export function useProblemsLive(client: ProblemsLiveClient = defaultClient): ProblemsLiveBindings {
  const sessionGeneration = useSession((state) => state.status.generation);
  const sessionId = useSession((state) => state.sessionId);
  const panelOpen = useProblems((state) => state.panelOpen);
  const analysisToken = useProblems((state) => state.analysisToken);
  const groupPage = useProblems((state) => state.groupPage);
  const groupLoading = useProblems((state) => state.groupLoading);
  const groupPageError = useProblems((state) => state.groupPageError);
  const statusRequestRef = useRef(0);
  const groupRequestRef = useRef(0);
  const occurrenceRequestRef = useRef(0);
  const detailRequestRef = useRef(0);
  const [progressSubscriptionRevision, setProgressSubscriptionRevision] = useState(0);

  const refreshStatus = useCallback(() => {
    const requestId = ++statusRequestRef.current;
    useProblems.setState({ statusLoading: true, statusError: null });
    void client
      .getStatus()
      .then((status) => {
        if (requestId !== statusRequestRef.current) return;
        acceptProblemsStatus(status);
      })
      .catch((error: unknown) => {
        if (requestId !== statusRequestRef.current) return;
        useProblems.setState({
          statusLoading: false,
          statusError: errorCode(error),
        });
      });
  }, [client]);

  const failGroupRequest = useCallback(
    (error: unknown, token: AnalysisToken, requestId: number) => {
      if (requestId !== groupRequestRef.current || !isCurrentAnalysis(token)) return;
      const code = errorCode(error);
      if (code.includes("snapshot-expired")) {
        useProblems.getState().markSnapshotExpired("groups");
      } else {
        useProblems.setState({ groupLoading: false, groupPageError: code });
      }
      if (code.includes("stale-analysis-token")) refreshStatus();
    },
    [refreshStatus],
  );

  const loadGroups = useCallback(
    (append: boolean) => {
      const state = useProblems.getState();
      const currentPage = state.groupPage;
      const token = state.analysisToken;
      if (!token || state.groupLoading) return;
      if (!append && currentPage) return;
      if (append && (!currentPage || currentPage.nextCursor == null)) return;

      const requestId = ++groupRequestRef.current;
      const frozenPage = append ? currentPage : null;
      const kind = state.kindFilters.length === 1 ? state.kindFilters[0] : null;
      useProblems.setState({ groupLoading: true, groupPageError: null });
      const request: ProblemGroupQueryRequest = frozenPage
        ? {
            expectedAnalysisToken: token,
            cursor: frozenPage.nextCursor!,
            limit: PAGE_SIZE,
          }
        : {
            expectedAnalysisToken: token,
            cursor: null,
            kind,
            sort: state.sort,
            limit: PAGE_SIZE,
          };
      void client
        .getGroups(request)
        .then((page) => {
          if (requestId !== groupRequestRef.current || !isCurrentAnalysis(token)) return;
          if (!sameAnalysisToken(page.analysisToken, token)) {
            failGroupRequest("stale-analysis-token", token, requestId);
            return;
          }
          const latest = useProblems.getState();
          if (frozenPage && latest.groupPage?.snapshotHandle !== frozenPage.snapshotHandle) {
            return;
          }
          if (append) latest.appendGroupPage(page);
          else latest.replaceGroupPage(page);
        })
        .catch((error: unknown) => failGroupRequest(error, token, requestId));
    },
    [client, failGroupRequest],
  );

  const failOccurrenceRequest = useCallback(
    (error: unknown, token: AnalysisToken, requestId: number) => {
      if (requestId !== occurrenceRequestRef.current || !isCurrentAnalysis(token)) return;
      const code = errorCode(error);
      if (code.includes("snapshot-expired")) {
        useProblems.getState().markSnapshotExpired("occurrences");
      } else {
        useProblems.setState({
          occurrenceLoading: false,
          occurrencePageError: code,
        });
      }
      if (code.includes("stale-analysis-token")) refreshStatus();
    },
    [refreshStatus],
  );

  const loadOccurrences = useCallback(
    (groupId: number, append: boolean) => {
      const state = useProblems.getState();
      const currentPage = state.occurrencePage;
      const token = state.analysisToken;
      if (!token || state.selectedGroupId !== groupId) return;
      if (append && (!currentPage || currentPage.nextCursor == null)) return;

      const requestId = ++occurrenceRequestRef.current;
      const frozenPage = append ? currentPage : null;
      useProblems.setState({
        occurrenceLoading: true,
        occurrencePageError: null,
      });
      const request: ProblemOccurrenceQueryRequest = frozenPage
        ? {
            expectedAnalysisToken: token,
            cursor: frozenPage.nextCursor!,
            limit: PAGE_SIZE,
          }
        : {
            expectedAnalysisToken: token,
            cursor: null,
            groupId,
            limit: PAGE_SIZE,
          };
      void client
        .getOccurrences(request)
        .then((page) => {
          const latest = useProblems.getState();
          if (
            requestId !== occurrenceRequestRef.current ||
            !isCurrentAnalysis(token) ||
            latest.selectedGroupId !== groupId
          ) {
            return;
          }
          if (!sameAnalysisToken(page.analysisToken, token)) {
            failOccurrenceRequest("stale-analysis-token", token, requestId);
            return;
          }
          if (
            frozenPage &&
            latest.occurrencePage?.snapshotHandle !== frozenPage.snapshotHandle
          ) {
            return;
          }
          if (append) latest.appendOccurrencePage(page);
          else latest.replaceOccurrencePage(page);
        })
        .catch((error: unknown) => failOccurrenceRequest(error, token, requestId));
    },
    [client, failOccurrenceRequest],
  );

  const failDetailRequest = useCallback(
    (error: unknown, token: AnalysisToken, eventId: number, requestId: number) => {
      const state = useProblems.getState();
      if (
        requestId !== detailRequestRef.current ||
        !sameAnalysisToken(state.analysisToken, token) ||
        state.selectedEventId !== eventId
      ) {
        return;
      }
      const code = errorCode(error);
      useProblems.setState({ detailLoading: false, detailError: code });
      if (code.includes("stale-analysis-token")) refreshStatus();
    },
    [refreshStatus],
  );

  const loadDetail = useCallback(
    (eventId: number) => {
      const state = useProblems.getState();
      const token = state.analysisToken;
      if (!token || state.selectedEventId !== eventId) return;

      const requestId = ++detailRequestRef.current;
      useProblems.setState({ detailLoading: true, detailError: null });
      void client
        .getDetail({ eventId, expectedAnalysisToken: token })
        .then((detail) => {
          const latest = useProblems.getState();
          if (
            requestId !== detailRequestRef.current ||
            !sameAnalysisToken(latest.analysisToken, token) ||
            latest.selectedEventId !== eventId
          ) {
            return;
          }
          if (!sameAnalysisToken(detail.analysisToken, token)) {
            failDetailRequest("stale-analysis-token", token, eventId, requestId);
            return;
          }
          latest.setDetail(detail);
        })
        .catch((error: unknown) => failDetailRequest(error, token, eventId, requestId));
    },
    [client, failDetailRequest],
  );

  const releaseSnapshot = useCallback(
    async (request: ProblemReleaseSnapshotRequest | null) => {
      if (!request) return;
      try {
        await client.releaseSnapshot(request);
      } catch {
        // Session/analysis replacement makes old snapshots unreachable already.
      }
    },
    [client],
  );

  const replaceGroupQuery = useCallback(
    (next: { kind?: ProblemKind | null; sort?: ProblemsSort }) => {
      const state = useProblems.getState();
      const token = state.analysisToken;
      if (!token) return;
      const currentKind = state.kindFilters.length === 1 ? state.kindFilters[0] : null;
      if (next.kind !== undefined && next.kind === currentKind) return;
      if (next.sort !== undefined && next.sort === state.sort) return;

      const releases = [
        snapshotRequest(state.groupPage),
        snapshotRequest(state.occurrencePage),
      ];
      const transitionRequestId = ++groupRequestRef.current;
      occurrenceRequestRef.current += 1;
      detailRequestRef.current += 1;
      if (next.kind !== undefined) {
        useProblems.getState().setKindFilters(next.kind == null ? [] : [next.kind]);
      } else if (next.sort !== undefined) {
        useProblems.getState().setSort(next.sort);
      }
      useProblems.setState({ groupLoading: true, hasNewResults: false });

      void Promise.all(releases.map(releaseSnapshot)).then(() => {
        if (
          transitionRequestId !== groupRequestRef.current ||
          !isCurrentAnalysis(token)
        ) {
          return;
        }
        useProblems.setState({ groupLoading: false });
        loadGroups(false);
      });
    },
    [loadGroups, releaseSnapshot],
  );

  const selectGroup = useCallback(
    (groupId: number) => {
      const previousSnapshot = snapshotRequest(useProblems.getState().occurrencePage);
      occurrenceRequestRef.current += 1;
      detailRequestRef.current += 1;
      useProblems.getState().selectGroup(groupId);
      void releaseSnapshot(previousSnapshot).then(() => {
        if (useProblems.getState().selectedGroupId === groupId) {
          loadOccurrences(groupId, false);
        }
      });
    },
    [loadOccurrences, releaseSnapshot],
  );

  const selectOccurrence = useCallback(
    (eventId: number) => {
      detailRequestRef.current += 1;
      useProblems.getState().selectEvent(eventId);
      loadDetail(eventId);
    },
    [loadDetail],
  );

  const refreshGroups = useCallback(() => {
    const state = useProblems.getState();
    const token = state.analysisToken;
    if (!token) return;
    const releases = [snapshotRequest(state.groupPage), snapshotRequest(state.occurrencePage)];
    groupRequestRef.current += 1;
    occurrenceRequestRef.current += 1;
    detailRequestRef.current += 1;
    useProblems.setState({
      selectedGroupId: null,
      selectedEventId: null,
      groupPage: null,
      occurrencePage: null,
      detail: null,
      groupLoading: true,
      occurrenceLoading: false,
      detailLoading: false,
      groupPageError: null,
      occurrencePageError: null,
      detailError: null,
      hasNewResults: false,
    });
    void Promise.all(releases.map(releaseSnapshot)).then(() => {
      if (!isCurrentAnalysis(token)) return;
      useProblems.setState({ groupLoading: false });
      loadGroups(false);
    });
  }, [loadGroups, releaseSnapshot]);

  const retryOccurrences = useCallback(() => {
    const state = useProblems.getState();
    const groupId = state.selectedGroupId;
    if (groupId == null) return;
    if (state.occurrencePageError === "snapshot-expired") {
      const request = snapshotRequest(state.occurrencePage);
      occurrenceRequestRef.current += 1;
      state.selectGroup(groupId);
      void releaseSnapshot(request).then(() => loadOccurrences(groupId, false));
      return;
    }
    loadOccurrences(groupId, state.occurrencePage != null);
  }, [loadOccurrences, releaseSnapshot]);

  useEffect(() => {
    statusRequestRef.current += 1;
    groupRequestRef.current += 1;
    occurrenceRequestRef.current += 1;
    detailRequestRef.current += 1;
    if (useProblems.getState().analysisToken?.sessionGeneration !== sessionGeneration) {
      useProblems.getState().clearAnalysis();
    }
    refreshStatus();
  }, [refreshStatus, sessionGeneration, sessionId]);

  useEffect(() => {
    let disposed = false;
    const unlisten = client
      .onProgress((progress) => {
        if (disposed) return;
        acceptProblemsStatus(problemsStatusFromProgress(progress));
      })
      .catch((error: unknown) => {
        if (!disposed) {
          useProblems.setState({
            statusLoading: false,
            statusError: errorCode(error),
          });
        }
        return null;
      });
    return () => {
      disposed = true;
      void unlisten.then((stop) => stop?.());
    };
  }, [client, progressSubscriptionRevision]);

  const retryStatus = useCallback(() => {
    refreshStatus();
    setProgressSubscriptionRevision((revision) => revision + 1);
  }, [refreshStatus]);

  useEffect(() => {
    if (panelOpen && analysisToken && !groupPage && !groupLoading && !groupPageError) {
      loadGroups(false);
    }
  }, [analysisToken, groupLoading, groupPage, groupPageError, loadGroups, panelOpen]);

  return {
    onOpen: () => loadGroups(false),
    onRefresh: refreshGroups,
    onSelectGroup: selectGroup,
    onSelectOccurrence: selectOccurrence,
    onLoadMoreGroups: () => loadGroups(true),
    onLoadMoreOccurrences: () => {
      const groupId = useProblems.getState().selectedGroupId;
      if (groupId != null) loadOccurrences(groupId, true);
    },
    onRetryStatus: retryStatus,
    onRetryGroups: () => {
      const state = useProblems.getState();
      if (state.groupPageError === "snapshot-expired") refreshGroups();
      else loadGroups(state.groupPage != null);
    },
    onRetryOccurrences: retryOccurrences,
    onRetryDetail: () => {
      const eventId = useProblems.getState().selectedEventId;
      if (eventId != null) loadDetail(eventId);
    },
    onSetKindFilter: (kind) => replaceGroupQuery({ kind }),
    onSetSort: (sort) => replaceGroupQuery({ sort }),
  };
}
