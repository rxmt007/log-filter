import { readFileSync } from "node:fs";

const files = {
  session: readFileSync("src/store/session.ts", "utf8"),
  minimap: readFileSync("src/components/Minimap.tsx", "utf8"),
  table: readFileSync("src/components/LogTable.tsx", "utf8"),
  css: readFileSync("src/index.css", "utf8"),
};

function expectContract(name, condition) {
  if (!condition) {
    throw new Error(`Missing interaction contract: ${name}`);
  }
}

expectContract("store has ScrollRequest", files.session.includes("ScrollRequest"));
expectContract("store tracks viewportResultIndex", files.session.includes("viewportResultIndex"));
expectContract("store exposes navigateToResultIndex", files.session.includes("navigateToResultIndex"));
expectContract("minimap stores grab offset", files.minimap.includes("grabOffsetRef"));
expectContract("minimap uses viewportResultIndex", files.minimap.includes("viewportResultIndex"));
expectContract("table consumes scrollRequest", files.table.includes("scrollRequest"));
expectContract("table updates viewportResultIndex", files.table.includes("setViewportResultIndex"));
expectContract("table no longer resets on bookmarkRevision", !files.table.includes("bookmarkRevision"));
expectContract(
  "table row click does not call navigate",
  !/onClick=\{[\s\S]*?navigateToResultIndex/.test(files.table),
);
expectContract("table has column model", files.table.includes("TABLE_COLUMNS"));
expectContract("table has resize handle", files.table.includes("lf-column-resize-handle"));
expectContract("table has column menu", files.table.includes("lf-column-menu"));
expectContract("css styles resize handle", files.css.includes(".lf-column-resize-handle"));
expectContract("css styles column menu", files.css.includes(".lf-column-menu"));

console.log("log table interaction contracts verified");
