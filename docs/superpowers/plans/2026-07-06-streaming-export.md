# 导出不持锁流式化 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 导出 10GB 文件时不再全程持有 session 锁——改为分段取数(锁内拷字节)、锁外写盘,并经 `export:progress` 事件回传进度,消除导出期间 `get_rows`/`get_status` 被阻塞导致的 UI 冻结。

**Architecture:** 快照式分段导出:Phase A 分块驱动索引完成 → Phase B(仅 Filtered 视图)按当前 spec 分块重算出**局部**命中数组(复用 `run_chunked_scan`,不写回 session)→ Phase C 每批 4096 行:`lock_session_if_current` 内把行字节追加进缓冲,锁外 `BufWriter` 写盘 + 节流进度。任何一次持锁发现会话代号失效即中止(Err),盘上留部分文件(与现错误路径一致)。logcore 只新增纯函数原语,编排全在 src-tauri。

**Tech Stack:** Rust(logcore + Tauri v2 命令层)、React(ExportDialog 进度显示)。

## Global Constraints

- **铁律**:`logcore` 不得引入 Tauri/UI 依赖;进度经闭包/事件在 src-tauri 侧发出。
- **输出逐字节等价**:同一会话状态下,分段导出的输出文件必须与 `Session::export_view`/`export_range` 逐字节一致(等价性测试固定)。
- **既有行为变更(明示、可接受)**:旧实现经 `prepare_file_tool` 把过滤结果**写回 session**(副作用),新实现 Filtered 视图用局部数组、range 模式不再重建过滤——输出不变,仅去掉无人消费的副作用;`Session::export_view`/`export_range`/`prepare_file_tool` 原样保留(测试与小文件路径继续使用)。
- **并发不变量**:导出线程对 session 的每次访问都必须经 `lock_session_if_current(generation)`;generation 在导出开始时捕获一次。
- **验证命令**:`cargo test -p logcore && cargo test -p log-filter && cargo clippy --workspace --all-targets && pnpm typecheck && pnpm lint && pnpm test`,全绿零警告。
- **提交规范**:conventional commits;TDD(纯函数原语与等价性测试先行)。

---

### Task 1: 分段导出

**Files:**
- Modify: `crates/logcore/src/session.rs`(新增 `ExportPlan`、`export_plan_snapshot`、`append_line_bytes`、`validate_export_target`;`create_export_file` 改为调用 `validate_export_target`)
- Modify: `src-tauri/src/commands.rs`(`export_logs` 增加 `app: AppHandle` 参数;`export_logs_blocking` 重写为 `run_chunked_export` 编排;新增 `EXPORT_CHUNK_LINES`/`EXPORT_PROGRESS_STRIDE` 常量)
- Modify: `src-tauri/src/dto.rs`(新增 `ExportProgressDto`)
- Modify: `src/types.ts`、`src/lib/ipc.ts`、`src/components/ToolDialogs.tsx`(ExportDialog 进度)
- Test: `crates/logcore/src/session.rs` tests、`src-tauri/src/commands.rs` tests

**Interfaces:**

logcore(session.rs,全部 `pub`):

```rust
/// 导出计划:一次持锁产出的**快照**。AllLines 用行号区间(不物化);
/// Indices 是 0-based 源行号数组(Filtered 克隆命中数组;Bookmarks/Errors 转换/克隆小数组)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportPlan {
    AllLines { total: usize },
    Indices(Vec<u32>),
}

impl Session {
    /// 对指定视图产出导出快照。Filtered 且过滤未激活时按 All 处理(与 export_view 一致)。
    /// 注意:Filtered 激活时返回**当前** filtered 的克隆;调用方若需要"完整重算"语义,
    /// 应先用 FilterMatcher 分块重算出局部数组,不使用本方法的 Filtered 分支。
    pub fn export_plan_snapshot(&self, view: RowsView) -> ExportPlan;

    /// 把第 source_idx 行(0-based)的原始字节(含行尾换行)追加进 out,返回追加的字节数;
    /// 行不可用(未索引/越界)返回 0。供"锁内拷贝、锁外写盘"的分段导出使用。
    pub fn append_line_bytes(&self, source_idx: usize, out: &mut Vec<u8>) -> u64;

    /// 校验导出目标合法(不得与源文件相同)并确保父目录存在。
    /// 从 create_export_file 中拆出,后者改为先调用本方法。
    pub fn validate_export_target(&self, output: &Path) -> io::Result<()>;
}
```

`export_plan_snapshot` 各分支:`All` → `AllLines { total: self.total_lines() }`;`Filtered` 且 `filter_active` → `Indices(self.filtered.clone())`,否则 → `AllLines`;`Bookmarks` → `Indices(bookmark_source_lines() 转 0-based u32,u32::try_from 失败的行跳过)`;`Errors` → `Indices(self.error_lines.clone())`。

commands.rs:

```rust
const EXPORT_CHUNK_LINES: usize = 4096;
const EXPORT_PROGRESS_STRIDE: usize = 65_536; // 与 SEARCH_PROGRESS_STRIDE 同数量级

/// 分段导出编排。进度回调 on_progress(written_lines, written_bytes, done) 在锁外调用;
/// 事件发送由 export_logs_blocking 注入闭包完成,本函数不依赖 Tauri,可直接单测。
fn run_chunked_export(
    app_state: &AppState,
    session_generation: u64,
    request: &ExportRequest,
    on_progress: &mut dyn FnMut(usize, u64, bool),
) -> Result<ExportSummaryDto, String>
```

dto.rs:

```rust
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgressDto {
    pub written_lines: usize,
    pub written_bytes: u64,
    pub done: bool,
}
```

事件名 `export:progress`。前端 `ExportProgress { writtenLines: number; writtenBytes: number; done: boolean }`。

**`run_chunked_export` 逻辑(实现基准):**

1. 校验 `request.path` 非空;`let output = PathBuf::from(&request.path)`。
2. **首次持锁**(`lock_session_if_current` 失败即 `Err("session changed during export")`,session 为 None 即 `Err("open a log file before exporting")`——下同):`session.validate_export_target(&output)`;range 模式校验 `start_line`/`end_line` 存在且 `start >= 1 && end >= start`(沿用 export_range 的错误文案 "export range must be 1-based and ascending")。
3. **Phase A(索引补完)**:循环 { 持锁 → `session.index_step(INDEX_BUDGET)` → 若返回 true(done)跳出 };每轮锁外 `std::thread::yield_now()`。
4. **Phase B(确定导出对象)**:
   - range 模式:持锁取 `total = session.total_lines()`,`start = start_line.min(total as u64 + 1)`、`end = end_line.min(total as u64)`,导出对象为行号区间(转 0-based `AllLines`-风格区间 `[start-1, end)`,空区间输出 0 行)。
   - view 模式:持锁取 `active_spec = session.active_filter_spec()` 与视图。若视图为 Filtered 且 spec 存在:`FilterMatcher::new(&spec)` 后用 `run_chunked_scan(app_state, generation, || true, |s, a, b| s.filter_indexed_range(&matcher, a, b), |_, _| {})` 重算局部命中数组(返回 None 即代号失效 → Err),导出对象 `Indices(该数组)`;否则持锁 `session.export_plan_snapshot(view)`。
5. **Phase C(分段写盘)**:锁外 `BufWriter::new(File::create(&output))`(validate 已在锁内做过;创建失败 → Err)。把导出对象统一成"index 批次流":`AllLines`/区间 → 按 `EXPORT_CHUNK_LINES` 切区间;`Indices` → `chunks(EXPORT_CHUNK_LINES)`。每批:
   ```rust
   buf.clear();
   let mut batch_lines = 0usize;
   {
       let Some(guard) = app_state.lock_session_if_current(session_generation) else {
           return Err("session changed during export".to_string());
       };
       let Some(session) = guard.as_ref() else { ... Err ... };
       for idx in batch {
           let appended = session.append_line_bytes(idx, &mut buf);
           if appended > 0 {
               batch_lines += 1;
               written_bytes += appended;
           }
       }
   }
   writer.write_all(&buf).map_err(...)?;
   written_lines += batch_lines;
   if written_lines - last_emitted >= EXPORT_PROGRESS_STRIDE {
       last_emitted = written_lines;
       on_progress(written_lines, written_bytes, false);
   }
   std::thread::yield_now();
   ```
6. `writer.flush()`;`on_progress(written_lines, written_bytes, true)`;返回 `ExportSummaryDto { written_lines, written_bytes }`。

`export_logs_blocking` 收缩为:读 `state.generation.load(SeqCst)` → 构造 emit 闭包(`app.emit("export:progress", ExportProgressDto {...})`)→ 调 `run_chunked_export`。`export_logs` 命令签名加 `app: AppHandle`,clone 进 `spawn_blocking` 闭包。

- [ ] **Step 1: logcore 原语 TDD(红)** — session.rs tests 新增:

```rust
#[test]
fn append_line_bytes_copies_raw_line_and_reports_length() {
    let f = temp_filter_log();
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    let mut out = Vec::new();
    let n = s.append_line_bytes(1, &mut out);
    assert_eq!(
        out,
        b"04-20 12:06:02.225   200   220 I Network: GET /home ok\n"
    );
    assert_eq!(n, out.len() as u64);
    assert_eq!(s.append_line_bytes(99, &mut out), 0); // 越界行不追加
    assert_eq!(out.len(), n as usize);
}

#[test]
fn export_plan_snapshot_matches_view_semantics() {
    let f = temp_filter_log();
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    s.toggle_bookmark(2).unwrap();

    assert_eq!(
        s.export_plan_snapshot(RowsView::All),
        ExportPlan::AllLines { total: 4 }
    );
    // 过滤未激活时 Filtered 退化为 All
    assert_eq!(
        s.export_plan_snapshot(RowsView::Filtered),
        ExportPlan::AllLines { total: 4 }
    );
    s.set_filter(&FilterSpec {
        tag_include: FilterField::plain(true, "Network"),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        s.export_plan_snapshot(RowsView::Filtered),
        ExportPlan::Indices(vec![1, 2])
    );
    assert_eq!(
        s.export_plan_snapshot(RowsView::Bookmarks),
        ExportPlan::Indices(vec![1])
    );
    assert_eq!(
        s.export_plan_snapshot(RowsView::Errors),
        ExportPlan::Indices(vec![3])
    );
}

#[test]
fn validate_export_target_rejects_source_file() {
    let f = temp_filter_log();
    let mut s = Session::open(f.path()).unwrap();
    s.index_all();
    assert!(s.validate_export_target(f.path()).is_err());
    let dir = tempfile::tempdir().unwrap();
    assert!(s.validate_export_target(&dir.path().join("sub/out.log")).is_ok());
    assert!(dir.path().join("sub").exists()); // 父目录已创建
}
```

- [ ] **Step 2: 实现三个原语,logcore 全绿**(`create_export_file` 改为 `self.validate_export_target(output)?; Ok(BufWriter::new(File::create(output)?))`;既有 export 测试是行为回归)
- [ ] **Step 3: Commit** — `feat: add export snapshot primitives to session`
- [ ] **Step 4: commands.rs 编排 TDD(红)** — commands.rs tests 新增(不需要 AppHandle,`on_progress` 传闭包):

```rust
fn export_state_with_session(path: &std::path::Path) -> (AppState, u64) {
    let mut session = logcore::session::Session::open(path).unwrap();
    session.index_all();
    let state = AppState::new();
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    *state.lock_session() = Some(session);
    (state, generation)
}

#[test]
fn chunked_export_matches_export_view_output_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.log");
    let mut content = String::new();
    for i in 0..9000 {
        let level = if i % 7 == 0 { "E" } else { "I" };
        content.push_str(&format!(
            "04-20 12:06:02.{:03}   200   220 {level} Net: msg {i}\n",
            i % 1000
        ));
    }
    std::fs::write(&src, &content).unwrap();

    // 期望输出:独立会话跑旧路径 export_view(Filtered)
    let expected_path = dir.path().join("expected.log");
    let spec = logcore::filter::FilterSpec {
        levels: logcore::filter::LevelMask::from_levels(&["E", "F"]),
        ..Default::default()
    };
    {
        let mut oracle = logcore::session::Session::open(&src).unwrap();
        oracle.index_all();
        oracle.set_filter(&spec).unwrap();
        oracle
            .export_view(logcore::session::RowsView::Filtered, &expected_path)
            .unwrap();
    }

    // 实际输出:分段导出(9000 行 > EXPORT_CHUNK_LINES,保证跨批)
    let (state, generation) = export_state_with_session(&src);
    {
        let mut guard = state.lock_session();
        guard.as_mut().unwrap().set_filter(&spec).unwrap();
    }
    let out_path = dir.path().join("chunked.log");
    let request = ExportRequest {
        mode: "view".to_string(),
        view: Some("filtered".to_string()),
        start_line: None,
        end_line: None,
        path: out_path.to_string_lossy().to_string(),
    };
    let mut progress_calls = Vec::new();
    let summary = run_chunked_export(&state, generation, &request, &mut |lines, bytes, done| {
        progress_calls.push((lines, bytes, done));
    })
    .unwrap();

    let expected = std::fs::read(&expected_path).unwrap();
    let actual = std::fs::read(&out_path).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(summary.written_bytes as usize, actual.len());
    assert_eq!(progress_calls.last().map(|c| c.2), Some(true)); // 最终 done 事件
}

#[test]
fn chunked_export_range_matches_export_range_output_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.log");
    std::fs::write(
        &src,
        "04-20 12:06:02.125   146   179 D A: one\n04-20 12:06:02.225   200   220 I B: two\n04-20 12:06:02.325   200   221 W C: three\n",
    )
    .unwrap();
    let expected_path = dir.path().join("expected.log");
    {
        let mut oracle = logcore::session::Session::open(&src).unwrap();
        oracle.index_all();
        oracle.export_range(2, 3, &expected_path).unwrap();
    }
    let (state, generation) = export_state_with_session(&src);
    let out_path = dir.path().join("chunked.log");
    let request = ExportRequest {
        mode: "range".to_string(),
        view: None,
        start_line: Some(2),
        end_line: Some(3),
        path: out_path.to_string_lossy().to_string(),
    };
    let summary =
        run_chunked_export(&state, generation, &request, &mut |_, _, _| {}).unwrap();
    assert_eq!(
        std::fs::read(&out_path).unwrap(),
        std::fs::read(&expected_path).unwrap()
    );
    assert_eq!(summary.written_lines, 2);
}

#[test]
fn chunked_export_aborts_when_session_generation_changes() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.log");
    std::fs::write(&src, "04-20 12:06:02.125   146   179 D A: one\n").unwrap();
    let (state, generation) = export_state_with_session(&src);
    state.generation.fetch_add(1, Ordering::SeqCst); // 模拟导出期间 open 了新文件
    let request = ExportRequest {
        mode: "view".to_string(),
        view: Some("all".to_string()),
        start_line: None,
        end_line: None,
        path: dir.path().join("out.log").to_string_lossy().to_string(),
    };
    let err = run_chunked_export(&state, generation, &request, &mut |_, _, _| {}).unwrap_err();
    assert!(err.contains("session changed"), "{err}");
}
```

(`ExportRequest` 字段名以 dto.rs 实际定义为准,测试构造时对齐。)

- [ ] **Step 5: 实现 `run_chunked_export` + `export_logs_blocking` 改造 + `ExportProgressDto`,`cargo test -p log-filter` 全绿**
- [ ] **Step 6: Commit** — `perf: chunked export without holding the session lock across the run`
- [ ] **Step 7: 前端进度** — `src/types.ts` 加 `ExportProgress`;`src/lib/ipc.ts` 加 `onExportProgress`(镜像 `onSplitProgress`);`ToolDialogs.tsx` 的 `ExportDialog`:`useState<ExportProgress | null>` + 监听 effect(unlisten 清理,模式同 SplitDialog)+ `runExport` 开始时置 null + busy 按钮文案 `progress ? `导出中 · 已写入 ${progress.writtenLines.toLocaleString()} 行` : "导出中"`。
- [ ] **Step 8: 全量验证** — Global Constraints 的完整命令集,零警告。
- [ ] **Step 9: Commit** — `feat: show export progress in export dialog`

## 语义说明(评审对照)

- **快照语义**:流式抓取过程中导出,Phase B 之后追加的新行不会进入本次导出(旧实现同样如此——单次持锁快照)。
- **不再写回 session**:旧 `prepare_file_tool` 会把重算的过滤结果 apply 回 session、range 模式也顺带重建过滤;新实现输出等价但无此副作用,UI 状态不受导出影响(该副作用从未发事件、无人消费)。
- **中止行为**:导出中途 `open_file`/`start_logcat`/`clear_logcat` 换会话 → 下一次持锁即失败,返回 "session changed during export",磁盘留部分输出文件(与旧实现 IO 错误路径一致,前端以错误 toast 呈现)。
- **进度节流**:每 65_536 行一次 + 最终 done,10GB/1 亿行 ≈ 1500 个事件,可接受。

## 范围外

- 导出期间的取消按钮(UI 侧中止);Bookmarks/Errors 视图导出进度粒度(小数组,一批完成)。
