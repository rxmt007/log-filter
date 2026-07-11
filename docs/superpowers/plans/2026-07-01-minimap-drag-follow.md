# Minimap Drag Follow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让左侧小地图像常规 slider/scrollbar 一样支持点击跳转和按住拖动实时跟手。

**Architecture:** 只改前端 `Minimap` 交互层,不改 `logcore`/Tauri 契约。`Minimap` 用 Pointer Events 和 `requestAnimationFrame` 把指针位置转换为当前结果索引,写入 zustand 的 `selectedResultIndex`;`LogTable` 继续通过现有 `selectedResultIndex` effect 滚动并只请求可见窗口。

**Tech Stack:** React + TypeScript + zustand + TanStack Virtual + Tailwind v4 CSS-first。

---

## 文件结构

- 修改 `src/components/Minimap.tsx`: 增加坐标换算 helper、Pointer Events、RAF 节流、拖动态。
- 修改 `src/index.css`: 增加小地图拖动状态样式。
- 不修改 `crates/logcore`、`src-tauri`、`src/lib/ipc.ts`: 后端契约保持不变。

---

### Task 1: 锁定小地图拖动交互结构

**Files:**
- Test: 临时 `node <<'NODE'` 静态检查
- Modify: `src/components/Minimap.tsx`
- Modify: `src/index.css`

- [ ] **Step 1: 写失败检查**

Run:

```bash
node <<'NODE'
const fs = require('fs');
const minimap = fs.readFileSync('src/components/Minimap.tsx', 'utf8');
const css = fs.readFileSync('src/index.css', 'utf8');
const failures = [];
if (!minimap.includes('pointerToResultIndex')) failures.push('Minimap must extract pointerToResultIndex helper');
if (!minimap.includes('onPointerDown')) failures.push('Minimap must start drag with onPointerDown');
if (!minimap.includes('onPointerMove')) failures.push('Minimap must follow drag with onPointerMove');
if (!minimap.includes('onPointerUp')) failures.push('Minimap must end drag with onPointerUp');
if (!minimap.includes('onPointerCancel')) failures.push('Minimap must handle pointer cancel');
if (!minimap.includes('requestAnimationFrame')) failures.push('Minimap must throttle pointer movement with requestAnimationFrame');
if (!minimap.includes('setPointerCapture')) failures.push('Minimap must capture the pointer during drag');
if (minimap.includes('onClick=')) failures.push('Minimap must not rely on onClick as the main interaction path');
if (minimap.includes('setView("all")')) failures.push('Minimap must not switch back to all view');
if (!css.includes('data-dragging="true"')) failures.push('CSS must expose a dragging visual state');
if (failures.length) {
  console.error(failures.join('\n'));
  process.exit(1);
}
NODE
```

Expected: 失败,输出缺少 pointer/RAF/dragging 相关能力。

- [ ] **Step 2: 实现坐标换算 helper**

在 `src/components/Minimap.tsx` 中,保留 `bucketRanges` 和 `rangeStyle`,新增:

```ts
function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

function pointerToResultIndex(clientY: number, rect: DOMRect, resultCount: number) {
  if (resultCount <= 0 || rect.height <= 0) return null;
  const frac = clamp((clientY - rect.top) / rect.height, 0, 1);
  return clamp(Math.floor(frac * resultCount), 0, resultCount - 1);
}
```

- [ ] **Step 3: 实现 Pointer Events + RAF**

在 `Minimap` 组件中新增 refs 和拖动状态:

```ts
const [dragging, setDragging] = useState(false);
const draggingRef = useRef(false);
const frameRef = useRef<number | null>(null);
const pendingIndexRef = useRef<number | null>(null);

const scheduleResultIndex = useCallback(
  (index: number) => {
    pendingIndexRef.current = index;
    if (frameRef.current != null) return;
    frameRef.current = window.requestAnimationFrame(() => {
      frameRef.current = null;
      const next = pendingIndexRef.current;
      pendingIndexRef.current = null;
      if (next != null) {
        setSelectedResultIndex(next);
      }
    });
  },
  [setSelectedResultIndex],
);

const updateFromPointer = useCallback(
  (event: React.PointerEvent<HTMLElement>) => {
    if (!resultCount) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const resultIndex = pointerToResultIndex(event.clientY, rect, resultCount);
    if (resultIndex != null) {
      scheduleResultIndex(resultIndex);
    }
  },
  [resultCount, scheduleResultIndex],
);

const endDrag = useCallback(() => {
  draggingRef.current = false;
  setDragging(false);
}, []);
```

注意把 import 改成:

```ts
import { useCallback, useEffect, useRef, useState } from "react";
```

- [ ] **Step 4: 替换 button 事件**

把小地图根节点从 `onClick` 改成:

```tsx
<button
  className="lf-minimap"
  data-dragging={dragging || undefined}
  type="button"
  aria-label="日志小地图"
  onPointerDown={(event) => {
    if (!resultCount) return;
    event.preventDefault();
    draggingRef.current = true;
    setDragging(true);
    event.currentTarget.setPointerCapture(event.pointerId);
    updateFromPointer(event);
  }}
  onPointerMove={(event) => {
    if (!draggingRef.current) return;
    event.preventDefault();
    updateFromPointer(event);
  }}
  onPointerUp={(event) => {
    updateFromPointer(event);
    endDrag();
  }}
  onPointerCancel={endDrag}
  onLostPointerCapture={endDrag}
>
```

- [ ] **Step 5: 清理 RAF**

在 `Minimap` 组件中加入 unmount 清理:

```ts
useEffect(() => {
  return () => {
    if (frameRef.current != null) {
      window.cancelAnimationFrame(frameRef.current);
    }
  };
}, []);
```

- [ ] **Step 6: 增加拖动视觉状态**

在 `src/index.css` 中 `.lf-minimap-viewport` 附近增加:

```css
.lf-minimap[data-dragging="true"] {
  cursor: grabbing;
}

.lf-minimap[data-dragging="true"] .lf-minimap-viewport {
  border-color: var(--lf-accent-strong);
  background: rgba(59, 130, 246, 0.2);
}
```

- [ ] **Step 7: 运行检查**

Run: Step 1 的 `node <<'NODE'...` 脚本。

Expected: exit 0。

- [ ] **Step 8: 构建验证**

Run:

```bash
pnpm build
```

Expected: TypeScript 和 Vite build 通过。

- [ ] **Step 9: 提交**

```bash
git add src/components/Minimap.tsx src/index.css
git commit -m "feat(ui): make minimap drag follow table"
```

---

### Task 2: 完整验证与自审

**Files:**
- Review-only: `src/components/Minimap.tsx`, `src/components/LogTable.tsx`, `src/index.css`

- [ ] **Step 1: 运行完整验证**

Run:

```bash
cargo test -p logcore
cargo build --workspace
pnpm build
```

Expected:

- `cargo test -p logcore`:全部测试通过。
- `cargo build --workspace`:通过。
- `pnpm build`:通过。

- [ ] **Step 2: 对抗式自审**

Run:

```bash
rg "onClick=|onPointerDown|onPointerMove|requestAnimationFrame|setPointerCapture|setView\\(|getRows\\(|WINDOW|MAX_ROWS|selectedResultIndex" src src-tauri crates/logcore/src -n
```

检查结论必须满足:

- `Minimap` 主交互入口是 Pointer Events。
- `Minimap` 不调用 `setView("all")`。
- `LogTable` 仍通过 `selectedResultIndex` 滚动。
- 前端仍用 `WINDOW = 200` 取可见窗口。
- 后端 `get_rows` 仍有 `MAX_ROWS = 512` 上限。
- 本迭代没有新增后端命令。

- [ ] **Step 3: 如发现 Critical/Important 问题,先修复再继续**

修复后运行:

```bash
pnpm build
```

再用独立提交:

```bash
git commit -m "fix(ui): stabilize minimap drag follow"
```

- [ ] **Step 4: 里程碑提交**

若工作树干净且所有验证通过:

```bash
git commit --allow-empty -m "milestone: complete minimap drag follow"
```

- [ ] **Step 5: 汇报**

最终回复包含:

- 做了什么。
- 验证命令结果。
- 对设计稿的偏差与原因。
- 待 GUI 复核点:点击小地图非框位置跳转、按住拖动跟手、快速拖动上下边界、空结果无误跳。
