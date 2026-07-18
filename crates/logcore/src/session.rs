use crate::bookmarks::{BookmarkDirection, BookmarkStore};
use crate::encoding::{ResolvedTextEncoding, TextEncoding};
use crate::export::{write_raw_line, ExportSummary};
use crate::filter::{FilterError, FilterMatcher, FilterSpec};
use crate::indexer::{line_span, Indexer};
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line_ref;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultTarget {
    pub line_no: u64,
    pub result_index: usize,
}

pub struct Session {
    source_path: PathBuf,
    source: MmapSource,
    indexer: Indexer,
    filtered: Vec<u64>,
    filter_active: bool,
    filter_spec: FilterSpec,
    search_matches: Vec<u64>,
    search_spec: Option<SearchSpec>,
    bookmarks: BookmarkStore,
    error_lines: Vec<u64>,
    error_scan_lines: usize,
    encoding: ResolvedTextEncoding,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Session> {
        Self::open_with_encoding(path, TextEncoding::Utf8)
    }

    pub fn open_with_encoding(path: &Path, encoding: TextEncoding) -> std::io::Result<Session> {
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
            search_spec: None,
            bookmarks,
            error_lines: Vec::new(),
            error_scan_lines: 0,
            encoding: encoding.resolve(),
        })
    }

    pub fn set_encoding(&mut self, encoding: TextEncoding) {
        self.encoding = encoding.resolve();
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

    /// 重新映射源文件。用于 adb/logcat 等会增长的会话文件。
    pub fn remap_source(&mut self) -> io::Result<()> {
        self.source = MmapSource::open(&self.source_path)?;
        Ok(())
    }

    pub fn remap_and_index_step(&mut self, budget: usize) -> io::Result<bool> {
        self.remap_source()?;
        Ok(self.index_step(budget))
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

    pub fn next_bookmark_in_current_result(
        &self,
        from_line_no: u64,
        direction: BookmarkDirection,
    ) -> Option<ResultTarget> {
        let mut targets = self
            .bookmark_source_lines()
            .into_iter()
            .filter_map(|line_no| {
                let source_idx = line_no.saturating_sub(1);
                self.current_result_index_for_source_idx(source_idx)
                    .map(|result_index| ResultTarget {
                        line_no,
                        result_index,
                    })
            })
            .collect::<Vec<_>>();
        if targets.is_empty() {
            return None;
        }
        targets.sort_by_key(|target| target.result_index);
        let from_source_idx = from_line_no.saturating_sub(1);
        let from_result_idx = self
            .current_result_index_for_source_idx(from_source_idx)
            .unwrap_or(0);
        match direction {
            BookmarkDirection::Next => {
                let idx = targets
                    .iter()
                    .position(|target| target.result_index > from_result_idx)
                    .unwrap_or(0);
                Some(targets[idx])
            }
            BookmarkDirection::Previous => {
                let idx = targets
                    .iter()
                    .rposition(|target| target.result_index < from_result_idx)
                    .unwrap_or(targets.len() - 1);
                Some(targets[idx])
            }
        }
    }

    pub fn minimap(&self, buckets: usize) -> Minimap {
        let total = self.current_result_len();
        if buckets == 0 || total == 0 {
            return Minimap {
                bookmarks: Vec::new(),
                errors: Vec::new(),
            };
        }
        if !self.filter_active {
            return self.source_minimap(buckets);
        }
        // 反向遍历:书签/错误行是小集合,逐个二分反查在过滤结果中的位置,
        // 避免 O(过滤结果总数) 的全量扫描(minimap 会被状态事件高频触发)。
        let bookmarks = self
            .bookmark_source_lines()
            .into_iter()
            .filter_map(|line_no| self.filtered.binary_search(&(line_no - 1)).ok())
            .filter_map(|result_idx| bucket_for_zero_based(result_idx, total, buckets))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let errors = self
            .error_lines
            .iter()
            .filter_map(|idx| self.filtered.binary_search(idx).ok())
            .filter_map(|result_idx| bucket_for_zero_based(result_idx, total, buckets))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Minimap { bookmarks, errors }
    }

    fn current_result_len(&self) -> usize {
        self.filtered_count()
    }

    fn current_result_index_for_source_idx(&self, source_idx: u64) -> Option<usize> {
        if self.filter_active {
            self.filtered.binary_search(&source_idx).ok()
        } else if (source_idx as usize) < self.indexer.total_lines() {
            Some(source_idx as usize)
        } else {
            None
        }
    }

    pub fn result_index_for_line_no(&self, line_no: u64) -> Option<usize> {
        if line_no == 0 {
            return None;
        }
        self.current_result_index_for_source_idx(line_no - 1)
    }

    fn source_minimap(&self, buckets: usize) -> Minimap {
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
                for source_idx in 0..self.indexer.total_lines() {
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
        let total = self.indexer.total_lines() as u64;
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
        if let Some(count) = self.set_filter_pending(spec)? {
            return Ok(count);
        }
        let matcher = FilterMatcher::new(spec)?;
        let matches = self.filter_indexed_range(&matcher, 0, self.total_lines());
        Ok(self.apply_filter_results(spec, matches))
    }

    pub fn set_filter_pending(&mut self, spec: &FilterSpec) -> Result<Option<usize>, FilterError> {
        self.filter_spec = spec.clone();
        if !spec.is_active() {
            self.filtered.clear();
            self.filter_active = false;
            return Ok(Some(self.total_lines()));
        }
        FilterMatcher::new(spec)?;
        Ok(None)
    }

    pub fn active_filter_spec(&self) -> Option<FilterSpec> {
        self.filter_spec
            .is_active()
            .then(|| self.filter_spec.clone())
    }

    pub fn filter_indexed_range(
        &self,
        matcher: &FilterMatcher,
        start: usize,
        end: usize,
    ) -> Vec<u64> {
        let frontier = self.indexed_frontier();
        let end = end.min(self.total_lines());
        let mut matches = Vec::new();
        for (idx, (span_start, span_end)) in (start.min(end)..end).zip(self.indexer.line_spans(
            self.source.bytes(),
            start,
            end,
            frontier,
        )) {
            let text = self.encoding.decode(&self.source.bytes()[span_start..span_end]);
            let entry = parse_line_ref(&text);
            let marked = matcher.requires_mark() && self.is_bookmarked(idx as u64 + 1);
            if matcher.is_match_with_mark(&entry, marked) {
                matches.push(idx as u64);
            }
        }
        matches
    }

    pub fn apply_filter_results(&mut self, spec: &FilterSpec, matches: Vec<u64>) -> usize {
        self.filter_spec = spec.clone();
        if !spec.is_active() {
            self.filtered.clear();
            self.filter_active = false;
            return self.total_lines();
        }
        let count = matches.len();
        self.filtered = matches;
        self.filter_active = true;
        count
    }

    pub fn append_filter_results(&mut self, spec: &FilterSpec, matches: Vec<u64>) -> usize {
        if !spec.is_active() {
            self.filtered.clear();
            self.filter_active = false;
            return self.total_lines();
        }
        self.filter_spec = spec.clone();
        self.filter_active = true;
        self.filtered.extend(matches);
        self.filtered.sort_unstable();
        self.filtered.dedup();
        self.filtered.len()
    }

    pub fn filtered_count(&self) -> usize {
        if self.filter_active {
            self.filtered.len()
        } else {
            self.total_lines()
        }
    }

    pub fn search(&mut self, spec: &SearchSpec) -> Result<SearchSummary, SearchError> {
        if !self.set_search_pending(spec)? {
            return Ok(SearchSummary::from_matches(&self.search_matches));
        }
        let matcher = SearchMatcher::new(spec)?;
        let matches = self.search_indexed_range(&matcher, 0, self.total_lines());
        Ok(self.apply_search_results(spec, matches))
    }

    pub fn set_search_pending(&mut self, spec: &SearchSpec) -> Result<bool, SearchError> {
        if spec.query.is_empty() {
            self.search_spec = None;
            self.search_matches.clear();
            return Ok(false);
        }
        SearchMatcher::new(spec)?;
        self.search_spec = Some(spec.clone());
        Ok(true)
    }

    pub fn active_search_spec(&self) -> Option<SearchSpec> {
        self.search_spec
            .clone()
            .filter(|spec| !spec.query.is_empty())
    }

    pub fn search_indexed_range(
        &self,
        matcher: &SearchMatcher,
        start: usize,
        end: usize,
    ) -> Vec<u64> {
        let frontier = self.indexed_frontier();
        let end = end.min(self.total_lines());
        let mut matches = Vec::new();
        for (idx, (span_start, span_end)) in (start.min(end)..end).zip(self.indexer.line_spans(
            self.source.bytes(),
            start,
            end,
            frontier,
        )) {
            let text = self.encoding.decode(&self.source.bytes()[span_start..span_end]);
            let entry = parse_line_ref(&text);
            if matcher.is_entry_match(&entry) {
                matches.push(idx as u64);
            }
        }
        matches
    }

    pub fn apply_search_results(&mut self, spec: &SearchSpec, matches: Vec<u64>) -> SearchSummary {
        self.search_spec = (!spec.query.is_empty()).then(|| spec.clone());
        self.search_matches = matches;
        SearchSummary {
            count: self.search_matches.len(),
            first: self.search_matches.first().map(|idx| idx + 1),
        }
    }

    pub fn append_search_results(&mut self, spec: &SearchSpec, matches: Vec<u64>) -> SearchSummary {
        self.search_spec = (!spec.query.is_empty()).then(|| spec.clone());
        self.search_matches.extend(matches);
        self.search_matches.sort_unstable();
        self.search_matches.dedup();
        SearchSummary {
            count: self.search_matches.len(),
            first: self.search_matches.first().map(|idx| idx + 1),
        }
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
            RowsView::All => self.indexer.total_lines(),
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
                RowsView::Bookmarks => {
                    let Some(line_no) = bookmark_lines.get(view_idx) else {
                        continue;
                    };
                    (line_no - 1) as usize
                }
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
        let (start, end) = line_span(&self.indexer, self.source.bytes(), source_idx, frontier)?;
        Some((source_idx as u64 + 1, self.parse_source_span(start, end)))
    }

    fn parse_source_span(&self, start: usize, end: usize) -> LogEntry {
        let text = self.encoding.decode(&self.source.bytes()[start..end]);
        LogEntry::from(parse_line_ref(&text))
    }

    fn source_line_bytes(&self, source_idx: usize, frontier: usize) -> Option<&[u8]> {
        let (start, end) = line_span(&self.indexer, self.source.bytes(), source_idx, frontier)?;
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
        let total = self.indexer.total_lines();
        let bytes = self.source.bytes();
        for (idx, (span_start, span_end)) in (self.error_scan_lines..total).zip(
            self.indexer
                .line_spans(bytes, self.error_scan_lines, total, frontier),
        ) {
            if matches!(
                crate::parser::level_byte_of_line(&bytes[span_start..span_end]),
                Some(b'E') | Some(b'F')
            ) {
                self.error_lines.push(idx as u64);
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
    use crate::filter::{FilterField, FilterMatcher, FilterSpec, LevelMask};
    use crate::search::{SearchDirection, SearchSpec};
    use std::io::Write;
    use std::time::Instant;

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
    fn marked_only_filter_intersects_with_levels() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.toggle_bookmark(4).unwrap();

        let count = s
            .set_filter(&FilterSpec {
                levels: LevelMask::from_levels(&["E", "F"]),
                marked_only: true,
                ..Default::default()
            })
            .unwrap();

        assert_eq!(count, 1);
        let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 4);
        assert_eq!(rows[0].1.level, "E");
    }

    #[test]
    fn next_bookmark_uses_current_result_order() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.toggle_bookmark(2).unwrap();
        s.toggle_bookmark(4).unwrap();
        s.set_filter(&FilterSpec {
            levels: LevelMask::from_levels(&["E", "F"]),
            ..Default::default()
        })
        .unwrap();

        let target = s
            .next_bookmark_in_current_result(1, BookmarkDirection::Next)
            .unwrap();
        assert_eq!(target.line_no, 4);
        assert_eq!(target.result_index, 0);
    }

    #[test]
    fn minimap_uses_current_filtered_result_buckets() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..4 {
            writeln!(f, "04-20 12:06:02.{i:03}   300   330 E Payment: error {i}").unwrap();
        }
        for i in 4..8 {
            writeln!(f, "04-20 12:06:02.{i:03}   300   330 I Payment: info {i}").unwrap();
        }
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.set_filter(&FilterSpec {
            levels: LevelMask::from_levels(&["E", "F"]),
            ..Default::default()
        })
        .unwrap();

        let map = s.minimap(4);
        assert_eq!(map.errors, vec![0, 1, 2, 3]);
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
    fn pending_filter_spec_is_available_for_index_completion_rerun() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let spec = FilterSpec {
            tag_include: FilterField::plain(true, "Network"),
            ..Default::default()
        };

        assert_eq!(s.set_filter_pending(&spec).unwrap(), None);
        assert_eq!(s.active_filter_spec(), Some(spec.clone()));

        let matcher = FilterMatcher::new(&spec).unwrap();
        let matches = s.filter_indexed_range(&matcher, 0, s.total_lines());
        assert_eq!(s.apply_filter_results(&spec, matches), 2);
    }

    #[test]
    fn empty_search_clears_active_search_spec_and_matches() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let spec = SearchSpec::plain("Network");
        assert!(s.set_search_pending(&spec).unwrap());
        assert_eq!(s.active_search_spec(), Some(spec));

        let empty = SearchSpec::plain("");
        assert!(!s.set_search_pending(&empty).unwrap());
        assert_eq!(s.active_search_spec(), None);
        assert_eq!(s.search_next(1, SearchDirection::Next), None);
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
    fn filtered_minimap_marks_only_buckets_containing_hits() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..8 {
            let level = if i == 6 { "E" } else { "I" };
            writeln!(f, "04-20 12:06:02.{i:03}   300   330 {level} Payment: m{i}").unwrap();
        }
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.toggle_bookmark(2).unwrap();
        // 过滤后结果为行 2(书签, result 0)与行 7(错误, result 1)
        s.set_filter(&FilterSpec {
            word_include: FilterField::plain(true, "m1|m6"),
            ..Default::default()
        })
        .unwrap();

        let map = s.minimap(4);
        assert_eq!(map.bookmarks, vec![0]);
        assert_eq!(map.errors, vec![2]);
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

    #[test]
    fn synthetic_large_log_indexing_and_window_reads_stay_fast() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for i in 0..100_000 {
            writeln!(
                f,
                "04-20 12:06:02.{:03}   146   179 D PerfTag: synthetic message {}",
                i % 1000,
                i
            )
            .unwrap();
        }
        let bytes = f.as_file().metadata().unwrap().len();
        let mut s = Session::open(f.path()).unwrap();

        let index_start = Instant::now();
        s.index_all();
        let index_elapsed = index_start.elapsed();
        let throughput = bytes as f64 / index_elapsed.as_secs_f64();
        assert_eq!(s.total_lines(), 100_000);
        assert!(
            throughput > 1_000_000.0,
            "index throughput regressed to {throughput:.0} bytes/sec"
        );

        let read_start = Instant::now();
        let rows = s.get_rows(90_000, 200);
        let read_elapsed = read_start.elapsed();
        assert_eq!(rows.len(), 200);
        assert_eq!(rows[0].0, 90_001);
        assert!(
            read_elapsed.as_millis() < 250,
            "get_rows latency regressed to {:?}",
            read_elapsed
        );
    }

    #[test]
    fn remap_and_index_step_reads_lines_appended_after_trailing_newline() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "04-20 12:06:02.125   146   179 D One: first").unwrap();
        f.flush().unwrap();
        let mut s = Session::open(f.path()).unwrap();

        s.remap_and_index_step(usize::MAX).unwrap();
        assert_eq!(s.total_lines(), 1);

        write!(f, "04-20 12:06:02.225   200   220 I Two: second").unwrap();
        f.flush().unwrap();
        s.remap_and_index_step(usize::MAX).unwrap();

        assert_eq!(s.total_lines(), 2);
        let rows = s.get_rows(1, 1);
        assert_eq!(rows[0].0, 2);
        assert_eq!(rows[0].1.tag, "Two");
    }

    #[test]
    fn appends_filter_results_for_newly_indexed_range() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "04-20 12:06:02.125   146   179 D One: first").unwrap();
        f.flush().unwrap();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let spec = FilterSpec {
            tag_include: FilterField::plain(true, "Two"),
            ..Default::default()
        };
        assert_eq!(s.set_filter(&spec).unwrap(), 0);

        writeln!(f, "04-20 12:06:02.225   200   220 I Two: second").unwrap();
        f.flush().unwrap();
        let previous_total = s.total_lines();
        s.remap_and_index_step(usize::MAX).unwrap();
        let matcher = FilterMatcher::new(&spec).unwrap();
        let matches = s.filter_indexed_range(&matcher, previous_total, s.total_lines());
        assert_eq!(s.append_filter_results(&spec, matches), 1);

        let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 2);
    }
}
