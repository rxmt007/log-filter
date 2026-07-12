import { readFileSync } from "node:fs";

const files = {
  session: readFileSync("src/store/session.ts", "utf8"),
  minimap: readFileSync("src/components/Minimap.tsx", "utf8"),
  table: readFileSync("src/components/LogTable.tsx", "utf8"),
  toolbar: readFileSync("src/components/Toolbar.tsx", "utf8"),
  css: readFileSync("src/index.css", "utf8"),
};

function expectContract(name, condition) {
  if (!condition) {
    throw new Error(`Missing interaction contract: ${name}`);
  }
}

function effectBodyWithDependency(source, dependency) {
  const dependencyEnd = source.indexOf(`}, [${dependency}]);`);
  if (dependencyEnd < 0) return "";
  const effectStart = source.lastIndexOf("useEffect(() => {", dependencyEnd);
  if (effectStart < 0) return "";
  return source.slice(effectStart, dependencyEnd);
}

const filterRevisionEffect = effectBodyWithDependency(files.table, "filterResultRevision");

expectContract("store has ScrollRequest", files.session.includes("ScrollRequest"));
expectContract("store tracks viewportResultIndex", files.session.includes("viewportResultIndex"));
expectContract("store exposes navigateToResultIndex", files.session.includes("navigateToResultIndex"));
expectContract("minimap stores grab offset", files.minimap.includes("grabOffsetRef"));
expectContract("minimap uses viewportResultIndex", files.minimap.includes("viewportResultIndex"));
expectContract("minimap renders marked-only as continuous content", files.minimap.includes("markedOnlyContinuous"));
expectContract("table consumes scrollRequest", files.table.includes("scrollRequest"));
expectContract("table updates viewportResultIndex", files.table.includes("setViewportResultIndex"));
expectContract("table no longer resets on bookmarkRevision", !files.table.includes("bookmarkRevision"));
expectContract("table tracks cache freshness by epoch", files.table.includes("filledEpoch"));
expectContract(
  "filter refresh keeps visible cache while refetching",
  !!filterRevisionEffect && !filterRevisionEffect.includes("cache.current.clear()"),
);
expectContract(
  "table row click does not call navigate",
  !/onClick=\{[\s\S]*?navigateToResultIndex/.test(files.table),
);
expectContract("table has column model", files.table.includes("TABLE_COLUMNS"));
expectContract("table has resize handle", files.table.includes("lf-column-resize-handle"));
expectContract("table has column menu", files.table.includes("lf-column-menu"));
expectContract("css styles resize handle", files.css.includes(".lf-column-resize-handle"));
expectContract("css styles column menu", files.css.includes(".lf-column-menu"));
expectContract("root scroll is isolated from table overscroll", files.css.includes("overscroll-behavior: none"));
expectContract("table scroll contains boundary overscroll", files.css.includes("overscroll-behavior: contain"));
expectContract(
  "table captures wheel at vertical boundaries",
  files.table.includes("handleTableWheel") && files.table.includes("onWheelCapture={handleTableWheel}"),
);
expectContract("table exposes marked row state", files.table.includes("data-marked"));
expectContract("css styles marked rows", files.css.includes('.lf-table-row[data-marked="true"]'));
expectContract("css defines marked row color tokens", files.css.includes("--lf-row-marked"));
expectContract("table formats copied rows as inline fields", files.table.includes("formatRowForClipboard"));
expectContract(
  "table overrides native clipboard copy",
  files.table.includes("handleTableCopy") && files.table.includes("onCopy={handleTableCopy}"),
);
expectContract("table exposes copy selection row state", files.table.includes("data-copy-selected"));
expectContract(
  "css styles native copy-selected rows",
  files.css.includes('.lf-table-row[data-copy-selected="true"]'),
);
expectContract("css defines copy-selected text token", files.css.includes("--lf-row-copy-selected-text"));
expectContract(
  "css makes copy-selected row text override level colors",
  files.css.includes('.lf-table-row[data-copy-selected="true"] .lf-level') &&
    files.css.includes('.lf-table-row[data-copy-selected="true"] .lf-message'),
);
expectContract(
  "css makes copy-selected rows override level backgrounds",
  files.css.includes('.lf-table-row[data-copy-selected="true"][data-level="W"]') &&
    files.css.includes('.lf-table-row[data-copy-selected="true"][data-level="E"]') &&
    files.css.includes('.lf-table-row[data-copy-selected="true"][data-level="F"]') &&
    files.css.includes('.lf-table-row[data-copy-selected="true"][data-marked="true"]'),
);
expectContract(
  "css uses subdued copy-selected color tokens",
  files.css.includes("--lf-row-copy-selected: #cfe3ff") &&
    files.css.includes("--lf-row-copy-selected-text: #17345f"),
);
expectContract("toolbar declares tooltip text", files.toolbar.includes("data-tooltip"));
expectContract("css styles shared tooltips", files.css.includes("[data-tooltip]::after"));
expectContract(
  "toolbar custom tooltip controls omit native title",
  [
    'title="Source file"',
    'title="Current source"',
    'title="Logcat command"',
    'title="Start"',
    'title="Pause"',
    'title="Stop"',
    'title="Clear"',
    'title="Open file"',
    'title="Export"',
    'title="Split file"',
    'title="Settings"',
    'title="Theme"',
    'title="All levels"',
    'title={LEVEL_TOOLTIPS[level]}',
    'title="Marked only"',
    'title="Case sensitive"',
    'title="Regex search"',
    'title="Highlight color"',
    'title="Previous match"',
    'title="Next match"',
    'title="Regex filter"',
  ].every((title) => !files.toolbar.includes(title)),
);
expectContract(
  "table custom tooltip controls omit native title",
  !files.table.includes('title="Show columns"'),
);
expectContract(
  "top toolbar tooltips open downward",
  (files.toolbar.match(/data-tooltip-placement="bottom"/g) ?? []).length >= 3,
);
expectContract(
  "css supports bottom tooltip placement",
  files.css.includes('[data-tooltip-placement="bottom"]::after'),
);
expectContract("table tracks continuous selection range", files.table.includes("selectionRange"));
expectContract("table collects continuous selected rows", files.table.includes("collectRowsInRange"));
expectContract("table supports context menu bookmark actions", files.table.includes("bookmarkMenu"));
expectContract("table exposes context menu handler", files.table.includes("onContextMenu"));
expectContract("css styles table context menu", files.css.includes(".lf-table-context-menu"));

console.log("log table interaction contracts verified");
