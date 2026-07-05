# 统一筛选控件实施计划

> **面向 agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按已批准设计,把 `全部 / 过滤 / 书签 / 错误 / 全级别` 与等级芯片合并为一个统一筛选表达式,并让表格、小地图、F2/F3 都基于当前结果集工作。

**Architecture:** `logcore` 增加 `marked_only` 过滤语义和当前结果导航/小地图能力,但仍只保存命中源行号数组。Tauri 继续做薄 DTO/命令封装。前端移除手动视图按钮,让主表格始终请求 `filtered` 当前结果窗口,并用结果索引驱动滚动和小地图。

**Tech Stack:** Rust `logcore` + Tauri v2 commands + React/TypeScript + zustand + TanStack Virtual + Tailwind v4 CSS-first + Lucide `Bookmark` icon。

---

## 文件结构

- 修改 `AGENTS.md`: 记录后续项目文档默认中文的协作规则。
- 修改 `crates/logcore/src/filter.rs`: 给 `FilterSpec` 增加 `marked_only`,并让 `FilterMatcher` 支持带标记状态的匹配。
- 修改 `crates/logcore/src/session.rs`: 在 `set_filter` 中串行叠加标记过滤;新增当前结果索引/行号定位、当前结果内书签导航、当前结果小地图。
- 修改 `src-tauri/src/dto.rs`: 增加 `markedOnly` DTO 字段和导航目标 DTO。
- 修改 `src-tauri/src/commands.rs`: 更新 `next_bookmark`,让它返回当前结果内的目标;保持 `get_rows` 的 `MAX_ROWS` 限制。
- 修改 `src/types.ts`: 增加 `FilterSpec.markedOnly` 和 `NavigationTarget` 类型。
- 修改 `src/lib/ipc.ts`: 更新 `nextBookmark` 返回类型。
- 修改 `src/store/session.ts`: 增加 `markedOnly` 默认值和 `selectedResultIndex` 状态;保留旧 `RowsView` 类型但 UI 主路径不再暴露它。
- 修改 `src/App.tsx`: 简化过滤 effect,不再在 `all/filtered/bookmarks/errors` 之间切换;F2/F3 使用当前结果内目标。
- 修改 `src/components/Toolbar.tsx`: 移除下方视图按钮;在等级芯片组加入 `全部` 和带 Bookmark 图标的 `仅标记`。
- 修改 `src/components/LogTable.tsx`: 始终读取 `filtered` 当前结果;点击行同时记录源行号和结果索引;按 `selectedResultIndex` 滚动。
- 修改 `src/components/Minimap.tsx`: 用当前结果行数定位,把连续 bucket 合并成红/蓝段。
- 修改 `src/components/StatusBar.tsx`: 用 `当前结果` 替换旧视图标签,并显示 `仅标记` 提示。
- 修改 `src/index.css`: 补充 `全部` 与等级之间的间距、`仅标记` 芯片和小地图连续段样式。

---

### Task 1: logcore 过滤语义加入 marked_only

**Files:**
- Modify: `crates/logcore/src/filter.rs`
- Modify: `crates/logcore/src/session.rs`

- [ ] **Step 1: 先写 filter 失败测试**

在 `crates/logcore/src/filter.rs` 的 `#[cfg(test)] mod tests` 中新增:

```rust
#[test]
fn empty_level_mask_matches_no_known_levels() {
    let entries = vec![
        entry("D", "100", "101", "Net", "debug"),
        entry("E", "100", "101", "Net", "error"),
    ];
    let spec = FilterSpec {
        levels: LevelMask::from_bits(0),
        ..Default::default()
    };

    assert_eq!(filter_entries(&entries, &spec).unwrap(), Vec::<u64>::new());
}

#[test]
fn marked_only_requires_marked_row() {
    let matcher = FilterMatcher::new(&FilterSpec {
        marked_only: true,
        ..Default::default()
    })
    .unwrap();
    let entry = entry("I", "100", "101", "Tag", "message");

    assert!(!matcher.is_match_with_mark(&entry, false));
    assert!(matcher.is_match_with_mark(&entry, true));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p logcore filter::tests::empty_level_mask_matches_no_known_levels filter::tests::marked_only_requires_marked_row`

Expected: 编译失败,提示 `FilterSpec` 没有 `marked_only` 字段或 `FilterMatcher` 没有 `is_match_with_mark`。

- [ ] **Step 3: 实现最小 filter 改动**

在 `FilterSpec` 增加字段和默认值:

```rust
pub struct FilterSpec {
    pub levels: LevelMask,
    pub marked_only: bool,
    pub pid: FilterField,
    pub tid: FilterField,
    pub tag_include: FilterField,
    pub tag_exclude: FilterField,
    pub word_include: FilterField,
    pub word_exclude: FilterField,
}

impl Default for FilterSpec {
    fn default() -> Self {
        Self {
            levels: LevelMask::all(),
            marked_only: false,
            pid: FilterField::default(),
            tid: FilterField::default(),
            tag_include: FilterField::default(),
            tag_exclude: FilterField::default(),
            word_include: FilterField::default(),
            word_exclude: FilterField::default(),
        }
    }
}
```

更新 `is_active`:

```rust
impl FilterSpec {
    pub fn is_active(&self) -> bool {
        !self.levels.is_all()
            || self.marked_only
            || self.pid.is_active()
            || self.tid.is_active()
            || self.tag_include.is_active()
            || self.tag_exclude.is_active()
            || self.word_include.is_active()
            || self.word_exclude.is_active()
    }
}
```

在 `FilterMatcher` 中保留旧 API,新增带标记状态的 API:

```rust
impl FilterMatcher {
    pub fn is_match(&self, entry: &LogEntry) -> bool {
        self.is_match_with_mark(entry, false)
    }

    pub fn is_match_with_mark(&self, entry: &LogEntry, marked: bool) -> bool {
        if self.spec.marked_only && !marked {
            return false;
        }
        if !self.spec.levels.is_all() && !self.spec.levels.contains_level(&entry.level) {
            return false;
        }
        if !include_exact(&self.pid, &entry.pid) || !include_exact(&self.tid, &entry.tid) {
            return false;
        }
        if !include_contains(&self.tag_include, &entry.tag)
            || exclude_contains(&self.tag_exclude, &entry.tag)
        {
            return false;
        }
        if !include_contains(&self.word_include, &entry.message)
            || exclude_contains(&self.word_exclude, &entry.message)
        {
            return false;
        }
        true
    }
}
```

- [ ] **Step 4: 让 Session 使用标记状态匹配**

在 `Session::set_filter` 循环中替换匹配调用:

```rust
let marked = self.is_bookmarked(idx as u64 + 1);
if matcher.is_match_with_mark(&entry, marked) {
    matches.push(idx as u64);
}
```

- [ ] **Step 5: 运行 logcore 测试**

Run: `cargo test -p logcore`

Expected: 全部通过。

- [ ] **Step 6: 提交**

```bash
git add crates/logcore/src/filter.rs crates/logcore/src/session.rs
git commit -m "feat(logcore): add marked-only filter semantics"
```

---

### Task 2: 当前结果内导航与小地图

**Files:**
- Modify: `crates/logcore/src/session.rs`

- [ ] **Step 1: 写 Session 失败测试**

在 `crates/logcore/src/session.rs` 测试模块中新增:

```rust
#[test]
fn marked_only_filter_intersects_with_levels() {
    let f = temp_filter_log();
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    s.toggle_bookmark(4).unwrap();

    let count = s
        .set_filter(&FilterSpec {
            levels: crate::filter::LevelMask::from_levels(&["E", "F"]),
            marked_only: true,
            ..Default::default()
        })
        .unwrap();

    assert_eq!(count, 1);
    let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 4);
    assert_eq!(rows[0].1.level, "E");
}

#[test]
fn next_bookmark_uses_current_result_order() {
    let f = temp_filter_log();
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    s.toggle_bookmark(2).unwrap();
    s.toggle_bookmark(4).unwrap();
    s.set_filter(&FilterSpec {
        levels: crate::filter::LevelMask::from_levels(&["E", "F"]),
        ..Default::default()
    })
    .unwrap();

    let target = s
        .next_bookmark_in_current_result(1, BookmarkDirection::Next)
        .unwrap();
    assert_eq!(target.line_no, 4);
    assert_eq!(target.result_index, 0);
}

#[test]
fn minimap_uses_current_filtered_result_buckets() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for i in 0..8 {
        writeln!(f, "04-20 12:06:02.{i:03}   300   330 E Payment: error {i}").unwrap();
    }
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    s.set_filter(&FilterSpec {
        levels: crate::filter::LevelMask::from_levels(&["E", "F"]),
        ..Default::default()
    })
    .unwrap();

    let map = s.minimap(4);
    assert_eq!(map.errors, vec![0, 1, 2, 3]);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test -p logcore session::tests::marked_only_filter_intersects_with_levels session::tests::next_bookmark_uses_current_result_order session::tests::minimap_uses_current_filtered_result_buckets`

Expected: 编译失败,提示缺少 `marked_only` 字段或 `next_bookmark_in_current_result`。

- [ ] **Step 3: 增加导航目标类型**

在 `session.rs` 增加:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultTarget {
    pub line_no: u64,
    pub result_index: usize,
}
```

- [ ] **Step 4: 增加当前结果辅助方法**

在 `impl Session` 私有方法区增加:

```rust
fn current_result_len(&self) -> usize {
    self.filtered_count()
}

fn current_result_source_idx(&self, result_idx: usize) -> Option<usize> {
    if self.filter_active {
        self.filtered.get(result_idx).map(|idx| *idx as usize)
    } else if result_idx < self.indexer.offsets().len() {
        Some(result_idx)
    } else {
        None
    }
}

fn current_result_index_for_source_idx(&self, source_idx: u64) -> Option<usize> {
    if self.filter_active {
        self.filtered.binary_search(&source_idx).ok()
    } else if (source_idx as usize) < self.indexer.offsets().len() {
        Some(source_idx as usize)
    } else {
        None
    }
}

fn source_idx_is_error(&self, source_idx: u64) -> bool {
    self.error_lines.binary_search(&source_idx).is_ok()
}
```

- [ ] **Step 5: 实现当前结果内书签导航**

在 `impl Session` 增加:

```rust
pub fn next_bookmark_in_current_result(
    &self,
    from_line_no: u64,
    direction: BookmarkDirection,
) -> Option<ResultTarget> {
    let mut targets = self
        .bookmark_source_lines()
        .into_iter()
        .filter_map(|line_no| {
            let source_idx = line_no.saturating_sub(1);
            self.current_result_index_for_source_idx(source_idx)
                .map(|result_index| ResultTarget { line_no, result_index })
        })
        .collect::<Vec<_>>();
    if targets.is_empty() {
        return None;
    }
    targets.sort_by_key(|target| target.result_index);
    let from_source_idx = from_line_no.saturating_sub(1);
    let from_result_idx = self
        .current_result_index_for_source_idx(from_source_idx)
        .unwrap_or(0);
    match direction {
        BookmarkDirection::Next => {
            let idx = targets
                .iter()
                .position(|target| target.result_index > from_result_idx)
                .unwrap_or(0);
            Some(targets[idx])
        }
        BookmarkDirection::Previous => {
            let idx = targets
                .iter()
                .rposition(|target| target.result_index < from_result_idx)
                .unwrap_or(targets.len() - 1);
            Some(targets[idx])
        }
    }
}
```

- [ ] **Step 6: 改写 minimap 为当前结果坐标系**

替换 `Session::minimap` 主体:

```rust
pub fn minimap(&self, buckets: usize) -> Minimap {
    let total = self.current_result_len();
    if buckets == 0 || total == 0 {
        return Minimap {
            bookmarks: Vec::new(),
            errors: Vec::new(),
        };
    }
    let mut bookmarks = BTreeSet::new();
    let mut errors = BTreeSet::new();
    for result_idx in 0..total {
        let Some(source_idx) = self.current_result_source_idx(result_idx) else {
            continue;
        };
        let Some(bucket) = bucket_for_zero_based(result_idx, total, buckets) else {
            continue;
        };
        if self.is_bookmarked(source_idx as u64 + 1) {
            bookmarks.insert(bucket);
        }
        if self.source_idx_is_error(source_idx as u64) {
            errors.insert(bucket);
        }
    }
    Minimap {
        bookmarks: bookmarks.into_iter().collect(),
        errors: errors.into_iter().collect(),
    }
}
```

- [ ] **Step 7: 运行 logcore 测试**

Run: `cargo test -p logcore`

Expected: 全部通过。

- [ ] **Step 8: 提交**

```bash
git add crates/logcore/src/session.rs
git commit -m "feat(logcore): navigate and map current filter results"
```

---

### Task 3: Tauri DTO 与 IPC 契约

**Files:**
- Modify: `src-tauri/src/dto.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src/types.ts`
- Modify: `src/lib/ipc.ts`

- [ ] **Step 1: 写契约检查脚本并确认失败**

Run:

```bash
node <<'NODE'
const fs = require('fs');
const dto = fs.readFileSync('src-tauri/src/dto.rs', 'utf8');
const types = fs.readFileSync('src/types.ts', 'utf8');
const ipc = fs.readFileSync('src/lib/ipc.ts', 'utf8');
const failures = [];
if (!dto.includes('marked_only')) failures.push('FilterSpecDto must expose marked_only');
if (!dto.includes('NavigationTargetDto')) failures.push('Tauri must expose NavigationTargetDto');
if (!types.includes('markedOnly: boolean')) failures.push('TS FilterSpec must expose markedOnly');
if (!types.includes('interface NavigationTarget')) failures.push('TS must expose NavigationTarget');
if (!ipc.includes('invoke<NavigationTarget | null>')) failures.push('nextBookmark must return NavigationTarget | null');
if (failures.length) {
  console.error(failures.join('\n'));
  process.exit(1);
}
NODE
```

Expected: 失败并输出以上缺失项。

- [ ] **Step 2: 更新 Rust DTO**

在 `FilterSpecDto` 增加:

```rust
pub marked_only: bool,
```

在 `From<FilterSpecDto>` 增加:

```rust
marked_only: value.marked_only,
```

在 `dto.rs` 增加导航目标:

```rust
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NavigationTargetDto {
    pub line_no: u64,
    pub result_index: usize,
}

impl From<logcore::session::ResultTarget> for NavigationTargetDto {
    fn from(value: logcore::session::ResultTarget) -> Self {
        Self {
            line_no: value.line_no,
            result_index: value.result_index,
        }
    }
}
```

- [ ] **Step 3: 更新 next_bookmark 命令**

修改 imports 后,替换 `next_bookmark` 返回值:

```rust
pub fn next_bookmark(
    from_line_no: u64,
    direction: String,
    state: State<AppState>,
) -> Option<NavigationTargetDto> {
    let direction = match direction.as_str() {
        "previous" => logcore::bookmarks::BookmarkDirection::Previous,
        _ => logcore::bookmarks::BookmarkDirection::Next,
    };
    let guard = state.lock_session();
    guard
        .as_ref()
        .and_then(|session| session.next_bookmark_in_current_result(from_line_no, direction))
        .map(Into::into)
}
```

- [ ] **Step 4: 更新 TS 类型与 IPC**

在 `src/types.ts` 增加:

```ts
export interface NavigationTarget {
  lineNo: number;
  resultIndex: number;
}
```

给 `FilterSpec` 增加:

```ts
markedOnly: boolean;
```

在 `src/lib/ipc.ts` 的 import 中加入 `NavigationTarget`,并更新:

```ts
export const nextBookmark = (fromLineNo: number, direction: "next" | "previous") =>
  invoke<NavigationTarget | null>("next_bookmark", { fromLineNo, direction });
```

- [ ] **Step 5: 运行契约检查与 Rust build**

Run: 上面的 `node <<'NODE'...` 脚本。

Expected: 输出为空且 exit 0。

Run: `cargo build --workspace`

Expected: 通过。

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/dto.rs src-tauri/src/commands.rs src/types.ts src/lib/ipc.ts
git commit -m "feat(ipc): expose marked filters and result navigation"
```

---

### Task 4: 前端状态与表格当前结果模型

**Files:**
- Modify: `src/store/session.ts`
- Modify: `src/App.tsx`
- Modify: `src/components/LogTable.tsx`
- Modify: `src/components/StatusBar.tsx`

- [ ] **Step 1: 写前端状态检查并确认失败**

Run:

```bash
node <<'NODE'
const fs = require('fs');
const store = fs.readFileSync('src/store/session.ts', 'utf8');
const table = fs.readFileSync('src/components/LogTable.tsx', 'utf8');
const status = fs.readFileSync('src/components/StatusBar.tsx', 'utf8');
const failures = [];
if (!store.includes('markedOnly: false')) failures.push('DEFAULT_FILTER must include markedOnly false');
if (!store.includes('selectedResultIndex')) failures.push('store must track selectedResultIndex');
if (!table.includes('getRows("filtered"')) failures.push('LogTable must always fetch current filtered result');
if (!table.includes('setSelectedResultIndex')) failures.push('LogTable must record result index on row click');
if (!status.includes('当前结果')) failures.push('StatusBar must show 当前结果');
if (failures.length) {
  console.error(failures.join('\n'));
  process.exit(1);
}
NODE
```

Expected: 失败。

- [ ] **Step 2: 更新 zustand 状态**

在 `SessionState` 增加:

```ts
selectedResultIndex: number | null;
setSelectedResultIndex: (index: number | null) => void;
selectRow: (line: number | null, resultIndex: number | null) => void;
```

在初始状态和 `beginSession` 中设置:

```ts
selectedResultIndex: null,
```

在 `DEFAULT_FILTER` 加:

```ts
markedOnly: false,
```

增加方法:

```ts
setSelectedResultIndex: (index) => set({ selectedResultIndex: index }),
selectRow: (line, resultIndex) => set({ selectedLine: line, selectedResultIndex: resultIndex }),
```

保留旧 `setSelectedLine` 给搜索等旧路径使用:

```ts
setSelectedLine: (line) => set({ selectedLine: line }),
```

- [ ] **Step 3: 简化 App 过滤 effect**

在 `isFilterSpecActive` 中加入:

```ts
filter.markedOnly ||
```

移除自动 `setView("filtered")` / `setView("all")` 逻辑。过滤 effect 只负责:

```ts
setFilter(filter)
  .then((count) => {
    if (useSession.getState().filterRevision !== requestedRevision) return;
    setFilteredLines(count);
  })
  .catch((err) => {
    console.error("set_filter failed", err);
  });
```

F2/F3 更新为使用导航目标:

```ts
nextBookmark(selectedLine ?? 1, direction).then((target) => {
  if (target) {
    useSession.getState().selectRow(target.lineNo, target.resultIndex);
  }
});
```

- [ ] **Step 4: 让 LogTable 始终读取当前结果**

删除按 `view` 分支选择 total 的逻辑,改为:

```ts
const total = useSession((s) => s.status.filteredLines);
const selectedResultIndex = useSession((s) => s.selectedResultIndex);
const selectRow = useSession((s) => s.selectRow);
```

请求行固定为:

```ts
const rows = await getRows("filtered", block, WINDOW);
```

缓存清理依赖移除 `view`,保留:

```ts
}, [sessionId, bookmarkRevision, filterResultRevision]);
```

按结果索引滚动:

```ts
useEffect(() => {
  if (selectedResultIndex == null) return;
  rv.scrollToIndex(Math.max(0, selectedResultIndex), { align: "center" });
}, [selectedResultIndex, rv]);
```

点击行:

```tsx
onClick={() => row && selectRow(row.lineNo, vi.index)}
```

选中判断:

```ts
const selected =
  vi.index === selectedResultIndex ||
  row?.lineNo === selectedLine ||
  row?.lineNo === currentSearchLine;
```

- [ ] **Step 5: 更新 StatusBar**

删除 `viewLabel`,改为:

```tsx
const markedOnly = useSession((s) => s.filter.markedOnly);
```

显示:

```tsx
<span className="lf-status-accent">
  当前结果 {status.filteredLines.toLocaleString()} 行{markedOnly ? " · 仅标记" : ""}
</span>
```

右侧设备状态固定为:

```tsx
当前结果
```

- [ ] **Step 6: 运行检查与 build**

Run: 上面的 `node <<'NODE'...` 脚本。

Expected: exit 0。

Run: `pnpm build`

Expected: 通过。

- [ ] **Step 7: 提交**

```bash
git add src/store/session.ts src/App.tsx src/components/LogTable.tsx src/components/StatusBar.tsx
git commit -m "feat(ui): make table render current filter results"
```

---

### Task 5: 工具栏 UI 合并

**Files:**
- Modify: `src/components/Toolbar.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: 写 UI 结构检查并确认失败**

Run:

```bash
node <<'NODE'
const fs = require('fs');
const toolbar = fs.readFileSync('src/components/Toolbar.tsx', 'utf8');
const css = fs.readFileSync('src/index.css', 'utf8');
const failures = [];
for (const text of ['>全部<', '>过滤<', '>书签<', '>错误<', '>全级别<']) {
  if (toolbar.includes(text)) failures.push(`old lower view button remains: ${text}`);
}
if (!toolbar.includes('Bookmark')) failures.push('Toolbar must import/use Bookmark icon');
if (!toolbar.includes('仅标记')) failures.push('Toolbar must render 仅标记');
if (!toolbar.includes('setFilter({ levels: ALL_LEVELS })')) failures.push('Toolbar must render 全部 as level all-select');
if (!toolbar.includes('markedOnly')) failures.push('Toolbar must toggle markedOnly');
if (!css.includes('lf-level-all')) failures.push('CSS must separate 全部 from level chips');
if (failures.length) {
  console.error(failures.join('\n'));
  process.exit(1);
}
NODE
```

Expected: 失败。

- [ ] **Step 2: 更新 Toolbar imports**

在 lucide import 中加入:

```ts
Bookmark,
```

- [ ] **Step 3: 重写等级芯片组**

把当前 `.lf-level-chips` 内容改为先渲染 `全部`,再渲染等级,最后渲染 `仅标记`:

```tsx
<div className="lf-level-chips">
  <button
    className="lf-level-chip lf-level-all"
    data-active={filter.levels === ALL_LEVELS}
    type="button"
    onClick={() => useSession.getState().setFilter({ levels: ALL_LEVELS })}
  >
    <b>全部</b>
  </button>
  {LEVELS.map(([level, bit]) => {
    const on = (filter.levels & bit) !== 0;
    return (
      <button
        key={level}
        className="lf-level-chip"
        data-level={level}
        data-active={on}
        type="button"
        onClick={() => toggleLevel(bit)}
      >
        <span />
        <b>{level}</b>
      </button>
    );
  })}
  <button
    className="lf-level-chip lf-marked-only-chip"
    data-active={filter.markedOnly}
    type="button"
    onClick={() => useSession.getState().setFilter({ markedOnly: !filter.markedOnly })}
  >
    <Bookmark />
    <b>仅标记</b>
  </button>
</div>
```

- [ ] **Step 4: 删除下方视图按钮**

把 `.lf-filter-title` 中的按钮全部删除,保留:

```tsx
<div className="lf-filter-title">
  <span>过滤条件</span>
</div>
```

- [ ] **Step 5: 增加 CSS 间距与标记芯片**

在 `src/index.css` 中添加:

```css
.lf-level-all {
  margin-right: 5px;
}

.lf-level-all b {
  font-family: inherit;
  font-size: 11px;
}

.lf-marked-only-chip {
  margin-left: 5px;
  color: var(--lf-accent-strong);
}

.lf-marked-only-chip svg {
  width: 12px;
  height: 12px;
  fill: none;
  stroke: currentColor;
}

.lf-marked-only-chip[data-active="true"] {
  opacity: 1;
  border-color: var(--lf-accent);
  background: var(--lf-accent-soft);
  color: var(--lf-accent-strong);
}
```

- [ ] **Step 6: 运行检查与 build**

Run: 上面的 `node <<'NODE'...` 脚本。

Expected: exit 0。

Run: `pnpm build`

Expected: 通过。

- [ ] **Step 7: 提交**

```bash
git add src/components/Toolbar.tsx src/index.css
git commit -m "feat(ui): unify level and marked filter controls"
```

---

### Task 6: 小地图当前结果与连续段

**Files:**
- Modify: `src/components/Minimap.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: 写小地图结构检查并确认失败**

Run:

```bash
node <<'NODE'
const fs = require('fs');
const minimap = fs.readFileSync('src/components/Minimap.tsx', 'utf8');
const css = fs.readFileSync('src/index.css', 'utf8');
const failures = [];
if (!minimap.includes('bucketRanges')) failures.push('Minimap must merge adjacent buckets into ranges');
if (!minimap.includes('status.filteredLines')) failures.push('Minimap must use current result count');
if (minimap.includes('setView("all")')) failures.push('Minimap click must not reset to all view');
if (!minimap.includes('setSelectedResultIndex')) failures.push('Minimap click must set selected result index');
if (!css.includes('lf-minimap-segment')) failures.push('CSS must define minimap segments');
if (failures.length) {
  console.error(failures.join('\n'));
  process.exit(1);
}
NODE
```

Expected: 失败。

- [ ] **Step 2: 增加 bucketRanges helper**

在 `Minimap.tsx` 中新增:

```ts
function bucketRanges(buckets: number[]) {
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

function rangeStyle(range: { start: number; end: number }) {
  const start = (range.start / BUCKETS) * 100;
  const end = ((range.end + 1) / BUCKETS) * 100;
  return {
    top: `${start}%`,
    height: `${Math.max(0.7, end - start)}%`,
  };
}
```

- [ ] **Step 3: 改用当前结果计数和结果索引**

在组件中读取:

```ts
const filterResultRevision = useSession((s) => s.filterResultRevision);
const selectedResultIndex = useSession((s) => s.selectedResultIndex);
const setSelectedResultIndex = useSession((s) => s.setSelectedResultIndex);
const resultCount = status.filteredLines;
```

effect 依赖包含:

```ts
[status.totalBytes, status.filteredLines, status.errorLines, sessionId, bookmarkRevision, filterResultRevision]
```

viewport:

```ts
const viewportTop = resultCount
  ? Math.min(92, Math.max(0, ((selectedResultIndex ?? 0) / resultCount) * 100))
  : 0;
```

点击:

```ts
if (!resultCount) return;
const rect = event.currentTarget.getBoundingClientRect();
const frac = (event.clientY - rect.top) / rect.height;
const resultIndex = Math.min(resultCount - 1, Math.max(0, Math.floor(frac * resultCount)));
setSelectedResultIndex(resultIndex);
```

- [ ] **Step 4: 渲染连续段**

替换 tick map:

```tsx
{bucketRanges(data.bookmarks).map((range) => (
  <span
    className="lf-minimap-segment lf-minimap-bookmark"
    key={`b-${range.start}-${range.end}`}
    style={rangeStyle(range)}
  />
))}
{bucketRanges(data.errors).map((range) => (
  <span
    className="lf-minimap-segment lf-minimap-error"
    key={`e-${range.start}-${range.end}`}
    style={rangeStyle(range)}
  />
))}
```

- [ ] **Step 5: 增加 CSS**

在 `src/index.css` 中增加:

```css
.lf-minimap-segment {
  position: absolute;
  border-radius: 2px;
}

.lf-minimap-segment.lf-minimap-bookmark {
  left: 2px;
  width: 9px;
  background: var(--lf-accent);
}

.lf-minimap-segment.lf-minimap-error {
  right: 2px;
  width: 11px;
  background: var(--lf-lv-e);
}
```

保留旧 `.lf-minimap-tick` 不影响构建,后续可清理。

- [ ] **Step 6: 运行检查与 build**

Run: 上面的 `node <<'NODE'...` 脚本。

Expected: exit 0。

Run: `pnpm build`

Expected: 通过。

- [ ] **Step 7: 提交**

```bash
git add src/components/Minimap.tsx src/index.css
git commit -m "feat(ui): map current results in minimap"
```

---

### Task 7: 最终验证与自审

**Files:**
- Review-only: changed files from Tasks 1-6

- [ ] **Step 1: 运行完整验证**

Run:

```bash
cargo test -p logcore
cargo build --workspace
pnpm build
```

Expected:

- `cargo test -p logcore`: 全部测试通过。
- `cargo build --workspace`: 通过。
- `pnpm build`: 通过。

- [ ] **Step 2: 对抗式自审**

Run:

```bash
rg "getRows\\(|get_rows|MAX_ROWS|WINDOW|markedOnly|marked_only|RowsView::Bookmarks|RowsView::Errors|setView\\(" src src-tauri crates/logcore/src -n
```

检查结论必须满足:

- `get_rows` 仍有 `MAX_ROWS` 限制。
- 前端热路径仍只用 `WINDOW = 200` 请求可见窗口。
- UI 不再暴露 `全部 / 过滤 / 书签 / 错误 / 全级别` 视图按钮。
- `RowsView::Bookmarks` 和 `RowsView::Errors` 若仍存在,只作为兼容内部能力,不是主 UI 路径。
- 小地图不调用 `setView("all")`。

- [ ] **Step 3: 如有 Critical/Important 问题,先修复再继续**

修复时使用独立 commit,提交名格式:

```bash
git commit -m "fix(ui): <具体问题>"
```

- [ ] **Step 4: 里程碑提交**

若工作树干净且所有验证通过:

```bash
git commit --allow-empty -m "milestone: complete unified filter controls"
```

- [ ] **Step 5: 汇报**

最终回复包含:

- 做了什么。
- 验证命令结果。
- 设计偏差: 无,按批准 spec 和 UI 设计稿风格实现。
- 待 GUI 复核点: `全部` 与等级间距、`仅标记` 图标/芯片、连续小地图红蓝条、F2/F3 当前结果跳转。
