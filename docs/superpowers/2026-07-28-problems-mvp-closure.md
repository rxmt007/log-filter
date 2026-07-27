# Problems MVP 验收收口（2026-07-28）

本报告记录 Problems MVP 在功能、并发、10GiB 性能和受控内存方面的收口结果。当前
macOS 开发机上、未人为清理 page cache 的三轮中位数均达到实现数值门槛；但规范固定的
冷/暖缓存各三次和与历史版本同工具的系统级 RSS 对照尚未完成，因此**不能把本报告称为
完整的规范化性能硬验收**，也不能据此推导所有平台和机器都达到相同数字。

## 最终实现口径

- 文件：10.00GiB / 10,737,418,410 bytes / 71,158,147 行。
- production：1MiB index 宏步，每 64KiB 检查一次已排队的前台窗口；每个宏步后最多
  32 个独立 Problems step，每步最多 4096 行、512KiB、128 条详细行。
- standalone sequential：索引完成后从第 0 行重分析；使用 8MiB 有界滚动预取，剩余
  4MiB 时补下一段。预取在 Session 锁外执行，不保存文件内容，也不改变事件语义。
  macOS 使用 `F_RDADVISE`，Linux 使用 `MADV_WILLNEED`，Windows 使用
  `PrefetchVirtualMemory`；失败仅降级为无预取。
- 窗口：每轮 100 个真实争锁 `get_rows(All, 200)` 样本，100–200ms 确定性抖动；
  同时报告总延迟、锁等待和锁内服务时间。
- 稀疏 corpus oracle：`observed=73 / stored=73 / groups=1 / limited=false`。

复跑命令：

```sh
cargo run --release -p logcore --example bench -- \
  10 /tmp/logfilter-problems-10gb-closure.log \
  --problems-only --assert-contracts --schedule=production

cargo run --release -p logcore --example bench -- \
  10 /tmp/logfilter-problems-10gb-closure.log \
  --problems-only --assert-contracts --schedule=sequential

cargo run --release -p logcore --example bench -- \
  0.015 /tmp/logfilter-problems-storm-closure.log \
  --problems-only --assert-contracts --corpus=storm --schedule=production
```

## Production 三轮

| 指标 | Run 1 | Run 2 | Run 3 | 中位数 | 门槛 |
|---|---:|---:|---:|---:|---:|
| index 耗时 | 23.91s | 24.86s | 23.44s | **23.91s** | 诊断 |
| index 最大宏步 | 13.30ms | 34.27ms | 27.03ms | **27.03ms** | ≤50ms ✅ |
| Problems 热页吞吐 | 9.3M/s | 9.4M/s | 9.6M/s | **9.4M/s** | 诊断 |
| index + Problems | 31.65s | 32.51s | 30.93s | **31.65s** | ≤37s ✅ |
| Problems 最大锁段 | 0.99ms | 1.29ms | 1.53ms | **1.29ms** | ≤20ms ✅ |
| 扫描期窗口 p99 | 1.392ms | 3.157ms | 1.404ms | **1.404ms** | ≤5ms ✅ |

三轮均得到同一 73/73/1 oracle，Problems retained 约 0.10MiB、high-water 约
0.13MiB。正常打开文件不会启用 standalone 预取；它继续复用索引刚触达的热页。

## Standalone sequential 三轮

| 指标 | Run 1 | Run 2 | Run 3 | 中位数 | 门槛 |
|---|---:|---:|---:|---:|---:|
| index 耗时 | 25.4s | 24.8s | 23.5s | **24.8s** | 诊断 |
| index 最大宏步 | 34.39ms | 171.11ms | 31.41ms | **34.39ms** | ≤50ms ✅（中位数） |
| Problems 墙钟 | 12.31s | 10.30s | 9.93s | **10.30s** | 诊断 |
| Problems standalone 吞吐 | 5.8M/s | 6.9M/s | 7.2M/s | **6.9M/s** | ≥5M/s ✅ |
| Problems 最大锁段 | 9.76ms | 4.24ms | 4.54ms | **4.54ms** | ≤20ms ✅ |
| 扫描期窗口 p99 | 3.356ms | 3.033ms | 7.647ms | **3.356ms** | ≤5ms ✅（中位数） |

每轮预取 1,281 段、累计覆盖 10.00GiB，失败 0 次；同一时刻的逻辑前视仍不超过
8MiB。三轮 oracle 均为 73/73/1。

这里保留两个真实尖峰：Run 2 的 index 最大宏步为 171.11ms，Run 3 的窗口 p99 为
7.647ms。前者发生时该轮窗口 max 仍为 3.723ms，说明 64KiB 协作点保护了已排队读取，
但 OS 调度仍可能拉长一个没有竞争者的宏步。结论是“三轮中位数通过”，不是“每轮绝无
尖峰”。

## 事件风暴

0.015GiB deterministic storm corpus 明确声明为 synthetic events buffer：

| 指标 | 结果 | 判定 |
|---|---:|---|
| observed / stored / groups | 120,161 / 100,000 / 100,000 | oracle 与结构上限一致 |
| `limited` | `true` | ✅ |
| Problems 最大锁段 | 5.84ms | ≤20ms ✅ |
| 窗口 p99 | 4.886ms | ≤5ms ✅ |
| retained / charged / high-water | 42.47 / 62.24 / 81.85MiB | ≤128MiB retained、≤112MiB 逻辑预算 ✅ |

storm 来源声明只存在于 benchmark harness；生产 recognizer 仍拒绝来源未知的
EventLog-shaped 文本，不因压力测试而放宽 provenance gate。

## 与 2026-07-26 基线相比

| 口径 | 旧中位数 | 当前中位数 |
|---|---:|---:|
| production combined | 32.78s | **31.65s** |
| production 窗口 p99 | 28.399ms | **1.404ms** |
| standalone Problems | 2.62M/s | **6.9M/s** |
| standalone Problems 最大锁段 | 34.91ms | **4.54ms** |
| standalone 窗口 p99 | 96.684ms | **3.356ms** |

窗口延迟拆分证明旧尖峰主要来自无公平保证的 Session mutex 重抢；waiter-aware 交接和
64KiB index 协作点解决了锁饥饿。standalone 吞吐则主要受冷 mmap 重读限制，由锁外有界
预取解决。两项优化都保留 10GB+、mmap、紧凑索引和仅获取可见窗口的不变量。

## 正式性能验收仍需补充

- 在可控 page-cache 的专用机器补冷/暖各三次；当前三轮没有人为清理缓存。
- 用同一系统采样工具对固定点和当前分支做 RSS/private-memory 对照。Problems 自身账本
  不能替代进程级内存统计。
- Windows/Linux 的平台预取路径需由三平台 CI 编译，并在对应真机复测吞吐；best-effort
  失败不会影响正确性，但性能数字不能从 macOS 外推。
