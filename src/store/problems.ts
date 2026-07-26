import { create } from "zustand";
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

type ProblemsSort = "last-seen-desc" | "count-desc";
type SnapshotError = "snapshot-expired" | "snapshot-error" | null;

interface ProblemsState {
  analysisToken: AnalysisToken | null;
  status: ProblemsStatus | null;
  panelOpen: boolean;
  panelHeight: number;
  kindFilters: ProblemKind[];
  sort: ProblemsSort;
  selectedGroupId: number | null;
  selectedEventId: number | null;
  groupPage: ProblemPage<ProblemGroup> | null;
  occurrencePage: ProblemPage<ProblemOccurrence> | null;
  detail: ProblemDetail | null;
  groupPageError: SnapshotError;
  occurrencePageError: SnapshotError;
  hasNewResults: boolean;
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

function sameToken(left: AnalysisToken | null, right: AnalysisToken) {
  return (
    left?.sessionGeneration === right.sessionGeneration &&
    left.analysisGeneration === right.analysisGeneration
  );
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
  groupPageError: null,
  occurrencePageError: null,
  hasNewResults: false,
};

export const useProblems = create<ProblemsState>()((set) => ({
  analysisToken: null,
  status: null,
  ...initialUi,
  resetForAnalysis: (analysisToken) =>
    set({
      analysisToken,
      status: null,
      ...initialUi,
      kindFilters: [],
    }),
  acceptStatus: (status) =>
    set((state) => {
      if (!sameToken(state.analysisToken, status.analysisToken)) return {};
      return {
        status,
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
      groupPageError: null,
      occurrencePageError: null,
    }),
  setSort: (sort) =>
    set({
      sort,
      selectedGroupId: null,
      selectedEventId: null,
      groupPage: null,
      occurrencePage: null,
      detail: null,
      groupPageError: null,
      occurrencePageError: null,
    }),
  selectGroup: (selectedGroupId) =>
    set({
      selectedGroupId,
      selectedEventId: null,
      occurrencePage: null,
      detail: null,
      occurrencePageError: null,
    }),
  selectEvent: (selectedEventId) => set({ selectedEventId, detail: null }),
  replaceGroupPage: (groupPage) =>
    set({
      groupPage,
      groupPageError: null,
      hasNewResults: false,
    }),
  appendGroupPage: (page) =>
    set((state) => ({
      groupPage: state.groupPage
        ? appendSnapshotPage(state.groupPage, page, (item) => item.id)
        : page,
      groupPageError: null,
    })),
  replaceOccurrencePage: (occurrencePage) =>
    set({ occurrencePage, occurrencePageError: null }),
  appendOccurrencePage: (page) =>
    set((state) => ({
      occurrencePage: state.occurrencePage
        ? appendSnapshotPage(state.occurrencePage, page, (item) => item.eventId)
        : page,
      occurrencePageError: null,
    })),
  setDetail: (detail) =>
    set((state) =>
      detail && !sameToken(state.analysisToken, detail.analysisToken) ? {} : { detail },
    ),
  markSnapshotExpired: (kind) =>
    set(
      kind === "groups"
        ? { groupPageError: "snapshot-expired" }
        : { occurrencePageError: "snapshot-expired" },
    ),
  clearNewResults: () => set({ hasNewResults: false }),
}));
