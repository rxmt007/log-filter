# LogFilter

LogFilter 是面向 Android `logcat` 的跨平台桌面查看器，支持本地日志文件分析与 adb 实时采集。项目采用 Rust、Tauri v2 和 React 构建，核心引擎通过 mmap、检查点索引和窗口化数据访问处理大型日志文件。

## 项目状态

项目仍处于早期公开开发阶段，目前主要提供源码构建。平台状态如下：

| 平台 | 当前状态 |
| --- | --- |
| macOS Apple Silicon | 已完成手动运行测试；构建未进行代码签名和 Apple 公证 |
| macOS Intel | 尚未完成手动验证 |
| Windows | 尚未完成手动验证 |
| Linux | 尚未完成手动验证 |

CI 会在 macOS、Windows 和 Linux 上执行编译、测试与静态检查，但通过 CI 不代表安装包已经在对应平台完成运行验证。欢迎相关平台用户报告构建和运行结果。

LogFilter 面向大文件场景设计，并已在 macOS Apple Silicon 上完成 10 GiB、约 7,115 万行日志的基准验证。测试环境、方法与结果见[基础性能基准](docs/superpowers/2026-07-06-benchmark-10gb.md)及
[Problems 增量分析基准](docs/superpowers/2026-07-26-benchmark-problems-10gb.md)；后者仍有扫描期窗口 p99
和独立重扫吞吐未达标，实际性能取决于硬件、操作系统和日志内容。

## 功能

- 组合过滤：支持级别、PID、TID、Tag 与关键词等七类条件，并提供多值、排除和正则匹配
- 全局搜索：在完整日志范围内搜索并定位结果
- 窗口化浏览：虚拟化渲染与按需读取，避免将完整文件传入前端
- 故障调查工作台：按 AOSP / Android 系统典型日志规则识别并聚合崩溃、ANR、进程重启及内存终止事件
- 实时采集：通过 adb 启动、暂停、恢复、停止和清空 logcat
- 书签与 Minimap：标记关键行并浏览日志分布
- 导出与切分：按范围导出日志，并切分大型日志文件
- 多编码与主题：支持常见文本编码及浅色、深色主题

### 故障调查工作台（Problems）

Problems 是主日志表下方的故障调查入口。它把多行证据整理为事件，按稳定指纹聚合同类
事件，并提供首次/最后发生位置、重复次数、事实明细、原文定位、临时未过滤上下文和范围
导出。事件索引只保存行号、类别、指纹、进程身份和紧凑证据引用；日志原文仍由既有窗口
接口按需读取，不会因打开工作台而把整份文件载入内存或传给前端。

识别采用固定、可测试的确定性规则，当前主要覆盖 AOSP / Android 系统常见格式，包括
Java/Kotlin 未处理异常与 fatal `OutOfMemoryError`、ANR、native crash、进程重启/明确
signal exit、LMK 和 kernel OOM。它不是对任意应用日志做关键词猜测：关键词只用于找到候选，候选还必须满足
对应事件的字段、来源、多行边界和关联约束。应用或 OEM 自定义格式若不满足这些契约，
可能不会被识别。

当前实现不调用大模型。界面严格区分“日志中检测到的事实”和“排查提示”：事实可回到
具体源行，提示只是按事件类别提供的通用检查清单。事件同组仅表示规范化指纹相同，不表示
根因相同；工具不会自动宣称已经判断根因。

## 从源码构建

开发环境需要 Rust stable、Node.js 22、pnpm 11，以及目标平台所需的 Tauri v2 系统依赖。

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

构建当前平台的安装包：

```bash
pnpm tauri build
```

当前没有经过签名和公证的 macOS 发行包。请在使用开发构建前了解并评估操作系统给出的安全提示。

## 验证

提交改动前，请运行与 CI 对应的完整验证命令：

```bash
cargo test -p logcore && cargo test -p log-filter \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --all -- --check \
  && pnpm typecheck && pnpm lint && pnpm test
```

## 文档

- [用户手册](docs/user-manual.md)
- [架构说明](docs/architecture.md)
- [开源许可与重写风险审查](docs/superpowers/2026-07-21-open-source-license-review.md)

仓库保留了用户手册和架构说明的 HTML 副本，供克隆或下载仓库后在本地浏览器中打开。GitHub 文件页会显示其源代码，因此 README 不直接链接这些 HTML 文件。

## 参与项目

欢迎试用项目、参与讨论、报告问题、补充平台验证结果并提交改进。开始贡献前请阅读[贡献指南](CONTRIBUTING.md)，其中说明了开发环境、架构约束、验证要求和提交约定。

## 致谢与来源关系

本项目受到 [iookill/LogFilter](https://github.com/iookill/LogFilter) 的启发。原项目曾用于维护者的实际 Android 日志分析工作，在此向其作者与贡献者致谢。

当前项目依据所需功能和使用流程，使用 Rust、Tauri v2 与 React 独立重新实现。据当前仓库核查，未发现引入原项目源代码或资源；本项目也不代表原项目的官方延续或关联项目。

## 许可证

LogFilter 以 [`GPL-3.0-or-later`](LICENSE) 许可发布。分发修改版或二进制时，请遵守 GPL 对完整对应源代码和许可声明的要求。第三方组件仍适用各自的许可证，相关说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
