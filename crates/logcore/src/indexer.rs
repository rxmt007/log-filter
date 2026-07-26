use memchr::{memchr, memchr_iter};

const DEFAULT_CHECKPOINT_STRIDE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineCheckpoint {
    line: usize,
    offset: u64,
}

/// 增量构建检查点行索引。每隔固定行数记录一次行首偏移,定位具体行时从最近检查点前扫。
pub struct Indexer {
    checkpoints: Vec<LineCheckpoint>,
    total_lines: usize,
    completed_lines: usize,
    cursor: usize,
    checkpoint_stride: usize,
    last_line_start: usize,
}

impl Indexer {
    pub fn new() -> Self {
        Self::with_checkpoint_stride(DEFAULT_CHECKPOINT_STRIDE)
    }

    pub fn with_checkpoint_stride(checkpoint_stride: usize) -> Self {
        Self {
            checkpoints: Vec::new(),
            total_lines: 0,
            completed_lines: 0,
            cursor: 0,
            checkpoint_stride: checkpoint_stride.max(1),
            last_line_start: 0,
        }
    }

    /// 从内部 cursor 起,最多处理 `budget` 字节。返回本次处理的字节数。
    pub fn step(&mut self, bytes: &[u8], budget: usize) -> usize {
        if self.cursor < bytes.len() {
            if self.total_lines == 0 {
                self.add_line_start(0);
            } else if self.cursor > 0
                && bytes[self.cursor - 1] == b'\n'
                && self.last_line_start != self.cursor
            {
                self.add_line_start(self.cursor);
            }
        }
        let end = self.cursor.saturating_add(budget).min(bytes.len());
        let chunk = &bytes[self.cursor..end];
        for pos in memchr_iter(b'\n', chunk) {
            self.completed_lines += 1;
            let abs_next = self.cursor + pos + 1;
            if abs_next < bytes.len() {
                self.add_line_start(abs_next);
            }
        }
        let processed = end - self.cursor;
        self.cursor = end;
        processed
    }

    pub fn is_done(&self, total: usize) -> bool {
        self.cursor >= total
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn total_lines(&self) -> usize {
        self.total_lines
    }

    /// 已经消费到行尾换行符的完整行数。
    ///
    /// 这与 `total_lines`（已发现的行首数）有意分离：增量输入的最后一行可能已经有
    /// 行首，但当前读块尚未包含它的换行符。
    pub fn completed_lines(&self) -> usize {
        self.completed_lines
    }

    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    /// 第 i 行的字节区间 [start, end)(end 含末尾换行,取文本时再裁剪)。
    pub fn line_span(&self, bytes: &[u8], i: usize, frontier: usize) -> Option<(usize, usize)> {
        self.line_spans(bytes, i, i.saturating_add(1), frontier)
            .into_iter()
            .next()
    }

    pub fn line_spans(
        &self,
        bytes: &[u8],
        start: usize,
        end: usize,
        frontier: usize,
    ) -> Vec<(usize, usize)> {
        let end = end.min(self.total_lines);
        let mut spans = Vec::with_capacity(end.saturating_sub(start));
        self.for_each_line_span(bytes, start, end, frontier, |_, span_start, span_end| {
            spans.push((span_start, span_end));
        });
        spans
    }

    /// 单次前向扫描 [start, end),对每一行调用 `f(line, span_start, span_end)`,
    /// 不物化 Vec。`line_spans` 与导出批量原语共用此扫描:定位一次检查点、一路 memchr 到底。
    pub fn for_each_line_span(
        &self,
        bytes: &[u8],
        start: usize,
        end: usize,
        frontier: usize,
        mut f: impl FnMut(usize, usize, usize),
    ) {
        self.for_each_line_span_while(bytes, start, end, frontier, |line, span_start, span_end| {
            f(line, span_start, span_end);
            true
        });
    }

    /// `for_each_line_span` 的可中止版本。回调返回 false 时,当前行仍算已访问；
    /// 返回值是第一条未访问行,调用方可把它作为下一批起点。
    pub fn for_each_line_span_while(
        &self,
        bytes: &[u8],
        start: usize,
        end: usize,
        frontier: usize,
        mut f: impl FnMut(usize, usize, usize) -> bool,
    ) -> usize {
        let end = end.min(self.total_lines);
        if start >= end {
            return start.min(end);
        }
        let frontier = frontier.min(bytes.len());
        let Some(checkpoint) = self.checkpoint_before_or_at(start) else {
            return start;
        };
        let mut line = checkpoint.line;
        let mut offset = checkpoint.offset as usize;
        while line < start {
            let Some(next) = next_line_start(bytes, offset) else {
                return line;
            };
            offset = next;
            line += 1;
        }

        while line < end {
            let line_start = offset;
            let line_end = if line + 1 < self.total_lines {
                match next_line_start(bytes, offset) {
                    Some(next) => next,
                    None => frontier,
                }
            } else {
                frontier
            };
            let keep_scanning = f(line, line_start, line_end.min(frontier));
            offset = line_end;
            line += 1;
            if !keep_scanning {
                break;
            }
        }
        line
    }

    /// Scan a contiguous prefix of `[start, end)`.
    ///
    /// Unlike `for_each_line_span_while`, returning `false` means the current
    /// line was **not** accepted: it is returned as the next start and must not
    /// have been processed by the callback. This lets callers enforce a byte
    /// budget before touching the next line instead of overshooting it.
    pub fn for_each_line_span_prefix(
        &self,
        bytes: &[u8],
        start: usize,
        end: usize,
        frontier: usize,
        mut accept: impl FnMut(usize, usize, usize) -> bool,
    ) -> usize {
        let end = end.min(self.total_lines);
        if start >= end {
            return start.min(end);
        }
        let frontier = frontier.min(bytes.len());
        let Some(checkpoint) = self.checkpoint_before_or_at(start) else {
            return start;
        };
        let mut line = checkpoint.line;
        let mut offset = checkpoint.offset as usize;
        while line < start {
            let Some(next) = next_line_start(bytes, offset) else {
                return line;
            };
            offset = next;
            line += 1;
        }

        while line < end {
            let line_start = offset;
            let line_end = if line + 1 < self.total_lines {
                match next_line_start(bytes, offset) {
                    Some(next) => next,
                    None => frontier,
                }
            } else {
                frontier
            };
            if !accept(line, line_start, line_end.min(frontier)) {
                break;
            }
            offset = line_end;
            line += 1;
        }
        line
    }

    fn checkpoint_before_or_at(&self, line: usize) -> Option<LineCheckpoint> {
        let idx = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.line <= line)
            .checked_sub(1)?;
        self.checkpoints.get(idx).copied()
    }

    fn add_line_start(&mut self, offset: usize) {
        let line = self.total_lines;
        if line.is_multiple_of(self.checkpoint_stride) {
            self.checkpoints.push(LineCheckpoint {
                line,
                offset: offset as u64,
            });
        }
        self.total_lines += 1;
        self.last_line_start = offset;
    }
}

fn next_line_start(bytes: &[u8], offset: usize) -> Option<usize> {
    let relative = memchr(b'\n', bytes.get(offset..)?)?;
    Some(offset + relative + 1)
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_line_starts() {
        let bytes = b"a\nbb\nccc";
        let mut ix = Indexer::new();
        ix.step(bytes, bytes.len());
        assert!(ix.is_done(bytes.len()));
        assert_eq!(ix.total_lines(), 3);
        assert_eq!(ix.line_span(bytes, 0, bytes.len()), Some((0, 2)));
        assert_eq!(ix.line_span(bytes, 1, bytes.len()), Some((2, 5)));
        assert_eq!(ix.line_span(bytes, 2, bytes.len()), Some((5, 8)));
    }

    #[test]
    fn trailing_newline_makes_no_empty_line() {
        let bytes = b"a\nbb\n";
        let mut ix = Indexer::new();
        ix.step(bytes, bytes.len());
        assert_eq!(ix.total_lines(), 2);
        assert_eq!(ix.line_span(bytes, 0, bytes.len()), Some((0, 2)));
        assert_eq!(ix.line_span(bytes, 1, bytes.len()), Some((2, 5)));
    }

    #[test]
    fn completed_lines_excludes_a_budget_split_partial_line() {
        let bytes = b"first line\nsecond line\n";
        let mut ix = Indexer::new();

        ix.step(bytes, 5);

        assert_eq!(ix.total_lines(), 1);
        assert_eq!(ix.completed_lines(), 0);
    }

    #[test]
    fn completed_lines_advances_only_after_the_newline_is_processed() {
        let bytes = b"one\ntwo\n";
        let mut ix = Indexer::new();

        ix.step(bytes, 4);

        assert_eq!(ix.total_lines(), 2);
        assert_eq!(ix.completed_lines(), 1);
    }

    #[test]
    fn completing_a_partial_line_advances_the_frontier_once() {
        let bytes = b"one\ntwo\n";
        let mut ix = Indexer::new();
        ix.step(bytes, 7);
        assert_eq!(ix.completed_lines(), 1);

        ix.step(bytes, 1);
        assert_eq!(ix.completed_lines(), 2);

        ix.step(bytes, usize::MAX);
        assert_eq!(ix.completed_lines(), 2);
    }

    #[test]
    fn crlf_split_between_budgets_is_counted_once() {
        let bytes = b"one\r\ntwo\r\n";
        let mut ix = Indexer::new();

        ix.step(bytes, 4);
        assert_eq!(ix.completed_lines(), 0);

        ix.step(bytes, 1);
        assert_eq!(ix.completed_lines(), 1);

        ix.step(bytes, usize::MAX);
        assert_eq!(ix.completed_lines(), 2);
    }

    #[test]
    fn every_byte_budget_split_matches_the_complete_line_oracle() {
        let bytes = b"one\r\ntwo\nthree";
        for split in 0..=bytes.len() {
            let mut ix = Indexer::new();
            ix.step(bytes, split);
            ix.step(bytes, usize::MAX);

            assert_eq!(ix.total_lines(), 3, "split at byte {split}");
            assert_eq!(ix.completed_lines(), 2, "split at byte {split}");
        }
    }

    #[test]
    fn chunked_stepping_matches_single_step() {
        let bytes = b"line1\nline2\nline3\nline4";
        let mut a = Indexer::new();
        a.step(bytes, bytes.len());
        let mut b = Indexer::new();
        // 逐 3 字节步进,跨越 '\n' 边界
        while !b.is_done(bytes.len()) {
            b.step(bytes, 3);
        }
        assert_eq!(a.total_lines(), b.total_lines());
        for i in 0..a.total_lines() {
            assert_eq!(
                a.line_span(bytes, i, bytes.len()),
                b.line_span(bytes, i, bytes.len())
            );
        }
    }

    #[test]
    fn growing_file_counts_line_added_after_trailing_newline() {
        let mut ix = Indexer::new();
        let first = b"line1\n";
        ix.step(first, first.len());
        assert_eq!(ix.total_lines(), 1);
        assert!(ix.is_done(first.len()));

        let grown = b"line1\nline2";
        ix.step(grown, grown.len());

        assert_eq!(ix.total_lines(), 2);
        assert_eq!(ix.line_span(grown, 1, grown.len()), Some((6, 11)));
    }

    #[test]
    fn empty_is_zero_lines() {
        let mut ix = Indexer::new();
        ix.step(b"", 0);
        assert_eq!(ix.total_lines(), 0);
        assert!(ix.is_done(0));
    }

    #[test]
    fn checkpoint_index_does_not_store_every_line_start() {
        let mut text = String::new();
        for i in 0..32 {
            text.push_str(&format!("line-{i}\n"));
        }
        let bytes = text.as_bytes();
        let mut ix = Indexer::with_checkpoint_stride(8);

        ix.step(bytes, bytes.len());

        assert_eq!(ix.total_lines(), 32);
        assert_eq!(ix.checkpoint_count(), 4);
        assert!(
            ix.checkpoint_count() < ix.total_lines(),
            "checkpoint index must not keep one u64 offset per line"
        );
    }

    #[test]
    fn checkpoint_line_spans_are_correct_across_blocks() {
        let bytes = b"aa\nbbbb\nc\nddddd\nee\nfffff\ng\nhhhh\n";
        let mut ix = Indexer::with_checkpoint_stride(3);

        ix.step(bytes, bytes.len());

        assert_eq!(ix.line_span(bytes, 0, bytes.len()), Some((0, 3)));
        assert_eq!(ix.line_span(bytes, 2, bytes.len()), Some((8, 10)));
        assert_eq!(ix.line_span(bytes, 5, bytes.len()), Some((19, 25)));
        assert_eq!(ix.line_span(bytes, 7, bytes.len()), Some((27, 32)));
        assert_eq!(ix.line_span(bytes, 8, bytes.len()), None);
    }

    #[test]
    fn checkpoint_line_spans_scan_ranges_sequentially() {
        let bytes = b"aa\nbbbb\nc\nddddd\nee\nfffff\ng\nhhhh\n";
        let mut ix = Indexer::with_checkpoint_stride(3);

        ix.step(bytes, bytes.len());

        assert_eq!(
            ix.line_spans(bytes, 2, 6, bytes.len()),
            vec![(8, 10), (10, 16), (16, 19), (19, 25)]
        );
    }

    #[test]
    fn controllable_span_scan_reports_the_first_unvisited_line() {
        let bytes = b"zero\none\ntwo\nthree\n";
        let mut indexer = Indexer::new();
        indexer.step(bytes, bytes.len());
        let mut visited = Vec::new();
        let next = indexer.for_each_line_span_while(
            bytes,
            0,
            indexer.total_lines(),
            bytes.len(),
            |line, _, _| {
                visited.push(line);
                line < 1
            },
        );
        assert_eq!(visited, vec![0, 1]);
        assert_eq!(next, 2);
    }

    #[test]
    fn prefix_span_scan_leaves_the_rejected_line_for_the_next_step() {
        let bytes = b"one\ntwo\nthree\n";
        let mut indexer = Indexer::new();
        indexer.step(bytes, bytes.len());
        let mut visited = Vec::new();

        let next = indexer.for_each_line_span_prefix(
            bytes,
            0,
            indexer.total_lines(),
            bytes.len(),
            |line, start, end| {
                if line == 1 {
                    return false;
                }
                visited.push((line, start, end));
                true
            },
        );

        assert_eq!(next, 1);
        assert_eq!(visited, vec![(0, 0, 4)]);
    }
}
