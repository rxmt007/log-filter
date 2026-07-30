# LogFilter 内置 ADB 可行性评估（2026-07-30）

## 结论

**技术上可行，推荐采用 A：保留可配置/系统 ADB，并增加内置 ADB 兜底；不推荐只提供
内置 ADB。**

建议的默认解析顺序是：

1. 用户明确配置的 ADB；
2. 当前 `PATH` 和常见 Android SDK 目录中的系统 ADB；
3. 随 LogFilter 安装的内置 ADB；
4. 前一项在“无法启动、架构错误、缺少依赖、版本探测失败”时才回退下一项；“没有设备”
   不是回退理由。

这延续了现有行为，又能让没有安装 Android Studio/SDK 的用户零配置开始使用。系统 ADB
优先还有一个重要原因：不同版本的 ADB client 默认共用本机 `localhost:5037` server。
ADB 的协议明确提供 `host:version` 和 `host:kill`；当 client 发现 server 版本不匹配时，
源码会结束旧 server 并启动自己的版本。这会打断 Android Studio、命令行脚本或其他调试
工具正在使用的 ADB 连接，而不是一个只影响 LogFilter 的内部实现细节
（[ADB services](https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/dev/services.md)、
[版本检查源码](https://android.googlesource.com/platform/packages/modules/adb/+/refs/tags/android-14.0.0_r27/client/adb_client.cpp)）。
仓库已有真机验证记录也因此把“系统优先、内置兜底”列为约束
（[adb 真机验证](2026-07-06-adb-device-verification.md)）。

内置时不应打包整个 Platform-Tools 目录。LogFilter 只需要 ADB，`fastboot` 等工具既不被
调用，也会扩大包体、许可清单和攻击面。应按固定版本提取 ADB 的最小宿主运行闭包：

- Windows：`adb.exe`、`AdbWinApi.dll`、`AdbWinUsbApi.dll`，以及对应 NOTICE/来源清单；
- macOS：`adb` 及 NOTICE/来源清单；
- Linux：`adb` 及 NOTICE/来源清单。

Windows 运行闭包不能简化成一个 `adb.exe`。AOSP 当前构建描述明确把 `AdbWinApi` 声明
为共享依赖、把 `AdbWinUsbApi` 声明为必需产物
（[ADB Android.bp](https://android.googlesource.com/platform/packages/modules/adb/+/refs/heads/main/Android.bp)）。
每次升级仍要从实际固定版本归档重新验证文件列表，不能永久假定上述集合不会变化。

## 当前实现与改动边界

现有实现已经具备混合方案的大部分上层能力：

- `AppConfig.adb_path` 是可选路径，设置界面可以选择 ADB
  （[config.rs](../../crates/logcore/src/config.rs)、[ToolDialogs.tsx](../../src/components/ToolDialogs.tsx)）。
- `resolve_adb_path` 当前按“显式配置 → `PATH` → 常见 SDK 目录”查找；`adb_command`、
  `list_devices` 和 logcat 子进程生命周期都在 `logcore` 中，并已处理 Windows 无控制台
  窗口和设备列表超时（[adb.rs](../../crates/logcore/src/adb.rs)）。
- Tauri 层只把解析出的绝对路径交给 `logcore`，不会把 ADB 逻辑放到前端
  （[commands.rs](../../src-tauri/src/commands.rs)）。
- 当前包配置只有许可证等资源，没有外部二进制
  （[tauri.conf.json](../../src-tauri/tauri.conf.json)）；三平台打包在各自原生 runner 上
  生成 MSI/NSIS、DMG、DEB
  （[desktop-build.yml](../../.github/workflows/desktop-build.yml)）。

因此不需要重写实时抓取链路。合理的模块边界是：

- `src-tauri` 负责解析安装包内 sidecar 的平台路径，并将它作为一个候选路径传入；
- `logcore` 保持 Tauri 无关，只负责候选优先级、能力探测和现有
  `std::process::Command` 生命周期；
- 一次抓取会话解析一次 ADB 路径，暂停/恢复继续使用同一可执行文件；抓取进行中不能静默
  切换来源；
- 状态/UI 显示 `configured`、`system` 或 `bundled`、`adb version` 结果和实际回退原因，
  便于支持，不把“用了哪个 ADB”藏在实现里。

Tauri v2 原生支持 `bundle.externalBin`，并要求为每个 target triple 准备带后缀的输入文件；
Rust 端也有正式的 sidecar 启动 API
（[Tauri sidecar 文档](https://v2.tauri.app/develop/sidecar/)）。LogFilter 可以继续使用已经
验证过的 `std::process::Command`，但仍应让 `externalBin` 负责安装位置、Unix 可执行位和
macOS 签名；不要把 ADB 当普通只读 resource 后再临时复制执行。普通资源路径应使用
Tauri 的路径解析 API，而不是自行拼接安装目录
（[Tauri resources 文档](https://v2.tauri.app/develop/resources/)）。

## 三种方案比较

| 方案 | 用户体验 | 可复现性 | 兼容与回退 | 包/签名/许可 | 支持成本 | 判断 |
|---|---|---|---|---|---|---|
| A. 内置 + 系统/自定义可选 | 无 SDK 也可用；高级用户可选 | 内置路径可固定，系统路径仍有差异 | 最强，可避开某个 ADB 回归 | 增加一次完整治理 | 中等，策略要清楚 | **推荐** |
| B. 仅内置 | 最简单的首次使用 | 最强 | 最差；只能随 LogFilter 升级，无法绕开企业/OEM 环境问题 | 与 A 相同 | 表面低，故障时高 | 不推荐 |
| C. 仅系统/自定义（现状） | 需要用户安装或定位 SDK | 最弱 | 可由用户自行升级/替换 | 最小 | “找不到/版本太旧/PATH 不同”较多 | 可作为第一阶段基线 |

只内置 ADB 还会失去一个重要兼容通道：LogFilter 目前没有 `adb connect`/`adb pair` UI。
Android TV 的网络连接常由用户或其他工具预先建立。继续复用系统 ADB server 能直接看到
这些连接；另起隔离 server 则需要重新实现连接、配对、认证和状态管理。

## ADB 本身的兼容性边界

ADB 是 client、宿主 server 和设备端 `adbd` 三部分；所有普通 clients 默认连接本机
5037 端口，server 再复用设备连接
（[Android ADB 文档](https://developer.android.com/tools/adb)、
[ADB man page](https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/user/adb.1.md)）。
这带来以下判断：

- 内置 ADB 不等于“私有 ADB”。不增加参数时，它仍与 Android Studio/系统 ADB 共用
  5037 server。
- `-P`/`-L` 可以改变 server 端口或 socket，但隔离不是 MVP 的正确默认值：它会失去
  已连接的网络 TV，两个 server 也可能竞争同一 USB 设备，并要求 LogFilter 自己管理
  connect/pair 和认证。
- ADB 官方说明最新 Platform-Tools 对较旧 Android 版本向后兼容，原则上应使用最新稳定版
  （[Platform-Tools release notes](https://developer.android.com/tools/releases/platform-tools)）。
  这降低了宿主 ADB 与旧 TV 的风险，但不保证 OEM 修改的 `logcat` 格式、缓冲区权限或
  设备端 `logcat -T` 行为一致。
- 当前 LogFilter 只使用 `devices -l` 和受限的 `logcat -v threadtime` 参数，命令解析器
  拒绝管道、重定向和额外 shell 参数；内置 ADB 不需要扩大命令范围
  （[adb.rs](../../crates/logcore/src/adb.rs)）。
- `-T` 是传给设备端 logcat 的能力。替换宿主 ADB 不能修复老设备或 OEM logcat 不支持
  `-T` 的问题；现有“失败后停止、用户重新开始”的行为仍需保留。

截至本报告日期，官方页面列出的 37.0.1 仍标为 Canary，并包含 Windows Server 缺少
`wlanapi.dll` 时延迟加载的修复；发布版不应因为版本号较大就自动选择 Canary。实际实施时
应固定“当日最新稳定版”，同时在普通 Windows 桌面和 GitHub Windows Server runner 上
分别验证。版本升级只能通过 LogFilter 正式发版完成，不在运行时覆盖已安装的 sidecar。
尤其在 macOS 上，修改 `.app` 内已签名的 ADB 会破坏外层签名。

## 平台打包与签名

### Windows

推荐输入布局：

```text
src-tauri/binaries/
  logfilter-adb-x86_64-pc-windows-msvc.exe
  windows/AdbWinApi.dll
  windows/AdbWinUsbApi.dll
  windows/NOTICE.txt
```

`externalBin` 使用名称 `logfilter-adb`，避免与系统 `adb.exe` 混淆；两个 DLL 必须以原名
安装到 sidecar 可执行文件能安全加载的位置。Windows 对非完整路径 DLL 有明确的搜索顺序，
可写当前目录会形成 DLL preloading 风险；依赖应与可执行文件一起放入受安装器保护的目录，
并用绝对 ADB 路径启动
（[Microsoft DLL security](https://learn.microsoft.com/windows/win32/dlls/dynamic-link-library-security)、
[DLL redirection guidance](https://learn.microsoft.com/windows/win32/dlls/dynamic-link-library-redirection)）。

不能只签 MSI/NSIS 安装器。Windows Smart App Control 会在实际加载时评估各个二进制，
Microsoft 要求测试安装/卸载和所有代码路径；未知且未签名的内部 EXE/DLL 仍可能被拦截
（[Smart App Control](https://learn.microsoft.com/windows/apps/develop/smart-app-control/overview)、
[签名测试](https://learn.microsoft.com/windows/apps/develop/smart-app-control/test-your-app-with-smart-app-control)）。
发布门禁应检查 `logfilter-adb.exe`、两个 DLL、主程序和安装器的 Authenticode 状态。
SignTool 支持 EXE、DLL 和 MSI
（[SignTool](https://learn.microsoft.com/windows/win32/seccrypto/using-signtool-to-sign-a-file)）。

Windows 仍可能需要设备 OEM 的 USB driver；把 ADB 放进应用不会安装驱动。官方设备指南
同样把 Windows ADB driver 列为单独准备项
（[hardware device setup](https://developer.android.com/studio/run/device)）。

### macOS

Tauri 会把 `externalBin` 放到 `.app/Contents/MacOS`，把它加入 sidecar 签名列表，并先签
内层二进制、最后签应用；其 bundler 源码明确实现了这个顺序
（[Tauri macOS bundler](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/macos/app.rs)）。
这正好符合 Apple 对嵌套 helper/tool “由内向外签名”的要求
（[Apple Code Signing Guide](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html)）。

必须验证固定版 macOS ADB 的实际架构。若它是 universal Mach-O，可在 staging 时为
`x86_64-apple-darwin` 和 `aarch64-apple-darwin` 两个 Tauri 输入名使用同一经校验内容；
若不是，就必须分别提供对应产物，不能靠 Rosetta 当正式支持策略。DMG 对外发布还应完成
Developer ID 签名、notarization 和 Gatekeeper 验证；Apple 的公证服务会检查恶意内容和
代码签名问题
（[Apple notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)）。

现有工程尚未在打包配置中建立完整签名/公证参数，因此“加入 ADB”和“正式签名发布链”
应作为同一发布门禁处理，不能只验证本机未签名 DMG 能启动。

### Linux

当前只发布 Ubuntu 22.04 runner 生成的 `.deb`。Tauri bundler 会把 `externalBin` 复制到
DEB 的 `/usr/bin`；源码可见资源则进入 `/usr/lib/<package>`
（[Tauri Debian bundler](https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/linux/debian.rs)）。
因此 sidecar 必须命名为 `logfilter-adb`，不能叫 `adb`，否则会与发行版
`android-sdk-platform-tools` 的文件所有权发生冲突。

Linux USB 仍依赖用户组和 udev 规则。Android 官方要求 Ubuntu 用户属于 `plugdev`，
并安装覆盖设备的 udev rules；这些规则来自社区维护的
`android-sdk-platform-tools-common`，不是 Google 的 ADB 可执行文件自带内容
（[hardware device setup](https://developer.android.com/studio/run/device)）。LogFilter
安装器不应静默修改系统组或广泛安装 udev 规则；应在“设备未发现”诊断中给出明确指引。

固定版本还需在最低支持的 Ubuntu/Debian 上检查 ELF 架构、动态依赖和所需 glibc 版本。
当前官方公开下载页没有给出适用于本项目的 Linux host ABI 承诺，因此不能仅凭在
`ubuntu-22.04` 打包成功就宣称所有“较新 LTS”均兼容。若未来发布 Linux ARM64，需先确认
Google 是否提供对应 host binary；没有时只能继续使用系统 ADB、自己按 AOSP 构建并承担
完整供应链，或不支持该组合。

## 许可与再分发

ADB 源码仓库声明 Apache-2.0，并携带 NOTICE；Apache-2.0 允许按条件再分发目标代码
（[ADB NOTICE](https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/NOTICE)、
[ADB package license](https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/Android.bp)）。
Android SDK License Agreement 同时规定：一般 SDK 内容不得随意再分发，但以开源许可证
发布的组件，其使用、复制和分发仅由对应开源许可证治理
（[Android SDK terms，第 3.4–3.5 条](https://developer.android.com/studio/terms)）。

这支持“ADB 可以依法内置”的方向，但**不能直接推出 Google 当前 Platform-Tools ZIP 中
每一个预编译文件都已完成再分发审查**。正式发布前必须把以下项目设为法律/发布 gate：

1. 固定具体归档版本和 SHA-256，不使用会变化的 `platform-tools-latest-*` 作为发布输入；
2. 保存官方来源 URL、下载日期、归档 hash、提取文件 hash 和 `adb version`；
3. 审阅该归档随附的完整 `NOTICE.txt`，确认 Windows 两个 DLL 和所有静态链接依赖的通知；
4. 在安装包中保留 Apache-2.0 文本、完整 NOTICE 和来源说明；
5. 在 [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md) 增加手工条目。现有
   Rust/pnpm 许可证生成器不会发现手工放入的 ADB 二进制；
6. 不使用 Android/Google 商标暗示 Google 对 LogFilter 的背书，并复核出口限制。

若对 Google 预编译归档的授权范围仍有疑问，风险更低但成本显著更高的替代是从固定 AOSP
tag 自行构建 ADB，并生成完整源码、工具链和依赖 SBOM。此选择不能在没有法务结论时被
当作默认实现细节。

## 安全、故障回退与支持成本

- **供应链**：发布构建不实时取 `latest`；下载固定内容、校验 hash、缓存后再 staging。
  运行时不下载或替换 ADB。
- **执行边界**：始终执行已解析的绝对路径，不通过 shell；继续保留当前 logcat 参数白名单。
- **自定义路径**：这是用户主动授权执行任意外部程序的能力。UI 应显示实际路径和版本，
  配置变更需显式完成，不能从日志内容或工作目录自动发现一个同名 `adb`。
- **共享 server**：不默认传 `-a`，继续只使用 localhost server。内置/系统切换前显示
  “可能重启本机 ADB server”的事实；不要把 `kill-server` 当常规清理动作。
- **认证密钥**：不要随应用分发固定 `adbkey`。ADB 的 `ADB_VENDOR_KEYS` 是用户/环境
  范围配置（[ADB man page](https://android.googlesource.com/platform/packages/modules/adb/+/HEAD/docs/user/adb.1.md)）。
- **回退触发**：仅限二进制不存在、不可执行、架构/依赖错误、`adb version` 失败或
  `devices -l` 明确表明 client 本身启动失败；设备 offline/unauthorized/空列表必须原样
  呈现，不能换另一个 ADB 后伪装成已修复。
- **可诊断性**：错误中区分“未找到 ADB”“ADB 无法启动”“server 版本切换”
  “USB driver/udev 权限”“设备未授权”“设备端 logcat 不兼容”。
- **会话一致性**：开始抓取后固定 ADB 来源；恢复抓取不重新走候选优先级，避免同一会话
  跨版本切换。

## 包体、性能和 CI 影响

每个平台安装包只携带本平台的最小 ADB 运行闭包，不应把 Windows/macOS/Linux 三套全部
放入同一 artifact。包体会增加一个宿主 ADB 可执行文件及 Windows DLL，但不会改变
10GB+ 日志的 mmap、增量索引、窗口读取或前端传输不变量。ADB server 的常驻内存属于
已有实时抓取成本；内置与系统版本不会同时常驻，除非刻意做端口隔离。

在没有选定固定版本前，本报告不写一个易过时的 MB 数字。实施 spike 应记录三平台：

- 原始归档、提取闭包、未压缩安装包和最终压缩 installer 的增量；
- `adb version`、架构和依赖检查结果；
- 冷启动 `devices -l` 耗时、无设备时退出行为、server 已存在时行为；
- Windows Smart App Control/Defender、macOS Gatekeeper/notarization、Linux 最低 LTS
  的安装后启动结果。

[desktop-build.yml](../../.github/workflows/desktop-build.yml) 需要在 `Tauri build` 前增加
“准备固定版 ADB”步骤：按 matrix 下载或从可信缓存取件、校验 SHA-256、只提取白名单、
重命名为 target-triple sidecar、保留 DLL/NOTICE，然后执行 `adb version`。不能把二进制
直接提交到 Git 历史，也不能让 tag 构建跟随可变 `latest`。

打包后的门禁至少包括：

1. 解包 MSI/NSIS、DMG、DEB，确认只含本平台闭包且文件位置正确；
2. 从安装后的绝对路径执行 `version`、`devices -l`、start/stop logcat 生命周期；
3. Windows 校验 EXE/DLL/installer 签名，macOS 执行
   `codesign --verify --deep --strict`、Gatekeeper assessment 和公证票验证；
4. 与一个先运行的系统 ADB 同版本和异版本各测一次，确认 UI 说明与 server 行为；
5. Windows USB + OEM driver、macOS USB、Ubuntu USB + udev，以及 Android TV 网络 ADB
   各做真机回归；
6. 无内置文件、内置文件损坏、缺 DLL、不可执行、用户自定义路径失效时验证回退。

## 分阶段建议

### Phase 0：不改变发布物的能力与法务 spike

- 选定一个最新稳定 Platform-Tools 版本，保存三平台归档和文件 hash；
- 核实实际文件清单、架构、动态依赖、NOTICE 和预编译包再分发结论；
- 在临时打包分支验证 Tauri sidecar 的 MSI/NSIS、DMG、DEB 安装布局和签名；
- 量化包体增量；不修改现有 ADB 优先级。

退出条件：三平台安装后 `adb version` 成功，Windows DLL 闭包、macOS 签名/公证、Linux
执行位和许可清单均有确定结论。

### Phase 1：混合来源 MVP

- 增加 `AdbSource = Auto | Bundled | Custom`；默认 `Auto` 的实际顺序为
  configured → system → bundled；
- Tauri 层提供内置候选，`logcore` 完成可测试的候选选择和探测；
- UI 显示来源、版本、路径和失败原因；
- 只在开始会话时回退，抓取中固定来源；
- 内置 ADB 随正式 LogFilter 更新，不做独立自动更新。

退出条件：上节打包门禁全部通过，并证明现有系统/自定义 ADB 用户的行为没有改变。

### Phase 2：根据真实支持数据决定是否调整默认值

- 收集不含设备序列号/路径的本地诊断：来源类别、版本、失败类别；是否上报必须另行取得
  用户同意；
- 若“系统 ADB 版本差异”远少于“找不到 ADB”，可评估把 Auto 改为 bundled 优先；
- 若网络 TV 是主场景，先补 `connect`/`pair` 产品设计，再讨论私有 server/端口隔离；
- 每次 ADB 升级单独做设备矩阵和安装包签名回归。

## 需要产品/发布侧确认

1. 是否接受推荐的 **configured → system → bundled** 默认顺序，还是更重视完全可复现而
   要 bundled 优先？
2. Windows/macOS 正式发布是否准备建立付费代码签名与 macOS notarization？没有它们，
   内置可执行文件的用户拦截风险不能算收口。
3. 当前正式支持的宿主架构是否仅 Windows x64、macOS Intel/Apple Silicon、Linux x64？
   Linux/Windows ARM64 会显著改变 ADB 来源策略。
4. 是否接受 Linux 安装后出现名字为 `/usr/bin/logfilter-adb` 的私有 CLI；若不接受，
   需要定制 DEB 布局到 `/usr/lib/logfilter/` 并自行保证权限和定位。
5. 对 Google 预编译 Platform-Tools 中最小 ADB 闭包的再分发，是否由项目维护者自行完成
   notice 审查，还是需要正式法务确认？
6. 是否把 `adb connect`/无线配对纳入近期范围？在它们缺席时，不建议做独立 5037 server。
