import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { ColorSelect } from "@/components/ui/dropdown";
import { FILTER_FIELD_DEFINITIONS } from "@/lib/filterDefinitions";
import { summarizeActiveFilters } from "@/lib/filterSummary";
import { useSession } from "@/store/session";

const HIGHLIGHT_COLORS = ["yellow", "green", "blue", "purple"] as const;
const FILTER_CONTENT_ID = "lf-filter-panel-content";
const EMPTY_SUMMARY = "未启用过滤或高亮条件";

export function FilterBar() {
  const [collapsed, setCollapsed] = useState(false);
  const filter = useSession((state) => state.filter);
  const setFilterField = useSession((state) => state.setFilterField);
  const setHighlightRule = useSession((state) => state.setHighlightRule);
  const activeSummaries = useMemo(() => summarizeActiveFilters(filter), [filter]);
  const summaryText = activeSummaries.length ? activeSummaries.join(" · ") : EMPTY_SUMMARY;
  const configuredSummary = `当前配置：${summaryText}`;

  return (
    <div className="lf-filter-bar" data-collapsed={collapsed}>
      <div className="lf-filter-title">
        <button
          aria-controls={FILTER_CONTENT_ID}
          aria-expanded={!collapsed}
          aria-label={collapsed ? `展开过滤条件。${configuredSummary}` : "折叠过滤条件"}
          className="lf-filter-toggle"
          title={collapsed ? configuredSummary : "折叠过滤条件"}
          type="button"
          onClick={() => setCollapsed((value) => !value)}
        >
          {collapsed ? <ChevronRight /> : <ChevronDown />}
          <span className="lf-filter-heading">过滤条件</span>
          {collapsed ? (
            <span className="lf-filter-summary" data-testid="filter-summary">
              {activeSummaries.length ? (
                <span className="lf-filter-summary-count">{activeSummaries.length} 项配置</span>
              ) : null}
              <span className="lf-filter-summary-text">{configuredSummary}</span>
            </span>
          ) : null}
        </button>
      </div>

      <div id={FILTER_CONTENT_ID} hidden={collapsed}>
        {!collapsed ? (
          <>
            <div className="lf-filter-fields">
              {FILTER_FIELD_DEFINITIONS.map((field) => {
                const value = filter[field.key];
                return (
                  <label className="lf-filter-field" data-enabled={value.enabled} key={field.key}>
                    <button
                      aria-label={`${field.label} 过滤`}
                      aria-pressed={value.enabled}
                      className="lf-switch"
                      data-active={value.enabled}
                      type="button"
                      onClick={(event) => {
                        event.preventDefault();
                        setFilterField(field.key, { enabled: !value.enabled });
                      }}
                    >
                      <span />
                    </button>
                    <span
                      className={field.badge === "-" ? "lf-badge lf-badge-exclude" : "lf-badge"}
                    >
                      {field.badge ?? ""}
                    </span>
                    <span className="lf-filter-label">{field.label}</span>
                    <input
                      value={value.pattern}
                      placeholder={field.placeholder}
                      onChange={(event) =>
                        setFilterField(field.key, { pattern: event.target.value })
                      }
                    />
                    {field.supportsRegex ? (
                      <button
                        aria-label={`${field.label} 正则`}
                        aria-pressed={value.regex}
                        className="lf-mini-toggle"
                        data-active={value.regex}
                        data-tooltip="Regex filter"
                        type="button"
                        onClick={(event) => {
                          event.preventDefault();
                          setFilterField(field.key, { regex: !value.regex });
                        }}
                      >
                        .*
                      </button>
                    ) : null}
                  </label>
                );
              })}
            </div>
            <div className="lf-highlight-fields">
              {filter.highlights.map((rule, index) => (
                <label
                  className="lf-filter-field lf-highlight-field"
                  data-enabled={rule.enabled}
                  key={index}
                >
                  <button
                    aria-label={`高亮 ${index + 1}`}
                    aria-pressed={rule.enabled}
                    className="lf-switch"
                    data-active={rule.enabled}
                    type="button"
                    onClick={(event) => {
                      event.preventDefault();
                      setHighlightRule(index, { enabled: !rule.enabled });
                    }}
                  >
                    <span />
                  </button>
                  <span className="lf-filter-label">高亮 {index + 1}</span>
                  <input
                    value={rule.pattern}
                    placeholder="keyword"
                    onChange={(event) => setHighlightRule(index, { pattern: event.target.value })}
                  />
                  <button
                    aria-label={`高亮 ${index + 1} 正则`}
                    aria-pressed={rule.regex}
                    className="lf-mini-toggle"
                    data-active={rule.regex}
                    data-tooltip="Regex highlight"
                    type="button"
                    onClick={(event) => {
                      event.preventDefault();
                      setHighlightRule(index, { regex: !rule.regex });
                    }}
                  >
                    .*
                  </button>
                  <button
                    aria-label={`高亮 ${index + 1} 大小写`}
                    aria-pressed={rule.caseSensitive}
                    className="lf-mini-toggle"
                    data-active={rule.caseSensitive}
                    data-tooltip="Case sensitive"
                    type="button"
                    onClick={(event) => {
                      event.preventDefault();
                      setHighlightRule(index, { caseSensitive: !rule.caseSensitive });
                    }}
                  >
                    Aa
                  </button>
                  <ColorSelect
                    value={rule.color}
                    options={HIGHLIGHT_COLORS.map((color) => ({ value: color, label: color }))}
                    onChange={(color) => setHighlightRule(index, { color })}
                  />
                  <span className="lf-highlight-color" data-color={rule.color} />
                </label>
              ))}
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
