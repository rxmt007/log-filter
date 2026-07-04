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
  const sessionId = useSession((s) => s.sessionId);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef<Map<number, Row>>(new Map());
  const filled = useRef<Map<number, number>>(new Map()); // block 起点 -> 已缓存行数
  const inflight = useRef<Set<number>>(new Set());
  const [, force] = useState(0);

  // 切换文件(sessionId 变化)时清空缓存,避免残留上一个文件的行。
  useEffect(() => {
    cache.current.clear();
    filled.current.clear();
    inflight.current.clear();
    parentRef.current?.scrollTo({ top: 0 });
    force((x) => x + 1);
  }, [sessionId]);

  const rv = useVirtualizer({
    count: total,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 20,
  });

  const items = rv.getVirtualItems();

  const ensureBlock = useCallback(async (block: number, totalNow: number) => {
    const want = Math.min(WINDOW, totalNow - block); // 该块当前实际存在的行数
    if (want <= 0) return;
    if ((filled.current.get(block) ?? 0) >= want) return; // 已缓存全部可用行
    if (inflight.current.has(block)) return;
    inflight.current.add(block);
    try {
      const rows = await getRows("all", block, WINDOW);
      rows.forEach((r, i) => cache.current.set(block + i, r));
      filled.current.set(block, rows.length);
      force((x) => x + 1);
    } finally {
      inflight.current.delete(block);
    }
  }, []);

  // items 或 total 变化时按可见范围取块;total 增长会让未满块重新取。
  useEffect(() => {
    if (items.length === 0) return;
    const first = items[0].index;
    const last = items[items.length - 1].index;
    ensureBlock(Math.floor(first / WINDOW) * WINDOW, total);
    ensureBlock(Math.floor(last / WINDOW) * WINDOW, total);
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
