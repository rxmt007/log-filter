export type HighlightToken =
  | { text: string; kind: "text" }
  | { text: string; kind: "highlight"; color: string }
  | { text: string; kind: "search" };

export interface HighlightRule {
  query: string;
  color: string;
  regex: boolean;
  caseSensitive: boolean;
}

export interface SearchHighlightRule {
  query: string;
  regex: boolean;
  caseSensitive: boolean;
}

interface Range {
  start: number;
  end: number;
  kind: "highlight" | "search";
  color?: string;
}

export function splitHighlightTokens(
  text: string,
  options: {
    highlights?: HighlightRule[];
    search?: SearchHighlightRule;
  },
): HighlightToken[] {
  const ranges = [
    ...(options.highlights ?? []).flatMap((rule) =>
      collectRanges(text, rule.query, rule.regex, rule.caseSensitive).map((range) => ({
        ...range,
        kind: "highlight" as const,
        color: rule.color,
      })),
    ),
    ...(options.search
      ? collectRanges(
          text,
          options.search.query,
          options.search.regex,
          options.search.caseSensitive,
        ).map((range) => ({ ...range, kind: "search" as const }))
      : []),
  ].filter((range) => range.end > range.start);

  if (ranges.length === 0) return [{ text, kind: "text" }];

  const boundaries = [
    ...new Set([0, text.length, ...ranges.flatMap((range) => [range.start, range.end])]),
  ]
    .filter((boundary) => boundary >= 0 && boundary <= text.length)
    .sort((left, right) => left - right);

  const tokens: HighlightToken[] = [];
  for (let i = 0; i < boundaries.length - 1; i += 1) {
    const start = boundaries[i];
    const end = boundaries[i + 1];
    if (start === end) continue;

    const active = ranges.filter((range) => range.start <= start && end <= range.end);
    const chosen =
      active.find((range) => range.kind === "search") ??
      active.sort((left, right) => left.start - right.start)[0];

    if (!chosen) {
      tokens.push({ text: text.slice(start, end), kind: "text" });
    } else if (chosen.kind === "search") {
      tokens.push({ text: text.slice(start, end), kind: "search" });
    } else {
      tokens.push({
        text: text.slice(start, end),
        kind: "highlight",
        color: chosen.color ?? "yellow",
      });
    }
  }

  return mergeAdjacentTokens(tokens);
}

function collectRanges(
  text: string,
  query: string,
  regex: boolean,
  caseSensitive: boolean,
): Array<Omit<Range, "kind" | "color">> {
  if (!query) return [];
  if (regex) {
    try {
      const re = new RegExp(query, caseSensitive ? "g" : "gi");
      const ranges: Array<Omit<Range, "kind" | "color">> = [];
      for (const match of text.matchAll(re)) {
        const start = match.index ?? 0;
        const hit = match[0];
        if (!hit) continue;
        ranges.push({ start, end: start + hit.length });
      }
      return ranges;
    } catch {
      return [];
    }
  }

  const haystack = caseSensitive ? text : text.toLowerCase();
  const needle = caseSensitive ? query : query.toLowerCase();
  const ranges: Array<Omit<Range, "kind" | "color">> = [];
  let cursor = 0;
  while (cursor < haystack.length) {
    const index = haystack.indexOf(needle, cursor);
    if (index < 0) break;
    ranges.push({ start: index, end: index + query.length });
    cursor = index + Math.max(1, query.length);
  }
  return ranges;
}

function mergeAdjacentTokens(tokens: HighlightToken[]): HighlightToken[] {
  const merged: HighlightToken[] = [];
  for (const token of tokens) {
    const previous = merged[merged.length - 1];
    if (
      previous &&
      previous.kind === token.kind &&
      (token.kind !== "highlight" ||
        (previous.kind === "highlight" && previous.color === token.color))
    ) {
      previous.text += token.text;
    } else if (token.text) {
      merged.push(token);
    }
  }
  return merged;
}
