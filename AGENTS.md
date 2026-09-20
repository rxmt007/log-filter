# AGENTS.md — LogFilter

跨平台 Android logcat 桌面工具，Windows 优先，兼顾 macOS / Linux；支持 10GB+ 日志。项目文档默认中文。

## 按任务取上下文

设计以[复刻规范](docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md)及相关功能规范为准；只读当前任务涉及的章节。代码结构和实现细节见[架构说明](docs/architecture.md)，历史计划和验收记录不是每次任务的前置流程。

- UI 改动参考[主界面稿](docs/design/LogFilter.dc.html)和[交互窗口稿](docs/design/LogWindow.dc.html)，离线渲染脚本为 `docs/design/support.js`。需求无其他说明时按设计稿实现；功能要求优先于设计稿，差异写入变更说明。
- IPC 改动核对 `src/lib/ipc.ts`、`src/types.ts` 与 `src-tauri/src/{commands,dto,lib}.rs`；并发实现见 `src-tauri/src/{state,problems}.rs`。
- 性能改动参考[10GiB 基准](docs/superpowers/2026-07-06-benchmark-10gb.md)；Problems 参考[功能规范](docs/superpowers/specs/2026-07-26-logfilter-problems-workbench-design.md)和[验收报告](docs/superpowers/2026-07-28-problems-mvp-closure.md)。Problems 的受控冷/暖缓存及其他平台真机性能仍未验收闭环，不得把已有数值达标表述为完整硬验收。
- adb / 解析兼容性参考[真机验证记录](docs/superpowers/2026-07-06-adb-device-verification.md)。`track-devices` 已规划未实施。

## 项目约束

- `crates/logcore/` 是独立 Rust 引擎，不依赖 Tauri / UI；`src-tauri/` 为 Tauri v2 薄封装，`src/` 为 React 19 + TypeScript + Vite 前端。
- 文件通过 mmap 访问，过滤只保存命中行号 `Vec<u32/u64>`，不复制全文。前端只经 `get_rows` / `get_rows_checked` 读取可见窗口，**每次最多 512 行**，不得传输整文件或整份过滤结果。
- 使用 TanStack Virtual 自研虚拟列表，不用 shadcn Data Table；Tailwind v4 为 CSS-first，配置在 `src/index.css`；shadcn/ui 使用 Base UI · nova preset · Lucide，状态用 zustand。
- Session 由 `Mutex` 保护：写侧持锁时先递增 session / analysis generation，再替换 Session。普通后台任务通过 `lock_session_if_current`、Problems 通过 `lock_analysis_if_current` 持锁校验；filter / search / export 用各自任务代号取消旧任务。
- Problems 每个 1MiB 索引片后最多用 32 个独立短锁追赶，每步最多 4096 物理行 / 128 条详细行；`index:progress` 按累计 8MiB 或终态节流。调整这些预算需性能回归。
- 仅解析 `-v time` / `-v threadtime`，自动识别；七类过滤叠加，保留各自 enabled、`|` 多值及正则开关。配置用 TOML，存平台标准 app 配置目录，位置可配置且可在 GUI 修改；adb 路径可配置并自动扫描常见位置。
- 不做 iOS、kernel `/proc/kmsg` 或移动端；v1 不做多标签会话、过滤器预设、platform-tools 内置和自动更新。
- `LogFilter/` 仅作行为参考，不复制、翻译或紧密改写原 Java 工程受版权保护的内容。写入仓库的内容不得包含真实姓名 / 姓名拼音、真实本地路径、真实内网 IP、设备标识、密钥或生产日志；示例用相对路径、`/Users/alice/...`、`192.168.x.x`。第三方许可要求见[贡献指南](CONTRIBUTING.md)。

## 实现与验证

- 按当前需求完成实现、相关验证及本次改动引入的问题修复；常规可逆的实现选择自主处理。只有会实质影响结果且无法从上下文确定的决策才需澄清；用户明确要求优先于技能的默认流程。
- 解析 / 过滤 / 搜索等纯函数新增或改变行为时采用 TDD，测试覆盖行为与边界。验证范围和完整命令统一见[贡献指南·验证](CONTRIBUTING.md#验证)：纯文档检查内容、链接与差异；代码按影响范围检查；集成门禁仍保留完整验证。
- 跨模块或需分阶段交付的大任务先在 `docs/superpowers/plans/` 写简要计划。重要改动合并前由另一位贡献者或维护者独立审查；按风险选择评审点，无需每个子任务都重复终审，也不在首次实现后自动停工。
- 存在能节省时间或提高质量的独立子任务时委派，并说明范围和交付物；避免让多个代理重复调查同一问题。
- 技能只在任务匹配时加载。已有需求和接口足以开展 TDD / 评审时，无需另建工单系统、重复确认接口或采用固定代理人数；是否提交、发布由当前任务的授权决定。
- 性能改动运行相关 bench，并在 `docs/superpowers/` 记录可比较的前后数字、环境及限制。持久决策落在 `docs/`，本文件只保留长期约束和入口；`.superpowers/` 是已忽略的会话草稿。

## 常用命令与交付

- 包管理用 pnpm，版本见 `package.json`，构建脚本审批记录在 `pnpm-workspace.yaml`。
- 开发：`pnpm tauri dev`；引擎测试：`cargo test -p logcore`；Tauri 测试：`cargo test -p log-filter`。
- 基准：`cargo run --release -p logcore --example bench -- [GB] [文件路径]`。已有文件大小匹配时复用。
- 打包：`pnpm tauri build`，产物在仓库根 `target/release/bundle/`，支持 Windows msi / nsis、macOS dmg、Linux deb。
- Conventional Commits；`main` 为主干，外部贡献向 main 提 PR；维护者可 rebase 合入长期缓冲分支 `dev`，批次提交 dev→main PR。合并方式一律 rebase。
- `dev` 不跑 CI，代码集成前需本地完整验证；main PR 由 [CI](.github/workflows/ci.yml) 跑三系统矩阵，纯文档按其过滤规则跳过。main push 不触发 CI；[打包工作流](.github/workflows/desktop-build.yml) 仅手动或 `v*` tag 触发。
- 合并后对齐 dev 是单独的维护操作；仅在用户明确要求对齐且确认本地工作、未合入提交已妥善保留后，执行 `reset --hard main` 与 `push --force-with-lease`。
