import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProblemsDock } from "@/components/ProblemsDock";
import { useProblems } from "@/store/problems";
import { TestResizeObserver } from "@/test/setup";
import type {
  AnalysisToken,
  ProblemDetail,
  ProblemGroup,
  ProblemOccurrence,
  ProblemsStatus,
} from "@/types";

const token: AnalysisToken = { sessionGeneration: 3, analysisGeneration: 1 };
const status: ProblemsStatus = {
  analysisToken: token,
  scannedLines: 900,
  stableLines: 1_000,
  scanning: true,
  finished: false,
  coverage: {
    origin: "static-file",
    requestedBuffers: null,
    rangeCompleteness: "bounded",
  },
  stats: {
    observedOccurrenceCount: 7,
    storedOccurrenceCount: 5,
    droppedOccurrenceCount: 2,
    provisionalOccurrenceCount: 0,
    storedGroupCount: 1,
    ungroupedDroppedOccurrenceCount: 1,
    droppedRecentObservationCount: 3,
    revision: 4,
    limited: true,
    correlationLimited: true,
  },
};

const group: ProblemGroup = {
  id: 2,
  kind: "java-crash",
  fingerprintVersion: 1,
  signatureQuality: "stack",
  identityQuality: "known-process",
  processSummary: "com.example.app",
  processSummaryTruncated: false,
  signatureSummary: "IllegalStateException · MainActivity.onCreate",
  signatureSummaryTruncated: false,
  fingerprint: "java-crash:example",
  observedOccurrenceCount: 5,
  storedOccurrenceCount: 4,
  droppedOccurrenceCount: 1,
  firstLine: 120,
  firstTimestamp: "07-26 11:00:00.000",
  lastLine: 920,
  lastTimestamp: "07-26 12:00:00.000",
  firstEventId: 7,
  lastEventId: 8,
  representativeEventId: 8,
};

const occurrence: ProblemOccurrence = {
  eventId: 8,
  groupId: 2,
  kind: "java-crash",
  startLine: 910,
  endLine: 930,
  anchorLine: 912,
  pid: 42,
  timestamp: "07-26 12:00:00.000",
  processInstanceId: 3,
  evidenceFlags: ["primary"],
  outcomeFlags: ["death-observed"],
  boundaryFlags: ["truncated-by-limit", "observation-refs-truncated"],
};

const detail: ProblemDetail = {
  analysisToken: token,
  revision: 4,
  occurrence,
  observationTotal: 9,
  factsTruncated: true,
  facts: [
    {
      code: "java-uncaught-exception",
      sourceLine: 912,
      ruleId: "aosp.java-uncaught.v1",
      role: "primary",
      evidenceFormat: "aosp-text",
      provenance: "known-main",
    },
    {
      code: "exception-type-recorded",
      sourceLine: 914,
      ruleId: "aosp.java-uncaught.v1",
      role: "supporting",
      evidenceFormat: "aosp-text",
      provenance: "known-main",
    },
  ],
};

function seedOpenDock() {
  const state = useProblems.getState();
  state.acceptStatus(status);
  state.setPanelOpen(true);
  state.replaceGroupPage({
    analysisToken: token,
    snapshotHandle: "snapshot-10",
    revision: 4,
    total: 1,
    items: [group],
    nextCursor: null,
  });
  state.selectGroup(group.id);
  state.replaceOccurrencePage({
    analysisToken: token,
    snapshotHandle: "snapshot-11",
    revision: 4,
    total: 1,
    items: [occurrence],
    nextCursor: null,
  });
  state.selectEvent(occurrence.eventId);
  state.setDetail(detail);
}

describe("ProblemsDock", () => {
  beforeEach(() => {
    useProblems.setState(useProblems.getInitialState(), true);
    useProblems.getState().resetForAnalysis(token);
    Object.defineProperty(window, "innerHeight", {
      configurable: true,
      value: 720,
    });
  });

  it("is folded by default, requests no list, and distinguishes detected from expandable", () => {
    useProblems.getState().acceptStatus(status);
    const onOpen = vi.fn();
    render(<ProblemsDock onOpen={onOpen} />);

    expect(screen.queryByRole("region", { name: "故障调查工作台" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Problems/ })).toHaveTextContent("检出 7 · 可展开 5");
    expect(screen.getByText(/正在分析 900 \/ 1,000 行/)).toHaveTextContent(
      "文件日志 · buffer 范围未知 · 仅覆盖文件所含范围",
    );
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("requests the first group page only after the user expands it", async () => {
    useProblems.getState().acceptStatus(status);
    const onOpen = vi.fn();
    render(<ProblemsDock onOpen={onOpen} />);

    await userEvent.click(screen.getByRole("button", { name: /Problems/ }));

    expect(screen.getByRole("region", { name: "故障调查工作台" })).toBeInTheDocument();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("separates located facts from static non-conclusive hints", async () => {
    seedOpenDock();
    const onLocateFact = vi.fn();
    render(<ProblemsDock onLocateFact={onLocateFact} />);

    expect(screen.getByRole("heading", { name: "检测到的事实" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "日志记录的结局" })).toBeInTheDocument();
    expect(screen.getByText("同一进程实例的结束得到日志佐证")).toBeInTheDocument();
    expect(screen.getByText("检测器在安全上限处截断了事件证据")).toBeInTheDocument();
    expect(
      screen.getAllByText(/aosp\.java-uncaught\.v1 · 主证据 · AOSP 文本 · 已证明来源：main/)
        .length,
    ).toBeGreaterThan(0);
    expect(screen.getByRole("heading", { name: "排查提示（非结论）" })).toBeInTheDocument();
    expect(screen.getByText("仅展示 2/9 条关键证据，可查看事件范围")).toBeInTheDocument();
    expect(screen.getByText("容量限制：检出 7 项，可展开 5 项，未保存 2 项。")).toBeInTheDocument();
    expect(screen.getByText("同组仅表示事件指纹相同，不代表根因相同。")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "定位到第 914 行" }));
    expect(onLocateFact).toHaveBeenCalledWith(914);
    expect(screen.queryByText(/根因是|确定为|已导致/)).not.toBeInTheDocument();
  });

  it("exposes a single fault-category filter and group ordering controls", async () => {
    seedOpenDock();
    const onSetKindFilter = vi.fn();
    const onSetSort = vi.fn();
    render(
      <ProblemsDock onSetKindFilter={onSetKindFilter} onSetSort={onSetSort} />,
    );

    expect(screen.getByRole("group", { name: "故障分类" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "全部故障" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("combobox", { name: "分组排序" })).toHaveValue(
      "last-seen-desc",
    );

    await userEvent.click(screen.getByRole("button", { name: "仅看 ANR" }));
    await userEvent.selectOptions(
      screen.getByRole("combobox", { name: "分组排序" }),
      "count-desc",
    );

    expect(onSetKindFilter).toHaveBeenCalledWith("anr");
    expect(onSetSort).toHaveBeenCalledWith("count-desc");
  });

  it("shows detected group count separately from the expandable retained count", () => {
    seedOpenDock();
    render(<ProblemsDock />);

    expect(
      screen.getByRole("option", {
        name: /Java\/Kotlin 崩溃 · 5 次（可展开 4）/,
      }),
    ).toBeInTheDocument();
  });

  it("offers locate, temporary context, and raw-log export actions", async () => {
    seedOpenDock();
    const onLocateOccurrence = vi.fn();
    const onOpenContext = vi.fn();
    const onExportOccurrence = vi.fn();
    render(
      <ProblemsDock
        onLocateOccurrence={onLocateOccurrence}
        onOpenContext={onOpenContext}
        onExportOccurrence={onExportOccurrence}
      />,
    );

    await userEvent.click(screen.getByRole("button", { name: "定位事件" }));
    await userEvent.click(screen.getByRole("button", { name: "查看上下文" }));
    await userEvent.click(screen.getByRole("button", { name: "导出原始日志" }));

    expect(onLocateOccurrence).toHaveBeenCalledWith(occurrence);
    expect(onOpenContext).toHaveBeenCalledWith(occurrence);
    expect(onExportOccurrence).toHaveBeenCalledWith(occurrence);
  });

  it("keeps a frozen page visible when loading the next page fails", () => {
    seedOpenDock();
    useProblems.setState({
      groupLoading: false,
      groupPageError: "snapshot-expired",
    });

    render(<ProblemsDock />);

    expect(screen.getByText("结果快照已过期；当前内容已保留，请手动刷新。")).toBeInTheDocument();
    expect(screen.getAllByText("Java/Kotlin 崩溃")).not.toHaveLength(0);
  });

  it("renders distinct loading and failure states", () => {
    useProblems.getState().setPanelOpen(true);
    useProblems.setState({
      groupLoading: false,
      groupPageError: "network unavailable",
    });

    render(<ProblemsDock onRetryGroups={vi.fn()} />);

    expect(screen.getByText("读取故障分组失败：network unavailable")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重试故障分组" })).toBeInTheDocument();
  });

  it("shows status loading independently from an empty analysis result", () => {
    useProblems.getState().setPanelOpen(true);
    useProblems.setState({ status: null, statusLoading: true });

    render(<ProblemsDock />);

    expect(screen.getByText("正在读取故障分析状态…")).toBeInTheDocument();
  });

  it("shows the bounded empty-capture message for an empty finished snapshot", () => {
    useProblems.getState().acceptStatus({
      ...status,
      scanning: false,
      finished: true,
    });
    useProblems.getState().setPanelOpen(true);
    useProblems.getState().replaceGroupPage({
      analysisToken: token,
      snapshotHandle: "snapshot-19",
      revision: 4,
      total: 0,
      items: [],
      nextCursor: null,
    });

    render(<ProblemsDock />);

    expect(screen.getByText("在已捕获范围内未检测到可展示的故障事件。")).toBeInTheDocument();
  });

  it("keeps focus on the listbox and activates non-focusable options by keyboard", async () => {
    seedOpenDock();
    const secondGroup: ProblemGroup = {
      ...group,
      id: 3,
      fingerprint: "java-crash:second",
      firstLine: 1_020,
      lastLine: 1_080,
    };
    useProblems.getState().replaceGroupPage({
      analysisToken: token,
      snapshotHandle: "snapshot-10",
      revision: 4,
      total: 2,
      items: [group, secondGroup],
      nextCursor: null,
    });
    useProblems.getState().selectGroup(null);
    const onSelectGroup = vi.fn();
    render(<ProblemsDock onSelectGroup={onSelectGroup} />);
    const listbox = screen.getByRole("listbox", { name: "故障分组" });
    const options = screen.getAllByRole("option", { name: /Java\/Kotlin 崩溃/ });

    expect(listbox).toHaveAttribute("tabindex", "0");
    expect(options.every((option) => option.tagName !== "BUTTON")).toBe(true);
    expect(options.every((option) => option.getAttribute("tabindex") == null)).toBe(true);

    listbox.focus();
    await userEvent.keyboard("{ArrowDown}{Enter}");

    expect(document.activeElement).toBe(listbox);
    expect(listbox).toHaveAttribute("aria-activedescendant", "lf-problem-group-3");
    expect(onSelectGroup).toHaveBeenCalledWith(3);
  });

  it("requests the next frozen page from keyboard and scroll boundaries", async () => {
    seedOpenDock();
    useProblems.getState().replaceGroupPage({
      analysisToken: token,
      snapshotHandle: "snapshot-10",
      revision: 4,
      total: 2,
      items: [group],
      nextCursor: "cursor-1",
    });
    useProblems.getState().selectGroup(null);
    const onLoadMoreGroups = vi.fn();
    render(<ProblemsDock onLoadMoreGroups={onLoadMoreGroups} />);
    const listbox = screen.getByRole("listbox", { name: "故障分组" });

    listbox.focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(onLoadMoreGroups).toHaveBeenCalledTimes(1);
    const secondGroup = {
      ...group,
      id: 3,
      fingerprint: "java-crash:next-page",
    };
    act(() => {
      useProblems.getState().replaceGroupPage({
        analysisToken: token,
        snapshotHandle: "snapshot-10",
        revision: 4,
        total: 3,
        items: [group, secondGroup],
        nextCursor: "cursor-2",
      });
    });
    expect(listbox).toHaveAttribute("aria-activedescendant", "lf-problem-group-3");

    Object.defineProperties(listbox, {
      clientHeight: { configurable: true, value: 200 },
      scrollHeight: { configurable: true, value: 500 },
      scrollTop: { configurable: true, value: 300 },
    });
    fireEvent.scroll(listbox);
    expect(onLoadMoreGroups).toHaveBeenCalledTimes(2);
  });

  it("cancels a pending keyboard page advance after navigation or a failed load", async () => {
    seedOpenDock();
    useProblems.getState().replaceGroupPage({
      analysisToken: token,
      snapshotHandle: "snapshot-keyboard-race",
      revision: 4,
      total: 2,
      items: [group],
      nextCursor: "cursor-1",
    });
    useProblems.getState().selectGroup(null);
    const onLoadMoreGroups = vi.fn();
    render(<ProblemsDock onLoadMoreGroups={onLoadMoreGroups} />);
    const listbox = screen.getByRole("listbox", { name: "故障分组" });
    const secondGroup = {
      ...group,
      id: 3,
      fingerprint: "java-crash:keyboard-race",
    };

    listbox.focus();
    await userEvent.keyboard("{ArrowDown}{Home}");
    act(() => {
      useProblems.getState().replaceGroupPage({
        analysisToken: token,
        snapshotHandle: "snapshot-keyboard-race",
        revision: 4,
        total: 2,
        items: [group, secondGroup],
        nextCursor: null,
      });
    });
    expect(listbox).toHaveAttribute("aria-activedescendant", "lf-problem-group-2");

    act(() => {
      useProblems.getState().replaceGroupPage({
        analysisToken: token,
        snapshotHandle: "snapshot-keyboard-race-2",
        revision: 5,
        total: 3,
        items: [group],
        nextCursor: "cursor-2",
      });
    });
    await userEvent.keyboard("{ArrowDown}");
    act(() => {
      useProblems.setState({
        groupLoading: true,
        groupPageError: null,
      });
    });
    act(() => {
      useProblems.setState({
        groupLoading: false,
        groupPageError: "snapshot-expired",
      });
    });
    act(() => {
      useProblems.getState().replaceGroupPage({
        analysisToken: token,
        snapshotHandle: "snapshot-keyboard-race-2",
        revision: 5,
        total: 2,
        items: [group, secondGroup],
        nextCursor: null,
      });
    });
    expect(listbox).toHaveAttribute("aria-activedescendant", "lf-problem-group-2");
  });

  it("virtualizes a synthetic 10,000-group frozen snapshot", () => {
    useProblems.getState().setPanelOpen(true);
    const groups = Array.from({ length: 10_000 }, (_, index) => ({
      ...group,
      id: index + 1,
      fingerprint: `java-crash:${index + 1}`,
      firstLine: index * 10 + 1,
      lastLine: index * 10 + 5,
    }));
    useProblems.getState().replaceGroupPage({
      analysisToken: token,
      snapshotHandle: "snapshot-23",
      revision: 4,
      total: 10_000,
      items: groups,
      nextCursor: null,
    });

    render(<ProblemsDock />);

    const options = within(
      screen.getByRole("listbox", { name: "故障分组" }),
    ).getAllByRole("option");
    expect(options.length).toBeLessThan(100);
    expect(options[0]).toHaveAttribute("aria-setsize", "10000");
  });

  it("clamps separator keyboard and pointer resizing to the observed workbench", async () => {
    useProblems.getState().setPanelOpen(true);
    const { container } = render(
      <div className="lf-workbench">
        <ProblemsDock />
      </div>,
    );
    const workbench = container.querySelector(".lf-workbench");
    if (!workbench) throw new Error("missing workbench");
    const observer = [...TestResizeObserver.instances].find((item) => item.observed.has(workbench));
    if (!observer) throw new Error("workbench is not observed");

    act(() => observer.emit(workbench, 1_180, 640));
    const separator = screen.getByRole("separator", { name: "调整 Problems 面板高度" });
    expect(separator).toHaveAttribute("aria-valuemin", "180");
    expect(separator).toHaveAttribute("aria-valuemax", "324");
    expect(separator).toHaveAttribute("aria-valuenow", "280");

    separator.focus();
    await userEvent.keyboard("{ArrowUp}{PageUp}");
    expect(separator).toHaveAttribute("aria-valuenow", "324");
    await userEvent.keyboard("{Home}");
    expect(separator).toHaveAttribute("aria-valuenow", "180");
    await userEvent.keyboard("{End}");
    expect(separator).toHaveAttribute("aria-valuenow", "324");

    fireEvent.pointerDown(separator, { clientY: 400, pointerId: 1 });
    fireEvent.pointerMove(window, { clientY: 560 });
    fireEvent.pointerUp(window, { pointerId: 1 });
    expect(useProblems.getState().panelHeight).toBe(180);
  });

  it("temporarily collapses in an extremely short workbench without losing open preference", () => {
    useProblems.getState().setPanelOpen(true);
    const { container } = render(
      <div className="lf-workbench">
        <ProblemsDock />
      </div>,
    );
    const workbench = container.querySelector(".lf-workbench");
    if (!workbench) throw new Error("missing workbench");
    const observer = [...TestResizeObserver.instances].find((item) => item.observed.has(workbench));
    if (!observer) throw new Error("workbench is not observed");

    act(() => observer.emit(workbench, 960, 330));
    expect(screen.queryByRole("region", { name: "故障调查工作台" })).not.toBeInTheDocument();
    expect(useProblems.getState().panelOpen).toBe(true);

    act(() => observer.emit(workbench, 1_180, 640));
    expect(screen.getByRole("region", { name: "故障调查工作台" })).toBeInTheDocument();
    expect(useProblems.getState().panelOpen).toBe(true);
  });
});
