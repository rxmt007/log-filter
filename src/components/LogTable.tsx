import { useCallback, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { getRows } from "@/lib/ipc";
import type { Row } from "@/types";
import { useSession } from "@/store/session";

const WINDOW = 200; // 每次向后端取的行数
const ROW_H = 20;
const COLS = "64px 60px 96px 22px 56px 56px 150px 1fr";

const dim = { color: "var(--muted-foreground, #888)", padding: "0 4px" } as const;
const cell = { padding: "0 4px" } as const;

export function LogTable() {
  const total = useSession((s) => s.status.totalLines);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef<Map<number, Row>>(new Map());
  const loaded = useRef<Set<number>>(new Set()); // 已请求的 block 起点
  const [, force] = useState(0);

  const rv = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 20,
  });

  const items = rv.getVirtualItems();

  const ensureBlock = useCallback(async (block: number) => {
    if (loaded.current.has(block)) return;
    loaded.current.add(block);
    try {
      const rows = await getRows("all", block, WINDOW);
      rows.forEach((r, i) => cache.current.set(block + i, r));
      force((x) => x + 1);
    } catch {
      loaded.current.delete(block); // 失败允许重试
    }
  }, []);

  useEffect(() => {
    if (items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    ensureBlock(Math.floor(first / WINDOW) * WINDOW);
    ensureBlock(Math.floor(last / WINDOW) * WINDOW);
  }, [items, ensureBlock, total]);

  return (
    <div ref={parentRef} style={{ height: "100%", overflow: "auto", fontFamily: "monospace", fontSize: 12 }}>
      <div style={{ height: rv.getTotalSize(), position: "relative" }}>
        {items.map((vi) => {
          const row = cache.current.get(vi.index);
          return (
            <div
              key={vi.key}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: ROW_H,
                transform: `translateY(${vi.start}px)`,
                display: "grid",
                gridTemplateColumns: COLS,
                whiteSpace: "nowrap",
                alignItems: "center",
              }}
            >
              {row ? (
                <>
                  <span style={dim}>{row.lineNo}</span>
                  <span style={dim}>{row.date}</span>
                  <span style={dim}>{row.time}</span>
                  <span style={cell}>{row.level}</span>
                  <span style={dim}>{row.pid}</span>
                  <span style={dim}>{row.tid}</span>
                  <span style={cell}>{row.tag}</span>
                  <span style={{ ...cell, overflow: "hidden", textOverflow: "ellipsis" }}>{row.message}</span>
                </>
              ) : (
                <span style={{ gridColumn: "1 / -1", padding: "0 4px", color: "var(--muted-foreground, #aaa)" }}>…</span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
