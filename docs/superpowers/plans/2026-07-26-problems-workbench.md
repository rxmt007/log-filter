# 故障调查工作台实施计划

> **执行方式:** 按任务依赖逐项实施。每个纯函数/状态机先写失败测试再实现;每个重要任务完成后由未参与实现的人独立评审,不得未经评审直接合并。

**目标:** 在不破坏 10GB+、mmap、窗口读取和 logcore/UI 解耦不变量的前提下,新增 AOSP-first 确定性故障识别、紧凑事件索引、分页查询和默认折叠的底部 Problems 工作台。

**设计依据:** `docs/superpowers/specs/2026-07-26-logfilter-problems-workbench-design.md`

**状态说明（2026-07-28）:** 核心引擎、有界 IPC、TableScope、底部 Problems 工作台、
上下文与原文导出已经实现。Java、Java OOM、ANR、native、lifecycle、LMK 和 kernel OOM
均有可逐字段审阅的 positive/high-similarity-negative golden；另有 raw continuation、
逐行 prefix、双 PID/tag 交错和 growing 任意字节分段 append 与 static 逐字段等价测试。
本轮补齐 libc signal-only、kernel OOM 多行 opener/range、snapshot 幂等释放/冻结分页、
旧 token 拒绝、过滤 revision 等待、context/follow-latest 和无障碍焦点/live-region
缺口。10GiB production 与 standalone 在未人为清理 page cache 的三轮中位数达到数值
门槛，完整数据见
[`2026-07-28-problems-mvp-closure.md`](../2026-07-28-problems-mvp-closure.md)；规范
要求的可控冷/暖缓存各三次、进程级内存对照和其他平台真机性能仍是未完成验收项，不能
从当前 macOS 数字外推，也不能据此声称完整性能硬验收。
下方 checkbox 保留原始任务拆解和当时的逐项记录；没有可追溯 RED/GREEN 证据的旧条目不
做事后补勾。最终 MVP 验收判断以本状态说明、收口报告和当前测试名为准。

**总体架构:** `crates/logcore/src/problems/` 是深模块,隐藏候选分类、多行状态机、证据关联、fingerprint、group 更新和内存上限。`Session` 只负责按稳定完整行顺序推进和提供有界查询。Tauri 只负责 generation 校验、调度、DTO 和事件。前端以唯一 `TableScope` 驱动主表,Problems 只保存分页元数据,原始日志继续通过 `get_rows(..., count≤512)` 窗口读取。

## Global Constraints

- **Gate 0:** 稳定完整行前沿未完成并通过所有跨块测试前,不得接入任何 Problems recognizer。
- **原文不复制:** occurrence/group 不保存日志正文、完整堆栈或无界 String。
- **有界 IPC:** 不允许返回全量 group/occurrence;日志正文只走既有 `get_rows`。
- **事实优先:** 后端模型不得包含 `rootCause`、`diagnosis` 或自动因果结论。
- **AOSP-first:** MVP 只实现 fixture-backed AOSP 规则;不增加任意 OEM/应用正则 DSL。
- **无大模型:** MVP 检测与提示均不调用大模型。
- **确定性:** 同一最终字节输入无论如何切块,最终 occurrence/group 必须逐字段一致。
- **来源诚实:** EventLog-shaped 文本不等于已认证 events buffer;每项事实保存自己的 provenance,unknown 不得升级为 known。
- **并发:** 每次 Session 访问先校验 generation;progress emit 必须发生在锁外。
- **窗口铁律:** `get_rows` 后端硬上限仍为 512,前端默认窗口仍为 200。
- **隐私:** fixtures 全部合成或脱敏,使用 `com.example.*`、示例路径和虚构标识。
- **TDD:** parser、规则、边界、指纹、关联、分页等纯逻辑必须红—绿—重构。
- **验证全集:** 每个逻辑批次改动前后运行:

  ```sh
  cargo test -p logcore
  cargo test -p log-filter
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
  pnpm typecheck
  pnpm lint
  pnpm test
  ```

## 任务依赖

```text
Task 1 稳定完整行前沿/输入封口 ──────┐
                                      ├─→ Task 3 引擎骨架与 fixture harness
Task 2 事实/来源/身份/指纹/索引 ─────┘             │
                                                   ├─→ Task 4 Java/OOM
                                                   ├─→ Task 5 ANR
                                                   ├─→ Task 6 Native
                                                   ├─→ Task 7 Lifecycle
                                                   └─→ Task 8 LMK/Kernel OOM
                                                               │
                                                               ↓
                                               Task 9 混合仲裁与 Session 增量接入
                                                               ↓
                                               Task 10 Tauri 调度与有界 IPC
                                                     ┌─────────┴─────────┐
                                                     ↓                   ↓
                                           Task 11 TableScope    Task 12 Problems Dock
                                                     └─────────┬─────────┘
                                                               ↓
                                           Task 13 定位/上下文/导出/可访问性
                                                               ↓
                                           Task 14 10GiB 基准与独立终审
```

Task 1 与 Task 2 可并行。Task 4–8 在 Task 3 后可由不同实现者并行且各自修改独立 recognizer 文件,但 Task 9 必须统一跑混合 corpus 并进行交叉评审。

---

## Task 1: 稳定完整行前沿与输入封口(Gate 0)

**目的:** 明确区分“已发现行首”和“已看到换行的完整行”,修复增长文件尾行跨读块时可能被派生扫描提前消费的问题。

**Files:**

- Modify: `crates/logcore/src/indexer.rs`
- Modify: `crates/logcore/src/session.rs`
- Modify: `src-tauri/src/commands.rs`
- Test: 上述文件现有 tests

**Interfaces:**

```rust
impl Indexer {
    pub fn completed_lines(&self) -> usize;
}

impl Session {
    pub fn stable_lines(&self) -> usize;
    pub fn pause_growing_input(&mut self) -> Result<(), InputLifecycleError>;
    pub fn resume_paused_input(&mut self) -> Result<(), InputLifecycleError>;
    pub fn seal_growing_input(&mut self) -> Result<(), InputLifecycleError>;
}
```

Session 创建时区分 static/growing 输入:

- `open*` 为 static。
- 新增 growing 构造入口供 adb session 使用。
- static 只有索引真正到 EOF 后才把最后无换行行纳入 stable lines。
- growing 只暴露 `completed_lines`。
- 生命周期固定为 `Growing → Paused → Growing` 或 `Growing/Paused → Sealed`。
- Pause 不 finish;只有 Paused 可 resume。
- Stop 且 stdout EOF 后 seal/finish;Sealed 禁止 resume,重新采集必须创建新 session/generation。

**TDD steps:**

- [x] 在 `indexer.rs` 增加失败测试:
  - 预算停在第一行中间时 `total_lines=1`、`completed_lines=0`。
  - 预算正好停在换行后,只增加已闭合行数,不把新行首算完整。
  - 同一行后续补齐换行后 `completed_lines` 恰好增加一次。
  - CRLF 被字节预算拆开时不漏不重。
- [x] 实现 Indexer 的 `completed_lines` 跟踪,保持既有 checkpoint/line span 语义。
- [x] 在 `session.rs` 增加 static/growing、stable frontier 与 pause/resume/seal 状态机测试。
- [x] 修改 `refresh_error_lines` 只推进到 stable frontier。
- [x] 修改实时 append 的 filter/search 增量范围,从 previous stable 到 current stable。
- [x] Session/status DTO 暴露 `stableLines`;Problems context 的 All rowCount 只能使用该字段。
- [ ] 增加 filter/search/errors 对“同一尾行跨两个 stdout 块补齐”的回归测试。
- [ ] 增加 truncate/rotate 后所有 stable/派生游标归零测试。
- [x] 修改 `resume_logcat_blocking` 只接受 `runtime.paused == true`;Stop 后 resume 返回确定错误,Start 创建新 session。
- [ ] 增加“半行 → Stop/seal → Resume 被拒绝”和“半行 → Pause → Resume → 补齐”回归测试。
- [ ] 运行验证全集。
- [ ] 独立评审:重点检查 trailing newline、最后无换行行、Pause/Stop/Resume 和 Windows remap。
- [ ] Commit: `fix: track stable complete lines for incremental scans`

**Gate acceptance:**

- 同一输入在每个可能字节位置切块,stable line 序列与一次性 oracle 相同。
- Growing 尾行未换行时 filter/search/errors 不匹配;补齐后恰好匹配一次。
- Static 最后一行无换行只提交一次。
- Sealed 输入不可继续追加/恢复;Pause 不封口。
- trailing newline 不产生空行。

---

## Task 2: 事实模型、来源归属、进程身份、稳定指纹与紧凑索引

**目的:** 在 recognizer 前冻结事实可审计性、来源可信度、PID reuse 身份、分页稳定性与内存上限。

**Files:**

- Modify: `crates/logcore/Cargo.toml`(固定 BLAKE3 依赖版本)
- Modify: `crates/logcore/src/lib.rs`
- Add: `crates/logcore/src/problems/mod.rs`
- Add: `crates/logcore/src/problems/model.rs`
- Add: `crates/logcore/src/problems/facts.rs`
- Add: `crates/logcore/src/problems/provenance.rs`
- Add: `crates/logcore/src/problems/process_instance.rs`
- Add: `crates/logcore/src/problems/fingerprint.rs`
- Add: `crates/logcore/src/problems/index.rs`

**Model:**

```rust
pub enum ProblemKind {
    JavaCrash,
    JavaOom,
    Anr,
    NativeCrash,
    ProcessRestart,
    SignalExit,
    LmkKill,
    KernelOomKill,
}

pub struct ProblemEvent {
    // 0-based internal u32 line indices
    start_line: u32,
    end_line: u32,
    anchor_line: u32,
    pid: u32,
    process_instance: u32,
    group_id: u32,
    observation_start: u32,
    anchor_timestamp: PackedLogTimestamp,
    kind: ProblemKind,
    observation_len: u8,
    observation_total: u16,
    evidence: EvidenceFlags,
    outcome: OutcomeFlags,
    boundary: BoundaryFlags,
}

#[repr(C)]
pub struct ObservationRef {
    line: u32,
    rule: u16,
    role_and_format: u8,
    source_and_provenance: u8,
}
```

具体布局由实现决定,但外部 interface 和内存门槛固定:

- `ProblemEvent` 不含 String/Vec/borrow。
- `size_of::<ProblemEvent>() ≤ 48 bytes`。
- `size_of::<ObservationRef>() == 8 bytes`,每个 occurrence 最多引用 8 项。
- adopted observation 按 `(line,rule,role)` 去重且每 event 最多 4,096 项;
  `observation_total` checked increment,禁止回绕。
- public DTO 使用 u64 1-based 行号。
- `u32` 溢出进入明确 limited 状态。

**Provenance/identity contract:**

- `InputCoverage` 保存 static/adb-live、requested buffer 位集和 range completeness。
- `SourceSpanIndex` 只保存输入适配器能证明的连续 source spans;普通 multi-buffer threadtime 行默认 Unknown/Inferred,不是 Known。
- `ObservedLine` 必须携带 `InputCoverage + LineProvenance`。
- requested buffers 排除 events 时,伪造 EventLog tag 不能成为提交级证据;Unknown
  kernel-shaped 文本永不升级。
- standalone EventLog-shaped 只有 Known(events) 可独立建 Problem;Inferred(events)
  只能佐证非 EventLog `minimumCommitGrammar`。
- `ProcessInstanceTracker` 维护 pid/process/user/uid/start epoch,处理 PID reuse;active map 65,536、recent terminated 4,096 的确定上限。
- `ProcessFingerprintKey` 是独立类型,MVP 只含 normalized process name;禁止传入 pid/epoch/user/uid。

**Fingerprint contract:**

- BLAKE3 domain-separated 前 128 bit。
- domain 包含 kind 和 fingerprint version。
- canonical token 逐项写入,不构造整段字符串。
- 固定 golden hash。
- 暴露正交的 `SignatureQuality` 与 `IdentityQuality`,两者进入 hash domain/group key,不使用概率。

**ProblemIndex limits:**

- stored occurrences ≤1,000,000。
- observation refs ≤4,000,000。
- groups ≤100,000。
- query snapshots ≤8、TTL 5 分钟、ID vectors 合计 ≤16MiB。
- intern strings ≤8MiB。
- 统一 `ProblemMemoryBudget` 对受控 container capacity/保守 overhead 收费,逻辑 payload 预算 112MiB;受控 retained heap 目标 ≤128MiB。
- 达到限制设置 `limited` 与 dropped counters,停止无界分配。
- `observed/stored/droppedOccurrenceCount` 分开;代表 occurrence 必须已存储。
- 新 group 无法创建时只计 `ungroupedDroppedOccurrenceCount`,不猜 distinct dropped groups。
- event/refs/group membership/intern/index 容量先原子预留,任一失败则 occurrence 整体 dropped。
- 每组 occurrence id 单调 append;append 时不全量 sort/dedup。
- 第一次查询 materialize 稳定 `querySnapshotId`;revision 增长不改变旧 snapshot。
- occurrence snapshot 只冻结 `{groupId,frozenLen/maxEventId}`,不复制最多 100 万 event IDs。
- group snapshot 以 4,096 records/step 短锁捕获、锁外排序、analysis-token 校验后安装;TTL/LRU/release 使用显式 `&mut self`。

**TDD steps:**

- [ ] 写 model/ObservationRef size、无原文字段、8 refs 截断优先级、
  `observation_total: u16`/4,096 上限和 line overflow 测试。
- [x] 冻结 `(RuleId,ObservationRole) → FactCode` total mapping;写穷尽测试,不存在的组合必须报 detector bug。
- [ ] 写 SourceCoverage/SourceSpanIndex 测试:live requested buffers、static unknown、可靠 span、main-only 伪造 tag、unknown kernel text、Inferred EventLog standalone 不提交/与独立 text grammar 配对后只作佐证。
- [x] 写 ProcessInstanceTracker 测试:日志中段 provisional epoch、PID reuse、UID/user 缺失、name-only 禁止关联、active/recent 表确定性淘汰。
- [x] 写通用 fingerprint builder golden 测试:domain/version/两种 quality/`ProcessFingerprintKey`;类别 exception/frame/signal 归一化随 Tasks 4–8 测。
- [x] 写 ProblemIndex append/group/count/first/last/representative 测试;observed line/time 与 stored event id 分开,first/last/最近排序按 source line order,墙钟回拨不重排。
- [x] 写 query snapshot 分页测试:第一页后 revision 增长,第二页无重复/遗漏/重排;TTL、8 个/16MiB 淘汰与 session reset。
- [ ] 在 100k groups/1M occurrences 下测 snapshot 首建/分页/release;任何 Session 锁段 ≤20ms,并发 `get_rows` p99 仍达标。
- [ ] 写 full-stack/type-only、known/unknown-process 不跨质量合并测试。
- [ ] 写 100 万 occurrence 元数据预算测试。
- [x] 写高 distinct fingerprint 事件风暴触发 `limited=true` 的测试。
- [x] 写 observed/stored/dropped 与 representative 语义测试。
- [ ] 写 event 容量足够但 refs/group/intern 容量不足的原子 drop 测试;不得半提交。
- [x] 实现模型、fingerprint 和索引。
- [ ] 运行验证全集。
- [ ] 独立评审:重点检查隐式分配、每次 append 的复杂度和 overflow。
- [ ] Commit: `feat: add compact problem facts and fingerprint index`

---

## Task 3: ProblemEngine 骨架、RuleContract 与 golden fixture harness

**目的:** 在具体规则前验证深模块 interface、跨块状态机和统一规则契约。

**Files:**

- Add: `crates/logcore/src/problems/engine.rs`
- Add: `crates/logcore/src/problems/eventlog.rs`
- Add: `crates/logcore/src/problems/classifier.rs`
- Add: `crates/logcore/src/problems/recognizers/mod.rs`
- Add: `crates/logcore/tests/problems_golden.rs`
- Add: `crates/logcore/tests/fixtures/problems/**`

**External seam:**

```rust
impl ProblemEngine {
    pub fn observe(&mut self, line: ObservedLine<'_>) -> ProblemDelta;
    pub fn finish_input(&mut self) -> ProblemDelta;
    pub fn reset(&mut self);
    pub fn stats(&self) -> ProblemStats;
    pub fn begin_group_snapshot(&mut self, query: &GroupQuery) -> SnapshotBuild;
    pub fn snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> SnapshotPage;
    pub fn release_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool;
    pub fn event(&self, id: ProblemEventId) -> Option<ProblemEvent>;
    pub fn detail(&self, id: ProblemEventId) -> Option<ProblemDetail>;
}
```

Recognizer 是模块内部 seam。规则固定声明:

```text
ruleId/kind/schemaVariants/candidateStart/minimumCommitGrammar/
optionalEvidence/supportingEvidence/forbiddenEvidence/
correlationKey/boundary/fingerprint/
priority/outcomeContribution/coverageRequirement
```

**EventLog parser requirements:**

- 支持无 user、前置 user 和现代尾字段变体。
- 从固定数字前缀与固定尾字段双向解析,reason/message 允许逗号。
- 只有唯一 schema 成功且 5.7 来源门槛允许时才产生可参与提交/关联的
  EventLog-shaped observation;解析成功不声称 buffer 已认证。
- malformed/ambiguous 不猜字段。
- 所有 recognizer 共用 Task 2 的 `ProcessInstanceTracker`,不能各自用 pid/name 临时拼身份。

**TDD steps:**

- [ ] 先用 fake recognizer 写跨 1/4096 行扫描块的状态机测试。
- [ ] 写 growing 暂时 EOF 不 finish、static finish、reset、maxLines/maxBytes/unmatchedBudget 测试。
- [ ] 对每个 RuleContract 验证 candidateStart 只开候选,未满足 minimumCommitGrammar 时不得因 EOF/limit 升级;optional evidence 缺失仍可按契约提交。
- [ ] 写两个 PID/tag 交错但状态不串线测试。
- [x] 写 EventLog E01–E05 schema/golden 测试。
- [ ] 写 raw continuation 与每行带 logcat prefix 的统一输入测试;raw 行恰好匹配一个 pending 才附着,两个兼容候选时忽略并计 ambiguity。
- [ ] 写单行 1MiB、active pending 64、pending state 8MiB 和 deterministic eviction 事件风暴测试。
- [ ] 验证超限时只有已满足 `minimumCommitGrammar` 的 candidate 才 truncated
  commit,其他 candidate 丢弃为 malformed。
- [ ] 实现廉价 classifier;证明非候选行不执行 regex/完整 decode。
- [ ] 实现 RuleContract 骨架和 fixture runner。
- [ ] 运行验证全集。
- [ ] 独立评审:重点检查 interface 深度、internal seam 是否泄漏和 EventLog 歧义处理。
- [ ] Commit: `feat: add deterministic problem engine and golden harness`

---

## Task 4: Java/Kotlin 未处理异常与 Java OOM

**Files:**

- Add: `crates/logcore/src/problems/recognizers/java.rs`
- Add: `crates/logcore/src/problems/recognizers/java_oom.rs`
- Add: `crates/logcore/tests/fixtures/problems/java/**`
- Add: `crates/logcore/tests/fixtures/problems/memory/java/**`

**Scope:**

- schema-validated `am_crash` managed exception,保留 inferred/known provenance。
- AndroidRuntime 普通 `FATAL EXCEPTION` 与 `*** FATAL EXCEPTION IN SYSTEM PROCESS:` 独立 envelope。
- Process/PID、Throwable、Caused by/Suppressed/frames。
- 未处理 OOME 主归类为 `JavaOom`,不再双报 JavaCrash。
- runtime `Throwing OutOfMemoryError` 只作为 supporting observation;MVP 不生成默认可见 standalone Problem。
- 普通过程 payload PID 必须等于 AndroidRuntime header PID;system envelope 不强制不存在的 Process/PID 行。
- `minimumCommitGrammar` 固定为
  normal=`FATAL + matching Process/PID + Throwable`,
  system=`exact system envelope + Throwable`;frames 均为 optional。
- 指纹使用 `ProcessFingerprintKey`;质量 `FullStack/TypeFile/TypeOnly` 与 IdentityQuality 分开。

**TDD matrix:**

- [ ] J01–J11:完整 causal chain、system process、am_crash-only、双锚点合并、无 death、PID 冲突、main-only 伪造 tag、截断、Kotlin frames、OOME 单归类、同组/拆组。
- [ ] O01–O05:runtime throw 合并、standalone 不升级、Dalvik 变体、普通 mention、GC/trim 负例。
- [ ] 每个正例在每一行截断,不 panic、不吞后续事件。
- [ ] normal/system 分别在 `minimumCommitGrammar` 前一行截断为零事件、满足阈值后
  截断为正事件;缺 stack 仍按低 SignatureQuality 提交。
- [ ] message/PID/源码行变化不改变预期 fingerprint。
- [ ] 完整 custom-handler FATAL 仍产生 occurrence,但 outcome 为 unknown;“仅 FATAL 文本”才是负例。
- [ ] Java FATAL↔am_crash 只在同 instance、±512 行、同 timestamp segment 可比较时 ≤60s 且最近唯一时合并。
- [x] 实现并运行定向测试。
- [ ] 运行验证全集。
- [ ] 独立评审:检查 `FATAL EXCEPTION != 已死亡`、OOME 不等于泄漏。
- [ ] Commit: `feat: detect aosp managed crashes and fatal java oom`

---

## Task 5: ANR recognizer

**Files:**

- Add: `crates/logcore/src/problems/recognizers/anr.rs`
- Add: `crates/logcore/tests/fixtures/problems/anr/**`

**Scope:**

- schema-validated `am_anr`,保留 source provenance。
- tag 精确为 ActivityManager 的 `ANR in` block,同 producer block 必须有 victim `PID:`;Reason 可选,header PID 不得当 victim。
- InputDispatcher/WindowManager 只作 supporting evidence。
- Reason category 仅使用 fixture-backed AOSP 前缀。

**TDD matrix:**

- [ ] A01–A09:EventLog variants、rich block、双锚点合并、InputDispatcher-only、ANR-WatchDog、PID 冲突、截断、双 PID 交错、reason normalization。
- [ ] 验证 `Reason` 只作为 recorded trigger,不进入原因字段。
- [ ] 验证 block endLine 只到最后匹配 evidence。
- [ ] ANR block↔am_anr 只在同 victim instance、±512 行、同 timestamp segment 可比较时 ≤60s 且最近唯一时合并。
- [x] 实现并运行定向测试。
- [ ] 运行验证全集。
- [ ] 独立评审。
- [ ] Commit: `feat: detect aosp anr events`

---

## Task 6: Native crash recognizer

**Files:**

- Add: `crates/logcore/src/problems/recognizers/native.rs`
- Add: `crates/logcore/tests/fixtures/problems/native/**`

**Scope:**

- schema-validated Native `am_crash` 与 recoverable 字段。
- libc fatal signal allowlist 固定为 SIGABRT/SIGBUS/SIGFPE/SIGILL/SIGSEGV/SIGSTKFLT/SIGSYS/SIGTRAP。
- tombstone 的 `minimumCommitGrammar` 为 separator + pid/tid +
  `>>> process <<<`/`Cmdline:` victim process + fatal signal;thread `name:`
  不能替代 process,crashed-thread/backtrace 只作支持证据。
- requested debuggerd dump/Signal Catcher 负例。
- symbolized frame 用 `module#symbol` 去 offset;unsymbolized frame 用 `BuildId+module+relativePC` 去绝对地址;abort category MVP 只作 facet。

**TDD matrix:**

- [ ] N01–N09:完整 crash、recoverable、signal-only、requested dump、截断、非 fatal signal、地址/BuildId 分组、recoverable+death conflict、超长 tombstone。
- [ ] 明确 `EXPLICITLY_RECOVERABLE`、`DEATH_OBSERVED` 和 `CONFLICT` 可同时保留。
- [ ] 验证绝对地址、寄存器和 PID 不进入 fingerprint。
- [ ] tombstone 缺 process name 不提交;libc signal-only 无 process mapping 时只形成 UnknownProcess/SignalOnly 且不跨 Observation 合并。
- [ ] recoverable native 是正 occurrence 且 death assertion 为负,不是整体检测负例。
- [ ] libc/tombstone/native am_crash 只在同 instance、±4096 行、同 timestamp segment 可比较时 ≤60s 且最近唯一时合并。
- [x] 实现并运行定向测试。
- [ ] 运行验证全集。
- [ ] 独立评审。
- [ ] Commit: `feat: detect aosp native crash evidence`

---

## Task 7: 进程生命周期与 start-after-death

**Files:**

- Add: `crates/logcore/src/problems/recognizers/process.rs`
- Add: `crates/logcore/tests/fixtures/problems/lifecycle/**`

**Scope:**

- schema-validated `am_proc_start` / `am_proc_died`。
- ActivityManager Start proc/has died。
- 有 active mapping 时的 Zygote exit。
- 复用 Task 2 的 `ProcessInstanceTracker`;本任务只实现生命周期 observation/occurrence 策略。
- strict start-after-death。
- 30 秒仅作为 Problems UI 突出阈值,不是事实关系成立条件。

**TDD matrix:**

- [ ] P01–P09:普通 death 只作 outcome、严格 restart、UID/user 冲突、am_kill-only、schedule-only、anonymous zygote、PID reuse、显式 restarted、session 边界。
- [ ] standalone death 只在明确 signal exit 或严格 restart 时进入 Problems;普通回收不制造噪声。
- [ ] `am_kill` 只作为内部 observation;仅在同 instance、双向 4096 行、同 timestamp segment ≤60s 且唯一时附加到 provisional fault/lifecycle occurrence并设置 `KILL_REQUESTED`,不独立建 Problem、不设置 death、不回写 finalized event。
- [ ] death→start 要求 UID+process 匹配,双方都有 user 时 user 也匹配;无 user schema 只能借 active/historical mapping,永不 name-only。
- [ ] death→start 只关联下一个未冲突 matching start,不设成立时间阈值;≤30s 只影响 UI badge,recent identity 被容量淘汰时暴露 coverage limited。
- [ ] lifecycle fingerprint 使用 ProcessFingerprintKey + relation/signal class,pid/epoch/elapsed time 变化不拆组。
- [x] 实现并运行定向测试。
- [ ] 运行验证全集。
- [ ] 独立评审。
- [ ] Commit: `feat: correlate android process lifecycle facts`

---

## Task 8: LMK、kernel OOM 与内存类别互斥

**Files:**

- Add: `crates/logcore/src/problems/recognizers/lmk.rs`
- Add: `crates/logcore/src/problems/recognizers/kernel_oom.rs`
- Add: `crates/logcore/tests/fixtures/problems/memory/lmk/**`
- Add: `crates/logcore/tests/fixtures/problems/memory/kernel-oom/**`
- Add: `crates/logcore/tests/fixtures/problems/negative/**`

**Scope:**

- modern userspace lmkd strict variants。
- known kernel provenance 下的 legacy lowmemorykiller。
- known kernel provenance 下的 global/memcg OOM killer。
- LMK、kernel OOM、Java OOM 互斥。
- source unknown 的 kernel-shaped 文本不升级为正式 Problem。

**TDD matrix:**

- [ ] L01–L10:modern/legacy 变体、source unknown、select/skip/pressure、ActivityManager、death outcome、kernel OOM 互斥、OEM 自由文案、reason grouping。
- [ ] O06–O10:global/memcg、source unknown、bad_alloc native、death outcome。
- [ ] 验证 victim PID 从 message 取,而非 lmkd header PID。
- [ ] 验证 oom_adj/rss/swap/bytes 不进入 fingerprint。
- [ ] tag 必须精确为 `lowmemorykiller`;unknown/inferred kernel-shaped text 不能升级。
- [ ] kill↔death 只在同 instance、death 在后、4096 行内、同 timestamp segment 可比较时 0–60s 且最近唯一时合并。
- [ ] kernel OOM fingerprint 只含 ProcessFingerprintKey + global/memcg mechanism,动态 counters/constraint 数值不拆组。
- [x] 实现并运行定向测试。
- [ ] 运行验证全集。
- [ ] 独立评审:检查 `KILL_ISSUED != DEATH_OBSERVED`,victim 不等于内存原因。
- [ ] Commit: `feat: detect aosp lmk and kernel oom facts`

---

## Task 9: 混合仲裁与 Session 增量接入

**Files:**

- Modify: `crates/logcore/src/problems/engine.rs`
- Modify: `crates/logcore/src/session.rs`
- Add: `crates/logcore/tests/fixtures/problems/mixed/**`
- Test: `crates/logcore/tests/problems_golden.rs`

**Session interface:**

```rust
impl Session {
    pub fn scan_problems_step(&mut self, max_lines: usize) -> ProblemScanStep;
    pub fn finish_problem_input(&mut self) -> ProblemScanStep;
    pub fn problem_stats(&self) -> ProblemStats;
    pub fn begin_problem_group_snapshot(&mut self, query: &GroupQuery) -> SnapshotBuild;
    pub fn create_problem_occurrence_snapshot(
        &mut self,
        group: GroupId,
    ) -> QuerySnapshotId;
    pub fn problem_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> SnapshotPage;
    pub fn release_problem_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool;
    pub fn problem_event(&self, id: ProblemEventId) -> Option<ProblemEvent>;
    pub fn problem_detail(&self, id: ProblemEventId) -> Option<ProblemDetail>;
}
```

**Arbitration:**

- managed fatal OOME →一个 JavaOom occurrence,附 managed facet。
- lmkd kill + death →一个 LmkKill occurrence,附 death outcome。
- kernel OOM kill + death →一个 KernelOomKill occurrence,永不改成 LMK。
- crash + same-instance death →crash occurrence 附 death outcome;不重复生成普通 death。
- 无规则关系的相邻 Observation 不强行合并。
- 一项 supporting evidence 同时可落入两个候选时保持原子事实,不猜关联。
- 所有 relation 使用设计 9.4 冻结的 line/time 窗口、nearest unique 和一对一规则。
- `minimumCommitGrammar` 成立后先进入 `ProvisionalOccurrence` 固定
  `[ObservationRef;8]`;late relation 在安全 line watermark 内补证据,定稿时一次性 append
  连续 refs,finalized event 永不 relocate/回写。process death 对 Java/ANR/native/
  `am_kill` 双向关系不是安全定稿水位;timestamp 门槛只能拒绝单次关联,不能单独定稿。
- recent Observation ring ≤16,384 refs/256KiB;插入前先清理 expiry 已过项,硬上限仍超出时按
  `observationSeq` FIFO 强制淘汰,累计 `droppedRecentObservationCount` 并设置
  `correlationLimited/limited`。provisional occurrences ≤4,096/4MiB,超限确定性定稿最早项并标
  `correlationTruncatedByLimit`。
- adopted observation 按 `(line,rule,role)` 去重且每 event ≤4,096;
  `observationTotal: u16` 使用 checked increment。超过上限设置
  `observationCountLimited`;merged occurrence 超过 8 项时按固定优先级截断物化 refs、
  保留总数并明示,不截短已采用 evidence 的 event range。

**TDD steps:**

- [x] 建立 mixed golden corpus,固定所有 event tuple、每项 ObservationRef、source provenance 和 group counts。
- [ ] X01 chunk partition invariance:至少逐行边界和随机字节边界。
- [ ] X02 append invariance:任意 prefix + append remainder。
- [ ] X03–X10:逐行截断、interleave budget、同毫秒/PID、fuzz、10GB noise、分组、歧义、stable hash。
- [ ] late-correlation golden:先到/后到 am_*、death、am_kill,水位前可补 ref,水位后
  finalized event 不回写;特别覆盖 death 先到、process instance 已闭合、随后窗口内才出现
  `am_crash`/`am_kill`,证明不会提前定稿。
- [ ] 边界固定覆盖 511/512/513、4095/4096/4097 行、59/60/61 秒和等距竞争。
- [ ] provisional/recent-ring 达上限、finish flush、arena 连续性与 memory accounting
  测试;固定 ring 正常 expiry、FIFO 强制淘汰顺序、dropped counter 和
  `correlationLimited` oracle。
- [ ] 单 event 构造 8/9/255/256/4096/4097 项去重后 evidence,证明
  `observationTotal` 不回绕且超限可见。
- [x] Session 使用 `Indexer::for_each_line_span`,不物化 spans。
- [x] Session 为 static import 建立 Unknown/可靠 divider span coverage,为 adb live 接收 requested buffer 位集;逐行无法证明时保持 Unknown/Inferred。
- [x] `scan_problems_step` 每次最多 4096 行并只读 stable frontier。
- [x] reset/truncation 同时清 index、pending 和 scan cursor。
- [x] static worker 实现 `Indexing → SealStaticSource → CatchingUpProblems(循环至 scanned==stable) → FinishPending → Finished`;每批 generation 检查/yield,done 只发送一次。
- [ ] 编码/profile 变化只递增 `analysisGeneration`、清 Problems/query snapshots 并从稳定行 0 重扫;`sessionGeneration` 只在替换输入时变化。
- [x] 新增 `lock_analysis_if_current(sessionGeneration,analysisGeneration)`;扫描中的旧 analysis task 不得继续更新。`streamGeneration` 仅取消 reader,Pause/Resume 不改变 analysis token。
- [ ] 在 UI 前先运行 release 后端性能闸门:Problems ≥5.0M 行/s,记录正常/事件风暴 corpus;不达标先优化后续 IPC。
- [ ] 运行验证全集。
- [ ] 独立交叉评审:规则实现者不得独自批准混合仲裁。
- [ ] Commit: `feat: integrate incremental problem analysis into sessions`

---

## Task 10: Tauri 调度、进度与有界 IPC

**Files:**

- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/dto.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: 以上文件 tests

**Scheduling:**

现有 static index worker 在每个索引片后，用短临界区追赶仍在页缓存中的 Problems 数据:

```text
index_step(1MiB) → unlock
repeat at most 32 times:
  scan_problems_step(max 4096 physical lines / 128 detail lines) → unlock → yield
emit index progress after 8MiB cumulative advance or terminal state
emit throttled Problems progress
```

事件风暴会在达到 128 条实际 candidate/pending-detail 行时提前结束当前 step；普通稀疏
语料仍受 4096 物理行上限约束。静态文件到 EOF 后必须进入 catch-up 循环，不能假设 32
步一定追平。analysis token 只含 session/analysis generation；`streamGeneration` 仅为
transport cancellation token。

Pause/Resume/Stop/Start 通过专用 `stream_control` mutex 串行化。转换顺序为:使 reader generation 失效 → 不持 Session lock 时 kill/join 并确认 EOF → 用匹配 session generation 更新 Session lifecycle → 发布 runtime 状态。失败进入 `ControlError`,未确认 EOF 时不得标 Sealed。

**Commands/events:**

- `get_problems_status`
- `get_problem_groups`
- `get_problem_occurrences`
- `get_problem_detail`
- `export_problem_logs`
- `release_problem_snapshot`
- `problems:progress`

不新增 Problems 原文读取命令。上下文复用
`get_rows_checked(view="all", count≤512)`；`export_problem_logs` 只在同一 Session 锁内
把 opaque event id 解析成范围，随后复用现有 range export worker。

**Limits:**

- group page default 100,max 200。
- occurrence page default 100,max 200。
- 第一次查询在服务端冻结 core snapshot，客户端只收到 `snapshotHandle`；后续 cursor
  绑定 snapshot + position + query signature 且单次消费，不暴露 core `querySnapshotId`。
- snapshot 同时 ≤8、TTL 5 分钟、ID vectors 合计 ≤16MiB;过期返回 `snapshot-expired`。
- 分类/排序/group 切换和刷新显式调用 generation-safe `release_problem_snapshot`;异常客户端遗留项由 TTL/LRU 回收。
- 所有 Problems 请求携带
  `expectedAnalysisToken { sessionGeneration, analysisGeneration }`;response 回显 token/revision/snapshot。
- Session 保存 applied `filterResultRevision/requestId`；`get_rows_checked` 与
  `map_source_line` 接收 expected analysis token + decode revision；filtered scope 另校验
  expectedFilterResultRevision，不匹配返回 `stale-filter-result` 并回显实际 revision。
- `export_problem_logs(eventId,expectedAnalysisToken,mode,radius)` 原子解析 range,消除跨 Session TOCTOU。
- stale session、unknown id、expired snapshot 返回明确错误。
- progress 只有 counters/revision,无数组;包含
  `droppedRecentObservationCount` 与 `correlationLimited`,不得把关联证据被淘汰伪装成
  完整 coverage。
- `set_filter` 接收/回显 filter input revision 或 request id,使前端能区分输入 revision 与已应用 result revision。

**TDD steps:**

- [x] DTO serde/camelCase 测试。
- [ ] 分页 clamp、未知 id、stale generation、snapshot expiry/capacity 测试。
- [ ] 第一页后扫描 revision 增长,用同 snapshot 取第二页无重复/遗漏/重排。
- [ ] recent ring 强制淘汰后 status/progress 精确回显
  `droppedRecentObservationCount` 与 `correlationLimited=true`。
- [ ] release snapshot 的 generation/unknown/idempotent 测试;快速切换时已释放旧项不挤掉当前 group/occurrence snapshot。
- [ ] group snapshot 4,096-record 短锁 build/锁外 sort/安装测试;max cardinality 下任一 Session lock 段 ≤20ms。
- [ ] index worker 交替推进、EOF catch-up 到最终 stable line 后才 done 测试。
- [ ] stream append/Pause/Resume/Stop→Sealed/Stop 后 Resume 被拒绝/Clear 测试。
- [ ] 重叠 Pause+Resume、Pause+Stop、Stop+Start 竞态测试;断言单一转换顺序、无 join-under-Session-lock、runtime/Session 不分叉。
- [x] live start 把 requested buffers 送入 Session,但混合 stdout 行不被误标 Known。
- [ ] 选择 event 后切换 Session,detail/locate/export 全部拒绝旧 generation 的竞态测试。
- [ ] `set_config` 改 encoding 时只递增 analysis generation + decodeRevision 并重扫;旧 progress/query 失效,Session/input 不替换。
- [ ] Pause/Resume analysis token 不变;Stop 后 Start session/analysis token 都变化。
- [ ] filter request id/revision 乱序完成测试。
- [ ] filtered mapping/get_rows 与 `filter:done` 两种完成顺序竞态测试;R1 请求不得按 R2 dataset 回答。
- [ ] generation 变化后旧任务不更新 Session、不 emit 可接受事件。
- [ ] progress emit 在锁外的结构评审/测试。
- [ ] 在前端任务前运行 combined release gate:index+Problems 中位数 ≤37s、`get_rows(200)` p99 ≤5ms、Problems lock ≤20ms;Task 14 仅做最终复测/报告。
- [ ] 运行验证全集。
- [ ] 独立评审:检查 Tauri 中没有复制 recognizer 逻辑。
- [ ] Commit: `feat: expose bounded problems queries and progress`

---

## Task 11: 主表 TableScope 深模块

**Files:**

- Modify: `package.json`
- Modify: `pnpm-lock.yaml`
- Modify: `vite.config.ts`
- Add: `src/test/setup.ts`
- Modify: `src/types.ts`
- Modify: `src/store/session.ts`
- Modify: `src/store/session.test.ts`
- Modify: `src/lib/table.ts`
- Modify: `src/lib/table.test.ts`
- Add: `src/lib/tableScopeController.ts`
- Add: `src/lib/tableScopeController.test.ts`
- Modify: `src/lib/ipc.ts`
- Modify: `src/App.tsx`
- Add: `src/App.tableScope.test.tsx`
- Modify: `src/components/Toolbar.tsx`
- Modify: `src/components/LogTable.tsx`
- Modify: `src/components/Minimap.tsx`
- Modify: `src/components/StatusBar.tsx`

**Model:**

```ts
type TableScope =
  | { kind: "results"; view: "filtered" }
  | {
      kind: "problem-context";
      occurrence: ProblemOccurrenceRef;
      eventRange: LineRange;
      contextRange: LineRange;
      returnPoint: TableReturnPoint;
    };
```

先固定组件测试基建:Vitest `jsdom` + Testing Library + user-event + jest-dom,由 `vite.config.ts` 和 `src/test/setup.ts` 统一配置;setup 提供可控的 `ResizeObserver` stub。Task 11/12/13 的交互验收必须自动化,不留到 Task 13 再决定。

新增纯函数
`resolveTableDataset(scope,status,sessionGeneration,decodeRevision,filterResultRevision,sourceDataRevision)`
统一产出:

- rowsView
- rowCount
- cacheKey/revision
- minimap visible/hidden

revision 语义:

- `filterInputRevision`:用户编辑时递增,用于 returnPoint/请求 identity。
- `filterResultRevision`:后端已应用 filter dataset 的 identity,只用于 Results cache/filtered IPC expected revision。
- `sourceDataRevision`:stable All 行变化时递增,只更新 rowCount/尾块状态。
- `decodeRevision`:encoding 变化时递增。
- All-context cache identity 只含 session generation + decodeRevision + rows view,排除 filter/source revision;完整历史块不可因 append 全量失效,只刷新不足额尾块。

新增唯一 `navigateToSourceLine(lineNo,reason)`:

- Task 11 创建 controller 骨架并实现该入口、scope mapping 与 request nonce;Task 13 在同一模块扩展 Problems 方法。
- results scope 先做 analysis-token + expectedFilterResultRevision-safe mapping;anchor 使用 Exact,viewport restore 使用 Nearest(同距优先之前)。
- context scope 直接使用 All 行序。
- scope/selection/viewport/scrollRequest 单次原子更新。
- Toolbar search/行号、App F2/F3 bookmark、LogTable search scroll 和 follow-latest 不得再直接消费 filtered result index。

**TDD steps:**

- [x] 先写 ResultsScope 与现有 filtered 行为逐字段等价测试。
- [x] 写 ProblemContextScope → rowsView all、rowCount stableLines、minimap hidden;live 尾部未闭合行不可见。
- [x] 写 Results cache key 包含 decodeRevision + filterResultRevision;All cache identity 包含 decodeRevision 并排除 filterInput/filterResult/source revision。
- [ ] 历史 context 连续 append 时不重拉完整可见历史 blocks;不足额尾块才按需刷新。
- [x] encoding 改变只通过 decodeRevision 失效已解码 All/Results blocks,Pause/Resume 不失效。
- [ ] 写 context 中 `filter:done` 不钳位 selection/viewport/rowCount。
- [ ] 写 search/行号/bookmark/follow-latest 全部经过 `navigateToSourceLine` 的 reducer/controller 测试。
- [ ] 在 `src/App.tableScope.test.tsx` 用 mocked IPC 做 Toolbar/App/LogTable
  集成测试,确保没有绕过 controller 的直接 filtered mapping 调用。
- [ ] 把 `WINDOW=200` 提升为共享常量,IPC adapter 对 `count<=0` 或 `count>512` 拒绝。
- [x] LogTable 总数、view、cache、scroll clamp 全部改读 TableDataset。
- [ ] LogTable 从首个可见且已加载 row 记录 viewport source line,迟到 rows/mapping 用 generation + request nonce 丢弃。
- [ ] Minimap context 时零请求且渲染不可聚焦 presentation rail,返回 results 后重新请求。
- [x] StatusBar 在 context 显示“临时未过滤上下文”,Toolbar/App 导航遵守 scope。
- [x] follow-latest 在 context 中先退出 context,再恢复 tail-follow。
- [ ] 退役当前未生效/产生双重语义的 store `view`。
- [ ] 运行验证全集。
- [ ] 独立评审:确认主表没有残留硬编码 `filteredLines/getRows("filtered")`。
- [ ] Commit: `refactor: make table scope the source of row semantics`

---

## Task 12: Problems 分页状态与底部工作台

**Files:**

- Modify: `src/types.ts`
- Modify: `src/lib/ipc.ts`
- Add: `src/lib/problems.ts`
- Add: `src/lib/problems.test.ts`
- Add: `src/lib/problemHints.ts`
- Add: `src/lib/problemHints.test.ts`
- Add: `src/store/problems.ts`
- Add: `src/store/problems.test.ts`
- Add: `src/components/ProblemsDock.tsx`
- Add: `src/components/ProblemsDock.test.tsx`
- Modify: `src/App.tsx`
- Modify: `src/index.css`

**UI state:**

```ts
interface ProblemsUiState {
  panelOpen: boolean;
  panelHeight: number;
  kindFilters: ProblemKind[];
  sort: "last-seen-desc" | "count-desc";
  selectedGroupId: string | null;
  selectedEventId: string | null;
  groupSnapshotId: string | null;
  occurrenceSnapshotId: string | null;
  hasNewResults: boolean;
}
```

**Behavior:**

- 默认折叠,检测完成/严重事件不自动展开。
- 展开默认 280px,下限 180px;ResizeObserver 观察包含 Main+dock 的 `.lf-workbench`,按 `min(45vh,workbenchHeight-160px)` 钳位,不能观察 `.lf-main` 形成反馈环。高度低于 340px 时临时 layout-collapse 并保留 open preference。
- 折叠只订阅 summary/progress,不请求列表页。
- group/occurrence 使用虚拟分页。
- 当前 revision 更新时显示“有新结果,刷新”,继续使用旧 snapshot;只有用户刷新才创建新 snapshot。
- 分类/排序/group 切换和刷新先调用 `release_problem_snapshot`;后端 TTL 仅作异常兜底。
- snapshot 过期显示明确状态并保留已渲染内容,不自动混入新排序。
- 后端 `FactCode/ObservationRef` 映射为可单独定位的 facts;hints 只来自独立 `problemHints.ts` 静态目录。
- facts 与 hints 结构/视觉分区,类型上不允许 hints 反写后端事实。
- limited 时分别显示 observed/stored/dropped;列表和导出只针对 stored occurrences。
- `correlationLimited` 时显示“部分晚到关联证据可能未保留”及 dropped recent count,
  不把它表述成 occurrence 丢失或事实未发生。
- 折叠 badge 正常显示 observed count,limited 时显示“检出 N · 可展开 M”。

**TDD steps:**

- [x] Problems snapshot pagination、去重、旧 generation 丢弃和 snapshot-expired 状态测试。
- [x] 第一页后 revision 增长,第二页仍来自同 snapshot 且无重复/遗漏/重排。
- [x] 面板折叠时 group/occurrence 请求为 0。
- [x] 展开只取第一页,靠近尾端才取下一页。
- [x] 分类/排序/group 切换通过 IPC 释放对应 snapshot 并重置 cursor。
- [x] 10,000+ synthetic groups 的虚拟列表验证。
- [ ] 空态固定为“在已捕获范围内未检测到”;覆盖扫描中、完成、limited、coverage/provenance 不足和错误态。
- [ ] `correlationLimited` 单独文案与 dropped recent count 测试;不得显示成“未死亡”或
  根因结论。
- [x] `FactCode → 文案` 与 `ProblemKind → 排查提示` 都做 TypeScript 穷尽映射测试;提示不得出现后端事实字段。
- [x] 每项 fact 的定位按钮使用自己的 sourceLine,不是统一 anchor。
- [x] `observationRefsTruncated` 时显示“仅展示 8/M 条关键证据,可查看事件范围”,不得静默少事实。
- [ ] `ProblemsDock.test.tsx` 固定断言 badge 的 N 来自
  `observedOccurrenceCount`、M 来自 `storedOccurrenceCount`;limited 时不得把 dropped
  occurrence 算作可展开项。
- [ ] progress live region 只宣告完成、limited、错误和“有新结果”,不逐批朗读。
- [ ] 虚拟 listbox 使用 `aria-activedescendant`;ArrowUp/Down + scrollToIndex、跨分页加载、aria-posinset/setsize、Enter/Space、snapshot-expired focus fallback 测试。
- [ ] listbox 保持唯一焦点,option 不可聚焦且不是 button;group 的
  `aria-setsize` 取冻结 snapshot 的 queryable/stored group 数,occurrence 取冻结 stored
  count。
- [ ] separator 的 aria min/max/now、方向键 16px、Page 64px、Home/End 和 ResizeObserver clamp 测试。
- [ ] 覆盖 960×720、1180×720、极矮窗口和工具栏换行后的 dock clamp,断言无 ResizeObserver 振荡。
- [x] 插入位置固定为 Main 与 StatusBar 之间。
- [ ] 运行验证全集。
- [ ] 独立视觉/交互评审,对照已确认底部工作台设计。
- [ ] Commit: `feat: add the bottom problems workbench`

---

## Task 13: 定位、临时上下文、返回、导出与可访问性

**Files:**

- Modify: `src/lib/tableScopeController.ts`
- Modify: `src/lib/tableScopeController.test.ts`
- Modify: `src/store/session.ts`
- Modify: `src/App.tsx`
- Modify: `src/components/Toolbar.tsx`
- Modify: `src/components/LogTable.tsx`
- Modify: `src/components/ProblemsDock.tsx`
- Modify: `src/components/StatusBar.tsx`
- Modify: `src/components/ToolDialogs.tsx`
- Add: `src/components/ToolDialogs.test.tsx`
- Modify: `src/types.ts`
- Modify: `src/index.css`

**Controller interface:**

```ts
interface TableScopeController {
  navigateToSourceLine(lineNo: number, reason: NavigationReason): Promise<void>;
  locateProblem(occurrence: ProblemOccurrenceRef): Promise<"located" | "context-opened">;
  openProblemContext(occurrence: ProblemOccurrenceRef, radius?: number): void;
  returnToResults(): Promise<void>;
}
```

复杂度隐藏在 controller:

- filtered anchor 映射。
- context range 钳位。
- returnPoint。
- filter input/result revision 分离与
  `PendingRestore { viewportLine, selectedLine, filterInputRevision, requestNonce }`。
- session/generation 失效。
- async request nonce。
- tail-follow 暂停。
- focus/live announcement。

**TDD/acceptance:**

- [x] anchor 在 filtered 中:不切 scope,居中定位。
- [x] anchor 被过滤:自动打开 all-backed context。
- [x] 显式查看上下文始终进入 context。
- [ ] 进入/切换/返回期间 `setFilter` 调用次数为 0,FilterSpec 深比较不变。
- [x] context 中切换 occurrence 不覆盖原 returnPoint。
- [x] 返回恢复原 viewport/selection;过滤变化时按 source line 重映射。
- [ ] context 中修改过滤并立即返回时保存完整 PendingRestore;只有匹配当前 input revision 的 `filter:done` 到达后才最终定位。
- [ ] pending 等待 R1 时再次编辑为 R2:保留 source-line 目标,把 pending revision 替换为
  R2 并递增 nonce;R1 done/映射迟到均忽略,只有 R2 applied 后恢复。继续编辑 R3
  同理,不能积累多个 pending。
- [x] viewport line 不可见时用 Nearest 映射,selected line 不可见时清 selection;不得沿用旧 result index。
- [ ] filter 失败/取消时清 pending、显示错误并按最后一次已应用结果安全恢复。
- [ ] input revision 尚未应用时的普通 source-line 导航排队等待,显示“正在应用最新过滤…”,不使用旧结果。
- [ ] context 中任何 `filter:done` 不改变 context viewport/selection/rowCount。
- [ ] session 变化时安全退出且旧返回点不可用。
- [x] session/analysis generation 或 request nonce 变化后,迟到的 rows/line mapping/detail 响应全部丢弃。
- [ ] filtered rows/mapping 携带 expectedFilterResultRevision;`stale-filter-result` 触发按当前 applied revision 重试/等待,不接受跨 dataset 响应。
- [x] context 行请求始终为 all 且 count≤512。
- [x] event range 淡色、anchor 强高亮。
- [ ] context banner 即使 Problems 折叠仍可见,固定文案“当前过滤保持,但暂不应用于此上下文”;StatusBar 显示临时未过滤语义。
- [ ] tail-follow 进入和返回后均保持暂停。
- [x] 用户主动“追最新”在 context 中先退出 context,再恢复尾随。
- [x] ExportDialog 支持“事件范围(含区间内原始日志)”和 ±50 行上下文。
- [x] 前端导出只传 eventId/expectedAnalysisToken/mode/radius;切换 Session 或 encoding 后旧 event 导出被后端拒绝。
- [x] 导出原始字节,不插 facts/hints。
- [ ] ExportDialog 单一 owner、initial focus、focus trap、Escape、关闭后恢复 occurrence;虚拟项卸载时 fallback 到 panel heading/toggle。
- [ ] 折叠按钮、region、chips、separator、列表、live region 和 focus return 可仅键盘操作,由 Task 11 的 jsdom/Testing Library 基建自动验证。
- [ ] 运行验证全集。
- [ ] 独立可访问性与交互评审。
- [ ] Commit: `feat: navigate and export problem context without changing filters`

---

## Task 14: 10GiB 基准、文档与独立终审

**Files:**

- Modify: `crates/logcore/examples/bench.rs`
- Add: `docs/superpowers/2026-07-xx-benchmark-problems-10gb.md`
- Modify: `docs/architecture.md`
- Modify: `README.md` 或用户手册的功能说明(以当前文档结构为准)
- Modify: `AGENTS.md`(仅在接口/性能基线成为持久约束后)

**Benchmark corpus:**

- 正常稀疏事件密度。
- 多行事件跨 8MiB 边界。
- 高重复 fingerprint。
- 高 distinct fingerprint。
- 事件风暴触发 limits。
- Problems 扫描期间并发采样 `get_rows(200)`。

**Hard gates:**

- Problems 单独吞吐 ≥5.0M 行/s。
- 10GiB index + Problems ≤37s,不超过 20.6s baseline 的 1.8 倍。
- 扫描期间 `get_rows(200)` p99 ≤5ms。
- Problems 单次锁临界区最大 ≤20ms。
- 原索引锁停顿最大 ≤50ms。
- adversarial corpus 受结构性数量/逻辑预算限制且 `limited=true`;受控 retained heap 目标 ≤128MiB。

**Stretch:**

- index + Problems ≤25.8s。
- `get_rows(200)` p99 ≤2ms。
- occurrence 平均 32–40 bytes。

**Final steps:**

- [ ] 记录机器、文件大小、行数、release 命令、冷/暖缓存各 3 次与中位数、前后数字和 retained heap/RSS 口径。
- [x] 运行完整 positive/negative/mixed/incremental corpus。
- [x] 运行验证全集。
- [x] 做隐私扫描,确认 fixtures/docs 无真实路径、姓名、设备标识和内网信息。
- [x] 独立终审者检查:
  - 是否消费未稳定尾行。
  - Stop 是否封口且不可 Resume,Pause 是否保持可恢复。
  - source provenance 是否被夸大,伪造 EventLog/kernel 文本是否越权升级。
  - 是否保存日志原文或无界 String。
  - merged occurrence 的每项事实是否有有界 ObservationRef 和独立定位行。
  - 是否每次 append 全量排序。
  - 是否遗漏 session/analysis generation 或存在跨 Session TOCTOU。
  - snapshot 是否在 revision 增长时稳定分页且受 TTL/容量约束。
  - IPC 是否能返回全量数据。
  - facts 是否暗含根因判断。
  - filter 是否在 context 期间被修改,filter input/result revision 是否混淆。
  - 10GiB 报告是否同时覆盖吞吐、内存和锁停顿。
- [x] 修复终审问题后重新跑验证全集。
- [x] Commit: `docs: record problems workbench architecture and benchmark`

## 范围外

- OEM/应用自定义规则 DSL。
- LLM/AI 自动分析。
- sidecar cache。
- bugreport/tombstone/ANR traces 容器导入。
- native/R8 符号化。
- 跨 session/boot/file 关联。
- 全页时间线。
- 调查证据包。

以上范围必须另立设计和实施计划,不能在本计划执行中顺带加入。
