import type { StreamAppend } from "@/types";

export interface StreamAppendTimer {
  setTimeout(callback: () => void, delayMs: number): number;
  clearTimeout(handle: number): void;
}

interface StreamAppendBatcherOptions {
  delayMs?: number;
  onFlush: (append: StreamAppend) => void;
  timer?: StreamAppendTimer;
}

const DEFAULT_DELAY_MS = 75;

function defaultTimer(): StreamAppendTimer {
  return {
    setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
    clearTimeout: (handle) => window.clearTimeout(handle),
  };
}

export function createStreamAppendBatcher({
  delayMs = DEFAULT_DELAY_MS,
  onFlush,
  timer = defaultTimer(),
}: StreamAppendBatcherOptions) {
  let pending: StreamAppend | null = null;
  let timeout: number | null = null;

  const flush = () => {
    timeout = null;
    const next = pending;
    pending = null;
    if (next) onFlush(next);
  };

  return {
    push(append: StreamAppend) {
      pending = pending
        ? {
            ...append,
            appendedBytes: pending.appendedBytes + append.appendedBytes,
          }
        : append;
      if (timeout == null) {
        timeout = timer.setTimeout(flush, delayMs);
      }
    },
    dispose() {
      if (timeout != null) {
        timer.clearTimeout(timeout);
        timeout = null;
      }
      pending = null;
    },
  };
}
