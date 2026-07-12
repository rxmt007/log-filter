# LogTable 选区对比增强设计

日期: 2026-07-02

## 背景

当前表格拖选复制时,选中范围使用浅蓝整行背景。实际日志中 W/E/F 行带有级别底色,文字又按日志级别着色,导致拖选范围和内容层混在一起。用户希望选中效果更清楚,尤其在 W/E/F 行、标记行和长消息行连续出现时,能够一眼看出选中范围。

## 目标

- 采用已确认的方案 B:选区作为独立的整行视觉层。
- 选区视觉优先级高于 W/E/F 背景、标记行背景和普通行背景。
- 选区范围保留整行蓝色背景,并增加更清晰的边界感。
- 选区内文字保持可读,避免只依赖淡蓝背景。
- 不改变复制数据格式、不改变 `logcore`、不新增 IPC。

## 视觉规则

1. 被原生文本拖选覆盖到的行继续输出 `data-copy-selected="true"`。
2. `data-copy-selected="true"` 行使用更明确的选区色:
   - 浅色主题:比当前选区蓝更深,但不使用反白。
   - 深色主题:使用更高透明度的蓝色层。
3. 选区行统一覆盖 W/E/F、标记行等背景。
4. 选区行的日志文字统一切到深蓝/高对比文本色,并轻微加粗,避免不同级别颜色削弱选区感。
5. 选区行保留行首蓝色边线;如实现成本低,可以为选区范围首尾增加边界线。但本次默认不引入复杂连续范围计算,先通过整行背景和文字对比解决主要问题。

## 非目标

- 不做 C 方案的强反白选区,避免大面积拖选时过重。
- 不改变单击选中行 `data-selected` 的行为。
- 不改变标记行的 bookmark 高亮逻辑。
- 不改变复制为 `str1  str2 ...` 的格式。

## 实现建议

在 `src/index.css` 中调整现有 copy-selected 相关 token 和规则:

- 提升 `--lf-row-copy-selected` 的对比度。
- 增加 `--lf-row-copy-selected-text` token。
- 对 `.lf-table-row[data-copy-selected="true"] .lf-level/.lf-tag/.lf-message` 等文本列统一设色。
- 保持 `.lf-table-row[data-copy-selected="true"]` 的规则位于 W/E/F 与 marked 规则之后,确保选区层优先。
- 保留 `::selection { background: transparent; }`,避免浏览器原生块状选区重新干扰整行效果。

## 验证

- 交互契约脚本应检查 copy-selected 有专门文本色 token 或规则。
- `pnpm build` 应通过。
- `cargo test -p logcore` 与 `cargo build --workspace` 应保持通过。
- GUI 复核点:拖选覆盖 W/E/F 行时,选区范围应明显强于级别底色;日志内容仍能读清。
