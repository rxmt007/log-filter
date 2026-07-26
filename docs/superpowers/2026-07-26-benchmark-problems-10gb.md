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

## 测量限制

- 管理环境不能可靠执行系统级 page-cache purge，因而无法声称完成了“受控冷缓存 3 次 +
  受控暖缓存 3 次”。以上是同 corpus 的三次重复 production 与三次重复 sequential，
  缓存状态未被人为控制。
- `/usr/bin/time -l` 在当前受限环境无法读取完整系统统计，因此没有给出可信的进程 RSS。
  报告中的内存是 Problems 自身账本的 charged/retained/high-water，不等同于进程 RSS。
- 单次 `--assert-contracts` 的 PASS/FAIL 只是该次可适用门槛；最终判定按三次中位数，并且
  production 与 standalone 使用各自正确的门槛。

## 后续性能工作

1. 分离并量化随机窗口的锁等待、checkpoint 扫描和 mmap 缺页，优先解决扫描期 p99；
   不能通过暂停 worker、补结束后样本或改成只测缓存命中来弱化门槛。
2. 为 encoding/profile 重分析设计不依赖 index worker 的受控预取或 I/O 调度，使
   standalone 重扫达到 5M 行/秒且单步最长 ≤20ms。
3. 在可控 page-cache 的环境补齐冷/暖各三次；在此之前实施计划中的完整性能验收保持未完成。
