import { useEffect, useRef } from "react";
import {
  AlertTriangle,
  ChevronDown,
  ChevronUp,
  Download,
  Eye,
  MapPin,
  RefreshCw,
} from "lucide-react";
import { problemFactLabel, problemKindLabel } from "@/lib/problems";
import { problemHints } from "@/lib/problemHints";
import { useProblems } from "@/store/problems";
import type { ProblemOccurrence } from "@/types";

interface ProblemsDockProps {
  onOpen?: () => void;
  onRefresh?: () => void;
  onSelectGroup?: (groupId: number) => void;
  onSelectOccurrence?: (eventId: number) => void;
  onLocateFact?: (lineNo: number) => void;
  onLocateOccurrence?: (occurrence: ProblemOccurrence) => void;
  onOpenContext?: (occurrence: ProblemOccurrence) => void;
  onExportOccurrence?: (occurrence: ProblemOccurrence) => void;
}

export function ProblemsDock({
  onOpen,
  onRefresh,
  onSelectGroup,
  onSelectOccurrence,
  onLocateFact,
  onLocateOccurrence,
  onOpenContext,
  onExportOccurrence,
}: ProblemsDockProps) {
  const panelOpen = useProblems((state) => state.panelOpen);
  const setPanelOpen = useProblems((state) => state.setPanelOpen);
  const panelHeight = useProblems((state) => state.panelHeight);
  const status = useProblems((state) => state.status);
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
  const openRequestArmed = useRef(true);

  useEffect(() => {
    if (!panelOpen) {
      openRequestArmed.current = true;
      return;
    }
    if (openRequestArmed.current) {
      openRequestArmed.current = false;
      onOpen?.();
    }
  }, [onOpen, panelOpen]);

  const stats = status?.stats;
  const detected = stats?.observedOccurrenceCount ?? 0;
  const expandable = stats?.storedOccurrenceCount ?? 0;
  const badge = stats?.limited
    ? `检出 ${detected.toLocaleString()} · 可展开 ${expandable.toLocaleString()}`
    : detected.toLocaleString();

  const toggle = (
    <button
      type="button"
      className="lf-problems-toggle"
      aria-expanded={panelOpen}
      aria-controls="lf-problems-panel"
      aria-label={`Problems，${badge}`}
      onClick={() => setPanelOpen(!panelOpen)}
    >
      <AlertTriangle aria-hidden="true" />
      <span>Problems</span>
      <span className="lf-problems-badge">{badge}</span>
      {panelOpen ? <ChevronDown aria-hidden="true" /> : <ChevronUp aria-hidden="true" />}
    </button>
  );

  if (!panelOpen) {
    return <div className="lf-problems-collapsed">{toggle}</div>;
  }

  const occurrence = detail?.occurrence ?? null;

  return (
    <section
      id="lf-problems-panel"
      className="lf-problems-dock"
      style={{ height: panelHeight }}
      role="region"
      aria-label="故障调查工作台"
    >
      <header className="lf-problems-header">
        {toggle}
        <div className="lf-problems-coverage">
          {status?.scanning
            ? `正在分析 ${status.scannedLines.toLocaleString()} / ${status.stableLines.toLocaleString()} 行`
            : status?.finished
              ? `已分析 ${status.stableLines.toLocaleString()} 行`
              : "等待日志分析"}
        </div>
        {hasNewResults ? (
          <button type="button" className="lf-problems-refresh" onClick={onRefresh}>
            <RefreshCw aria-hidden="true" />
            有新结果，刷新
          </button>
        ) : null}
      </header>

      {stats?.correlationLimited ? (
        <div className="lf-problems-limit-note" role="status">
          部分晚到关联证据可能未保留（已淘汰{" "}
          {stats.droppedRecentObservationCount.toLocaleString()} 条紧凑观察引用）
        </div>
      ) : null}

      <div className="lf-problems-columns">
        <div className="lf-problems-pane lf-problems-groups">
          <h2>故障分组</h2>
          {groupPageError === "snapshot-expired" ? (
            <p className="lf-problems-state">结果快照已过期；当前内容已保留，请手动刷新。</p>
          ) : null}
          {groupPage ? (
            <div
              className="lf-problems-list"
              role="listbox"
              aria-label="故障分组"
              aria-activedescendant={
                selectedGroupId == null ? undefined : `lf-problem-group-${selectedGroupId}`
              }
            >
              {groupPage.items.map((group, index) => (
                <button
                  id={`lf-problem-group-${group.id}`}
                  key={group.id}
                  type="button"
                  role="option"
                  aria-selected={selectedGroupId === group.id}
                  aria-posinset={index + 1}
                  aria-setsize={groupPage.total}
                  className="lf-problems-item"
                  onClick={() => {
                    selectGroup(group.id);
                    onSelectGroup?.(group.id);
                  }}
                >
                  <span className="lf-problems-item-title">{problemKindLabel(group.kind)}</span>
                  <span>{group.storedOccurrenceCount.toLocaleString()} 次</span>
                  <span>
                    第 {group.firstLine.toLocaleString()}–{group.lastLine.toLocaleString()} 行
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <p className="lf-problems-state">
              {status?.finished
                ? "在已捕获范围内未检测到可展示的故障事件。"
                : "展开后正在读取首批故障分组…"}
            </p>
          )}
        </div>

        <div className="lf-problems-pane lf-problems-occurrences">
          <h2>发生记录</h2>
          {occurrencePageError === "snapshot-expired" ? (
            <p className="lf-problems-state">发生记录快照已过期；请重新选择分组。</p>
          ) : null}
          {occurrencePage ? (
            <div
              className="lf-problems-list"
              role="listbox"
              aria-label="发生记录"
              aria-activedescendant={
                selectedEventId == null ? undefined : `lf-problem-event-${selectedEventId}`
              }
            >
              {occurrencePage.items.map((item, index) => (
                <button
                  id={`lf-problem-event-${item.eventId}`}
                  key={item.eventId}
                  type="button"
                  role="option"
                  aria-selected={selectedEventId === item.eventId}
                  aria-posinset={index + 1}
                  aria-setsize={occurrencePage.total}
                  className="lf-problems-item"
                  onClick={() => {
                    selectEvent(item.eventId);
                    onSelectOccurrence?.(item.eventId);
                  }}
                >
                  <span className="lf-problems-item-title">
                    {item.timestamp ?? `第 ${item.anchorLine.toLocaleString()} 行`}
                  </span>
                  <span>锚点 第 {item.anchorLine.toLocaleString()} 行</span>
                  <span>
                    范围 {item.startLine.toLocaleString()}–{item.endLine.toLocaleString()}
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <p className="lf-problems-state">
              {selectedGroupId == null ? "选择一个故障分组查看发生记录。" : "正在读取发生记录…"}
            </p>
          )}
        </div>

        <div className="lf-problems-pane lf-problems-detail">
          {detail && occurrence ? (
            <>
              <div className="lf-problems-detail-heading">
                <div>
                  <h2>{problemKindLabel(occurrence.kind)}</h2>
                  <p>
                    第 {occurrence.startLine.toLocaleString()}–
                    {occurrence.endLine.toLocaleString()} 行
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

              <section className="lf-problems-facts">
                <h3>检测到的事实</h3>
                {detail.facts.map((fact, index) => (
                  <div className="lf-problems-fact" key={`${fact.sourceLine}-${fact.code}-${index}`}>
                    <span>{problemFactLabel(fact.code)}</span>
                    <button
                      type="button"
                      aria-label={`定位到第 ${fact.sourceLine} 行`}
                      onClick={() => onLocateFact?.(fact.sourceLine)}
                    >
                      第 {fact.sourceLine.toLocaleString()} 行
                    </button>
                  </div>
                ))}
                {detail.observationTotal > detail.facts.length ? (
                  <p className="lf-problems-truncated">
                    仅展示 {detail.facts.length}/{detail.observationTotal} 条关键证据，可查看事件范围
                  </p>
                ) : null}
              </section>

              <section className="lf-problems-hints">
                <h3>排查提示（非结论）</h3>
                <ul>
                  {problemHints(occurrence.kind).map((hint) => (
                    <li key={hint}>{hint}</li>
                  ))}
                </ul>
              </section>
            </>
          ) : (
            <p className="lf-problems-state">
              {selectedEventId == null ? "选择一次发生记录查看事实与上下文。" : "正在读取事件详情…"}
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
