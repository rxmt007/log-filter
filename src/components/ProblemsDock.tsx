import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { useVirtualizer, type Rect, type Virtualizer } from "@tanstack/react-virtual";
import {
  AlertTriangle,
  ChevronDown,
  ChevronUp,
  Download,
  Eye,
  MapPin,
  RefreshCw,
} from "lucide-react";
import { problemKindLabel } from "@/lib/problems";
import { useProblems, type ProblemsSort } from "@/store/problems";
import type { InputCoverage, ProblemKind, ProblemOccurrence } from "@/types";

const MIN_PANEL_HEIGHT = 180;
const MIN_MAIN_HEIGHT = 160;
const LIST_ITEM_HEIGHT = 54;
const GROUP_LIST_ITEM_HEIGHT = 66;

const PROBLEM_KIND_FILTERS: Array<{ kind: ProblemKind; label: string }> = [
  { kind: "java-crash", label: "Java/Kotlin" },
  { kind: "java-oom", label: "Java OOM" },
  { kind: "anr", label: "ANR" },
  { kind: "native-crash", label: "Native" },
  { kind: "process-restart", label: "进程重启" },
  { kind: "signal-exit", label: "Signal" },
  { kind: "lmk-kill", label: "LMK" },
  { kind: "kernel-oom-kill", label: "Kernel OOM" },
];

const BUFFER_LABELS = {
  main: "main",
  system: "system",
  events: "events",
  crash: "crash",
  radio: "radio",
  kernel: "kernel",
} as const;

function coverageDescription(coverage: InputCoverage): string {
  const origin = coverage.origin === "static-file" ? "文件日志" : "ADB 实时抓取";
  const buffers =
    coverage.requestedBuffers == null
      ? "buffer 范围未知"
      : coverage.requestedBuffers.length === 0
        ? "未声明 buffer"
        : `buffer: ${coverage.requestedBuffers.map((item) => BUFFER_LABELS[item]).join(", ")}`;
  const completeness = {
    unknown: "时间范围完整性未知",
    bounded: "仅覆盖文件所含范围",
    "start-truncated": "抓取开始前的日志不在样本中",
    "end-truncated": "日志尾部可能不完整",
  }[coverage.rangeCompleteness];
  return `${origin} · ${buffers} · ${completeness}`;
}

function clampPanelHeight(height: number, maximum: number): number {
  return Math.min(Math.max(MIN_PANEL_HEIGHT, height), maximum);
}

function observeProblemListRect<TScrollElement extends Element, TItemElement extends Element>(
  instance: Virtualizer<TScrollElement, TItemElement>,
  callback: (rect: Rect) => void,
) {
  const element = instance.scrollElement;
  if (!element) return;
  const update = () => {
    const rect = element.getBoundingClientRect();
    callback({
      width: rect.width || element.clientWidth || 320,
      height: rect.height || element.clientHeight || 240,
    });
  };
  update();
  const observer = new ResizeObserver(update);
  observer.observe(element);
  return () => observer.disconnect();
}

interface ProblemListboxProps<T> {
  label: string;
  idPrefix: string;
  datasetKey: string;
  items: T[];
  total: number;
  selectedId: number | null;
  loading?: boolean;
  loadFailed?: boolean;
  getId: (item: T) => number;
  onChoose: (item: T) => void;
  onReachEnd?: () => void;
  itemHeight?: number;
  children: (item: T) => ReactNode;
}

function ProblemListbox<T>({
  label,
  idPrefix,
  datasetKey,
  items,
  total,
  selectedId,
  loading = false,
  loadFailed = false,
  getId,
  onChoose,
  onReachEnd,
  itemHeight = LIST_ITEM_HEIGHT,
  children,
}: ProblemListboxProps<T>) {
  const parentRef = useRef<HTMLDivElement>(null);
  const pendingKeyboardAdvance = useRef<{
    datasetKey: string;
    fromCount: number;
    fromIndex: number;
    fromId: number;
    sawLoading: boolean;
  } | null>(null);
  const [activeIndex, setActiveIndex] = useState(() => {
    const selectedIndex = items.findIndex((item) => getId(item) === selectedId);
    return selectedIndex >= 0 ? selectedIndex : 0;
  });
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => itemHeight,
    getItemKey: (index) => getId(items[index]),
    overscan: 6,
    initialRect: { width: 320, height: 240 },
    observeElementRect: observeProblemListRect,
  });

  useEffect(() => {
    const selectedIndex = items.findIndex((item) => getId(item) === selectedId);
    if (selectedIndex >= 0) {
      setActiveIndex(selectedIndex);
      return;
    }
    setActiveIndex((current) => Math.min(Math.max(0, current), Math.max(0, items.length - 1)));
  }, [getId, items, selectedId]);

  useEffect(() => {
    const pending = pendingKeyboardAdvance.current;
    if (!pending) return;
    if (pending.datasetKey !== datasetKey || loadFailed) {
      pendingKeyboardAdvance.current = null;
      return;
    }
    if (items.length > pending.fromCount) {
      const unchanged =
        activeIndex === pending.fromIndex &&
        items[pending.fromIndex] != null &&
        getId(items[pending.fromIndex]) === pending.fromId;
      pendingKeyboardAdvance.current = null;
      if (unchanged) setActiveIndex(pending.fromCount);
      return;
    }
    if (loading) {
      pending.sawLoading = true;
    } else if (pending.sawLoading) {
      pendingKeyboardAdvance.current = null;
    }
  }, [activeIndex, datasetKey, getId, items, loadFailed, loading]);

  useEffect(() => {
    pendingKeyboardAdvance.current = null;
  }, [datasetKey, selectedId]);

  useEffect(() => {
    if (items.length === 0) return;
    virtualizer.scrollToIndex(activeIndex, { align: "auto" });
  }, [activeIndex, items.length, virtualizer]);

  const choose = useCallback(
    (index: number) => {
      const item = items[index];
      if (!item) return;
      pendingKeyboardAdvance.current = null;
      setActiveIndex(index);
      onChoose(item);
    },
    [items, onChoose],
  );

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (items.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (activeIndex >= items.length - 1) {
        pendingKeyboardAdvance.current =
          onReachEnd == null
            ? null
            : {
                datasetKey,
                fromCount: items.length,
                fromIndex: activeIndex,
                fromId: getId(items[activeIndex]),
                sawLoading: loading,
              };
        onReachEnd?.();
      } else {
        pendingKeyboardAdvance.current = null;
        setActiveIndex(activeIndex + 1);
      }
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      pendingKeyboardAdvance.current = null;
      setActiveIndex(Math.max(0, activeIndex - 1));
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      pendingKeyboardAdvance.current = null;
      setActiveIndex(0);
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      pendingKeyboardAdvance.current = null;
      setActiveIndex(items.length - 1);
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      choose(activeIndex);
    }
  };

  const handleScroll = () => {
    const element = parentRef.current;
    if (!element || !onReachEnd) return;
    if (element.scrollTop + element.clientHeight >= element.scrollHeight - itemHeight) {
      onReachEnd();
    }
  };

  const virtualItems = virtualizer.getVirtualItems();
  const activeItem = items[activeIndex];
  const activeId = activeItem == null ? undefined : `${idPrefix}-${getId(activeItem)}`;

  return (
    <div
      ref={parentRef}
      className="lf-problems-list"
      role="listbox"
      tabIndex={0}
      aria-label={label}
      aria-activedescendant={activeId}
      onKeyDown={handleKeyDown}
      onScroll={handleScroll}
    >
      <div className="lf-problems-list-virtual" style={{ height: virtualizer.getTotalSize() }}>
        {virtualItems.map((virtualItem) => {
          const item = items[virtualItem.index];
          const id = getId(item);
          return (
            <div
              id={`${idPrefix}-${id}`}
              key={virtualItem.key}
              role="option"
              aria-selected={selectedId === id}
              aria-posinset={virtualItem.index + 1}
              aria-setsize={total}
              className="lf-problems-item"
              style={{
                height: virtualItem.size,
                transform: `translateY(${virtualItem.start}px)`,
              }}
              onClick={() => {
                parentRef.current?.focus();
                choose(virtualItem.index);
              }}
            >
              {children(item)}
            </div>
          );
        })}
      </div>
    </div>
  );
}

interface ProblemsDockProps {
  onOpen?: () => void;
  onRefresh?: () => void;
  onSelectGroup?: (groupId: number) => void;
  onSelectOccurrence?: (eventId: number) => void;
  onLoadMoreGroups?: () => void;
  onLoadMoreOccurrences?: () => void;
  onRetryStatus?: () => void;
  onRetryGroups?: () => void;
  onRetryOccurrences?: () => void;
  onRetryDetail?: () => void;
  onSetKindFilter?: (kind: ProblemKind | null) => void;
  onSetSort?: (sort: ProblemsSort) => void;
  onLocateOccurrence?: (occurrence: ProblemOccurrence) => void;
  onOpenContext?: (occurrence: ProblemOccurrence) => void;
  onExportOccurrence?: (occurrence: ProblemOccurrence) => void;
}

export function ProblemsDock({
  onOpen,
  onRefresh,
  onSelectGroup,
  onSelectOccurrence,
  onLoadMoreGroups,
  onLoadMoreOccurrences,
  onRetryStatus,
  onRetryGroups,
  onRetryOccurrences,
  onRetryDetail,
  onSetKindFilter,
  onSetSort,
  onLocateOccurrence,
  onOpenContext,
  onExportOccurrence,
}: ProblemsDockProps) {
  const panelOpen = useProblems((state) => state.panelOpen);
  const setPanelOpen = useProblems((state) => state.setPanelOpen);
  const panelHeight = useProblems((state) => state.panelHeight);
  const status = useProblems((state) => state.status);
  const statusLoading = useProblems((state) => state.statusLoading);
  const groupPage = useProblems((state) => state.groupPage);
  const occurrencePage = useProblems((state) => state.occurrencePage);
  const detail = useProblems((state) => state.detail);
  const selectedGroupId = useProblems((state) => state.selectedGroupId);
  const selectedEventId = useProblems((state) => state.selectedEventId);
  const selectGroup = useProblems((state) => state.selectGroup);
  const selectEvent = useProblems((state) => state.selectEvent);
  const hasNewResults = useProblems((state) => state.hasNewResults);
  const groupPageError = useProblems((state) => state.groupPageError);
  const occurrencePageError = useProblems((state) => state.occurrencePageError);
  const detailError = useProblems((state) => state.detailError);
  const statusError = useProblems((state) => state.statusError);
  const groupLoading = useProblems((state) => state.groupLoading);
  const occurrenceLoading = useProblems((state) => state.occurrenceLoading);
  const detailLoading = useProblems((state) => state.detailLoading);
  const setPanelHeight = useProblems((state) => state.setPanelHeight);
  const kindFilters = useProblems((state) => state.kindFilters);
  const sort = useProblems((state) => state.sort);
  const setKindFilters = useProblems((state) => state.setKindFilters);
  const setSort = useProblems((state) => state.setSort);
  const openRequestArmed = useRef(true);
  const toggleRef = useRef<HTMLButtonElement>(null);
  const resizeCleanupRef = useRef<(() => void) | null>(null);
  const [panelMaximum, setPanelMaximum] = useState(() =>
    Math.max(MIN_PANEL_HEIGHT, Math.floor(window.innerHeight * 0.45)),
  );
  const [layoutCollapsed, setLayoutCollapsed] = useState(false);
  const visibleOpen = panelOpen && !layoutCollapsed;

  useEffect(() => {
    if (!visibleOpen) {
      openRequestArmed.current = true;
      return;
    }
    if (openRequestArmed.current) {
      openRequestArmed.current = false;
      onOpen?.();
    }
  }, [onOpen, visibleOpen]);

  useEffect(() => {
    const workbench = toggleRef.current?.closest(".lf-workbench");
    if (!workbench) return;
    const observer = new ResizeObserver((entries) => {
      const height = entries[entries.length - 1]?.contentRect.height ?? 0;
      if (height <= 0) return;
      const shouldCollapse = height < MIN_MAIN_HEIGHT + MIN_PANEL_HEIGHT;
      setLayoutCollapsed(shouldCollapse);
      const maximum = Math.max(
        MIN_PANEL_HEIGHT,
        Math.floor(Math.min(window.innerHeight * 0.45, height - MIN_MAIN_HEIGHT)),
      );
      setPanelMaximum(maximum);
      if (shouldCollapse) return;
      const current = useProblems.getState().panelHeight;
      const clamped = clampPanelHeight(current, maximum);
      if (clamped !== current) setPanelHeight(clamped);
    });
    observer.observe(workbench);
    return () => observer.disconnect();
  }, [panelOpen, setPanelHeight]);

  useEffect(
    () => () => {
      resizeCleanupRef.current?.();
    },
    [],
  );

  const resizeBy = useCallback(
    (delta: number) => {
      const current = useProblems.getState().panelHeight;
      setPanelHeight(clampPanelHeight(current + delta, panelMaximum));
    },
    [panelMaximum, setPanelHeight],
  );

  const handleSeparatorKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "ArrowUp") {
      event.preventDefault();
      resizeBy(16);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      resizeBy(-16);
    } else if (event.key === "PageUp") {
      event.preventDefault();
      resizeBy(64);
    } else if (event.key === "PageDown") {
      event.preventDefault();
      resizeBy(-64);
    } else if (event.key === "Home") {
      event.preventDefault();
      setPanelHeight(MIN_PANEL_HEIGHT);
    } else if (event.key === "End") {
      event.preventDefault();
      setPanelHeight(panelMaximum);
    }
  };

  const startPanelResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    event.preventDefault();
    resizeCleanupRef.current?.();
    const startY = event.clientY;
    const startHeight = useProblems.getState().panelHeight;
    const resize = (moveEvent: PointerEvent) => {
      setPanelHeight(clampPanelHeight(startHeight + startY - moveEvent.clientY, panelMaximum));
    };
    const finish = () => {
      window.removeEventListener("pointermove", resize);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      resizeCleanupRef.current = null;
    };
    window.addEventListener("pointermove", resize);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
    resizeCleanupRef.current = finish;
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // Window listeners keep the resize bounded if pointer capture is unavailable.
    }
  };

  const stats = status?.stats;
  const detected = stats?.observedOccurrenceCount ?? 0;
  const expandable = stats?.storedOccurrenceCount ?? 0;
  const provisional = stats?.provisionalOccurrenceCount ?? 0;
  const badge = stats?.limited
    ? `检出 ${detected.toLocaleString()} · 可展开 ${expandable.toLocaleString()}`
    : provisional > 0
      ? `${detected.toLocaleString()} · 待定 ${provisional.toLocaleString()}`
      : detected.toLocaleString();
  const scanLabel =
    statusLoading && !status
      ? "正在读取故障分析状态…"
      : status?.scanning
        ? `正在分析 ${status.scannedLines.toLocaleString()} / ${status.stableLines.toLocaleString()} 行`
        : status?.finished
          ? `已分析 ${status.stableLines.toLocaleString()} 行`
          : status?.coverage.origin === "adb-live"
            ? provisional > 0
              ? `已追上当前日志 · ${provisional.toLocaleString()} 项待定稿`
              : "已追上当前日志 · 持续监听"
            : "等待日志分析";
  const liveAnalysisNote =
    status?.coverage.origin === "adb-live" && !status.finished
      ? "实时抓取期间持续增量分析；停止抓取后仅定稿尾部未闭合事件"
      : null;
  const coverageLabel = status
    ? `${scanLabel} · ${coverageDescription(status.coverage)}${
        liveAnalysisNote ? ` · ${liveAnalysisNote}` : ""
      }`
    : scanLabel;

  const toggle = (
    <button
      ref={toggleRef}
      type="button"
      className="lf-problems-toggle"
      aria-expanded={visibleOpen}
      aria-controls="lf-problems-panel"
      aria-label={`Problems，${badge}`}
      onClick={() => setPanelOpen(!panelOpen)}
    >
      <AlertTriangle aria-hidden="true" />
      <span>Problems</span>
      <span className="lf-problems-badge">{badge}</span>
      {visibleOpen ? <ChevronDown aria-hidden="true" /> : <ChevronUp aria-hidden="true" />}
    </button>
  );

  if (!visibleOpen) {
    return (
      <div className="lf-problems-collapsed" data-layout-collapsed={layoutCollapsed || undefined}>
        {toggle}
        <span className="lf-problems-coverage">{coverageLabel}</span>
      </div>
    );
  }

  const occurrence = detail?.occurrence ?? null;
  const selectedGroup = groupPage?.items.find((group) => group.id === selectedGroupId) ?? null;
  const selectedKind = kindFilters.length === 1 ? kindFilters[0] : null;

  return (
    <section
      id="lf-problems-panel"
      className="lf-problems-dock"
      style={{ height: panelHeight }}
      role="region"
      aria-label="故障调查工作台"
    >
      <div
        className="lf-problems-separator"
        role="separator"
        aria-label="调整 Problems 面板高度"
        aria-orientation="horizontal"
        aria-valuemin={MIN_PANEL_HEIGHT}
        aria-valuemax={panelMaximum}
        aria-valuenow={panelHeight}
        tabIndex={0}
        onKeyDown={handleSeparatorKeyDown}
        onPointerDown={startPanelResize}
      />
      <header className="lf-problems-header">
        {toggle}
        <div className="lf-problems-coverage">{coverageLabel}</div>
        {hasNewResults ? (
          <button type="button" className="lf-problems-refresh" onClick={onRefresh}>
            <RefreshCw aria-hidden="true" />
            有新结果，刷新
          </button>
        ) : null}
      </header>

      {statusError ? (
        <div className="lf-problems-limit-note" role="alert">
          故障分析状态读取失败：{statusError}
          {onRetryStatus ? (
            <button type="button" aria-label="重试故障分析状态" onClick={onRetryStatus}>
              重试
            </button>
          ) : null}
        </div>
      ) : null}

      {stats?.correlationLimited ? (
        <div className="lf-problems-limit-note" role="status">
          部分晚到关联证据可能未保留（已淘汰 {stats.droppedRecentObservationCount.toLocaleString()}{" "}
          条紧凑观察引用）
        </div>
      ) : null}

      {stats?.limited ? (
        <div className="lf-problems-limit-note" role="status">
          容量限制：检出 {stats.observedOccurrenceCount.toLocaleString()} 项，可展开{" "}
          {stats.storedOccurrenceCount.toLocaleString()} 项，未保存{" "}
          {stats.droppedOccurrenceCount.toLocaleString()} 项。
        </div>
      ) : null}

      <div className="lf-problems-toolbar">
        <div className="lf-problems-kind-filters" role="group" aria-label="故障分类">
          <button
            type="button"
            aria-label="全部故障"
            aria-pressed={selectedKind == null}
            onClick={() => {
              if (onSetKindFilter) onSetKindFilter(null);
              else setKindFilters([]);
            }}
          >
            全部
          </button>
          {PROBLEM_KIND_FILTERS.map(({ kind, label }) => (
            <button
              type="button"
              key={kind}
              title={problemKindLabel(kind)}
              aria-label={`仅看 ${problemKindLabel(kind)}`}
              aria-pressed={selectedKind === kind}
              onClick={() => {
                if (onSetKindFilter) onSetKindFilter(kind);
                else setKindFilters([kind]);
              }}
            >
              {label}
            </button>
          ))}
        </div>
        <label className="lf-problems-sort">
          <span>排序</span>
          <select
            aria-label="分组排序"
            value={sort}
            onChange={(event) => {
              const nextSort = event.currentTarget.value as ProblemsSort;
              if (onSetSort) onSetSort(nextSort);
              else setSort(nextSort);
            }}
          >
            <option value="last-seen-desc">最近发生</option>
            <option value="count-desc">重复次数</option>
          </select>
        </label>
      </div>

      <div className="lf-problems-columns">
        <div className="lf-problems-pane lf-problems-groups">
          <h2>故障分组</h2>
          {groupPageError === "snapshot-expired" ? (
            <p className="lf-problems-state">结果快照已过期；当前内容已保留，请手动刷新。</p>
          ) : groupPageError ? (
            <p className="lf-problems-state" role="alert">
              读取故障分组失败：{groupPageError}
              {onRetryGroups ? (
                <button type="button" aria-label="重试故障分组" onClick={onRetryGroups}>
                  重试
                </button>
              ) : null}
            </p>
          ) : null}
          {groupPage && groupPage.items.length > 0 ? (
            <ProblemListbox
              label="故障分组"
              idPrefix="lf-problem-group"
              datasetKey={groupPage.snapshotHandle}
              items={groupPage.items}
              total={groupPage.total}
              selectedId={selectedGroupId}
              loading={groupLoading}
              loadFailed={groupPageError != null}
              itemHeight={GROUP_LIST_ITEM_HEIGHT}
              getId={(group) => group.id}
              onReachEnd={groupPage.nextCursor == null ? undefined : onLoadMoreGroups}
              onChoose={(group) => {
                if (onSelectGroup) onSelectGroup(group.id);
                else selectGroup(group.id);
              }}
            >
              {(group) => {
                const kindLabel = problemKindLabel(group.kind);
                const signature = group.signatureSummary.trim() || kindLabel;
                const process = group.processSummary.trim();
                return (
                  <>
                    <span
                      className="lf-problems-item-title"
                      title={group.signatureSummaryTruncated ? group.signatureSummary : undefined}
                    >
                      {signature}
                    </span>
                    <span title={group.processSummaryTruncated ? group.processSummary : undefined}>
                      {process ? `${process} · ${kindLabel}` : kindLabel}
                      {" · "}
                      {group.observedOccurrenceCount.toLocaleString()} 次
                      {group.storedOccurrenceCount < group.observedOccurrenceCount
                        ? `（可展开 ${group.storedOccurrenceCount.toLocaleString()}）`
                        : ""}
                    </span>
                    <span>
                      {group.firstTimestamp ?? `第 ${group.firstLine.toLocaleString()} 行`}
                      {" → "}
                      {group.lastTimestamp ?? `第 ${group.lastLine.toLocaleString()} 行`}
                    </span>
                  </>
                );
              }}
            </ProblemListbox>
          ) : (
            <p className="lf-problems-state">
              {groupLoading
                ? "正在读取首批故障分组…"
                : groupPageError
                  ? "当前没有可展示的故障分组。"
                  : status?.finished
                    ? "在已捕获范围内未检测到可展示的故障事件。"
                    : provisional > 0
                      ? `当前分析共有 ${provisional.toLocaleString()} 项待定稿，仍在等待晚到关联证据；尾部未闭合事件会在停止抓取后定稿，符合当前分类且保留详情的事件届时可展开。`
                      : "在已捕获范围内暂未检测到可展示的故障事件。"}
            </p>
          )}
          {groupPage?.nextCursor != null ? (
            <button
              type="button"
              className="lf-problems-refresh"
              disabled={groupLoading}
              onClick={onLoadMoreGroups}
            >
              {groupLoading ? "正在加载…" : "加载更多分组"}
            </button>
          ) : null}
        </div>

        <div className="lf-problems-pane lf-problems-occurrences">
          <h2>发生记录</h2>
          {occurrencePageError === "snapshot-expired" ? (
            <p className="lf-problems-state">发生记录快照已过期；请重新选择分组。</p>
          ) : occurrencePageError ? (
            <p className="lf-problems-state" role="alert">
              读取发生记录失败：{occurrencePageError}
              {onRetryOccurrences ? (
                <button type="button" aria-label="重试发生记录" onClick={onRetryOccurrences}>
                  重试
                </button>
              ) : null}
            </p>
          ) : null}
          {occurrencePage && occurrencePage.items.length > 0 ? (
            <ProblemListbox
              label="发生记录"
              idPrefix="lf-problem-event"
              datasetKey={occurrencePage.snapshotHandle}
              items={occurrencePage.items}
              total={occurrencePage.total}
              selectedId={selectedEventId}
              loading={occurrenceLoading}
              loadFailed={occurrencePageError != null}
              getId={(item) => item.eventId}
              onReachEnd={occurrencePage.nextCursor == null ? undefined : onLoadMoreOccurrences}
              onChoose={(item) => {
                if (onSelectOccurrence) onSelectOccurrence(item.eventId);
                else selectEvent(item.eventId);
              }}
            >
              {(item) => (
                <>
                  <span className="lf-problems-item-title">
                    {item.timestamp ?? `第 ${item.anchorLine.toLocaleString()} 行`}
                  </span>
                  <span>
                    事件 #{item.eventId.toLocaleString()}
                    {item.pid == null ? "" : ` · PID ${item.pid.toLocaleString()}`} · 锚点 第{" "}
                    {item.anchorLine.toLocaleString()} 行
                  </span>
                  <span>
                    范围 {item.startLine.toLocaleString()}–{item.endLine.toLocaleString()}
                  </span>
                </>
              )}
            </ProblemListbox>
          ) : (
            <p className="lf-problems-state">
              {selectedGroupId == null
                ? "选择一个故障分组查看发生记录。"
                : occurrenceLoading
                  ? "正在读取发生记录…"
                  : occurrencePageError
                    ? "当前没有可展示的发生记录。"
                    : "该分组没有可展示的发生记录。"}
            </p>
          )}
          {occurrencePage?.nextCursor != null ? (
            <button
              type="button"
              className="lf-problems-refresh"
              disabled={occurrenceLoading}
              onClick={onLoadMoreOccurrences}
            >
              {occurrenceLoading ? "正在加载…" : "加载更多发生记录"}
            </button>
          ) : null}
        </div>

        <div
          className="lf-problems-pane lf-problems-detail"
          role="region"
          aria-label="事件详情"
        >
          {detailError ? (
            <p className="lf-problems-state" role="alert">
              读取事件详情失败：{detailError}
              {onRetryDetail ? (
                <button type="button" aria-label="重试事件详情" onClick={onRetryDetail}>
                  重试
                </button>
              ) : null}
            </p>
          ) : null}
          {detail && occurrence ? (
            <>
              <div className="lf-problems-detail-heading">
                <div>
                  <h2>{problemKindLabel(occurrence.kind)}</h2>
                  <p>
                    事件 #{occurrence.eventId.toLocaleString()} · 第{" "}
                    {occurrence.startLine.toLocaleString()}–{occurrence.endLine.toLocaleString()} 行
                    {occurrence.pid == null ? "" : ` · PID ${occurrence.pid}`}
                  </p>
                </div>
                <div className="lf-problems-actions">
                  <button type="button" onClick={() => onLocateOccurrence?.(occurrence)}>
                    <MapPin aria-hidden="true" />
                    定位事件
                  </button>
                  <button type="button" onClick={() => onOpenContext?.(occurrence)}>
                    <Eye aria-hidden="true" />
                    查看上下文
                  </button>
                  <button type="button" onClick={() => onExportOccurrence?.(occurrence)}>
                    <Download aria-hidden="true" />
                    导出原始日志
                  </button>
                </div>
              </div>
              {selectedGroup ? (
                <div className="lf-problems-group-note">
                  <span
                    title={
                      selectedGroup.signatureSummaryTruncated
                        ? selectedGroup.signatureSummary
                        : undefined
                    }
                  >
                    {selectedGroup.signatureSummary.trim() || problemKindLabel(selectedGroup.kind)}
                  </span>
                  {selectedGroup.processSummary.trim() ? (
                    <span
                      title={
                        selectedGroup.processSummaryTruncated
                          ? selectedGroup.processSummary
                          : undefined
                      }
                    >
                      进程 {selectedGroup.processSummary}
                    </span>
                  ) : null}
                  <span className="lf-problems-fingerprint" title={selectedGroup.fingerprint}>
                    指纹 {selectedGroup.fingerprint}
                  </span>
                  <span>同组仅表示事件指纹相同，不代表根因相同。</span>
                </div>
              ) : null}
            </>
          ) : (
            <p className="lf-problems-state">
              {selectedEventId == null
                ? "选择一次发生记录查看事件信息与上下文。"
                : detailLoading
                  ? "正在读取事件详情…"
                  : detailError
                    ? "当前没有可展示的事件详情。"
                    : "选择的事件没有可展示详情。"}
            </p>
          )}
        </div>
      </div>

      <div className="sr-only" aria-live="polite">
        {status?.finished
          ? `故障分析完成，检出 ${detected} 项`
          : stats?.limited
            ? `故障索引达到限制，可展开 ${expandable} 项`
            : hasNewResults
              ? "故障分析有新结果"
              : ""}
      </div>
    </section>
  );
}
