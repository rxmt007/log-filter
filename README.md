# LogFilter

跨平台桌面版 Android logcat 查看器，提供多维过滤、全局搜索与实时 adb 抓取能力。基于 Tauri v2 + Rust 构建。

## 功能特性

- **多维过滤**：级别位掩码、PID、TID、Tag（显示/排除）、关键词（查找/排除）共 7 类过滤条件叠加，每类支持独立开关、`|` 多值与正则开关
- **全局搜索**：跨全量日志的高速搜索与跳转
- **书签**：标记关键行，支持跳转到上一 / 下一书签
- **Minimap**：日志全貌缩略导航
- **实时抓取**：通过 adb 连接设备，支持 logcat 的启动 / 暂停 / 恢复 / 停止 / 清空
- **导出**：导出全量或过滤结果，支持取消与进度展示
- **文件切分**：按需切分大体积日志文件
- **多编码支持**：兼容常见文本编码
- **明暗主题**：内置浅色 / 深色两套界面主题

## 快速开始

```bash
# 安装依赖
pnpm install

# 启动桌面端开发环境
pnpm tauri dev

# 构建安装包（Windows .msi/.exe、macOS .dmg、Linux .deb）
pnpm tauri build
```

## 测试与验证

```bash
# Rust 引擎与应用测试
cargo test -p logcore
cargo test -p log-filter

# 代码规范检查
cargo clippy --workspace --all-targets
cargo fmt --all -- --check

# 前端检查
pnpm typecheck
pnpm lint
pnpm test
```

## 文档

- [用户手册](docs/user-manual.md)（[HTML 版](docs/user-manual.html)）
- [架构说明](docs/architecture.md)（[HTML 版](docs/architecture.html)）

## 许可

内部项目,暂未选定开源许可。

