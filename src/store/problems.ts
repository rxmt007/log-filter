import { create } from "zustand";
import { sameAnalysisToken } from "@/lib/analysisToken";
import { appendSnapshotPage } from "@/lib/problems";
import type {
  AnalysisToken,
  ProblemDetail,
  ProblemGroup,
  ProblemKind,
  ProblemOccurrence,
  ProblemPage,
  ProblemsStatus,
} from "@/types";

export type ProblemsSort = "last-seen-desc" | "count-desc";
type ProblemsLoadError = string | null;

interface ProblemsState {
  analysisToken: AnalysisToken | null;
  status: ProblemsStatus | null;
  statusLoading: boolean;
  statusError: ProblemsLoadError;
  panelOpen: boolean;
  panelHeight: number;
  kindFilters: ProblemKind[];
  sort: ProblemsSort;
  selectedGroupId: number | null;
  selectedEventId: number | null;
  groupPage: ProblemPage<ProblemGroup> | null;
  occurrencePage: ProblemPage<ProblemOccurrence> | null;
  detail: ProblemDetail | null;
  groupLoading: boolean;
  occurrenceLoading: boolean;
  detailLoading: boolean;
  groupPageError: ProblemsLoadError;
  occurrencePageError: ProblemsLoadError;
  detailError: ProblemsLoadError;
  hasNewResults: boolean;
  clearAnalysis: () => void;
  resetForAnalysis: (token: AnalysisToken) => void;
  acceptStatus: (status: ProblemsStatus) => void;
  setPanelOpen: (open: boolean) => void;
  setPanelHeight: (height: number) => void;
  setKindFilters: (kinds: ProblemKind[]) => void;
  setSort: (sort: ProblemsSort) => void;
  selectGroup: (id: number | null) => void;
  selectEvent: (id: number | null) => void;
  replaceGroupPage: (page: ProblemPage<ProblemGroup>) => void;
  appendGroupPage: (page: ProblemPage<ProblemGroup>) => void;
  replaceOccurrencePage: (page: ProblemPage<ProblemOccurrence>) => void;
  appendOccurrencePage: (page: ProblemPage<ProblemOccurrence>) => void;
  setDetail: (detail: ProblemDetail | null) => void;
  markSnapshotExpired: (kind: "groups" | "occurrences") => void;
  clearNewResults: () => void;
}

const initialUi = {
  panelOpen: false,
  panelHeight: 280,
  kindFilters: [] as ProblemKind[],
  sort: "last-seen-desc" as ProblemsSort,
  selectedGroupId: null,
  selectedEventId: null,
  groupPage: null,
  occurrencePage: null,
  detail: null,
  groupLoading: false,
  occurrenceLoading: false,
  detailLoading: false,
  groupPageError: null,
  occurrencePageError: null,
  detailError: null,
  hasNewResults: false,
};

export const useProblems = create<ProblemsState>()((set) => ({
  analysisToken: null,
  status: null,
  statusLoading: false,
  statusError: null,
  ...initialUi,
  clearAnalysis: () =>
    set((state) => ({
      analysisToken: null,
      status: null,
      statusLoading: false,
      statusError: null,
      ...initialUi,
      panelOpen: state.panelOpen,
      panelHeight: state.panelHeight,
      kindFilters: state.kindFilters,
      sort: state.sort,
    })),
  resetForAnalysis: (analysisToken) =>
    set((state) => ({
      analysisToken,
      status: null,
      statusLoading: false,
      statusError: null,
      ...initialUi,
      panelOpen: state.panelOpen,
      panelHeight: state.panelHeight,
      kindFilters: state.kindFilters,
      sort: state.sort,
    })),
  acceptStatus: (status) =>
    set((state) => {
      if (!sameAnalysisToken(state.analysisToken, status.analysisToken)) return {};
      if (
        state.status &&
        (status.scannedLines < state.status.scannedLines ||
          status.stats.revision < state.status.stats.revision)
      ) {
        return {};
      }
      return {
        status,
        statusLoading: false,
        statusError: null,
        hasNewResults:
          state.hasNewResults ||
          (state.groupPage != null && status.stats.revision > state.groupPage.revision),
      };
    }),
  setPanelOpen: (panelOpen) => set({ panelOpen }),
  setPanelHeight: (panelHeight) => set({ panelHeight }),
  setKindFilters: (kindFilters) =>
    set({
      kindFilters,
      selectedGroupId: null,
      selectedEventId: null,
      groupPage: null,
      occurrencePage: null,
      detail: null,
      groupLoading: false,
      occurrenceLoading: false,
      detailLoading: false,
      groupPageError: null,
      occurrencePageError: null,
      detailError: null,
    }),
  setSort: (sort) =>
    set({
      sort,
      selectedGroupId: null,
      selectedEventId: null,
      groupPage: null,
      occurrencePage: null,
      detail: null,
      groupLoading: false,
      occurrenceLoading: false,
      detailLoading: false,
      groupPageError: null,
      occurrencePageError: null,
      detailError: null,
    }),
  selectGroup: (selectedGroupId) =>
    set({
      selectedGroupId,
      selectedEventId: null,
      occurrencePage: null,
      detail: null,
      occurrenceLoading: false,
      detailLoading: false,
      occurrencePageError: null,
      detailError: null,
    }),
  selectEvent: (selectedEventId) =>
    set({
      selectedEventId,
      detail: null,
      detailLoading: false,
      detailError: null,
    }),
  replaceGroupPage: (groupPage) =>
    set((state) =>
      sameAnalysisToken(state.analysisToken, groupPage.analysisToken)
        ? {
            groupPage,
            groupLoading: false,
            groupPageError: null,
            hasNewResults: false,
          }
        : {},
    ),
  appendGroupPage: (page) =>
    set((state) =>
      sameAnalysisToken(state.analysisToken, page.analysisToken)
        ? {
            groupPage: state.groupPage
              ? appendSnapshotPage(state.groupPage, page, (item) => item.id)
              : page,
            groupLoading: false,
            groupPageError: null,
          }
        : {},
    ),
  replaceOccurrencePage: (occurrencePage) =>
    set((state) =>
      sameAnalysisToken(state.analysisToken, occurrencePage.analysisToken)
        ? {
            occurrencePage,
            occurrenceLoading: false,
            occurrencePageError: null,
          }
        : {},
    ),
  appendOccurrencePage: (page) =>
    set((state) =>
      sameAnalysisToken(state.analysisToken, page.analysisToken)
        ? {
            occurrencePage: state.occurrencePage
              ? appendSnapshotPage(state.occurrencePage, page, (item) => item.eventId)
              : page,
            occurrenceLoading: false,
            occurrencePageError: null,
          }
        : {},
    ),
  setDetail: (detail) =>
    set((state) =>
      detail && !sameAnalysisToken(state.analysisToken, detail.analysisToken)
        ? {}
        : { detail, detailLoading: false, detailError: null },
    ),
  markSnapshotExpired: (kind) =>
    set(
      kind === "groups"
        ? { groupLoading: false, groupPageError: "snapshot-expired" }
        : { occurrenceLoading: false, occurrencePageError: "snapshot-expired" },
    ),
  clearNewResults: () => set({ hasNewResults: false }),
}));
