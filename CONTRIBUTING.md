# 贡献指南

感谢你关注 LogFilter。欢迎通过问题报告、平台验证、文档改进、测试和代码贡献参与项目。

## 提交问题与建议

提交问题前，请先检查已有 Issue。运行异常报告应尽量包含操作系统与架构、LogFilter 提交或版本、复现步骤、期望结果、实际结果以及必要的日志片段。

日志可能包含设备序列号、账号、文件路径、网络地址或业务数据。公开提交前请完成脱敏，不要上传密钥、访问令牌、真实日志文件或其他无权公开的内容。

## 开发环境

项目需要以下工具：

- Rust stable 工具链，并安装 `rustfmt` 与 `clippy`
- Node.js 22
- pnpm 11（具体版本记录在 `package.json`）
- Tauri v2 在目标平台要求的系统依赖

安装前端依赖并启动开发环境：

```bash
pnpm install --frozen-lockfile
pnpm tauri dev
```

## 架构约束

涉及架构或行为变更时，按需阅读[架构说明](docs/architecture.md)和相关设计规范的对应章节。特别需要保持以下不变量：

- 前端只通过有上限的窗口接口读取可见行，不接收完整文件或完整过滤结果
- `logcore` 不依赖 Tauri 或界面层，并保持可独立测试
- 大文件通过 mmap 访问，过滤结果只保存命中行号，不复制完整文本
- 解析、过滤和搜索等纯函数新增或改变行为时采用测试先行方式

本项目只复现原 LogFilter 的功能目标与外部工作流。贡献不得复制、翻译或紧密改写原项目的源码、注释、资源、文档或其他受版权保护的表达。

## 验证

验证按改动影响范围选择。修改前仅在复现问题或建立对照有帮助时运行相关检查；修改后运行受影响检查，并修复本次改动引入的失败。检查通过后，只有新改动、失败或未解决风险才需要扩大或重复验证；已有无关失败应说明，不顺带扩大任务。

| 改动范围 | 验证要求 |
| --- | --- |
| 纯文档、注释、未改变执行逻辑的指令文件 | 检查内容、链接、规则一致性及 `git diff --check`；无需 Rust / 前端测试或性能基准 |
| `logcore` 行为 | 对应模块与集成测试、Rust 格式和相关 clippy 检查 |
| Tauri / IPC / 并发 | 相关 `log-filter` 测试、Rust 格式和 clippy；IPC 变化同时验证前端类型和受影响调用方 |
| 前端行为或样式 | `pnpm typecheck`、`pnpm lint`、相关 Vitest 测试；可见变化检查实际界面 |
| 依赖、构建、跨层架构 | 完整验证及受影响的构建检查 |

可用 `cargo test -p logcore <测试筛选>`、`cargo test -p log-filter <测试筛选>` 或 `pnpm exec vitest run <测试文件>` 运行相关测试。测试应覆盖行为、边界和回归，不为低风险机械改动增加只复述实现的测试。

提交代码 PR、合入无 CI 的 `dev` 或发布前，运行一次完整验证；同一代码状态下已通过的结果可复用。纯文档改动按上表检查。main PR 的三系统 CI 门禁保持不变，本地验证不能替代目标平台验证。

完整验证命令（与 CI 一致）：

```bash
cargo test -p logcore && cargo test -p log-filter \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --all -- --check \
  && pnpm typecheck && pnpm lint && pnpm test
```

涉及性能的改动还应运行对应基准，记录测试环境、数据集、修改前后结果和内存变化。测试方法见[现有基准报告](docs/superpowers/2026-07-06-benchmark-10gb.md)。

## 提交与 Pull Request

- 从最新 `main` 创建分支，并向 `main` 提交 Pull Request
- 使用 Conventional Commits，例如 `feat:`、`fix:`、`test:`、`docs:` 或 `refactor:`
- 一个 Pull Request 聚焦一个明确目标，并说明行为变化、验证结果和已知限制
- 功能变化应包含相应测试；用户可见变化应同步更新文档
- 不要提交真实本地路径、个人联系信息、设备标识、内网地址、密钥或生产日志

维护者可以使用 `dev` 作为批量集成缓冲分支；外部贡献不需要以 `dev` 为起点。

提交贡献即表示你有权提供相关内容，并同意按项目的 `GPL-3.0-or-later` 许可证发布该贡献。第三方代码或资源必须在引入前确认许可兼容性，并保留必要的版权与许可声明。
