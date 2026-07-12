# LogFilter Autonomous Iterations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. User has explicitly selected autonomous inline execution with no mid-flight questions unless a listed stop condition occurs.

**Goal:** 按用户指定顺序完成引擎硬化、测试/工具链、M3 adb 实时抓取、平价 UX、M6 打包配置,每个小迭代均 TDD、验证、自审、提交。

**Architecture:** `logcore` 保持 UI 无关,用 mmap/索引/行号结果集支撑 10GB+ 文件;Tauri 命令只做薄封装、事件桥接和后台任务调度;前端继续只请求可见窗口并按设计稿呈现工具栏、表格、小地图和状态栏。

**Tech Stack:** Rust workspace (`logcore`, Tauri v2), Vite + React + TypeScript, Tailwind v4 CSS-first, Zustand, TanStack Virtual, pnpm.

---

## Global Rules

- 每个生产改动先写失败测试,运行并确认 RED,再写实现,再确认 GREEN。
- 每个提交前执行完整验证: `cargo test -p logcore`, `cargo build --workspace`, `pnpm typecheck` 或等效 `tsc`, `pnpm build`。新增 lint 后再加入 `pnpm lint`。
- 涉 adb 真机的迭代先动态执行 `adb devices`,选择当前 `device` 状态的在线序列号,再执行 `adb -s <serial> logcat -d -t 500 -v threadtime` 自证;不得写死序列号或 adb 路径。
- 后端不得向前端返回整文件或整文本结果;`get_rows` 保持 `count <= 512` 的热路径上限。
- 设计稿里的 `shell cat /proc/kmsg` 与规范非目标冲突,命令列表实现时按功能规范移除,并在总结标注偏差。

## File Map

- `crates/logcore/src/indexer.rs`: dense 行偏移索引替换为检查点/稀疏索引,保留 `line_span` 行定位能力。
- `crates/logcore/src/session.rs`: 会话、过滤/搜索、结果集、增量索引前沿重算、流式会话增长重映射入口。
- `crates/logcore/src/filter.rs`: 过滤规格、`highlight` 字段、匹配器纯函数测试。
- `crates/logcore/src/search.rs`: 搜索匹配器,去掉逐行 `to_lowercase` 分配。
- `crates/logcore/src/adb.rs`: adb 路径发现、`adb devices` 解析、logcat 子进程生命周期、stdout 写入会话文件。
- `crates/logcore/src/mmap_source.rs`: 支持实时文件增长后的重新映射。
- `src-tauri/src/state.rs`: 当前会话、后台任务、取消 token、stream 状态。
- `src-tauri/src/commands.rs`: 命令薄封装、后台 filter/search、adb 命令、事件发射。
- `src-tauri/src/dto.rs`: Rust/TS DTO,包括 filter highlight、adb device、stream status、config 最近文件/命令/窗口大小。
- `src/lib/ipc.ts`, `src/types.ts`: IPC 类型与事件监听。
- `src/store/session.ts`: Zustand 状态、最近文件、命令预设、实时跟随、搜索/过滤进度状态。
- `src/components/Toolbar.tsx`: 来源/设备/命令真下拉、运行控制、快捷入口。
- `src/components/LogTable.tsx`: 可见窗口获取、自动跟随尾部、跳转行号、剪贴板、highlight 渲染。
- `src/components/Minimap.tsx`: 当前结果集小地图纯逻辑测试入口。
- `src/components/StatusBar.tsx`: 编码/格式/设备状态不硬编码。
- `package.json`, `vite.config.ts`: Vitest、typecheck、ESLint/Prettier scripts。
- `src-tauri/tauri.conf.json`, `.github/workflows/release.yml`: M6 bundler 和 CI。

## Iteration 1: Engine Hardening

### Task 1.1: Sparse/checkpoint index

- [ ] Write RED tests in `crates/logcore/src/indexer.rs`:
  - dense storage is bounded by checkpoints for many lines.
  - `line_span` returns correct spans across checkpoint blocks.
  - chunked stepping and trailing newline behavior remain unchanged.
- [ ] Run `cargo test -p logcore indexer` and confirm the new tests fail for dense-only API.
- [ ] Implement a checkpoint index with a configurable stride, per-block newline deltas, and compatibility helpers for current callers.
- [ ] Run `cargo test -p logcore indexer` and fix until green.

### Task 1.2: Session row access over sparse index

- [ ] Write RED tests in `crates/logcore/src/session.rs`:
  - `get_rows_for_view` returns correct rows beyond multiple checkpoint blocks.
  - indexing frontiers do not expose unindexed tail text.
  - filtered/bookmark/error views continue using source line numbers.
- [ ] Run targeted session tests and confirm failure where APIs changed.
- [ ] Update session methods to avoid `offsets().len()` assumptions and use sparse row lookup/count APIs.
- [ ] Run `cargo test -p logcore session`.

### Task 1.3: Async filter/search with cancellation and frontier rerun

- [ ] Write RED Rust/Tauri tests for cancellation tokens and progress payload DTOs where pure logic can be isolated.
- [ ] Run targeted tests and confirm failure.
- [ ] Refactor `Session` to expose scan slices/chunks that do not hold the Tauri mutex for the whole scan.
- [ ] Add Tauri background filter/search task management with generation/cancel token, `filter:done` and `search:progress` events.
- [ ] On index completion, rerun the last active filter/search over the final frontier.
- [ ] Frontend listens for these events and updates result/search counts only for current generation.

### Task 1.4: Allocation-free insensitive plain search

- [ ] Write RED tests/instrumentable helper tests in `crates/logcore/src/search.rs` for ASCII and Unicode-insensitive search behavior without per-line lowercase allocation in the plain ASCII path.
- [ ] Run `cargo test -p logcore search`.
- [ ] Implement lowercase-once needle plus byte/char window comparison, using allocation only for non-ASCII fallback if necessary.
- [ ] Run `cargo test -p logcore search`.

### Task 1.5: Verification and commit

- [ ] Run full verification commands.
- [ ] Perform adversarial self-review for Critical/Important issues: lock duration, stale generation, index correctness, memory regressions, event ordering.
- [ ] Fix any Critical/Important issue and rerun full verification.
- [ ] Commit with a clear message such as `feat: harden engine indexing and async scans`.

## Iteration 2: Tests and Toolchain

- [ ] Add Vitest and tests for store transitions, highlight tokenization, minimap helpers, column config, clipboard formatting.
- [ ] Add Rust unit tests for `commands.rs`, `dto.rs`, `export.rs`.
- [ ] Add `pnpm typecheck`, ESLint, Prettier scripts and config.
- [ ] Add big-file performance regression tests/bench-style ignored tests for synthetic indexing throughput and row latency.
- [ ] Full verification, adversarial self-review, commit.

## Iteration 3: M3 adb Live Capture

- [ ] Add `logcore/src/adb.rs` tests for adb path discovery, `adb devices` parsing, command construction, process lifecycle state.
- [ ] Implement adb resolver from config path then PATH/common locations without hardcoding this machine.
- [ ] Implement stream writer: spawn `adb -s <serial> logcat -v threadtime -b <buffers>`, write stdout to session file, remap mmap on growth, index incrementally, pause/resume/stop/clear.
- [ ] Add Tauri commands `list_devices/start_logcat/pause_logcat/resume_logcat/stop_logcat/clear_logcat` and `stream:append` event.
- [ ] Wire frontend source/device/command dropdowns, logcat buffers, run/pause/stop/clear, auto-follow tail with scroll-up pause.
- [ ] Run full verification plus dynamic `adb devices` and `adb -s <serial> logcat -d -t 500 -v threadtime` snapshot self-test.
- [ ] Adversarial self-review, commit.

## Iteration 4: Parity and UX

- [ ] Add tests for config persistence: encoding, recent files max 10, last filter, command presets, window size.
- [ ] Wire encoding setting/status through `logcore`, Tauri DTO, store, settings dialog, status bar.
- [ ] Add recent files persistence and source menu entries.
- [ ] Implement drag-and-drop open.
- [ ] Implement jump to line dialog/input and shortcuts: Ctrl/Cmd+G, Ctrl/Cmd+O, Ctrl/Cmd+F.
- [ ] Replace status bar hardcoded strings with session/config/source state.
- [ ] Ensure toolbar source/device/command are true dropdowns following `LogWindow.dc.html`.
- [ ] Implement design empty state from `LogFilter.dc.html`.
- [ ] Add filter `highlight` field and multi-color keyword highlighting distinct from red search hit.
- [ ] Persist last filter, command presets, and window size.
- [ ] Full verification, adversarial self-review, commit.

## Iteration 5: M6 Packaging and CI

- [ ] Add/adjust Tauri bundler config for Windows MSI/NSIS EXE, macOS DMG, Linux DEB.
- [ ] Add GitHub Actions matrix workflow for cargo test/clippy, pnpm typecheck/lint/build, and `tauri build`.
- [ ] Run local full verification plus `pnpm tauri build` on macOS and verify DMG exists.
- [ ] Adversarial self-review, commit.
- [ ] Stop and report that real three-platform CI artifacts require user GitHub push/actions enablement.

