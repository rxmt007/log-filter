import { FILTER_FIELD_DEFINITIONS, LOG_LEVELS } from "@/lib/filterDefinitions";
import { ALL_LEVELS } from "@/store/session";
import type { FilterSpec } from "@/types";

function withModifiers(summary: string, modifiers: string[]) {
  return modifiers.length ? `${summary}（${modifiers.join("、")}）` : summary;
}

export function summarizeActiveFilters(filter: FilterSpec): string[] {
  const summaries: string[] = [];

  if (filter.levels !== ALL_LEVELS) {
    const levels = LOG_LEVELS.filter(({ bit }) => (filter.levels & bit) !== 0).map(
      ({ label }) => label,
    );
    summaries.push(`级别：${levels.length ? levels.join(" / ") : "无"}`);
  }

  if (filter.markedOnly) summaries.push("仅标记");

  for (const field of FILTER_FIELD_DEFINITIONS) {
    const value = filter[field.key];
    const pattern = value.pattern.trim();
    if (!value.enabled || !pattern) continue;
    summaries.push(withModifiers(`${field.label}：${pattern}`, value.regex ? ["正则"] : []));
  }

  filter.highlights.forEach((rule, index) => {
    const pattern = rule.pattern.trim();
    if (!rule.enabled || !pattern) return;
    summaries.push(
      withModifiers(
        `高亮 ${index + 1}：${pattern}`,
        [rule.regex ? "正则" : "", rule.caseSensitive ? "区分大小写" : ""].filter(Boolean),
      ),
    );
  });

  return summaries;
}
