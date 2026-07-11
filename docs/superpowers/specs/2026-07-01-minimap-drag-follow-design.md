# 小地图跟手拖动设计

日期:2026-07-01

## 背景

当前左侧小地图已经按“当前结果集”渲染书签蓝段、错误红段和蓝色视口框。点击小地图时会跳到对应结果位置,但拖动时体验不够像常规滚动条:用户按住移动时,视觉上的视口框和中间日志表格没有持续跟手,需要松开或触发最终点击后才看到明显位置变化。

本设计优化小地图拖动体验,让它更接近原生 scrollbar/slider:点击哪里跳哪里,按住拖动时视口框和表格都实时跟随。

## 目标

1. 保留小地图任意位置点击跳转能力。
2. 增加按住拖动能力,拖动期间蓝色视口框实时跟随鼠标。
3. 拖动期间中间日志表格同步滚动到对应当前结果位置。
4. 继续基于当前结果集坐标,不是源文件全量坐标。
5. 不改变 `logcore` 和 Tauri 后端契约,前端仍只取可见窗口。

## 非目标

1. 不新增后端命令。
2. 不改变小地图红/蓝段渲染语义。
3. 不实现独立 scrollbar 组件或替换 TanStack Virtual。
4. 不调整 M3/M6 或 adb/CI 范围。

## 交互设计

小地图整体作为可交互轨道:

- 鼠标单击小地图任意位置:立即计算该位置对应的 `resultIndex`,更新 `selectedResultIndex`,表格滚动到该结果行。
- 鼠标按下小地图任意位置:立即跳到按下位置,并进入拖动状态。
- 按住移动:持续根据指针 Y 坐标更新 `selectedResultIndex`;蓝色视口框和表格实时跟手。
- 鼠标松开、取消、窗口失焦或 pointer capture 结束:退出拖动状态,停在最后位置。
- 当前结果为 0 行时:点击和拖动都不产生动作。

允许从小地图任意位置开始拖动,不要求精确点中蓝色视口框。原因是小地图宽度很窄,要求命中蓝框会降低可用性。

## 技术设计

### Minimap

`src/components/Minimap.tsx` 从 `onClick` 升级为 Pointer Events:

- 使用 `onPointerDown` 开始交互,调用 `event.currentTarget.setPointerCapture(event.pointerId)`。
- 使用 `onPointerMove` 在拖动状态下持续更新结果索引。
- 使用 `onPointerUp`、`onPointerCancel` 和 capture 丢失处理结束拖动。
- 将 “clientY -> resultIndex” 提取为纯 helper,便于测试和复用。
- 使用 `requestAnimationFrame` 合并高频 pointermove,避免一次拖动触发过多 zustand 更新。
- 保留 track click 语义:因为 `pointerdown` 已经立即更新索引,短点击也会直接跳转。

### LogTable

`LogTable` 已监听 `selectedResultIndex` 并调用 TanStack Virtual 的 `scrollToIndex`。本迭代不改变表格数据获取模型:

- 表格仍固定调用 `getRows("filtered", block, WINDOW)`。
- `WINDOW` 仍为 200。
- 后端 `get_rows` 仍有 `MAX_ROWS = 512` 上限。

拖动时如果目标行所在窗口尚未加载,虚拟列表会先滚动到该 index,现有 `ensureBlock` 会按可见范围请求对应窗口。这样仍满足“只传可见窗口”铁律。

### 视觉状态

拖动时可给小地图增加 `data-dragging="true"`:

- 鼠标 cursor 保持可拖动语义。
- 视口框可略微增强边框或背景,表示正在拖动。

该状态只用于视觉反馈,不参与核心逻辑。

## 边界条件

1. `resultCount <= 0`:不更新。
2. 指针 Y 超出小地图上下边界:clamp 到 `[0, resultCount - 1]`。
3. 快速拖动到尚未加载行:允许虚拟列表先滚动,数据按现有窗口机制补齐。
4. 过滤结果变化时:现有状态会清理过期 `selectedResultIndex`;拖动逻辑不额外保留旧索引。
5. pointer capture 失败或取消:退出拖动状态即可,不抛错。

## 测试与验证

前端结构检查:

- `Minimap.tsx` 包含 `onPointerDown`、`onPointerMove`、`requestAnimationFrame` 和 `setPointerCapture`。
- `Minimap.tsx` 不再依赖 `onClick` 作为唯一交互入口。
- `Minimap.tsx` 不调用 `setView("all")`。
- `LogTable.tsx` 仍通过 `selectedResultIndex` 滚动。

构建验证:

- `cargo test -p logcore`
- `cargo build --workspace`
- `pnpm build`

人工 GUI 复核:

- 单击小地图非蓝框位置,表格立即跳转。
- 按住小地图拖动,蓝框与表格实时跟手。
- 快速拖动到顶部/底部不会越界。
- 空结果时小地图不误跳。

## 设计偏差

无。该优化不改变既有视觉基准,只补足小地图交互手感。功能与设计稿不冲突。
