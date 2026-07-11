# 日志表格与小地图交互修正 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修正小地图框内拖动瞬移、表格双击/过滤跳首屏、单击自动居中,并补齐日志表格列宽调整与列显隐持久化。

**Architecture:** 把“选中行”“当前视口”“一次性滚动请求”拆成独立前端状态;表格只在打开新会话时回到顶部,过滤刷新保留锚点,书签刷新只更新受影响行。列宽/列显隐作为 TOML 配置进入 `logcore` 配置模型、Tauri DTO 和前端 `AppConfig`,但不改变 `get_rows` 热路径。

**Tech Stack:** Rust `logcore` + Tauri v2 DTO + React/TypeScript + zustand + TanStack Virtual + Tailwind v4 CSS-first + Lucide 图标。

---

## 文件结构

- 修改 `crates/logcore/src/config.rs`:新增表格列配置模型、默认值、归一化和单测。
- 修改 `src-tauri/src/dto.rs`:在 `AppConfigDto` 中桥接表格列配置。
- 修改 `src/types.ts`:补齐 `TableColumnConfig`、`TableConfig`、`ScrollRequest` 等 TS 类型。
- 修改 `src/store/session.ts`:新增 `viewportResultIndex`、`scrollRequest` 和导航 action;默认配置补表格列。
- 修改 `src/App.tsx`:F2/F3 从“只选中”改为“选中 + 创建滚动请求”。
- 修改 `src/components/Minimap.tsx`:蓝框内拖动保留 grab offset;蓝框位置来自 `viewportResultIndex`;小地图导航创建滚动请求。
- 修改 `src/components/LogTable.tsx`:列模型化、列宽拖拽、列显隐浮层、过滤刷新保留锚点、书签更新不清缓存、单击不滚动。
- 修改 `src/index.css`:增加列按钮、浮层、resize handle、表头布局等 CSS。
- 新建 `scripts/verify-logtable-interaction.mjs`:前端 headless 结构回归检查。

## Task 1: 表格列配置进入 logcore/Tauri/TS

**Files:**
- Modify: `crates/logcore/src/config.rs`
- Modify: `src-tauri/src/dto.rs`
- Modify: `src/types.ts`
- Modify: `src/store/session.ts`

- [ ] **Step 1: 写失败的 Rust 配置测试**

在 `crates/logcore/src/config.rs` 的 `mod tests` 中加入:

```rust
#[test]
fn default_config_includes_complete_table_columns() {
    let config = AppConfig::default();
    let ids: Vec<&str> = config.table.columns.iter().map(|column| column.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["bookmark", "lineNo", "date", "time", "level", "pid", "tid", "tag", "message"]
    );
    assert!(config.table.columns.iter().all(|column| column.visible));
    assert!(config.table.columns.iter().all(|column| column.width > 0));
}

#[test]
fn toml_round_trip_preserves_table_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut config = AppConfig::default();
    config.table.columns.iter_mut().for_each(|column| {
        if column.id == "tag" {
            column.width = 210;
        }
        if column.id == "pid" {
            column.visible = false;
        }
    });

    save_config(&path, &config).unwrap();
    let loaded = load_config(&path).unwrap();
    assert_eq!(loaded.table, config.table);
}

#[test]
fn table_columns_are_normalized_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
theme = "light"
encoding = "UTF-8"
font_size = 13
row_height = 20

[[table.columns]]
id = "tag"
width = 999
visible = false

[[table.columns]]
id = "unknown"
width = 77
visible = true

[[table.columns]]
id = "message"
width = 1
visible = false
"#,
    )
    .unwrap();

    let config = load_config(&path).unwrap();
    let tag = config.table.columns.iter().find(|column| column.id == "tag").unwrap();
    let message = config.table.columns.iter().find(|column| column.id == "message").unwrap();
    assert_eq!(tag.width, 260);
    assert!(!tag.visible);
    assert_eq!(message.width, 220);
    assert!(message.visible);
    assert!(!config.table.columns.iter().any(|column| column.id == "unknown"));
    assert_eq!(config.table.columns.len(), 9);
}
```

- [ ] **Step 2: 运行测试并确认失败**

Run:

```bash
cargo test -p logcore config::tests::default_config_includes_complete_table_columns
```

Expected:编译失败,提示 `AppConfig` 没有 `table` 字段。

- [ ] **Step 3: 实现 Rust 配置模型**

在 `crates/logcore/src/config.rs` 增加:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableColumnConfig {
    pub id: String,
    pub width: u16,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableConfig {
    pub columns: Vec<TableColumnConfig>,
}

#[derive(Clone, Copy)]
struct TableColumnSpec {
    id: &'static str,
    width: u16,
    min: u16,
    max: u16,
}

const TABLE_COLUMN_SPECS: [TableColumnSpec; 9] = [
    TableColumnSpec { id: "bookmark", width: 24, min: 22, max: 36 },
    TableColumnSpec { id: "lineNo", width: 58, min: 52, max: 120 },
    TableColumnSpec { id: "date", width: 50, min: 48, max: 90 },
    TableColumnSpec { id: "time", width: 98, min: 82, max: 160 },
    TableColumnSpec { id: "level", width: 40, min: 36, max: 60 },
    TableColumnSpec { id: "pid", width: 54, min: 48, max: 100 },
    TableColumnSpec { id: "tid", width: 54, min: 48, max: 100 },
    TableColumnSpec { id: "tag", width: 154, min: 110, max: 260 },
    TableColumnSpec { id: "message", width: 360, min: 220, max: 1200 },
];
```

并让 `AppConfig` 增加 `pub table: TableConfig`。`Default` 使用 `TableConfig::default()`,`normalized()` 调用 `self.table = self.table.normalized()`。

- [ ] **Step 4: 扩展 DTO/TS/store 默认配置**

`src-tauri/src/dto.rs` 新增 DTO:

```rust
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TableColumnConfigDto {
    pub id: String,
    pub width: u16,
    pub visible: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TableConfigDto {
    pub columns: Vec<TableColumnConfigDto>,
}
```

`AppConfigDto` 增加 `pub table: TableConfigDto`,并在 `from_config`/`TryFrom<AppConfigDto>` 中双向转换。

`src/types.ts` 增加:

```ts
export interface TableColumnConfig {
  id: string;
  width: number;
  visible: boolean;
}

export interface TableConfig {
  columns: TableColumnConfig[];
}
```

`AppConfig` 增加 `table: TableConfig`。`src/store/session.ts` 的 `DEFAULT_CONFIG` 增加 9 列默认配置。

- [ ] **Step 5: 验证并提交**

Run:

```bash
cargo test -p logcore config
pnpm build
```

Expected:`logcore` 配置测试通过;`pnpm build` 若因前端尚未消费新类型失败,先修正 DTO/TS 默认值再提交。

Commit:

```bash
git add crates/logcore/src/config.rs src-tauri/src/dto.rs src/types.ts src/store/session.ts
git commit -m "feat(config): persist log table columns"
```

## Task 2: 前端交互结构回归测试

**Files:**
- Create: `scripts/verify-logtable-interaction.mjs`
- Modify later: `src/store/session.ts`
- Modify later: `src/components/Minimap.tsx`
- Modify later: `src/components/LogTable.tsx`

- [ ] **Step 1: 写失败的 headless 检查脚本**

Create `scripts/verify-logtable-interaction.mjs`:

```js
import { readFileSync } from "node:fs";

const files = {
  session: readFileSync("src/store/session.ts", "utf8"),
  minimap: readFileSync("src/components/Minimap.tsx", "utf8"),
  table: readFileSync("src/components/LogTable.tsx", "utf8"),
  css: readFileSync("src/index.css", "utf8"),
};

function expect(name, condition) {
  if (!condition) {
    throw new Error(`Missing interaction contract: ${name}`);
  }
}

expect("store has ScrollRequest", files.session.includes("ScrollRequest"));
expect("store tracks viewportResultIndex", files.session.includes("viewportResultIndex"));
expect("store exposes navigateToResultIndex", files.session.includes("navigateToResultIndex"));
expect("minimap stores grab offset", files.minimap.includes("grabOffsetRef"));
expect("minimap uses viewportResultIndex", files.minimap.includes("viewportResultIndex"));
expect("table consumes scrollRequest", files.table.includes("scrollRequest"));
expect("table updates viewportResultIndex", files.table.includes("setViewportResultIndex"));
expect("table no longer resets on bookmarkRevision", !files.table.includes("bookmarkRevision"));
expect("table row click does not call navigate", !/onClick=\\{[^}]*navigateToResultIndex/s.test(files.table));
expect("table has column model", files.table.includes("TABLE_COLUMNS"));
expect("table has resize handle", files.table.includes("lf-column-resize-handle"));
expect("table has column menu", files.table.includes("lf-column-menu"));
expect("css styles resize handle", files.css.includes(".lf-column-resize-handle"));
expect("css styles column menu", files.css.includes(".lf-column-menu"));
```

- [ ] **Step 2: 运行脚本并确认失败**

Run:

```bash
node scripts/verify-logtable-interaction.mjs
```

Expected:失败,至少报 `store has ScrollRequest`。

- [ ] **Step 3: 暂存脚本但不提交实现**

脚本作为本轮前端红灯测试保留,实现任务通过后再提交。

## Task 3: 拆分选中、视口与滚动请求;修复小地图框内拖动

**Files:**
- Modify: `src/types.ts`
- Modify: `src/store/session.ts`
- Modify: `src/App.tsx`
- Modify: `src/components/Toolbar.tsx`
- Modify: `src/components/Minimap.tsx`

- [ ] **Step 1: 扩展前端类型和 zustand 状态**

`src/types.ts` 增加:

```ts
export interface ScrollRequest {
  index: number;
  align: "auto" | "center" | "start";
  reason: "minimap" | "bookmark" | "search";
  nonce: number;
}
```

`SessionState` 增加:

```ts
viewportResultIndex: number;
scrollRequest: ScrollRequest | null;
setViewportResultIndex: (index: number) => void;
navigateToResultIndex: (
  index: number,
  options?: { lineNo?: number | null; align?: ScrollRequest["align"]; reason?: ScrollRequest["reason"] },
) => void;
```

`selectRow` 只写 `selectedLine` 和 `selectedResultIndex`。`navigateToResultIndex` 写 `selectedResultIndex`、可选 `selectedLine`、递增 `scrollRequest.nonce`。

- [ ] **Step 2: 更新 F2/F3 和搜索导航**

`src/App.tsx` 中 F2/F3 成功返回后调用:

```ts
navigateToResultIndex(target.resultIndex, {
  lineNo: target.lineNo,
  align: "center",
  reason: "bookmark",
});
```

`src/components/Toolbar.tsx` 中搜索跳转保持 `setCurrentSearchLine(line)`,由 `LogTable` 在默认顺序下创建滚动。

- [ ] **Step 3: 修复 Minimap 拖动映射**

`src/components/Minimap.tsx` 增加:

```ts
const VIEWPORT_MIN_HEIGHT = 22;
const VIEWPORT_HEIGHT_RATIO = 0.08;
const grabOffsetRef = useRef<number | null>(null);

function viewportHeightPx(rect: DOMRect) {
  return Math.max(VIEWPORT_MIN_HEIGHT, rect.height * VIEWPORT_HEIGHT_RATIO);
}

function indexToViewportTopPx(index: number, rect: DOMRect, resultCount: number) {
  if (resultCount <= 0) return 0;
  const maxTop = Math.max(0, rect.height - viewportHeightPx(rect));
  return clamp((index / resultCount) * rect.height, 0, maxTop);
}
```

`onPointerDown` 判断指针是否在当前蓝框内部。内部拖动保存 `grabOffsetRef`;外部点击将 `grabOffsetRef` 置为 0 并立即导航。拖动更新时使用 `pointerY - grabOffsetRef.current` 计算目标索引。

- [ ] **Step 4: 运行前端结构检查**

Run:

```bash
node scripts/verify-logtable-interaction.mjs
```

Expected:仍可能因 `LogTable` 列模型未实现失败,但不再因 store/minimap 缺少字段失败。

Commit:

```bash
git add src/types.ts src/store/session.ts src/App.tsx src/components/Toolbar.tsx src/components/Minimap.tsx scripts/verify-logtable-interaction.mjs
git commit -m "fix(ui): decouple row selection from navigation"
```

## Task 4: 修复 LogTable 缓存刷新、单击行为与列模型

**Files:**
- Modify: `src/components/LogTable.tsx`

- [ ] **Step 1: 去掉 `bookmarkRevision` 驱动的全表重置**

把现有:

```ts
useEffect(() => {
  cache.current.clear();
  filled.current.clear();
  inflight.current.clear();
  parentRef.current?.scrollTo({ top: 0 });
  force((x) => x + 1);
}, [sessionId, bookmarkRevision, filterResultRevision]);
```

拆成:

```ts
useEffect(() => {
  cache.current.clear();
  filled.current.clear();
  inflight.current.clear();
  parentRef.current?.scrollTo({ top: 0 });
  setViewportResultIndex(0);
  force((x) => x + 1);
}, [sessionId, setViewportResultIndex]);

useEffect(() => {
  cache.current.clear();
  filled.current.clear();
  inflight.current.clear();
  force((x) => x + 1);
}, [filterResultRevision]);
```

- [ ] **Step 2: `scrollRequest` 驱动程序化滚动**

替换监听 `selectedResultIndex` 的滚动 effect:

```ts
useEffect(() => {
  if (!scrollRequest) return;
  rv.scrollToIndex(Math.max(0, scrollRequest.index), { align: scrollRequest.align });
}, [rv, scrollRequest]);
```

单击行只调用:

```ts
selectRow(row.lineNo, vi.index);
```

- [ ] **Step 3: 反向更新视口索引**

在 `items` 生成后增加:

```ts
useEffect(() => {
  const first = items[0]?.index;
  if (first == null) return;
  setViewportResultIndex(first);
}, [items, setViewportResultIndex]);
```

- [ ] **Step 4: 书签切换只更新当前缓存行**

`toggleRowBookmark` 使用 `toggleBookmark` 的返回值:

```ts
const marked = await toggleBookmark(row.lineNo);
cache.current.forEach((cached, index) => {
  if (cached.lineNo === row.lineNo) {
    cache.current.set(index, { ...cached, marked });
  }
});
const bookmarks = await listBookmarks();
setBookmarks(bookmarks);
force((x) => x + 1);
```

- [ ] **Step 5: 列模型化**

在 `LogTable.tsx` 顶部定义:

```ts
const TABLE_COLUMNS = [
  { id: "bookmark", label: "", className: "lf-bookmark-cell", min: 22, max: 36 },
  { id: "lineNo", label: "行号", className: "lf-num", min: 52, max: 120 },
  { id: "date", label: "日期", className: "lf-meta", min: 48, max: 90 },
  { id: "time", label: "时间", className: "lf-meta", min: 82, max: 160 },
  { id: "level", label: "级别", className: "lf-level", min: 36, max: 60 },
  { id: "pid", label: "PID", className: "lf-num", min: 48, max: 100 },
  { id: "tid", label: "TID", className: "lf-num", min: 48, max: 100 },
  { id: "tag", label: "Tag", className: "lf-tag", min: 110, max: 260 },
  { id: "message", label: "消息", className: "lf-message", min: 220, max: 1200 },
] as const;
```

根据 `appConfig.table.columns` 计算 `visibleColumns` 和 `gridTemplateColumns`。消息列输出 `minmax(${width}px, 1fr)`,其它列输出 `${width}px`。

- [ ] **Step 6: 运行结构检查**

Run:

```bash
node scripts/verify-logtable-interaction.mjs
```

Expected:若列菜单尚未实现,只剩 column menu/CSS 相关失败。

Commit:

```bash
git add src/components/LogTable.tsx
git commit -m "fix(ui): keep log table anchored during refresh"
```

## Task 5: 列宽拖拽、列显隐 UI 与 CSS

**Files:**
- Modify: `src/components/LogTable.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: 实现列宽拖拽保存**

在 `LogTable.tsx` 增加 `resizeRef` 记录拖拽列、起始 X、起始宽度。`pointermove` 更新本地 `draftColumns`,同步 `setAppConfig`;`pointerup` 调用 `saveAppConfig(nextConfig)`。

手柄 JSX:

```tsx
<span
  className="lf-column-resize-handle"
  onPointerDown={(event) => startColumnResize(event, column.id)}
/>
```

- [ ] **Step 2: 实现列显隐浮层**

表头右侧增加:

```tsx
<button className="lf-column-menu-button" type="button" title="显示列" onClick={() => setColumnMenuOpen((open) => !open)}>
  <Columns3 />
</button>
```

浮层:

```tsx
{columnMenuOpen && (
  <div className="lf-column-menu">
    {TABLE_COLUMNS.map((column) => (
      <label key={column.id}>
        <input
          checked={isColumnVisible(column.id)}
          disabled={column.id === "message"}
          type="checkbox"
          onChange={() => toggleColumnVisibility(column.id)}
        />
        <span>{column.label || "书签"}</span>
      </label>
    ))}
  </div>
)}
```

- [ ] **Step 3: 添加 CSS**

在 `src/index.css` 的表格区域增加:

```css
.lf-table-header-cell {
  position: relative;
  height: 28px;
  line-height: 28px;
  padding: 0 6px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  border-right: 1px solid var(--lf-border);
}

.lf-column-resize-handle {
  position: absolute;
  top: 0;
  right: -3px;
  width: 6px;
  height: 100%;
  cursor: col-resize;
  z-index: 2;
}

.lf-column-menu-button {
  position: absolute;
  top: 2px;
  right: 4px;
  width: 24px;
  height: 24px;
}

.lf-column-menu {
  position: absolute;
  top: 28px;
  right: 6px;
  z-index: 20;
  width: 150px;
  padding: 6px;
  border: 1px solid var(--lf-border);
  border-radius: 7px;
  background: var(--lf-panel-2);
  box-shadow: 0 12px 28px rgba(15, 23, 42, 0.16);
}
```

- [ ] **Step 4: 结构检查、构建并提交**

Run:

```bash
node scripts/verify-logtable-interaction.mjs
pnpm build
```

Expected:两个命令通过。

Commit:

```bash
git add src/components/LogTable.tsx src/index.css scripts/verify-logtable-interaction.mjs
git commit -m "feat(ui): add adjustable log table columns"
```

## Task 6: 全量验证与对抗式自审

**Files:**
- Review all changed files

- [ ] **Step 1: 全量验证**

Run:

```bash
cargo test -p logcore
cargo build --workspace
pnpm build
node scripts/verify-logtable-interaction.mjs
```

Expected:全部 exit 0。

- [ ] **Step 2: 对抗式自审**

Run:

```bash
rg -n "scrollTo\\(\\{ top: 0 \\}\\)|bookmarkRevision|setSelectedResultIndex\\(vi\\.index\\)|const COLS|const HEADERS|getRows\\(\"filtered\", block, WINDOW\\)" src/components/LogTable.tsx src/components/Minimap.tsx src/store/session.ts src/App.tsx
```

Expected:

- `scrollTo({ top: 0 })` 只允许出现在新会话 effect。
- `bookmarkRevision` 不出现在 `LogTable.tsx`。
- `setSelectedResultIndex(vi.index)` 不出现在行点击。
- `const COLS`/`const HEADERS` 不存在。
- `getRows("filtered", block, WINDOW)` 仍存在,说明没有破坏可见窗口规则。

- [ ] **Step 3: 整理提交列表**

Run:

```bash
git log --oneline --decorate -8
git status --short
```

Expected:只有用户参考视频 `refs/` 可以保持未跟踪;实现文件已提交。
