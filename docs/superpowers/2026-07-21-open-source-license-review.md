# LogFilter 开源许可与重写风险审查（2026-07-21）

> 本文是基于公开一手资料与当前依赖清单的工程风险审查，不是针对任何司法辖区的正式法律意见。若准备商业发行、接受大量外部贡献，或发现与原项目存在实质性代码 / 资源相似，应由熟悉适用法域的软件知识产权律师复核。

## 结论摘要

1. **推荐本项目采用 `GPL-3.0-or-later`。** 对当前以本地运行和分发安装包为主的桌面应用，GPLv3 已能要求下游在分发修改版或二进制时以 GPL 许可整个衍生程序并提供对应源代码；AGPLv3 的额外价值主要出现在“修改后只通过网络供远程用户使用、却不分发副本”的场景。[GPLv3 第 5、6 节](https://www.gnu.org/licenses/gpl-3.0.html#section5)、[AGPLv3 第 13 节](https://www.gnu.org/licenses/agpl-3.0.html#section13)、[GNU 对 AGPL 用途的说明](https://www.gnu.org/licenses/why-affero-gpl.html)
2. **原 `iookill/LogFilter` 不能作为“整个项目已按 GPL 开源”处理。** 截至本次审查，其仓库没有仓库级 `LICENSE` / `COPYING`，GitHub API 的许可证识别为 `null`；但仓库内一个文件单独声明为 GPL v2 或以后版本，另一个文件带有明确的专有与禁止复制声明。整体属于许可状态混合且不完整，而不是统一 GPL 项目。[原仓库固定快照](https://github.com/iookill/LogFilter/tree/fd5e19f90735afb293c5deafbae8e6ecbb3867ed)、[仓库元数据 API](https://api.github.com/repos/iookill/LogFilter)、[GPL 文件声明](https://github.com/iookill/LogFilter/blob/fd5e19f90735afb293c5deafbae8e6ecbb3867ed/src/RecentFileMenu.java#L1-L22)、[专有文件声明](https://github.com/iookill/LogFilter/blob/fd5e19f90735afb293c5deafbae8e6ecbb3867ed/src/T.java#L4-L18)
3. **独立复现功能和外部行为通常比翻译源码风险低得多，但不能据此承诺“零风险”。** 美国版权局明确说明，程序的思想、逻辑、算法、功能、系统设计等不受版权保护，受保护的是具体可版权表达；同时，复制、改编和分发属于权利人的专有权利。[美国版权局：Computer Programs](https://www.copyright.gov/register/tx-programs.html)、[17 U.S.C. §§ 102、106](https://www.copyright.gov/title17/92chap1.html#102)
4. **致谢值得保留，但致谢不是授权。** 若当前代码确为独立实现，法律上通常不因复现功能而必须致谢；从项目历史、社区透明度和善意归因角度，README 应明确感谢原项目，同时避免使用可能暗示复制源码的“基于原项目代码改写”等表述。若实际复制了受保护表达，致谢和给新仓库添加 GPL 都不能补足原本不存在的授权。[GitHub 关于无许可证仓库的说明](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository)、[17 U.S.C. § 106](https://www.copyright.gov/title17/92chap1.html#106)
5. **当前直接依赖未发现明显的 GPLv3 许可冲突。** Rust / JavaScript 顶层依赖以 MIT、Apache-2.0、BSD-3-Clause、ISC、Unlicense 为主，另有一个 OFL-1.1 字体包；这些许可可与 GPLv3 程序组合或作为独立字体聚合分发，但仍需保留各自的版权与许可通知。[GNU 许可兼容性说明](https://www.gnu.org/licenses/license-compatibility.html)、[GNU 许可列表](https://www.gnu.org/licenses/license-list.html)、[OFL 官方 FAQ](https://openfontlicense.org/ofl-faq/)

## 1. 原项目的许可状态

### 1.1 仓库级状态

审查对象固定为原仓库提交 `fd5e19f90735afb293c5deafbae8e6ecbb3867ed`。其根目录和递归文件树中没有仓库级 `LICENSE`、`COPYING`、`NOTICE` 或等效的统一授权文件，GitHub 仓库 API 的 `license` 字段为 `null`。[固定快照](https://github.com/iookill/LogFilter/tree/fd5e19f90735afb293c5deafbae8e6ecbb3867ed)、[递归文件树 API](https://api.github.com/repos/iookill/LogFilter/git/trees/fd5e19f90735afb293c5deafbae8e6ecbb3867ed?recursive=1)、[仓库元数据 API](https://api.github.com/repos/iookill/LogFilter)

公开可读不等于获得复制、改编和再发布许可。GitHub 官方文档也明确指出：仓库没有许可证时，默认版权规则仍适用，他人不能据此任意复制、分发或制作衍生作品。[GitHub：Licensing a repository](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository)

### 1.2 文件级状态并不一致

- `RecentFileMenu.java` 自带第三方版权和 **GPL v2 或以后版本**的文件头。该授权只能够直接证明这个文件的许可，不能反向推定整个仓库都采用 GPL。[文件头](https://github.com/iookill/LogFilter/blob/fd5e19f90735afb293c5deafbae8e6ecbb3867ed/src/RecentFileMenu.java#L1-L22)
- `T.java` 自带第三方公司的专有、保密以及未经书面同意不得复制或利用的声明。该文件不应复制、翻译、紧密改写或纳入新项目；原仓库维护者也未必有权替该第三方重新授权。[文件头](https://github.com/iookill/LogFilter/blob/fd5e19f90735afb293c5deafbae8e6ecbb3867ed/src/T.java#L4-L18)
- 其余源码未发现足以覆盖全仓库的明确许可声明。因此，不能以某一个 GPL 文件为依据，把其余文件也当作 GPL 代码。

**工程结论：** 对原项目的代码、注释、资源和文档应采取“除非具体文件有明确许可或另获书面授权，否则不复制”的基线。即使采用 GPLv3 发布本项目，也不会自动获得原项目未授予的权利。只有版权持有人才能授予相应许可；GNU FAQ 对重新许可和版权持有人权限也作了同样区分。[GNU GPL FAQ：版权持有人与许可](https://www.gnu.org/licenses/gpl-faq.html#HeardOtherLicense)

## 2. GPL-3.0-or-later 与 AGPL-3.0-or-later

两者都是 OSI 批准的开源许可证，也是 SPDX 许可清单中的标准标识。[OSI：GPL-3.0](https://opensource.org/license/gpl-3.0)、[OSI：AGPL-3.0](https://opensource.org/license/agpl-3.0)、[SPDX License List](https://spdx.org/licenses/)

| 对比项 | `GPL-3.0-or-later` | `AGPL-3.0-or-later` |
|---|---|---|
| 分发修改版源码 | 修改后分发时，整个衍生程序须按 GPL 授权 | 同样适用 |
| 分发二进制 | 须按第 6 节提供机器可读的 Corresponding Source | 同样适用 |
| 仅在服务器运行、不分发副本 | 通常不因该行为本身触发向网络用户提供源码 | 若修改版支持网络远程交互，第 13 节要求向远程用户显著提供对应源码 |
| 对当前本地桌面应用的额外效果 | 已覆盖主要分发场景 | 通常与 GPL 相同，网络条款很少有机会生效 |
| 更适合的项目形态 | 桌面应用、命令行程序、会被再分发的软件 | Web 服务、网络服务端、希望覆盖 SaaS 漏洞的软件 |

GPLv3 第 5 节要求分发修改版源码时把整个作品作为整体按 GPL 授权；第 6 节规定了分发目标代码时提供对应源码的方式。[GPLv3 第 5、6 节](https://www.gnu.org/licenses/gpl-3.0.html#section5)

AGPLv3 在这些要求上增加第 13 节：修改版若支持用户通过计算机网络远程交互，必须向这些用户显著、免费提供取得对应源码的机会。GNU 对 AGPL 的官方说明也指出，该附加要求针对“修改后放在服务器上供用户交互、但不发布副本”的缺口。[AGPLv3 第 13 节](https://www.gnu.org/licenses/agpl-3.0.html#section13)、[GNU：Why the Affero GPL](https://www.gnu.org/licenses/why-affero-gpl.html)、[GNU GPL FAQ：GPL 网络使用](https://www.gnu.org/licenses/gpl-faq.html#UnreleasedMods)、[GNU GPL FAQ：AGPL 网络使用](https://www.gnu.org/licenses/gpl-faq.html#UnreleasedModsAGPL)

### 推荐：`GPL-3.0-or-later`

当前产品是安装到用户电脑上的跨平台桌面客户端，不是由项目方托管的日志分析服务。用户获得安装包这一“分发”场景已经落入 GPLv3 的强 copyleft 和对应源码要求；采用 AGPL 不会让普通桌面分发“再严格一层”，只会为将来可能出现的网络交互版本增加规则。因此，`GPL-3.0-or-later` 与项目形态最匹配。

选择 `or-later` 时，收件人始终仍可按 GPLv3 使用该版本；以后版本不会撤回已经由 v3 授予的选择。GNU 官方也说明并推荐“第 3 版或以后版本”的表达，以便未来许可证版本提供澄清或兼容性选项。[GNU GPL FAQ：Version 3 or later](https://www.gnu.org/licenses/gpl-faq.html#VersionThreeOrLater)、[GPLv3 第 14 节](https://www.gnu.org/licenses/gpl-3.0.html#section14)

如果未来新增独立的服务端，并且项目目标变为要求“公开运行修改后的服务也必须向远程用户提供源码”，可对服务端从一开始采用 `AGPL-3.0-or-later`。若届时已有外部贡献，改变既有代码的许可可能需要相应版权持有人的同意，因此应在服务端创建时就确定边界，而不是假定维护者能单方面重授权所有贡献。

### 落地要求

采用 GPL 不只是增加一个徽章或仓库根文件。至少应当：

1. 在仓库根目录放置**未经修改的完整 GPLv3 文本**；GNU 明确要求完整保留许可证文本。[GPLv3 官方文本](https://www.gnu.org/licenses/gpl-3.0.html)、[GNU GPL FAQ：不要删减许可证](https://www.gnu.org/licenses/gpl-faq.html#GPLOmitPreamble)
2. 在 README 明确写明项目整体采用 `GPL-3.0-or-later`，并在 `Cargo.toml`、`package.json` 等元数据中使用同一 SPDX 标识。SPDX 已将模糊的旧标识拆分为 `-only` 和 `-or-later`，应使用当前标识。[SPDX License List](https://spdx.org/licenses/)
3. 在源码文件中加入简短的 SPDX / 许可声明，或至少确保每个源码副本都能明确关联到仓库许可。GNU FAQ 指出，只有一份许可证文件而没有说明哪些代码受其约束会留下不必要的歧义。[GNU GPL FAQ：LicenseCopyOnly](https://www.gnu.org/licenses/gpl-faq.html#LicenseCopyOnly)、[GNU GPL FAQ：NoticeInSourceFile](https://www.gnu.org/licenses/gpl-faq.html#NoticeInSourceFile)
4. 发布安装包或其他目标代码时，同时提供该版本的完整 Corresponding Source，包含构建和安装所需脚本；通过下载页发布时，可在同一位置免费提供源代码并清楚链接。[GPLv3 第 1、6 节](https://www.gnu.org/licenses/gpl-3.0.html#section6)
5. 保留第三方依赖自身的版权与许可文本，并生成随发行物提供的第三方许可清单。把兼容许可并入 GPL 程序不等于可以删除其原有通知。[GNU：License Compatibility and Relicensing](https://www.gnu.org/licenses/license-compatibility.html)

## 3. 独立重写的风险边界

### 3.1 可以独立实现的内容

美国版权法第 102(b) 条排除思想、程序、过程、系统、操作方法、概念和原则；美国版权局对程序的官方说明进一步列出，程序的思想、逻辑、算法、系统、方法、概念、布局不属于版权保护对象。Circular 61 同样指出，版权保护程序中的可版权表达，而不是算法、格式、功能、逻辑或系统设计。[17 U.S.C. § 102(b)](https://www.copyright.gov/title17/92chap1.html#102)、[美国版权局：Computer Programs](https://www.copyright.gov/register/tx-programs.html)、[Circular 61](https://www.copyright.gov/circs/circ61.pdf)

因此，以下做法通常位于较安全的一侧，但必须由新实现者自己完成表达：

- 根据 Android / adb 的公开接口重新实现日志读取、解析、过滤、搜索、书签、导出等功能；
- 通过观察程序外部行为编写新的需求和测试，再用 Rust、Tauri、React 独立实现；
- 采用通用桌面控件和日志查看器常见交互，并自行决定模块、数据结构、文案、图形和视觉细节；
- 对公开文件格式、协议和事实进行兼容实现。

### 3.2 必须避免复制或紧密改写的内容

版权持有人享有复制、制作衍生作品和分发的专有权。更换编程语言、框架或文件结构不当然消除衍生风险；GNU FAQ 也把代码翻译视为一种修改，并强调其仍受原作品版权支配。[17 U.S.C. § 106](https://www.copyright.gov/title17/92chap1.html#106)、[GNU GPL FAQ：TranslateCode](https://www.gnu.org/licenses/gpl-faq.html#TranslateCode)

不得从原项目复制、逐句翻译或紧密改写：

- Java 源码、注释、类 / 方法的独创性组织以及非必要的独特命名；
- 配置文件的完整内容、帮助文字、文档段落、报错文案或其他具有表达性的文本；
- 图标、图片、配色资源、声音、安装素材或其他美术资产；
- 具有独创性的屏幕表达。虽然纯功能与布局本身不受保护，计算机屏幕中具体的图形、文字和可版权表达仍可能受保护；美国版权局的登记规则会把程序及其可版权屏幕显示作为可登记内容处理。[37 C.F.R. § 202.20(c)(2)(vii)(C)](https://www.copyright.gov/title37/202/37cfr202-20.html)、[美国版权局 Compendium](https://www.copyright.gov/comp3/index.html)
- 原仓库中带专有声明的 `T.java` 的任何非微不足道内容；其文件头明确禁止未经书面同意复制或利用。[原文件声明](https://github.com/iookill/LogFilter/blob/fd5e19f90735afb293c5deafbae8e6ecbb3867ed/src/T.java#L4-L18)

若实际复用了 `RecentFileMenu.java`，则必须按其 `GPL-2.0-or-later` 文件许可保留版权与许可通知并满足对应义务；“v2 或以后版本”允许选择 GPLv3，但并不允许删除原作者通知。[原文件声明](https://github.com/iookill/LogFilter/blob/fd5e19f90735afb293c5deafbae8e6ecbb3867ed/src/RecentFileMenu.java#L1-L22)、[GNU GPL FAQ：GPLv2-or-later 与 GPLv3](https://www.gnu.org/licenses/gpl-faq.html#v2v3Compatibility)

### 3.3 对当前仓库的有限核查

当前跟踪文件中没有 `LogFilter/` 原 Java 工程，规范文档也明确记录“全新语言重写、不拷贝原始 Java 代码”，并特别标记了带专有文件头的源文件不得复用。[重写设计规范](specs/2026-07-01-logfilter-cross-platform-rewrite-design.md)

本次用原仓库中特征性文件名、版权关键词和类名对当前跟踪内容做了快速检索，未在产品源码中发现原 Java 版权头或明显的原文件副本。该结果只能证明没有发现直接、显眼的复制，**不等同于完整的代码相似性、资源溯源或司法鉴定**。由于开发过程曾把原工程作为行为参考，严格意义上也不属于双方隔离的 clean-room 流程；若担心某个模块过度相似，最稳妥的做法是针对该模块进行独立相似性审查，必要时重新实现。

建议继续保留以下证据：独立需求文档、TDD 测试、设计来源、实现提交记录以及“未复制原代码 / 资源”的贡献要求。接受社区贡献时，也应要求贡献者确认其提交有权按项目许可证发布。

## 4. 致谢应如何写

若本项目只借鉴目标、使用体验和外部行为，致谢不是取得授权的替代品，也通常不是独立代码获得版权的前提；但原项目确实启发了本项目且曾用于实际工作，主动致谢符合开源社区透明度。

建议 README 使用以下表述：

> 本项目受到 [iookill/LogFilter](https://github.com/iookill/LogFilter) 的启发。原项目曾长期服务于实际 Android 日志分析工作，在此向其作者与贡献者致谢。本项目使用 Rust、Tauri 和 React 构建独立的新实现，不包含原项目源代码。

最后一句只有在持续核实为真时才保留。建议避免“基于原项目代码重写”“移植自原项目”或“基于其设计修改”等容易让读者理解为代码 / 表达衍生的说法；“受到启发的独立重新实现”更准确。

若将来确实引入任何原项目文件，应在引入前逐文件确认许可，保留原版权和许可通知，并单独记录来源。仅在 README 致谢不能补救缺少授权的问题。[GitHub：无许可证仓库的默认规则](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository)

## 5. 当前依赖许可的初步兼容性检查

检查范围是当前 [`Cargo.toml`](../../Cargo.toml)、[`Cargo.lock`](../../Cargo.lock)、[`crates/logcore/Cargo.toml`](../../crates/logcore/Cargo.toml)、[`src-tauri/Cargo.toml`](../../src-tauri/Cargo.toml)、[`package.json`](../../package.json)、`pnpm-lock.yaml` 和已安装依赖元数据。结果如下：

- Rust 直接 / 构建依赖主要为 `MIT OR Apache-2.0`；`encoding_rs` 还包含 BSD-3-Clause 条款，`memchr` 提供 `Unlicense OR MIT` 选择。
- 前端生产依赖主要为 MIT、Apache-2.0 或 ISC。
- `@fontsource-variable/geist` 的字体许可为 OFL-1.1。

锁定依赖的元数据扫描覆盖 474 个 Rust 第三方包和 501 个 pnpm 包记录，未发现缺少许可证字段的包，也未发现仅以 GPL 或 AGPL 授权的依赖。除常见宽松许可证外，传递依赖中还包含少量 MPL-2.0、OFL-1.1、CC-BY-4.0、Python-2.0 等许可证，需要在实际发行物中保留相应声明并按许可证边界处理。

GNU 官方资料确认 Apache-2.0 与 GPLv3 兼容，MIT / Expat、BSD、ISC 和 Unlicense 等宽松许可也可并入 GPL 程序，但原许可和通知仍须保留。[GNU：Apache-2.0 与 GPLv3](https://www.gnu.org/licenses/license-list.html#apache2)、[GNU：Unlicense](https://www.gnu.org/licenses/license-list.html#Unlicense)、[GNU：License Compatibility and Relicensing](https://www.gnu.org/licenses/license-compatibility.html)

OFL 官方 FAQ 明确允许把 OFL 字体与其他 FLOSS 软件聚合、随应用打包；字体自身继续受 OFL 约束，不会因此把整个应用改成 OFL。若随应用分发字体，至少应保留适用的版权声明、许可通知和许可文本；若修改字体，还需遵守 Reserved Font Names 等要求。[OFL FAQ 1.2、1.3、1.20](https://openfontlicense.org/ofl-faq/)

**初步结论：未发现明显阻止项目采用 `GPL-3.0-or-later` 的直接或锁定传递依赖。** 但元数据扫描不等同于完整的发行物许可证审计：桌面包还会涉及不同平台的系统 WebView / 动态库，最终产物也会随依赖升级而变化。发布前应以实际构建产物为对象生成 SBOM / 第三方许可证清单，并在 CI 中阻止未知许可或 GPLv3 不兼容许可进入锁文件。

建议后续补充：

1. `THIRD_PARTY_NOTICES` 或等效的自动生成清单，并随安装包分发；
2. 对 Rust 依赖启用许可证 allowlist（例如审查 `cargo-deny` 生成的结果）；
3. 对 pnpm 锁文件和实际打包资源执行许可证扫描；
4. 每次升级 Tauri、前端框架、字体或新增二进制依赖时重新审查；
5. 在 release 流程中验证对应源码、GPL 全文、第三方许可文本和构建脚本都可从下载页获得。

## 6. 最终建议

- 本项目：采用 **`GPL-3.0-or-later`**。
- 未来独立网络服务：若希望覆盖“仅远程运行而不分发”的修改版，再为服务端选择 **`AGPL-3.0-or-later`**。
- 原项目关系：使用“受到启发的独立重新实现”，明确致谢，但不要声称原仓库整体采用 GPL，也不要把致谢当成许可。
- 代码边界：只复现功能、协议和外部行为；不复制 / 翻译原代码、注释、配置内容、资源和具有独创性的 UI 表达。
- 发布边界：在发布任何安装包前，加入完整 GPLv3 文本、项目级许可声明、源码获取方式和第三方通知，并对实际发行物再做一次许可证审计。
