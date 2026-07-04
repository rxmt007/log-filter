use crate::indexer::{line_span, Indexer};
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line;
use std::path::Path;

pub struct Session {
    source: MmapSource,
    indexer: Indexer,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Session> {
        let source = MmapSource::open(path)?;
        Ok(Session {
            source,
            indexer: Indexer::new(),
        })
    }

    pub fn total_bytes(&self) -> usize {
        self.source.len()
    }

    pub fn indexed_bytes(&self) -> usize {
        self.indexer.cursor()
    }

    pub fn total_lines(&self) -> usize {
        self.indexer.total_lines()
    }

    pub fn is_indexing_done(&self) -> bool {
        self.indexer.is_done(self.source.len())
    }

    /// 后台按预算步进索引;返回是否已完成。
    pub fn index_step(&mut self, budget: usize) -> bool {
        self.indexer.step(self.source.bytes(), budget);
        self.is_indexing_done()
    }

    /// 测试/小文件:一次性建完索引。
    pub fn index_all(&mut self) {
        let total = self.source.len();
        self.indexer.step(self.source.bytes(), total);
    }

    /// 取 [start, start+count) 行(按已建索引裁剪),返回 (行号1-indexed, 解析结果)。
    pub fn get_rows(&self, start: usize, count: usize) -> Vec<(u64, LogEntry)> {
        // 索引进行中时,最后一行尚未见到换行,真实结尾未知;用已索引前沿(cursor)兜底,
        // 避免把"尚未索引的整段剩余"当成一行(会违反"只传可见窗口"铁律)。
        let frontier = if self.is_indexing_done() {
            self.source.len()
        } else {
            self.indexer.cursor()
        };
        let offsets = self.indexer.offsets();
        let end = start.saturating_add(count).min(offsets.len());
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for i in start..end {
            let (s, e) = line_span(offsets, i, frontier).expect("i in range");
            let text = String::from_utf8_lossy(&self.source.bytes()[s..e]);
            out.push((i as u64 + 1, parse_line(&text)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_log() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "04-20 12:06:02.125   146   179 D BatteryService: update start").unwrap();
        writeln!(f, "04-17 09:01:18.910 D/LightsService(  139): BKL : 106").unwrap();
        writeln!(f, "--------- beginning of main").unwrap();
        f
    }

    #[test]
    fn opens_indexes_and_reads_rows() {
        let f = temp_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        assert_eq!(s.total_lines(), 3);

        let rows = s.get_rows(0, 100);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, 1); // 行号 1-indexed
        assert_eq!(rows[0].1.tag, "BatteryService");
        assert_eq!(rows[1].1.tag, "LightsService");
        assert_eq!(rows[2].1.message, "--------- beginning of main");
    }

    #[test]
    fn get_rows_clamps_range() {
        let f = temp_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let rows = s.get_rows(2, 100);
        assert_eq!(rows.len(), 1); // 仅第 3 行
        assert_eq!(rows[0].0, 3);
    }

    #[test]
    fn frontier_row_not_spanning_to_eof_while_indexing() {
        // C2 回归:索引未完成时,前沿那一行不能把"未索引的整段剩余"吞成一行。
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for _ in 0..2000 {
            writeln!(f, "04-20 12:06:02.125   146   179 D T: msg").unwrap();
        }
        let mut s = Session::open(f.path()).unwrap();
        s.index_step(100); // 只索引一小段,未完成
        assert!(!s.is_indexing_done());
        let n = s.total_lines();
        let rows = s.get_rows(n.saturating_sub(1), 1); // 前沿那一行
        assert!(
            rows[0].1.message.len() < 1024,
            "frontier row leaked {} bytes",
            rows[0].1.message.len()
        );
    }
}
