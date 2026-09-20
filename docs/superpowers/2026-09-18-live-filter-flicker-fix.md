# 实时筛选日志区域闪烁修复

## 现象与原因

录屏中，adb 抓取开启 Tag 筛选后，历史日志与整屏 `...` 占位行反复交替，滚动位置基本不变。

`stream:append` 携带增长的 `filterResultRevision`，使 LogTable 的结果集 cache key 改变。
此前每次改变都会清空行缓存；新的 `get_rows_checked` 尚未返回时，已显示的日志全部变成占位行。
此外，旧纪元请求的 `finally` 会无条件删除同块的 in-flight 登记，可能误删新请求，造成重复取行。

## 改动

- 区分“结果版本刷新”和“数据身份变化”。同一会话、分析代号、编码、视图、已应用筛选下的追加刷新只递增缓存纪元，保留已显示的行，窗口响应到达后替换对应块。
- 会话、分析代号、编码、视图或已应用筛选变化时，仍在绘制前清空旧缓存；不让别的数据集混入当前视图。
- 保留完整的 `filterResultRevision` 请求校验与迟到响应拒绝；旧纪元请求结束时不能删除新纪元的 in-flight 登记。
- 不改变 UI 布局、尾随状态机、IPC 契约、200 行窗口或 64 块 LRU 上限。

## 回归测试

先新增 `src/components/LogTable.liveRows.test.tsx`，再修改实现。修复前两项失败：

1. 初次加载完成后，追加状态一更新，可见的 `history-1` 消失，DOM 变成 `...`。
2. 旧请求结束后再渲染，预期总请求数 3，实际为 4。

修复后两项通过；另覆盖会话、分析代号、编码和已应用筛选切换时清除旧行。

以下检查通过：

```sh
pnpm typecheck
pnpm lint
pnpm exec vitest run src/components/LogTable.liveRows.test.tsx \
  src/components/LogTable.test.tsx src/lib/rowCache.test.ts src/lib/table.test.ts \
  src/lib/tableScopeController.test.ts src/lib/sessionTableScope.test.ts \
  src/store/session.test.ts src/lib/streamAppend.test.ts src/App.tableScope.test.tsx
```

共 9 个测试文件、70 项测试通过。独立代码审查未发现必须修复的问题。

## 浏览器对照

在同一台 macOS 开发机、同一 Chrome 会话中运行真实 React LogTable、FilterBar、TanStack Virtual 和项目 CSS，使用合成数据与延迟 IPC 响应。初始 600 行，其中 300 行匹配；每轮 32 次追加，每次增加 8 个源行，间隔 75ms，取行响应延迟 35ms，视口为 1440×900。

用 `requestAnimationFrame` 采样实际视口内的行；当可见行全部为 `.lf-loading-row` 时，计为整屏占位帧。修复前组件取自改动前的仓库版本，修复后组件取自当前工作区，其余条件相同。每个版本连续运行三轮：

| 版本 / 轮次 | 采样帧 | 整屏占位帧 | 含占位行帧 | 窗口请求数 |
| --- | ---: | ---: | ---: | ---: |
| 修复前 1 | 153 | 75 | 75 | 32 |
| 修复前 2 | 154 | 74 | 74 | 32 |
| 修复前 3 | 153 | 72 | 72 | 32 |
| 修复后 1 | 152 | 0 | 0 | 32 |
| 修复后 2 | 151 | 0 | 0 | 32 |
| 修复后 3 | 151 | 0 | 0 | 32 |

三轮中滚动位置均保持不变，每次取行最大 200 行。另将请求延迟放大后截图，确认修复前为整屏占位，修复后仍显示历史日志。

补充交互检查：切换 Tag 后可见行全部属于新标签；输入无匹配标签后显示空结果；恢复筛选并开启“追最新”后，连续 32 次追加仍停留在底部，154 个采样帧中无整屏占位或局部占位。该临时页面为“追最新”提供测试回调，实际应用导航由相关单元测试覆盖。

这是前端显示行为的受控对照，不是实际 adb 吞吐、10GiB 引擎基准或桌面 WebView 真机验收。本次没有修改 Rust 引擎，也未重跑无关的引擎吞吐基准；实际设备和其他平台仍需后续复测。录屏中的真实日志与设备信息没有复制进仓库。
