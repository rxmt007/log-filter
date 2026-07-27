# AGENTS.md — LogFilter 跨平台复刻

> 面向维护者与贡献者的工程指南。**权威设计以规范文档为准**,先读它:
> [`docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md`](docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md)

## 项目简介

把 2013 年的 Java Swing 工具 **LogFilter v1.8**(Android logcat 查看器)从零复刻为**跨平台桌面客户端**(Windows 主打,兼顾 macOS / Linux)。相较原版的核心增强:**支持 10GB+ 超大日志文件**——这是工程目标,已有 10GiB 实测佐证,见 [`docs/superpowers/2026-07-06-benchmark-10gb.md`](docs/superpowers/2026-07-06-benchmark-10gb.md)。

仓库:`github.com/rxmt007/log-filter`。

## 技术栈

- **后端引擎**:Rust,独立 `logcore` crate(mmap + 检查点索引 + 过滤 / 搜索),不依赖 UI
- **桌面壳**:Tauri v2
- **前端**:Vite + React 19 + TypeScript + **Tailwind v4(CSS-first;配置在 `src/index.css`,无 `tailwind.config.js`)** + shadcn/ui(Base UI · nova preset · Lucide 图标)
- **表格虚拟化**:TanStack **Virtual**(自研虚拟列表;**不用** shadcn Data Table)
- **前端状态**:zustand　**配置**:TOML

## 核心架构不变量

1. **只传可见窗口**:前端永不整体接收文件,一律经 `get_rows(view, start, count)` 取可见窗口,`count` 有上限(≤512 硬上限)。任何"把整文件 / 整过滤结果发给前端"的做法都禁止。
2. **引擎与 UI 解耦**:`logcore` 不依赖 Tauri / UI,解析 / 索引 / 过滤 / 搜索 / 切分全部可脱离界面单测。
3. **绝不整体载入**:文件用 mmap;过滤只产出**命中行号数组**(`Vec<u32/u64>`),不复制文本。
4. **纯函数先行 TDD**:解析器、过滤器等为纯函数,先写测试再写实现。
5. **不拷贝原始 Java 代码**:`./LogFilter` 仅作行为参考(带第三方版权、已 gitignore、成型后删除)。

## UI 设计稿(实现基准)

- **设计稿**:[`docs/design/LogFilter.dc.html`](docs/design/LogFilter.dc.html)(主界面 · 实现目标)、[`docs/design/LogWindow.dc.html`](docs/design/LogWindow.dc.html)(交互窗口稿)、`docs/design/support.js`(设计稿离线渲染脚本)。
- **实现规则**:后续迭代实现功能时,**若需求无其他说明,则严格按 UI 设计稿实现**;**当功能与 UI 设计冲突时,以功能为准,并在变更说明中明确记录差异**。

## 目录结构

```
crates/logcore/     纯 Rust 引擎(model/mmap_source/indexer/parser/filter/
                    search/bookmarks/session/adb/export/split/config)
src-tauri/          Tauri v2 应用(main / commands 薄封装 / events 进度事件)
src/                前端(components / lib/ipc.ts / hooks / store / types.ts)
docs/               规范与设计文档(superpowers/specs 设计、superpowers/plans
                    实施计划、superpowers/ 根下基准与验证报告)
.github/workflows/  ci.yml(验证矩阵)、desktop-build.yml(打包)
LogFilter/          原 Java 工程(只读参考,已忽略,将删除)
```

## 开发与命令

- 引擎单测:`cargo test -p logcore`;Tauri 侧测试:`cargo test -p log-filter`
- 桌面 dev:`pnpm tauri dev`
- **本地验证全集**(任何改动前后必须全绿,详见下文「协作流程」):

  ```sh
  cargo test -p logcore && cargo test -p log-filter \
    && cargo clippy --workspace --all-targets \
    && cargo fmt --all -- --check \
    && pnpm typecheck && pnpm lint && pnpm test
  ```

- **基准回归**:`cargo run --release -p logcore --example bench -- [GB] [文件路径]`(目标文件已存在且大小匹配则直接复用,不重新生成)
- 打包:`pnpm tauri build` → Windows `.msi`/`.exe`(nsis)、macOS `.dmg`、Linux `.deb`(Debian / Ubuntu 较新 LTS);**workspace 布局:产物在仓库根 `target/release/bundle/`,不在 `src-tauri/target/`**
- 包管理器:pnpm(pnpm 11 的构建脚本审批记录在 `pnpm-workspace.yaml`)

## 分支与 CI 策略

- **main** 为主干;**dev** 为维护者使用的长期缓冲分支:维护者的日常工作可 **rebase 合入 dev**,dev 上不跑 CI,依赖本地验证全集。
- 维护者可按批次开 **dev→main PR**;外部贡献请直接向 **main** 提交 PR;合并方式**一律 rebase**。
- main 合并后,维护者将 dev 对齐 main:`reset --hard main` 并 `push --force-with-lease` 推送对齐。
- **CI(`.github/workflows/ci.yml`)**:任何目标为 **main** 的 `pull_request` 都会触发完整三系统矩阵(ubuntu-latest / macos-latest / windows-latest),内容与本地验证全集一致:pnpm typecheck / lint / test、`cargo fmt --all -- --check`、`cargo test -p logcore`、`cargo test -p log-filter`、`cargo clippy --workspace --all-targets -- -D warnings`。
  - 仅修改 `docs/**` 或 `**.md` 的 PR 不触发;**main push 不触发**;也可通过 `workflow_dispatch` 手动触发。
- **打包(`.github/workflows/desktop-build.yml` "Desktop Build")**:仅 `workflow_dispatch` 或 `v*` tag 触发;`tauri-action` 三平台产出 msi / nsis / dmg / deb;artifact 上传设 `if-no-files-found: error`(防产物路径漂移静默漏传)。

## 贡献流程与约束

以下约定适用于所有维护者与贡献者:

1. **验证**:任何改动前后必须跑「开发与命令」中的本地验证全集,全绿才可宣称完成。
2. **TDD 与评审**:纯函数(解析 / 过滤 / 搜索等)先写测试再写实现;**实现与评审分离**,重要改动需由另一位贡献者或维护者独立审查,不得未经审查直接合并。
3. **大任务流程**:先写实施计划到 `docs/superpowers/plans/`(现有计划文档即模板),按任务拆分 → 逐任务实现 + 评审 → 最后整体终审。
4. **性能改动**:必须用 bench 例子跑回归,并把前后数字写进 `docs/superpowers/` 下的报告(参照 `2026-07-06-benchmark-10gb.md`)。
5. **隐私红线(写入仓库的任何内容)**:
   - 禁止出现真实姓名 / 姓名拼音;
   - 禁止真实本地路径(示例路径一律用 `/Users/alice/...` 或相对路径);
   - 禁止真实内网 IP(用 `192.168.x.x` 打码)。
6. **提交规范**:conventional commits(`feat:` / `fix:` / `perf:` / `test:` / `docs:` / `refactor:` / `chore:` …)。
7. **知识落盘**:`.superpowers/` 为会话级草稿目录(已 gitignore);持久知识必须落在 `docs/` 与本文件,**不得依赖任何会话记忆**。
8. **语言**:项目文档默认中文;引用外部 API / 代码标识可保留英文原文。

## IPC 接口与并发不变量

- **Tauri 命令**:`open_file`、`list_devices`、`start_logcat`、`pause_logcat`、`resume_logcat`、`stop_logcat`、`clear_logcat`、`get_status`、`get_rows`/`get_rows_checked`(≤512 行硬上限)、`map_source_line`、`set_filter`、`get_filtered_count`、`search`、`search_next`、`toggle_bookmark`、`list_bookmarks`、`next_bookmark`、`line_to_result_index`、`get_minimap`、`get_problems_status`、`get_problem_groups`、`get_problem_occurrences`、`get_problem_detail`、`release_problem_snapshot`、`export_logs`、`export_problem_logs`、`cancel_export`、`split_log_file`、`get_config`、`set_config`。
- **事件**:`index:progress`、`filter:done`、`search:progress`、`stream:append`、`stream:control`、`stream:error`、`problems:progress`、`split:progress`、`export:progress`。
- **并发不变量**:全局 Session 在 `Mutex` 内;写侧持锁时**先递增 session/analysis generation 再替换 session**;普通后台任务经 `lock_session_if_current`、Problems 经 `lock_analysis_if_current` 持锁校验;filter / search / export 各有任务代号(`next_*_generation`)实现取消。Problems 每个 1MiB 索引片后最多以 32 个独立短锁追赶,每步最多 4096 物理行/128 条详细行;`index:progress` 仍按累计 8MiB 或终态节流。

## 性能基线(10GiB / 7115 万行实测)

- 索引 20.6s(498 MB/s),索引单步锁停顿峰值 36.9ms
- 过滤:明文 5.3–5.8M 行/s,正则 3.7M 行/s;搜索 4.4M 行/s
- 窗口读 `get_rows` p99 1.6ms
- 导出:All 576 MB/s,Filtered 36 MB/s;私有内存峰值 ≈1.4GiB
- 完整数据与方法见 [`docs/superpowers/2026-07-06-benchmark-10gb.md`](docs/superpowers/2026-07-06-benchmark-10gb.md);性能改动以此为回归基线。
- Problems production 中位数:index + 分析 28.28s、索引最大宏步 28.08ms、
  Problems 最大锁段 3.03ms、扫描期窗口
  p99 0.974ms;standalone 重扫 8.6M 行/s、Problems 最大锁段 4.22ms、窗口 p99
  2.692ms;受控事件风暴 retained heap 42.47MiB。以上为同机同 corpus、未人为清理
  page cache 的三轮中位数，数值门槛通过，但仍保留单轮调度/I/O 尖峰；规范要求的可控
  冷/暖缓存各三次尚未完成，因此正式性能硬验收没有闭环，其他平台也未真机复测。
  完整口径见
  [`docs/superpowers/2026-07-28-problems-mvp-closure.md`](docs/superpowers/2026-07-28-problems-mvp-closure.md)。

## 关键约定

- **配置**:存各平台标准 app 配置目录(可配置位置),TOML 格式,**GUI 内也可修改**。
- **adb**:支持"可配置 adb 可执行文件路径" + 自动扫描常见位置;`adb devices` 选设备,`logcat` 子进程 run/pause/stop。platform-tools 内置为后续可选。
- **真机结论(Android 9 / MiTV)**:`logcat -T "MM-DD HH:MM:SS.mmm"` 续抓可用且包含边界时间戳(同毫秒有少量重复);threadtime 的 tag 为定宽空格填充(解析已 trim)。
- **解析**:仅 `-v time` 与 `-v threadtime` 两种格式,自动识别。
- **过滤**:7 类叠加(级别位掩码 / PID / TID / Tag 显示·排除 / 关键词 查找·排除),各带 enabled,`|` 多值,含**正则开关**。

## 范围提醒

- **不做**:iOS、kernel `/proc/kmsg`、移动端。
- **v1 不做**(留待后续):多标签会话、过滤器预设、platform-tools 内置、自动更新。
- **v1 新增(已实现)**:全局搜索、导出过滤结果(分块流式)、大文件切分。
- **track-devices 已规划未实施**(协议:4 位十六进制长度前缀 + `serial\tstate` 行;等内置 adb 决策),见 [`docs/superpowers/2026-07-06-adb-device-verification.md`](docs/superpowers/2026-07-06-adb-device-verification.md)。

## 备注

- UI 当前样式为**规划基线**,后续会再做一版交互设计。
- 索引步长、`get_rows` count 上限、DOM/canvas 渲染等细节,在实施中按实际功能 / 性能确定与调整。
