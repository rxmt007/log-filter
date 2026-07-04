# AGENTS.md — LogFilter 跨平台复刻

> 面向 AI 助手与协作者的工程指南。**权威设计以规范文档为准**,先读它:
> [`docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md`](docs/superpowers/specs/2026-07-01-logfilter-cross-platform-rewrite-design.md)

## 项目简介

把 2013 年的 Java Swing 工具 **LogFilter v1.8**(Android logcat 查看器)从零复刻为**跨平台桌面客户端**(Windows 主打,兼顾 macOS / Linux)。相较原版的核心增强:**支持 10GB+ 超大日志文件**。

## 技术栈

- **后端引擎**:Rust,独立 `logcore` crate(mmap + 索引 + 过滤 / 搜索),不依赖 UI
- **桌面壳**:Tauri v2
- **前端**:Vite + React + TypeScript + **Tailwind v4(CSS-first;配置在 `src/index.css`,无 `tailwind.config.js`)** + shadcn/ui(Base UI · nova preset · Lucide 图标)
- **表格虚拟化**:TanStack **Virtual**(自研虚拟列表;**不用** shadcn Data Table)
- **前端状态**:zustand　**配置**:TOML

## 架构铁律(不可违背)

1. **只传可见窗口**:前端永不整体接收文件,一律经 `get_rows(view, start, count)` 取可见窗口,`count` 有上限(如 ≤512)。任何"把整文件 / 整过滤结果发给前端"的做法都禁止。
2. **引擎与 UI 解耦**:`logcore` 不依赖 Tauri / UI,解析 / 索引 / 过滤 / 搜索 / 切分全部可脱离界面单测。
3. **绝不整体载入**:文件用 mmap;过滤只产出**命中行号数组**(`Vec<u32/u64>`),不复制文本。
4. **纯函数先行 TDD**:解析器、过滤器等为纯函数,先写测试再写实现。
5. **不拷贝原始 Java 代码**:`./LogFilter` 仅作行为参考(带第三方版权、已 gitignore、成型后删除)。

## 目录结构

```
crates/logcore/     纯 Rust 引擎(model/mmap_source/indexer/parser/filter/
                    search/bookmarks/session/adb/export/split/config)
src-tauri/          Tauri v2 应用(main / commands 薄封装 / events 进度事件)
src/                前端(components / lib/ipc.ts / hooks / store / types.ts)
docs/               规范与设计文档
LogFilter/          原 Java 工程(只读参考,已忽略,将删除)
```

## 开发与命令(脚手架建好后适用)

- 引擎单测:`cargo test -p logcore`
- 桌面 dev(**前期在本机调试**):`pnpm tauri dev`
- 打包:`pnpm tauri build` → Windows `.msi`/`.exe`、macOS `.dmg`、Linux `.deb`(Debian / Ubuntu 较新 LTS)
- 包管理器:pnpm(已确定;pnpm 11 的构建脚本审批记录在 `pnpm-workspace.yaml`)
- **CI**:规划 GitHub Actions 三系统矩阵 + `tauri-action`;由用户择机推库并开启,当前不依赖 CI。

## 关键约定

- **配置**:存各平台标准 app 配置目录(可配置位置),TOML 格式,**GUI 内也可修改**。
- **adb**:支持"可配置 adb 可执行文件路径" + 自动扫描常见位置;`adb devices` 选设备,`logcat` 子进程 run/pause/stop。platform-tools 内置为后续可选。
- **解析**:仅 `-v time` 与 `-v threadtime` 两种格式,自动识别。
- **过滤**:7 类叠加(级别位掩码 / PID / TID / Tag 显示·排除 / 关键词 查找·排除),各带 enabled,`|` 多值,含**正则开关**。

## 范围提醒

- **不做**:iOS、kernel `/proc/kmsg`、移动端。
- **v1 不做**(后续):多标签会话、过滤器预设、platform-tools 内置、自动更新。
- **v1 新增**:全局搜索、导出过滤结果、大文件切分。

## 备注

- UI 当前样式为**规划基线**,后续会再做一版交互设计。
- §4 索引步长、`get_rows` count 上限、DOM/canvas 渲染等细节,在实施中按实际功能 / 性能确定与调整。
