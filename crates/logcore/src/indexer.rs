use memchr::memchr_iter;

/// 增量构建行起始偏移。`offsets[i]` = 第 i 行首字节偏移;行数 = offsets.len()。
pub struct Indexer {
    offsets: Vec<u64>,
    cursor: usize,
}

impl Indexer {
    pub fn new() -> Self {
        Self {
            offsets: Vec::new(),
            cursor: 0,
        }
    }

    /// 从内部 cursor 起,最多处理 `budget` 字节。返回本次处理的字节数。
    pub fn step(&mut self, bytes: &[u8], budget: usize) -> usize {
        if self.offsets.is_empty() && !bytes.is_empty() {
            self.offsets.push(0);
        }
        let end = self.cursor.saturating_add(budget).min(bytes.len());
        let chunk = &bytes[self.cursor..end];
        for pos in memchr_iter(b'\n', chunk) {
            let abs_next = (self.cursor + pos + 1) as u64;
            if (abs_next as usize) < bytes.len() {
                self.offsets.push(abs_next);
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
        self.offsets.len()
    }

    pub fn offsets(&self) -> &[u64] {
        &self.offsets
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self::new()
    }
}

/// 第 i 行的字节区间 [start, end)(end 含末尾换行,取文本时再裁剪)。
pub fn line_span(offsets: &[u64], i: usize, total: usize) -> Option<(usize, usize)> {
    if i >= offsets.len() {
        return None;
    }
    let start = offsets[i] as usize;
    let end = if i + 1 < offsets.len() {
        offsets[i + 1] as usize
    } else {
        total
    };
    Some((start, end))
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
        assert_eq!(ix.offsets(), &[0, 2, 5]);
        assert_eq!(ix.total_lines(), 3);
        assert_eq!(line_span(ix.offsets(), 1, bytes.len()), Some((2, 5)));
        assert_eq!(line_span(ix.offsets(), 2, bytes.len()), Some((5, 8)));
    }

    #[test]
    fn trailing_newline_makes_no_empty_line() {
        let bytes = b"a\nbb\n";
        let mut ix = Indexer::new();
        ix.step(bytes, bytes.len());
        assert_eq!(ix.offsets(), &[0, 2]);
        assert_eq!(ix.total_lines(), 2);
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
        assert_eq!(a.offsets(), b.offsets());
    }

    #[test]
    fn empty_is_zero_lines() {
        let mut ix = Indexer::new();
        ix.step(b"", 0);
        assert_eq!(ix.total_lines(), 0);
        assert!(ix.is_done(0));
    }
}
