# LogTable 选区对比增强实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标:** 按已确认的方案 B 增强 LogTable 拖选复制时的整行选区对比度。

**架构:** 本次只调整前端表格选区视觉层。`LogTable.tsx` 已经输出 `data-copy-selected="true"`,实现只需要增加 CSS token 与更高优先级的选区文本规则,并用现有结构检查脚本防回归。

**技术栈:** React + TypeScript + Tailwind v4 CSS-first,现有 `scripts/verify-logtable-interaction.mjs` 结构契约检查。

---

### Task 1: 选区对比契约

**Files:**
- Modify: `scripts/verify-logtable-interaction.mjs`

- [ ] **Step 1: 写失败检查**

在 marked / copy-selected 相关检查之后加入:

```js
expectContract("css defines copy-selected text token", files.css.includes("--lf-row-copy-selected-text"));
expectContract(
  "css makes copy-selected row text override level colors",
  files.css.includes('.lf-table-row[data-copy-selected="true"] > span'),
);
```

- [ ] **Step 2: 运行并确认失败**

Run: `node scripts/verify-logtable-interaction.mjs`

预期: FAIL,错误包含 `css defines copy-selected text token`。

### Task 2: 方案 B CSS

**Files:**
- Modify: `src/index.css`

- [ ] **Step 1: 增加浅色主题 token**

在 `:root` 的 copy-selected token 旁加入:

```css
--lf-row-copy-selected: #b9d7ff;
--lf-row-copy-selected-border: #1d4ed8;
--lf-row-copy-selected-text: #123a7a;
```

- [ ] **Step 2: 增加深色主题 token**

在 `.lf-theme-dark` 的 copy-selected token 旁加入:

```css
--lf-row-copy-selected: rgba(91, 157, 255, 0.34);
--lf-row-copy-selected-border: #b7d3ff;
--lf-row-copy-selected-text: #d7e6ff;
```

- [ ] **Step 3: 增加选区行文本覆盖规则**

将以下规则放在日志级别文字色规则之后,确保能覆盖 W/E/F 的文字色:

```css
.lf-table-row[data-copy-selected="true"] > span {
  color: var(--lf-row-copy-selected-text);
  font-weight: 650;
}
```

- [ ] **Step 4: 保持原生文字选区透明**

确认现有规则仍保留:

```css
.lf-table-row[data-copy-selected="true"] ::selection {
  color: inherit;
  background: transparent;
}
```

### Task 3: 验证与提交

**Files:**
- Verify: `scripts/verify-logtable-interaction.mjs`
- Verify: `src/index.css`

- [ ] **Step 1: 运行交互契约检查**

Run: `node scripts/verify-logtable-interaction.mjs`

预期: PASS,输出 `log table interaction contracts verified`。

- [ ] **Step 2: 运行项目验证**

Run:

```bash
cargo test -p logcore
cargo build --workspace
pnpm build
git diff --check
```

预期: 全部 exit 0。

- [ ] **Step 3: 提交**

Run:

```bash
git add docs/superpowers/plans/2026-07-02-logtable-selection-contrast.md scripts/verify-logtable-interaction.mjs src/index.css
git commit -m "fix(ui): strengthen table selection contrast"
```
