# 日志表格与小地图交互修正设计

日期:2026-07-02

状态:用户已确认,进入实施计划与实现

关联规范:

- `docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md`
- `docs/superpowers/specs/2026-07-01-logfilter-unified-filter-controls-design.md`
- `docs/superpowers/specs/2026-07-01-minimap-drag-follow-design.md`
- `docs/design/LogFilter.dc.html`

## 1. 背景

当前 M2/M4/M5 已具备统一过滤、书签、小地图和设置能力,但在高频交互上仍有几个会打断用户阅读节奏的问题:

1. 点击小地图蓝色视口框内部时,蓝框上边缘会先瞬移到鼠标位置,再进入拖动。
2. 双击日志行打标时,中间日志区域会跳回首屏;在首屏附近双击时还可能显示大量 `...` 占位。
3. 单击可见日志行时,行会被强制滚动到屏幕中央。
4. 修改过滤字段时,非首屏会跳回首屏;首屏附近会出现大量 `...`。
5. 日志表格列宽不能调整,列也不能隐藏/显示。

这些问题有共同根因:当前前端把“选中行”“需要滚动到某行”“过滤结果刷新”“书签数量刷新”混在同一批状态里处理,导致虚拟表格在不需要重置滚动位置时也清空缓存并滚到顶部。小地图的框内拖动也复用了轨道点击跳转算法,所以没有保留鼠标按下点相对蓝框顶部的偏移。

## 2. 目标

1. 小地图蓝框内部按下时不瞬移,直接以当前按下点为抓取点开始拖动。
2. 小地图蓝框外或轨道空白处按下时,保留现有“点击直接跳转,继续拖动继续跟手”的能力。
3. 小地图拖动期间,蓝框和表格都实时跟手。
4. 单击表格行只更新选中态,不触发自动居中。
5. F2/F3、搜索结果跳转、小地图点击/拖动等“导航动作”仍能滚动表格。
6. 双击打标不重置滚动位置,也不触发大面积 `...`。
7. 修改过滤条件后保持当前阅读锚点;如果旧位置超出新结果范围,只做边界内夹取,不回到首屏。
8. 表格列宽可拖拽调整,列可通过紧凑 UI 隐藏/显示。
9. 列宽和列显隐写入 TOML 配置,下次启动恢复。
10. 全程保持“只传可见窗口”铁律:前端仍只通过 `get_rows("filtered", start, count)` 获取有限窗口,count 不超过后端上限。

## 3. 非目标

1. 不实现列拖拽排序。配置中可预留顺序字段,但本轮不提供重排交互。
2. 不把表格替换为 shadcn Data Table。
3. 不改变 `logcore` 的日志解析、过滤、搜索算法。
4. 不新增 adb、三平台打包或 CI 能力。
5. 不做真机/emulator 手动验证。

## 4. 根因与修正方向

### 4.1 小地图蓝框瞬移

现状:小地图 `pointerdown` 一律把 `clientY` 映射为目标 `resultIndex`,等价于“把蓝框顶部挪到鼠标位置”。

修正:

- 判断 `pointerdown` 是否落在当前蓝框内部。
- 如果在蓝框内部,记录 `grabOffsetPx = pointerY - viewportTopPx`。
- 拖动时用 `pointerY - grabOffsetPx` 作为蓝框顶部,再映射为 `resultIndex`。
- 如果在蓝框外,沿用轨道点击语义:按下位置立即成为新目标,并进入拖动。

### 4.2 表格跳回首屏与大量占位

现状:`LogTable` 在 `sessionId`、`bookmarkRevision`、`filterResultRevision` 任一变化时都会清缓存并 `scrollTo({ top: 0 })`。双击打标会刷新 `bookmarkRevision`;修改过滤字段会刷新 `filterResultRevision`;两者都会触发首屏重置。重置后虚拟列表和窗口缓存短时间不匹配,就会出现大面积 `...`。

修正:

- 新会话打开文件时才回到顶部。
- 过滤结果刷新时清理旧窗口缓存,但保留当前滚动偏移或可见首行锚点。
- 书签普通刷新不清空全部行缓存;只更新被打标行的 `marked` 状态。
- 仅在 `markedOnly` 打开时,书签变化会导致过滤结果重算;此时仍按过滤刷新锚点规则处理。

### 4.3 单击行强制居中

现状:单击行同时写入 `selectedLine` 和 `selectedResultIndex`,而 `LogTable` 监听 `selectedResultIndex` 并执行 `scrollToIndex(..., align: "center")`。

修正:

- `selectedResultIndex` 只表达当前选中行在当前结果集中的位置。
- 新增独立的“滚动请求”状态,例如 `scrollRequest = { index, align, nonce }`。
- 单击行只设置选中行和选中索引,不创建滚动请求。
- F2/F3、搜索跳转、小地图点击/拖动创建滚动请求。

## 5. 交互设计

### 5.1 小地图

- 蓝框内部按下:蓝框不瞬移,指针按住的位置保持在蓝框内相同相对位置,拖动时框与表格同步移动。
- 蓝框外按下:立即跳转到对应结果位置,蓝框移动到目标附近;继续按住移动时持续跟手。
- 拖动越过小地图顶部/底部:目标索引 clamp 到 `[0, resultCount - 1]`。
- 当前结果为 0 行:小地图不响应跳转和拖动。
- 小地图仍基于当前过滤结果,蓝色书签段和红色错误段保持现有语义。

### 5.2 表格选中与导航

- 单击行:只选中该行,如果它已经在视口内,不移动表格。
- 双击行:切换书签状态,行仍留在当前视觉位置附近。
- F2/F3:跳到当前结果集内上一个/下一个书签,并把目标滚动到视口中部。
- 小地图点击/拖动:按结果集位置滚动表格。拖动期间使用高频但合并的滚动请求,避免过量状态更新。
- 搜索跳转:跳到搜索命中的源行。如果当前结果顺序等于源文件顺序,可按 `lineNo - 1` 映射到结果索引;否则保持当前已实现的源行高亮,不扫描整份结果向前端补映射。

### 5.3 过滤刷新

- 修改过滤字段、级别、仅标记后,表格使用刷新前的可见首行或当前滚动偏移作为锚点。
- 新结果数量大于 0 时,锚点索引 clamp 到新结果范围内。
- 新结果数量为 0 时,显示空态。
- 不因过滤刷新自动清除用户的源行选中态;如果选中源行不在新结果中,保留 `selectedLine` 供状态栏/搜索上下文使用,但不强制滚动。

### 5.4 列宽调整

- 每个可见列头右侧提供窄拖拽手柄。
- 拖动时实时更新 grid 列宽,表头和行内容同步变化。
- 松开鼠标后保存到配置。
- 列宽有最小值和最大值:
  - 书签列:22-36px
  - 行号:52-120px
  - 日期:48-90px
  - 时间:82-160px
  - 级别:36-60px
  - PID/TID:48-100px
  - Tag:110-260px
  - 消息:220px 起,可占据剩余空间
- 消息列默认填充剩余空间;当其它列变宽时,消息列缩到最小宽度后由横向内容裁剪承担。

### 5.5 列隐藏/显示

- 表头右侧增加一个紧凑图标按钮,使用 `Columns3` 或同类 Lucide 图标。
- 点击后展开小浮层,浮层内用复选框列出可切换列。
- 默认显示:书签、行号、日期、时间、级别、PID、TID、Tag、消息。后续如果恢复“默认隐藏书签列”的产品决策,只需改默认配置,不影响架构。
- 至少保留一个主内容列可见。推荐规则:消息列不能隐藏。
- 隐藏列后,表头和每行都移除该列,grid 模板同步重算。
- 列显隐变化立即保存到配置。

## 6. 状态与数据模型

### 6.1 前端状态

推荐新增:

```ts
interface ScrollRequest {
  index: number;
  align: "auto" | "center" | "start";
  reason: "minimap" | "bookmark" | "search";
  nonce: number;
}
```

状态规则:

- `selectedLine`:源文件行号,用于行选中和状态栏。
- `selectedResultIndex`:当前结果集索引,只用于行选中态。
- `viewportResultIndex`:当前虚拟表格可见窗口的首个结果索引,用于小地图蓝色视口框位置。
- `scrollRequest`:一次性导航请求。`nonce` 递增,允许连续请求同一行也触发表格滚动。
- `selectRow(lineNo, resultIndex)`:只选中,不滚动。
- `navigateToResultIndex(index, reason, align)`:选中结果索引并创建滚动请求。
- `setViewportResultIndex(index)`:由表格滚动位置反向更新,不触发表格滚动。

### 6.2 表格列配置

Rust TOML 配置推荐增加:

```rust
pub struct TableConfig {
    pub columns: Vec<TableColumnConfig>,
}

pub struct TableColumnConfig {
    pub id: String,
    pub width: u16,
    pub visible: bool,
}
```

TypeScript 对齐:

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

合法列 id:

- `bookmark`
- `lineNo`
- `date`
- `time`
- `level`
- `pid`
- `tid`
- `tag`
- `message`

配置加载时需要归一化:

- 缺失列补默认值。
- 未知列丢弃。
- 宽度 clamp 到列定义范围。
- `message.visible` 强制为 `true`。
- 如果所有列都被配置为隐藏,恢复默认列配置。

## 7. 架构约束

1. `logcore` 仍不依赖 UI/Tauri。
2. 表格列宽/显隐是配置和前端渲染问题,不影响 `get_rows` 返回结构。
3. 前端不可因为列显隐或过滤刷新请求整份结果。
4. `get_rows` 热路径仍使用 `WINDOW = 200`,并受后端 `MAX_ROWS = 512` 限制。
5. 书签切换不应触发表格全量数据重取,除非当前过滤条件包含 `markedOnly` 且后端结果集确实需要重算。

## 8. 测试策略

### 8.1 Rust 单测

- 默认配置包含合法表格列配置。
- TOML round trip 保留列宽和可见性。
- 加载配置时补齐缺失列、丢弃未知列、clamp 非法宽度。
- `message` 列不可被隐藏。

### 8.2 前端静态/结构测试

在没有现成 Vitest 环境时,使用脚本或 `rg` 级别的 headless 检查锁定关键结构:

- `Minimap.tsx` 有蓝框内部拖动 offset 逻辑。
- `LogTable.tsx` 不再因 `bookmarkRevision` 清缓存并滚到顶部。
- `LogTable.tsx` 不再由单击行触发滚动请求。
- 表格列由列模型渲染,不再使用固定 `COLS`/`HEADERS`。
- 表头包含列宽 resize handle。
- 表头包含列显隐按钮和复选框 UI。
- `AppConfig`/DTO/Rust config 都包含 table columns 字段。

### 8.3 构建验证

每轮完成前必须运行:

- `cargo test -p logcore`
- `cargo build --workspace`
- `pnpm build`

### 8.4 人工 GUI 复核点

由用户最终复核:

- 蓝框内部按住拖动不瞬移。
- 蓝框外点击仍可直接跳转。
- 双击 300 行以后任意行打标不跳首屏。
- 单击当前屏幕内行不自动居中。
- 修改过滤字段后不跳首屏,也不出现大面积 `...`。
- 拖动列宽时表头和行同步调整。
- 隐藏/显示列符合设计稿克制、紧凑的视觉风格。

## 9. 设计稿偏差

`docs/design/LogFilter.dc.html` 已明确表格是主角、控件克制,并在总规范 §5 中写明“列可显隐、可调宽”。本轮新增的列按钮与拖拽手柄属于功能落地细节,设计稿没有逐像素给出展开菜单形态。

偏差处理:

- 使用 28px 左右紧凑图标按钮、低圆角、细边框、Lucide 图标,与现有工具栏/表头风格保持一致。
- 不增加大面积说明文字。
- 不做装饰性卡片或复杂面板。

功能与设计稿无冲突。新增 UI 是为了完成既定功能。
