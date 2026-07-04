# LogFilter 跨平台复刻 — 设计规范

- **日期**:2026-07-01
- **状态**:设计已确认,待编写实施计划(writing-plans)
- **原工程**:`./LogFilter`(Java 6/8 Swing,v1.8,`iookill/LogFilter`;新工程成型后删除)

---

## 1. 背景与目标

原版 LogFilter v1.8 是一个 2013 年的 Java Swing 桌面工具(Eclipse 工程、无构建系统、约 4300 行、`LogFilterMain` 单文件 2084 行),用于抓取/打开 Android logcat 并做过滤、高亮、书签。痛点:**Java 8 编译、运行、分发都麻烦**。

**目标**:用现代技术栈从零复刻为跨平台桌面客户端(**Windows 主打**,兼顾 macOS / Linux),做到易构建、易分发,并在原版基础上支持**超大日志文件(10GB+)**。

**复刻方式**:全新语言重写,**不拷贝原始 Java 代码**(原 `T.java` 带 "WiseStone Co. CONFIDENTIAL" 版权头,不复用其代码;其余为行为参考)。

## 2. 非目标 (Non-goals)

- 不支持 iOS(原版的 "iOS" 下拉实为残留,底层仍跑 adb,无实际功能)。
- 不做 kernel `/proc/kmsg` 解析。
- 不做移动端。
- **v1 不做**(均列为后续):多标签会话、过滤器预设、platform-tools 内置打包、自动更新。

## 3. 技术栈

| 层 | 选型 |
|---|---|
| 后端引擎 | **Rust**,抽为独立 `logcore` crate(mmap + 索引 + 过滤/搜索),不依赖 UI |
| 桌面壳 | **Tauri v2** |
| 前端 | **Vite + React + TypeScript + Tailwind v4(CSS-first)+ shadcn/ui(Base UI)** |
| 表格虚拟化 | **TanStack Virtual**(日志表格自研虚拟列表,**不用** shadcn Data Table,后者会为所有行建模) |
| 前端状态 | zustand |
| 配置格式 | TOML |

**选型理由**:10GB+ 的性能/内存是走系统级语言的动机,Rust 在内存控制与扫描速度上最优;重活全在 Rust 后端,Tauri 与前端只负责渲染可见窗口,故显示超大文本无压力。

## 4. 核心架构:索引化 + mmap + 只传可见窗口

原版把所有行解析成对象塞进内存(`ArrayList<LogInfo>`),10GB 下必然爆内存。复刻改为 `klogg`/`glogg` 式的**索引化流式**架构:

- **来源层**:`adb logcat` 子进程 / 打开文件 / 拖拽,三种入口统一喂给后端。实时流式时后端把 stdout 边收边写入会话文件并**增量建索引**,前端自动跟随尾部。
- **Rust 后端(性能核心)**:
  - **mmap 文件存储**——文件始终在磁盘,内存只有映射,不复制全文;
  - **行偏移索引**——后台线程流式扫描,记录"行号 → 字节偏移";建索引时 UI 不卡,可边建边看;
  - **流式过滤引擎**——过滤只扫描、只产出**命中行号数组**(`Vec<u32/u64>`),绝不复制文本;复用高效扫描(`memchr` / `regex`,可参考 ripgrep 的 `grep-searcher`),10GB 全量扫描秒级,带进度与可取消。
- **IPC 边界**:前端滚动到哪,就问后端要"第 N~N+count 行",后端 seek → 解析这几十行返回。**每帧只过几十行 JSON**。
- **前端**:虚拟表格只渲染可见行;过滤后表格改为对"命中行号数组"寻址。

> **铁律**:前端永不整体接收文件,只有可见窗口(约 50 行)过 IPC —— 故 10GB 与 10KB 一样流畅。

**索引内存权衡**:1 亿行的稠密偏移索引约 800MB。默认采用**检查点(稀疏)索引**——每隔 N 行存一个偏移 + 块内前扫,内存可压到几 MB;需要极致跳转速度时切稠密。步长默认值在实施中定。

## 5. 功能范围

### 5.1 照搬(核心)
- **来源**:`adb logcat`(`-v time` / `-v threadtime`,`-b main/system/radio/events/crash` 缓冲区)、打开文件、拖拽文件、最近文件(10 个)。
- **表格**:9 列(行号 / 日期 / 时间 / 级别 / PID / TID / Tag / 书签 / 消息),列可显隐、可调宽。
- **过滤**:7 类叠加(级别位掩码、PID、TID、Tag 显示、Tag 排除、关键词查找、关键词排除),各带 enabled 开关、`|` 多值、实时防抖。
- **高亮/着色**:关键词高亮(多色)+ 按级别着色。
- **书签/错误导航**:加/删书签、F2/F3 上/下跳、只看书签 / 只看错误。
- **小地图**:书签/错误位置可视化 + 点击跳转。
- **实时流**:运行 / 暂停 / 停止 / 清空 + 自动跟随尾部。
- **设备**:`adb devices` 列表、多设备 `-s` 选择。
- **其它**:复制(行 / 单元格)、跳转行号、字体设置、编码(UTF-8 / 本地)、窗口 / 列宽 / 过滤条件 / 颜色 / 命令预设持久化。

### 5.2 改良(现代化,默认纳入 v1)
- 索引化架构撑 10GB+(见 §4)。
- 高亮改为前端按 token 着色(非原版 HTML `<span>` 拼接)。
- 过滤**新增正则选项**(原版仅子串)。
- **书签持久化**(原版退出即丢 → 存 sidecar,按文件关联)。
- 内置**浅色 / 深色主题**(原版硬编码颜色)。
- 配置 INI → **TOML**(结构化,仍可手改;并支持 GUI 内配置)。

### 5.3 砍掉
- "iOS" 下拉;kernel `/proc/kmsg`;未使用死类(`TagTable` / `TagFilterTableModel` / `ClassTaster` / `DevicesPanel`);`T.java` 调试类(换标准日志)。

### 5.4 v1 新增
- **全局搜索**:命中计数 + 上/下一个跳转。
- **导出**:当前过滤视图 / 选中范围 → 文件。
- **大文件切分工具**:按大小 / 行数切分。

### 5.5 后续(非 v1)
- 多标签会话、过滤器预设、platform-tools 内置、自动更新。

## 6. 日志解析

- **格式**:`-v time` 与 `-v threadtime` 两种,自动识别(仅此两种,kernel 已砍)。
- **字段**:`lineNo, date, time, level, pid, tid(仅 threadtime), tag, message`。
- **级别**:V / D / I / W / E / F(含 VERBOSE…全称)。以**位掩码**支持级别过滤。
- **缓冲区**:通过 `logcat -b <buffer>` 选择。
- **解析器**为纯函数,独立单测。

## 7. 过滤

- **7 类叠加**:级别(位掩码)、PID、TID、Tag 显示、Tag 排除、关键词查找、关键词排除;各带 enabled;`|` 多值;**新增正则开关**。
- **特殊模式**:只看书签 / 只看错误(与常规过滤互斥,优先判定)。
- **执行**:后台流式,产出命中行号索引 `Vec<u32/u64>`;可取消 + 进度事件。

## 8. 着色与高亮

- **级别配色**(默认:D 蓝 / I 绿 / W 橙 / E、F 红 / V 灰),可配置;随深浅主题翻转。
- **关键词高亮**多色(如黄底);**查找命中**红底。前端按 token 渲染。

## 9. 书签与小地图

- 加/删书签(双击 / 快捷键),F2 / F3 上/下一个;书签**持久化**到 sidecar(按文件路径/hash 关联)。
- 左侧**小地图**:书签(蓝)/ 错误(红)刻度 + 视口方框,点击跳转;`get_minimap` 返回分桶位置。

## 10. UI 布局

- **顶部工具栏**:来源 / 设备 / 命令三下拉 + 运行·暂停·停止·清空 + 打开·导出·切分·搜索·主题。
- **可折叠**过滤栏:级别彩色芯片(兼配色图例)+ 7 类过滤 + 正则开关。
- **左侧**小地图。
- **中间**:9 列虚拟表格;**默认显示** 行号/日期/时间/级别/PID/TID/Tag/消息,**默认隐藏"书签"列**(同原版);整行按级别着色;关键词高亮 + 查找命中底色;书签行左侧强调边 + 书签图标。
- **底部状态栏**:总行数 / 过滤后行数 / 索引进度 / 当前位置 / 编码 / 格式。

> 后续将再做一版 UI 交互设计,当前样式为规划基线。

## 11. 工程结构

```
log-filter/                       # 新工程根 (Cargo workspace)
├─ crates/logcore/                # 纯引擎,可独立单元测试
│  ├─ model.rs        LogEntry / LogLevel
│  ├─ mmap_source.rs  mmap + 增长文件(实时流)
│  ├─ indexer.rs      检查点行偏移索引(后台增量)
│  ├─ parser.rs       time / threadtime 解析 + 自动识别
│  ├─ filter.rs       过滤规格 → 命中行号索引(子串 / 正则)
│  ├─ search.rs       全局搜索
│  ├─ bookmarks.rs    书签 + sidecar 持久化
│  ├─ session.rs      source + index + filter 组装成会话
│  ├─ adb.rs          adb devices / logcat 子进程与生命周期
│  ├─ export.rs · split.rs   导出 / 切分
│  └─ config.rs       TOML 读写
└─ src-tauri/                     # Tauri v2 应用
   ├─ main.rs         注册命令、管理 State<AppState>
   ├─ commands.rs     薄封装:把 logcore 暴露给前端
   └─ events.rs       索引 / 流 / 过滤 进度事件

src/                              # 前端 (Vite + React + TS)
├─ components/  Toolbar · FilterBar(可折叠) · Minimap · LogTable(TanStack Virtual)
│              · StatusBar · SearchOverlay · dialogs/{Export,Split,Settings}
│              · ui/  (shadcn/ui · Base UI)
├─ lib/ipc.ts   类型化 invoke/listen 封装 = IPC 契约
├─ hooks/       useLogWindow · useFilter · useLiveStream · useIndexProgress
├─ store/       zustand:会话 / 过滤 / 列显隐 / 主题
└─ types.ts     与 Rust 对齐的 TS 类型
```

## 12. IPC 契约(前后端唯一边界)

```
命令 (invoke → 后端, async)
  会话  open_file(path) · start_logcat(device,cmd) · list_devices()
        pause/resume/stop_stream() · clear_session() · get_status()
  数据  get_rows(view, start, count) → Row[]      ← 热路径,只回可见窗口(count 有上限)
        set_filter(FilterSpec) · get_filtered_count()
  搜索  search(query,opts) → {matches, first} · search_next(from, dir)
  书签  toggle_bookmark(line) · list_bookmarks() · next_bookmark(from, dir)
  地图  get_minimap(view, buckets) → {bookmarks[], errors[]}
  工具  export(view, range, path) · split(path, by, value, outDir)
  配置  get_config() · set_config(patch)

事件 (后端 emit → 前端 listen)
  index:progress {indexedBytes,totalBytes,totalLines,done}
  stream:append  {newTotalLines}     # 触发前端刷新尾部 / 状态
  filter:done    {filteredLines}
  search:progress{scanned,matches}

Row        = {lineNo,date,time,level,pid,tid,tag,message,marked}
FilterSpec = {levels位掩码, showTag,removeTag,pid,tid,findWord,removeWord,highlight,
             各带 enabled 开关, regex:bool}
```

> **热路径铁律**:`get_rows` 的 `count` 设上限(如 ≤512);前端按可见范围调用 + LRU 缓存已取窗口 + 预取相邻。

## 13. 配置与持久化

- **TOML** 格式;默认存各平台标准 app 配置目录(不再像原版写在工作目录)。
- **存储位置可配置**;除直接手改 TOML 外,**GUI 内可直接配置**。
- 内容:窗口大小、列显隐/宽、过滤条件、颜色/主题、命令预设、最近文件、书签 sidecar、adb 路径。

## 14. adb 集成

- **可配置 "adb 可执行文件路径"** + **自动扫描**常见位置;`adb devices` 列设备,`-s` 选设备;`logcat` 子进程 run / pause / stop。
- platform-tools 内置打包:后续可选(第一阶段不做)。

## 15. 工具功能

- **全局搜索**:计数 + 上/下一个。
- **导出**:当前过滤视图 / 选中范围 → 文件。
- **切分**:按大小 / 行数切分大文件。

## 16. 打包分发

- **Windows(主打)**:`.msi` + NSIS `.exe`;WebView2 运行时缺失时由安装器自动引导。
- **macOS**:`.dmg`。
- **Linux**:`.deb`(目标 Debian / Ubuntu 较新 LTS)。
- **CI**:规划 **GitHub Actions** 三系统矩阵 + `tauri-action`,一条工作流产出三平台包并挂 Release。**前期在本机 dev / 调试**;用户择机推 GitHub 并开启 Actions。
- **自动更新**:可选(Tauri v2 updater),后续。

## 17. 性能与内存

- 窗口化数据流;检查点行偏移索引(默认稀疏,可切稠密);过滤/搜索复用高效扫描;前端窗口 LRU + 预取。
- 记录权衡:1 亿行稠密索引 ≈ 800MB。

## 18. 测试策略

- `logcore` 与 UI 解耦 → 纯 Rust 单元测试(解析 / 索引 / 过滤 / 搜索 / 切分),走 **TDD**。
- 前端组件测试 + IPC 契约类型对齐;用合成的大日志做性能基准。

## 19. 里程碑(粗)

- **M1** 引擎骨架:打开文件 + 索引 + 虚拟表格只读浏览(大文件跑通)。
- **M2** 过滤 + 着色 + 高亮 + 全局搜索。
- **M3** adb 实时流 + 设备选择 + 运行控制。
- **M4** 书签 + 小地图 + 持久化。
- **M5** 导出 + 切分 + 设置 GUI + 主题。
- **M6** 三平台打包 + CI。

## 20. 待实施中决定的细节(用户已授权我把控,按实际功能/性能调整)

- 检查点索引步长默认值;`get_rows` count 上限具体值;前端行渲染 DOM vs canvas;zustand store 结构;具体 adb 命令模板集合;filter/search 取消与防抖策略细节。

## 21. 参考

- 原工程:`./LogFilter`(Java Swing v1.8,新工程成型后删除)。
- 架构参考:klogg / glogg(大日志索引模型)、ripgrep / `grep-searcher`(高效扫描)。
