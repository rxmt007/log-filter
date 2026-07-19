use crate::bookmarks::{BookmarkDirection, BookmarkStore};
use crate::encoding::{ResolvedTextEncoding, TextEncoding};
use crate::export::ExportSummary;
use crate::filter::{FilterError, FilterMatcher, FilterSpec};
use crate::indexer::Indexer;
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line_ref;
use crate::search::{
    next_match, SearchDirection, SearchError, SearchMatcher, SearchSpec, SearchSummary,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::path::PathBuf;

/// 导出分批行数:每批用批量原语拷进临时缓冲再写盘,界定内存占用。
const EXPORT_CHUNK_LINES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowsView {
    All,
    Filtered,
    Bookmarks,
    Errors,
}

/// 小地图错误刻度:一个桶及桶内错误行数。前端据 count/每桶行数 计算刻度透明度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapBucket {
    pub bucket: usize,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minimap {
    pub bookmarks: Vec<usize>,
    /// 升序,每桶一条,count = 桶内错误行数(密度加权,取代旧的"命中即整桶"二值语义)。
    pub errors: Vec<MinimapBucket>,
}

/// 导出计划:一次持锁产出的**快照**。AllLines 用行号区间(不物化);
/// Indices 是 0-based 源行号数组(Filtered 克隆命中数组;Bookmarks/Errors 转换/克隆小数组)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportPlan {
    AllLines { total: usize },
    Indices(Vec<u32>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultTarget {
    pub line_no: u64,
    pub result_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemapStep {
    /// 源文件收缩(外部截断/轮转)导致派生状态被重建。
    pub reset: bool,
    /// 本步之后索引是否已追平文件末尾。
    pub done: bool,
}

pub struct Session {
    source_path: PathBuf,
    source: MmapSource,
    indexer: Indexer,
    filtered: Vec<u32>,
    filter_active: bool,
    filter_spec: FilterSpec,
    search_matches: Vec<u32>,
    search_spec: Option<SearchSpec>,
    bookmarks: BookmarkStore,
    error_lines: Vec<u32>,
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

    /// 重新映射源文件。文件未增长时跳过(流式 reader 每个读块都会调用,mmap/munmap 不便宜);
    /// 检测到收缩(外部截断/轮转)时旧索引全部失效,重建派生状态,避免越界访问乃至 SIGBUS。
    /// 返回 `true` 当且仅当发生了收缩重建(调用方据此从 0 起重扫过滤/查找)。
    pub fn remap_source(&mut self) -> io::Result<bool> {
        let disk_len = fs::metadata(&self.source_path)?.len() as usize;
        if disk_len < self.source.len() {
            self.source = MmapSource::open(&self.source_path)?;
            self.reset_derived_state();
            return Ok(true);
        }
        if disk_len > self.source.len() {
            self.source = MmapSource::open(&self.source_path)?;
        }
        Ok(false)
    }

    /// 收缩后清空派生状态:新建 Indexer、清 filtered/search/errors、错误扫描游标归零。
    /// `filter_active` 置 false。**重扫由调用方负责**:流式 reader 在 `RemapStep::reset`
    /// 为真时从 0 起重扫已索引区间(`filter_spec` 保留供其重建);`open_file` 路径则在索引
    /// 完成后由 `rerun_scans_after_index_done` 重算。本函数自身不产出任何命中。
    fn reset_derived_state(&mut self) {
        self.indexer = Indexer::new();
        self.filtered.clear();
        self.filter_active = false;
        self.search_matches.clear();
        self.error_lines.clear();
        self.error_scan_lines = 0;
    }

    pub fn remap_and_index_step(&mut self, budget: usize) -> io::Result<RemapStep> {
        let reset = self.remap_source()?;
        let done = self.index_step(budget);
        Ok(RemapStep { reset, done })
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
            // 超过 u32 的书签行不可能在命中数组内(命中数组元素都 ≤ u32::MAX)。
            .filter_map(|line_no| u32::try_from(line_no - 1).ok())
            .filter_map(|needle| self.filtered.binary_search(&needle).ok())
            .filter_map(|result_idx| bucket_for_zero_based(result_idx, total, buckets))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // 错误按桶累加:反查过滤结果中的位置,再定位其所在桶,同桶多条错误 count 递增。
        let mut error_counts: BTreeMap<usize, u32> = BTreeMap::new();
        for idx in &self.error_lines {
            if let Ok(result_idx) = self.filtered.binary_search(idx) {
                if let Some(bucket) = bucket_for_zero_based(result_idx, total, buckets) {
                    *error_counts.entry(bucket).or_insert(0) += 1;
                }
            }
        }
        let errors = collect_minimap_buckets(error_counts);
        Minimap { bookmarks, errors }
    }

    fn current_result_len(&self) -> usize {
        self.filtered_count()
    }

    fn current_result_index_for_source_idx(&self, source_idx: u64) -> Option<usize> {
        if self.filter_active {
            // 超过 u32 的 source_idx 不可能在命中数组内(命中数组元素都 ≤ u32::MAX)。
            let needle = u32::try_from(source_idx).ok()?;
            self.filtered.binary_search(&needle).ok()
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
        let mut error_counts: BTreeMap<usize, u32> = BTreeMap::new();
        for idx in &self.error_lines {
            if let Some(bucket) = bucket_for_zero_based(*idx as usize, total, buckets) {
                *error_counts.entry(bucket).or_insert(0) += 1;
            }
        }
        let errors = collect_minimap_buckets(error_counts);
        Minimap { bookmarks, errors }
    }

    pub fn export_view(&mut self, view: RowsView, output: &Path) -> io::Result<ExportSummary> {
        self.prepare_file_tool()?;
        let mut writer = self.create_export_file(output)?;
        let effective_view = if view == RowsView::Filtered && !self.filter_active {
            RowsView::All
        } else {
            view
        };
        let mut summary = ExportSummary {
            written_lines: 0,
            written_bytes: 0,
        };

        let mut buf = Vec::new();
        match effective_view {
            RowsView::All => {
                self.write_line_range(
                    0,
                    self.indexer.total_lines(),
                    &mut buf,
                    &mut writer,
                    &mut summary,
                )?;
            }
            RowsView::Filtered => {
                self.write_sorted_lines(&self.filtered, &mut buf, &mut writer, &mut summary)?;
            }
            RowsView::Bookmarks => {
                let indices: Vec<u32> = self
                    .bookmark_source_lines()
                    .into_iter()
                    .filter_map(|line_no| u32::try_from(line_no - 1).ok())
                    .collect();
                self.write_sorted_lines(&indices, &mut buf, &mut writer, &mut summary)?;
            }
            RowsView::Errors => {
                self.write_sorted_lines(&self.error_lines, &mut buf, &mut writer, &mut summary)?;
            }
        }

        writer.flush()?;
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
        let total = self.indexer.total_lines() as u64;
        let start = start_line_no.min(total + 1);
        let end = end_line_no.min(total);
        let mut summary = ExportSummary {
            written_lines: 0,
            written_bytes: 0,
        };

        // 1-based [start, end] → 0-based [start-1, end);空区间(start>end)输出 0 行。
        let first = start.saturating_sub(1) as usize;
        let count = (end + 1).saturating_sub(start) as usize;
        let mut buf = Vec::new();
        self.write_line_range(first, count, &mut buf, &mut writer, &mut summary)?;

        writer.flush()?;
        Ok(summary)
    }

    /// 对指定视图产出导出快照。Filtered 且过滤未激活时按 All 处理(与 export_view 一致)。
    /// 注意:Filtered 激活时返回**当前** filtered 的克隆;调用方若需要"完整重算"语义,
    /// 应先用 FilterMatcher 分块重算出局部数组,不使用本方法的 Filtered 分支。
    pub fn export_plan_snapshot(&self, view: RowsView) -> ExportPlan {
        match view {
            RowsView::All => ExportPlan::AllLines {
                total: self.total_lines(),
            },
            RowsView::Filtered => {
                if self.filter_active {
                    ExportPlan::Indices(self.filtered.clone())
                } else {
                    ExportPlan::AllLines {
                        total: self.total_lines(),
                    }
                }
            }
            RowsView::Bookmarks => ExportPlan::Indices(
                self.bookmark_source_lines()
                    .into_iter()
                    // 转 0-based;超过 u32 的行号不可能是有效源行号,跳过。
                    .filter_map(|line_no| u32::try_from(line_no - 1).ok())
                    .collect(),
            ),
            RowsView::Errors => ExportPlan::Indices(self.error_lines.clone()),
        }
    }

    /// 把第 source_idx 行(0-based)的原始字节(含行尾换行)追加进 out,返回追加的字节数;
    /// 行不可用(未索引/越界)返回 0。供"锁内拷贝、锁外写盘"的分段导出使用。
    pub fn append_line_bytes(&self, source_idx: usize, out: &mut Vec<u8>) -> u64 {
        let frontier = self.indexed_frontier();
        match self.source_line_bytes(source_idx, frontier) {
            Some(bytes) => {
                out.extend_from_slice(bytes);
                bytes.len() as u64
            }
            None => 0,
        }
    }

    /// 把一批**升序**源行号的原始字节(含行尾换行)追加进 out,返回 (追加的行数, 追加的字节数)。
    /// 单次前向扫描 [首行, 末行] 区间,复杂度 O(区间行数),不做每行独立的检查点回退。
    /// 不可用的行(未索引/越界)跳过。indices 为空返回 (0, 0)。
    /// 相邻重复索引会被**折叠**(只拷贝一次);现有调用方(过滤/错误/书签命中数组)均严格升序无重复。
    pub fn append_sorted_lines_bytes(&self, indices: &[u32], out: &mut Vec<u8>) -> (usize, u64) {
        debug_assert!(
            indices.windows(2).all(|w| w[0] <= w[1]),
            "append_sorted_lines_bytes requires ascending indices"
        );
        let Some(&first) = indices.first() else {
            return (0, 0);
        };
        let first = first as usize;
        let last = *indices.last().unwrap() as usize;
        let frontier = self.indexed_frontier();
        let bytes = self.source.bytes();
        let mut lines = 0usize;
        let mut written = 0u64;
        let mut cursor = 0usize; // indices 中下一个待拷贝行号(升序)
                                 // 单次前向扫描 [first, last+1),不物化 span Vec;命中行(== indices[cursor])才拷字节。
                                 // 越界行(≥ total_lines)不会被 for_each_line_span 产出,cursor 到达末尾后自然停。
        self.indexer.for_each_line_span(
            bytes,
            first,
            last.saturating_add(1),
            frontier,
            |line, span_start, span_end| {
                while cursor < indices.len() && (indices[cursor] as usize) < line {
                    cursor += 1; // 未产出/重复的小于当前行的行号:跳过
                }
                if cursor < indices.len() && indices[cursor] as usize == line {
                    out.extend_from_slice(&bytes[span_start..span_end]);
                    written += (span_end - span_start) as u64;
                    lines += 1;
                    cursor += 1;
                }
            },
        );
        (lines, written)
    }

    /// 把连续行区间 [start, start+count) 的原始字节追加进 out,返回 (行数, 字节数)。
    /// 单次前向扫描,不物化 span Vec,越过末尾的部分自动裁剪。
    pub fn append_line_range_bytes(
        &self,
        start: usize,
        count: usize,
        out: &mut Vec<u8>,
    ) -> (usize, u64) {
        let total = self.indexer.total_lines();
        let end = start.saturating_add(count).min(total);
        let frontier = self.indexed_frontier();
        let bytes = self.source.bytes();
        let mut lines = 0usize;
        let mut written = 0u64;
        self.indexer
            .for_each_line_span(bytes, start, end, frontier, |_, span_start, span_end| {
                out.extend_from_slice(&bytes[span_start..span_end]);
                written += (span_end - span_start) as u64;
                lines += 1;
            });
        (lines, written)
    }

    /// 校验导出目标合法(不得与源文件相同)并确保父目录存在。
    /// 从 create_export_file 中拆出,后者改为先调用本方法。
    pub fn validate_export_target(&self, output: &Path) -> io::Result<()> {
        if self.is_source_path(output) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "export output must differ from source file",
            ));
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
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
    ) -> Vec<u32> {
        let frontier = self.indexed_frontier();
        let end = end.min(self.total_lines());
        let mut matches = Vec::new();
        for (idx, (span_start, span_end)) in (start.min(end)..end).zip(self.indexer.line_spans(
            self.source.bytes(),
            start,
            end,
            frontier,
        )) {
            let text = self
                .encoding
                .decode(&self.source.bytes()[span_start..span_end]);
            let entry = parse_line_ref(&text);
            let marked = matcher.requires_mark() && self.is_bookmarked(idx as u64 + 1);
            if matcher.is_match_with_mark(&entry, marked) {
                push_hit(&mut matches, idx);
            }
        }
        matches
    }

    pub fn apply_filter_results(&mut self, spec: &FilterSpec, matches: Vec<u32>) -> usize {
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

    pub fn append_filter_results(&mut self, spec: &FilterSpec, matches: Vec<u32>) -> usize {
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
    ) -> Vec<u32> {
        let frontier = self.indexed_frontier();
        let end = end.min(self.total_lines());
        let mut matches = Vec::new();
        for (idx, (span_start, span_end)) in (start.min(end)..end).zip(self.indexer.line_spans(
            self.source.bytes(),
            start,
            end,
            frontier,
        )) {
            let text = self
                .encoding
                .decode(&self.source.bytes()[span_start..span_end]);
            let entry = parse_line_ref(&text);
            if matcher.is_entry_match(&entry) {
                push_hit(&mut matches, idx);
            }
        }
        matches
    }

    pub fn apply_search_results(&mut self, spec: &SearchSpec, matches: Vec<u32>) -> SearchSummary {
        self.search_spec = (!spec.query.is_empty()).then(|| spec.clone());
        self.search_matches = matches;
        SearchSummary {
            count: self.search_matches.len(),
            first: self.search_matches.first().map(|idx| u64::from(*idx) + 1),
        }
    }

    pub fn append_search_results(&mut self, spec: &SearchSpec, matches: Vec<u32>) -> SearchSummary {
        self.search_spec = (!spec.query.is_empty()).then(|| spec.clone());
        self.search_matches.extend(matches);
        self.search_matches.sort_unstable();
        self.search_matches.dedup();
        SearchSummary {
            count: self.search_matches.len(),
            first: self.search_matches.first().map(|idx| u64::from(*idx) + 1),
        }
    }

    pub fn search_next(&self, from_line_no: u64, direction: SearchDirection) -> Option<u64> {
        let zero_based = from_line_no.saturating_sub(1);
        // from 超过 u32::MAX 时饱和到 u32::MAX(排在所有命中之后),环绕导航语义正确。
        let from = u32::try_from(zero_based).unwrap_or(u32::MAX);
        next_match(&self.search_matches, from, direction).map(|idx| u64::from(idx) + 1)
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
        let (start, end) = self
            .indexer
            .line_span(self.source.bytes(), source_idx, frontier)?;
        Some((source_idx as u64 + 1, self.parse_source_span(start, end)))
    }

    fn parse_source_span(&self, start: usize, end: usize) -> LogEntry {
        let text = self.encoding.decode(&self.source.bytes()[start..end]);
        LogEntry::from(parse_line_ref(&text))
    }

    fn source_line_bytes(&self, source_idx: usize, frontier: usize) -> Option<&[u8]> {
        let (start, end) = self
            .indexer
            .line_span(self.source.bytes(), source_idx, frontier)?;
        Some(&self.source.bytes()[start..end])
    }

    /// 分批(每批 EXPORT_CHUNK_LINES 行)把连续行区间 [start, start+count) 用批量原语拷进
    /// `buf` 再写盘,单次前向扫描,避免一次性物化整段字节。
    fn write_line_range(
        &self,
        start: usize,
        count: usize,
        buf: &mut Vec<u8>,
        writer: &mut impl Write,
        summary: &mut ExportSummary,
    ) -> io::Result<()> {
        let total = self.indexer.total_lines();
        let end = start.saturating_add(count).min(total);
        let mut cursor = start.min(end);
        while cursor < end {
            let batch_end = cursor.saturating_add(EXPORT_CHUNK_LINES).min(end);
            buf.clear();
            let (lines, bytes) = self.append_line_range_bytes(cursor, batch_end - cursor, buf);
            writer.write_all(buf)?;
            summary.written_lines += lines;
            summary.written_bytes += bytes;
            cursor = batch_end;
        }
        Ok(())
    }

    /// 分批把一批升序源行号用批量原语拷进 `buf` 再写盘,单次前向扫描每个分批。
    fn write_sorted_lines(
        &self,
        indices: &[u32],
        buf: &mut Vec<u8>,
        writer: &mut impl Write,
        summary: &mut ExportSummary,
    ) -> io::Result<()> {
        for chunk in indices.chunks(EXPORT_CHUNK_LINES) {
            buf.clear();
            let (lines, bytes) = self.append_sorted_lines_bytes(chunk, buf);
            writer.write_all(buf)?;
            summary.written_lines += lines;
            summary.written_bytes += bytes;
        }
        Ok(())
    }

    fn create_export_file(&self, output: &Path) -> io::Result<BufWriter<File>> {
        self.validate_export_target(output)?;
        Ok(BufWriter::new(File::create(output)?))
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
                push_hit(&mut self.error_lines, idx);
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

/// 把 0-based 行号推入命中数组。行号超过 `u32::MAX` 时跳过并在 debug 下断言:
/// 10GB logcat 实际行数 ~3 亿,距 u32 上限尚有一个数量级,只需注释兜底即可。
fn push_hit(matches: &mut Vec<u32>, idx: usize) {
    if let Ok(idx32) = u32::try_from(idx) {
        matches.push(idx32);
    } else {
        debug_assert!(false, "line index exceeds u32 range");
    }
}

/// 把"桶→错误行数"映射按桶升序收成 `MinimapBucket` 列表(BTreeMap 已保证键有序)。
fn collect_minimap_buckets(counts: BTreeMap<usize, u32>) -> Vec<MinimapBucket> {
    counts
        .into_iter()
        .map(|(bucket, count)| MinimapBucket { bucket, count })
        .collect()
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
        assert_eq!(
            map.errors,
            vec![
                MinimapBucket {
                    bucket: 0,
                    count: 1
                },
                MinimapBucket {
                    bucket: 1,
                    count: 1
                },
                MinimapBucket {
                    bucket: 2,
                    count: 1
                },
                MinimapBucket {
                    bucket: 3,
                    count: 1
                },
            ]
        );
    }

    #[test]
    fn minimap_counts_multiple_errors_in_same_bucket() {
        // 8 行 / 4 桶 ⇒ 每桶 2 行;第 0、1 行都是 E,同落桶 0 ⇒ count == 2。
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "04-20 12:06:02.000   300   330 E Payment: boom 0").unwrap();
        writeln!(f, "04-20 12:06:02.001   300   330 E Payment: boom 1").unwrap();
        for i in 2..8 {
            writeln!(f, "04-20 12:06:02.{i:03}   300   330 I Payment: info {i}").unwrap();
        }
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        let map = s.minimap(4);
        assert_eq!(
            map.errors,
            vec![MinimapBucket {
                bucket: 0,
                count: 2
            }]
        );
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
        assert_eq!(
            map.errors,
            vec![MinimapBucket {
                bucket: 3,
                count: 1
            }]
        );
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
        assert_eq!(
            map.errors,
            vec![MinimapBucket {
                bucket: 2,
                count: 1
            }]
        );
    }

    #[test]
    fn append_line_bytes_copies_raw_line_and_reports_length() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let mut out = Vec::new();
        let n = s.append_line_bytes(1, &mut out);
        assert_eq!(
            out,
            b"04-20 12:06:02.225   200   220 I Network: GET /home ok\n"
        );
        assert_eq!(n, out.len() as u64);
        assert_eq!(s.append_line_bytes(99, &mut out), 0); // 越界行不追加
        assert_eq!(out.len(), n as usize);
    }

    #[test]
    fn append_sorted_lines_bytes_matches_per_line_oracle() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        // 等价预言:批量拷贝应逐字节等于逐行 append_line_bytes 拼接。
        let indices = [0u32, 2, 3];
        let mut oracle = Vec::new();
        for idx in indices {
            s.append_line_bytes(idx as usize, &mut oracle);
        }

        let mut batch = Vec::new();
        let (lines, bytes) = s.append_sorted_lines_bytes(&indices, &mut batch);
        assert_eq!(batch, oracle);
        assert_eq!(lines, 3);
        assert_eq!(bytes, oracle.len() as u64);
    }

    #[test]
    fn append_sorted_lines_bytes_handles_empty_and_out_of_range() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        // 空输入
        let mut out = Vec::new();
        assert_eq!(s.append_sorted_lines_bytes(&[], &mut out), (0, 0));
        assert!(out.is_empty());

        // 越界行(99 ≥ total_lines)被跳过,只拷第 1 行。
        let mut out = Vec::new();
        let (lines, bytes) = s.append_sorted_lines_bytes(&[1, 99], &mut out);
        let mut expected = Vec::new();
        s.append_line_bytes(1, &mut expected);
        assert_eq!(out, expected);
        assert_eq!(lines, 1);
        assert_eq!(bytes, expected.len() as u64);
    }

    #[test]
    fn append_line_range_bytes_full_range_equals_file_bytes() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        let mut out = Vec::new();
        let (lines, bytes) = s.append_line_range_bytes(0, s.total_lines(), &mut out);
        let file_bytes = std::fs::read(f.path()).unwrap();
        assert_eq!(out, file_bytes);
        assert_eq!(lines, 4);
        assert_eq!(bytes, file_bytes.len() as u64);
    }

    #[test]
    fn append_line_range_bytes_clamps_past_eof() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();

        // 从第 3 行起要 100 行,应裁到文件末尾(行 3、4)。
        let mut out = Vec::new();
        let (lines, bytes) = s.append_line_range_bytes(2, 100, &mut out);
        let mut expected = Vec::new();
        s.append_line_bytes(2, &mut expected);
        s.append_line_bytes(3, &mut expected);
        assert_eq!(out, expected);
        assert_eq!(lines, 2);
        assert_eq!(bytes, expected.len() as u64);
    }

    #[test]
    fn export_plan_snapshot_matches_view_semantics() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        s.toggle_bookmark(2).unwrap();

        assert_eq!(
            s.export_plan_snapshot(RowsView::All),
            ExportPlan::AllLines { total: 4 }
        );
        // 过滤未激活时 Filtered 退化为 All
        assert_eq!(
            s.export_plan_snapshot(RowsView::Filtered),
            ExportPlan::AllLines { total: 4 }
        );
        s.set_filter(&FilterSpec {
            tag_include: FilterField::plain(true, "Network"),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            s.export_plan_snapshot(RowsView::Filtered),
            ExportPlan::Indices(vec![1, 2])
        );
        assert_eq!(
            s.export_plan_snapshot(RowsView::Bookmarks),
            ExportPlan::Indices(vec![1])
        );
        assert_eq!(
            s.export_plan_snapshot(RowsView::Errors),
            ExportPlan::Indices(vec![3])
        );
    }

    #[test]
    fn validate_export_target_rejects_source_file() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        assert!(s.validate_export_target(f.path()).is_err());
        let dir = tempfile::tempdir().unwrap();
        assert!(s
            .validate_export_target(&dir.path().join("sub/out.log"))
            .is_ok());
        assert!(dir.path().join("sub").exists()); // 父目录已创建
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

    // 下面两个测试模拟"外部进程截断会话文件":Windows 上带活动映射的文件无法被
    // 外部截断(OS 报 1224 user-mapped section open),该场景仅存在于 Unix,
    // 收缩重建路径在 Windows 上由操作系统天然免疫,故测试只在 Unix 编译。
    #[cfg(unix)]
    #[test]
    fn remap_after_truncation_rebuilds_index_without_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shrink.log");
        std::fs::write(
            &path,
            "04-20 12:06:02.125   146   179 D T: one\n04-20 12:06:02.225   146   179 E T: two\n",
        )
        .unwrap();
        let mut s = Session::open(&path).unwrap();
        s.index_all();
        assert_eq!(s.total_lines(), 2);
        assert_eq!(s.error_count(), 1);

        std::fs::write(&path, "04-20 12:06:03.000   146   179 D T: fresh\n").unwrap();
        let outcome = s.remap_and_index_step(usize::MAX).unwrap();
        assert!(outcome.reset);

        assert_eq!(s.total_lines(), 1);
        assert_eq!(s.error_count(), 0);
        let rows = s.get_rows(0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1.message, "fresh");
    }

    #[cfg(unix)]
    #[test]
    fn truncation_reset_rescans_filter_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stream.log");
        std::fs::write(
            &path,
            "04-20 12:06:02.125   146   179 D One: first\n04-20 12:06:02.225   200   220 I Two: second\n",
        )
        .unwrap();
        let mut s = Session::open(&path).unwrap();
        s.index_all();
        let spec = FilterSpec {
            tag_include: FilterField::plain(true, "Two"),
            ..Default::default()
        };
        assert_eq!(s.set_filter(&spec).unwrap(), 1);
        let previous_total = s.total_lines();

        // 外部截断 + 重写:新内容里 Two 出现在第 1 行
        std::fs::write(&path, "04-20 12:06:03.000   200   220 I Two: reborn\n").unwrap();
        let outcome = s.remap_and_index_step(usize::MAX).unwrap();
        assert!(outcome.reset);

        // 模拟 stream reader 的重扫决策:reset 后从 0 起扫
        let scan_start = if outcome.reset { 0 } else { previous_total };
        let matcher = FilterMatcher::new(&spec).unwrap();
        let matches = s.filter_indexed_range(&matcher, scan_start, s.total_lines());
        assert_eq!(s.append_filter_results(&spec, matches), 1);
        let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1.message, "reborn");
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
        let outcome = s.remap_and_index_step(usize::MAX).unwrap();
        assert!(!outcome.reset);
        let matcher = FilterMatcher::new(&spec).unwrap();
        let matches = s.filter_indexed_range(&matcher, previous_total, s.total_lines());
        assert_eq!(s.append_filter_results(&spec, matches), 1);

        let rows = s.get_rows_for_view(RowsView::Filtered, 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 2);
    }
}
