# Problems 工作台 10GiB 基准报告（2026-07-26）

本报告验证故障调查工作台在 10GiB 日志上的增量扫描、锁竞争、窗口读取和受控内存。
结论是：**production 交错调度的总时长、索引/Problems 短锁、事件 oracle 与内存目标通过；
随机跨文件窗口 p99 和脱离索引热页的 standalone 重扫吞吐未通过。** 因此本轮不能宣称
Problems 的全部性能硬门槛已经验收完成。

## 环境与口径

- 环境：macOS 开发机（Apple Silicon），release 构建，单 Session/单分析 worker。
- 工具：`crates/logcore/examples/bench.rs`，corpus 格式签名
  `format=v2 corpus=sparse|storm`。
- 稀疏语料：10.00GiB / 71,158,147 行；每约 100 万行一组四行 Java crash，另有一组
  首行恰止于 byte 8MiB、其余证据跨入下一片的 crash。
- production 调度：每个 1MiB index 短锁后，最多 32 个独立 Problems step；每个 step
  最多 4096 个物理行/128 条详细行。UI 的 `index:progress` 仍按 8MiB 节流。
- 窗口采样：扫描期间以 200–400ms 抖动间隔发出最多 100 次随机
  `get_rows(All, 200)`，不暂停 worker、不补扫描完成后的空闲样本。
- production 中的 “Problems core-call” 只用于热页 CPU 诊断，**不作为 standalone
  ≥5M 行/秒的替代值**。standalone 使用先完整索引、再连续 Problems 扫描的 sequential
  调度，墙钟包含锁等待、yield、窗口竞争和实际 mmap 缺页。

复跑命令（示例路径不含开发机信息）：

```sh
cargo run --release -p logcore --example bench -- \
  10 /tmp/logfilter-problems-10gb.log \
  --problems-only --assert-contracts --schedule=production

cargo run --release -p logcore --example bench -- \
  10 /tmp/logfilter-problems-10gb.log \
  --problems-only --assert-contracts --schedule=sequential

cargo run --release -p logcore --example bench -- \
  0.015 /tmp/logfilter-problems-storm.log \
  --problems-only --assert-contracts --corpus=storm
```

## Production 交错调度

同一最终代码、同一 v2 corpus 连续测量三次：

| 指标 | Run 1 | Run 2 | Run 3 | 中位数 | 门槛 |
|---|---:|---:|---:|---:|---:|
| index 耗时 | 22.39s | 24.15s | 23.92s | **23.92s** | 诊断 |
| index 最大锁段 | 26.97ms | 19.34ms | 25.66ms | **25.66ms** | ≤50ms ✅ |
| Problems core-call 热页吞吐 | 8.2M/s | 7.9M/s | 8.2M/s | **8.2M/s** | 非 standalone |
| index + Problems 墙钟 | 31.21s | 33.30s | 32.78s | **32.78s** | ≤37s ✅ |
| Problems 最大锁段 | 2.80ms | 2.82ms | 3.00ms | **2.82ms** | ≤20ms ✅ |
| 窗口 median | 3.432ms | 3.759ms | 5.080ms | **3.759ms** | 诊断 |
| 窗口 p99 | 14.720ms | 28.399ms | 55.500ms | **28.399ms** | ≤5ms ❌ |
| 窗口 max | 16.885ms | 33.826ms | 82.071ms | **33.826ms** | 诊断 |

三次均得到 `observed=73 / stored=73 / groups=1 / limited=false`，与生成器 oracle
完全一致。Problems 受控内存三次均约为 retained 0.10MiB、high-water 0.13MiB；它只随
73 个事件及一个分组增长，不随 10GiB 文件字节数增长。

把 `INDEX_BUDGET` 从 8MiB 降为 1MiB 后，索引平均锁段约 2.2–2.4ms，production
总时长中位数 32.78s。为了不把短锁换成高频 UI 事件，桌面端仍只在累计 8MiB 或终态时
发送 `index:progress`。

## Standalone sequential 重扫

这一口径用于验证“索引完成后，Problems 独立连续重扫”的真实墙钟，不借用 production
交错调度的热页局部性：

| 指标 | Run 1 | Run 2 | Run 3 | 中位数 | 门槛 |
|---|---:|---:|---:|---:|---:|
| index 耗时 | 25.0s | 25.2s | 24.4s | **25.0s** | 诊断 |
| index 最大锁段 | 39.97ms | 9.94ms | 12.81ms | **12.81ms** | ≤50ms ✅ |
| Problems 墙钟 | 27.11s | 27.29s | 26.80s | **27.11s** | 诊断 |
| Problems standalone 吞吐 | 2.62M/s | 2.61M/s | 2.65M/s | **2.62M/s** | ≥5M/s ❌ |
| Problems 最大锁段 | 59.33ms | 34.91ms | 28.53ms | **34.91ms** | ≤20ms ❌ |
| 窗口 median | 4.585ms | 4.933ms | 7.990ms | **4.933ms** | 诊断 |
| 窗口 p99 | 96.684ms | 54.530ms | 97.706ms | **96.684ms** | ≤5ms ❌ |

三次 sequential 都得到同一 73/73/1 oracle。用户正常打开文件走 production
交错调度，但 encoding/profile 改变后的从零重分析更接近此口径；因此 standalone 失败
不能被 production 热页吞吐掩盖。

## 终审后窗口与锁预算首修复测

独立终审后做了两项不改变事件语义的性能首修：

- All 连续窗口从“每行重新定位 checkpoint”改为一次定位后前向扫描整个窗口。
- `scan_problems_step` 在 4096 行/128 detail 行之外增加确定性的 512KiB 多行切片上限；
  接受下一行前若会越界就释放 Session 锁，不使用墙钟截止。单个原子物理行可独占一步，
  因此这不是任意输入下的锁时长保证。

最终 512KiB 版本各做一轮同 corpus spot check（用于确认方向，不替代冷/暖各三次）：

| 口径 | index | Problems / combined | Problems max | 窗口 median / p99 | 判定 |
|---|---:|---:|---:|---:|---|
| production | 25.71s | 8.5M/s / 34.17s | 2.38ms | 2.716 / 33.118ms | combined/锁通过，p99 失败 |
| sequential | 25.1s | 2.49M/s / 53.68s | 21.60ms | 5.515 / 59.003ms | 吞吐/锁/p99 失败 |

窗口批量化把这两轮的最小服务时间降到 0.211ms / 0.136ms，production median 也降至
2.716ms；但随机冷页和调度尖峰仍使 p99 远超 5ms。把字节预算继续缩到 256KiB 的探索轮
次出现 2.07M/s、40.00ms max，没有稳定改善缺页尖峰且增加调度开销，因此未保留。结论是：
下一步应拆分锁等待与锁内服务时间，并做有界 sequential read-ahead；不能继续靠缩小行块
或更换采样口径宣称过门槛。

## 事件风暴与内存

0.015GiB deterministic storm corpus 的一次 release 结果：

| 指标 | 结果 | 判定 |
|---|---:|---|
| observed occurrences | 120,161 | 与 corpus oracle 一致 |
| stored occurrences | 100,000 | group 结构上限生效 |
| stored groups | 100,000 | distinct fingerprint oracle 一致 |
| `limited` | `true` | ✅ |
| Problems 最大 core 锁段 | 9.96ms | ≤20ms ✅ |
| 受控 retained heap | 42.47MiB | ≤128MiB ✅ |
| 逻辑 charged / high-water | 62.24MiB / 81.62MiB | ≤112MiB 预算 ✅ |

风暴窗口延迟不纳入稀疏 corpus 的交互门槛；它用于证明极端高基数输入仍受结构上限和统一
逻辑预算约束，并显式报告 `limited=true`，不会无限增长或静默少报。

## 与历史基线的关系

历史 index-only 报告是 8MiB 步长、20.6s、窗口完成后 p99 1.56ms。当前 production
加入确定性 Problems 分析并改为 1MiB 短锁后，index 中位数 23.92s，index + Problems
中位数 32.78s，仍低于 37s 硬门槛；但扫描期间的随机跨文件窗口 p99 明显高于完成后窗口
基线，不能把两者当成同一口径。

| 版本/口径 | index | index + Problems | 内存口径 |
|---|---:|---:|---|
| 历史无 Problems（2026-07-06） | 20.6s | 不适用 | 私有内存峰值约 1.4GiB |
| 当前 production 中位数 | 23.92s | 32.78s | Problems retained 0.10MiB / high-water 0.13MiB |

这张表只用于展示功能加入前后的已知数据，**不是同一二进制、同一系统内存采样方式的严格
before/after 实验**。Problems 在历史版本中不存在，因此没有可比较的 Problems retained
值；历史“私有内存”也不能与当前 Problems 自身账本直接相减。合并前仍需在可控机器上用
同一采样工具分别跑固定点与当前分支，才能给出可信的进程内存增量。

## 补充系统级内存采样

在允许读取 macOS `time -l` 统计的环境中，另跑了一次同一 10GiB production 命令：

```sh
/usr/bin/time -l cargo run --release -p logcore --example bench -- \
  10 /tmp/logfilter-problems-10gb.log \
  --problems-only --assert-contracts --schedule=production
```

该次运行报告 `maximum resident set size = 5,737,496,576 bytes`（5.343GiB）和
`peak memory footprint = 33,791,872 bytes`（32.23MiB）。最大 RSS 包含顺序触达 10GiB
mmap 后的驻留文件页，不能解释为 Problems heap；32.23MiB footprint 也不是历史报告的
“私有内存”同一口径。本次额外运行受缓存状态影响，index + Problems 为 55.15s、窗口
p99 31.582ms，故不混入上面的三次中位数，只作为系统统计可得性和内存量纲记录。

## 测量限制

- 管理环境不能可靠执行系统级 page-cache purge，因而无法声称完成了“受控冷缓存 3 次 +
  受控暖缓存 3 次”。以上是同 corpus 的三次重复 production 与三次重复 sequential，
  缓存状态未被人为控制。
- 已补一轮系统级 RSS/footprint，但尚无固定点的同工具对照，也没有冷/暖各三次，因此不能
  用它声称 Problems 的进程内存增量已经验收。报告中的 charged/retained/high-water 仍只
  是 Problems 自身账本，不等同于进程 RSS。
- 单次 `--assert-contracts` 的 PASS/FAIL 只是该次可适用门槛；最终判定按三次中位数，并且
  production 与 standalone 使用各自正确的门槛。

## 后续性能工作

1. 分离并量化随机窗口的锁等待、checkpoint 扫描和 mmap 缺页，优先解决扫描期 p99；
   不能通过暂停 worker、补结束后样本或改成只测缓存命中来弱化门槛。
2. 为 encoding/profile 重分析设计不依赖 index worker 的受控预取或 I/O 调度，使
   standalone 重扫达到 5M 行/秒且单步最长 ≤20ms。
3. 在可控 page-cache 的环境补齐冷/暖各三次；在此之前实施计划中的完整性能验收保持未完成。
