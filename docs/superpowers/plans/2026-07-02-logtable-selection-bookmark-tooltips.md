# LogTable 选择、批量标记与按钮提示实施计划

> **面向执行代理:** 按任务逐项执行本计划;推荐使用 `superpowers:subagent-driven-development`,也可使用 `superpowers:executing-plans`。步骤使用 checkbox (`- [ ]`) 语法追踪。

**目标:** 实现统一 tooltip、连续拖选范围、右键批量 bookmark 菜单,并保持复制与选区视觉一致。

**架构:** 本次只修改前端交互层。Toolbar 增加 tooltip 合约和文案;LogTable 将原来的离散 `Set<number>` 选区升级为连续 `SelectionRange`,复制和右键菜单都读取同一个范围;批量 bookmark 继续调用现有 `toggleBookmark(lineNo)` IPC,不新增后端契约。

**技术栈:** React + TypeScript + Zustand + TanStack Virtual + Tailwind v4 CSS-first + 现有结构契约脚本。

---

### 任务 1: 结构契约红灯

**文件:**
- 修改: `scripts/verify-logtable-interaction.mjs`

- [ ] **步骤 1: 写失败检查**

在现有 copy-selected 检查后加入以下契约:

```js
expectContract("toolbar declares tooltip text", files.toolbar.includes("data-tooltip"));
expectContract("css styles shared tooltips", files.css.includes("[data-tooltip]::after"));
expectContract("table tracks continuous selection range", files.table.includes("selectionRange"));
expectContract("table collects continuous selected rows", files.table.includes("collectRowsInRange"));
expectContract("table supports context menu bookmark actions", files.table.includes("bookmarkMenu"));
expectContract("table exposes context menu handler", files.table.includes("onContextMenu"));
expectContract("css styles table context menu", files.css.includes(".lf-table-context-menu"));
```

同时把 `files` 对象补充:

```js
toolbar: readFileSync("src/components/Toolbar.tsx", "utf8"),
```

- [ ] **步骤 2: 确认失败**

运行: `node scripts/verify-logtable-interaction.mjs`

预期: 失败,错误包含 `toolbar declares tooltip text`。

### 任务 2: Tooltip 文案与样式

**文件:**
- 修改: `src/components/Toolbar.tsx`
- 修改: `src/components/LogTable.tsx`
- 修改: `src/index.css`

- [ ] **步骤 1: 增加 Toolbar tooltip 文案**

给工具栏按钮、级别按钮、搜索按钮加入 `data-tooltip` 和 `aria-label`,同时保留 `title`。级别映射使用:

```ts
const LEVEL_TOOLTIPS = {
  V: "Verbose",
  D: "Debug",
  I: "Info",
  W: "Warning",
  E: "Error",
  F: "Fatal",
} as const;
```

- [ ] **步骤 2: 增加 LogTable 列按钮 tooltip**

给显示列按钮加入:

```tsx
aria-label="Show columns"
data-tooltip="Show columns"
title="Show columns"
```

- [ ] **步骤 3: 增加 CSS tooltip**

在 `src/index.css` 添加:

```css
[data-tooltip] {
  position: relative;
}

[data-tooltip]::after {
  content: attr(data-tooltip);
  position: absolute;
  left: 50%;
  bottom: calc(100% + 7px);
  z-index: 60;
  transform: translateX(-50%) translateY(2px);
  opacity: 0;
  pointer-events: none;
  white-space: nowrap;
  padding: 4px 7px;
  border: 1px solid var(--lf-border-strong);
  border-radius: 5px;
  background: var(--lf-text);
  color: var(--lf-bg);
  font-size: 11px;
  line-height: 1.2;
  box-shadow: 0 8px 18px rgba(15, 23, 42, 0.16);
  transition: opacity 120ms ease, transform 120ms ease;
}

[data-tooltip]:hover::after,
[data-tooltip]:focus-visible::after {
  opacity: 1;
  transform: translateX(-50%) translateY(0);
}
```

### 任务 3: 连续选区模型

**文件:**
- 修改: `src/components/LogTable.tsx`
- 修改: `src/index.css`

- [ ] **步骤 1: 增加类型与 helper**

在 `ResizeState` 附近加入:

```ts
interface SelectionRange {
  start: number;
  end: number;
}

function normalizeSelectionRange(start: number, end: number): SelectionRange {
  return start <= end ? { start, end } : { start: end, end: start };
}

function selectionRangeEqual(left: SelectionRange | null, right: SelectionRange | null) {
  if (left === right) return true;
  if (!left || !right) return false;
  return left.start === right.start && left.end === right.end;
}
```

- [ ] **步骤 2: 替换离散 Set 状态**

把:

```ts
const [copySelectedRows, setCopySelectedRows] = useState<Set<number>>(() => new Set());
```

替换为:

```ts
const [selectionRange, setSelectionRange] = useState<SelectionRange | null>(null);
```

- [ ] **步骤 3: 增加连续收集函数**

新增:

```ts
const collectRowsInRange = useCallback(
  (range: SelectionRange | null = selectionRange) => {
    if (!range) return [];
    const rows: Array<{ index: number; row: Row }> = [];
    for (let index = range.start; index <= range.end; index += 1) {
      const row = cache.current.get(index);
      if (row) rows.push({ index, row });
    }
    return rows;
  },
  [selectionRange],
);
```

- [ ] **步骤 4: 修改 selectionchange 逻辑**

让 `refreshCopySelection` 先调用现有 `collectRowsFromSelection()`,如果没有命中则清空;如果有命中,取最小和最大 index 形成连续 `selectionRange`。

- [ ] **步骤 5: 修改复制逻辑与行属性**

`handleTableCopy` 使用 `collectRowsInRange()`。行属性改为:

```tsx
data-copy-selected={
  selectionRange && vi.index >= selectionRange.start && vi.index <= selectionRange.end
    ? true
    : undefined
}
```

- [ ] **步骤 6: 调整选区字重**

将 `.lf-table-row[data-copy-selected="true"] ... { font-weight: 650; }` 改为 `font-weight: 600;`。

### 任务 4: 右键批量 bookmark 菜单

**文件:**
- 修改: `src/components/LogTable.tsx`
- 修改: `src/index.css`

- [ ] **步骤 1: 增加菜单状态类型**

在接口区加入:

```ts
interface BookmarkMenuState {
  x: number;
  y: number;
  range: SelectionRange;
}
```

组件内增加:

```ts
const [bookmarkMenu, setBookmarkMenu] = useState<BookmarkMenuState | null>(null);
```

- [ ] **步骤 2: 增加右键 handler**

新增:

```ts
const openBookmarkMenu = useCallback(
  (event: ReactMouseEvent<HTMLDivElement>, index: number) => {
    const row = cache.current.get(index);
    if (!row) return;
    event.preventDefault();
    const range =
      selectionRange && index >= selectionRange.start && index <= selectionRange.end
        ? selectionRange
        : { start: index, end: index };
    setBookmarkMenu({ x: event.clientX, y: event.clientY, range });
    selectRow(row.lineNo, index);
  },
  [selectRow, selectionRange],
);
```

需要从 React 类型中引入 `MouseEvent as ReactMouseEvent`。

- [ ] **步骤 3: 增加批量执行函数**

新增:

```ts
const applyBookmarkRange = useCallback(
  async (targetMarked: boolean) => {
    if (!bookmarkMenu) return;
    const rows = collectRowsInRange(bookmarkMenu.range);
    for (const { index, row } of rows) {
      if (row.marked === targetMarked) continue;
      const marked = await toggleBookmark(row.lineNo);
      cache.current.set(index, { ...row, marked });
    }
    const bookmarks = await listBookmarks();
    setBookmarks(bookmarks);
    setBookmarkMenu(null);
    force((x) => x + 1);
  },
  [bookmarkMenu, collectRowsInRange, setBookmarks],
);
```

- [ ] **步骤 4: 关闭菜单**

增加 effect:点击窗口其他位置或按 `Escape` 关闭 `bookmarkMenu`。

- [ ] **步骤 5: 渲染菜单**

在表格滚动容器后渲染:

```tsx
{bookmarkMenu && (
  <div className="lf-table-context-menu" style={{ left: bookmarkMenu.x, top: bookmarkMenu.y }}>
    <button type="button" onClick={() => applyBookmarkRange(true)}>标记选中行</button>
    <button type="button" onClick={() => applyBookmarkRange(false)}>取消标记</button>
  </div>
)}
```

每行增加:

```tsx
onContextMenu={(event) => openBookmarkMenu(event, vi.index)}
```

- [ ] **步骤 6: 增加菜单 CSS**

添加 `.lf-table-context-menu`、`.lf-table-context-menu button` 和 hover 样式,使用 `position: fixed`,项目面板色、边框和紧凑行高。

### 任务 5: 验证与提交

**文件:**
- 验证: `scripts/verify-logtable-interaction.mjs`
- 验证: `src/components/Toolbar.tsx`
- 验证: `src/components/LogTable.tsx`
- 验证: `src/index.css`

- [ ] **步骤 1: 运行契约脚本**

运行: `node scripts/verify-logtable-interaction.mjs`

预期: 通过。

- [ ] **步骤 2: 运行项目验证**

运行:

```bash
cargo test -p logcore
cargo build --workspace
pnpm build
git diff --check
```

预期: 全部 exit 0。

- [ ] **步骤 3: 提交实现**

运行:

```bash
git add docs/superpowers/plans/2026-07-02-logtable-selection-bookmark-tooltips.md scripts/verify-logtable-interaction.mjs src/components/Toolbar.tsx src/components/LogTable.tsx src/index.css
git commit -m "feat(ui): enhance table selection and bookmark actions"
```
