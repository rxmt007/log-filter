# LogFilter

LogFilter 是面向 Android `logcat` 的跨平台桌面查看器，支持本地日志文件分析与 adb 实时采集。项目采用 Rust、Tauri v2 和 React 构建，核心引擎通过 mmap、检查点索引和窗口化数据访问处理大型日志文件。

## 项目状态

项目仍处于早期开发阶段。源码仓库仅供授权协作者使用；面向用户的文档、问题反馈和后续正式安装包统一发布在
[`rxmt007/log-filter-desktop`](https://github.com/rxmt007/log-filter-desktop)。平台状态如下：

| 平台 | 当前状态 |
| --- | --- |
| macOS Apple Silicon | 已完成手动运行测试；构建未进行代码签名和 Apple 公证 |
| macOS Intel | 尚未完成手动验证 |
| Windows | 尚未完成手动验证 |
| Linux | 尚未完成手动验证 |

CI 会在 macOS、Windows 和 Linux 上执行编译、测试与静态检查，但通过 CI 不代表安装包已经在对应平台完成运行验证。欢迎相关平台用户通过[公开 Issue](https://github.com/rxmt007/log-filter-desktop/issues)报告构建和运行结果。

LogFilter 面向大文件场景设计，并已在 macOS Apple Silicon 上完成 10 GiB、约 7,115 万行日志的基准验证。测试环境、方法与结果见[性能基准报告](docs/superpowers/2026-07-06-benchmark-10gb.md)；实际性能取决于硬件、操作系统和日志内容。

## 功能

- 组合过滤：支持级别、PID、TID、Tag 与关键词等七类条件，并提供多值、排除和正则匹配
- 全局搜索：在完整日志范围内搜索并定位结果
- 窗口化浏览：虚拟化渲染与按需读取，避免将完整文件传入前端
- 实时采集：通过 adb 启动、暂停、恢复、停止和清空 logcat
- 书签与 Minimap：标记关键行并浏览日志分布
- 导出与切分：按范围导出日志，并切分大型日志文件
- 多编码与主题：支持常见文本编码及浅色、深色主题

## 内部开发与构建

授权协作者的开发环境需要 Rust stable、Node.js 22、pnpm 11，以及目标平台所需的 Tauri v2 系统依赖。

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

仓库保留了用户手册和架构说明的 HTML 副本，供克隆或下载仓库后在本地浏览器中打开。GitHub 文件页会显示其源代码，因此 README 不直接链接这些 HTML 文件。

## 问题反馈

欢迎试用项目、参与讨论、报告问题并补充平台验证结果。面向用户的问题与建议请提交到[公开 Issue](https://github.com/rxmt007/log-filter-desktop/issues)；源码变更仅接受授权协作者提交，并遵循[内部贡献指南](CONTRIBUTING.md)。

## 致谢与来源关系

本项目受到 [iookill/LogFilter](https://github.com/iookill/LogFilter) 的启发。原项目曾用于维护者的实际 Android 日志分析工作，在此向其作者与贡献者致谢。

当前项目依据所需功能和使用流程，使用 Rust、Tauri v2 与 React 独立重新实现。据当前仓库核查，未发现引入原项目源代码或资源；本项目也不代表原项目的官方延续或关联项目。

## 许可

本仓库中的第一方源码、文档和其他材料采用[专有许可](LICENSE)，未经明确授权不得复制、修改、分发或披露。先前单独发布的版本仍遵循其发布时附带的许可条款。第三方组件继续适用各自的许可证，相关说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
