# Review 修复:稳定性与性能加固 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 2026-07-06 全量代码 review 的优先级,修复 2 个正确性缺陷(clear_logcat 截断顺序、generation 竞态)、4 个大文件性能热点(索引期全量解析、解析器分配、minimap 全量扫描、前端缓存无界),并完成导出/切分异步化、adb 加固与引擎内存优化。

**Architecture:** 全部修改遵循现有分层:logcore(纯引擎,无 Tauri 依赖)→ src-tauri(薄命令层)→ 前端(只经 `get_rows` 取可见窗口)。热路径改造原则:字节层/借用式优先,杜绝每行堆分配;并发改造原则:generation 校验必须与 session 锁在同一临界区内。

**Tech Stack:** Rust(memchr/memmap2/regex/encoding_rs)、Tauri v2、React 19 + zustand + TanStack Virtual、vitest。

## Global Constraints

- **铁律(AGENTS.md)**:前端只经 `get_rows(view, start, count)` 取行,`count ≤ 512`;`logcore` 不得引入 Tauri/UI 依赖;文件一律 mmap,过滤只产出命中行号数组;解析器/过滤器等纯函数先写测试再实现(TDD)。
- **验证命令**:`cargo test -p logcore`、`cargo test -p log-filter`、`pnpm test`、`pnpm typecheck`、`pnpm lint`。每个任务结束时四者(与该任务相关者)必须全绿。
- **依赖限制**:不新增运行时第三方依赖;唯一允许的新增是 `src-tauri` 的 dev-dependency `tempfile = "3"`。
- **提交规范**:conventional commits(`fix:`/`feat:`/`perf:`/`refactor:`/`test:`),中文或英文均可,与现有历史一致;每个任务至少一个独立 commit。
- **注释风格**:与现有代码一致——只在"代码本身无法表达的约束"处写中文注释;不写"这行做了什么"式注释。
- **平台**:Windows 专属代码用 `#[cfg(windows)]` 门控,macOS 上必须编译通过(开发机是 macOS)。
- **并发不变量**(Task 2 起全局生效):`AppState.generation` 必须先递增、后替换 `session`(`open_file`/`start_logcat`/`clear_logcat` 已满足);任何后台任务对 session 的读写必须使用 `lock_session_if_current` 在持锁状态下校验代号。

## 任务依赖

Task 1、2 独立(commands/state 层)。Task 3 → Task 4(同在 parser/session 热路径,4 基于 3 的产物)。Task 5 依赖 4(session.rs 已稳定)。Task 6 纯前端,独立。Task 7、8 依赖 1、2(commands.rs 已重排)。Task 9 依赖 4、5(session 内部类型)。Task 10 最后(清理)。执行顺序:1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10。

---

### Task 1: clear_logcat 截断顺序修复(P0 正确性)

**背景**:`clear_logcat` 在旧 `Session` 仍持有目标文件 mmap 时执行 `File::create(&path)` 截断。Windows 上截断带活动映射的文件会报 `ERROR_USER_MAPPED_FILE`(清空功能必然失败);Unix 上截断成功但并发 `get_rows` 读旧 mmap 已截掉的页会 SIGBUS 崩溃。必须先 drop 旧 Session(释放 mmap)再截断。顺带清掉该路径的书签 sidecar(否则旧行号书签会错误标记新日志)。

**Files:**
- Modify: `src-tauri/src/commands.rs`(`clear_logcat`,约 484-509 行;新增私有函数 `reset_stream_session_file`)
- Modify: `src-tauri/Cargo.toml`(新增 `[dev-dependencies] tempfile = "3"`)
- Test: `src-tauri/src/commands.rs` 内置 `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `fn reset_stream_session_file(state: &AppState, path: &Path, encoding: logcore::encoding::TextEncoding) -> Result<u64, String>` — 返回新 session generation。Task 2 的不变量(先递增 generation、后替换 session)在此函数内必须成立。

- [ ] **Step 1: 加 dev-dependency**

`src-tauri/Cargo.toml` 追加:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: 写失败测试**

在 `src-tauri/src/commands.rs` 的 `mod tests` 中追加(需要 `use std::sync::atomic::Ordering;` 已存在于文件顶部):

```rust
#[test]
fn reset_stream_session_file_drops_old_mmap_then_truncates() {
    let state = AppState::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("logcat-1.log");
    std::fs::write(&path, "04-20 12:06:02.125   146   179 D T: one\n").unwrap();
    let mut session = logcore::session::Session::open(&path).unwrap();
    session.index_all();
    session.toggle_bookmark(1).unwrap();
    *state.lock_session() = Some(session);
    let before = state.generation.load(Ordering::SeqCst);

    let generation =
        reset_stream_session_file(&state, &path, logcore::encoding::TextEncoding::Utf8).unwrap();

    assert_eq!(generation, before + 1);
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    assert!(!logcore::bookmarks::sidecar_path_for(&path).exists());
    let guard = state.lock_session();
    assert_eq!(guard.as_ref().unwrap().total_lines(), 0);
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p log-filter reset_stream_session_file`
Expected: 编译失败,`reset_stream_session_file` 未定义。

- [ ] **Step 4: 实现**

在 `commands.rs` 中新增(放在 `clear_logcat` 之前):

```rust
/// 重建流式会话文件。顺序不可变:必须先 drop 旧 Session(释放 mmap),再截断文件——
/// Windows 上截断带活动映射的文件报 ERROR_USER_MAPPED_FILE;Unix 上并发读旧 mmap 会 SIGBUS。
fn reset_stream_session_file(
    state: &AppState,
    path: &std::path::Path,
    encoding: logcore::encoding::TextEncoding,
) -> Result<u64, String> {
    *state.lock_session() = None;
    File::create(path).map_err(|err| err.to_string())?;
    let _ = fs::remove_file(logcore::bookmarks::sidecar_path_for(path));
    let session = logcore::session::Session::open_with_encoding(path, encoding)
        .map_err(|err| err.to_string())?;
    let session_generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.next_filter_task_generation();
    state.next_search_task_generation();
    *state.lock_session() = Some(session);
    Ok(session_generation)
}
```

`clear_logcat` 中原来的 `File::create(&path)`、`Session::open_with_encoding`、generation 递增、`*state.lock_session() = Some(session)` 一整段(约 495-502 行)替换为:

```rust
let session_generation =
    reset_stream_session_file(state.inner(), &path, config_encoding(&config))?;
```

后续 `runtime.last_request.session_generation = session_generation` 保持不变。

- [ ] **Step 5: 运行测试确认通过**

Run: `cargo test -p log-filter`
Expected: 全部 PASS。

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/Cargo.toml Cargo.lock
git commit -m "fix: drop session mmap before truncating stream file on clear"
```

---

### Task 2: generation 校验收进 session 锁内(P0 正确性)

**背景**:`spawn_filter_task` / `spawn_search_task` / stream reader 都是"先读 generation 原子、再另行加 session 锁"。两步之间 `open_file` 可能换掉会话,旧任务会把旧文件的行号 apply 到新 Session 上。因为写侧顺序固定为"先递增 generation、后替换 session",所以只要**持锁后**再校验 generation 即可排除该竞态。

**Files:**
- Modify: `src-tauri/src/state.rs`(新增 `lock_session_if_current`)
- Modify: `src-tauri/src/commands.rs`(`spawn_filter_task` 约 687-744 行、`spawn_search_task` 约 757-831 行、`spawn_stream_reader` 循环体约 264-296 行)
- Test: `src-tauri/src/state.rs` 内置 tests

**Interfaces:**
- Produces: `AppState::lock_session_if_current(&self, session_generation: u64) -> Option<MutexGuard<'_, Option<Session>>>`。后续所有任务(7、9、10)对 session 的后台访问一律使用它。

- [ ] **Step 1: 写失败测试**(`state.rs` tests 模块)

```rust
#[test]
fn lock_session_if_current_rejects_stale_generation() {
    let state = AppState::new();
    let first = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    assert!(state.lock_session_if_current(first).is_some());

    let second = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    assert!(state.lock_session_if_current(first).is_none());
    assert!(state.lock_session_if_current(second).is_some());
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p log-filter lock_session_if_current`
Expected: 编译失败,方法未定义。

- [ ] **Step 3: 实现**(`state.rs`,`impl AppState` 内)

```rust
/// 持锁校验会话代号。依赖写侧顺序"先递增 generation、后替换 session"
/// (open_file / start_logcat / reset_stream_session_file),
/// 因此持锁后代号仍匹配 ⇒ guard 里就是该代号对应的会话。
pub fn lock_session_if_current(
    &self,
    session_generation: u64,
) -> Option<MutexGuard<'_, Option<Session>>> {
    let guard = self.lock_session();
    if self.generation.load(Ordering::SeqCst) == session_generation {
        Some(guard)
    } else {
        None
    }
}
```

- [ ] **Step 4: 替换三处调用点**

`spawn_filter_task` 分块循环体(原"检查 generation → 单独 lock"两段)改为:

```rust
let chunk = {
    let Some(guard) = app_state.lock_session_if_current(session_generation) else {
        return;
    };
    if !app_state.is_current_filter_task(task_generation) {
        return;
    }
    match guard.as_ref() {
        Some(session) => session.filter_indexed_range(&matcher, start, end),
        None => return,
    }
};
```

收尾 apply 段同构改写:

```rust
let filtered_lines = {
    let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
        return;
    };
    if !app_state.is_current_filter_task(task_generation) {
        return;
    }
    match guard.as_mut() {
        Some(session) => { /* 原 apply + 追加扫描逻辑不变 */ }
        None => return,
    }
};
```

`spawn_search_task` 的分块循环与收尾段做完全相同的改写(用 `is_current_search_task`)。

`spawn_stream_reader` 循环体内原:

```rust
if app_state.generation.load(Ordering::SeqCst) != session_generation
    || !app_state.is_current_stream_task(stream_generation)
{
    break;
}
```

删除;`let update = { let mut guard = app_state.lock_session(); ... }` 改为:

```rust
let update = {
    let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
        break;
    };
    if !app_state.is_current_stream_task(stream_generation) {
        break;
    }
    let Some(session) = guard.as_mut() else {
        break;
    };
    /* 原 remap/index/append 逻辑不变 */
};
```

注意:reader 循环开头原有的"读原子、不加锁"快速退出检查**随本次改写删除**(上方代码已体现)——持锁校验是决定性防线,预检查只省一次读写、不值得留两套判定。

- [ ] **Step 5: 运行测试**

Run: `cargo test -p log-filter && cargo test -p logcore`
Expected: 全部 PASS。

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/commands.rs
git commit -m "fix: check session generation under the session lock in background tasks"
```

---

### Task 3: 错误行扫描去解析化(P0 性能)

**背景**:`index_step` 每步都经 `refresh_error_lines` 对**每一行**做 decode + `parse_line`(每行 1 个 Vec + ~7 个 String 分配),把 memchr 级索引拖成全量解析,10GB 索引从秒级变分钟级,且全程持 session 锁。错误行识别只需要级别字符,可在字节层完成、零分配。

**Files:**
- Modify: `crates/logcore/src/parser.rs`(新增 `level_byte_of_line` 及私有辅助)
- Modify: `crates/logcore/src/session.rs`(`refresh_error_lines`,约 652-665 行)
- Test: `crates/logcore/src/parser.rs` tests

**Interfaces:**
- Produces: `pub fn level_byte_of_line(line: &[u8]) -> Option<u8>` — 语义与 `parse_line(text).level` 一致(threadtime → time → None)。Task 4 不改动它。

**语义边界(实现者须知)**:字节版以 ASCII 空白分词;`split_whitespace` 认的非 ASCII 空白(如 U+00A0)在字节版中不认,此类怪异行会被判为"无级别"——与解析回退方向一致,可接受。logcat 头部字段全为 ASCII,UTF-8/GBK 等 ASCII 兼容编码下判定正确。

- [ ] **Step 1: 写失败测试**(`parser.rs` tests)

```rust
#[test]
fn level_byte_matches_parse_line_level_on_corpus() {
    let corpus = [
        "04-20 12:06:02.125   146   179 D BatteryService: update start",
        "04-20 12:06:02.425   300   330 E Payment: SocketTimeoutException",
        "04-20 12:06:02.425   300   330 F Zygote: fatal",
        "04-20 12:06:02.125   146   179 E NoColonTag message without delimiter",
        "04-17 09:01:18.910 D/LightsService(  139): BKL : 106",
        "04-17 09:01:18.910 E/Crash(1): boom",
        "04-17 09:01:18.910 E/NoParen: message",
        "--------- beginning of main",
        "04-20 12:06:02.125   abc   179 D T: bad pid",
        "04-20 12:06:02.125   146   179 X T: bad level",
        "04-20 12:06:02.125 146 179 E",
        "01-01 00:00:00.000 中文消息 hello",
        "01-01 00:00:00.000 中/x(1): y",
        "",
        "   ",
        "04-20 12:06:02.425   300   330 E Payment: with newline\n",
        "04-17 09:01:18.910 F/Crash(1): crlf\r\n",
    ];
    for line in corpus {
        let expected = parse_line(line).level;
        let got = level_byte_of_line(line.as_bytes())
            .map(|b| (b as char).to_string())
            .unwrap_or_default();
        assert_eq!(got, expected, "line: {line:?}");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p logcore level_byte`
Expected: 编译失败,函数未定义。

- [ ] **Step 3: 实现**(`parser.rs`)

```rust
/// 零分配地判断一行的日志级别字节(b'V'..b'F'),语义与 parse_line(...).level 一致。
/// 仅供索引期错误行扫描等热路径使用;以 ASCII 空白分词,对 ASCII 兼容编码有效。
pub fn level_byte_of_line(line: &[u8]) -> Option<u8> {
    threadtime_level_byte(line).or_else(|| time_level_byte(line))
}

fn ascii_tokens(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|b| b.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
}

fn threadtime_level_byte(line: &[u8]) -> Option<u8> {
    let mut tokens = ascii_tokens(line);
    let _date = tokens.next()?;
    let _time = tokens.next()?;
    let pid = tokens.next()?;
    let tid = tokens.next()?;
    let level = tokens.next()?;
    let _tail = tokens.next()?; // 与 parse_threadtime 一致:至少 6 个 token
    if !pid.iter().all(u8::is_ascii_digit) || !tid.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if level.len() != 1 || !b"VDIWEF".contains(&level[0]) {
        return None;
    }
    Some(level[0])
}

fn time_level_byte(line: &[u8]) -> Option<u8> {
    let rest = rest_after_ascii_tokens(line, 2)?;
    let level = *rest.first()?;
    if !b"VDIWEF".contains(&level) || rest.get(1) != Some(&b'/') {
        return None;
    }
    let after = &rest[2..];
    let open = after.iter().position(|b| *b == b'(')?;
    let close = after.iter().position(|b| *b == b')')?;
    if close < open {
        return None;
    }
    Some(level)
}

fn rest_after_ascii_tokens(line: &[u8], n: usize) -> Option<&[u8]> {
    let mut rest = trim_ascii_start(line);
    for _ in 0..n {
        let ws = rest.iter().position(|b| b.is_ascii_whitespace())?;
        rest = trim_ascii_start(&rest[ws..]);
    }
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p logcore`
Expected: 全部 PASS。

- [ ] **Step 5: 改写 `refresh_error_lines`**(`session.rs`)

```rust
fn refresh_error_lines(&mut self) {
    let frontier = self.indexed_frontier();
    let total = self.indexer.total_lines();
    let bytes = self.source.bytes();
    for (idx, (span_start, span_end)) in (self.error_scan_lines..total).zip(self.indexer.line_spans(
        bytes,
        self.error_scan_lines,
        total,
        frontier,
    )) {
        if matches!(
            crate::parser::level_byte_of_line(&bytes[span_start..span_end]),
            Some(b'E') | Some(b'F')
        ) {
            self.error_lines.push(idx as u64);
        }
    }
    self.error_scan_lines = total;
}
```

- [ ] **Step 6: 运行全部引擎测试**(`error_view_returns_error_and_fatal_rows` 等既有测试即等价性回归)

Run: `cargo test -p logcore`
Expected: 全部 PASS,含 `synthetic_large_log_indexing_and_window_reads_stay_fast`。

- [ ] **Step 7: Commit**

```bash
git add crates/logcore/src/parser.rs crates/logcore/src/session.rs
git commit -m "perf: scan error lines at byte level without per-line parsing"
```

---

### Task 4: 借用式解析器,过滤/搜索热路径零分配(P0 性能)

**背景**:`parse_threadtime` 用 `split_whitespace().collect::<Vec<_>>()` 收集整行 token,`LogEntry` 七个字段全 owned `String`;过滤/搜索每行走这条路,几千万行 = 几亿次堆分配。改为借用式 `ParsedLine<'a>`,owned `LogEntry` 仅保留给 `get_rows` → IPC 边界。

**Files:**
- Modify: `crates/logcore/src/parser.rs`(新增 `ParsedLine<'a>` + `parse_line_ref`/`parse_threadtime_ref`/`parse_time_ref`;旧函数改为薄包装)
- Modify: `crates/logcore/src/model.rs`(删除死代码 `LogLevel`;`LogEntry` 增加 `as_parsed()`;新增 `From<ParsedLine<'_>> for LogEntry`)
- Modify: `crates/logcore/src/filter.rs`(`FilterMatcher::is_match*` 参数改 `&ParsedLine<'_>`;新增 `requires_mark()`)
- Modify: `crates/logcore/src/search.rs`(`is_entry_match` 参数改 `&ParsedLine<'_>`)
- Modify: `crates/logcore/src/session.rs`(`filter_indexed_range`/`search_indexed_range` 用 `parse_line_ref`,书签查询按需)
- Test: 各文件既有 tests 全部适配并保持通过

**Interfaces:**
- Produces:
  - `pub struct ParsedLine<'a> { pub date: &'a str, pub time: &'a str, pub level: &'a str, pub pid: &'a str, pub tid: &'a str, pub tag: &'a str, pub message: &'a str }`(derive `Debug, Clone, Copy, PartialEq, Eq, Default`)
  - `pub fn parse_line_ref(line: &str) -> ParsedLine<'_>`(threadtime → time → raw 回退,与旧 `parse_line` 语义逐字段一致)
  - `impl LogEntry { pub fn as_parsed(&self) -> ParsedLine<'_> }`
  - `impl From<ParsedLine<'_>> for LogEntry`
  - `FilterMatcher::is_match(&self, entry: &ParsedLine<'_>) -> bool`、`is_match_with_mark(&self, entry: &ParsedLine<'_>, marked: bool) -> bool`、`requires_mark(&self) -> bool`(= `spec.marked_only`)
  - `SearchMatcher::is_entry_match(&self, entry: &ParsedLine<'_>) -> bool`
- Consumes: Task 3 的 `level_byte_of_line`(不动)。

**关键实现约束:**
1. `parse_threadtime_ref` **禁止 collect**:用 `split_whitespace()` 迭代器 `next()` 取前 5 个 token 校验(pid/tid 全数字、level 单字符 ∈ VDIWEF),tag/message 仍用现有 `rest_after_tokens(line, 5)` + `find(':')` 逻辑,返回 `&str` 切片:

```rust
pub fn parse_threadtime_ref(line: &str) -> Option<ParsedLine<'_>> {
    let mut tokens = line.split_whitespace();
    let date = tokens.next()?;
    let time = tokens.next()?;
    let pid = tokens.next()?;
    let tid = tokens.next()?;
    let level = tokens.next()?;
    if !is_all_ascii_digits(pid) || !is_all_ascii_digits(tid) {
        return None;
    }
    if level.len() != 1 || !"VDIWEF".contains(level) {
        return None;
    }
    let tail = rest_after_tokens(line, 5)?;
    let (tag, message) = if let Some(colon) = tail.find(':') {
        (&tail[..colon], tail[colon + 1..].trim_start())
    } else if let Some(ws) = tail.find(char::is_whitespace) {
        (&tail[..ws], tail[ws..].trim_start())
    } else {
        (tail, "")
    };
    Some(ParsedLine { date, time, level, pid, tid, tag, message })
}
```

2. `parse_time_ref` 从现有 `parse_time` 机械改写(所有 `to_string()` 去掉,返回切片;`level` 字段用 `&rest[..slash_idx]`,即级别字符的单字符切片;多字节安全逻辑原样保留)。
3. `parse_line_ref`:`trim_end_matches(['\r','\n'])` 后 threadtime → time → `ParsedLine { message: line, ..Default::default() }`。
4. 旧 API 保留为包装,现有测试的断言不改语义:`parse_line(line) = LogEntry::from(parse_line_ref(line))`,`parse_threadtime(line) = parse_threadtime_ref(line).map(Into::into)`,`parse_time` 同。
5. `filter.rs`/`search.rs` 的 `filter_entries`/`search_entries`(仅测试在用)改为内部 `entry.as_parsed()` 适配,函数签名不变。
6. `session.rs` 热路径:

```rust
pub fn filter_indexed_range(&self, matcher: &FilterMatcher, start: usize, end: usize) -> Vec<u64> {
    let frontier = self.indexed_frontier();
    let end = end.min(self.total_lines());
    let mut matches = Vec::new();
    for (idx, (span_start, span_end)) in (start.min(end)..end).zip(self.indexer.line_spans(
        self.source.bytes(),
        start,
        end,
        frontier,
    )) {
        let text = self.encoding.decode(&self.source.bytes()[span_start..span_end]);
        let entry = crate::parser::parse_line_ref(&text);
        let marked = matcher.requires_mark() && self.is_bookmarked(idx as u64 + 1);
        if matcher.is_match_with_mark(&entry, marked) {
            matches.push(idx as u64);
        }
    }
    matches
}
```

   `search_indexed_range` 同构(`parse_line_ref` + `is_entry_match`)。`get_rows_for_view` 路径继续走 `parse_source_span → LogEntry`(IPC 需要 owned),内部改为 `LogEntry::from(parse_line_ref(&text))`。
7. `model.rs` 删除 `LogLevel` 枚举及其测试(全仓无生产调用)。

- [ ] **Step 1: 先写 `ParsedLine` 等价性测试**(parser.rs tests;红)

```rust
#[test]
fn parse_line_ref_matches_owned_parse_line() {
    let corpus = [
        "04-20 12:06:02.125   146   179 D BatteryService: update start",
        "04-20 12:06:02.125   146   179 E NoColonTag message without delimiter",
        "04-17 09:01:18.910 D/LightsService(  139): BKL : 106",
        "--------- beginning of main",
        "01-01 00:00:00.000 中文消息 hello",
        "01-01 00:00:00.000 中/x(1): y",
        "",
    ];
    for line in corpus {
        let owned = parse_line(line);
        let parsed = parse_line_ref(line);
        assert_eq!(parsed.date, owned.date, "line: {line:?}");
        assert_eq!(parsed.time, owned.time, "line: {line:?}");
        assert_eq!(parsed.level, owned.level, "line: {line:?}");
        assert_eq!(parsed.pid, owned.pid, "line: {line:?}");
        assert_eq!(parsed.tid, owned.tid, "line: {line:?}");
        assert_eq!(parsed.tag, owned.tag, "line: {line:?}");
        assert_eq!(parsed.message, owned.message, "line: {line:?}");
    }
}
```

- [ ] **Step 2: 实现 parser/model 改造,测试转绿**(先只动 parser.rs + model.rs,旧函数变包装,全量 `cargo test -p logcore` 通过)
- [ ] **Step 3: Commit 第一段**

```bash
git add crates/logcore/src/parser.rs crates/logcore/src/model.rs
git commit -m "perf: add zero-alloc ParsedLine parser, keep LogEntry as IPC wrapper"
```

- [ ] **Step 4: filter/search 签名改造**(is_match* 收 `&ParsedLine`;filter.rs/search.rs 各自 tests 里构造 `LogEntry` 的辅助不变,经 `as_parsed()` 适配;`FilterMatcher` 内 `include_exact`/`include_contains` 等逻辑不变)
- [ ] **Step 5: session 热路径改造**(上文第 6 点代码)
- [ ] **Step 6: 全量验证**

Run: `cargo test -p logcore && cargo test -p log-filter`
Expected: 全部 PASS;`synthetic_large_log_indexing_and_window_reads_stay_fast` 通过。

- [ ] **Step 7: Commit**

```bash
git add crates/logcore/src crates/logcore/Cargo.toml src-tauri/src
git commit -m "perf: filter and search on borrowed ParsedLine without per-line allocation"
```

---

### Task 5: minimap 反向遍历 + 前端节流(P1)

**背景**:过滤激活时 `Session::minimap` 遍历**全部**过滤结果(每行两次二分);前端 effect 依赖 `status.filteredLines/errorLines`,索引每 8MB、流式每 75ms 都触发一次全量扫描。改为遍历小集合(书签、错误行)反查 `filtered`,复杂度 O(k log n);前端加 250ms 拖尾合并。

**Files:**
- Modify: `crates/logcore/src/session.rs`(`minimap`,约 191-223 行;可删除不再使用的 `current_result_source_idx`/`source_idx_is_error`,若仍有他处引用则保留)
- Modify: `src/components/Minimap.tsx`(约 81-96 行 effect)
- Test: `crates/logcore/src/session.rs` 既有 minimap 测试为等价回归;新增一条过滤态断言

- [ ] **Step 1: 新增回归测试**(session.rs tests;当前实现下应直接通过,作为改写保护网)

```rust
#[test]
fn filtered_minimap_marks_only_buckets_containing_hits() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    for i in 0..8 {
        let level = if i == 6 { "E" } else { "I" };
        writeln!(f, "04-20 12:06:02.{i:03}   300   330 {level} Payment: m{i}").unwrap();
    }
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    s.toggle_bookmark(2).unwrap();
    // 过滤后结果为行 2(书签, result 0)与行 7(错误, result 1)
    s.set_filter(&FilterSpec {
        word_include: FilterField::plain(true, "m1|m6"),
        ..Default::default()
    })
    .unwrap();

    let map = s.minimap(4);
    assert_eq!(map.bookmarks, vec![0]);
    assert_eq!(map.errors, vec![2]);
}
```

Run: `cargo test -p logcore filtered_minimap` → PASS(保护网就位)。

- [ ] **Step 2: 改写 `minimap` 过滤分支**

```rust
pub fn minimap(&self, buckets: usize) -> Minimap {
    let total = self.current_result_len();
    if buckets == 0 || total == 0 {
        return Minimap {
            bookmarks: Vec::new(),
            errors: Vec::new(),
        };
    }
    if !self.filter_active {
        return self.source_minimap(buckets);
    }
    // 反向遍历:书签/错误行是小集合,逐个二分反查在过滤结果中的位置,
    // 避免 O(过滤结果总数) 的全量扫描(minimap 会被状态事件高频触发)。
    let bookmarks = self
        .bookmark_source_lines()
        .into_iter()
        .filter_map(|line_no| self.filtered.binary_search(&(line_no - 1)).ok())
        .filter_map(|result_idx| bucket_for_zero_based(result_idx, total, buckets))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let errors = self
        .error_lines
        .iter()
        .filter_map(|idx| self.filtered.binary_search(idx).ok())
        .filter_map(|result_idx| bucket_for_zero_based(result_idx, total, buckets))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Minimap { bookmarks, errors }
}
```

Run: `cargo test -p logcore` → 全部 PASS(含既有 `minimap_uses_current_filtered_result_buckets`、`minimap_returns_bookmark_and_error_buckets`)。

- [ ] **Step 3: 前端节流**(Minimap.tsx)

在组件内加 `const throttleRef = useRef<number | null>(null);`,数据拉取 effect 改为:

```tsx
useEffect(() => {
  if (!status.totalBytes) {
    setData({ bookmarks: [], errors: [] });
    return;
  }
  if (throttleRef.current != null) return;
  throttleRef.current = window.setTimeout(() => {
    throttleRef.current = null;
    getMinimap(MINIMAP_BUCKETS)
      .then(setData)
      .catch(() => setData({ bookmarks: [], errors: [] }));
  }, 250);
}, [
  status.totalBytes,
  status.filteredLines,
  status.errorLines,
  sessionId,
  bookmarkRevision,
  filterResultRevision,
]);

useEffect(
  () => () => {
    if (throttleRef.current != null) window.clearTimeout(throttleRef.current);
  },
  [],
);
```

(拖尾定时器触发时才真正调用 `getMinimap`,读到的是触发时刻的最新数据,天然合并突发。)

- [ ] **Step 4: 验证**

Run: `cargo test -p logcore && pnpm typecheck && pnpm lint && pnpm test`
Expected: 全绿。

- [ ] **Step 5: Commit**

```bash
git add crates/logcore/src/session.rs src/components/Minimap.tsx
git commit -m "perf: invert filtered minimap scan and throttle frontend fetches"
```

---

### Task 6: 前端行缓存限界(P1)

**背景**:`LogTable` 的 `cache.current: Map<number, Row>` 只在换会话时清空,大文件滚动会把所有取过的行永久留在 webview 内存,等效把整个文件慢慢搬进前端。抽出带 LRU 上限的块缓存模块。

**Files:**
- Create: `src/lib/rowCache.ts`
- Create: `src/lib/rowCache.test.ts`
- Modify: `src/components/LogTable.tsx`(替换 `cache`/`filledEpoch` 两个 ref,约 162-163、405-423、444-468、525-577、664 行等引用点)

**Interfaces:**
- Produces(`src/lib/rowCache.ts`):

```ts
import type { Row } from "@/types";

interface Block {
  rows: Map<number, Row>; // key: 全局 result index
  count: number;          // 本块实际填充行数
  epoch: number;          // 填充时的缓存纪元
}

export class RowBlockCache {
  constructor(maxBlocks: number);
  /** 命中返回行,并把所属块提升为最近使用。 */
  get(index: number, blockSize: number): Row | undefined;
  /** 该块在给定纪元下是否已填充到 want 行(决定是否需要重新拉取)。 */
  isFresh(blockStart: number, want: number, epoch: number): boolean;
  /** 写入一个块;超出 maxBlocks 时逐出最久未使用的块。 */
  fill(blockStart: number, rows: Row[], epoch: number): void;
  clear(): void;
  /** 对所有驻留行应用变换(书签状态更新用)。 */
  updateRows(update: (row: Row) => Row): void;
  blockCount(): number;
}
```

- 实现要点:`Map<number, Block>` 以插入序做 LRU(`get`/`fill` 命中时 delete + 重新 set);`fill` 后若 `blocks.size > maxBlocks`,删除 `blocks.keys().next()`(最旧)。**纪元语义与现状保持一致**:换过滤条件时 LogTable 只递增 epoch 不 clear,旧行继续显示直到新数据覆盖(避免闪烁),`isFresh` 因 epoch 不匹配返回 false 触发重拉。

- [ ] **Step 1: 写测试**(`src/lib/rowCache.test.ts`;用 `vitest` 的 `describe/it/expect`,Row 构造用最小字面量 `{ lineNo, date: "", time: "", level: "", pid: "", tid: "", tag: "", message: "m", marked: false }`)

测试用例(每条一个 `it`):
1. `fill` 后 `get(blockStart + i)` 返回对应行,`isFresh(block, rows.length, epoch)` 为 true;
2. `isFresh` 在 `want > count` 时为 false(尾块变长需重拉);
3. `isFresh` 在 epoch 不同(旧纪元填充)时为 false,但 `get` 仍返回旧行(防闪烁语义);
4. 超过 `maxBlocks` 时逐出最久未使用块(先 fill A、B、C,`get` 触碰 A,再 fill D → B 被逐出:`get(B内index)` 为 undefined,A/C/D 仍在);
5. `updateRows` 能改写驻留行的 `marked` 字段;
6. `clear` 之后 `get` 全部 undefined、`blockCount() === 0`。

- [ ] **Step 2: 运行确认失败** — Run: `pnpm test rowCache` → 模块不存在,FAIL。
- [ ] **Step 3: 实现 `RowBlockCache`,测试转绿** — Run: `pnpm test` → PASS。
- [ ] **Step 4: Commit 第一段**

```bash
git add src/lib/rowCache.ts src/lib/rowCache.test.ts
git commit -m "feat: add bounded LRU row block cache"
```

- [ ] **Step 5: LogTable 接线**

- `const cache = useRef<Map<number, Row>>(new Map()); const filledEpoch = useRef<Map<number, FilledBlock>>(new Map());` 替换为 `const cache = useRef(new RowBlockCache(64));`(64 块 × 200 行 ≈ 1.3 万行上限);删除 `FilledBlock` 接口。
- `ensureBlock`:`filledEpoch` 判断改 `if (cache.current.isFresh(block, want, cacheEpoch.current)) return;`;成功回包后 `cache.current.fill(block, rows, epoch);`(原 rows.forEach set + 越界 delete + filledEpoch.set 三段删除)。
- 行读取:`cache.current.get(vi.index)` → `cache.current.get(vi.index, WINDOW)`;`collectRowsInRange`/`collectRowsFromSelection`/`openBookmarkMenu` 同步改。
- `sessionId` effect:`cache.current.clear();`(原三行清 Map 改一行,`inflight.current.clear()` 保留)。
- `filterResultRevision` effect:只 `cacheEpoch.current += 1; inflight.current.clear();`,不 clear 缓存(保持防闪烁语义,与现状一致)。
- `toggleRowBookmark`/`applyBookmarkRange` 中 `cache.current.forEach(...)` 改:

```ts
cache.current.updateRows((cached) =>
  cached.lineNo === row.lineNo ? { ...cached, marked } : cached,
);
```

- [ ] **Step 6: 验证** — Run: `pnpm typecheck && pnpm lint && pnpm test` → 全绿。
- [ ] **Step 7: Commit**

```bash
git add src/components/LogTable.tsx
git commit -m "perf: bound LogTable row cache with LRU block eviction"
```

---

### Task 7: 导出/切分异步化 + BufWriter + 按行切分 + 会话保留策略(P1)

**背景**:`export_logs`/`split_log_file` 是同步命令(Tauri v2 同步命令在主线程执行),10GB 操作会冻结窗口;导出逐行直写裸 `File`(每行一次系统调用);字节切分会把行切成两半;流式会话文件只增不删。

**Files:**
- Modify: `crates/logcore/src/session.rs`(`create_export_file` 返回 `BufWriter<File>`;`write_source_line` 参数改 `&mut impl Write`;`export_view`/`export_range` 末尾 `writer.flush()?`)
- Modify: `crates/logcore/src/split.rs`(`split_by_bytes` 改为行对齐;新增进度回调变体)
- Modify: `src-tauri/src/commands.rs`(`export_logs`/`split_log_file` 改 async + `spawn_blocking`;`start_logcat` 调用会话清理)
- Create: `src-tauri/src/commands.rs` 内 `prune_stream_sessions` 私有函数
- Modify: `src/lib/ipc.ts`(新增 `onSplitProgress`)、`src/components/ToolDialogs.tsx`(SplitDialog 显示进度)、`src/types.ts`(新增 `SplitProgress` 类型)
- Test: `crates/logcore/src/split.rs`、`src-tauri/src/commands.rs` tests

**Interfaces:**
- Produces:
  - `pub fn split_file_with_progress(path: &Path, out_dir: &Path, mode: SplitMode, on_part: &mut dyn FnMut(usize, u64)) -> io::Result<SplitSummary>`(每关闭一个分片回调一次:`(已完成分片数, 已处理字节)`;`split_file` 变为空回调包装,签名不变)
  - `fn prune_stream_sessions(dir: &Path, keep: usize)`(只删除文件名匹配 `logcat-<纯数字>.log` 的文件及其 `.lfbookmarks.toml` sidecar,按文件名倒序保留最新 `keep` 个;任何 IO 错误静默忽略)
  - 事件 `split:progress`,payload `{ parts: number, bytesProcessed: number }`(camelCase)
- **行为变更(明示)**:字节切分从"精确字节数、可能切断行"改为"行对齐、单分片 ≤ limit,除非单行本身超过 limit(该行独占一片)"。既有测试 `byte_split_preserves_bytes_and_limits_non_final_parts` 按新语义重写。

- [ ] **Step 1: split 行对齐 TDD**(先改写/新增测试,红)

```rust
#[test]
fn byte_split_aligns_to_line_boundaries() {
    let source = source_file(b"aaaa\nbb\ncccccc\ndd\n"); // 行字节数: 5,3,7,3
    let out_dir = tempfile::tempdir().unwrap();

    let summary = split_file(source.path(), out_dir.path(), SplitMode::Bytes(8)).unwrap();

    assert_eq!(joined_parts(&summary.parts), b"aaaa\nbb\ncccccc\ndd\n");
    assert_eq!(summary.parts.len(), 3); // "aaaa\nbb\n" / "cccccc\n" / "dd\n"
    for part in &summary.parts {
        let bytes = fs::read(part).unwrap();
        assert!(bytes.ends_with(b"\n"), "part must end at line boundary");
        assert!(bytes.len() <= 8);
    }
}

#[test]
fn oversized_single_line_gets_its_own_part() {
    let source = source_file(b"abcdefghij\nx\n"); // 首行 11 字节 > limit 4
    let out_dir = tempfile::tempdir().unwrap();

    let summary = split_file(source.path(), out_dir.path(), SplitMode::Bytes(4)).unwrap();

    assert_eq!(summary.parts.len(), 2);
    assert_eq!(fs::read(&summary.parts[0]).unwrap(), b"abcdefghij\n");
    assert_eq!(fs::read(&summary.parts[1]).unwrap(), b"x\n");
}

#[test]
fn split_reports_progress_per_part() {
    let source = source_file(b"a\nb\nc\nd\n");
    let out_dir = tempfile::tempdir().unwrap();
    let mut calls = Vec::new();

    split_file_with_progress(source.path(), out_dir.path(), SplitMode::Lines(2), &mut |parts, bytes| {
        calls.push((parts, bytes));
    })
    .unwrap();

    assert_eq!(calls, vec![(1, 4), (2, 8)]);
}
```

- [ ] **Step 2: 实现 split 改造**:`split_by_bytes` 改为 `read_until(b'\n')` 逐行累积(轮换条件:`writer 已存在 && current_bytes > 0 && current_bytes + line.len() > limit`);两种模式的 writer 统一包 `BufWriter`;关闭分片时(轮换与收尾)回调 `on_part(parts.len(), bytes_processed)`。`split_file` 委托 `split_file_with_progress(.., &mut |_, _| {})`。

Run: `cargo test -p logcore split` → PASS。

- [ ] **Step 3: Commit split**

```bash
git add crates/logcore/src/split.rs
git commit -m "feat: line-aligned byte split with per-part progress callback"
```

- [ ] **Step 4: 导出 BufWriter**:`session.rs` 中 `create_export_file` 返回 `io::Result<BufWriter<File>>`(`BufWriter::new(File::create(output)?)`);`write_source_line(&self, source_idx, frontier, writer: &mut impl Write, summary)`;`export_view`/`export_range` 收尾 `writer.flush()?;`。顶部 `use std::io::{self, BufWriter, Write};`。

Run: `cargo test -p logcore export` → PASS。

- [ ] **Step 5: 命令异步化**(commands.rs)

```rust
#[tauri::command]
pub async fn export_logs(
    request: ExportRequest,
    state: State<'_, AppState>,
) -> Result<ExportSummaryDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || export_logs_blocking(&app_state, request))
        .await
        .map_err(|err| err.to_string())?
}
```

`export_logs_blocking(app_state: &AppState, request: ExportRequest) -> Result<ExportSummaryDto, String>` 即原函数体(`state.lock_session()` 改 `app_state.lock_session()`)。

```rust
#[tauri::command]
pub async fn split_log_file(
    request: SplitRequest,
    app: AppHandle,
) -> Result<SplitSummaryDto, String> {
    tauri::async_runtime::spawn_blocking(move || split_log_file_blocking(request, &app))
        .await
        .map_err(|err| err.to_string())?
}
```

`split_log_file_blocking` 为原函数体,`split_file` 调用改为:

```rust
let summary = logcore::split::split_file_with_progress(
    &PathBuf::from(request.path),
    &PathBuf::from(request.out_dir),
    mode,
    &mut |parts, bytes_processed| {
        let _ = app.emit(
            "split:progress",
            SplitProgressDto {
                parts,
                bytes_processed,
            },
        );
    },
)
.map_err(|err| err.to_string())?;
```

`dto.rs` 新增:

```rust
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SplitProgressDto {
    pub parts: usize,
    pub bytes_processed: u64,
}
```

前端异步命令无需改动调用方(`invoke` 本就返回 Promise)。

- [ ] **Step 6: 会话保留策略**(commands.rs)

```rust
/// 只识别本应用生成的 `logcat-<millis>.log`,按文件名倒序保留最新 keep 个,
/// 其余连同书签 sidecar 一起删除;所有 IO 失败静默忽略(清理是尽力而为)。
fn prune_stream_sessions(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut sessions: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("logcat-"))
                .and_then(|rest| rest.strip_suffix(".log"))
                .is_some_and(|millis| !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    sessions.sort();
    for stale in sessions.iter().rev().skip(keep) {
        let _ = fs::remove_file(stale);
        let _ = fs::remove_file(logcore::bookmarks::sidecar_path_for(stale));
    }
}
```

`start_logcat` 中 `File::create(&session_path)` 之后调用 `prune_stream_sessions(session_path.parent().unwrap_or(&session_path), 10);`。

单元测试:

```rust
#[test]
fn prunes_old_stream_sessions_keeping_newest() {
    let dir = tempfile::tempdir().unwrap();
    for millis in [1, 2, 3, 4] {
        std::fs::write(dir.path().join(format!("logcat-{millis}.log")), b"x").unwrap();
    }
    std::fs::write(dir.path().join("user-notes.log"), b"keep me").unwrap();

    prune_stream_sessions(dir.path(), 2);

    assert!(!dir.path().join("logcat-1.log").exists());
    assert!(!dir.path().join("logcat-2.log").exists());
    assert!(dir.path().join("logcat-3.log").exists());
    assert!(dir.path().join("logcat-4.log").exists());
    assert!(dir.path().join("user-notes.log").exists());
}
```

- [ ] **Step 7: 前端进度显示**

`src/types.ts`:

```ts
export interface SplitProgress {
  parts: number;
  bytesProcessed: number;
}
```

`src/lib/ipc.ts`:

```ts
export const onSplitProgress = (cb: (progress: SplitProgress) => void): Promise<UnlistenFn> =>
  listen<SplitProgress>("split:progress", (e) => cb(e.payload));
```

`ToolDialogs.tsx` 的 `SplitDialog`:新增 `const [progress, setProgress] = useState<SplitProgress | null>(null);`,`useEffect` 挂 `onSplitProgress(setProgress)`(卸载时 unlisten,模式同 App.tsx 的 `onFilterDone`);`runSplit` 开始时 `setProgress(null)`;busy 时按钮文案改为 `progress ? `切分中 · 已生成 ${progress.parts} 份` : "切分中"`。

- [ ] **Step 8: 验证** — Run: `cargo test -p logcore && cargo test -p log-filter && pnpm typecheck && pnpm lint && pnpm test` → 全绿。
- [ ] **Step 9: Commit**

```bash
git add src-tauri/src crates/logcore/src/session.rs src/lib/ipc.ts src/types.ts src/components/ToolDialogs.tsx
git commit -m "feat: async export/split commands with buffered writes, split progress and session pruning"
```

---

### Task 8: adb 调用加固(P1)

**背景**:设备轮询每 4s 同步 spawn `adb devices -l`,无超时(adb 挂起时请求堆积),Windows 上每次轮询闪控制台窗口;resume 重放整个 ring buffer 造成重复日志。

**Files:**
- Modify: `crates/logcore/src/adb.rs`(`adb_command` 辅助、`list_devices_with_timeout`、`build_logcat_command` 增加 `since` 参数、`last_log_timestamp`)
- Modify: `src-tauri/src/commands.rs`(`list_devices`/`start_logcat`/`resume_logcat` 改 async + `spawn_blocking`;spawn 用 `adb_command`;resume 读取尾部时间戳)
- Modify: `src-tauri/src/state.rs`(`StreamRequestState` 增加 `since_timestamp: Option<String>`)
- Modify: `src/components/Toolbar.tsx`(refreshDevices 防重入)
- Test: `crates/logcore/src/adb.rs` tests

**Interfaces:**
- Produces:
  - `pub fn adb_command(path: &Path) -> Command` — Windows 下带 `CREATE_NO_WINDOW (0x0800_0000)` creation flag;所有 adb 子进程(devices、logcat)必须经它创建。
  - `pub fn list_devices_with_timeout(adb_path: &Path, timeout: Duration) -> io::Result<Vec<AdbDevice>>`;`list_devices` 变为 `Duration::from_secs(5)` 的包装。
  - `pub fn build_logcat_command(adb_path: PathBuf, serial: &str, buffers: &[LogcatBuffer], since: Option<&str>) -> LogcatCommand` — `since` 存在时追加 `-T <since>`。
  - `pub fn last_log_timestamp(tail_text: &str) -> Option<String>` — 从尾部文本反向找最后一条可解析日志,返回 `"MM-DD HH:MM:SS.mmm"`;校验形状(date 5 字符含 `-`,time 12 字符 `HH:MM:SS.mmm`)。
- Consumes: Task 4 的 `parse_line_ref`。

- [ ] **Step 1: TDD `last_log_timestamp` 与 `-T` 参数**(adb.rs tests,红)

```rust
#[test]
fn extracts_last_parseable_timestamp_from_tail() {
    let tail = "garbage line\n04-20 12:06:02.125   146   179 D T: one\n04-20 12:06:03.900   146   179 I T: two\ntrailing junk";
    assert_eq!(
        last_log_timestamp(tail).as_deref(),
        Some("04-20 12:06:03.900")
    );
    assert_eq!(last_log_timestamp("no logs here\n"), None);
    assert_eq!(last_log_timestamp(""), None);
}

#[test]
fn logcat_command_appends_since_timestamp() {
    let command = build_logcat_command(
        PathBuf::from("adb"),
        "usb",
        &[LogcatBuffer::Main],
        Some("04-20 12:06:03.900"),
    );
    assert_eq!(
        command.args,
        vec!["-s", "usb", "logcat", "-v", "threadtime", "-b", "main", "-T", "04-20 12:06:03.900"]
    );
}
```

既有 `builds_threadtime_logcat_command_with_unique_buffers` 测试的调用补 `None` 参数。

- [ ] **Step 2: 实现 adb.rs**

```rust
/// 所有 adb 子进程必须经此创建:Windows 下抑制控制台窗口闪烁。
pub fn adb_command(path: &Path) -> Command {
    let command = Command::new(path);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = command;
        command.creation_flags(CREATE_NO_WINDOW);
        command
    };
    command
}

pub fn list_devices(adb_path: &Path) -> io::Result<Vec<AdbDevice>> {
    list_devices_with_timeout(adb_path, Duration::from_secs(5))
}

/// adb server 冷启动或 USB 抖动时 `adb devices` 可能长时间挂起;超时后杀掉子进程返回错误,
/// 避免上层轮询堆积。
pub fn list_devices_with_timeout(
    adb_path: &Path,
    timeout: Duration,
) -> io::Result<Vec<AdbDevice>> {
    let mut child = adb_command(adb_path)
        .arg("devices")
        .arg("-l")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(status) => {
                let mut stdout = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                if !status.success() {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    return Err(io::Error::other(stderr));
                }
                return Ok(parse_adb_devices(&stdout));
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(io::ErrorKind::TimedOut, "adb devices timed out"));
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// 从会话文件尾部文本提取最后一条可解析日志的时间戳,供 resume 时 `logcat -T` 续抓去重。
/// 注意:`-T <time>` 的设备兼容性尚未在真机全面验证;不支持的旧设备上 logcat 会立即退出,
/// 表现为流自动停止,用户可重新 Start 全量抓取。
pub fn last_log_timestamp(tail_text: &str) -> Option<String> {
    tail_text.lines().rev().find_map(|line| {
        let parsed = crate::parser::parse_line_ref(line);
        let date = parsed.date;
        let time = parsed.time;
        let date_ok = date.len() == 5
            && date.as_bytes()[2] == b'-'
            && date.bytes().enumerate().all(|(i, b)| i == 2 || b.is_ascii_digit());
        let time_ok = time.len() == 12
            && time.as_bytes()[2] == b':'
            && time.as_bytes()[5] == b':'
            && time.as_bytes()[8] == b'.'
            && time
                .bytes()
                .enumerate()
                .all(|(i, b)| matches!(i, 2 | 5 | 8) || b.is_ascii_digit());
        (date_ok && time_ok).then(|| format!("{date} {time}"))
    })
}
```

`build_logcat_command` 末尾:

```rust
if let Some(since) = since {
    args.push("-T".to_string());
    args.push(since.to_string());
}
```

顶部补 `use std::io::Read; use std::process::Stdio; use std::time::{Duration, Instant};`。

Run: `cargo test -p logcore adb` → PASS。

- [ ] **Step 3: Commit adb 引擎侧**

```bash
git add crates/logcore/src/adb.rs
git commit -m "feat: adb command hardening - no-window flag, devices timeout, resume -T support"
```

- [ ] **Step 4: commands.rs 接线**

- `StreamRequestState` 增字段 `pub since_timestamp: Option<String>`(state.rs);`start_logcat` 构造处置 `None`。
- `spawn_logcat_stream` 中 `build_logcat_command(request.adb_path.clone(), &device.serial, &buffers)` → 补 `request.since_timestamp.as_deref()`;`Command::new(&command.adb_path)` → `logcore::adb::adb_command(&command.adb_path)`。
- `resume_logcat` 在 `spawn_logcat_stream` 前:

```rust
let mut request = request;
request.since_timestamp = read_session_tail(&request.session_path, 64 * 1024)
    .as_deref()
    .and_then(logcore::adb::last_log_timestamp);
```

新增:

```rust
/// 读会话文件末尾至多 max_bytes 的内容(lossy 解码),供 resume 提取最后时间戳。
fn read_session_tail(path: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max_bytes))).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}
```

- `list_devices`/`start_logcat`/`resume_logcat` 三个命令改 async:

```rust
#[tauri::command]
pub async fn list_devices() -> Result<DeviceListDto, String> {
    tauri::async_runtime::spawn_blocking(list_devices_blocking)
        .await
        .map_err(|err| err.to_string())?
}
```

(原函数体挪进 `fn list_devices_blocking() -> Result<DeviceListDto, String>`;`start_logcat`/`resume_logcat` 同模式,闭包捕获 `state.inner().clone()` 与 `app`。)

- [ ] **Step 5: Toolbar 防重入**(Toolbar.tsx)

```tsx
const refreshInflightRef = useRef(false);
const refreshDevices = useCallback(async () => {
  if (refreshInflightRef.current) return;
  refreshInflightRef.current = true;
  try {
    const result = await listDevices();
    setDevices(result.devices);
  } catch (err) {
    console.error("list_devices failed", err);
    setDevices([]);
  } finally {
    refreshInflightRef.current = false;
  }
}, [setDevices]);
```

- [ ] **Step 6: 验证** — Run: `cargo test -p logcore && cargo test -p log-filter && pnpm typecheck && pnpm lint && pnpm test` → 全绿。
- [ ] **Step 7: Commit**

```bash
git add src-tauri/src src/components/Toolbar.tsx
git commit -m "feat: async adb commands, resume from last timestamp, device poll reentrancy guard"
```

---

### Task 9: 引擎内存与 remap 优化(P2)

**背景**:`filtered`/`search_matches`/`error_lines` 用 `Vec<u64>`,高命中率过滤在亿行级文件上是 ~800MB;stream reader 每 64KB remap 一次(mmap/munmap 系统调用);文件被外部截断时旧索引直接失效但无防护。

**Files:**
- Modify: `crates/logcore/src/session.rs`(三个 Vec 改 `Vec<u32>`;`remap_source` 增长判定与收缩重建;新增 `reset_derived_state`)
- Modify: `crates/logcore/src/search.rs`(`next_match` 签名改 `&[u32], from: u32`)
- Modify: `src-tauri/src/commands.rs`(`filter_indexed_range`/`append_*` 相关类型跟随;编译器指引)
- Test: `crates/logcore/src/session.rs` tests

**Interfaces:**
- `Session::filter_indexed_range`/`search_indexed_range` 返回 `Vec<u32>`;`apply_filter_results`/`append_filter_results`/`apply_search_results`/`append_search_results` 入参 `Vec<u32>`。对外的行号 API(`get_rows*`、`search_next`、书签、`result_index_for_line_no`)保持 `u64`/`usize` 不变,在边界处转换。
- 行号超过 `u32::MAX` 的行不进入命中数组(`debug_assert!` + release 下跳过);10GB logcat 实际行数 ~3 亿,距上限一个数量级,注释说明即可。

- [ ] **Step 1: 收缩重建 TDD**(session.rs tests,红)

```rust
#[test]
fn remap_after_truncation_rebuilds_index_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shrink.log");
    std::fs::write(
        &path,
        "04-20 12:06:02.125   146   179 D T: one\n04-20 12:06:02.225   146   179 E T: two\n",
    )
    .unwrap();
    let mut s = Session::open(&path).unwrap();
    s.index_all();
    assert_eq!(s.total_lines(), 2);
    assert_eq!(s.error_count(), 1);

    std::fs::write(&path, "04-20 12:06:03.000   146   179 D T: fresh\n").unwrap();
    s.remap_and_index_step(usize::MAX).unwrap();

    assert_eq!(s.total_lines(), 1);
    assert_eq!(s.error_count(), 0);
    let rows = s.get_rows(0, 10);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.message, "fresh");
}
```

- [ ] **Step 2: 实现 remap 判定**

```rust
/// 重新映射源文件。文件未增长时跳过(流式 reader 每个读块都会调用,mmap/munmap 不便宜);
/// 检测到收缩(外部截断/轮转)时旧索引全部失效,重建派生状态,避免越界访问乃至 SIGBUS。
pub fn remap_source(&mut self) -> io::Result<()> {
    let disk_len = fs::metadata(&self.source_path)?.len() as usize;
    if disk_len < self.source.len() {
        self.source = MmapSource::open(&self.source_path)?;
        self.reset_derived_state();
        return Ok(());
    }
    if disk_len > self.source.len() {
        self.source = MmapSource::open(&self.source_path)?;
    }
    Ok(())
}

fn reset_derived_state(&mut self) {
    self.indexer = Indexer::new();
    self.filtered.clear();
    self.filter_active = false;
    self.search_matches.clear();
    self.error_lines.clear();
    self.error_scan_lines = 0;
}
```

(收缩后 `filter_active` 置 false:过滤 spec 仍在 `filter_spec` 里,索引完成后 `rerun_scans_after_index_done` 会按 pending spec 重算——与 `open_file` 后的行为一致。)

Run: `cargo test -p logcore remap` → PASS(含既有 `remap_and_index_step_reads_lines_appended_after_trailing_newline`)。

- [ ] **Step 3: `Vec<u64>` → `Vec<u32>`**:session.rs 三个字段与相关方法签名(`filter_indexed_range`/`search_indexed_range` 返回值、apply/append 入参、`current_result_index_for_source_idx` 内 `binary_search(&(source_idx as u32))`、minimap 反查处 `binary_search(&(*idx))` 等);`search.rs::next_match(matches: &[u32], from: u32, ..) -> Option<u32>`,`Session::search_next` 内 `from` 以 `u32::try_from(zero_based).unwrap_or(u32::MAX)` 收窄;命中推入处:

```rust
if let Ok(idx32) = u32::try_from(idx) {
    matches.push(idx32);
} else {
    debug_assert!(false, "line index exceeds u32 range");
}
```

commands.rs 中 `append_filter_for_range`/`append_search_for_range`/`spawn_*_task` 的 `Vec` 类型让编译器指引改完。`error_lines` 对外的 `u64` 语义(如 `write_source_line(*source_idx as usize, ..)`)在使用处 `as usize` 不变。

- [ ] **Step 4: 全量验证** — Run: `cargo test -p logcore && cargo test -p log-filter` → 全绿。
- [ ] **Step 5: Commit**

```bash
git add crates/logcore/src src-tauri/src
git commit -m "perf: u32 hit arrays, growth-gated remap, rebuild on external truncation"
```

---

### Task 10: 简洁性清理(P2)

**背景**:review 点名的繁琐处:`stop_stream_task` 布尔参数组合、filter/search 扫描任务 ~80 行重复、`indexer::line_span` 透传 shim、大小写不敏感搜索 O(n·m) 朴素扫描、搜索进度事件过密(1 亿行 2.4 万个 IPC 事件)、config 迁移逻辑无注释。

**Files:**
- Modify: `src-tauri/src/commands.rs`、`crates/logcore/src/indexer.rs`、`crates/logcore/src/session.rs`、`crates/logcore/src/search.rs`、`crates/logcore/src/config.rs`

**子项(每项独立 commit):**

- [ ] **10a: `stop_stream_task` 语义化**

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamStop {
    Pause,       // 保留 last_request,标记 paused,可 resume
    Stop,        // 保留 last_request(clear 复用路径),不标记 paused
    Forget,      // 丢弃 last_request(切换到新会话前)
}
```

`stop_stream_task(state, mode: StreamStop)`;映射:`open_file`/`start_logcat` → `Forget`;`pause_logcat`/`resume_logcat` → `Pause`;`stop_logcat`/`clear_logcat` → `Stop`。`take_stream_task` 的两个布尔参数同步收编。
Run: `cargo test -p log-filter` → PASS。Commit: `refactor: replace stop_stream_task boolean flags with StreamStop mode`。

- [ ] **10b: 合并 filter/search 扫描任务骨架**

抽出共用分块循环(commands.rs 私有):

```rust
/// 分块扫描 [0, 快照总行数);每块持锁校验会话与任务代号,任一失效即放弃(返回 None)。
/// scan 在持锁状态下执行;on_chunk 在锁外执行(用于进度事件)。
fn run_chunked_scan(
    app_state: &AppState,
    session_generation: u64,
    is_current_task: impl Fn() -> bool,
    scan: impl Fn(&logcore::session::Session, usize, usize) -> Vec<u32>,
    mut on_chunk: impl FnMut(usize, usize),
) -> Option<Vec<u32>> {
    let total_lines = {
        let guard = app_state.lock_session_if_current(session_generation)?;
        guard.as_ref().map(|session| session.total_lines())?
    };
    let mut matches = Vec::new();
    let mut start = 0;
    while start < total_lines {
        let end = start.saturating_add(SCAN_CHUNK_LINES).min(total_lines);
        let chunk = {
            let guard = app_state.lock_session_if_current(session_generation)?;
            if !is_current_task() {
                return None;
            }
            scan(guard.as_ref()?, start, end)
        };
        matches.extend(chunk);
        on_chunk(end, matches.len());
        start = end;
        std::thread::yield_now();
    }
    Some(matches)
}
```

`spawn_filter_task`/`spawn_search_task` 改为:调用 `run_chunked_scan`(filter 的 `on_chunk` 为空;search 的 `on_chunk` 做节流进度,见 10c),`None` 即 return;收尾 apply 段保持各自逻辑(Task 2 的持锁校验模式)。**注意 `search_indexed_range` 首块 `first_line` 逻辑**:改在 `on_chunk` 外由 `matches.first()` 推导(`matches` 有序,`first_line = matches.first().map(|idx| u64::from(*idx) + 1)`),行为不变。
Run: `cargo test -p log-filter && cargo test -p logcore` → PASS。Commit: `refactor: share chunked scan loop between filter and search tasks`。

- [ ] **10c: 搜索进度事件节流**

`spawn_search_task` 的 `on_chunk` 中:仅当 `scanned - last_emitted >= 65_536`(约 16 块)或 `matches_len` 首次 > 0 时 emit `search:progress`(`done: false`),并更新 `last_emitted`;最终 `done: true` 事件保持不变。
Run: `cargo test -p log-filter` → PASS。Commit: `perf: throttle search progress events`(可与 10b 合并为一个 commit,若实现时自然融合)。

- [ ] **10d: 大小写不敏感明文搜索走 regex 字面量**

`search.rs` 的 `CompiledSearch::compile`,`!case_sensitive` 明文分支改为:

```rust
return RegexBuilder::new(&regex::escape(&spec.query))
    .case_insensitive(true)
    .build()
    .map(Self::Regex)
    .map_err(|err| SearchError {
        message: err.to_string(),
    });
```

删除 `contains_case_insensitive_ascii` 与 `Plain` 变体的 `case_sensitive` 字段(`Plain` 仅剩区分大小写路径,退化为 `Plain(String)`);既有测试 `case_sensitive_search_can_be_disabled`、`case_insensitive_plain_search_keeps_unicode_behavior` 必须保持通过,`ascii_case_insensitive_plain_search_matches_without_lowercase_copy` 测试改为经 `SearchMatcher` 公共 API 断言同样场景。
Run: `cargo test -p logcore search` → PASS。Commit: `perf: case-insensitive plain search via regex literal engine`。

- [ ] **10e: 删除 `indexer::line_span` 透传函数**

`session.rs` 两处 `line_span(&self.indexer, ...)` 改 `self.indexer.line_span(...)`;indexer.rs 顶层函数与其 import 删除;indexer.rs 测试内的 `line_span(&ix, ...)` 改 `ix.line_span(...)`。
Run: `cargo test -p logcore` → PASS。Commit: `refactor: drop line_span passthrough helper`。

- [ ] **10f: config 迁移逻辑注释**

`config.rs` `normalized()` 的 command/legacy-buffers 块(约 219-242 行)前补注释:

```rust
// current_command 选取规则(兼容早期仅有 command_buffers 的配置):
// 1) current_command 能解析且不是"默认值 + 存在 legacy buffers"的组合 → 用它;
//    (默认值 + legacy 并存,说明用户旧配置只设过 buffers,应尊重 buffers)
// 2) 否则取第一个 legacy buffer 组装命令;
// 3) 都没有 → 默认 main。
```

Run: `cargo test -p logcore config` → PASS。Commit: `docs: explain config command migration rules`(可并入 10e commit)。

---

## 范围外(明确不做,后续另立计划)

- `adb track-devices` 长连接设备监听:输出格式跨 adb 版本差异未验证,需真机调研后再做(当前以"async + 超时 + 防重入"的轮询兜底)。
- `logcat -T` 真机兼容性矩阵验证(代码已带注释与失败退化路径)。
- Toolbar.tsx 拆分子组件、dto.rs 压缩(dto 层实际隔离了 config TOML 的 snake_case 序列化格式,直接复用 logcore 类型会破坏现有配置文件,维持现状)。
- 无级别行(`--------- beginning of ...`)在级别过滤下被隐藏的语义确认(需对照原版 Java LogFilter 行为)。
- 导出期间不持锁的流式导出、导出进度事件。
- rayon 并行过滤(等 Task 4 落地后按实测数据决定)。

## 验收清单(最终整分支 review 时逐项核对)

1. `cargo test -p logcore`、`cargo test -p log-filter`、`pnpm test`、`pnpm typecheck`、`pnpm lint` 全绿。
2. 铁律不破:`get_rows` 上限 512 不变;logcore 无新增 Tauri/UI 依赖;无任何路径把整文件/整结果发给前端。
3. `clear_logcat` 路径:session 置 None 严格先于 `File::create`。
4. 所有后台任务对 session 的访问都经 `lock_session_if_current`。
5. 热路径(`filter_indexed_range`/`search_indexed_range`/`refresh_error_lines`)无按行 `String`/`Vec` 分配(decode 对合法 UTF-8 是借用)。
6. 前端 `RowBlockCache` 有上限且书签更新路径工作正常。
