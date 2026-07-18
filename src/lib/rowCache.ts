import type { Row } from "@/types";

interface Block {
  rows: Map<number, Row>; // key: 全局 result index
  count: number; // 本块实际填充行数
  epoch: number; // 填充时的缓存纪元
}

/**
 * 以块(block)为粒度的行缓存,带 LRU 上限。用插入序(Map 的天然顺序)
 * 表示最近使用:命中或写入时先 delete 再 set,把块提到最新;超出上限时
 * 逐出最旧的块。纪元(epoch)语义与 LogTable 一致——换过滤条件时只递增
 * 纪元、不清缓存,`get` 仍返回旧行避免闪烁,`isFresh` 因纪元不匹配返回
 * false 触发重拉。
 */
export class RowBlockCache {
  private readonly blocks = new Map<number, Block>();

  constructor(private readonly maxBlocks: number) {}

  /** 命中返回行,并把所属块提升为最近使用。 */
  get(index: number, blockSize: number): Row | undefined {
    const blockStart = Math.floor(index / blockSize) * blockSize;
    const block = this.blocks.get(blockStart);
    if (!block) return undefined;
    // 提升为最近使用。
    this.blocks.delete(blockStart);
    this.blocks.set(blockStart, block);
    return block.rows.get(index);
  }

  /** 该块在给定纪元下是否已填充到 want 行(决定是否需要重新拉取)。 */
  isFresh(blockStart: number, want: number, epoch: number): boolean {
    const block = this.blocks.get(blockStart);
    if (!block) return false;
    return block.epoch === epoch && block.count >= want;
  }

  /** 写入一个块;超出 maxBlocks 时逐出最久未使用的块。 */
  fill(blockStart: number, rows: Row[], epoch: number): void {
    const map = new Map<number, Row>();
    rows.forEach((row, i) => map.set(blockStart + i, row));
    // 整块替换:删除旧条目后重新 set,把块提到最新;缩短的尾块不会残留幽灵行。
    this.blocks.delete(blockStart);
    this.blocks.set(blockStart, { rows: map, count: rows.length, epoch });
    if (this.blocks.size > this.maxBlocks) {
      const oldest = this.blocks.keys().next().value;
      if (oldest !== undefined) this.blocks.delete(oldest);
    }
  }

  clear(): void {
    this.blocks.clear();
  }

  /** 对所有驻留行应用变换(书签状态更新用)。 */
  updateRows(update: (row: Row) => Row): void {
    for (const block of this.blocks.values()) {
      for (const [index, row] of block.rows) {
        block.rows.set(index, update(row));
      }
    }
  }

  blockCount(): number {
    return this.blocks.size;
  }
}
