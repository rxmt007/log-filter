# LogFilter Desktop 应用图标（纯白底蓝色同系）

本目录是“双带几何结”蓝色同系方案的正式桌面端素材包。该版本已同步到 `src-tauri/icons/`，作为应用当前使用的图标。

## 视觉规格

- 主图形：双带几何结
- 深蓝目标色：`#0067C0`
- 亮蓝目标色：`#22A7F0`
- 背景：铺满画布的纯白色 `#FFFFFF`，四角保持不透明
- 主图形约占画布宽高的 64%（横纵方向略有差异），兼顾 Dock 与 16/32px Finder 列表尺寸
- 无文字、编号、描边、外部投影、环境阴影、光晕或 mockup 效果

macOS 图标输入保持完整方形，不预先绘制圆角或透明裁切，显示时由系统施加平台圆角蒙版。这样避免透明外缘触发灰色兼容底板，也避免自绘圆角的半透明抗锯齿边缘在 Finder 选中背景上形成灰边。该做法遵循 [Apple App icons 设计指南](https://developer.apple.com/design/human-interface-guidelines/app-icons/)。

小尺寸可读性以蓝色主体的明暗层次和负空间维持，不额外增加边框。

选定的栅格方案保留轻微蓝色色调变化；上述色值用于品牌与后续矢量化时的色彩基准。

## 文件结构

```text
source/
  logfilter-app-icon-1024.png     1024×1024 不透明方形主 PNG

generated/tauri-cli-2.11.4/
  icon.icns                       macOS 多尺寸图标
  icon.ico                        Windows 多尺寸图标
  icon.png                        512×512 通用 / Linux PNG
  32x32.png
  64x64.png
  128x128.png
  128x128@2x.png                  256×256
  StoreLogo.png
  Square*Logo.png                 Windows / Microsoft Store 尺寸
```

`icon.ico` 包含 16、24、32、48、64、256 px 图层；`icon.icns` 包含最高 1024 px 的 macOS 表示层。

## 重新生成

在仓库根目录执行：

```sh
pnpm tauri icon \
  docs/design/app-icon/source/logfilter-app-icon-1024.png \
  --output docs/design/app-icon/generated/tauri-cli-2.11.4
```

Tauri CLI 默认还会生成移动端目录。本项目不支持移动端，因此正式素材包只保留桌面端根目录文件。

## 接入说明

`generated/tauri-cli-2.11.4/` 根目录中的桌面端与 Windows Store 文件已同步到 `src-tauri/icons/`。后续若修改主图，应重新生成整套派生文件并整体同步，避免平台间图标版本不一致。
