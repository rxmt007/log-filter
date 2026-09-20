# LogFilter 产品方向规划 · Design QA

- **日期**：2026-08-20
- **检查对象**：[`index.html`](index.html)
- **权威视觉源**：[`../LogFilter.dc.html`](../LogFilter.dc.html) 中的 LogWindow 浅色 1180×860 画布；交互细节对照 [`../LogWindow.dc.html`](../LogWindow.dc.html)
- **比较状态**：浅色、默认筛选、原型初始“抓取中断”状态
- **比较尺寸**：源与实现均为 CSS 1180×860；源截图从 DPR 2 归一为 1×，两张证据图均为 1180×860 像素

## 1. 同屏比较证据

- 源截图：[`qa/source-logwindow-1180x860.png`](qa/source-logwindow-1180x860.png)
- 实现截图：[`qa/implementation-investigation-loop-1180x860.png`](qa/implementation-investigation-loop-1180x860.png)
- 同屏对照：[`qa/source-vs-implementation.png`](qa/source-vs-implementation.png)
- 可重复打开的比较页：[`qa/comparison.html`](qa/comparison.html)

同屏检查覆盖 titlebar、两层工具栏、过滤区、健康 context banner、小地图、密集日志表、Problems 式 bottom dock 与状态栏。原型保留了权威稿的系统字体/等宽正文、蓝色主交互、1px hairline、6–9px 圆角、紧凑行高和“日志表为主角”的层级。

以下差异是有意的产品扩展，不是视觉漂移：

- 六行过滤器在原型初始态收敛为配方与条件摘要，为调查闭环腾出正文空间；方向卡已明确实际产品仍可展开编辑。
- 搜索区旁增加抓取健康入口，表格上方使用既有 context banner 解释断流与恢复。
- 下方复用 Problems dock 承载密度摘要、结构化详情和调查会话，不增加永久侧栏或独立 dashboard。

## 2. 修复历史

| 轮次 | 发现 | 级别 | 修复 | 复验 |
|---|---|---:|---|---|
| 1 | 设计稿 `faint` 灰在浅色小字号下对比不足 | P1 | 先收紧弱文本色，再由独立终审补测所有实际表面 | 进入第 2 轮 |
| 1 | 自动化环境中的原生 dialog 未可靠响应 Escape，也没有可验证的焦点返回 | P1 | 增加显式 Escape 处理，并沿用 `close` 事件把焦点还给触发器 | 通过；对话框关闭，焦点返回 `recipe-trigger` |
| 1 | 无 favicon 时浏览器会额外尝试请求 `/favicon.ico` | P2 | 内联现有 32×32 LogFilter app icon 的 PNG data URI | 通过；隔离构建运行时资源列表为空 |
| 2 | 白字 / `#3b82f6` 仅 3.68:1，且弱文本在 panel/header/accent-soft 上不足 4.5:1 | P1 | 实心控件改用 `accent-strong` + `on-accent`；浅/深弱文本分别改为 `#59616d` / `#8b93a1` | 通过；实心控件浅/深为 5.17:1 / 7.64:1，弱文本跨实际表面的最低值为 5.52:1 / 4.81:1 |
| 2 | 非法 hash 可触发 selector 异常；跨优先级深链会让卡片、筛选器和计数不一致 | P2 | 改用 `getElementById`；有效深链会清空查询/验证项并同步到目标优先级 | 通过；`#dir-]` 无错误，P1 → P2 深链得到 P2 激活、6/16、目标展开 |
| 2 | 配方与时间范围的组合操作会互相覆盖计数 | P2 | 将 `appliedRecipe` 与 `rangeActive` 分离，并用一个函数派生计数 | 通过；未应用配方时清范围恢复 87,211，范围中应用配方仍为 12，最后清范围为 4,208 |
| 2 | 健康 banner 隐藏后 `aria-expanded` 未同步；dock 缺少页签语义，切换 panel 后可能把焦点留在隐藏内容 | P2 | 同步隐藏状态与焦点；补齐 `tablist/tab/tabpanel`、关联 ID、roving tabindex 和方向键；加入会话后聚焦可见 session tab | 通过；“知道了”返回健康入口，ArrowRight 切到结构化视图，加入会话后焦点位于可见 `dock-tab-session` |
| 2 | 两张 `.png` 证据资源实际保存为 JPEG | P3 | 重新以 PNG 编码生成实现与同屏对照证据 | 通过；三张 `.png` 均由文件签名确认是真实 PNG |
| 3 | 修正后的同尺寸全视图与关键区域未见裁切、重叠、错误圆角、字体跳变或状态语义冲突 | — | 刷新实现与同屏比较证据 | 通过 |

最终无遗留 P0、P1 或 P2 视觉/交互问题。

## 3. 主交互回归

在浏览器中使用真实控件操作，而非直接改 DOM：

- 方向筛选：P1 为 7/16；只看验证项为 8/16；搜索“PID 重用”为 1/16。
- 卡片：全部展开为 16，全部折叠为 0；`#dir-p2-multifile` 深链会展开目标、滚到标题并同步 P2 / 6/16；非法 `#dir-]` 不抛异常。
- 对话框：配方、时间、安全分享均可打开/关闭；Escape 关闭后焦点回到触发器。
- 状态组合：未应用配方时应用/清除范围为 12 → 87,211；范围中应用配方仍为 12，清除后为 4,208。
- 代表性路径：恢复抓取 → 应用“支付失败复现” → 应用时间范围 → 选择第 24 行 JSON → 加入调查会话 → 生成安全副本。
- 路径终态：健康状态为“抓取健康”、配方已命名、范围 context 可见、第 24 行选中、调查会话计数为 2、焦点位于可见 session tab、安全分享 dialog 已关闭。
- 深浅主题切换正常；全流程 console warning/error 为 0。

## 4. 布局与可访问性

| 视口 | 页面横向溢出 | 原型行为 | 结果 |
|---|---:|---|---|
| 1440×900 | 0 | 1180×860 原型完整置于内容区 | passed |
| 1024×768 | 0 | 原型仅在自身 932/1208px scroller 内横向滚动 | passed |
| 390×844 | 0 | 规划正文单列；原型仅在自身 362/1208px scroller 内横向滚动 | passed |

核心控件使用 `button`、`input`、`details`、`dialog`，具备 `aria-pressed`、`aria-expanded`、`aria-live`、可见 focus ring 与键盘关闭路径。分析 dock 具备完整 `tablist/tab/tabpanel` 关联和方向键切换；健康 banner 隐藏后状态与焦点同步。紧凑日志行仍用于只读证据浏览；所有规划级主操作保持更大的点击区域。

## 5. 离线门

- HTML 的 CSS、JavaScript、方向正文、图标符号与 favicon 全部内联；没有 CDN、远程字体、`fetch`、XHR、动态 import、外部脚本或远程图片。
- 将唯一的 `index.html` 放入隔离目录后加载，DOM 中远程资源引用为 0，运行时 Resource Timing 为 `[]`，console warning/error 为 0；这证明页面不依赖同目录资产或网络响应。
- 以 `file:///offline/index.html` 文档 URL 在本地 DOM 运行时解析并执行同一文件：16 张卡、P1=7、展开/折叠=16/0，恢复抓取、配方、时间范围、结构化行和调查会话均通过，脚本错误为 0。
- 自动化浏览器的安全策略禁止直接导航本机 `file://` 地址，因此视觉布局由相同字节的隔离 localhost 文档验证，`file:` 协议下的解析与交互由本地运行时单独验证；没有把该工具限制误写成真实浏览器已点击 `file://`。

## 6. 结论

规划文档保持现有 LogFilter 设计语言，核心路径可交互，桌面/中等宽度/移动宽度布局通过，且单文件离线边界成立。

**final result: passed**

## 7. 2026-09-20 提交前复查

本次只复验以下两处修正；前述完整视觉与交互记录保留为 2026-08-20 的历史检查结果。

- 调查会话的“保存到本地 sidecar”按钮原先无反馈，现点击后明确提示：“概念演示：调查状态仅保留在当前页面，未写入 sidecar 文件；刷新后重置”。真实浏览器点击通过，未增加持久化或文件写入逻辑；点击前后 localStorage 与 sessionStorage 均为 0 项，页面附加资源请求为空，console warning/error 均为 0。
- 初始来源统一为“设备采集”；断流 banner 改为“历史日志已保留，当前暂无新数据”，与已显示的历史行和过滤后计数一致，不再将断流误写为零命中。
- 重拍并目视检查 1180×860 原型初始截图及 1280×504 同屏对照截图，修改后的来源和 banner 均完整可见。权威设计源截图未修改，SHA-256 前后相同。

**本次复查结果：passed。** 原型仍只用于概念演示，不保存真实调查数据。
