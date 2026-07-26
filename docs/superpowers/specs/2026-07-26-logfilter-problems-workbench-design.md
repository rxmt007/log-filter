# LogFilter 故障调查工作台设计

日期: 2026-07-26

状态: 设计已确认,待实施

关联规范:

- `docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md`
- `docs/superpowers/specs/2026-07-02-logfilter-ui-control-system-and-live-follow-design.md`
- `docs/superpowers/2026-07-06-benchmark-10gb.md`
- `docs/design/LogFilter.dc.html`
- `docs/design/LogWindow.dc.html`

本设计是主规范的功能增补。与主规范冲突时,仍以“10GB+、不整体载入、前端只取可见窗口、logcore 与 UI 解耦”等架构不变量为最高约束。

## 1. 背景

当前 LogFilter 能高效打开、过滤、搜索和导出 10GB 级 Android 日志,但用户仍需要依靠经验在数千万行文本中回答:

- 日志中是否记录了 Java/Kotlin 崩溃、ANR、native crash、OOM/LMK 或进程异常退出?
- 同一种故障发生了多少次,首次和最后一次在哪里?
- 当前过滤条件隐藏了故障锚点时,如何查看原始上下文而不破坏现场?
- 哪些内容是日志明确记录的事实,哪些只是后续排查方向?

当前 `RowsView::Errors` 只收集 E/F 级别行,没有多行事件边界、同进程证据关联、指纹或分组语义。Problems 必须是独立的派生事件索引,不能改写 Errors 的既有含义。

用户已确认采用**默认折叠的底部工作台**。右侧检查器和全页时间线不作为 MVP 主布局;时间线可在后续作为用户主动进入的调查模式。

## 2. 目标

1. 基于 Android/AOSP 的稳定日志事实,确定性识别:
   - Java/Kotlin 未处理异常
   - ANR
   - native crash
   - 进程死亡和快速再次启动
   - Java OOM、kernel OOM kill
   - LMK/lmkd kill
2. 正确处理多行事件、扫描块边界、实时增长文件的暂时 EOF、PID 重用和相邻事件交错。
3. 将同类事件按版本化指纹分组,展示首次/最后发生时间、重复次数及每个 occurrence 的原始行范围。
4. 点击事件时定位当前过滤结果;若锚点被过滤隐藏,进入临时未过滤上下文,且不修改当前 FilterSpec。
5. 导出事件范围(含区间内原始日志)或事件前后上下文,保持原始日志逐字节语义,不插入分析文字。
6. 严格区分“检测到的事实”和“排查提示”,不自动声称根因。
7. 保持 10GB+ 架构不变量,事件索引不保存日志原文,IPC 不返回无界数组。
8. 静态文件和 adb 实时抓取使用同一识别实现,在输入最终完整后得到相同事件结果。

## 3. 非目标

MVP 不做:

1. 大模型扫描、自动根因判断或自动修复建议。
2. 任意应用业务日志的自动语义理解,例如把 `Payment failed` 自动提升为平台故障。
3. 默认开启未经脱敏样本和负例验证的 OEM 猜测规则。
4. 完整 bugreport、独立 tombstone 文件、ANR traces 文件的专用导入器。
5. native 符号化、ProGuard/R8 mapping 反混淆。
6. 跨文件、跨设备或跨会话的因果关联。
7. Problems sidecar 持久缓存。
8. 全页事件时间线。
9. 将所有事件、所有分组或大段日志正文一次性发送到前端。

## 4. 已确认的产品与技术取舍

| 项目 | MVP 决策 |
|---|---|
| 主布局 | 主表下方的底部工作台,默认折叠 |
| 检测范围 | 扫描日志中全部进程;普通生命周期事件默认弱化/折叠 |
| 快速再次启动 | 严格身份匹配的 death→start 始终是事实关系;间隔 ≤30 秒时在 Problems 中突出显示 |
| OOM 默认范围 | 未处理/致命 Java OOME、明确 kernel OOM kill 和明确 lmkd kill |
| 规则来源 | AOSP-first 固定规则 profile;OEM 规则后置且默认关闭 |
| 识别方式 | 确定性解析 + 多行状态机 + 证据关联;关键词只做廉价候选预筛 |
| 大模型 | 不进入 MVP 检测链路 |
| 导出 | 仅原始事件范围或原始上下文,不混入分析文案 |
| 缓存 | MVP 每次会话重扫,暂不持久化 Problems 索引 |
| 同类语义 | fingerprint 用于分组,不用于删除真实重复 occurrence |

## 5. 领域术语

### 5.1 Observation

一条由确定性规则识别出的原子日志事实。例如:

- AndroidRuntime 记录 `FATAL EXCEPTION`
- 日志中出现符合 AOSP `am_crash` schema 的 EventLog-shaped 记录
- lmkd 记录已发出 kill
- ActivityManager 记录 `am_proc_died`

Observation 自身不携带根因结论。

### 5.2 Problem occurrence

用户在 Problems 中看到的一次故障事件。一个 occurrence 可以由一项已满足
`minimumCommitGrammar` 的 Observation 单独形成,也可以由同一进程实例内、满足明确关联规则的多个
Observation 组成。

例如 AndroidRuntime 未处理 OOME、`am_crash` 和同一进程实例的 `am_proc_died` 可以组成一个 Java OOM occurrence;详情分别列出三项事实,结局标记为“进程结束得到日志佐证”。这不等于宣称某个分配动作是唯一根因。

### 5.3 Problem group

相同 `kind + fingerprintVersion + fingerprint` 的 occurrence 集合。组展示重复次数、首次/最后 occurrence 和代表性签名。

“同组”只表示归一化日志签名相似,不表示根因相同。

### 5.4 Event range 与 anchor

- `startLine`: occurrence 第一条已匹配证据行,1-based 展示。
- `endLine`: occurrence 最后一条已匹配证据行,包含端点。状态机可以跨过预算内的交错行,但不能因为“正在等待后续证据”把任意日志推进为 endLine。
- `anchorLine`: 最适合作为用户定位目标的提交级证据行。

内部存储为 0-based 索引,IPC/界面统一转换为 1-based。

### 5.5 Process instance

用于避免 PID 重用的进程实例身份。至少由:

- pid
- 已解析的 process name
- user/uid(存在时)
- 最近一次明确 start observation 所形成的 epoch;日志从进程中段开始时,首个满足身份 grammar 的强 observation 可创建 provisional epoch

共同确定。provisional epoch 在 death、矛盾 identity 或新 start 时闭合,并以较低 identity completeness 暴露。只有 PID 相同不足以关联两个 Observation。

### 5.6 Process fingerprint key

`ProcessFingerprintKey` 只用于同类分组,与 Process instance 严格分开:

- MVP 固定为归一化 process name。
- 不包含 pid、start epoch、时间或 session generation。
- user/uid 不进入 `process-fingerprint-v1`,避免同一应用的不同运行实例被无意义拆组。
- process name 缺失时使用明确 unknown 值并标记 `IdentityQuality::UnknownProcess`。

`ProcessInstanceKey` 用于 occurrence 内关联;`ProcessFingerprintKey` 用于跨 occurrence 分组。实现中必须使用不同类型,禁止把前者直接喂给 fingerprint。

### 5.7 Coverage

当前日志实际包含的输入范围和检测能力,包括:

- 采集命令请求过哪些 logcat buffers
- 已扫描的稳定行数
- 日志是否截断
- 规则 profile/version
- 是否达到事件索引上限

空态必须写“在已捕获范围内未检测到”,不能写“没有发生”。

Coverage 分三层:

- session 级 `InputCoverage`:
  - `requestedBuffers`:live capture 启动时请求的 main/system/events/crash/radio 位集;静态导入通常为 unknown。
  - `captureOrigin`:static-file/adb-live。
  - `rangeCompleteness`:unknown/bounded/start-truncated/end-truncated。
  - 请求过某 buffer 只说明采集能力,不等于每一行都能证明来自该 buffer。
- 行级 `LineProvenance`:
  - `Known(buffer)`:仅当输入适配器提供可靠 buffer 元数据,或格式规范明确保证当前 source span 时使用。
  - `Inferred(buffer)`:精确 source-specific tag/schema 与 session coverage 不矛盾,但原始文本没有逐行 buffer 标识。
  - `Unknown`:无法可靠归属。
  - 连续已知区间使用紧凑 `SourceSpanIndex` 保存,不得给每行附加字符串。普通 `-v time/threadtime` 多 buffer 合并输出不保留逐行 buffer identity,默认不能标为 `Known`。
- occurrence 级:
  - `evidenceFormat`:eventlog-shaped-text/aosp-text/tombstone-shaped-text/kernel-shaped-text。
  - 每个 `ObservationRef` 保存其自己的 source/provenance,不能把 occurrence 中一条已知来源推广给全部事实。
  - boundary/truncation flags。

来源判定规则:

- 精确 EventLog tag/schema 是**文本格式事实**,不是经过认证的 buffer 事实。UI 写“日志中出现符合 AOSP `am_crash` 格式的记录”,不得无条件写“events buffer 记录了崩溃”。
- live capture 的 `requestedBuffers` 明确不包含 events 时,EventLog-shaped
  文本不能成为 EventLog 提交级证据;它最多是未采用的候选。
- events 在 requested buffers 中但逐行来源缺失时,唯一 schema 可标为 `Inferred(events)`。静态文件没有 contradictory known span 时也只能标 `Inferred(events)`。
- standalone EventLog-shaped 记录只有 `Known(events)` 才可独立提交 Problem。`Inferred(events)` 只能佐证另一个非 EventLog 的 minimum commit grammar;若该行已被证明处于 main/system span,则完全拒绝 EventLog anchor。live 只请求 events 或可靠 bugreport event section 可提供 Known;普通多 buffer 合并输出不能。
- 当前 adb live capture 不包含 kernel buffer。kernel OOM/legacy kernel LMK 只有 `Known(kernel)` 才能升级为正式 Problem;来源 inferred/unknown 的 kernel-shaped 文本仅计入 coverage/malformed 统计。
- 静态文件只有在适配器能够证明 divider/span 语义时才建立 `SourceSpanIndex`;看见一行 `beginning of ...` 但无法证明后续区间语义时仍保持 Unknown。

## 6. 信任模型:事实与提示严格分离

### 6.1 后端只产出事实代码

logcore 只产出:

- `ProblemKind`
- `RuleId`
- `FactCode`
- 每项事实的 `ObservationRef(ruleId,line,role,evidenceFormat,lineProvenance)`
- `EvidenceFlags`
- 行范围与 pid/`ProcessInstanceKey`
- `OutcomeFlags`
- `BoundaryFlags`
- fingerprint 及其版本
- coverage/limited 状态

模型中不得出现:

- `rootCause`
- `diagnosis`
- `causeConfidence`
- “内存泄漏”“死锁”“代码缺陷”等自动结论字段

### 6.2 OutcomeFlags

结局使用可组合、可审计的事实位,不使用百分比置信度:

- `KILL_REQUESTED`:平台或系统服务记录了 kill 请求,但没有证明信号已成功发出。
- `KILL_ISSUED`:lmkd/kernel 明确记录已发出 kill。
- `DEATH_OBSERVED`:同一进程实例的结束得到独立日志记录。
- `START_AFTER_DEATH_OBSERVED`:严格身份匹配的进程在 death 后再次启动。
- `EXPLICITLY_RECOVERABLE`:平台字段明确标记该 native 事件可恢复。
- `CONFLICT`:日志同时记录了互相矛盾或无法自动解释的结局事实。

最低提交语法已经满足、但没有任何上述结局位时,UI 显示“结局未知”,不能显示“未死亡”。
`truncatedByLimit`、`truncatedByInput` 等属于边界标记,不与 OutcomeFlags 混合。

### 6.3 前端文案分区

详情固定分为:

```text
检测到的事实
—— 来自具体规则和源行,可定位

排查提示(非结论)
—— 按 ProblemKind 映射的通用调查清单
```

固定文案不得使用“根因是”“已导致”“确定为内存泄漏”等表达。ANR Reason、lmkd reason 等必须写成“系统记录的 reason”,不能改写成根因。

### 6.4 大模型的未来位置

如后续增加用户主动触发的辅助分析,其输入只能是:

- 已确定的结构化事实
- 用户明确选择的、上限不超过 512 行的上下文

输出只能进入“排查提示/调查笔记”,不能修改 kind、range、fingerprint、count、OutcomeFlags 或 coverage。不得自动上传整份日志。该能力不属于 MVP。

## 7. 检测流水线

```text
mmap 字节与稀疏行索引
  → 稳定完整行前沿
  → InputCoverage/SourceSpanIndex 归属
  → ASCII Tag/前缀候选分类
  → 候选行借用式解析
  → 多类 recognizer 状态机
  → 同一进程实例内的确定性关联与仲裁
  → 版本化指纹
  → 紧凑 occurrence/group 索引
  → 有界分页查询
```

### 7.1 候选预筛

每行先用无分配字节检查识别少量候选 Tag/前缀,例如:

- `AndroidRuntime`
- `ActivityManager`
- `am_crash` / `am_anr` / `am_proc_start` / `am_proc_died`
- `libc` / `DEBUG`
- `lmkd` / `lowmemorykiller`
- kernel OOM 的严格前缀

只有候选行及其已打开状态机所需的 continuation 才进入完整解析/解码。不能对全部 7115 万行分别运行多套正则。

### 7.2 规则 profile

MVP 固定 `aosp-v1` profile:

- 规则具有稳定 `RuleId`,如 `aosp.java-uncaught.v1`。
- 规则变化会增加 detector/fingerprint version。
- 不向用户开放任意规则 DSL 或插件 seam。
- OEM 规则必须作为独立 profile,有正例、最近似负例和来源说明,默认关闭。

### 7.3 RuleContract

每个规则必须显式声明,不能只留下一个宽泛 regex:

```text
ruleId
kind
schemaVariants
candidateStart
minimumCommitGrammar
optionalEvidence
supportingEvidence
forbiddenEvidence
correlationKey
boundary { start, continue, stop, maxLines, maxBytes, unmatchedBudget }
fingerprintCanonical
signatureQuality
identityQuality
priority/dedup
outcomeContribution
coverageRequirement
```

`candidateStart` 只允许打开有界候选,不能产生 Problem;只有
`minimumCommitGrammar` 全部满足后候选才有资格成为 provisional occurrence。
`optionalEvidence`/`supportingEvidence` 只能丰富 range、facts、fingerprint 或 outcome,不能补救缺失的最低语法。

规则一旦发布不得在同一 version 下悄悄改变语义。任何影响边界、归类或 fingerprint 的变更都必须升版并保留旧 fixture。

### 7.4 EventLog-shaped AOSP schema

符合 AOSP EventLog tag 的文本必须按 arity 和字段类型解析,不能按逗号简单切分或假设只有一个 Android 版本:

- `am_anr`:历史版本可能为 `[pid,package,flags,reason...]` 或前置 user。
- `am_proc_died`:可能为 `[pid,process]`、前置 user,或尾置 oomAdj/procState。
- `am_proc_start`:可能无 user 或前置 user。
- `am_crash`:可能无 user、前置 user,或尾置 recoverable。
- `am_kill`:可能无 user、前置 user,或尾置 rss。

reason/message 可能含逗号。解析器应从固定数字前缀和固定尾字段双向验证;只有唯一
schema 成功时才有资格参与提交或关联。歧义、字段溢出和缺失右括号记录为
malformed/coverage,不能猜字段。

解析成功只证明文本符合该 schema。`ObservedLine` 还必须携带 `InputCoverage` 和
`LineProvenance`;规则再按 5.7 的来源判定决定它能否独立满足最低提交语法,还是只能作为
支持证据。字段值、header pid 与活跃 `ProcessInstanceKey` 冲突时不得靠字符串相似度强行关联。

### 7.5 官方格式基准

规则契约以一手格式为准:

- AOSP EventLog tags: <https://android.googlesource.com/platform/frameworks/base/+/master/services/core/java/com/android/server/am/EventLogTags.logtags>
- Android RuntimeInit: <https://android.googlesource.com/platform/frameworks/base/+/b9c98916b11932e7e55849c9c69d116002671a0e/core/java/com/android/internal/os/RuntimeInit.java>
- Native crash 格式: <https://source.android.com/docs/core/tests/debug/native-crash>
- Bugreport/ANR 阅读说明: <https://source.android.com/docs/core/tests/debug/read-bug-reports>
- lmkd 实现: <https://android.googlesource.com/platform/system/memory/lmkd/+/refs/heads/main/lmkd.cpp>
- LMKD 说明: <https://source.android.com/docs/core/perf/lmkd>

## 8. 各类 Problem 的确定性规则契约

### 8.1 Java/Kotlin 未处理异常

候选与最低提交语法:

1. 通过 5.7 来源门槛、唯一匹配 schema 的 `am_crash` EventLog-shaped 记录,且异常类别不是 native crash;或
2. 普通进程:
   - candidate start:`AndroidRuntime` E/F 级别的严格 `FATAL EXCEPTION:`。
   - minimum commit grammar:同一 producer candidate 中同时出现
     `Process: <name>, PID: <pid>`、payload/header PID 一致、至少一个 Throwable 类型。
   - optional: `Caused by:` / `Suppressed:` / `at ...` / `... N more` 等 stack grammar;或
3. system process:
   - candidate start:`AndroidRuntime` E/F 级别的严格
     `*** FATAL EXCEPTION IN SYSTEM PROCESS: <thread>`。
   - minimum commit grammar:exact system envelope + 至少一个 Throwable 类型。
   - optional:stack grammar;不要求普通进程才会输出的 `Process: ..., PID: ...` 行。

普通进程文本规则要求 payload PID 与 AndroidRuntime header PID 一致;冲突时设置 malformed/conflict,不把文本块升级为正式 occurrence。system-process envelope 不要求不存在的 `Process: ..., PID: ...` 行;其 `ProcessInstanceKey` 来自已知 system_server mapping/header,无法确定时使用 unknown identity 并降低 `IdentityQuality`。

边界:

- 从 `candidateStart` 记录 `startLine`;只有满足 `minimumCommitGrammar` 后才升级为
  provisional occurrence。
- 接受同一 AndroidRuntime producer 的标准 continuation。
- 允许状态机跨过少量、受上限约束的交错行;`endLine` 只推进到最后一条已匹配证据行,交错文本不进入 fingerprint。
- 遇到新的 `candidateStart`、明确非 continuation 或安全上限时闭合。

误报控制:

- 应用普通消息中出现 `FATAL EXCEPTION` 文本不够。
- 普通 envelope 缺少 Process/PID 或 Throwable 时不形成 occurrence;system envelope
  只要求其独立 `minimumCommitGrammar`。
- `FATAL EXCEPTION` 本身不自动等价于进程已终止;自定义 uncaught handler 可能改变默认流程。

指纹:

```text
java-v1
+ ProcessFingerprintKey
+ 最内层已报告 Throwable class
+ 前 3 个归一化稳定栈帧
```

归一化去掉源码行号、对象地址、线程编号、动态 ID 和普通消息中的易变数字。

### 8.2 ANR

可提交路径:

1. 通过 5.7 来源门槛、唯一匹配 schema 的 `am_anr` EventLog-shaped 记录;或
2. tag 精确为 `ActivityManager` 的 system_server producer block,包含严格 `ANR in <process>` 和同一 producer block 内的 victim `PID: <pid>`。`Reason:` 可选,header pid 是 producer/system_server pid,不得误当 victim pid。

支持证据:

- InputDispatcher/WindowManager input dispatch timeout
- CPU/load 摘要
- subject/component
- ErrorId/frozen 状态

这些支持证据单独出现时不能形成 ANR occurrence:

- 第三方 `ANR-WatchDog`
- 普通 “timeout” 文本
- 单独 traces 堆栈

ANR 的 `Reason` 是平台记录的触发描述,不是根因。指纹使用:

```text
anr-v1
+ ProcessFingerprintKey
+ component(存在时)
+ reason category
+ 归一化 reason
```

### 8.3 Native crash

可提交路径:

1. 通过 5.7 来源门槛的 `am_crash` EventLog-shaped 记录明确记录 Native crash;或
2. libc 的严格 fatal signal 记录,signal 属于 `SIGABRT/SIGBUS/SIGFPE/SIGILL/SIGSEGV/SIGSTKFLT/SIGSYS/SIGTRAP` allowlist,且可解析 victim identity;或
3. debuggerd/tombstone 的完整起始标记,并在同一候选中同时解析 separator、pid/tid、victim process 和上述 fatal signal。victim process 必须来自严格 `>>> <process> <<<` 或 `Cmdline: <process>` grammar;`name:` 只是 thread name,不能替代 process。crashed-thread/backtrace 是支持证据,不是最低 commit grammar 的必选项。

支持证据:

- abort message
- signal/si_code
- fault address
- crashed thread
- backtrace frames

排除:

- 用户/工具主动请求的 debuggerd backtrace
- Signal Catcher dump
- 只有 tombstone 字样但没有 signal/victim grammar

Android 新版本可能记录 recoverable GWP-ASan/MTE。存在明确 recoverable 标记时设置 `EXPLICITLY_RECOVERABLE`,不能自动声明进程死亡;若同一范围又观察到 death,两项事实都保留并设置 `CONFLICT`,不替用户解释。

libc signal-only 规则若只能得到 pid 而无法从 active `ProcessInstanceTracker` 得到
process,可以形成 `UnknownProcess`/`SignalOnly` 低质量 occurrence,但禁止与
tombstone、`am_crash` 或 death 做跨 Observation 合并。tombstone 规则自身不允许用
unknown process 满足 `minimumCommitGrammar`。

指纹:

```text
native-v1
+ ProcessFingerprintKey
+ signal + si_code
+ crashed thread 前 3 个稳定 frame/module identity
```

symbolized frame 归一化为 `module#symbol`,去掉函数 offset;未符号化 frame 归一化为 `BuildId+module+relativePC`,去掉绝对地址。只有绝对地址且没有 BuildId/module identity 的 frame 不进入稳定指纹。abort category 默认只作为详情 facet,不参与 `native-v1`;未来若经样本证明需要拆组,必须提升 fingerprint version。

### 8.4 进程死亡与快速再次启动

可提交的死亡记录:

- 通过 5.7 来源门槛的 `am_proc_died` EventLog-shaped 记录
- system_server 的严格 `Process <name> (pid <pid>) has died`
- 可映射到已知进程实例的 zygote exit

可提交的启动记录:

- 通过 5.7 来源门槛的 `am_proc_start` EventLog-shaped 记录
- system_server 的严格 `Start proc`

排除:

- `am_kill`:杀进程请求,不是死亡事实
- schedule/restart plan:计划,不是已重启
- 仅 PID 相同但进程身份或 epoch 不一致

start-after-death 是派生事实关系:

```text
同一历史 ProcessInstanceKey
+ death 后出现明确 start
+ 中间没有矛盾的实例切换
```

UID + process name 必须匹配;两侧都有 user 时 user 也必须匹配。某 schema 没有 user 时,只能借助当时 active/historical UID+process mapping 关联,不得退化为 name-only。仅 process name 相同只能展示两条相邻生命周期记录。间隔 ≤30 秒时 UI 标为“快速再次启动”,更长间隔只保留普通生命周期关系。界面文案只能写“进程结束后 12.4 秒再次启动”,不能写“崩溃导致自动重启”。

当死亡已经作为 crash/OOM/LMK occurrence 的结局佐证时,不再产生重复的 standalone death occurrence;仍在详情中保留独立事实和源行。普通 `am_proc_died` 可能只是正常回收,默认只保留为内部生命周期 observation,不进入 Problems。只有严格 start-after-death 或明确 signal exit 才产生独立生命周期 occurrence。

生命周期指纹:

```text
lifecycle-v1
+ ProcessFingerprintKey
+ relation class(start-after-death 或 signal-exit)
+ explicit signal(存在时)
```

pid、epoch 和 death→start 时间间隔不参与分组。

### 8.5 Java OOM 与 kernel OOM

Java OOM:

- 未处理 OOME 出现在已验证的 Java fatal/am_crash 事件中时,产生一个 `JavaOom` occurrence,并包含 Java crash facet,不再额外计一个 JavaCrash。
- ART `Throwing OutOfMemoryError`、Dalvik `Out of memory on a ... allocation` 只说明 OOME 被抛出;它可能被捕获。MVP 默认不把它放进 Problems 主列表。
- GC、trim、`am_low_memory`、free-memory 摘要不能单独形成 OOM Problem。
- 不允许根据 OOME 自动生成“内存泄漏”事实。

kernel OOM:

- 仅在输入明确包含 kernel 来源且匹配严格 OOM killer grammar、victim pid/name 时产生 `KernelOomKill`。
- 必须与 userspace LMK 分开。
- 缺少 kernel buffer 时 coverage 明确说明该类无法完整检测。
- 指纹为 `oom-kernel-v1 + ProcessFingerprintKey + global/memcg mechanism`;动态 memory counters、pid 和 constraint 数值不参与。

Java OOM 指纹复用 Java 栈结构并增加 `oom-java-v1` domain;分配字节数不参与指纹。

### 8.6 LMK/lmkd

可提交路径:

- userspace tag 精确为 `lowmemorykiller`,并匹配 lmkd 成功记录 `Kill '<name>' (<pid>) ... to free ...; reason: ...`
- 旧内核明确 `lowmemorykiller: Killing ...`

排除:

- select/candidate-to-kill
- `Skipping kill`
- pressure/psi 摘要
- ActivityManager `Killing`
- `am_kill`
- `am_low_memory`
- kernel OOM killer

lmkd 最低提交语法成立时设置 `KILL_ISSUED`;只有随后同一进程实例的死亡事实才能再设置
`DEATH_OBSERVED`。

指纹:

```text
lmk-v1
+ ProcessFingerprintKey
+ userspace/kernel mechanism
+ 归一化 policy reason
```

oom_score_adj、释放字节数和动态内存值不参与指纹。

## 9. 多行边界、关联与仲裁

### 9.1 稳定完整行前沿是 Gate 0

现有 Indexer 在看到行首后即可增加 `total_lines`;索引预算或实时 stdout 块可能停在一行中间。Problems 不得直接把 `total_lines` 当作“可最终分析的完整行数”。

Indexer 需要显式跟踪 `completed_lines`,即已经见到行尾换行的行数:

- 增长中的输入只允许扫描 `completed_lines`。
- 静态文件索引真正到达 EOF 后,最后一个无换行行可提交一次。
- growing 输入有显式 `Growing → Paused → Growing` 或 `Growing/Paused → Sealed` 生命周期。
- Pause 可恢复,不视为 EOF,不得闭合末尾半行或 pending candidate。
- Stop 等 stdout EOF 后进入 `Sealed`,完成最后一行及 pending candidate;sealed 输入禁止 Resume。
- Stop 后再次开始采集会建立新的 capture/session,不能向已经封口的字节输入续写。`resume_logcat` 只接受 runtime 确实处于 paused 的状态。
- 文件截断/轮转时 stable frontier、Problems、filter/search/errors 增量游标全部归零重建。

Gate 0 同时补齐现有实时 filter/search/errors 对跨读块尾行的回归测试,避免只为 Problems 新建一套正确性语义。

### 9.2 状态机跨块

- 4096 行扫描批次结束不能闭合事件。
- mmap remap 后不能保留任何 `&str`/slice 借用。
- pending state 只保存数值、固定大小 hash state、intern id 和有严格长度上限的摘要。
- 新的 `candidateStart` 可以闭合旧候选并立即开始新候选。
- 没有 logcat prefix 的 raw continuation 只有在**恰好一个** pending candidate 与 producer/schema/boundary 兼容时才可附着;多个候选都兼容时标记 ambiguous 并忽略该行。

### 9.3 安全上限

初始 RuleContract 上限:

| kind | maxLines | maxBytes | unmatchedBudget |
|---|---:|---:|---:|
| Java/Kotlin | 512 | 128KiB | 32 |
| ANR | 512 | 256KiB | 16 |
| Native/tombstone | 4096 | 2MiB | 16 |
| Kernel OOM episode | 512 | 512KiB | 16 |
| Legacy kernel LMK | 32 | 64KiB | 8 |
| 单行 EventLog/lmkd | 1 | 单行硬上限 | 0 |

此外冻结以下全局防御上限:

- 单个物理日志行最多 1MiB;超出部分不进入 recognizer,并计入 malformed/limited 统计。
- 单候选最多 4096 行/4MiB。
- 同时 active pending candidates 最多 64 个,其固定摘要/状态合计最多 8MiB。
- active PID mapping 最多 65,536 项,recent terminated identity 最多 4,096 项;进程死亡后释放 active PID 槽,事件仍引用 compact intern id。
- 超限时按 `(lastTouchedLine,candidateId)` 确定性淘汰最旧 incomplete candidate,不得依赖 HashMap 遍历顺序。

达到边界上限时,只有候选已经满足该规则的 `minimumCommitGrammar`
才提交 occurrence 并设置 `truncatedByLimit`;尚未满足最低提交语法的候选必须丢弃并增加
malformed/truncated-candidate 计数,不能因超限反而升级为 Problem。具体 recognizer
可以使用更小的结束条件,但不能突破全局硬上限。上述初始值必须经过 fixtures
和事件风暴压测;调整会变更 RuleContract version。

### 9.4 确定性关联

只有满足规则的 Observation 才能进入同一 occurrence:

- 同一 process instance。
- 行序与时间顺序一致。
- 处于 kind-specific 关联窗口。
- 没有矛盾的新 start/PID identity。
- 至少有一项证据满足该规则的 `minimumCommitGrammar`。

MVP 冻结以下窗口;距离按 physical source lines 计算,时间戳只作附加约束。只有两侧属于同一可比较的 timestamp segment 时才应用秒数门槛;缺少年份、跨年、时钟回拨或格式不一致导致歧义时视为“时间不可比较”,不得仅据此拒绝 line/identity 已唯一匹配的关系:

| relation | 必选 identity | 行窗口 | 时间附加约束 |
|---|---|---:|---|
| Java FATAL ↔ managed `am_crash` | 同一 `ProcessInstanceKey` | 双向 512 | 同 timestamp segment 时绝对差 ≤60s |
| ActivityManager ANR block ↔ `am_anr` | 同一 victim instance | 双向 512 | 同 timestamp segment 时绝对差 ≤60s |
| libc ↔ tombstone ↔ native `am_crash` | 同一 victim instance | 双向 4096 | 同 timestamp segment 时绝对差 ≤60s |
| crash/OOM/LMK ↔ death | 同一 instance,death 在后 | 4096 | 同 timestamp segment 时差值 0–60s |
| `am_kill` ↔ provisional fault/lifecycle occurrence | 同一 instance | 双向 4096 | 同 timestamp segment 时绝对差 ≤60s |
| death → start | 5.5/8.4 的历史身份规则 | 下一个未冲突的 matching start | 无成立阈值;≤30s 仅控制 UI 突出 |

窗口内只采用最近且唯一的一对一匹配;若两个候选距离相同、identity 不完整或同一 Observation 会被多个 candidate 竞争,则不自动合并并设置 ambiguity 统计。时间接近本身不构成因果。关联只改变“同一次事件的证据集合”和 OutcomeFlags,不生成隐藏的原因字段。

`am_kill` 自身只是一项内部 `KILL_REQUESTED` observation,不独立创建 Problem。仅当窗口内
已有由其他 `minimumCommitGrammar` 建立的 provisional occurrence 时才附加该
fact/flag;它永不设置 `KILL_ISSUED` 或 `DEATH_OBSERVED`。已定稿 event 不回写。

### 9.5 Late correlation 与不可变 side table

Java/ANR/native 主体闭合后,`am_*` 或 death 仍可能在 512/4096 行窗口内晚到。为保持 `observationStart + observationLen` 连续且避免事件更新时搬移 arena,MVP 采用延迟定稿:

1. recognizer 满足 minimum commit grammar 后创建 `ProvisionalOccurrence`,在固定
   `[ObservationRef; 8]` 中保存关键 refs 和 `observationTotal`,暂不进入可查询 index/group。
2. correlation engine 保留最多 16,384 项、最多 256KiB 逻辑 payload 的 recent
   Observation FIFO ring,用于把先到的 EventLog-shaped/death 证据关联到新
   provisional occurrence。每项保存单调 `observationSeq` 和按最大 backward line window
   计算的 expiry;正常情况下只淘汰已经过期的队首。
3. provisional occurrence 接受窗口内后到证据;只有 source-line watermark
   **严格越过其所有双向 relation 的最大 forward line**、输入 finish,或某项
   RuleContract 明确声明 process-close 对该方向是 terminal watermark 时才定稿。
   普通 process death 不是 Java/ANR/native/`am_kill` 双向关系的安全定稿水位:
   多 buffer 排序下,同一实例的 `am_crash`/`am_kill` 可能在 death 后才出现。
   timestamp 超过 60 秒可以拒绝同一可比较 segment 内的单次关联,但不能单独触发不可逆
   定稿,因为之后的时钟回拨/新 segment 会变为时间不可比较。
4. 定稿时只追加一次实际 `observationLen` 个连续 refs 到全局 side table,随后 `ProblemEvent` 不可变并原子加入 group。
5. provisional 同时最多 4,096 个、受 4MiB 逻辑预算约束。超限时按
   `(finalizeAfterLine,eventId)` 定稿最早项并设置
   `correlationTruncatedByLimit`;`minimumCommitGrammar` 已满足,所以这只会漏掉晚到
   supporting/outcome,不会制造 false Problem。
6. recent ring 插入前先清除正常过期项;仍超过数量或字节上限时,按
   `observationSeq` 从旧到新强制淘汰,不得依赖 hash 顺序。每次强制淘汰递增
   `droppedRecentObservationCount`,设置 session 级 `correlationLimited=true/limited=true`;
   已经复制进 provisional 的 refs 不受影响。UI 必须说明“部分晚到关联证据可能未保留”,
   不能把缺失 supporting/outcome 解读为其没有发生。

查询和 observed/stored counters 只统计已定稿 occurrence;进度可另报 `provisionalOccurrenceCount`。事件风暴、block 边界和 finish 必须证明无 ref relocation、无 arena 泄漏、静态/live 最终结果一致。

### 9.6 重复与续抓重放

- 相同 fingerprint 的真实重复必须保留为多个 occurrence。
- 不允许按 fingerprint 全局去重。
- `logcat -T` 可能重放边界毫秒的少量行。MVP 的 count 定义为“当前日志文件中识别到的 occurrence 数”;如果采集层没有消除重放,coverage/帮助文案需说明可能包含续抓边界重复。
- 同一 occurrence 内满足最低提交语法的重叠证据,由 process instance、range overlap
  和 rule relation 折叠,不重复计数。

## 10. 指纹契约

### 10.1 稳定算法

- 使用显式版本和固定算法,不得使用 Rust 默认 HashMap hasher 作为持久/可比较指纹。
- 推荐 `BLAKE3` domain-separated 输入的前 128 bit。
- domain 至少包含 `ProblemKind`、rule/fingerprint version、`SignatureQuality` 和 `IdentityQuality`。
- 归一化 token 逐项写入 hash,不拼接整段堆栈 String。

### 10.2 版本语义

- 规则或归一化变化导致分组结果变化时,必须提升 fingerprint version。
- group key 为 `(kind, fingerprintVersion, signatureQuality, identityQuality, fingerprint128)`。
- DTO 中 fingerprint 为只读十六进制展示值;前端不得自行计算或解释。

### 10.3 聚合不变量

- PID/TID、时间戳、地址、UUID、源码行号和易变数值默认不参与。
- `ProcessFingerprintKey` 默认参与,避免不同应用的同名异常混组;`ProcessInstanceKey`、pid、epoch、user/uid 不参与。
- 同组 occurrence 仍独立保存 start/end/anchor。
- UI 固定展示“同组不等于同一根因”。

### 10.4 SignatureQuality 与 IdentityQuality

签名完整度和进程身份完整度是两个正交维度:

- `SignatureQuality`: `FullStack` / `TypeFile` / `TypeOnly` / `SignalOnly` /
  `StructuredFields` / `Minimal`。前四项用于 crash/OOM 栈签名,后两项用于 ANR、LMK 和 lifecycle 的结构化字段完整度。
- `IdentityQuality`: `KnownProcess` / `UnknownProcess`。

两者都进入 fingerprint domain 和 group key。`FullStack + UnknownProcess` 与 `TypeOnly + KnownProcess` 都能被准确表达,不能用单一枚举混淆。不同质量不跨级合并;例如完整堆栈和仅异常类型即使 canonical 前缀相同也必须拆组。质量表示“分组依据的完整度”,不是根因置信度。

## 11. 后端深模块设计

### 11.1 模块位置

新增:

```text
crates/logcore/src/problems/
├── mod.rs
├── model.rs
├── engine.rs
├── index.rs
├── fingerprint.rs
├── facts.rs
├── provenance.rs
├── process_instance.rs
├── eventlog.rs
├── classifier.rs
└── recognizers/
    ├── java.rs
    ├── java_oom.rs
    ├── anr.rs
    ├── native.rs
    ├── process.rs
    ├── lmk.rs
    └── kernel_oom.rs
```

这是一个深模块:调用方只需按稳定源行顺序推进和执行有界查询;候选分类、多行状态机、边界、仲裁、fingerprint、group 更新和内存限制全部隐藏在实现内。

`logcore` 不依赖 Tauri/UI。recognizer seam 是模块内部 seam,不因为测试而暴露成公共插件接口。

### 11.2 概念 interface

```rust
pub struct ProblemEngine { /* private implementation */ }

impl ProblemEngine {
    pub fn observe(&mut self, line: ObservedLine<'_>) -> ProblemDelta;
    pub fn finish_input(&mut self) -> ProblemDelta;
    pub fn reset(&mut self);

    pub fn stats(&self) -> ProblemStats;
    pub fn begin_group_snapshot(&mut self, query: &GroupQuery) -> SnapshotBuild;
    pub fn group_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> GroupPage;
    pub fn create_occurrence_snapshot(&mut self, group: GroupId) -> QuerySnapshotId;
    pub fn occurrence_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> OccurrencePage;
    pub fn release_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool;
    pub fn event(&self, id: ProblemEventId) -> Option<ProblemEvent>;
    pub fn detail(&self, id: ProblemEventId) -> Option<ProblemDetail>;
}
```

`ObservedLine` 至少包含 source line、借用的原始 bytes/parsed header、session `InputCoverage` 引用和该行 `LineProvenance`;recognizer 不允许仅凭 tag 把 Unknown 改写为 Known。`ProcessInstanceTracker` 在所有 recognizer 之前更新并提供 compact identity,不是 lifecycle recognizer 的私有实现。

`Session` 负责从 mmap/Indexer 取稳定行并调用该模块:

```rust
pub fn scan_problems_step(&mut self, max_lines: usize) -> ProblemScanStep;
pub fn finish_problem_input(&mut self) -> ProblemScanStep;
pub fn problem_stats(&self) -> ProblemStats;
pub fn begin_problem_group_snapshot(&mut self, query: &GroupQuery) -> SnapshotBuild;
pub fn create_problem_occurrence_snapshot(&mut self, group: GroupId) -> QuerySnapshotId;
pub fn problem_snapshot_page(
    &mut self,
    snapshot: QuerySnapshotId,
    page: PageSpec,
) -> SnapshotPage;
pub fn release_problem_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool;
pub fn problem_event(&self, id: ProblemEventId) -> Option<ProblemEvent>;
pub fn problem_detail(&self, id: ProblemEventId) -> Option<ProblemDetail>;
```

调用方不传任意 start/end 扫描范围;模块自己维护单调扫描游标,隐藏顺序不变量。

snapshot 构建不能在全局 Session mutex 内排序/复制百万 ID:

- occurrence source-order snapshot 只保存 `{groupId,frozenLen/maxEventId}`,分页直接读取该组已 append 的前缀,不复制 occurrence ID vector。
- group snapshot 用最多 4,096 个 group summary/step 的短锁分批捕获固定 `maxGroupId` 内的紧凑 sort records,锁外排序,再以相同 analysis token 安装 ID vector。
- TTL/LRU access 和 release 都是显式 `&mut self` 操作;不依赖隐藏 interior mutability。
- 最大 100,000 groups/1,000,000 occurrences 下的首次构建、翻页和 release 都必须纳入 ≤20ms lock gate。

## 12. 紧凑事件索引

### 12.1 occurrence

逻辑字段:

```text
startLine
endLine
anchorLine
anchorTimestamp
kind
pid
processInstanceId
groupId
observationStart
observationLen
observationTotal
rule/evidence flags
outcome/boundary flags
```

物理约束:

- MVP 内部行号沿用 0-based `u32`,符合现有 10GiB/7115 万行基线。
- 行号超过 `u32::MAX` 时进入明确 `lineIndexOverflow/limited` 状态,不得静默丢事件。
- occurrence 不含 `String`、`Vec`、原始文本或 mmap 借用。
- `size_of::<ProblemEvent>()` 目标不超过 48 bytes。
- fingerprint 只在 group 表保存一次,occurrence 使用紧凑 `groupId`。
- `ProblemEventId` 是当前 session/generation 内的 opaque id,不得跨会话持久化或由前端解析。
- `anchorTimestamp` 是可选 packed 64-bit log timestamp;没有可解析时间时 UI 回退显示 source line,不得为展示时间随机读取海量原文。

每个 occurrence 通过全局紧凑 side table 引用原子证据:

```rust
#[repr(C)]
pub struct ObservationRef {
    line: u32,
    rule: u16,
    role_and_format: u8,
    source_and_provenance: u8,
}
```

- `size_of::<ObservationRef>() == 8 bytes`。
- occurrence 保存 `observationStart: u32`、`observationLen: u8` 和
  `observationTotal: u16`,每个 occurrence 最多物化 8 项。
- adopted observation 先按 `(line,rule,role)` 去重,每个 event 的确定性硬上限为
  4,096 项;计数使用 checked `u16`,禁止饱和或回绕。超过上限的证据不再采用并设置
  `observationCountLimited`;event range 只覆盖已经采用的证据。
- 物化优先级固定为 minimum-grammar primary evidence → outcome/death/restart →
  correlation evidence → supporting evidence;超出 8 项设置
  `observationRefsTruncated`,但 event range 仍覆盖全部已采用证据。
- `FactCode` 不重复塞入 8-byte ref;由 versioned total mapping
  `(RuleId,ObservationRole) -> FactCode` 唯一导出。任何无法映射的组合是 detector bug,不得以 unknown 文案掩盖。
- `get_problem_detail` 只把这些 refs 转成有界 `FactCode/RuleId/sourceLine/role/evidenceFormat/provenance`,并返回 `factsTruncated/observationTotal`;不返回原文或后端自然语言原因。
- 前端按 `FactCode` 做穷尽的本地化文案映射;每项事实可单独定位其 source line。
- refs 截断时 UI 固定显示“仅展示 8/M 条关键证据,可查看事件范围”,不能静默隐藏。

### 12.2 group

group 保存:

- fingerprint/version/kind/两种 quality
- observed/stored/dropped occurrence count
- `first/lastObservedLine + first/lastObservedTimestamp`,即使 occurrence 因容量未存储也可更新
- `first/last/representativeStoredEventId`;representative 必须可查询
- process/signature intern id
- 必要的、有长度上限的代表性摘要

每组 occurrence id 单调 append,不在每次追加后全量排序。用于展示排序的索引按 revision 惰性重建。

“最近发生”排序使用 last event 的 source line,不使用可能回拨/跨年的墙钟时间;首次/最后时间只是对 source-order first/last event 的展示属性。

`GroupId` 对前端同样是不透明值;稳定分组语义由 `(kind,fingerprintVersion,signatureQuality,identityQuality,fingerprint128)` 决定,不是可变列表下标。

### 12.3 有界容量与内存目标

默认结构性硬限制:

- 最多存储 1,000,000 个 occurrence。
- 最多存储 4,000,000 个 `ObservationRef`。
- 最多存储 100,000 个 group。
- query snapshots 同时最多 8 个、TTL 5 分钟、ID vectors 合计最多 16MiB。
- intern strings 合计最多 8MiB,单字段有固定长度上限。
- pending/process tracker 受 9.3 的条数和容量限制。
- 所有受控 Vec/Map 在 reserve 前通过统一 `ProblemMemoryBudget` 按 capacity 和保守 overhead 估算并收费;逻辑 payload 预算 112MiB,给 allocator/hash metadata 留出余量。
- 受控 retained heap 的 benchmark 目标为 ≤128MiB;不把未测量的进程 RSS 宣称为事件索引大小。
- 代表性摘要单字段有固定长度上限。

达到限制后:

- `limited = true`
- recent Observation ring 被迫淘汰时另设 `correlationLimited=true` 并精确累计
  `droppedRecentObservationCount`;这表示 supporting/outcome 关联可能不完整,不是已存 occurrence
  被删除。
- `observedOccurrenceCount` 继续饱和计数,`storedOccurrenceCount` 与 `droppedOccurrenceCount` 分开。
- group 公开 `observed/stored/droppedOccurrenceCount`;代表 occurrence 必须是 stored event。
- 已存在 group 的 observed first/last/count 在不新增容量时继续维护;stored event ids 不指向 dropped occurrence。
- 无法创建新 group 时不声称知道 distinct group 数;每个这样的 occurrence 只增加精确定义的 `ungroupedDroppedOccurrenceCount`。
- append 前原子预留 event、全部 refs、group membership、intern 和必要索引容量;任一预留失败则整个 occurrence dropped,禁止半 event/半 refs/空 group。
- 停止无界分配
- UI 显示“事件索引达到上限,结果不完整”
- occurrence 列表和导出只覆盖 stored events;UI 在 limited 状态明确显示“检出 N,已保存 M,丢弃 D”,不能把 observed count 冒充可展开条数。

## 13. 增量处理与并发

### 13.1 静态文件

现有索引 worker 交替执行两个短临界区:

```text
lock_session_if_current
  → index_step(8MiB)
unlock

lock_session_if_current
  → scan_problems_step(最多 4096 行)
unlock
emit progress / yield
```

Indexer 到 EOF 时先 seal static source,使最后一个无换行行恰好成为 stable line;随后
Problems 追上最终 stable frontier,最后调用一次 `finish_problem_input()` 闭合已满足
`minimumCommitGrammar` 的 pending candidate。

完成状态机固定为:

```text
Indexing
  → SealStaticSource(仅一次,确定最终 stableLineCount)
  → CatchingUpProblems(scannedLine < stableLineCount 时循环 4096 行批次)
  → FinishPending(仅一次)
  → Finished
```

每个 catch-up 批次都重新校验 generation 并 yield。`done=true` 只能在 Problems 已追上最终 stable frontier 且 `finish_problem_input()` 完成之后发送;不能因为 Indexer 到 EOF 就提前宣布完成。

### 13.2 实时抓取

- stdout 仍写入会话文件,remap 后推进 Indexer。
- 每个读块只扫描新增 completed lines。
- pause 进入 `Paused`,保留 pending state;只有 Paused 可 resume。
- resume 回到 `Growing`,继续从已有扫描游标消费新增稳定行。
- stop 且 stdout EOF 后进入 `Sealed` 并 finish;sealed session 不可 resume。
- Stop 后重新开始采集创建新 session/generation,不能续写 sealed input。运行中 Clear
  会先受控终止并 join reader,清空后用相同抓取请求和尾时间戳自动续抓；它复用文件路径
  配置,但必须创建新 session/generation,不得恢复旧输入的扫描身份。非运行状态 Clear
  保持 Stopped。
- clear/truncation 重置 Problems 与全部派生游标。
- live 启动时把 `requestedBuffers` 作为 `InputCoverage` 传入 Session;若 transport 没有逐行 buffer identity,不得凭请求集合给每行标 `Known`。

Pause/Resume/Stop/Start 由专用 `stream_control` mutex 串行化完整转换,不能让
`StreamRuntime.paused` 与 Session input lifecycle 各自演进。锁序固定:

1. 持 control lock 使旧 reader `streamGeneration` 失效并取走 task。
2. 不持 Session/StreamRuntime lock 时 kill/join,确认 stdout EOF。
3. 以匹配的 session generation 更新 Session 为 Paused 或 Sealed。
4. 最后发布 StreamRuntime 状态和事件。

并发控制失败进入明确 `ControlError`,不把未确认 EOF 的输入伪装为 Sealed;需要 Start 新 session 恢复。禁止在 join 时持全局 Session mutex。

### 13.3 generation

generation 所有权严格分开:

- `sessionGeneration`:输入/Session 被替换时变化。Stop 后 Start、Open/Clear 新输入都会变化。
- `streamGeneration`:只取消 reader/transport task;Pause/Resume 可变化,但**不进入** Problems identity。
- `analysisGeneration`:encoding 或 detector/profile version 改变时变化;Pause/Resume 不变。
- `decodeRevision`:encoding 改变时变化,用于失效前端已解码 row block;profile-only 变化不必失效原文行。

每个分析批次使用
`lock_analysis_if_current(sessionGeneration,analysisGeneration)`;事件在锁外 emit。编码/profile 变化在 Session 锁内递增 analysis generation、reset Problems/pending/query snapshots 并从稳定行 0 重扫,不伪造一次 Session replacement。旧 analysis task 即使仍持有旧 session generation,也不能继续写入。固定 AOSP profile 下可不提供用户可见的手动重扫按钮,但实现仍必须保存 profile/version identity。

### 13.4 进度事件

新增:

```text
problems:progress {
  scannedLines,
  stableLines,
  observedOccurrenceCount,
  storedOccurrenceCount,
  droppedOccurrenceCount,
  provisionalOccurrenceCount,
  storedGroupCount,
  ungroupedDroppedOccurrenceCount,
  droppedRecentObservationCount,
  correlationLimited,
  revision,
  done,
  limited,
  sessionGeneration,
  analysisGeneration
}
```

进度节流发送,不能把 occurrence/group 数组附在事件中。

## 14. 有界 IPC

新增命令:

- `get_problems_status`
- `get_problem_groups`
- `get_problem_occurrences`
- `get_problem_detail`
- `export_problem_logs`
- `release_problem_snapshot`

不新增 Problems 原文接口:

- 上下文继续使用 `get_rows("all", start, count)`,后端维持 `count ≤ 512` 硬上限。
- `export_problem_logs` 只负责 generation-safe eventId→range 解析,实际字节写出复用现有 range exporter。
- 当前过滤内定位复用 `line_to_result_index`。

查询约束:

- group 初始页 100,硬上限 200。
- occurrence 初始页 100,硬上限 200。
- 第一次 group 查询按 11.2 的分批短锁/锁外排序流程 materialize 稳定 group ID vector;occurrence 查询只冻结该组 append prefix 的 `frozenLen/maxEventId`,两者都返回 `querySnapshotId`。
- cursor 对前端不透明,包含 `querySnapshotId + position + query signature`;扫描 revision 增长不改变既有 snapshot。
- query snapshot 同时最多 8 个、TTL 5 分钟、ID vectors 合计最多 16MiB。最旧未使用 snapshot 按 `(lastAccess,snapshotId)` 确定性淘汰;session/analysis generation 变化时全部清空。
- 前端在分类/排序/group 切换或主动刷新时调用 generation-safe `release_problem_snapshot`;异常退出未释放的 snapshot 仍由 TTL/LRU 回收。当前屏幕使用的 group/occurrence snapshot 不得因快速切换产生的已释放旧 snapshot 而被优先淘汰。
- 新事件只更新 summary 和“有新结果”提示;用户主动刷新才创建新 snapshot。
- 所有响应包含 session/analysis generation、Problems revision 和 snapshot id。
- 旧 generation、未知 id、过期 snapshot/cursor 和 limited 状态有确定错误/状态语义;`snapshot-expired` 不得静默回到第一页。
- group/detail DTO 不携带日志正文。

所有 Problems 派生操作都必须携带
`expectedAnalysisToken { sessionGeneration, analysisGeneration }`:

- `get_problem_detail(eventId,expectedAnalysisToken)` 在同一把 Session 锁内解析 refs。
- Session 保存已应用的 `filterResultRevision/requestId`。`get_rows("filtered")` 和
  `line_to_result_index` 必须接收 `expectedFilterResultRevision`;不匹配返回
  `stale-filter-result`,响应回显实际 revision。request nonce 只处理迟到顺序,不能替代 dataset identity。
- 定位使用带 analysis token + expected filter result revision 的 source-line mapping;返回值回显两者。
- 上下文 `get_rows("all",...)` 返回 analysis token + decodeRevision,前端不得把旧 session/encoding rows 放入新 cache。
- 导出使用 `export_problem_logs(eventId,expectedAnalysisToken,mode,radius)` 在同一锁内把 opaque event id 解析成 inclusive range,再复用现有 range export worker。不能由前端先取行号、会话切换后再向另一 Session 发 range。

前端只接收事实枚举和必要摘要。排查提示由独立前端内容目录按 kind 映射,不能由后端拼接自然语言根因。

## 15. 底部 Problems 工作台

### 15.1 页面位置与尺寸

主界面结构:

```text
Toolbar
Main: Minimap + LogTable
Problems dock
StatusBar
```

规则:

- 默认折叠,折叠条显示扫描状态;正常时显示
  `Problems · 检出 <observed count>`,limited 时显示 `检出 N · 可展开 M`。
  `correlationLimited` 另显示警告入口,展开后说明“部分晚到关联证据可能未保留”及
  dropped recent count,不能写成某项结局没有发生。
- 检测到严重事件不自动展开,避免抢夺用户视口。
- 展开默认约 280px,下限 180px;上限为
  `min(45vh, .lf-workbench 实际可用高度 - 160px)`。`.lf-workbench` 包含 Main + Problems dock,不能观察已扣除 dock 的 `.lf-main`,避免反馈振荡。
- 使用 `ResizeObserver` 监听 workbench 容器、工具栏换行和窗口变化,动态钳位 dock 高度。可用高度低于 `160 + 180px` 时临时 layout-collapse dock,保留用户的 open preference;高度恢复后再按 preference 展开,不进入 overlay。
- 用户主动展开状态可跨 session 保留;新 session 清空选择和分页。

### 15.2 信息架构

展开后两栏:

左栏:

- 分类 chips:崩溃、ANR、内存、生命周期
- 排序:最近发生/重复次数
- fingerprint group 虚拟列表
- kind、签名摘要、process、首次/最后时间、count

右栏:

- group 签名/进程摘要、fingerprint 和“同组不等于同一根因”
- occurrence 分页列表
- event id、pid、start/end/anchor
- 定位、上下文和导出动作

当前阶段为配合后续“进一步分析”能力，右侧暂不展示 OutcomeFlags、boundary/coverage、
“检测到的事实”和静态“排查提示”；相关紧凑后端数据与 IPC 契约继续保留。重新引入时
必须仍按“日志事实 / 非结论提示”分区，不能把提示写成自动根因判断。这是相对最初设计
稿的明确产品取舍，不影响事件定位、上下文读取、导出和指纹分组。

面板折叠时只更新 summary badge,不请求 group/occurrence 页。展开后才加载首屏。

### 15.3 增量列表

实时扫描新增事件时:

- 更新折叠 badge 和统计。
- 用户正在阅读时继续使用当前 `querySnapshotId`,不强制重排当前 group/occurrence 列表。
- 显示“有新结果,刷新”。
- 用户刷新后通过 IPC 释放旧 snapshot,按新 revision 创建 snapshot 并重新取第一页。
- snapshot 过期时保留当前已渲染内容,明确显示“结果快照已过期,请刷新”;不能把下一页偷偷拼到新排序。

## 16. 主表作用域与临时上下文

### 16.1 唯一 TableScope

当前 store 的 `view` 与 LogTable 实际硬编码 filtered 语义不一致。实施时以唯一 `tableScope` 驱动:

```text
ResultsScope:
  rowsView = filtered
  rowCount = status.filteredLines

ProblemContextScope:
  rowsView = all
  rowCount = status.stableLines
  eventRange
  contextRange
  returnPoint
```

`tableScope` 是以下行为的唯一来源:

- `get_rows` view
- row count
- row cache key/revision
- 滚动钳位
- minimap 是否可见
- event/anchor 高亮
- source-line 导航、selection、viewport 和 scroll request

revision 必须拆开:

- `filterInputRevision`:用户每次编辑 FilterSpec 时增加,用于判断进入 context 后过滤是否被编辑。
- `filterResultRevision`:由 Session 对已应用 filter request 分配/保存,`filter:done` 与 filtered mapping/rows 共同回显,用于 ResultsScope dataset identity。
- `sourceDataRevision`:All 行序随静态索引或 stream append 稳定增长时增加,只更新 rowCount/尾块状态,不进入历史 block cache identity。
- `decodeRevision`:encoding 改变时增加,用于所有已解码 row block identity。

ResultsScope cache key 包含 session generation + decodeRevision + `filterResultRevision`;ProblemContextScope block cache identity 只包含 session generation + decodeRevision + rows view,明确排除 filter/source revision。完整历史 bytes 不可变,append 只使不足额的尾块失效/补拉,已经完整的可见历史 block 继续复用。后端过滤请求携带 input revision/request id,`filter:done` 回显 applied result revision;旧完成事件不得覆盖较新的输入。

### 16.2 定位

“定位锚点”:

1. 调用 `line_to_result_index(anchorLine)`。
2. 命中当前 filtered 结果时,保持 ResultsScope 并居中滚动。
3. 未命中时,自动进入 ProblemContextScope,显示“事件被当前过滤隐藏,已打开临时未过滤上下文”。

“查看未过滤上下文”无论当前是否命中过滤结果,都进入 ProblemContextScope。

默认 context radius 为前后各 50 行,钳位到 `[1,stableLines]`。事件 range 与 context range 分开;live 尾部未闭合行不进入临时上下文。

所有 source-line 动作统一经过 `navigateToSourceLine(lineNo,reason)`:

- ResultsScope 调用带 generation 的 `line_to_result_index(line,bias)` 后定位 filtered result。Problem anchor 使用 `Exact`;返回 viewport 使用 `Nearest`(距离相同优先 source line 之前)。
- ProblemContextScope 直接使用 All 行序 `lineNo - 1`。
- scope、selection、viewport 和 scroll request 由一个原子 store action 更新。
- search next/previous、行号跳转和 F2/F3 bookmark 在当前 scope 内使用该入口;context 中不会错误消费 filtered result index。
- “追最新”若在 context 中触发,先显式退出 context,完成返回/失效处理后再恢复 tail-follow。

### 16.3 过滤和返回点不变量

- 进入/退出上下文期间不得调用 `setFilter`。
- FilterSpec 不保存进 returnPoint,也不进行“清空后恢复”。
- returnPoint 保存 session/analysis generation、`filterInputRevision`、`filterResultRevision`、原选中 source line、原 viewport source line/result index。
- `LogTable` 从首个可见且已加载 row 更新 viewport source line;暂未加载时回退到上一次有效 source line,不得用 All/Filtered result index 互相猜测。
- 上下文中切换另一个 occurrence 不覆盖第一次进入时的 returnPoint。
- session/generation 改变时立即作废上下文。
- 返回时如果过滤输入和已应用结果都未变化,优先恢复原 result index;过滤已变化则按原始 source line 重新映射。
- 若用户编辑过滤后立即返回而匹配的新 `filter:done` 尚未到达,先进入 ResultsScope 并保存:

  ```ts
  PendingRestore {
    viewportLine: number | null;
    selectedLine: number | null;
    filterInputRevision: number;
    requestNonce: number;
  }
  ```

  `PendingRestore` 始终追随**最新**过滤输入:等待 R1 时若用户又编辑出 R2,保留 source-line
  恢复目标,把 `filterInputRevision` 替换为 R2 并递增 `requestNonce`;R1 的完成/映射结果全部
  丢弃。只有 R2 成为已应用结果后,viewport 用 `Nearest` 映射;selected line 只用
  `Exact`,不可见则清除 selection。后续 R3 依此替换,不会同时保留多个 pending。
  用户发起新的显式导航时,新的导航意图取代 pending restore。filter 失败/取消时清除
  pending、显示错误并按最后一次已应用结果安全恢复 viewport。
- `navigateToSourceLine` 遇到 `filterInputRevision` 尚未应用时排队等待匹配的 `filter:done`,显示“正在应用最新过滤…”,不得默默按旧过滤结果定位。
- context 中收到任何 `filter:done` 只能更新后台 Results cache/revision,不能钳位 context 的 viewport、selection 或 rowCount。
- 所有异步映射带 session/analysis generation 和 request nonce;scope/session 已变化的迟到响应丢弃。
- 进入 Problems 定位时暂停 tail-follow;返回后不自动恢复,由用户主动“追最新”。
- 固定 banner 文案:“当前过滤保持,但暂不应用于此上下文”。StatusBar 显示“临时未过滤上下文”,不继续显示 filtered“当前结果”语义。

### 16.4 Minimap

ProblemContextScope 使用 All 行序,现有 minimap 是 filtered 语义。上下文中:

- 不请求/不显示 filtered minimap 数据。
- 可保留同宽 presentation rail 避免表格横向跳动,但不能保留可聚焦空 button;使用 `aria-hidden` 且不可交互。
- 返回 ResultsScope 后重新加载 filtered minimap。

## 17. 导出

详情提供:

- 事件范围(含区间内原始日志):`startLine..endLine`
- 事件上下文:`max(1,startLine-50)..min(stableLines,endLine+50)`

复用现有 ExportDialog 和 range 导出:

- 前端只传 `eventId + expectedAnalysisToken + mode/radius`;后端在同一 Session 锁内解析 inclusive range,避免切换文件后的 TOCTOU。
- 初始范围由 Problems 预填并再次钳位/验证。
- 原始字节和原始行尾保持。
- 不把 kind、facts、hints、fingerprint 插入日志文本。
- “事件范围”仍会包含 start/end 之间交错的原始日志;真正多段 evidence-only 导出和后续“调查证据包”另行设计 metadata/JSON,不污染 `.log`。

## 18. 可访问性

- 折叠按钮使用原生 button、`aria-expanded`、`aria-controls`。
- 面板为有名称的 region。
- 分类 chips 使用 `aria-pressed`,不能只靠颜色。
- group/occurrence 列表采用 container-focus 模式:可聚焦容器使用
  `role="listbox"` + `aria-activedescendant`;行是不可聚焦的 `role="option"`,不能同时嵌套
  或伪装成原生 button。Enter/Space 触发行的主操作,导出等次级动作放在详情操作区。
- 高度拖拽柄使用 horizontal separator 语义,提供 `aria-valuemin/max/now`;方向键每次 16px、PageUp/PageDown 每次 64px、Home/End 到动态边界。
- ProblemKind、指纹说明、操作入口和选中态均不能只依赖颜色。
- 上下文进入/返回通过 polite live region 宣告,不强行抢走日志表焦点。扫描 progress 不逐批宣告,只宣告完成、limited、错误和“有新结果”。
- 返回后恢复到触发 occurrence;虚拟项已卸载时回到面板标题/toggle。
- Problems 与 Toolbar 共用单一 ExportDialog owner。Dialog 具有 initial focus、focus trap、Escape 关闭和关闭后 focus restore;触发 occurrence 已卸载时回到面板标题/toggle。
- group/occurrence 虚拟列表使用 focus 保留在 container 的 `aria-activedescendant`
  模式。ArrowUp/Down 更新 active index 并调用 virtualizer `scrollToIndex`;到已加载末端时先取
  下一页再移动。option 提供稳定 id、`aria-posinset/aria-setsize`;group 列表的
  `aria-setsize` 取冻结 snapshot 的 queryable/stored group 数,occurrence 列表取该组冻结时的
  stored occurrence 数,不得使用包含 dropped 的 observed count。Enter/Space 选择。
  snapshot 过期时 focus 留在 listbox 并转向可见刷新动作。
- 仅键盘可完成展开、分组选择、occurrence 选择、定位、上下文、返回和导出。

## 19. 测试样本与验证策略

### 19.1 样本存放

建议新增:

```text
crates/logcore/tests/fixtures/problems/
├── java/
├── anr/
├── native/
├── lifecycle/
├── memory/
├── mixed/
└── negative/
```

所有样本必须是合成或脱敏日志,进程名使用 `com.example.*`,路径使用示例路径,不得包含真实设备标识、姓名、内网地址或业务数据。

每份输入配一份 golden expectation,固定:

- kind
- start/end/anchor
- pid/`ProcessInstanceKey`/`ProcessFingerprintKey`
- 每个 `ObservationRef` 的 fact/rule/line/role/format/provenance
- evidence/outcome/boundary flags
- fingerprint version/value
- SignatureQuality/IdentityQuality
- group/count

### 19.2 每类最低矩阵

每类至少:

- 10 个确定正例。
- 10 个高相似负例。
- 缺失关键 marker 的不完整样本。
- time 与 threadtime。
- raw continuation 与每行带 logcat prefix。
- 多个 AOSP 版本文案族。
- 同组与应拆组的 fingerprint 对。
- 与其他 Problem 相邻或交错。

重点负例:

- main-only capture 中应用主动打印 EventLog/kernel 关键词
- 只有 `FATAL EXCEPTION` 而缺少 `minimumCommitGrammar`
- ANR-WatchDog
- caught OOME
- debuggerd 主动 dump
- `am_kill` / restart plan
- lmkd select/skip/pressure
- GC/trim/`am_low_memory`
- kernel OOM 与 LMK 混淆

以下是**正 Observation/occurrence,但某项结局的负例**,不能误列为整体检测负例:

- 完整 custom-handler FATAL 仍是未处理异常 observation,但 outcome 必须为“结局未知”,不能自动设置 death。
- recoverable native 仍是 NativeCrash occurrence 并设置 `EXPLICITLY_RECOVERABLE`,但不能自动设置 death。

### 19.3 增量与性质测试

- 在输入每个可能字节位置切块,最终结果与一次性扫描逐字段相同。
- 1 byte、短行、4096 行、8MiB 等扫描预算。
- 事件跨索引块、stdout 块和 CRLF 边界。
- 静态最后一行无换行。
- live 尾行先不完整、后补齐换行。
- pause/resume、stop→sealed、sealed 后拒绝 resume、clear、truncate/rotate。
- PID 重用和同毫秒交错。
- malformed/invalid UTF-8 不 panic。
- 单行上限、pending event 行/字节上限、64+ interleaved candidate storm 和 process tracker 淘汰。
- 超限时“已满足 `minimumCommitGrammar` 才可 truncated commit”,否则零 Problem。
- raw continuation 同时兼容两个候选时不猜关联。
- main-only 伪造 `am_crash`、未知来源 kernel-shaped 文本和 source span 误判。
- full-stack/type-only、known/unknown process quality 不跨级合并。
- merged Java FATAL + `am_crash` + death 的三项事实各自可定位。
- late correlation 在定稿前追加 refs,定稿后 arena 不 relocate;覆盖
  death-before-late-`am_crash`/`am_kill`、provisional 水位/容量/finish,证明 process
  close 不会破坏双向 relation。
- recent Observation ring 覆盖正常 expiry、硬上限 FIFO 淘汰、
  `droppedRecentObservationCount` 和 `correlationLimited` 可见语义。
- 关联边界覆盖 511/512/513、4095/4096/4097 行和 59/60/61 秒,以及等距竞争。
- `(RuleId,ObservationRole) → FactCode` 映射穷尽;refs 超过 8 项时 DTO/UI 明示 8/M;
  adopted evidence 覆盖 8/9/255/256/4096/4097 边界且 `u16` 计数不回绕。
- snapshot 第一页后 revision 增长,第二页无重复、无遗漏、不重排;TTL/容量淘汰返回明确错误。
- 编码在扫描中切换时旧 generation 不再更新,新 analysis 从行 0 重扫。
- fingerprint golden 与归一化性质。

前端组件测试固定使用 Vitest + jsdom + Testing Library + user-event,覆盖 TableScope、dock 虚拟分页、Dialog 和键盘交互;不能把本节验收退化为手工截图。至少覆盖:

- context 中 `filter:done` 不改变 All rowCount/selection/viewport。
- 查看历史 context 时连续 append 不重拉完整可见历史 blocks,只按需刷新不足额尾块。
- context 中编辑过滤并立即返回,匹配完成事件到达后最终定位正确。
- search/行号/bookmark/follow-latest 只走 `navigateToSourceLine`。
- 迟到的旧 session/generation/nonce 响应被丢弃。
- minimap hidden rail 不可聚焦且零数据请求。
- 事件摘要/操作/指纹的固定呈现、固定空态文案和 Dialog focus restore。

## 20. 性能与内存目标

以 `docs/superpowers/2026-07-06-benchmark-10gb.md` 的同一机器、同一 10GiB/7115 万行 corpus 为基准。

硬验收:

- Problems 单独扫描吞吐不低于 5.0M 行/s。
- index + Problems 完成总时间不超过 37s,不超过 20.6s index-only 基线的 1.8 倍。
- 扫描期间 `get_rows(200)` p99 ≤5ms。
- 单次 Problems 锁临界区最大 ≤20ms。
- 原索引锁停顿最大仍 ≤50ms。
- 正常稀疏 corpus 的 Problems 内存只随 event/group 数增长,不随文件字节数增长。
- adversarial corpus 受结构性数量/逻辑预算限制并报告 `limited=true`;受控 retained heap 目标 ≤128MiB。

口径固定为同一机器、同一 10GiB/7115 万行 corpus、release build;冷缓存与暖缓存分别记录 3 次,硬门槛取同口径中位数。并发 `get_rows(200)` 采样贯穿 Indexing、CatchingUpProblems 和 FinishPending,不能只测扫描结束后。

优化目标:

- index + Problems ≤25.8s,即 index-only 的 1.25 倍。
- `get_rows(200)` p99 继续接近现有 2ms 以内。
- occurrence 平均物理存储 32–40 bytes。

性能实现完成后必须把前后数据写入 `docs/superpowers/` 的独立报告。

## 21. MVP 验收标准

1. golden corpus 的 kind、range、anchor、evidence、outcome、fingerprint、group 完全一致。
2. 高相似负例 corpus 零误报。
3. 静态扫描与随机 live chunking 在最终 seal/finish 后逐字段一致;Stop 后不能 resume sealed input。
4. 任何 event/group 中不保存原始日志正文或无界 String。
5. 当前过滤在进入、切换、退出 ProblemContext 前后完全不变。
6. 当前过滤可见的 anchor 直接定位;不可见的 anchor 自动打开未过滤上下文。
7. 上下文仍以 200 行窗口读取,任何 `get_rows` 请求不超过 512。
8. 返回原结果恢复视口/选择,session 变化时安全失效。
9. 事件范围和上下文导出与源文件对应范围逐字节一致。
10. requested buffer、逐行 provenance 未知、输入截断、规则限制和索引上限均在 UI 可见,事实文案不夸大来源。
11. 10GiB 性能、内存和锁停顿达到硬验收。
12. merged occurrence 的每项事实都有有界 ObservationRef 和独立定位行。
13. query snapshot 在扫描 revision 增长时仍稳定分页;跨 Session 操作无 TOCTOU。
14. 完整验证命令全绿,并由独立审查者检查事实语义、并发、内存和 IPC。

## 22. 分阶段路线

### MVP

- stable complete-line frontier
- `aosp-v1` 六类事实检测
- 紧凑 occurrence/group 索引
- 有界分页 IPC
- 默认折叠底部工作台
- 定位、临时上下文、返回点
- 原始范围导出
- coverage/limited/truncated 状态
- 10GiB 基准

### 后续阶段

- 有样本与负例支撑的 Android/OEM 版本规则包
- Problems sidecar 缓存
- bugreport/tombstone/ANR traces 专用导入
- R8/native 符号化
- 调查证据包
- 用户主动触发、严格隔离于事实层的辅助分析
- 全页事件时间线

## 23. 与现有设计的偏差

1. 当前 Errors 是 E/F 行视图;Problems 新增独立底部工作台,不替换 Errors。
2. 原主界面设计稿没有故障工作台;本设计在主表与 StatusBar 之间增加默认折叠 dock。
3. 当前 LogTable 固定读取 filtered;本设计要求以唯一 TableScope 支持临时 All 上下文。
4. 当前 minimap 固定使用 filtered 结果;临时 All 上下文中隐藏,避免语义错误。
5. 当前没有事件分页、规则 coverage 或事实/提示分区;本设计为新增能力。
6. 先前合成数据原型仅验证底部工作台的信息架构与交互,不作为 DTO、规则或后端实现契约;本设计文档是实施依据。

以上偏差均服务于已确认的“快速理解 10GB Android 日志中发生了什么”目标,并继续遵守主规范的窗口化与引擎/UI 解耦要求。
