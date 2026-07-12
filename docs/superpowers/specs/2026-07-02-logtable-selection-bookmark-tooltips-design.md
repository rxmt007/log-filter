# LogTable 选择、批量标记与按钮提示设计

日期: 2026-07-02

## 背景

当前 LogFilter 已支持拖选复制、双击单行标记和基础按钮 `title`。但实际使用中还有三个交互问题:

1. 工具栏和级别按钮缺少一致的 hover tips,尤其 `V/D/I/W/E/F` 对新用户不直观。
2. 多行选择后只能双击单行标记,无法对当前选中范围批量打 bookmark。
3. 当前拖选高亮依赖原生文本 selection 命中的具体 DOM 行,在 W/E/F 等级底色行上会出现视觉断层,看起来像某些行没有被选中。

## 目标

- 增加统一的轻量 tooltip 样式,用于常用工具栏按钮、级别按钮、搜索按钮和表格列按钮。
- 将表格拖选改为连续的 `selectionRange`,使选区视觉、复制和右键批量标记使用同一组行。
- 多行选中后右键提供菜单,支持批量“标记选中行”和“取消标记”。
- 保持架构铁律:不新增整文件 IPC,不把整文件传给前端,只基于当前可见虚拟行和缓存行操作。
- 保持 `logcore` 与 UI 解耦;本次只改前端交互层和现有 Tauri 单行 bookmark IPC 的调用方式。

## 交互设计

### Tooltip

使用统一 `data-tooltip` + CSS 的轻量 tooltip。

覆盖范围:

- 工具栏按钮:
  - Start、Pause、Stop、Clear
  - Open file、Export、Split file、Settings、Theme
- 日志级别:
  - All levels
  - Verbose、Debug、Info、Warning、Error、Fatal
  - Marked only
- 搜索和表格:
  - Case sensitive、Regex search、Previous match、Next match
  - Highlight color
  - Show columns

所有元素保留 `title` 或 `aria-label` 作为可访问性兜底;可见 tooltip 由 CSS 控制,hover 和 focus 都能触发。

### 连续选区模型

现有逻辑通过 `Selection.intersectsNode(rowElement)` 收集被原生文本选中的行。新逻辑改为:

1. 先找出与原生 selection 相交的可见行。
2. 取这些行的最小 / 最大 result index。
3. 形成连续闭区间 `selectionRange = { start, end }`。
4. 当前可见行只要 `start <= index <= end`,就输出 `data-copy-selected="true"`。
5. 复制时也按该连续区间内的缓存行顺序输出,每行内部继续使用 `str1  str2 ...` 格式。

这样 W/E/F 行、标记行和普通行都会在选区范围内保持连续蓝色背景,不会出现视觉空洞。

### 右键批量 bookmark

右键行为:

- 如果右键位置在当前连续选区内,菜单作用于整个连续选区。
- 如果右键位置不在当前选区内,菜单作用于右键所在单行。
- 菜单显示在鼠标位置附近,点击页面其他位置或按 Esc 关闭。

菜单项:

- `标记选中行`:只标记当前范围内未标记的行。
- `取消标记`:只取消当前范围内已标记的行。

实现上继续复用现有 `toggleBookmark(lineNo)` IPC。为了避免批量 toggle 反向污染状态,执行前先看缓存中的 `row.marked`,只对需要变化的行调用 IPC。

## 视觉设计

- 当前方案 B 的选区蓝色和文本覆盖保留。
- 连续选区内 W/E/F/marked 行全部显示选区背景,等级色不再打断蓝色范围。
- 选区文字不再过重:将 font-weight 从 `650` 调整到 `600`,保留清晰度但降低大面积拖选时的压迫感。
- 右键菜单使用项目现有面板色、边框和 6-7px 圆角,不做大面积卡片化。

## 非目标

- 不实现跨虚拟窗口的大范围拖选。当前基于浏览器原生选择和可见虚拟行,只保证当前可见范围内连续。
- 不新增快捷键。
- 不新增后端批量 bookmark IPC;如后续性能需要,再单独设计。
- 不改变导出选中范围逻辑。

## 验证

- 结构契约脚本应检查:
  - 存在 tooltip 合约和 CSS。
  - LogTable 存在连续 selection range 逻辑。
  - 复制使用连续 selection rows。
  - 存在右键菜单和批量 bookmark 操作。
- `pnpm build` 必须通过。
- `cargo test -p logcore` 与 `cargo build --workspace` 必须保持通过。
- GUI 复核点:
  - hover 级别按钮能看到完整名称。
  - 拖选跨 W/E/F 行时蓝色选区连续。
  - 右键选区后可以批量标记 / 取消标记,小地图和行 bookmark 状态随之刷新。
