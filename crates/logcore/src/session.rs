use crate::bookmarks::{BookmarkDirection, BookmarkStore};
use crate::export::{write_raw_line, ExportSummary};
use crate::filter::{FilterError, FilterMatcher, FilterSpec};
use crate::indexer::{line_span, Indexer};
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line;
use crate::search::{
    next_match, SearchDirection, SearchError, SearchMatcher, SearchSpec, SearchSummary,
};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowsView {
    All,
    Filtered,
    Bookmarks,
    Errors,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minimap {
    pub bookmarks: Vec<usize>,
    pub errors: Vec<usize>,
}

pub struct Session {
    source_path: PathBuf,
    source: MmapSource,
    indexer: Indexer,
    filtered: Vec<u64>,
    filter_active: bool,
    filter_spec: FilterSpec,
    search_matches: Vec<u64>,
    bookmarks: BookmarkStore,
    error_lines: Vec<u64>,
    error_scan_lines: usize,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Session> {
        let source = MmapSource::open(path)?;
        let bookmarks = BookmarkStore::load_for_source(path).unwrap_or_default();
        Ok(Session {
            source_path: path.to_path_buf(),
            source,
            indexer: Indexer::new(),
            filtered: Vec::new(),
            filter_active: false,
            filter_spec: FilterSpec::default(),
            search_matches: Vec::new(),
            bookmarks,
            error_lines: Vec::new(),
            error_scan_lines: 0,
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
        self.refresh_error_lines();
        self.is_indexing_done()
    }

    /// 测试/小文件:一次性建完索引。
    pub fn index_all(&mut self) {
        let total = self.source.len();
        self.indexer.step(self.source.bytes(), total);
        self.refresh_error_lines();
    }

    pub fn toggle_bookmark(&mut self, line_no: u64) -> io::Result<bool> {
        let marked = self.bookmarks.toggle(line_no);
        self.bookmarks.save_for_source(&self.source_path)?;
        Ok(marked)
    }

    pub fn is_bookmarked(&self, line_no: u64) -> bool {
        self.bookmarks.contains(line_no)
    }

    pub fn list_bookmarks(&self) -> Vec<u64> {
        self.bookmarks.list()
    }

    pub fn bookmark_count(&self) -> usize {
        self.bookmark_source_lines().len()
    }

    pub fn error_count(&self) -> usize {
        self.error_lines.len()
    }

    pub fn next_bookmark(&self, from_line_no: u64, direction: BookmarkDirection) -> Option<u64> {
        self.bookmarks.next(from_line_no, direction)
    }

    pub fn minimap(&self, buckets: usize) -> Minimap {
        if buckets == 0 || self.total_lines() == 0 {
            return Minimap {
                bookmarks: Vec::new(),
                errors: Vec::new(),
            };
        }
        let total = self.total_lines();
        let bookmarks = self
            .bookmark_source_lines()
            .into_iter()
            .filter_map(|line| bucket_for_zero_based((line - 1) as usize, total, buckets))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let errors = self
            .error_lines
            .iter()
            .filter_map(|idx| bucket_for_zero_based(*idx as usize, total, buckets))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Minimap { bookmarks, errors }
    }

    pub fn export_view(&mut self, view: RowsView, output: &Path) -> io::Result<ExportSummary> {
        self.prepare_file_tool()?;
        let mut writer = self.create_export_file(output)?;
        let frontier = self.indexed_frontier();
        let effective_view = if view == RowsView::Filtered && !self.filter_active {
            RowsView::All
        } else {
            view
        };
        let mut summary = ExportSummary {
            written_lines: 0,
            written_bytes: 0,
        };

        match effective_view {
            RowsView::All => {
                for source_idx in 0..self.indexer.offsets().len() {
                    self.write_source_line(source_idx, frontier, &mut writer, &mut summary)?;
                }
            }
            RowsView::Filtered => {
                for source_idx in &self.filtered {
                    self.write_source_line(
                        *source_idx as usize,
                        frontier,
                        &mut writer,
                        &mut summary,
                    )?;
                }
            }
            RowsView::Bookmarks => {
                for line_no in self.bookmark_source_lines() {
                    self.write_source_line(
                        (line_no - 1) as usize,
                        frontier,
                        &mut writer,
                        &mut summary,
                    )?;
                }
            }
            RowsView::Errors => {
                for source_idx in &self.error_lines {
                    self.write_source_line(
                        *source_idx as usize,
                        frontier,
                        &mut writer,
                        &mut summary,
                    )?;
                }
            }
        }

        Ok(summary)
    }

    pub fn export_range(
        &mut self,
        start_line_no: u64,
        end_line_no: u64,
        output: &Path,
    ) -> io::Result<ExportSummary> {
        self.prepare_file_tool()?;
        if start_line_no == 0 || end_line_no < start_line_no {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "export range must be 1-based and ascending",
            ));
        }
        let mut writer = self.create_export_file(output)?;
        let frontier = self.indexed_frontier();
        let total = self.indexer.offsets().len() as u64;
        let start = start_line_no.min(total + 1);
        let end = end_line_no.min(total);
        let mut summary = ExportSummary {
            written_lines: 0,
            written_bytes: 0,
        };

        for line_no in start..=end {
            self.write_source_line((line_no - 1) as usize, frontier, &mut writer, &mut summary)?;
        }

        Ok(summary)
    }

    pub fn set_filter(&mut self, spec: &FilterSpec) -> Result<usize, FilterError> {
        self.filter_spec = spec.clone();
        if !spec.is_active() {
            self.filtered.clear();
            self.filter_active = false;
            return Ok(self.total_lines());
        }
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
        self.filter_active = true;
        Ok(count)
    }

    pub fn filtered_count(&self) -> usize {
        if self.filter_active {
            self.filtered.len()
        } else {
            self.total_lines()
        }
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
        let effective_view = if view == RowsView::Filtered && !self.filter_active {
            RowsView::All
        } else {
            view
        };
        let bookmark_lines = if effective_view == RowsView::Bookmarks {
            self.bookmark_source_lines()
        } else {
            Vec::new()
        };
        let view_len = match effective_view {
            RowsView::All => self.indexer.offsets().len(),
            RowsView::Filtered => self.filtered.len(),
            RowsView::Bookmarks => bookmark_lines.len(),
            RowsView::Errors => self.error_lines.len(),
        };
        let end = start.saturating_add(count).min(view_len);
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        for view_idx in start..end {
            let source_idx = match effective_view {
                RowsView::All => view_idx,
                RowsView::Filtered => self.filtered[view_idx] as usize,
                RowsView::Bookmarks => (bookmark_lines[view_idx] - 1) as usize,
                RowsView::Errors => self.error_lines[view_idx] as usize,
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

    fn source_line_bytes(&self, source_idx: usize, frontier: usize) -> Option<&[u8]> {
        let offsets = self.indexer.offsets();
        let (start, end) = line_span(offsets, source_idx, frontier)?;
        Some(&self.source.bytes()[start..end])
    }

    fn write_source_line(
        &self,
        source_idx: usize,
        frontier: usize,
        writer: &mut File,
        summary: &mut ExportSummary,
    ) -> io::Result<()> {
        if let Some(bytes) = self.source_line_bytes(source_idx, frontier) {
            summary.written_bytes += write_raw_line(writer, bytes)?;
            summary.written_lines += 1;
        }
        Ok(())
    }

    fn create_export_file(&self, output: &Path) -> io::Result<File> {
        if self.is_source_path(output) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "export output must differ from source file",
            ));
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        File::create(output)
    }

    fn is_source_path(&self, output: &Path) -> bool {
        if output == self.source_path {
            return true;
        }
        match (
            fs::canonicalize(output),
            fs::canonicalize(&self.source_path),
        ) {
            (Ok(out), Ok(source)) => out == source,
            _ => false,
        }
    }

    fn prepare_file_tool(&mut self) -> io::Result<()> {
        self.index_all();
        if self.filter_spec.is_active() {
            let spec = self.filter_spec.clone();
            self.set_filter(&spec).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("cannot rebuild filter for export: {}", err.message),
                )
            })?;
        }
        Ok(())
    }

    fn refresh_error_lines(&mut self) {
        let frontier = self.indexed_frontier();
        let total = self.indexer.offsets().len();
        for idx in self.error_scan_lines..total {
            if let Some((_, entry)) = self.parse_source_row(idx, frontier) {
                if matches!(entry.level.as_str(), "E" | "F") {
                    self.error_lines.push(idx as u64);
                }
            }
        }
        self.error_scan_lines = total;
    }

    fn bookmark_source_lines(&self) -> Vec<u64> {
        let max = self.total_lines() as u64;
        self.bookmarks
            .list()
            .into_iter()
            .filter(|line| *line > 0 && *line <= max)
            .collect()
    }
}

fn bucket_for_zero_based(index: usize, total: usize, buckets: usize) -> Option<usize> {
    if total == 0 || buckets == 0 || index >= total {
        return None;
    }
    Some(((index * buckets) / total).min(buckets - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::BookmarkDirection;
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
    fn default_filter_does_not_materialize_all_line_numbers() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        assert_eq!(s.set_filter(&FilterSpec::default()).unwrap(), 4);
        assert_eq!(s.filtered_count(), 4);
        assert_eq!(s.filtered.len(), 0);

        let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[3].0, 4);
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
    fn bookmarks_persist_and_bookmark_view_uses_original_lines() {
        let f = temp_filter_log();
        {
            let mut s = Session::open(f.path()).unwrap();
            s.index_all();
            assert!(s.toggle_bookmark(2).unwrap());
            assert!(s.toggle_bookmark(4).unwrap());
            assert_eq!(s.list_bookmarks(), vec![2, 4]);

            let rows = s.get_rows_for_view(RowsView::Bookmarks, 0, 10);
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].0, 2);
            assert_eq!(rows[0].1.tag, "Network");
            assert_eq!(rows[1].0, 4);
            assert_eq!(rows[1].1.tag, "Payment");
        }

        let mut reopened = Session::open(f.path()).unwrap();
        reopened.index_all();
        assert_eq!(reopened.list_bookmarks(), vec![2, 4]);
    }

    #[test]
    fn next_bookmark_wraps_one_based_lines() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.toggle_bookmark(2).unwrap();
        s.toggle_bookmark(4).unwrap();

        assert_eq!(s.next_bookmark(2, BookmarkDirection::Next), Some(4));
        assert_eq!(s.next_bookmark(4, BookmarkDirection::Next), Some(2));
        assert_eq!(s.next_bookmark(2, BookmarkDirection::Previous), Some(4));
    }

    #[test]
    fn error_view_returns_error_and_fatal_rows() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        let rows = s.get_rows_for_view(RowsView::Errors, 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 4);
        assert_eq!(rows[0].1.level, "E");
    }

    #[test]
    fn minimap_returns_bookmark_and_error_buckets() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.toggle_bookmark(2).unwrap();

        let map = s.minimap(4);
        assert_eq!(map.bookmarks, vec![1]);
        assert_eq!(map.errors, vec![3]);
    }

    #[test]
    fn export_range_writes_original_source_lines() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let out = tempfile::NamedTempFile::new().unwrap();

        let summary = s.export_range(2, 3, out.path()).unwrap();

        assert_eq!(summary.written_lines, 2);
        let text = std::fs::read_to_string(out.path()).unwrap();
        assert_eq!(
            text,
            "04-20 12:06:02.225   200   220 I Network: GET /home ok\n04-20 12:06:02.325   200   221 W Network: slow request\n"
        );
    }

    #[test]
    fn export_filtered_view_writes_only_matching_source_lines() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.set_filter(&FilterSpec {
            tag_include: FilterField::plain(true, "Network"),
            ..Default::default()
        })
        .unwrap();
        let out = tempfile::NamedTempFile::new().unwrap();

        let summary = s.export_view(RowsView::Filtered, out.path()).unwrap();

        assert_eq!(summary.written_lines, 2);
        let text = std::fs::read_to_string(out.path()).unwrap();
        assert!(text.contains("GET /home ok"));
        assert!(text.contains("slow request"));
        assert!(!text.contains("SocketTimeoutException"));
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
