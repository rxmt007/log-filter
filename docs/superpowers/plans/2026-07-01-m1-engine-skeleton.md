# M1 引擎骨架:大文件打开 + 索引 + 虚拟表格浏览 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 做出一个能打开(超大)日志文件、后台建行索引、用虚拟表格只读浏览的 Tauri 桌面应用,端到端验证"mmap + 索引 + 只传可见窗口"架构。

**Architecture:** Rust `logcore` crate 负责 mmap、增量行偏移索引、按需解析;Tauri v2 后端把 `logcore` 暴露为 `open_file / get_status / get_rows` 命令,并在后台线程建索引、发 `index:progress` 事件;React 前端用 TanStack Virtual 只渲染可见行、按窗口向后端取行。**前端永不整体接收文件**。

**Tech Stack:** Rust(memmap2、memchr)、Tauri v2、Vite + React + TypeScript + Tailwind + shadcn/ui(Base UI)、TanStack Virtual、zustand。

**里程碑上下文:** 本计划是规范 §19 的 **M1**。不含过滤 / 高亮 / 搜索 / adb / 书签 / 导出 / 切分(那些属于 M2–M6,各自成计划)。M1 只做:打开文件 → 索引 → 解析 → 虚拟表格浏览。

---

## File Structure(本计划涉及的文件与职责)

```
Cargo.toml                       # 新增:workspace 根,members = ["src-tauri","crates/logcore"]
crates/logcore/
  Cargo.toml                     # 新增:logcore 依赖(memmap2/memchr)
  src/lib.rs                     # 新增:导出各模块
  src/model.rs                   # LogEntry 数据结构;LogLevel 枚举(供后续着色/过滤复用)
  src/parser.rs                  # 单行解析:threadtime / time / 回退;纯函数
  src/mmap_source.rs             # 文件 mmap;按字节区间取切片
  src/indexer.rs                 # 增量行偏移索引(memchr 扫 '\n');line_span
  src/session.rs                 # 组装 source+indexer;get_rows / total_lines / 索引步进
src-tauri/
  Cargo.toml                     # 修改:依赖 logcore、serde、tauri-plugin-dialog
  src/lib.rs (或 main.rs)        # 修改:注册命令、manage 状态、装 dialog 插件
  src/state.rs                   # 新增:AppState { session: Arc<Mutex<Option<Session>>> }
  src/dto.rs                     # 新增:Row / Status(serde, camelCase)
  src/commands.rs                # 新增:open_file / get_status / get_rows 命令 + 后台索引线程
  capabilities/default.json      # 修改:加 dialog 权限
src/                             # 前端
  types.ts                       # 新增:Row / Status TS 类型
  lib/ipc.ts                     # 新增:invoke/listen 类型化封装
  store/session.ts               # 新增:zustand 保存 Status
  components/LogTable.tsx        # 新增:TanStack Virtual 虚拟表格(窗口取行 + 缓存)
  components/StatusBar.tsx       # 新增:底部状态栏(行数 / 索引进度)
  components/Toolbar.tsx         # 新增:打开按钮
  App.tsx                        # 修改:组装 + 监听 index:progress
  index.css / main.tsx           # 修改:Tailwind 指令
scripts/gen_biglog.py            # 新增:生成合成大日志用于验证
```

**边界原则:** `logcore` 不引用任何 Tauri/UI 类型,可用 `cargo test -p logcore` 独立测试。Tauri 层只做薄封装 + 线程 + 事件。前端只经 `lib/ipc.ts` 与后端通信。

---

## Task 1: 脚手架(Tauri v2 + React/TS/Vite + Tailwind + shadcn + Cargo workspace + logcore)

**Files:**
- Create: `crates/logcore/{Cargo.toml,src/lib.rs}`, root `Cargo.toml`
- Modify: 由脚手架生成的 `package.json` / `src-tauri/Cargo.toml` / `src-tauri/tauri.conf.json`

- [ ] **Step 1: 生成 Tauri v2 + react-ts 脚手架到临时子目录再并入根目录**

当前根目录已有 `docs/`、`AGENTS.md`、`.gitignore`、`LogFilter/`,`create-tauri-app` 拒绝非空目录,故先生成到子目录再并入(排除其自带 `.gitignore`,保留我们已写好的):

```bash
cd /Users/alice/work_space_qa/log-filter
pnpm create tauri-app@latest _scaffold --template react-ts --manager pnpm --yes
rsync -a --exclude '.gitignore' _scaffold/ ./
rm -rf _scaffold
pnpm install
```

预期:根目录出现 `package.json`、`index.html`、`vite.config.ts`、`src/`、`src-tauri/`。

- [ ] **Step 2: 建立 Cargo workspace 并新增 logcore crate**

创建根 `Cargo.toml`:

```toml
[workspace]
members = ["src-tauri", "crates/logcore"]
resolver = "2"
```

创建 `crates/logcore/Cargo.toml`:

```toml
[package]
name = "logcore"
version = "0.1.0"
edition = "2021"

[dependencies]
memmap2 = "0.9"
memchr = "2"

[dev-dependencies]
tempfile = "3"
```

创建占位 `crates/logcore/src/lib.rs`:

```rust
pub mod model;
pub mod parser;
pub mod mmap_source;
pub mod indexer;
pub mod session;
```

（后续 Task 会创建这些模块文件;本步骤先建目录与空模块以让 workspace 成立。先建空文件:）

```bash
mkdir -p crates/logcore/src
: > crates/logcore/src/model.rs
: > crates/logcore/src/parser.rs
: > crates/logcore/src/mmap_source.rs
: > crates/logcore/src/indexer.rs
: > crates/logcore/src/session.rs
```

- [ ] **Step 3: 安装 Tailwind 与 shadcn(Base UI)**

```bash
pnpm add -D tailwindcss@3 postcss autoprefixer
pnpm dlx tailwindcss init -p
pnpm add @tanstack/react-virtual zustand
pnpm dlx shadcn@latest init
pnpm dlx shadcn@latest add button
```

`tailwind.config.js` 的 `content` 设为:

```js
content: ["./index.html", "./src/**/*.{ts,tsx}"],
```

在 `src/index.css` 顶部加(若 shadcn 未加):

```css
@tailwind base;
@tailwind components;
@tailwind utilities;
```

> 说明:`shadcn init` 若提示选择风格/基础库,选择 Base UI 变体(如可用);M1 仅用到 `Button`,不受影响。

- [ ] **Step 4: 验证脚手架可跑**

```bash
cargo build --workspace
pnpm tauri dev
```

预期:`cargo build` 通过(含空 logcore crate);`pnpm tauri dev` 弹出默认窗口。确认后关闭窗口。

- [ ] **Step 5: 提交**

```bash
git add -A
git commit -m "chore: scaffold tauri v2 + react/ts + tailwind + logcore workspace"
```

---

## Task 2: logcore `model.rs` — LogEntry 与 LogLevel

**Files:**
- Modify: `crates/logcore/src/model.rs`

- [ ] **Step 1: 写失败测试**

`crates/logcore/src/model.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_from_char_maps_known() {
        assert_eq!(LogLevel::from_char('E'), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_char('V'), Some(LogLevel::Verbose));
        assert_eq!(LogLevel::from_char('X'), None);
    }

    #[test]
    fn log_entry_default_is_empty() {
        let e = LogEntry::default();
        assert_eq!(e.message, "");
        assert_eq!(e.level, "");
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p logcore model::`
预期:FAIL(`LogLevel` / `LogEntry` 未定义)。

- [ ] **Step 3: 实现**

在 `crates/logcore/src/model.rs` 顶部加:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn from_char(c: char) -> Option<LogLevel> {
        match c {
            'V' => Some(LogLevel::Verbose),
            'D' => Some(LogLevel::Debug),
            'I' => Some(LogLevel::Info),
            'W' => Some(LogLevel::Warn),
            'E' => Some(LogLevel::Error),
            'F' => Some(LogLevel::Fatal),
            _ => None,
        }
    }
}

/// 一条日志的解析结果。行号由 session 赋值,不在此结构里。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogEntry {
    pub date: String,
    pub time: String,
    pub level: String,
    pub pid: String,
    pub tid: String,
    pub tag: String,
    pub message: String,
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test -p logcore model::`
预期:PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/logcore/src/model.rs
git commit -m "feat(logcore): add LogEntry and LogLevel model"
```

---

## Task 3: logcore `parser.rs` — threadtime 格式

**Files:**
- Modify: `crates/logcore/src/parser.rs`

- [ ] **Step 1: 写失败测试**

`crates/logcore/src/parser.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_threadtime_line() {
        let line = "04-20 12:06:02.125   146   179 D BatteryService: update start";
        let e = parse_threadtime(line).expect("should parse");
        assert_eq!(e.date, "04-20");
        assert_eq!(e.time, "12:06:02.125");
        assert_eq!(e.pid, "146");
        assert_eq!(e.tid, "179");
        assert_eq!(e.level, "D");
        assert_eq!(e.tag, "BatteryService");
        assert_eq!(e.message, "update start");
    }

    #[test]
    fn rejects_non_threadtime() {
        let line = "04-17 09:01:18.910 D/LightsService(  139): BKL : 106";
        assert!(parse_threadtime(line).is_none());
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p logcore parser::`
预期:FAIL(`parse_threadtime` 未定义)。

- [ ] **Step 3: 实现**

在 `crates/logcore/src/parser.rs` 顶部加:

```rust
use crate::model::LogEntry;

/// 返回跳过前 n 个空白分隔 token 后的剩余子串(保留其内部原始间隔)。
fn rest_after_tokens(line: &str, n: usize) -> Option<&str> {
    let mut rest = line.trim_start();
    for _ in 0..n {
        let ws = rest.find(char::is_whitespace)?;
        rest = rest[ws..].trim_start();
    }
    if rest.is_empty() { None } else { Some(rest) }
}

fn is_all_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `MM-DD HH:MM:SS.mmm  PID  TID L Tag: message`
pub fn parse_threadtime(line: &str) -> Option<LogEntry> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.len() < 6 {
        return None;
    }
    let (date, time, pid, tid, level) = (toks[0], toks[1], toks[2], toks[3], toks[4]);
    if !is_all_ascii_digits(pid) || !is_all_ascii_digits(tid) {
        return None;
    }
    if level.len() != 1 || !"VDIWEF".contains(level) {
        return None;
    }
    // tag+message 部分,保留原始间隔:跳过前 5 个 token
    let tail = rest_after_tokens(line, 5)?;
    let colon = tail.find(':')?;
    let tag = tail[..colon].to_string();
    let message = tail[colon + 1..].trim_start().to_string();
    Some(LogEntry {
        date: date.to_string(),
        time: time.to_string(),
        level: level.to_string(),
        pid: pid.to_string(),
        tid: tid.to_string(),
        tag,
        message,
    })
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test -p logcore parser::`
预期:PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/logcore/src/parser.rs
git commit -m "feat(logcore): parse logcat threadtime format"
```

---

## Task 4: logcore `parser.rs` — time 格式 + 自动识别入口

**Files:**
- Modify: `crates/logcore/src/parser.rs`

- [ ] **Step 1: 写失败测试**

在 `parser.rs` 的 `mod tests` 里追加:

```rust
    #[test]
    fn parses_time_line() {
        let line = "04-17 09:01:18.910 D/LightsService(  139): BKL : 106";
        let e = parse_time(line).expect("should parse");
        assert_eq!(e.date, "04-17");
        assert_eq!(e.time, "09:01:18.910");
        assert_eq!(e.level, "D");
        assert_eq!(e.tag, "LightsService");
        assert_eq!(e.pid, "139");
        assert_eq!(e.tid, "");
        assert_eq!(e.message, "BKL : 106");
    }

    #[test]
    fn parse_line_dispatches_and_falls_back() {
        let tt = parse_line("04-20 12:06:02.125   146   179 D BatteryService: update start");
        assert_eq!(tt.tag, "BatteryService");
        let tm = parse_line("04-17 09:01:18.910 D/LightsService(  139): BKL : 106");
        assert_eq!(tm.tag, "LightsService");
        let raw = parse_line("--------- beginning of main");
        assert_eq!(raw.message, "--------- beginning of main");
        assert_eq!(raw.tag, "");
    }
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p logcore parser::`
预期:FAIL(`parse_time` / `parse_line` 未定义)。

- [ ] **Step 3: 实现**

在 `parser.rs` 追加:

```rust
/// `MM-DD HH:MM:SS.mmm L/Tag(  pid): message`
pub fn parse_time(line: &str) -> Option<LogEntry> {
    let mut it = line.split_whitespace();
    let date = it.next()?;
    let time = it.next()?;
    let rest = rest_after_tokens(line, 2)?; // "D/LightsService(  139): BKL : 106"
    if rest.len() < 2 {
        return None;
    }
    let level = &rest[..1];
    if !"VDIWEF".contains(level) || &rest[1..2] != "/" {
        return None;
    }
    let after = &rest[2..]; // "LightsService(  139): BKL : 106"
    let open = after.find('(')?;
    let close = after.find(')')?;
    if close < open {
        return None;
    }
    let tag = after[..open].to_string();
    let pid = after[open + 1..close].trim().to_string();
    let message = after[close + 1..]
        .trim_start_matches(':')
        .trim_start()
        .to_string();
    Some(LogEntry {
        date: date.to_string(),
        time: time.to_string(),
        level: level.to_string(),
        pid,
        tid: String::new(),
        tag,
        message,
    })
}

/// 依次尝试 threadtime → time,失败则整行作为 message。
pub fn parse_line(line: &str) -> LogEntry {
    let line = line.trim_end_matches(['\r', '\n']);
    parse_threadtime(line)
        .or_else(|| parse_time(line))
        .unwrap_or_else(|| LogEntry {
            message: line.to_string(),
            ..Default::default()
        })
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test -p logcore parser::`
预期:PASS(全部 4 个 parser 测试)。

- [ ] **Step 5: 提交**

```bash
git add crates/logcore/src/parser.rs
git commit -m "feat(logcore): parse time format and add auto-detect parse_line"
```

---

## Task 5: logcore `mmap_source.rs` — 文件内存映射

**Files:**
- Modify: `crates/logcore/src/mmap_source.rs`

- [ ] **Step 1: 写失败测试**

`crates/logcore/src/mmap_source.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn maps_and_slices() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello\nworld").unwrap();
        let src = MmapSource::open(f.path()).unwrap();
        assert_eq!(src.len(), 11);
        assert_eq!(&src.bytes()[0..5], b"hello");
    }

    #[test]
    fn empty_file_is_zero_len() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let src = MmapSource::open(f.path()).unwrap();
        assert_eq!(src.len(), 0);
        assert_eq!(src.bytes(), b"");
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p logcore mmap_source::`
预期:FAIL(`MmapSource` 未定义)。

- [ ] **Step 3: 实现**

在 `mmap_source.rs` 顶部加:

```rust
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

/// 只读内存映射的文件源。空文件时 `mmap` 为 None(memmap2 无法映射 0 长度文件)。
pub struct MmapSource {
    mmap: Option<Mmap>,
}

impl MmapSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            return Ok(Self { mmap: None });
        }
        // Safety: 文件在应用生命周期内不被外部截断;只读访问。
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(Self { mmap: Some(mmap) })
    }

    pub fn len(&self) -> usize {
        self.mmap.as_ref().map_or(0, |m| m.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> &[u8] {
        self.mmap.as_ref().map_or(&[][..], |m| &m[..])
    }
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test -p logcore mmap_source::`
预期:PASS。

- [ ] **Step 5: 提交**

```bash
git add crates/logcore/src/mmap_source.rs
git commit -m "feat(logcore): add read-only MmapSource"
```

---

## Task 6: logcore `indexer.rs` — 增量行偏移索引

**Files:**
- Modify: `crates/logcore/src/indexer.rs`

- [ ] **Step 1: 写失败测试**

`crates/logcore/src/indexer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_line_starts() {
        let bytes = b"a\nbb\nccc";
        let mut ix = Indexer::new();
        ix.step(bytes, bytes.len());
        assert!(ix.is_done(bytes.len()));
        assert_eq!(ix.offsets(), &[0, 2, 5]);
        assert_eq!(ix.total_lines(), 3);
        assert_eq!(line_span(ix.offsets(), 1, bytes.len()), Some((2, 5)));
        assert_eq!(line_span(ix.offsets(), 2, bytes.len()), Some((5, 8)));
    }

    #[test]
    fn trailing_newline_makes_no_empty_line() {
        let bytes = b"a\nbb\n";
        let mut ix = Indexer::new();
        ix.step(bytes, bytes.len());
        assert_eq!(ix.offsets(), &[0, 2]);
        assert_eq!(ix.total_lines(), 2);
    }

    #[test]
    fn chunked_stepping_matches_single_step() {
        let bytes = b"line1\nline2\nline3\nline4";
        let mut a = Indexer::new();
        a.step(bytes, bytes.len());
        let mut b = Indexer::new();
        // 逐 3 字节步进,跨越 '\n' 边界
        while !b.is_done(bytes.len()) {
            b.step(bytes, 3);
        }
        assert_eq!(a.offsets(), b.offsets());
    }

    #[test]
    fn empty_is_zero_lines() {
        let mut ix = Indexer::new();
        ix.step(b"", 0);
        assert_eq!(ix.total_lines(), 0);
        assert!(ix.is_done(0));
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p logcore indexer::`
预期:FAIL(`Indexer` / `line_span` 未定义)。

- [ ] **Step 3: 实现**

在 `indexer.rs` 顶部加:

```rust
use memchr::memchr_iter;

/// 增量构建行起始偏移。`offsets[i]` = 第 i 行首字节偏移;行数 = offsets.len()。
pub struct Indexer {
    offsets: Vec<u64>,
    cursor: usize,
}

impl Indexer {
    pub fn new() -> Self {
        Self {
            offsets: Vec::new(),
            cursor: 0,
        }
    }

    /// 从内部 cursor 起,最多处理 `budget` 字节。返回本次处理的字节数。
    pub fn step(&mut self, bytes: &[u8], budget: usize) -> usize {
        if self.offsets.is_empty() && !bytes.is_empty() {
            self.offsets.push(0);
        }
        let end = self.cursor.saturating_add(budget).min(bytes.len());
        let chunk = &bytes[self.cursor..end];
        for pos in memchr_iter(b'\n', chunk) {
            let abs_next = (self.cursor + pos + 1) as u64;
            if (abs_next as usize) < bytes.len() {
                self.offsets.push(abs_next);
            }
        }
        let processed = end - self.cursor;
        self.cursor = end;
        processed
    }

    pub fn is_done(&self, total: usize) -> bool {
        self.cursor >= total
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn total_lines(&self) -> usize {
        self.offsets.len()
    }

    pub fn offsets(&self) -> &[u64] {
        &self.offsets
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}

/// 第 i 行的字节区间 [start, end)(end 含末尾换行,取文本时再裁剪)。
pub fn line_span(offsets: &[u64], i: usize, total: usize) -> Option<(usize, usize)> {
    if i >= offsets.len() {
        return None;
    }
    let start = offsets[i] as usize;
    let end = if i + 1 < offsets.len() {
        offsets[i + 1] as usize
    } else {
        total
    };
    Some((start, end))
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test -p logcore indexer::`
预期:PASS(4 个测试)。

- [ ] **Step 5: 提交**

```bash
git add crates/logcore/src/indexer.rs
git commit -m "feat(logcore): incremental line-offset indexer"
```

---

## Task 7: logcore `session.rs` — 组装并按窗口取行

**Files:**
- Modify: `crates/logcore/src/session.rs`

- [ ] **Step 1: 写失败测试**

`crates/logcore/src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_log() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "04-20 12:06:02.125   146   179 D BatteryService: update start").unwrap();
        writeln!(f, "04-17 09:01:18.910 D/LightsService(  139): BKL : 106").unwrap();
        writeln!(f, "--------- beginning of main").unwrap();
        f
    }

    #[test]
    fn opens_indexes_and_reads_rows() {
        let f = temp_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        assert_eq!(s.total_lines(), 3);

        let rows = s.get_rows(0, 100);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, 1); // 行号 1-indexed
        assert_eq!(rows[0].1.tag, "BatteryService");
        assert_eq!(rows[1].1.tag, "LightsService");
        assert_eq!(rows[2].1.message, "--------- beginning of main");
    }

    #[test]
    fn get_rows_clamps_range() {
        let f = temp_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let rows = s.get_rows(2, 100);
        assert_eq!(rows.len(), 1); // 仅第 3 行
        assert_eq!(rows[0].0, 3);
    }
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p logcore session::`
预期:FAIL(`Session` 未定义)。

- [ ] **Step 3: 实现**

在 `session.rs` 顶部加:

```rust
use crate::indexer::{line_span, Indexer};
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line;
use std::path::Path;

pub struct Session {
    source: MmapSource,
    indexer: Indexer,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Session> {
        let source = MmapSource::open(path)?;
        Ok(Session {
            source,
            indexer: Indexer::new(),
        })
    }

    pub fn total_bytes(&self) -> usize {
        self.source.len()
    }

    pub fn indexed_bytes(&self) -> usize {
        self.indexer.cursor()
    }

    pub fn total_lines(&self) -> usize {
        self.indexer.total_lines()
    }

    pub fn is_indexing_done(&self) -> bool {
        self.indexer.is_done(self.source.len())
    }

    /// 后台按预算步进索引;返回是否已完成。
    pub fn index_step(&mut self, budget: usize) -> bool {
        self.indexer.step(self.source.bytes(), budget);
        self.is_indexing_done()
    }

    /// 测试/小文件:一次性建完索引。
    pub fn index_all(&mut self) {
        let total = self.source.len();
        self.indexer.step(self.source.bytes(), total);
    }

    /// 取 [start, start+count) 行(按已建索引裁剪),返回 (行号1-indexed, 解析结果)。
    pub fn get_rows(&self, start: usize, count: usize) -> Vec<(u64, LogEntry)> {
        let total = self.source.len();
        let offsets = self.indexer.offsets();
        let end = start.saturating_add(count).min(offsets.len());
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for i in start..end {
            let (s, e) = line_span(offsets, i, total).expect("i in range");
            let text = String::from_utf8_lossy(&self.source.bytes()[s..e]);
            out.push((i as u64 + 1, parse_line(&text)));
        }
        out
    }
}
```

- [ ] **Step 4: 运行,确认通过**

Run: `cargo test -p logcore`
预期:PASS(全部 logcore 测试)。

- [ ] **Step 5: 提交**

```bash
git add crates/logcore/src/session.rs
git commit -m "feat(logcore): Session ties mmap+index+parser with windowed get_rows"
```

---

## Task 8: src-tauri — 状态、DTO 与命令(open_file / get_status / get_rows)

**Files:**
- Create: `src-tauri/src/state.rs`, `src-tauri/src/dto.rs`, `src-tauri/src/commands.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json`

- [ ] **Step 1: 加依赖**

`src-tauri/Cargo.toml` 的 `[dependencies]` 加:

```toml
logcore = { path = "../crates/logcore" }
serde = { version = "1", features = ["derive"] }
tauri-plugin-dialog = "2"
```

- [ ] **Step 2: DTO(serde camelCase)**

`src-tauri/src/dto.rs`:

```rust
use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub line_no: u64,
    pub date: String,
    pub time: String,
    pub level: String,
    pub pid: String,
    pub tid: String,
    pub tag: String,
    pub message: String,
    pub marked: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub total_lines: usize,
    pub indexed_bytes: u64,
    pub total_bytes: u64,
    pub indexing: bool,
}
```

- [ ] **Step 3: AppState**

`src-tauri/src/state.rs`:

```rust
use logcore::session::Session;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: 命令 + 后台索引线程**

`src-tauri/src/commands.rs`:

```rust
use crate::dto::{Row, Status};
use crate::state::AppState;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 每步 8MB
const MAX_ROWS: usize = 512;

fn status_from(session: &logcore::session::Session) -> Status {
    Status {
        total_lines: session.total_lines(),
        indexed_bytes: session.indexed_bytes() as u64,
        total_bytes: session.total_bytes() as u64,
        indexing: !session.is_indexing_done(),
    }
}

#[tauri::command]
pub fn open_file(path: String, state: State<AppState>, app: AppHandle) -> Result<Status, String> {
    let session = logcore::session::Session::open(&PathBuf::from(&path)).map_err(|e| e.to_string())?;
    let status = status_from(&session);
    *state.session.lock().unwrap() = Some(session);

    // 后台索引:小预算步进,步间释放锁,保证浏览不被阻塞。
    let session_arc = state.session.clone();
    std::thread::spawn(move || loop {
        let snapshot = {
            let mut guard = session_arc.lock().unwrap();
            match guard.as_mut() {
                Some(s) => {
                    let done = s.index_step(INDEX_BUDGET);
                    Some((status_from(s), done))
                }
                None => None, // 会话被替换/清空,退出
            }
        };
        match snapshot {
            Some((st, done)) => {
                let _ = app.emit("index:progress", st);
                if done {
                    break;
                }
            }
            None => break,
        }
    });

    Ok(status)
}

#[tauri::command]
pub fn get_status(state: State<AppState>) -> Status {
    let guard = state.session.lock().unwrap();
    match guard.as_ref() {
        Some(s) => status_from(s),
        None => Status {
            total_lines: 0,
            indexed_bytes: 0,
            total_bytes: 0,
            indexing: false,
        },
    }
}

#[tauri::command]
pub fn get_rows(view: String, start: usize, count: usize, state: State<AppState>) -> Vec<Row> {
    debug_assert_eq!(view, "all", "M1 只支持 all 视图;filtered 属于 M2");
    let count = count.min(MAX_ROWS);
    let guard = state.session.lock().unwrap();
    match guard.as_ref() {
        Some(s) => s
            .get_rows(start, count)
            .into_iter()
            .map(|(line_no, e)| Row {
                line_no,
                date: e.date,
                time: e.time,
                level: e.level,
                pid: e.pid,
                tid: e.tid,
                tag: e.tag,
                message: e.message,
                marked: false,
            })
            .collect(),
        None => Vec::new(),
    }
}
```

- [ ] **Step 5: 注册插件、状态、命令**

修改 `src-tauri/src/lib.rs`(Tauri v2 模板的入口 `run()`),加入模块声明与注册:

```rust
mod commands;
mod dto;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::get_status,
            commands::get_rows
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

> 若模板保留了默认的 `greet` 命令,可从 handler 中移除。

- [ ] **Step 6: 加 dialog 权限**

`src-tauri/capabilities/default.json` 的 `permissions` 数组加入:

```json
"dialog:default"
```

- [ ] **Step 7: 编译验证**

Run: `cargo build --workspace`
预期:PASS(无未使用/未定义符号错误)。

- [ ] **Step 8: 提交**

```bash
git add src-tauri crates
git commit -m "feat(tauri): open_file/get_status/get_rows commands + background indexing"
```

---

## Task 9: 前端 — 类型、IPC 封装与 zustand store

**Files:**
- Create: `src/types.ts`, `src/lib/ipc.ts`, `src/store/session.ts`

- [ ] **Step 1: TS 类型**

`src/types.ts`:

```ts
export interface Row {
  lineNo: number;
  date: string;
  time: string;
  level: string;
  pid: string;
  tid: string;
  tag: string;
  message: string;
  marked: boolean;
}

export interface Status {
  totalLines: number;
  indexedBytes: number;
  totalBytes: number;
  indexing: boolean;
}
```

- [ ] **Step 2: IPC 封装**

`src/lib/ipc.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Row, Status } from "../types";

export const openFile = (path: string) => invoke<Status>("open_file", { path });
export const getStatus = () => invoke<Status>("get_status");
export const getRows = (view: "all" | "filtered", start: number, count: number) =>
  invoke<Row[]>("get_rows", { view, start, count });

export const onIndexProgress = (cb: (s: Status) => void): Promise<UnlistenFn> =>
  listen<Status>("index:progress", (e) => cb(e.payload));
```

- [ ] **Step 3: zustand store**

`src/store/session.ts`:

```ts
import { create } from "zustand";
import type { Status } from "../types";

interface SessionState {
  status: Status;
  setStatus: (s: Status) => void;
}

const EMPTY: Status = { totalLines: 0, indexedBytes: 0, totalBytes: 0, indexing: false };

export const useSession = create<SessionState>((set) => ({
  status: EMPTY,
  setStatus: (status) => set({ status }),
}));
```

- [ ] **Step 4: 类型检查**

Run: `pnpm tsc --noEmit`
预期:PASS(无类型错误)。

- [ ] **Step 5: 提交**

```bash
git add src/types.ts src/lib/ipc.ts src/store/session.ts
git commit -m "feat(ui): typed IPC layer and session store"
```

---

## Task 10: 前端 — 虚拟表格 LogTable(窗口取行 + 缓存)

**Files:**
- Create: `src/components/LogTable.tsx`

- [ ] **Step 1: 实现虚拟表格**

`src/components/LogTable.tsx`:

```tsx
import { useCallback, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { getRows } from "../lib/ipc";
import type { Row } from "../types";
import { useSession } from "../store/session";

const WINDOW = 200; // 每次向后端取的行数
const ROW_H = 20;
const COLS = "64px 60px 96px 22px 56px 56px 150px 1fr";

export function LogTable() {
  const total = useSession((s) => s.status.totalLines);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef<Map<number, Row>>(new Map());
  const loaded = useRef<Set<number>>(new Set()); // 已请求的 block 起点
  const [, force] = useState(0);

  const rv = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 20,
  });

  const items = rv.getVirtualItems();

  const ensureBlock = useCallback(async (block: number) => {
    if (loaded.current.has(block)) return;
    loaded.current.add(block);
    try {
      const rows = await getRows("all", block, WINDOW);
      rows.forEach((r, i) => cache.current.set(block + i, r));
      force((x) => x + 1);
    } catch {
      loaded.current.delete(block); // 失败允许重试
    }
  }, []);

  useEffect(() => {
    if (items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    ensureBlock(Math.floor(first / WINDOW) * WINDOW);
    ensureBlock(Math.floor(last / WINDOW) * WINDOW);
  }, [items, ensureBlock, total]);

  return (
    <div ref={parentRef} className="h-full overflow-auto text-xs">
      <div style={{ height: rv.getTotalSize(), position: "relative" }}>
        {items.map((vi) => {
          const row = cache.current.get(vi.index);
          return (
            <div
              key={vi.key}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: ROW_H,
                transform: `translateY(${vi.start}px)`,
                display: "grid",
                gridTemplateColumns: COLS,
                fontFamily: "monospace",
                whiteSpace: "nowrap",
                alignItems: "center",
              }}
            >
              {row ? (
                <>
                  <span className="px-1 text-neutral-500">{row.lineNo}</span>
                  <span className="px-1 text-neutral-500">{row.date}</span>
                  <span className="px-1 text-neutral-500">{row.time}</span>
                  <span className="px-1">{row.level}</span>
                  <span className="px-1 text-neutral-500">{row.pid}</span>
                  <span className="px-1 text-neutral-500">{row.tid}</span>
                  <span className="px-1">{row.tag}</span>
                  <span className="px-1 overflow-hidden text-ellipsis">{row.message}</span>
                </>
              ) : (
                <span className="px-1 text-neutral-400" style={{ gridColumn: "1 / -1" }}>
                  …
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: 类型检查**

Run: `pnpm tsc --noEmit`
预期:PASS。

- [ ] **Step 3: 提交**

```bash
git add src/components/LogTable.tsx
git commit -m "feat(ui): virtualized LogTable with windowed row fetching"
```

---

## Task 11: 前端 — 工具栏、状态栏与 App 组装

**Files:**
- Create: `src/components/Toolbar.tsx`, `src/components/StatusBar.tsx`
- Modify: `src/App.tsx`

- [ ] **Step 1: 工具栏(打开按钮)**

`src/components/Toolbar.tsx`:

```tsx
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "./ui/button";
import { openFile } from "../lib/ipc";
import { useSession } from "../store/session";

export function Toolbar() {
  const setStatus = useSession((s) => s.setStatus);
  const onOpen = async () => {
    const path = await open({ multiple: false, directory: false });
    if (typeof path === "string") {
      const st = await openFile(path);
      setStatus(st);
    }
  };
  return (
    <div className="flex items-center gap-2 border-b p-2">
      <Button size="sm" onClick={onOpen}>
        打开
      </Button>
    </div>
  );
}
```

- [ ] **Step 2: 状态栏**

`src/components/StatusBar.tsx`:

```tsx
import { useSession } from "../store/session";

export function StatusBar() {
  const status = useSession((s) => s.status);
  const pct = status.totalBytes
    ? Math.round((status.indexedBytes / status.totalBytes) * 100)
    : 100;
  return (
    <div className="flex items-center gap-3 border-t px-3 py-1 text-xs text-neutral-500">
      <span>已加载 {status.totalLines.toLocaleString()} 行</span>
      <span>索引 {pct}%{status.indexing ? "(进行中)" : ""}</span>
    </div>
  );
}
```

- [ ] **Step 3: App 组装 + 监听 index:progress**

`src/App.tsx` 全量替换为:

```tsx
import { useEffect } from "react";
import { Toolbar } from "./components/Toolbar";
import { StatusBar } from "./components/StatusBar";
import { LogTable } from "./components/LogTable";
import { onIndexProgress } from "./lib/ipc";
import { useSession } from "./store/session";

export default function App() {
  const setStatus = useSession((s) => s.setStatus);
  useEffect(() => {
    const un = onIndexProgress(setStatus);
    return () => {
      un.then((f) => f());
    };
  }, [setStatus]);

  return (
    <div className="flex h-screen flex-col">
      <Toolbar />
      <div className="min-h-0 flex-1">
        <LogTable />
      </div>
      <StatusBar />
    </div>
  );
}
```

> 若模板的 `src/App.css` 带有会干扰布局的默认样式,删除其 import。确保 `src/main.tsx` 引入了含 Tailwind 指令的 `index.css`。

- [ ] **Step 4: 类型检查 + 构建**

Run: `pnpm tsc --noEmit && cargo build --workspace`
预期:PASS。

- [ ] **Step 5: 提交**

```bash
git add src/components/Toolbar.tsx src/components/StatusBar.tsx src/App.tsx
git commit -m "feat(ui): toolbar, status bar, app wiring with index progress"
```

---

## Task 12: 端到端验证(合成大日志)

**Files:**
- Create: `scripts/gen_biglog.py`

- [ ] **Step 1: 大日志生成脚本**

`scripts/gen_biglog.py`:

```python
import sys

# 用法: python scripts/gen_biglog.py <输出路径> <目标MB>
path = sys.argv[1]
target_bytes = int(sys.argv[2]) * 1024 * 1024

line = "04-20 12:06:{:02d}.{:03d}   146   179 D BatteryService: update start seq={}\n"
written = 0
i = 0
with open(path, "w", encoding="utf-8") as f:
    while written < target_bytes:
        s = line.format(i % 60, i % 1000, i)
        f.write(s)
        written += len(s)
        i += 1
print(f"wrote {written} bytes, {i} lines -> {path}")
```

- [ ] **Step 2: 生成 ~500MB 测试文件(先小后大)**

```bash
python3 scripts/gen_biglog.py /tmp/biglog_500m.log 500
```

预期:打印写入字节数与行数(约 8–900 万行)。

- [ ] **Step 3: 运行应用并验证**

Run: `pnpm tauri dev`

手动验证清单(逐条确认):
- 点"打开",选择 `/tmp/biglog_500m.log`;窗口**立即可交互**(不冻结)。
- 状态栏"索引 %"从低到 100% 递增,"已加载 N 行"随之增长。
- 表格能顺畅滚动到任意位置;快速拖动滚动条时可见短暂"…"占位随即被真实行替换。
- 打开系统活动监视器/任务管理器:进程内存**远小于文件大小**(不应接近 500MB 的整文件驻留;主要是索引与映射页)。

- [ ] **Step 4:(可选)更大文件压测**

```bash
python3 scripts/gen_biglog.py /tmp/biglog_5g.log 5000
```

用应用打开 5GB 文件,重复 Step 3 的观察;确认滚动与内存表现依旧良好。若不便造 10GB,5GB 足以验证架构。

- [ ] **Step 5: 提交**

```bash
git add scripts/gen_biglog.py
git commit -m "test: add big-log generator and E2E verification script"
```

---

## 自检(coverage / placeholders / type consistency)

**M1 范围覆盖:** 打开文件(Task 8/11)、mmap(Task 5)、增量索引 + 后台线程 + 进度事件(Task 6/8)、解析 time+threadtime+回退(Task 3/4)、按窗口取行且 count 有上限(Task 7/8)、虚拟表格只渲染可见行(Task 10)、状态栏进度(Task 11)、大文件端到端验证(Task 12)。均有对应任务。**不在 M1 的**(过滤/高亮/搜索/adb/书签/导出/切分)已在开头声明,留待 M2–M6。

**类型一致性:** Rust `Row`/`Status` 用 `#[serde(rename_all="camelCase")]`,字段 `lineNo/totalLines/indexedBytes/totalBytes/indexing` 与 `src/types.ts` 完全一致;命令名 `open_file/get_status/get_rows`、参数 `path`/`view,start,count`、事件名 `index:progress` 前后端一致;`get_rows` 的 `MAX_ROWS`(后端 512)与前端 `WINDOW=200 ≤ 512` 相容。

**无占位符:** 每个代码步骤均给出可编译/可运行的完整代码与命令。索引步长 `INDEX_BUDGET=8MB`、`WINDOW=200`、`MAX_ROWS=512` 为 M1 初值,后续按实测调整(规范 §20 已授权)。
