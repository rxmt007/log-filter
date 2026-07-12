# Toolbar Tooltip 显示规则设计

## 背景

工具栏控件同时使用了浏览器原生 `title` 和自绘 `data-tooltip`,鼠标 hover 时会出现两套 tooltip。顶部第一行控件默认向上弹出,靠近窗口标题栏时会被裁切。

## 方案

采用方案 A:保留自绘 tooltip,移除自绘控件上的原生 `title`,并让顶部第一行控件向下弹出。

- 带 `data-tooltip` 的控件不再设置 `title`,避免浏览器原生 tooltip 与自绘 tooltip 同时出现。
- 保留 `aria-label`,保证图标按钮仍有可访问名称。
- 顶部第一行三个来源/设备/命令选择控件设置 `data-tooltip-placement="bottom"`。
- 默认 tooltip 仍向上弹出,适用于工具栏动作按钮、级别按钮、搜索按钮和表格列按钮。
- CSS 增加 `bottom` placement 规则,不引入 portal 或额外运行时状态。

## 验证

结构契约脚本需要覆盖:

- Toolbar 自绘 tooltip 控件不包含对应的 `title` 属性。
- LogTable 列按钮不再使用原生 `title`。
- 顶部三个选择控件声明 `data-tooltip-placement="bottom"`。
- CSS 包含 `[data-tooltip-placement="bottom"]::after` 规则。
