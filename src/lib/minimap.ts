export const MINIMAP_BUCKETS = 180;
export const VIEWPORT_MIN_HEIGHT = 22;

export interface MinimapRect {
  top: number;
  height: number;
}

export function bucketRanges(buckets: number[]) {
  const sorted = [...new Set(buckets)].sort((a, b) => a - b);
  const ranges: Array<{ start: number; end: number }> = [];
  for (const bucket of sorted) {
    const last = ranges[ranges.length - 1];
    if (last && bucket <= last.end + 1) {
      last.end = bucket;
    } else {
      ranges.push({ start: bucket, end: bucket });
    }
  }
  return ranges;
}

/** 错误刻度样式:位置来自桶序号,透明度与桶内错误密度成正比(密度 = count / 每桶行数)。 */
export function errorTickStyle(
  entry: { bucket: number; count: number },
  totalRows: number,
  buckets = MINIMAP_BUCKETS,
): { top: string; height: string; opacity: number } {
  const rowsPerBucket = Math.max(1, totalRows / buckets);
  const density = Math.min(1, entry.count / rowsPerBucket);
  const opacity = Math.min(1, 0.16 + 0.84 * Math.min(1, density * 6));
  const top = (entry.bucket / buckets) * 100;
  const height = Math.max(0.55, 100 / buckets);
  return { top: `${formatPercent(top)}%`, height: `${formatPercent(height)}%`, opacity };
}

export function rangeStyle(range: { start: number; end: number }, buckets = MINIMAP_BUCKETS) {
  const start = (range.start / buckets) * 100;
  const end = ((range.end + 1) / buckets) * 100;
  return {
    top: `${formatPercent(start)}%`,
    height: `${formatPercent(Math.max(0.7, end - start))}%`,
  };
}

export function viewportHeightPx(rect: MinimapRect, resultCount: number, visibleRows: number) {
  if (rect.height <= 0 || resultCount <= 0) return 0;
  const proportional = rect.height * Math.min(1, normalizeVisibleRows(visibleRows) / resultCount);
  return Math.min(rect.height, Math.max(VIEWPORT_MIN_HEIGHT, proportional));
}

export function maxViewportTopPx(rect: MinimapRect, resultCount: number, visibleRows: number) {
  return Math.max(0, rect.height - viewportHeightPx(rect, resultCount, visibleRows));
}

export function maxViewportStartIndex(resultCount: number, visibleRows: number) {
  return Math.max(0, resultCount - normalizeVisibleRows(visibleRows));
}

export function indexToViewportTopPx(
  index: number,
  rect: MinimapRect,
  resultCount: number,
  visibleRows: number,
) {
  const maxIndex = maxViewportStartIndex(resultCount, visibleRows);
  if (maxIndex <= 0) return 0;
  const maxTop = maxViewportTopPx(rect, resultCount, visibleRows);
  return clamp((index / maxIndex) * maxTop, 0, maxTop);
}

export function viewportTopPxToResultIndex(
  topPx: number,
  rect: MinimapRect,
  resultCount: number,
  visibleRows: number,
) {
  if (resultCount <= 0 || rect.height <= 0) return null;
  const maxIndex = maxViewportStartIndex(resultCount, visibleRows);
  if (maxIndex <= 0) return 0;
  const maxTop = maxViewportTopPx(rect, resultCount, visibleRows);
  const frac = maxTop > 0 ? clamp(topPx / maxTop, 0, 1) : 0;
  return clamp(Math.round(frac * maxIndex), 0, maxIndex);
}

export function pointerToResultIndex(
  clientY: number,
  rect: MinimapRect,
  resultCount: number,
  visibleRows: number,
  grabOffset: number,
) {
  const topPx = clientY - rect.top - grabOffset;
  return viewportTopPxToResultIndex(topPx, rect, resultCount, visibleRows);
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function normalizeVisibleRows(visibleRows: number) {
  return Math.max(0, Math.ceil(visibleRows));
}

function formatPercent(value: number) {
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(4)));
}
