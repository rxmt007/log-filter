use crate::filter::{FilterError, FilterMatcher, FilterSpec};
use crate::indexer::{line_span, Indexer};
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line;
use crate::search::{
    next_match, SearchDirection, SearchError, SearchMatcher, SearchSpec, SearchSummary,
};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowsView {
    All,
    Filtered,
}

pub struct Session {
    source: MmapSource,
    indexer: Indexer,
    filtered: Vec<u64>,
    search_matches: Vec<u64>,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Session> {
        let source = MmapSource::open(path)?;
        Ok(Session {
            source,
            indexer: Indexer::new(),
            filtered: Vec::new(),
            search_matches: Vec::new(),
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

    pub fn set_filter(&mut self, spec: &FilterSpec) -> Result<usize, FilterError> {
        let matcher = FilterMatcher::new(spec)?;
        let frontier = self.indexed_frontier();
        let mut matches = Vec::new();
        for idx in 0..self.indexer.offsets().len() {
            if let Some((_, entry)) = self.parse_source_row(idx, frontier) {
                if matcher.is_match(&entry) {
                    matches.push(idx as u64);
                }
            }
        }
        let count = matches.len();
        self.filtered = matches;
        Ok(count)
    }

    pub fn filtered_count(&self) -> usize {
        self.filtered.len()
    }

    pub fn search(&mut self, spec: &SearchSpec) -> Result<SearchSummary, SearchError> {
        let matcher = SearchMatcher::new(spec)?;
        let frontier = self.indexed_frontier();
        let mut matches = Vec::new();
        for idx in 0..self.indexer.offsets().len() {
            if let Some((_, entry)) = self.parse_source_row(idx, frontier) {
                if matcher.is_entry_match(&entry) {
                    matches.push(idx as u64);
                }
            }
        }
        self.search_matches = matches;
        Ok(SearchSummary {
            count: self.search_matches.len(),
            first: self.search_matches.first().map(|idx| idx + 1),
        })
    }

    pub fn search_next(&self, from_line_no: u64, direction: SearchDirection) -> Option<u64> {
        let zero_based = from_line_no.saturating_sub(1);
        next_match(&self.search_matches, zero_based, direction).map(|idx| idx + 1)
    }

    /// 取 [start, start+count) 行(按已建索引裁剪),返回 (行号1-indexed, 解析结果)。
    pub fn get_rows(&self, start: usize, count: usize) -> Vec<(u64, LogEntry)> {
        self.get_rows_for_view(RowsView::All, start, count)
    }

    /// 取指定视图的 [start, start+count) 行,返回 (原始行号1-indexed, 解析结果)。
    pub fn get_rows_for_view(
        &self,
        view: RowsView,
        start: usize,
        count: usize,
    ) -> Vec<(u64, LogEntry)> {
        // 索引进行中时,最后一行尚未见到换行,真实结尾未知;用已索引前沿(cursor)兜底,
        // 避免把"尚未索引的整段剩余"当成一行(会违反"只传可见窗口"铁律)。
        let frontier = self.indexed_frontier();
        let view_len = match view {
            RowsView::All => self.indexer.offsets().len(),
            RowsView::Filtered => self.filtered.len(),
        };
        let end = start.saturating_add(count).min(view_len);
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for view_idx in start..end {
            let source_idx = match view {
                RowsView::All => view_idx,
                RowsView::Filtered => self.filtered[view_idx] as usize,
            };
            if let Some(row) = self.parse_source_row(source_idx, frontier) {
                out.push(row);
            }
        }
        out
    }

    fn indexed_frontier(&self) -> usize {
        if self.is_indexing_done() {
            self.source.len()
        } else {
            self.indexer.cursor()
        }
    }

    fn parse_source_row(&self, source_idx: usize, frontier: usize) -> Option<(u64, LogEntry)> {
        let offsets = self.indexer.offsets();
        let (start, end) = line_span(offsets, source_idx, frontier)?;
        let text = String::from_utf8_lossy(&self.source.bytes()[start..end]);
        Some((source_idx as u64 + 1, parse_line(&text)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::{FilterField, FilterSpec};
    use crate::search::{SearchDirection, SearchSpec};
    use std::io::Write;

    fn temp_log() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "04-20 12:06:02.125   146   179 D BatteryService: update start"
        )
        .unwrap();
        writeln!(f, "04-17 09:01:18.910 D/LightsService(  139): BKL : 106").unwrap();
        writeln!(f, "--------- beginning of main").unwrap();
        f
    }

    fn temp_filter_log() -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            "04-20 12:06:02.125   146   179 D ActivityManager: Start proc"
        )
        .unwrap();
        writeln!(f, "04-20 12:06:02.225   200   220 I Network: GET /home ok").unwrap();
        writeln!(f, "04-20 12:06:02.325   200   221 W Network: slow request").unwrap();
        writeln!(
            f,
            "04-20 12:06:02.425   300   330 E Payment: SocketTimeoutException"
        )
        .unwrap();
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
    fn filtered_rows_keep_original_line_numbers() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let spec = FilterSpec {
            tag_include: FilterField::plain(true, "Network"),
            ..Default::default()
        };
        assert_eq!(s.set_filter(&spec).unwrap(), 2);

        let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
        assert_eq!(s.filtered_count(), 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 2);
        assert_eq!(rows[0].1.message, "GET /home ok");
        assert_eq!(rows[1].0, 3);
        assert_eq!(rows[1].1.message, "slow request");
    }

    #[test]
    fn session_search_returns_one_based_line_navigation() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        let summary = s.search(&SearchSpec::plain("Network")).unwrap();
        assert_eq!(summary.count, 2);
        assert_eq!(summary.first, Some(2));
        assert_eq!(s.search_next(2, SearchDirection::Next), Some(3));
        assert_eq!(s.search_next(3, SearchDirection::Next), Some(2));
        assert_eq!(s.search_next(2, SearchDirection::Previous), Some(3));
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
