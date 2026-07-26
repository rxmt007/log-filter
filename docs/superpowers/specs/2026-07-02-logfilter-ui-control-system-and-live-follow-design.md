# LogFilter 主界面控件系统与实时跟随优化设计

日期: 2026-07-02

状态: 设计已确认,待用户复核书面 spec

关联规范:

- `docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md`
- `docs/superpowers/specs/2026-07-01-logfilter-unified-filter-controls-design.md`
- `docs/superpowers/specs/2026-07-02-logtable-minimap-interaction-design.md`
- `docs/design/LogFilter.dc.html`
- `docs/design/LogWindow.dc.html`

## 1. 背景

当前主界面已经具备 adb 实时抓取、打开文件、过滤、搜索、书签、小地图和设置能力,但几个交互点会让用户心智变复杂:

1. 顶部同时有"来源"下拉和"打开文件"按钮,打开文件入口重复;当当前会话是文件时,点击"开始"再进入 adb 抓取的语义不够直接。
2. 多处仍使用系统原生 `select`,macOS 下弹出菜单视觉与 LogFilter 自身设计稿不一致。
3. 拖入 log 文件当前不稳定,但空态仍可能让用户以为这是主入口。
4. 明亮模式下 ghost/icon 按钮 hover 反馈偏弱,不如暗色模式明显。
5. adb 实时抓取时需要更可靠的"自动贴底"语义:默认持续跟随尾部,但用户查看历史日志时不能被新日志抢回底部。

用户已确认本轮采用"趁早收束主界面控件系统"方向,而不是只做局部补丁。

## 2. 目标

1. 去掉顶部"来源"下拉,默认使用 adb 语境;打开文件只通过工具栏按钮、快捷键和最近文件入口完成。
2. 顶部职责调整为"操作",底部状态栏职责调整为"当前会话来源状态"。
3. 将所有可见下拉控件替换为统一自绘控件,不再使用系统样式菜单。
4. 命令控件做成 Select + Input 的 combobox,允许输入新命令并保存为配置预设。
5. 移除拖拽打开行为和相关文案承诺。
6. 增强明亮模式下按钮 hover 可见度,同时保持现有设计稿的克制风格。
7. 明确 adb 实时抓取的 tail-follow 状态机:默认贴底,用户主动离开尾部则暂停,手动回到底部后恢复。
8. 保持架构铁律:前端仍只通过 `get_rows(view, start, count)` 获取可见窗口,不新增整文件传输。

## 3. 非目标

1. 不实现过滤器预设。
2. 不做字体族选择。
3. 不做过滤栏折叠。
4. 不做自定义滚动条。
5. 不拆分 `LogTable` / `session.rs`。
6. 不开放任意 shell 命令执行;命令输入仅用于受限的 `logcat` 抓取模板。
7. 不做多标签会话。

## 4. 信息架构

### 4.1 顶部工具栏

顶部第一行去掉"来源"下拉,只保留:

- 设备下拉:显示当前 adb 设备,支持刷新后的在线设备列表。
- 命令 combobox:显示当前 logcat 命令,支持选择预设和输入新命令。

工具栏动作行保留:

- 运行 / 暂停 / 停止 / 清空
- 打开文件
- 导出
- 切分
- 设置
- 主题
- 级别芯片
- 搜索框
- 跳转行号

点击"开始"时,无论当前是空态还是已打开文件,都进入 adb 实时抓取流程。文件会话被新的 adb 会话替换;状态栏同步显示 adb 来源。

### 4.2 文件入口

文件入口保留:

- 工具栏"打开文件"按钮。
- `Ctrl/Cmd+O` 快捷键。
- 最近文件入口。最近文件可放入打开文件菜单或自绘下拉/弹层中,但不再挂在"来源"选择上。

拖拽打开从本轮功能面移除:

- 删除全局 `dragover` / `drop` 打开监听。
- 空态文案不再出现"拖入"。
- 空态只展示"打开文件"和"从设备抓取"两个主动作。

### 4.3 底部状态栏来源显示

状态栏最右侧显示当前会话来源:

- adb 模式:`adb · <device id>`
- 文件模式:`file · ~/path/.../name.log`
- 空态:`未打开文件 · 就绪` 或设计稿等价空态文案

路径显示规则:

- 用户 home 目录前缀显示为 `~/`。
- 路径过长时使用中间省略,保留开头和文件名。
- hover tooltip / `title` 保留完整绝对路径。
- 该显示只负责状态反馈,不是文件选择入口。

## 5. 自绘控件系统

本轮替换所有可见 `select`,包括:

- 顶部设备选择。
- 顶部命令选择 / 输入。
- 设置弹窗中的编码选择。
- 导出弹窗中的导出视图选择。
- 切分弹窗中的切分模式选择。
- 高亮颜色选择。

### 5.1 基础控件

建议新增或收束以下 UI 组件:

#### `DropdownButton`

用途:主界面和紧凑区域的下拉触发器。

能力:

- label + 当前值 + chevron。
- 可显示图标、在线状态点、色块。
- disabled / loading / error 状态。
- hover / focus-visible / active 视觉状态。

#### `DropdownMenu`

用途:所有浮层菜单的通用容器。

能力:

- 分组标题。
- checked item。
- icon / 色块 / 快捷键文本。
- 空状态。
- 删除自定义项等行内轻操作。
- 点击外部关闭。
- Esc 关闭。
- 键盘上下移动,Enter 选择。

菜单视觉:

- 使用项目面板色、边框、阴影。
- 圆角 6-8px。
- 不使用系统蓝色大浮层。
- 浅色和深色模式共享 token,但各自保证 hover 可见。

#### `SelectField`

用途:表单弹窗内的选择控件。

能力:

- 外观像输入框。
- 与 `DropdownMenu` 共享菜单逻辑。
- 支持 label、value、disabled、error/help 文案。

#### `ColorSelect`

用途:高亮颜色选择。

能力:

- 触发器显示色块 + 名称。
- 菜单项显示色块、颜色名、当前勾选。

### 5.2 CommandCombobox

命令控件不是普通 Select,而是 Select + Input 的 combobox。

交互:

- 输入框显示当前命令,使用等宽字体。
- 点击 chevron 展开预设列表。
- 用户可直接输入新命令。
- 按 Enter 或点击"添加为预设"后,将合法新命令保存到配置。
- 预设列表展示默认预设与自定义预设。
- 当前命令项显示 checked 状态。
- 自定义预设可删除;默认预设不可删除。
- 自定义预设去重,最多保存 20 条。

默认预设:

- `logcat -v threadtime -b main`
- `logcat -v threadtime -b main -b system -b crash -b events`
- `logcat -v threadtime -b system`
- `logcat -v threadtime -b radio`
- `logcat -v threadtime -b events`
- `logcat -v threadtime -b crash`

命令范围:

- v1 仅允许受限 `logcat` 命令。
- 必须是 `logcat`。
- 格式必须使用 `-v threadtime`。
- buffer 仅允许 `main` / `system` / `radio` / `events` / `crash`。
- 可重复使用 `-b`;重复 buffer 去重并保留首次出现顺序。
- 如果未写 `-b`,归一化为确定的 `main` + `system` + `crash`,不依赖不同 Android
  版本的隐式默认值。
- 不接受 `-b all`;需要多 buffer 时使用显式列表,使抓取覆盖范围可记录、可解释。
- 不执行任意 shell,不支持管道、重定向、`shell`、`&&` 等复合命令。

解析结果:

- 前端可先解析并给出 UI 错误。
- Tauri/Rust 命令层仍需做二次校验,避免绕过 UI。
- 后端实际 spawn 仍使用结构化参数,不要把用户输入原样拼接进 shell。

## 6. 配置与迁移

现有配置已有 `commandBuffers: Vec<String>`。本轮扩展为更适合命令 combobox 的结构。

推荐配置:

```toml
current_command = "logcat -v threadtime -b main"
command_presets = [
  "logcat -v threadtime -b main",
  "logcat -v threadtime -b main -b system -b crash -b events",
  "logcat -v threadtime -b radio",
]
```

迁移规则:

1. 读取旧配置时,如果存在 `commandBuffers`,把其中所有合法 buffer 按原顺序去重后生成
   一条 `current_command`。
2. 旧 `commandBuffers` 中合法 buffer 转成对应默认命令,合并进 `command_presets`。
3. 默认预设始终可用,即使用户配置缺失。
4. 保存新配置时写入 `current_command` 和 `command_presets`。
5. 可保留 `commandBuffers` 兼容读取,并让它与 `current_command` 中的完整 buffer 列表同步；
   新 UI 不再直接编辑它。

## 7. Tail-Follow 状态机

### 7.1 基本规则

adb 开始抓取后:

- 默认 `tailFollowing = true`。
- 收到 `stream:append` 且当前结果数大于 0 时,自动滚到最后一行。

暂停自动跟随的动作:

- 用户向上滚动。
- 用户单击 / 选中某一行。
- 搜索上一个 / 下一个跳转。
- F2 / F3 书签跳转。
- 小地图点击 / 拖动。
- 跳转行号。

恢复自动跟随的动作:

- 用户手动滚动到当前结果集底部附近。
- "底部附近"建议定义为最后可见结果索引 `>= total - 2`,沿用现有实现的容差。

暂停后:

- 新日志 append 只更新状态和可见窗口缓存。
- 不抢走用户视口。
- 如果用户再次滚到底部,后续 append 继续贴底。

文件模式:

- 不需要实时贴底语义。
- `tailFollowing` 仅影响 adb append 自动滚动。

### 7.2 状态与事件建议

前端 store 可继续保留 `tailFollowing`,但建议明确区分更新来源:

- `setTailFollowingFromViewport(isAtBottom)` 由表格滚动位置调用。
- `pauseTailFollowing(reason)` 由用户导航或选择动作调用。
- `beginSession(..., "adb")` 初始化为 `true`。
- `beginSession(..., "file")` 可置为 `false` 或保持不参与 append 行为。

这样避免程序滚动到底部时误把"用户正在看历史"判断为可恢复。真正恢复只由用户滚动到尾部触发。

## 8. 按钮 hover 与视觉规则

明亮模式中 ghost/icon 按钮 hover 应更明显:

- 背景从当前偏浅灰提升到更易感知的灰蓝或中性灰。
- 可同步提高边框可见度。
- 图标/文字颜色从 dim 切到正文色。
- active 可有轻微按下反馈,但不使用大幅动画。

暗色模式保持当前可见度,只做 token 对齐。

约束:

- 不引入大面积渐变、装饰圆点或营销式视觉。
- 控件圆角保持 6-8px 范围,与现有设计稿一致。
- 不把工具栏变成卡片套卡片。

## 9. 错误与空状态

### 9.1 无在线设备

- 设备下拉显示"无在线设备"空状态。
- 可提供"刷新设备列表"菜单项。
- 点击开始时若没有在线设备,给出清晰错误,不创建空会话。

### 9.2 命令非法

- combobox 内显示错误状态或附近状态文案。
- 不启动 adb。
- 错误说明应具体,例如"仅支持 logcat -v threadtime 和 main/system/radio/events/crash 缓冲区"。

### 9.3 文件会话切到 adb

- 点击开始前不需要二次确认。
- 直接停止/替换当前文件会话,创建新的 adb 会话。
- 最近文件列表保留,不会因切 adb 被清空。

### 9.4 清空实时抓取

- Clear 表示清空当前屏幕与当前会话的派生结果,不是 Stop 的别名。
- 正在抓取时,后端先受控终止并 join 旧 reader,提取已落盘日志的最后时间戳,安全重建空
  Session 后以相同设备、buffers 和 `-T` 时间戳自动续抓。
- 已停止或暂停时,Clear 只重建空 Session 并保持 Stopped,不会自行开始抓取。
- 不允许在仍有 reader 写入或 Session mmap 存活时截断会话文件。

## 10. 架构约束

1. `logcore` 仍不依赖 Tauri / UI。
2. 自绘控件只在前端层实现,不影响 `get_rows` 热路径。
3. 命令解析可在前端做即时反馈,但 Tauri/Rust 层必须再次校验并转换为结构化 adb 参数。
4. 不允许把命令字符串交给 shell 执行。
5. 状态栏 path 展示是前端显示 helper,不改变后端路径存储。
6. 移除拖拽打开不影响 `open_file(path)` IPC,也不影响打开按钮和最近文件。

## 11. 测试策略

### 11.1 前端纯逻辑测试

- `~/` 路径压缩:
  - home 下路径显示为 `~/...`。
  - 非 home 路径保留绝对路径开头。
  - 长路径中间省略,文件名保留。
- 命令解析:
  - 默认命令可解析出完整 buffer 列表。
  - 未写 `-b` 时归一化为 `main` + `system` + `crash`。
  - 非法 buffer 被拒绝。
  - 非 `logcat` / 非 `threadtime` / 复合 shell 命令被拒绝。
- 命令预设:
  - 去重。
  - 限制 20 条自定义预设。
  - 默认预设不可删除。
- tail-follow:
  - adb 会话开始时默认贴底。
  - 用户选择行 / 搜索跳转 / 小地图跳转会暂停。
  - 用户滚到底部恢复。

### 11.2 组件/结构检查

- 顶部不再存在"来源"下拉。
- `App.tsx` 不再注册全局 drag/drop 打开文件监听。
- 可见表单不再使用原生系统 `select` 作为用户可见下拉。
- 命令控件有输入、预设选择、添加和删除自定义预设能力。
- 状态栏最右侧显示 adb/file 来源详情。

### 11.3 Rust / 配置测试

- TOML 读取兼容旧 `commandBuffers`。
- 新 `current_command` / `command_presets` round trip 保持。
- 非法命令在 Tauri/Rust 层被拒绝。
- 合法命令转换为结构化 adb buffer 参数。

### 11.4 验证命令

- `cargo test -p logcore`
- `cargo build --workspace`
- `pnpm typecheck`
- `pnpm test`
- `pnpm build`

涉及 adb 真机验证时:

1. 先运行 `adb devices` 动态获取当前在线设备。
2. 再用 `adb -s <当前序列号> logcat -d -t 500 -v threadtime` 抓快照自证。
3. 不写死序列号,不写死本机 adb 路径。

## 12. 设计偏差

与 `docs/design/LogFilter.dc.html` / `LogWindow.dc.html` 的偏差:

1. 设计稿中顶部有"来源"下拉;本轮根据用户确认去掉,因为打开文件已有明确按钮入口,默认 adb 更符合当前工作流。
2. 设计稿空态提到"拖入";本轮移除拖拽打开和文案承诺,因为当前拖拽不稳定,不应展示不可依赖能力。
3. 设计稿命令下拉是普通菜单;本轮升级为 CommandCombobox,支持输入并保存命令预设,以满足用户新增要求。

以上偏差均为功能清晰度优先,已由用户确认。

## 13. 实施顺序建议

1. 新增基础自绘下拉控件与键盘/外部点击行为。
2. 替换顶部设备下拉和命令 combobox,同步配置模型。
3. 替换弹窗表单下拉与颜色选择。
4. 调整顶部来源信息架构、状态栏来源显示、空态文案。
5. 移除拖拽打开监听。
6. 收束 tail-follow 状态机。
7. 增强浅色 hover token。
8. 补测试并按验证命令跑全绿。
