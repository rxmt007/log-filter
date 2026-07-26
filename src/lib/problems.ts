import type {
  ProblemFactCode,
  ProblemBoundaryFlag,
  ProblemEvidenceFormat,
  ProblemKind,
  ProblemLineProvenance,
  ProblemObservationRole,
  ProblemOutcomeFlag,
  ProblemPage,
  ProblemsProgress,
  ProblemsStatus,
} from "@/types";

export function appendSnapshotPage<T>(
  current: ProblemPage<T>,
  incoming: ProblemPage<T>,
  keyOf: (item: T) => number,
): ProblemPage<T> {
  if (current.snapshotHandle !== incoming.snapshotHandle) {
    throw new Error("snapshot-mismatch");
  }
  if (current.revision !== incoming.revision) {
    throw new Error("snapshot-revision-mismatch");
  }

  const seen = new Set(current.items.map(keyOf));
  const items = current.items.slice();
  for (const item of incoming.items) {
    const key = keyOf(item);
    if (seen.has(key)) continue;
    seen.add(key);
    items.push(item);
  }
  return { ...incoming, items };
}

export function problemsStatusFromProgress(progress: ProblemsProgress): ProblemsStatus {
  return {
    analysisToken: {
      sessionGeneration: progress.sessionGeneration,
      analysisGeneration: progress.analysisGeneration,
    },
    scannedLines: progress.scannedLines,
    stableLines: progress.stableLines,
    scanning: !progress.done && progress.scannedLines < progress.stableLines,
    finished: progress.done,
    coverage: progress.coverage,
    stats: {
      observedOccurrenceCount: progress.observedOccurrenceCount,
      storedOccurrenceCount: progress.storedOccurrenceCount,
      droppedOccurrenceCount: progress.droppedOccurrenceCount,
      provisionalOccurrenceCount: progress.provisionalOccurrenceCount,
      storedGroupCount: progress.storedGroupCount,
      ungroupedDroppedOccurrenceCount: progress.ungroupedDroppedOccurrenceCount,
      droppedRecentObservationCount: progress.droppedRecentObservationCount,
      revision: progress.revision,
      limited: progress.limited,
      correlationLimited: progress.correlationLimited,
    },
  };
}

export const PROBLEM_FACT_CODES = [
  "java-uncaught-exception",
  "java-out-of-memory-error",
  "managed-crash-record",
  "anr-detected",
  "native-crash-detected",
  "signal-exit-detected",
  "process-started",
  "process-died",
  "process-restarted",
  "lmk-kill-issued",
  "kernel-oom-kill-issued",
  "kill-requested",
  "process-identity-recorded",
  "exception-type-recorded",
  "stack-frame-recorded",
  "anr-reason-recorded",
  "fatal-signal-recorded",
  "native-frame-recorded",
  "process-death-observed",
  "start-after-death-observed",
  "native-recovery-recorded",
  "supporting-evidence-recorded",
] as const satisfies readonly ProblemFactCode[];

const FACT_LABELS = {
  "java-uncaught-exception": "日志记录了未处理的 Java/Kotlin 异常",
  "java-out-of-memory-error": "日志记录了致命 OutOfMemoryError",
  "managed-crash-record": "日志出现符合 AOSP am_crash 格式的记录",
  "anr-detected": "系统日志记录了 ANR",
  "native-crash-detected": "日志记录了 native crash",
  "signal-exit-detected": "日志记录了进程 signal exit",
  "process-started": "系统日志记录了进程启动",
  "process-died": "系统日志记录了进程结束",
  "process-restarted": "同一进程身份在结束后再次启动",
  "lmk-kill-issued": "lmkd 记录已发出 kill",
  "kernel-oom-kill-issued": "kernel 记录 OOM kill",
  "kill-requested": "系统服务记录了 kill 请求",
  "process-identity-recorded": "日志记录了进程身份",
  "exception-type-recorded": "日志记录了异常类型",
  "stack-frame-recorded": "日志记录了 Java/Kotlin 栈帧",
  "anr-reason-recorded": "系统记录了 ANR Reason",
  "fatal-signal-recorded": "日志记录了 fatal signal",
  "native-frame-recorded": "日志记录了 native backtrace frame",
  "process-death-observed": "同一进程实例的结束得到日志佐证",
  "start-after-death-observed": "同一进程身份在结束后出现启动记录",
  "native-recovery-recorded": "日志字段明确标记 native 事件可恢复",
  "supporting-evidence-recorded": "日志记录了相关支持证据",
} satisfies Record<ProblemFactCode, string>;

const KIND_LABELS = {
  "java-crash": "Java/Kotlin 崩溃",
  "java-oom": "Java OOM",
  anr: "ANR",
  "native-crash": "Native Crash",
  "process-restart": "进程重启",
  "signal-exit": "Signal Exit",
  "lmk-kill": "LMK Kill",
  "kernel-oom-kill": "Kernel OOM Kill",
} satisfies Record<ProblemKind, string>;

const OUTCOME_LABELS = {
  "kill-requested": "系统服务记录了 kill 请求，但未证明已经执行",
  "kill-issued": "日志明确记录已发出 kill",
  "death-observed": "同一进程实例的结束得到日志佐证",
  "start-after-death-observed": "同一进程身份在结束后再次出现启动记录",
  "explicitly-recoverable": "日志字段明确标记该事件可恢复",
  conflict: "日志包含互相矛盾或无法自动解释的结局事实",
} satisfies Record<ProblemOutcomeFlag, string>;

const BOUNDARY_LABELS = {
  "truncated-by-input": "输入结束时事件仍处于开放状态",
  "truncated-by-limit": "检测器在安全上限处截断了事件证据",
  "observation-refs-truncated": "详情只物化了有界的关键证据引用",
  "observation-count-limited": "可采用的证据数量达到安全上限",
  "line-index-overflow": "源行号超出当前索引表示范围",
  "correlation-limited": "部分晚到关联证据可能未保留",
} satisfies Record<ProblemBoundaryFlag, string>;

const EVIDENCE_FORMAT_LABELS = {
  "aosp-text": "AOSP 文本",
  "event-log-shaped-text": "EventLog 格式文本",
  "tombstone-shaped-text": "tombstone 格式文本",
  "kernel-shaped-text": "kernel 格式文本",
} satisfies Record<ProblemEvidenceFormat, string>;

const ROLE_LABELS = {
  primary: "主证据",
  "process-identity": "进程身份",
  "exception-type": "异常类型",
  "stack-frame": "Java/Kotlin 栈帧",
  reason: "系统记录的 reason",
  signal: "fatal signal",
  "backtrace-frame": "native 栈帧",
  start: "启动记录",
  death: "结束记录",
  restart: "再次启动记录",
  "kill-request": "kill 请求",
  "kill-issued": "kill 已发出",
  supporting: "支持证据",
  recovery: "可恢复标记",
} satisfies Record<ProblemObservationRole, string>;

const PROVENANCE_LABELS = {
  unknown: "来源未证明",
  "inferred-main": "推断来源：main",
  "inferred-system": "推断来源：system",
  "inferred-events": "推断来源：events",
  "inferred-crash": "推断来源：crash",
  "inferred-radio": "推断来源：radio",
  "inferred-kernel": "推断来源：kernel",
  "known-main": "已证明来源：main",
  "known-system": "已证明来源：system",
  "known-events": "已证明来源：events",
  "known-crash": "已证明来源：crash",
  "known-radio": "已证明来源：radio",
  "known-kernel": "已证明来源：kernel",
} satisfies Record<ProblemLineProvenance, string>;

export function problemFactLabel(code: ProblemFactCode): string {
  return FACT_LABELS[code];
}

export function problemKindLabel(kind: ProblemKind): string {
  return KIND_LABELS[kind];
}

export function problemOutcomeLabel(flag: ProblemOutcomeFlag): string {
  return OUTCOME_LABELS[flag];
}

export function problemBoundaryLabel(flag: ProblemBoundaryFlag): string {
  return BOUNDARY_LABELS[flag];
}

export function problemEvidenceFormatLabel(format: ProblemEvidenceFormat): string {
  return EVIDENCE_FORMAT_LABELS[format];
}

export function problemObservationRoleLabel(role: ProblemObservationRole): string {
  return ROLE_LABELS[role];
}

export function problemProvenanceLabel(provenance: ProblemLineProvenance): string {
  return PROVENANCE_LABELS[provenance];
}
