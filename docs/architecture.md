# LogFilter 架构文档

> 面向维护者与贡献者。本文描述**现状代码**的分层、模块职责、并发模型与关键流程;
> 设计动机与需求背景见规范文档
> [`docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md`](superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md)。
> 本文中的行为描述均以源码为准;若与本文不一致,以源码为准并请更新本文。

- 仓库:`github.com/rxmt007/log-filter`
- 技术栈:Rust 引擎(`logcore`,mmap + 检查点索引)+ Tauri v2 + React 19 / TypeScript + zustand + TanStack Virtual + Tailwind v4(CSS-first)
- 目标:跨平台桌面 logcat 查看器,支持 10GB+ 超大日志文件

---

## 1. 分层总览与核心架构不变量

```
┌──────────────────────────────────────────────────────────┐
│ 前端 src/ (React 19 + zustand + TanStack Virtual)         │
│   只经 get_rows(view, start, count≤512) 拉可见窗口          │
│   RowBlockCache(64 块 LRU)+ 事件驱动状态                    │
└────────────────────────┬─────────────────────────────────┘
                         │ Tauri IPC(命令 + 事件)
┌────────────────────────┴─────────────────────────────────┐
│ src-tauri/ 薄命令层                                        │
│   state.rs   全局 AppState(Session 互斥锁 + 各任务代号)     │
│   commands.rs 命令实现 + 后台任务编排(索引/过滤/搜索/流/导出)  │
│   dto.rs     camelCase 序列化边界                           │
└────────────────────────┬─────────────────────────────────┘
                         │ 普通 Rust 函数调用
┌────────────────────────┴─────────────────────────────────┐
│ crates/logcore/ 纯引擎(不依赖 Tauri / UI,可独立单测)        │
│   mmap + 检查点行索引 + 借用式解析 + 过滤/搜索/书签/导出/切分   │
└──────────────────────────────────────────────────────────┘
```

以下四项是所有实现必须保持的**核心架构不变量**:

1. **只传可见窗口**:前端永不整体接收文件。一律经 `get_rows(view, start, count)` 取窗口,
   `count` 有硬上限(`MAX_ROWS = 512`,见 `src-tauri/src/commands.rs`)。任何"把整文件 /
   整过滤结果发给前端"的做法都禁止。
2. **引擎与 UI 解耦**:`logcore` 不依赖 Tauri / UI;解析、索引、过滤、搜索、切分全部可脱离界面单测。
3. **绝不整体载入**:文件用 mmap(`MmapSource`);过滤 / 搜索只产出**命中行号数组**
   (`Vec<u32>`,0-based 源行号),不复制文本。
4. **纯函数先行 TDD**:解析器、过滤器等为纯函数,先写测试再写实现。

---

## 2. logcore 模块地图

目录:`crates/logcore/src/`。

| 模块 | 职责一句话 | 关键类型 / 函数 |
|---|---|---|
| `model` | 一条日志的 owned 解析结果 | `LogEntry`(7 字段)、`as_parsed()` |
| `mmap_source` | 只读内存映射文件源 | `MmapSource`(空文件时 mmap 为 None) |
| `indexer` | 增量检查点行索引 | `Indexer`、`for_each_line_span` |
| `parser` | logcat 两种格式的借用式解析 | `ParsedLine<'a>`、`parse_line_ref`、`level_byte_of_line` |
| `filter` | 7 类叠加过滤 + 级别位掩码 | `FilterSpec`、`LevelMask`、`FilterMatcher` |
| `search` | 全字段全局搜索 | `SearchSpec`、`SearchMatcher`、`next_match` |
| `bookmarks` | 书签侧车持久化 | `BookmarkStore`、`sidecar_path_for` |
| `session` | 核心状态机(索引/过滤/搜索/视图/导出原语) | `Session`、`RowsView`、`ExportPlan` |
| `adb` | adb 命令构造、设备列举、时间戳提取 | `LogcatSpec`、`build_logcat_command`、`last_log_timestamp` |
| `export` | 导出摘要与原始字节写出 | `ExportSummary`、`write_raw_line` |
| `split` | 大文件切分(整行对齐) | `SplitMode`、`split_file_with_progress` |
| `config` | TOML 应用配置(加载/保存/归一化) | `AppConfig`、`default_config_dir` |
| `encoding` | UTF-8 / 本地编码解码 | `TextEncoding`、`ResolvedTextEncoding` |

### 2.1 model

`LogEntry` 是 owned 的 7 字段结构(`date/time/level/pid/tid/tag/message`),**不含行号**
(行号由 `Session` 在取行时赋值,1-based)。`as_parsed()` 提供零拷贝借用视图,让匹配器统一走
借用式路径;`From<ParsedLine>` 只在 `get_rows` → IPC 边界处使用。

### 2.2 mmap_source

`MmapSource::open` 打开只读映射;0 长度文件无法 mmap,内部用 `Option<Mmap>` 表达,
`bytes()` 对空文件返回空切片。**已知残留风险**(见源码注释):外部截断由
`Session::remap_source` 侦测重建,但两次 remap 之间发生的截断仍可能在访问已消失页时触发
SIGBUS。

### 2.3 indexer —— 检查点行索引(stride 1024)

不为每行存偏移(10GB 文件 ~3 亿行,每行一个 u64 就是 2.4GB)。`Indexer` 每隔
`DEFAULT_CHECKPOINT_STRIDE = 1024` 行记录一个 `LineCheckpoint { line, offset }`;
定位第 i 行时二分找到 `<= i` 的最近检查点,再用 memchr 前向扫到目标行。

- `step(bytes, budget)`:从内部 cursor 起最多处理 budget 字节,memchr_iter 找换行;
  可增量调用(文件增长后再 step 会把"尾行后追加"的新行正确计入)。
- `for_each_line_span(bytes, start, end, frontier, f)`:**单次前向扫描**产出区间内每行的
  字节 span,不物化 Vec;`line_spans` 与导出批量原语共用此扫描——定位一次检查点、一路
  memchr 到底(这是导出吞吐 21× 修复的关键,见 §8)。
- `frontier` 参数:索引未完成时把行尾裁剪到已索引前沿,防止把未索引的整段剩余当成一行。

### 2.4 parser —— 借用式零分配解析

仅支持两种 logcat 格式,自动识别,失败回退整行作 message:

- `parse_threadtime_ref`:`MM-DD HH:MM:SS.mmm  PID  TID L Tag: message`。
  真机(Android 9 / MiTV 实测)会把短 tag 填充到固定宽度(如 `vold    :`),解析时
  `trim_end` 去掉填充。
- `parse_time_ref`:`MM-DD HH:MM:SS.mmm L/Tag(  pid): message`。用 `char_indices`
  安全取"级别 + 斜杠",多字节字符(中文/emoji)不会 byte 切片 panic。

`ParsedLine<'a>` 七个字段全是源文本切片,过滤/搜索热路径**零堆分配**;owned `LogEntry`
仅在 IPC 边界生成。

`level_byte_of_line(&[u8]) -> Option<u8>`:字节级零分配地判断一行级别(`b'V'..b'F'`),
语义与 `parse_line(...).level` 一致(有等价性测试)。供索引期错误行扫描
(`Session::refresh_error_lines`)这类热路径使用,避免整行 UTF-8 解码。

### 2.5 filter —— 7 类叠加过滤

`FilterSpec` 的 7 类条件为**合取**(全部满足才命中):

| 条件 | 语义 | 匹配方式 |
|---|---|---|
| `levels: LevelMask` | 级别位掩码(V=1,D=2,I=4,W=8,E=16,F=32) | 全选时不过滤(raw 行也保留);非全选时 raw 行(无级别)被排除 |
| `marked_only` | 只看书签行 | 依赖 `is_match_with_mark` 传入的 marked 标记 |
| `pid` / `tid` | 进程/线程号 | **精确相等**(`equals_any`) |
| `tag_include` / `tag_exclude` | Tag 显示/排除 | 子串包含(`contains_any`) |
| `word_include` / `word_exclude` | 关键词 查找/排除 | 对 message 子串包含 |

每个字段是 `FilterField { enabled, pattern, regex }`;pattern 按 `|` 拆多值(任一命中即可),
每个值可整体切换为正则。正则编译失败返回 `FilterError`(命令层转为用户可见错误)。
`FilterMatcher` 是编译后的匹配器(正则只编译一次),`requires_mark()` 供 `Session` 决定
是否需要查书签。`highlights: Vec<HighlightRule>`(默认黄/绿/蓝三条)只用于前端渲染,
不参与过滤,随 spec 持久化到配置。

### 2.6 search

`SearchSpec { query, regex, case_sensitive }`。编译为三态 `CompiledSearch`:
Empty(空查询不命中)/ Plain(区分大小写明文,直接 `contains`)/ Regex。
**大小写不敏感明文**走 regex 引擎(`regex::escape` 转义字面量 + `case_insensitive`),
兼顾 ASCII 与 Unicode 折叠,避免 lowercase 拷贝(元字符按字面量处理,有测试)。
`is_entry_match` 对 7 个字段逐一尝试。`next_match(matches, from, direction)` 在升序命中
数组上二分 + 环绕导航。

### 2.7 bookmarks —— 侧车持久化

`BookmarkStore` 内部 `BTreeSet<u64>`,存 **1-based 行号**。每次 `Session::toggle_bookmark`
即时写侧车文件:`<日志文件路径>.lfbookmarks.toml`(TOML,`version/source/lines`)。
打开文件时自动加载。`next(from, direction)` 环绕导航。

### 2.8 session —— 核心状态机

`Session` 聚合:`source(MmapSource) + indexer + filtered(Vec<u32>) + filter_active +
filter_spec + search_matches(Vec<u32>) + search_spec + bookmarks + error_lines(Vec<u32>) +
error_scan_lines + encoding`。要点:

- **四个视图** `RowsView::{All, Filtered, Bookmarks, Errors}`。`get_rows_for_view`
  按视图把 view_idx 映射回源行号再解析;Filtered 未激活时自动退化为 All(前端因此可以
  永远请求 filtered 视图)。返回 `(1-based 行号, LogEntry)`。
- **默认过滤不物化**:`FilterSpec` 非激活时 `filtered` 保持空、`filter_active=false`,
  `filtered_count()` 直接返回 `total_lines()`——不为"全命中"生成 3 亿元素数组。
- **索引前沿**:`indexed_frontier()` 在索引未完成时返回 `indexer.cursor()`,
  `get_rows` 等一切读行路径用它裁剪,防止前沿行把未索引剩余吞成一行(有回归测试)。
- **错误行增量扫描**:`refresh_error_lines` 在每次 `index_step` 后从 `error_scan_lines`
  游标起用 `level_byte_of_line` 扫 `E/F` 行,只前进不回扫。
- **过滤/搜索的三段式 API**(供命令层分块调度):
  `set_filter_pending`(存 spec、校验正则;非激活时立即清空返回)→
  `filter_indexed_range(matcher, start, end)`(只读,产出区间命中)→
  `apply_filter_results` / `append_filter_results`(写回;append 会 sort+dedup)。
  搜索同构(`set_search_pending` / `search_indexed_range` / `apply|append_search_results`)。
- **remap**:`remap_source()` 重新 stat 文件——未增长时跳过(流式 reader 每个读块都调,
  mmap/munmap 不便宜);增长时重映射;**收缩**(外部截断/轮转)时重映射并
  `reset_derived_state()`(新建 Indexer、清 filtered/search/errors、`filter_active=false`,
  `filter_spec` 保留),返回 reset=true,**重扫由调用方负责**。
  `remap_and_index_step(budget)` 把 remap 与一步索引合并,返回 `RemapStep { reset, done }`。
- **命中数组用 u32**:`push_hit` 对超出 u32 的行号跳过并 debug_assert(10GB 实际 ~3 亿行,
  距 u32 上限尚有一个数量级)。
- **minimap(buckets)**:书签桶 + 错误桶(`MinimapBucket { bucket, count }`,密度加权)。
  过滤激活时**反向遍历**:书签/错误是小集合,逐个二分反查在 filtered 中的位置,
  避免 O(过滤结果总数) 全扫(minimap 被状态事件高频触发)。
- **导出原语**(供命令层"锁内拷贝、锁外写盘"的分段导出,见 §5.4):
  - `export_plan_snapshot(view) -> ExportPlan`:一次持锁产出快照。
    `AllLines { total }`(行区间,不物化)或 `Indices(Vec<u32>)`(Filtered 克隆命中数组;
    Bookmarks/Errors 转换/克隆小数组)。
  - `append_line_range_bytes(start, count, out)` / `append_sorted_lines_bytes(indices, out)`:
    把连续区间 / 一批升序行号的**原始字节**(含行尾换行)追加进缓冲,单次前向扫描,
    返回 (行数, 字节数);越界行跳过。
  - `validate_export_target(output)`:目标不得与源文件相同(含 canonicalize 比对),
    确保父目录存在。
  - 遗留的同步整体导出 `export_view` / `export_range` 仍在(内部也分批,
    `EXPORT_CHUNK_LINES = 4096`),现作为分段导出等价性测试的 oracle。

### 2.9 adb

- `LogcatSpec::parse`:只接受 `logcat -v threadtime [-b <buffer>]`,buffer 限
  main/system/radio/events/crash,单个 `-b`;含 `| & ; > <` 的复合 shell 命令直接拒绝
  (防注入)。`normalized()` 输出规范形式。
- `build_logcat_command(adb_path, serial, buffers, since)`:构造
  `adb -s <serial> logcat -v threadtime -b <buf>... [-T <time>]`;buffers 去重,空则 main。
- `adb_command(path)`:**所有 adb 子进程必须经此创建**——Windows 下加
  `CREATE_NO_WINDOW`,抑制控制台窗口闪烁。
- `list_devices_with_timeout`(默认 5s):轮询 `try_wait`,超时 kill 子进程返回
  `TimedOut`——adb server 冷启动或 USB 抖动时 `adb devices` 可能长时间挂起,避免上层轮询堆积。
- `parse_adb_devices` / `select_online_device`:解析 `adb devices -l`
  (serial、state、model/product/transport_id),优先指定 serial 的在线设备,否则第一台在线设备。
- `last_log_timestamp(tail_text)`:从会话文件尾部文本反向找最后一条可解析日志的
  `MM-DD HH:MM:SS.mmm` 时间戳,供 resume 时 `logcat -T` 续抓去重。真机结论(Android 9 / MiTV):
  `-T "<time>"` 续抓可用且**包含**边界时间戳(同毫秒有少量重复行,靠追加后的
  sort+dedup 语义与用户容忍度消化);旧设备不支持 `-T` 时 logcat 立即退出,表现为流自动停止。
- `resolve_adb_path`:配置路径 → `PATH` 扫描 → 常见位置
  (`ANDROID_HOME`/`ANDROID_SDK_ROOT` 的 platform-tools、`~/Library/Android/sdk`、
  `~/Android/Sdk`、Windows `LOCALAPPDATA`)。

### 2.10 export / split

`export.rs` 很薄:`ExportSummary { written_lines, written_bytes }` 与
`write_raw_line`(写原始字节并返回长度)。分段导出的主体在 `Session` 原语 +
`commands.rs::run_chunked_export`。

`split.rs`:`SplitMode::{Bytes(n), Lines(n)}`,64KB BufReader `read_until(b'\n')` 逐行搬运,
**永远整行对齐**——分片不截断行;单行超过字节上限时该行独占一个分片(可能超限)。
分片命名 `<原文件名>.partNNN`(001 起)。每关闭一个分片(writer 已 flush、字节已落盘)
回调 `on_part(已完成分片数, 已处理字节)`。0 上限被拒绝。

### 2.11 config

`AppConfig`(TOML):theme(light/dark)、adb_path、storage_dir(流式会话文件目录)、
encoding、font_size(10..=20)、row_height(16..=32)、table(9 列的宽度/可见性,
带 min/max 钳制;message 列强制可见;全隐藏时回退默认)、recent_files(去重、上限 10)、
last_filter(含 highlights,空则补默认三条)、current_command / command_presets /
command_buffers(logcat 命令预设,经 `LogcatSpec` 归一化,兼容早期仅有 buffers 的旧配置)、
window(960..=3840 × 560..=2160)。

`load_config` 不存在时返回默认;加载/保存都会 `normalized()`。配置目录按平台:
Windows `%APPDATA%\LogFilter`、macOS `~/Library/Application Support/LogFilter`、
Linux `$XDG_CONFIG_HOME/logfilter`(缺省 `~/.config/logfilter`),文件名 `config.toml`。

### 2.12 encoding

`TextEncoding::{Utf8, Local}`(配置值 `"UTF-8"` / `"Local"` / `"本地"`)。
`resolve()` 产出 `ResolvedTextEncoding`:Local 在 Windows 用 `GetACP()` 映射代码页
(936→GBK、950→BIG5、932→Shift_JIS、949→EUC-KR、1250-1258→windows-125x),
Unix 读 `LC_ALL/LC_CTYPE/LANG`。`decode()` 返回 `Cow<str>`(UTF-8 用 lossy,
其他经 encoding_rs)。`Session` 每次取行文本都过 `encoding.decode`。

---

## 3. src-tauri 命令层

| 文件 | 职责 |
|---|---|
| `src/state.rs` | 全局 `AppState`:会话锁 + 五个代号计数器 + 流运行时 |
| `src/commands.rs` | 23 个 Tauri 命令 + 后台任务编排(索引/过滤/搜索/流式/导出/切分) |
| `src/dto.rs` | 序列化边界:camelCase DTO 与 logcore 类型互转(配置转换会走 `normalized()`) |
| `src/lib.rs` | Tauri Builder:注册插件(opener/dialog)、`manage(AppState)`、命令表 |
| `src/main.rs` | 入口,调 `lib::run()` |

关键常量(`commands.rs`):

| 常量 | 值 | 用途 |
|---|---|---|
| `INDEX_BUDGET` | 8 MiB | 每步索引预算(步间释放锁) |
| `SCAN_CHUNK_LINES` | 4096 | 过滤/搜索分块扫描的块大小 |
| `SEARCH_PROGRESS_STRIDE` | 65 536 | 搜索进度事件节流(约 16 块) |
| `EXPORT_CHUNK_LINES` | 4096 | 导出每批行数 |
| `EXPORT_PROGRESS_STRIDE` | 65 536 | 导出进度事件节流 |
| `STREAM_READ_BUF` | 64 KiB | 流式 stdout 读块 |
| `MAX_ROWS` | 512 | `get_rows` 行数硬上限 |
| `STREAM_SESSION_KEEP` | 10 | 流式会话文件保留份数 |

阻塞线程池:`list_devices`、`start_logcat`、`resume_logcat`、`export_logs`、
`split_log_file` 是 async 命令,内部 `spawn_blocking`——它们要么等 adb(最长 5s 超时)、
要么 join 线程、要么处理 10GB 级 IO,不能占住命令处理线程。其余命令是同步短临界区。

---

## 4. 并发模型(重点)

### 4.1 单会话互斥锁

全局只有一个会话:`AppState.session: Arc<Mutex<Option<Session>>>`。所有读写(get_rows、
索引步进、扫描块、导出拷贝)都在这把锁下短暂持有。`lock_session()` 对 poisoned 锁做
`into_inner()` 恢复(有测试),后台线程 panic 不会永久毒化会话。

### 4.2 会话代号与写侧不变量

`AppState.generation: Arc<AtomicU64>` 是**会话代号**。写侧不变量(三处替换会话的地方都
遵守:`open_file`、`start_logcat_blocking`、`reset_stream_session_file`):

> **先 `generation.fetch_add(1)` 领取新代号,再把新 Session 放进锁里。**

读侧配套的 `lock_session_if_current(g)`:先拿锁、**持锁状态下**再比对代号。由于写侧
"先递增、后替换",持锁后代号仍匹配 ⇒ guard 里的会话一定是该代号对应的那个。所有后台任务
(索引线程、过滤/搜索扫描块、流式 reader、导出各阶段)每次持锁都经它校验,代号失效即自行退出。

### 4.3 任务代号(取消机制)

除会话代号外,有四个独立的任务代号计数器,语义都是"领新号 = 作废旧任务":

| 计数器 | 领号 | 校验 | 谁递增使其失效 |
|---|---|---|---|
| `filter_task_generation` | `next_filter_task_generation` | `is_current_filter_task` | 新 `set_filter`、换会话、索引完成重扫、`set_config` |
| `search_task_generation` | `next_search_task_generation` | `is_current_search_task` | 新 `search`、换会话、同上 |
| `export_task_generation` | `next_export_generation` | `is_current_export` | 新 `export_logs`(起始即领号)、`cancel_export` |
| `stream_generation` | `next_stream_generation` | `is_current_stream_task` | 任何 `take_stream_task`(pause/stop/forget)与新流启动 |

没有线程句柄级的强制中断:任务在每个分块 / 每批 / 每个读块的边界自查代号,发现失效就
放弃(过滤/搜索直接丢弃部分结果;导出删除半成品文件)。

### 4.4 流式 reader 线程生命周期

- **启动**(`spawn_logcat_stream`):列设备 → 选设备 → 构造命令 → spawn adb 子进程
  (stdout piped)→ 领 stream 代号 → spawn reader 线程。reader 先阻塞在 `start_rx.recv()`
  上,等主流程把 `StreamTask { generation, child, handle, serial }` 注册进
  `StreamRuntime` 后再发信号放行——保证 reader 干活时任务记录一定已就位。
- **运行**:见 §5.2。
- **停止**(`stop_stream_task(mode)`):`take_stream_task` 先领新 stream 代号(作废 reader),
  按 `StreamStop::{Pause, Stop, Forget}` 设置 `paused` 标记、决定 `last_request` 去留
  (Pause/Stop 保留供 resume/clear 复用,Forget 丢弃),取出任务;然后 **kill child → join
  reader 线程**。kill 会让 reader 阻塞中的 `read` 返回,join 保证线程完全退出后才继续
  (所以 start/resume 放阻塞线程池)。
- **reader 自身退出路径**:EOF / 读错 / 写盘失败 / 会话代号失效 / stream 代号失效 /
  会话为 None / remap 失败。无论哪条路径,退出前都 kill + wait 子进程,并在
  `StreamRuntime.task` 仍是自己代号时清掉它(避免误删后继任务)。

### 4.5 已知可接受的窄竞态(均有源码注释)

1. **mmap 截断 SIGBUS**(`mmap_source.rs`):两次 remap 之间外部进程截断文件,访问已消失页
   可能 SIGBUS。收缩重建路径已把窗口压到最小;Windows 上带活动映射的文件无法被外部截断
   (OS 报 1224),天然免疫,该场景仅 Unix 存在。
2. **导出取消删除文件的 TOCTOU**(`commands.rs` Phase C 注释,评审定级 Low):同路径快速
   连开两次导出时,旧任务的 `remove_file` 理论上可能落在新任务 `File::create` 之后。实际
   新任务需走完 Phase A/B 才建文件,时序几乎不可能交叉,且 create 会截断重建。

---

## 5. 关键流程

### 5.1 打开文件(`open_file`)

1. `stop_stream_task(Forget)`——切换会话前遗弃旧流。
2. 读配置,`Session::open_with_encoding`(加载书签侧车,不建索引)。
3. **先** `generation.fetch_add` 领新会话代号,作废过滤/搜索任务代号,**后**替换锁内会话;
   同步返回初始 `Status`。
4. spawn 后台索引线程,循环:
   `lock_session_if_current(my_gen)` → `index_step(8MB)`(内部顺带增量扫错误行)→
   释放锁 → `emit("index:progress", Status)` → `yield_now()`(减少与 get_rows 的锁争用)。
   代号失效或会话被清空即退出。
5. 索引完成时 `rerun_scans_after_index_done`:若有活跃 filter/search spec,各领新任务代号,
   spawn 完整重扫(索引边进行边看到的部分结果被完整结果替换)。

### 5.2 流式抓取(`start_logcat` → reader 线程)

1. `stop_stream_task(Forget)`;会话文件为 `<storage_dir>/logcat-<millis>.log`
   (storage_dir 缺省为配置目录下 `sessions/`),创建空文件并按文件名清理旧会话
   (只认 `logcat-<纯数字>.log`,保留最新 10 份,连同书签侧车删除,尽力而为)。
2. 对该文件建 Session(空文件),按写侧不变量换会话;构造 `StreamRequestState`
   (adb 路径、serial、buffers、会话路径、会话代号、since_timestamp)。
3. `spawn_logcat_stream` 拉起 adb 子进程与 reader 线程(见 §4.4)。
4. reader 主循环,每个 64KB 读块:
   - `read` → 以 append 模式**写入会话文件** + flush(引擎侧随后 remap 读到);
   - 持锁校验(会话代号 + stream 代号)→ `remap_and_index_step(8MB)`:
     remap 有**增长门控**(文件没长就不重映射)与**收缩重建**(reset=true);
   - 增量追加扫描:`scan_start = reset ? 0 : previous_total`,对
     `[scan_start, total_lines)` 跑 `append_filter_for_range` / `append_search_for_range`
     (append 结果 sort+dedup,截断重建后从 0 重扫,避免漏扫);
   - 锁外 emit:搜索活跃时发一条 `search:progress`(done=true,计数已含新命中),
     再发 `stream:append`(本块字节数 + 完整 Status)。
   前端对 `stream:append` 做 75ms 批处理(§7.4),后端不再额外节流。
5. **pause / resume**:pause 杀流保留 `last_request`;resume 读会话文件尾部 64KB,
   `last_log_timestamp` 提取最后时间戳,带 `-T <time>` 重启流续抓(尾部无可解析时间戳则
   全量重放)。**stop** 停流保留 last_request;**clear** 停流后
   `reset_stream_session_file`:**先置锁内会话为 None(释放 mmap)再截断文件**
   (顺序不可变:Windows 截断带活动映射的文件报 ERROR_USER_MAPPED_FILE,Unix 并发读旧
   mmap 会 SIGBUS),删书签侧车,重建 Session 换代号,回填 last_request 的会话代号。

### 5.3 过滤 / 搜索(`set_filter` / `search` + 分块扫描)

1. 命令入口:领**任务代号**,读当前**会话代号**;短临界区内
   `set_filter_pending` / `set_search_pending`(校验正则、存 spec)。
   过滤非激活(全选级别、无条件)或搜索空查询走**立即路径**:同步清空结果、直接 emit
   终态事件返回,不起后台任务。
2. 否则 spawn 扫描线程,核心是 `run_chunked_scan`:
   - 先持锁取 total_lines 快照;
   - 按 4096 行一块循环:**每块持锁**校验会话代号 + 任务代号,失效即返回 None(放弃);
     锁内跑 `filter_indexed_range` / `search_indexed_range`;锁外 `on_chunk` 回调 + `yield_now`。
3. 收尾(再次持锁校验后):`apply_*_results` 写回;若扫描期间 total_lines 又长了
   (流式追加),对 `[快照总数, 当前总数)` 补扫一段 `append_*_results`;emit 终态事件
   (`filter:done` / `search:progress{done:true}`)。
4. 搜索的进度事件节流:约每 65 536 行或**首命中出现**时发一条 done=false
   (首命中可提前上报是因为命中随前向扫描升序累积,`matches.first()` 即最终首命中);
   最终 done=true 不受节流影响。

### 5.4 分段导出(`export_logs` → `run_chunked_export`)

`export_logs` 起始即领导出代号(顺带作废还在跑的旧导出),入阻塞线程池。
`run_chunked_export` 不依赖 Tauri(进度经注入的回调发事件),可直接单测。四个阶段:

- **Phase 0(校验)**:持锁 `validate_export_target`(目标 ≠ 源文件、建父目录);
  range 模式校验 1-based 升序。
- **Phase A(补索引)**:循环 `index_step(8MB)` 直到索引完成;每轮锁外让出、检查导出代号
  (此阶段输出文件尚未创建,取消无半成品可删)。
- **Phase B(确定导出对象 `ExportSource`)**:
  - range 模式 → `Range { first, len }`(钳到总行数,不动过滤状态);
  - Filtered 且过滤激活 → 用当前 spec 经 `run_chunked_scan` **重算出局部命中数组**
    (不写回 session,不打扰 UI 状态);
  - 其余(All/Bookmarks/Errors、或 Filtered 未激活退化为 All)→ 持锁一次
    `export_plan_snapshot`。
- **Phase C(分批写盘)**:锁外 `File::create`;每批 4096 行:
  批前查导出代号(**失效 ⇒ drop writer、删半成品文件、返回 cancelled summary**)→
  持锁用 `append_line_range_bytes` / `append_sorted_lines_bytes` 把该批原始字节拷进缓冲 →
  释放锁 → `write_all` 落盘 → 进度回调(首批必发,其后每 65 536 行发一次)→ `yield_now`。
  结束 flush 并发 done=true 事件(带输出路径,供前端 toast「打开所在目录」);取消路径由
  外层补发 `cancelled: true` 终态事件。

### 5.5 切分(`split_log_file`)

入阻塞线程池,直接调 `logcore::split::split_file_with_progress`(不经 Session,与当前
会话无关);每完成一个分片(已 flush)emit `split:progress { parts, bytesProcessed }`。
返回 `SplitSummaryDto { parts: Vec<String>, totalBytes }`。

---

## 6. IPC 契约

### 6.1 命令(23 个,`src-tauri/src/lib.rs` 注册)

| 命令 | 入参 → 返回 | 说明 |
|---|---|---|
| `open_file` | `path` → `Status` | 打开文件,起后台索引 |
| `list_devices` | — → `DeviceListDto` | `adb devices -l`,5s 超时,阻塞池 |
| `start_logcat` | `StartLogcatRequest{deviceSerial?, command?, buffers[]}` → `StreamControlDto` | 起流,阻塞池 |
| `pause_logcat` | — → `StreamControlDto` | 停流,保留请求,paused=true |
| `resume_logcat` | — → `StreamControlDto` | 尾部时间戳 `-T` 续抓,阻塞池 |
| `stop_logcat` | — → `StreamControlDto` | 停流 |
| `clear_logcat` | — → `StreamControlDto` | 停流 + 截断会话文件重建 |
| `get_status` | — → `Status` | 状态快照 |
| `get_rows` | `view, start, count` → `Vec<Row>` | **count 硬上限 512**;view ∈ all/filtered/bookmarks/errors;未知 view / 无会话返回空 |
| `set_filter` | `FilterSpecDto` → `usize` | 立即路径同步返回;否则起后台任务,结果走 `filter:done` |
| `get_filtered_count` | — → `usize` | 当前结果行数 |
| `search` | `SearchSpecDto` → `SearchResult` | 空查询立即清空;否则起任务,结果走 `search:progress` |
| `search_next` | `fromLineNo, direction` → `Option<u64>` | 环绕导航(1-based 行号) |
| `toggle_bookmark` | `lineNo` → `bool` | 切换并立即写侧车 |
| `list_bookmarks` | — → `Vec<u64>` | 升序 1-based |
| `next_bookmark` | `fromLineNo, direction` → `Option<NavigationTargetDto>` | 在**当前结果序**内导航(带 resultIndex) |
| `line_to_result_index` | `lineNo` → `Option<NavigationTargetDto>` | 行号 → 当前结果下标(跳转用) |
| `get_minimap` | `buckets` → `MinimapDto` | 书签桶 + 错误密度桶 |
| `export_logs` | `ExportRequest{mode: "view"\|"range", view?, startLine?, endLine?, path}` → `ExportSummaryDto` | 分段导出,阻塞池,进度走事件 |
| `cancel_export` | — → `()` | 递增导出代号 |
| `split_log_file` | `SplitRequest{path, outDir, mode: "bytes"\|"lines", value}` → `SplitSummaryDto` | 切分,阻塞池 |
| `get_config` | — → `AppConfigDto` | 读 config.toml(带 configPath) |
| `set_config` | `AppConfigDto` → `AppConfigDto` | 归一化保存;更新会话编码并重扫过滤/搜索 |

所有 DTO 字段 camelCase(serde rename_all)。`Status` 结构:
`totalLines / filteredLines / bookmarkLines / errorLines / indexedBytes / totalBytes /
indexing / generation`——**generation 随事件下发**,前端据此丢弃旧会话的迟到事件。

### 6.2 事件

| 事件 | 载荷 | 发送时机 |
|---|---|---|
| `index:progress` | `Status` | 后台索引每步(8MB)一次;done 由 `indexing=false` 表达 |
| `filter:done` | `{filteredLines, generation}` | 过滤立即路径或后台任务收尾 |
| `search:progress` | `{scanned, matches, firstLine?, done, generation}` | 节流的中间进度(done=false)+ 终态(done=true);流式追加也会发 done=true 的增量汇总 |
| `stream:append` | `{appendedBytes, status, deviceSerial}` | 流式 reader 每个 64KB 读块处理完 |
| `split:progress` | `{parts, bytesProcessed}` | 每关闭一个分片(已 flush) |
| `export:progress` | `{writtenLines, writtenBytes, done, path?, cancelled}` | 首批 + 每 65 536 行 + 终态(成功带 path,取消带 cancelled=true) |

---

## 7. 前端要点(`src/`)

### 7.1 zustand store(`store/session.ts`)

单一 `useSession` store,职责:

- **会话身份**:`status`(含 generation)、`sessionId`(每 `beginSession` 自增,表格
  据此清缓存滚回顶部)、`sourcePath` / `sourceMode`("file" | "adb")。
- **事件守卫**:`setStatus` 只接受 `generation >=` 当前值的状态(丢旧会话迟到事件);
  `beginSession` 重置视图/搜索/选中/书签等派生状态。
- **过滤**:`filter`(FilterSpec)+ `filterRevision`(用户每次改条件自增,驱动 App 的
  220ms 防抖下发)+ `filterResultRevision`(结果集变化自增,驱动表格换纪元重拉)。
  `setFilteredLines(count, {invalidateRows})` 可选择只更新计数不作废行缓存
  (同一条件的重复 `filter:done` 不闪烁)。
- **搜索/导航**:`search` spec、`searchCount`、`currentSearchLine`、
  `selectedLine` / `selectedResultIndex`、`viewportResultIndex`(minimap 视口指示)、
  `scrollRequest {index, align, reason, nonce}`(nonce 防重放的一次性滚动请求)。
- **尾随状态机**(仅 adb 模式生效):`tailFollowing` 默认 true;
  `pauseTailFollowing(reason)`(点行/搜索/书签跳转/minimap/跳行/向上滚)关闭;
  `setTailFollowingFromViewport(isAtBottom, source)` 只在 `source==="user"` 时按
  "是否贴底"回写(程序化滚动不算);`requestTailFollow(index)` 发 align:"end" 的滚动请求;
  改过滤条件也会暂停尾随。
- **配置**:`appConfig` / `theme`;设备列表与 logcat 命令选择状态。

### 7.2 RowBlockCache(`lib/rowCache.ts`)

以 200 行(`WINDOW`)为块的行缓存,**64 块 LRU** 上限(约 12 800 行常驻)。
用 Map 插入序表达最近使用(命中/写入时 delete 再 set,超限逐出最旧)。
**纪元(epoch)防闪烁**:换过滤条件时只递增纪元、**不清缓存**——`get` 仍返回旧行避免白屏,
`isFresh` 因纪元不匹配返回 false 触发重拉,新数据到位后整块替换。
`updateRows` 供书签切换时原位更新驻留行。只有换会话(sessionId)才真正 `clear()`。

### 7.3 虚拟列表分块拉取(`components/LogTable.tsx`)

- TanStack Virtual(`useVirtualizer`,overscan 24,行高来自配置)驱动;行数 =
  `status.filteredLines`。
- **统一请求 `filtered` 视图**(引擎在过滤未激活时自动退化为 All),块大小 200 =
  `get_rows("filtered", blockStart, 200)`,远低于 512 上限。
- `ensureBlock`:块未 fresh 才拉;in-flight Map 去重并发请求;响应回来先校验纪元再写缓存。
  可见区间只保证首、尾两块(200 行块 + 24 overscan 下最多跨两块)。
- 缓存未命中的行渲染 "..." 占位,数据到位后 `force` 重渲。
- 滚动语义:程序化滚动(搜索定位/书签跳转/尾随)前 `markProgrammaticScroll`(160ms 窗口),
  scroll 事件里据此区分用户/程序,只有用户滚动影响尾随开关;滚轮向上立即暂停尾随;
  尾随的滚动请求(reason:"tail")先 `ensureBlock` 末块再滚到底。
- 其他:列宽拖拽/列显隐(持久化进配置)、按选区复制(按可见列拼 tab 文本)、双击切书签、
  右键范围标记菜单、搜索命中与关键词高亮(`lib/highlight.ts` 分词,search 命中优先于
  highlight 规则)。

### 7.4 事件接线与节流(`App.tsx` + `lib/streamAppend.ts`)

- `index:progress` → `setStatus`(store 内 generation 守卫)。
- `filter:done` → 比对 generation;用 dispatched/applied 两个请求序号判断"是否新条件的
  结果",决定 `invalidateRows`。
- `search:progress` → 只消费 done=true(中间进度目前仅供将来 UI 用)。
- `stream:append` → `createStreamAppendBatcher` **75ms 合并**(累加 appendedBytes,
  保最后一个 status)后:generation 守卫 → `setStatus`;若尾随开启则
  `requestTailFollow(filteredLines - 1)`。
- `export:progress` → done 时全局 toast(成功带「打开所在目录」,取消提示已取消);
  导出对话框自身另行监听做内联进度。
- 过滤条件 → **220ms 防抖**调 `set_filter`;`lastFilter` 600ms 防抖持久化;
  窗口尺寸 500ms 防抖持久化。快捷键:Ctrl/Cmd+O/F/G,F2/F3 书签导航。

### 7.5 Minimap(`components/Minimap.tsx` + `lib/minimap.ts`)

- 固定 180 桶(`MINIMAP_BUCKETS`);对 `get_minimap` 的调用做 **250ms 节流**
  (status/书签/过滤结果变化都可能触发)。
- 书签桶合并成连续区段渲染;错误刻度**密度渲染**:透明度 ∝ `count / 每桶行数`
  (`errorTickStyle`,0.16 基础透明度起步)。markedOnly 过滤时书签退化为整条连续带。
- 视口指示条位置由 `viewportResultIndex / (resultCount - 1)` 计算;拖拽经
  requestAnimationFrame 合帧,换算回结果下标后 `navigateToResultIndex`(reason:"minimap",
  同时暂停尾随)。

### 7.6 其他 lib

`lib/ipc.ts` 是唯一的 invoke/listen 封装层(命令名与事件名只出现在这里);
`lib/table.ts` 列定义与剪贴板格式;`lib/logcatCommand.ts` 与引擎 `LogcatSpec` 同规则的
前端预校验;`lib/recent.ts` 最近文件;`lib/sourceDisplay.ts` 路径缩略显示。

---

## 8. 性能基线与回归

基准工具:`crates/logcore/examples/bench.rs`。回归跑法:

```
cargo run --release -p logcore --example bench -- [GB] [文件路径]
# 文件存在且大小匹配时直接复用,不重新生成语料
```

10 GiB / 71,158,201 行实测(macOS Apple Silicon,release,单线程;完整报告:
[`docs/superpowers/2026-07-06-benchmark-10gb.md`](superpowers/2026-07-06-benchmark-10gb.md)):

| 指标 | 数值 |
|---|---|
| 索引耗时 / 吞吐 | 20.6s / 498 MB/s(3.46M 行/s) |
| 索引单步锁停顿(8MB 步进) | avg 16.1ms / **max 36.9ms** |
| 过滤(明文:级别/tag/关键词) | 5.3–5.8 M行/s |
| 过滤(正则多值) | 3.7 M行/s |
| 窗口读 get_rows(200 行) | All p99 **1.56ms** / Filtered p99 1.24ms |
| 搜索(明文不敏感) | 4.4 M行/s |
| 导出 All(10GiB) | **576 MB/s**(17.8s) |
| 导出 Filtered(3.56M 行/506MiB) | 36 MB/s |
| 私有内存峰值 | ≈1.37 GiB(含全部命中数组;max RSS 11.1 GiB 系 mmap 干净页,可随时被系统回收) |

历史教训:导出曾按行独立 `line_span`(每行从检查点回退平均多扫 ~512 行),吞吐仅
27/8 MB/s;改为 `for_each_line_span` 单次前向扫描的批量原语后提升 21×/4.5×。
新增行级读取路径时**务必复用批量原语**,不要逐行回退检查点。

---

## 9. 测试地图

本地全套验证(dev 分支日常依赖本地验证,不跑 CI):

```
cargo test -p logcore && cargo test -p log-filter && \
cargo clippy --workspace --all-targets && cargo fmt --all -- --check && \
pnpm typecheck && pnpm lint && pnpm test
```

### 9.1 logcore 单测(与实现同文件,`#[cfg(test)]`,约 100 个)

| 模块 | 覆盖面示例 |
|---|---|
| `indexer` | 行 span 正确性、分块步进 = 一次性步进、尾行追加、检查点不逐行存、跨检查点区间扫描 |
| `parser` | 两种格式解析、无冒号 tag、定宽填充 tag trim、多字节不 panic、借用式 = owned 等价、`level_byte_of_line` 语料等价 |
| `filter` | 级别掩码(全选保留 raw 行/空掩码)、markedOnly、pid/tid/tag 合取、include+exclude、`\|` 多值正则、非法正则报错 |
| `search` | 子串/正则/大小写折叠、元字符字面量化、Unicode、环绕导航 |
| `session`(31 个) | 视图取行与行号、过滤不物化默认结果、前沿行不吞未索引区、截断重建 + 重扫(Unix only)、增量追加过滤、书签持久化/导航、minimap 桶与密度、导出原语等价预言(批量 = 逐行拼接)、导出计划快照、目标校验、合成 10 万行吞吐/延迟阈值 |
| `adb` | devices 解析、设备选择、命令构造(含 `-T`)、spec 拒绝 shell 注入、尾部时间戳提取、预设归一化 |
| `bookmarks` / `config` / `split` / `encoding` / `export` / `mmap_source` | 各自的往返/归一化/边界(切分行对齐、超长行独占分片、进度回调;配置钳制与兼容迁移;编码映射) |

### 9.2 src-tauri 单测(`cargo test -p log-filter`,20 个)

- `commands.rs`:view 字符串解析、512 钳制、buffers 解析、流式会话清理(prune)、
  **`run_chunked_export` 等价性 ×3**(Filtered/All/Range 输出字节与旧 `export_view`/
  `export_range` oracle 完全一致,9000 行跨批)、导出期间换会话中止、
  **取消导出删除半成品**、`reset_stream_session_file` 先释放 mmap 再截断。
- `state.rs`:毒化锁恢复、四类任务代号作废语义、`lock_session_if_current` 拒绝旧代号。
- `dto.rs`:事件 camelCase 序列化快照、配置 DTO 归一化与预设往返。

### 9.3 前端 vitest(`pnpm test`,纯 lib/store 测试,不起 Tauri)

`lib/highlight`、`lib/logcatCommand`、`lib/minimap`、`lib/recent`、`lib/rowCache`
(LRU/纪元语义)、`lib/sourceDisplay`、`lib/streamAppend`(75ms 合并批)、`lib/table`,
以及 `store/session.test.ts`(尾随状态机、beginSession 重置、setFilteredLines 失效语义等)。

---

## 10. 分支与 CI(工程约定)

- **分支**:`main` 为主干;`dev` 为维护者使用的长期缓冲分支——维护者的日常工作可
  rebase 合入 dev(不跑 CI,依赖 §9 的本地全套验证),并按批次提交 dev→main PR。
  源码 PR 仅接受授权协作者提交;合并方式一律 rebase。main 合并后,维护者将 dev
  执行 `reset --hard main` 并通过 `--force-with-lease` 推送对齐。
- **CI**(`.github/workflows/ci.yml`):任何目标为 main 的 pull_request 都会触发完整三系统矩阵
  (ubuntu-latest / macos-latest / windows-latest),内容为 `pnpm typecheck/lint/test`、
  `cargo fmt --all -- --check`、`cargo test -p logcore`、`cargo test -p log-filter`、
  `cargo clippy --workspace --all-targets -- -D warnings`。纯文档改动(`docs/**`、`**.md`)
  不触发;main push 不触发;也可通过 `workflow_dispatch` 手动触发。
- **打包**(`.github/workflows/desktop-build.yml`,"Desktop Build"):仅 `workflow_dispatch`
  或 `v*` tag 触发,tauri-action 三平台产出 msi/nsis/dmg/deb;workspace 布局下产物在
  仓库根 `target/release/bundle/`(**不在** `src-tauri/target/`),artifact 上传设
  `if-no-files-found: error`。
