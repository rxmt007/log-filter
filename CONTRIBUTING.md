# 贡献指南

本指南面向获得源码仓库访问权限的协作者。面向用户的问题报告、平台验证和功能建议统一提交到
[`rxmt007/log-filter-desktop`](https://github.com/rxmt007/log-filter-desktop/issues)。

## 提交问题与建议

提交问题前，请先检查公开仓库中的已有 Issue。运行异常报告应尽量包含操作系统与架构、LogFilter 版本、复现步骤、期望结果、实际结果以及必要的日志片段。

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

开始实现前，请阅读[架构说明](docs/architecture.md)和相关设计规范。特别需要保持以下不变量：

- 前端只通过有上限的窗口接口读取可见行，不接收完整文件或完整过滤结果
- `logcore` 不依赖 Tauri 或界面层，并保持可独立测试
- 大文件通过 mmap 访问，过滤结果只保存命中行号，不复制完整文本
- 解析、过滤和搜索等纯函数改动采用测试先行方式

本项目只复现原 LogFilter 的功能目标与外部工作流。贡献不得复制、翻译或紧密改写原项目的源码、注释、资源、文档或其他受版权保护的表达。

## 验证

任何改动在提交前都应通过完整验证：

```bash
cargo test -p logcore && cargo test -p log-filter \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --all -- --check \
  && pnpm typecheck && pnpm lint && pnpm test
```

涉及性能的改动还应运行对应基准，记录测试环境、数据集、修改前后结果和内存变化。测试方法见[现有基准报告](docs/superpowers/2026-07-06-benchmark-10gb.md)。

## 提交与 Pull Request

- 获得维护者授权后，从最新 `main` 创建分支，并向 `main` 提交 Pull Request
- 使用 Conventional Commits，例如 `feat:`、`fix:`、`test:`、`docs:` 或 `refactor:`
- 一个 Pull Request 聚焦一个明确目标，并说明行为变化、验证结果和已知限制
- 功能变化应包含相应测试；用户可见变化应同步更新文档
- 不要提交真实本地路径、个人联系信息、设备标识、内网地址、密钥或生产日志

维护者可以使用 `dev` 作为批量集成缓冲分支。

源码、测试、文档或资源变更仅接受已签署适用书面贡献/IP 协议、并经维护者授权的协作者提交。提交者必须有权提供相关内容；第三方代码或资源必须在引入前确认其允许专有分发，并保留必要的版权、许可声明及源代码提供义务。
