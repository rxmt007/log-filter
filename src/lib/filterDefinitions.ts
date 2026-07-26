import { LEVEL_BITS } from "@/store/session";
import type { FilterSpec } from "@/types";

export const LOG_LEVELS = [
  { label: "V", bit: LEVEL_BITS.V, tooltip: "Verbose" },
  { label: "D", bit: LEVEL_BITS.D, tooltip: "Debug" },
  { label: "I", bit: LEVEL_BITS.I, tooltip: "Info" },
  { label: "W", bit: LEVEL_BITS.W, tooltip: "Warning" },
  { label: "E", bit: LEVEL_BITS.E, tooltip: "Error" },
  { label: "F", bit: LEVEL_BITS.F, tooltip: "Fatal" },
] as const;

type FilterFieldKey = keyof Omit<FilterSpec, "levels" | "markedOnly" | "highlights">;

interface FilterFieldDefinition {
  key: FilterFieldKey;
  label: string;
  badge?: "+" | "-";
  placeholder: string;
  supportsRegex?: boolean;
}

export const FILTER_FIELD_DEFINITIONS: ReadonlyArray<FilterFieldDefinition> = [
  {
    key: "tagInclude",
    label: "Tag 包含",
    badge: "+",
    placeholder: "*Manager",
    supportsRegex: true,
  },
  {
    key: "tagExclude",
    label: "Tag 屏蔽",
    badge: "-",
    placeholder: "chatty|GC",
    supportsRegex: true,
  },
  { key: "pid", label: "PID", placeholder: "12043|146" },
  { key: "tid", label: "TID", placeholder: "179|12095" },
  {
    key: "wordInclude",
    label: "内容包含",
    badge: "+",
    placeholder: "network|支付",
    supportsRegex: true,
  },
  {
    key: "wordExclude",
    label: "内容屏蔽",
    badge: "-",
    placeholder: "heartbeat",
    supportsRegex: true,
  },
];
