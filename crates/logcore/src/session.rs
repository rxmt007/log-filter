use crate::bookmarks::{BookmarkDirection, BookmarkStore};
use crate::encoding::{ResolvedTextEncoding, TextEncoding};
use crate::export::ExportSummary;
use crate::filter::{FilterError, FilterMatcher, FilterSpec};
use crate::indexer::Indexer;
use crate::mmap_source::MmapSource;
use crate::model::LogEntry;
use crate::parser::parse_line_ref;
use crate::problems::{
    classify_candidate, preclassify_problem_line, BufferProvenanceTracker, GroupId, GroupPage,
    GroupQuery, GroupSnapshotCapture, GroupSortRecord, InputCoverage, ObservationRef,
    OccurrencePage, PageSpec, ProblemEngine, ProblemEvent, ProblemEventId, ProblemGroupSummary,
    ProblemMemoryStats, ProblemStats, QuerySnapshotId, RangeCompleteness, SnapshotError,
    SourceSpan, SourceSpanError, SourceSpanIndex,
};
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
const PROBLEM_SCAN_MAX_LINES: usize = 4096;
const PROBLEM_SCAN_MAX_BYTES: usize = 512 * 1024;
const PROBLEM_SCAN_MAX_DETAIL_LINES: usize = 128;

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
    /// 实际量程桶数；短结果集不会保留请求量程中的空洞。
    pub bucket_count: usize,
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProblemScanStep {
    pub scanned_lines: usize,
    pub stable_lines: usize,
    pub committed_occurrences: usize,
    pub stored_occurrences: usize,
    pub dropped_occurrences: usize,
    pub failed_commits: usize,
    pub caught_up: bool,
    pub finished: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLifecycle {
    Growing,
    Paused,
    Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLifecycleError {
    StaticInput,
    InvalidTransition {
        from: InputLifecycle,
        operation: &'static str,
    },
    InputNotFullyIndexed,
}

impl std::fmt::Display for InputLifecycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaticInput => formatter.write_str("input is static"),
            Self::InvalidTransition { from, operation } => {
                write!(formatter, "cannot {operation} input while it is {from:?}")
            }
            Self::InputNotFullyIndexed => {
                formatter.write_str("cannot seal input before indexed bytes reach EOF")
            }
        }
    }
}

impl std::error::Error for InputLifecycleError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputState {
    Static,
    Growing(InputLifecycle),
}

pub struct Session {
    source_path: PathBuf,
    source: MmapSource,
    indexer: Indexer,
    input_state: InputState,
    filtered: Vec<u32>,
    filter_active: bool,
    /// The spec represented by `filtered`. A newly requested filter lives in
    /// `pending_filter_spec` until its complete hit vector is atomically applied.
    filter_spec: FilterSpec,
    pending_filter_spec: Option<FilterSpec>,
    search_matches: Vec<u32>,
    /// The spec represented by `search_matches`.
    search_spec: Option<SearchSpec>,
    pending_search_spec: Option<SearchSpec>,
    bookmarks: BookmarkStore,
    error_lines: Vec<u32>,
    error_scan_lines: usize,
    encoding: ResolvedTextEncoding,
    problem_engine: ProblemEngine,
    problem_scan_lines: usize,
    problem_finished: bool,
    problem_coverage: InputCoverage,
    problem_source_spans: SourceSpanIndex,
    problem_buffer_tracker: BufferProvenanceTracker,
}

impl Session {
    pub fn open(path: &Path) -> std::io::Result<Session> {
        Self::open_with_encoding(path, TextEncoding::Utf8)
    }

    pub fn open_with_encoding(path: &Path, encoding: TextEncoding) -> std::io::Result<Session> {
        Self::open_with_input_state(path, encoding, InputState::Static)
    }

    /// 打开会继续追加字节的输入。与静态文件不同，未见行尾换行的尾行不会成为稳定行。
    pub fn open_growing(path: &Path) -> std::io::Result<Session> {
        Self::open_growing_with_encoding(path, TextEncoding::Utf8)
    }

    pub fn open_growing_with_encoding(
        path: &Path,
        encoding: TextEncoding,
    ) -> std::io::Result<Session> {
        Self::open_with_input_state(path, encoding, InputState::Growing(InputLifecycle::Growing))
    }

    fn open_with_input_state(
        path: &Path,
        encoding: TextEncoding,
        input_state: InputState,
    ) -> std::io::Result<Session> {
        let source = MmapSource::open(path)?;
        let bookmarks = BookmarkStore::load_for_source(path).unwrap_or_default();
        let problem_coverage = match input_state {
            InputState::Static => InputCoverage::static_file(RangeCompleteness::Bounded),
            InputState::Growing(_) => InputCoverage::adb_live(
                crate::problems::BufferSet::MAIN,
                RangeCompleteness::StartTruncated,
            ),
        };
        Ok(Session {
            source_path: path.to_path_buf(),
            source,
            indexer: Indexer::new(),
            input_state,
            filtered: Vec::new(),
            filter_active: false,
            filter_spec: FilterSpec::default(),
            pending_filter_spec: None,
            search_matches: Vec::new(),
            search_spec: None,
            pending_search_spec: None,
            bookmarks,
            error_lines: Vec::new(),
            error_scan_lines: 0,
            encoding: encoding.resolve(),
            problem_engine: ProblemEngine::new(),
            problem_scan_lines: 0,
            problem_finished: false,
            problem_coverage,
            problem_source_spans: SourceSpanIndex::new(),
            problem_buffer_tracker: BufferProvenanceTracker::new(),
        })
    }

    pub fn set_encoding(&mut self, encoding: TextEncoding) {
        let resolved = encoding.resolve();
        if self.encoding.config_label() != resolved.config_label() {
            let desired_filter = self
                .pending_filter_spec
                .take()
                .unwrap_or_else(|| self.filter_spec.clone());
            if desired_filter.is_active() {
                self.filtered.clear();
                self.filter_active = true;
                self.filter_spec = desired_filter.clone();
                self.pending_filter_spec = Some(desired_filter);
            }
            let desired_search = self
                .pending_search_spec
                .take()
                .or_else(|| self.active_search_spec());
            self.search_matches.clear();
            self.search_spec = desired_search.clone();
            self.pending_search_spec = desired_search;
            self.encoding = resolved;
            self.reset_problem_analysis();
        }
    }

    pub fn encoding_config_label(&self) -> &'static str {
        self.encoding.config_label()
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

    /// 可供派生扫描消费的稳定完整行数。
    ///
    /// 静态输入只有在索引抵达 EOF 后才提交最后一个无换行行；增长输入在封口前只提交
    /// 已经看到换行符的行。
    pub fn stable_lines(&self) -> usize {
        match self.input_state {
            InputState::Static if self.is_indexing_done() => self.indexer.total_lines(),
            InputState::Growing(InputLifecycle::Sealed) => self.indexer.total_lines(),
            InputState::Static
            | InputState::Growing(InputLifecycle::Growing | InputLifecycle::Paused) => {
                self.indexer.completed_lines()
            }
        }
    }

    pub fn input_lifecycle(&self) -> Option<InputLifecycle> {
        match self.input_state {
            InputState::Static => None,
            InputState::Growing(lifecycle) => Some(lifecycle),
        }
    }

    pub fn pause_growing_input(&mut self) -> Result<(), InputLifecycleError> {
        match self.input_state {
            InputState::Growing(InputLifecycle::Growing) => {
                self.input_state = InputState::Growing(InputLifecycle::Paused);
                Ok(())
            }
            InputState::Static => Err(InputLifecycleError::StaticInput),
            InputState::Growing(from) => Err(InputLifecycleError::InvalidTransition {
                from,
                operation: "pause",
            }),
        }
    }

    pub fn resume_paused_input(&mut self) -> Result<(), InputLifecycleError> {
        match self.input_state {
            InputState::Growing(InputLifecycle::Paused) => {
                self.input_state = InputState::Growing(InputLifecycle::Growing);
                Ok(())
            }
            InputState::Static => Err(InputLifecycleError::StaticInput),
            InputState::Growing(from) => Err(InputLifecycleError::InvalidTransition {
                from,
                operation: "resume",
            }),
        }
    }

    pub fn seal_growing_input(&mut self) -> Result<(), InputLifecycleError> {
        match self.input_state {
            InputState::Static => Err(InputLifecycleError::StaticInput),
            InputState::Growing(InputLifecycle::Sealed) => {
                Err(InputLifecycleError::InvalidTransition {
                    from: InputLifecycle::Sealed,
                    operation: "seal",
                })
            }
            InputState::Growing(InputLifecycle::Growing | InputLifecycle::Paused) => {
                if !self.is_indexing_done() {
                    return Err(InputLifecycleError::InputNotFullyIndexed);
                }
                self.input_state = InputState::Growing(InputLifecycle::Sealed);
                self.refresh_error_lines();
                Ok(())
            }
        }
    }

    pub fn is_indexing_done(&self) -> bool {
        self.indexer.is_done(self.source.len())
    }

    /// 重新映射源文件。文件未增长时跳过(流式 reader 每个读块都会调用,mmap/munmap 不便宜);
    /// 检测到收缩(外部截断/轮转)时旧索引全部失效,重建派生状态,避免越界访问乃至 SIGBUS。
    /// 返回 `true` 当且仅当发生了收缩重建(调用方据此从 0 起重扫过滤/查找)。
    pub fn remap_source(&mut self) -> io::Result<bool> {
        if let InputState::Growing(lifecycle @ (InputLifecycle::Paused | InputLifecycle::Sealed)) =
            self.input_state
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot append while growing input is {lifecycle:?}"),
            ));
        }
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
        let desired_filter = self
            .pending_filter_spec
            .take()
            .unwrap_or_else(|| self.filter_spec.clone());
        let desired_search = self
            .pending_search_spec
            .take()
            .or_else(|| self.search_spec.clone());
        self.indexer = Indexer::new();
        self.filtered.clear();
        self.filter_active = desired_filter.is_active();
        self.filter_spec = desired_filter;
        self.search_matches.clear();
        self.search_spec = desired_search;
        self.error_lines.clear();
        self.error_scan_lines = 0;
        // A truncated/replaced source is a new input identity. Source-line
        // bookmarks from the previous bytes must never mark the replacement.
        self.bookmarks = BookmarkStore::new();
        let _ = fs::remove_file(crate::bookmarks::sidecar_path_for(&self.source_path));
        self.problem_source_spans = SourceSpanIndex::new();
        self.reset_problem_analysis();
    }

    fn reset_problem_analysis(&mut self) {
        self.problem_engine.reset();
        self.problem_scan_lines = 0;
        self.problem_finished = false;
        self.problem_buffer_tracker = BufferProvenanceTracker::new();
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

    /// Replace the capture-level provenance contract and restart only Problems analysis.
    ///
    /// Existing row/filter/search indexes remain valid because this changes fact admission,
    /// not source bytes or decoding.
    pub fn set_problem_input_coverage(&mut self, coverage: InputCoverage) {
        if self.problem_coverage != coverage {
            self.problem_coverage = coverage;
            self.reset_problem_analysis();
        }
    }

    pub const fn problem_input_coverage(&self) -> InputCoverage {
        self.problem_coverage
    }

    /// Add a source span proven by the input adapter. Adding provenance after scanning starts
    /// restarts Problems so previously unknown rows cannot keep a weaker admission decision.
    pub fn add_problem_source_span(&mut self, span: SourceSpan) -> Result<(), SourceSpanError> {
        self.problem_source_spans.insert(span)?;
        self.reset_problem_analysis();
        Ok(())
    }

    pub fn scan_problems_step(&mut self, max_lines: usize) -> ProblemScanStep {
        let stable_lines = self.stable_lines();
        if self.problem_finished {
            return ProblemScanStep {
                scanned_lines: self.problem_scan_lines,
                stable_lines,
                caught_up: self.problem_scan_lines >= stable_lines,
                finished: true,
                ..ProblemScanStep::default()
            };
        }

        let start = self.problem_scan_lines.min(stable_lines);
        let end = start
            .saturating_add(max_lines.min(PROBLEM_SCAN_MAX_LINES))
            .min(stable_lines);
        let frontier = self.indexed_frontier();
        let mut committed_occurrences = 0usize;
        let mut stored_occurrences = 0usize;
        let mut dropped_occurrences = 0usize;
        let mut failed_commits = 0usize;
        let scanned_end;

        {
            let source_bytes = self.source.bytes();
            let indexer = &self.indexer;
            let encoding = self.encoding;
            let coverage = self.problem_coverage;
            let source_spans = &self.problem_source_spans;
            let buffer_tracker = &mut self.problem_buffer_tracker;
            let engine = &mut self.problem_engine;
            let mut detail_lines = 0usize;
            let mut first_span_start = None;
            let mut stop_before_next = false;
            scanned_end = indexer.for_each_line_span_prefix(
                source_bytes,
                start,
                end,
                frontier,
                |line, span_start, span_end| {
                    if stop_before_next {
                        return false;
                    }
                    let step_start = first_span_start.unwrap_or(span_start);
                    if first_span_start.is_some()
                        && span_end.saturating_sub(step_start) > PROBLEM_SCAN_MAX_BYTES
                    {
                        return false;
                    }
                    first_span_start = Some(step_start);
                    let Ok(line) = u32::try_from(line) else {
                        failed_commits = failed_commits.saturating_add(1);
                        return true;
                    };
                    let raw = &source_bytes[span_start..span_end];
                    let provenance =
                        buffer_tracker.observe_stable_line(raw, source_spans.provenance_at(line));
                    let pending_detail = engine.requires_full_line();
                    let gate = preclassify_problem_line(raw);
                    let needs_detail = pending_detail || gate.might_be_candidate;
                    let delta = if needs_detail {
                        let text = encoding.decode(raw);
                        let parsed = parse_line_ref(&text);
                        if pending_detail || !classify_candidate(&parsed, raw).is_empty() {
                            detail_lines = detail_lines.saturating_add(1);
                        }
                        engine.observe(crate::problems::ObservedLine::new(
                            line, raw, parsed, provenance, coverage,
                        ))
                    } else {
                        engine.observe_non_candidate(line, gate.timestamp)
                    };
                    committed_occurrences =
                        committed_occurrences.saturating_add(usize::from(delta.committed()));
                    stored_occurrences =
                        stored_occurrences.saturating_add(usize::from(delta.stored()));
                    dropped_occurrences =
                        dropped_occurrences.saturating_add(usize::from(delta.dropped()));
                    failed_commits = failed_commits.saturating_add(usize::from(delta.failed()));
                    stop_before_next = detail_lines >= PROBLEM_SCAN_MAX_DETAIL_LINES;
                    true
                },
            );
        }
        self.problem_scan_lines = scanned_end;
        ProblemScanStep {
            scanned_lines: scanned_end,
            stable_lines,
            committed_occurrences,
            stored_occurrences,
            dropped_occurrences,
            failed_commits,
            caught_up: scanned_end >= stable_lines,
            finished: false,
        }
    }

    pub fn finish_problem_input(&mut self) -> ProblemScanStep {
        let stable_lines = self.stable_lines();
        let terminal = match self.input_state {
            InputState::Static => self.is_indexing_done(),
            InputState::Growing(InputLifecycle::Sealed) => true,
            InputState::Growing(InputLifecycle::Growing | InputLifecycle::Paused) => false,
        };
        if self.problem_finished || !terminal || self.problem_scan_lines < stable_lines {
            return ProblemScanStep {
                scanned_lines: self.problem_scan_lines,
                stable_lines,
                caught_up: self.problem_scan_lines >= stable_lines,
                finished: self.problem_finished,
                ..ProblemScanStep::default()
            };
        }

        let delta = self.problem_engine.finish_input();
        self.problem_finished = true;
        ProblemScanStep {
            scanned_lines: self.problem_scan_lines,
            stable_lines,
            committed_occurrences: usize::from(delta.committed()),
            stored_occurrences: usize::from(delta.stored()),
            dropped_occurrences: usize::from(delta.dropped()),
            failed_commits: usize::from(delta.failed()),
            caught_up: true,
            finished: true,
        }
    }

    pub fn problem_stats(&self) -> ProblemStats {
        self.problem_engine.stats()
    }

    pub fn problem_memory_stats(&self) -> ProblemMemoryStats {
        self.problem_engine.memory_stats()
    }

    pub fn problem_scanned_lines(&self) -> usize {
        self.problem_scan_lines
    }

    pub fn problem_analysis_finished(&self) -> bool {
        self.problem_finished
    }

    pub fn create_problem_group_snapshot(
        &mut self,
        query: &GroupQuery,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.problem_engine.create_group_snapshot(query)
    }

    pub fn problem_group_snapshot_capture(&self) -> GroupSnapshotCapture {
        self.problem_engine.group_snapshot_capture()
    }

    pub fn problem_group_sort_records(
        &self,
        query: &GroupQuery,
        capture: GroupSnapshotCapture,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<GroupSortRecord>, SnapshotError> {
        self.problem_engine
            .group_sort_records(query, capture, offset, limit)
    }

    pub fn install_problem_group_snapshot(
        &mut self,
        ids: Vec<GroupId>,
        revision: u64,
        query: crate::problems::GroupQuery,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.problem_engine
            .install_group_snapshot_ids(ids, revision, query)
    }

    pub fn create_problem_occurrence_snapshot(
        &mut self,
        group: GroupId,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.problem_engine.create_occurrence_snapshot(group)
    }

    pub fn problem_group_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<GroupPage, SnapshotError> {
        self.problem_engine.group_snapshot_page(snapshot, page)
    }

    pub fn problem_group_snapshot_page_for_query(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
        query: crate::problems::GroupQuery,
    ) -> Result<GroupPage, SnapshotError> {
        self.problem_engine
            .group_snapshot_page_for_query(snapshot, page, query)
    }

    pub fn problem_occurrence_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.problem_engine.occurrence_snapshot_page(snapshot, page)
    }

    pub fn problem_occurrence_snapshot_page_for_group(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
        group: GroupId,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.problem_engine
            .occurrence_snapshot_page_for_group(snapshot, page, group)
    }

    pub fn release_problem_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool {
        self.problem_engine.release_snapshot(snapshot)
    }

    pub fn problem_group(&self, id: GroupId) -> Option<ProblemGroupSummary> {
        self.problem_engine.group(id)
    }

    pub fn problem_event(&self, id: ProblemEventId) -> Option<ProblemEvent> {
        self.problem_engine.event(id)
    }

    pub fn problem_event_observations(&self, id: ProblemEventId) -> Option<&[ObservationRef]> {
        self.problem_engine.event_observations(id)
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
        let bucket_count = buckets.min(total);
        if bucket_count == 0 {
            return Minimap {
                bucket_count: 0,
                bookmarks: Vec::new(),
                errors: Vec::new(),
            };
        }
        if !self.filter_active {
            return self.source_minimap(bucket_count);
        }
        // 反向遍历:书签/错误行是小集合,逐个二分反查在过滤结果中的位置,
        // 避免 O(过滤结果总数) 的全量扫描(minimap 会被状态事件高频触发)。
        let bookmarks = self
            .bookmark_source_lines()
            .into_iter()
            // 超过 u32 的书签行不可能在命中数组内(命中数组元素都 ≤ u32::MAX)。
            .filter_map(|line_no| u32::try_from(line_no - 1).ok())
            .filter_map(|needle| self.filtered.binary_search(&needle).ok())
            .filter_map(|result_idx| bucket_for_zero_based(result_idx, total, bucket_count))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // 错误按桶累加:反查过滤结果中的位置,再定位其所在桶,同桶多条错误 count 递增。
        let mut error_counts: BTreeMap<usize, u32> = BTreeMap::new();
        for idx in &self.error_lines {
            if let Ok(result_idx) = self.filtered.binary_search(idx) {
                if let Some(bucket) = bucket_for_zero_based(result_idx, total, bucket_count) {
                    *error_counts.entry(bucket).or_insert(0) += 1;
                }
            }
        }
        let errors = collect_minimap_buckets(error_counts);
        Minimap {
            bucket_count,
            bookmarks,
            errors,
        }
    }

    fn current_result_len(&self) -> usize {
        self.filtered_count()
    }

    fn current_result_index_for_source_idx(&self, source_idx: u64) -> Option<usize> {
        if self.filter_active {
            // 超过 u32 的 source_idx 不可能在命中数组内(命中数组元素都 ≤ u32::MAX)。
            let needle = u32::try_from(source_idx).ok()?;
            self.filtered.binary_search(&needle).ok()
        } else if (source_idx as usize) < self.stable_lines() {
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

    /// Map a source line to the closest current filtered result. Equal-distance
    /// ties prefer the preceding source line to keep viewport restoration stable.
    pub fn nearest_result_for_line_no(&self, line_no: u64) -> Option<ResultTarget> {
        let stable_lines = self.stable_lines();
        if line_no == 0 || stable_lines == 0 {
            return None;
        }
        let source_idx = line_no.saturating_sub(1).min((stable_lines - 1) as u64);
        if !self.filter_active {
            return Some(ResultTarget {
                line_no: source_idx + 1,
                result_index: source_idx as usize,
            });
        }
        let needle = u32::try_from(source_idx).unwrap_or(u32::MAX);
        match self.filtered.binary_search(&needle) {
            Ok(result_index) => Some(ResultTarget {
                line_no: source_idx + 1,
                result_index,
            }),
            Err(insertion) => {
                let before = insertion
                    .checked_sub(1)
                    .and_then(|index| self.filtered.get(index).copied().map(|line| (index, line)));
                let after = self
                    .filtered
                    .get(insertion)
                    .copied()
                    .map(|line| (insertion, line));
                let (result_index, selected) = match (before, after) {
                    (Some(before), Some(after)) => {
                        let before_distance = needle.saturating_sub(before.1);
                        let after_distance = after.1.saturating_sub(needle);
                        if before_distance <= after_distance {
                            before
                        } else {
                            after
                        }
                    }
                    (Some(before), None) => before,
                    (None, Some(after)) => after,
                    (None, None) => return None,
                };
                Some(ResultTarget {
                    line_no: u64::from(selected) + 1,
                    result_index,
                })
            }
        }
    }

    fn source_minimap(&self, bucket_count: usize) -> Minimap {
        let total = self.stable_lines();
        let bookmarks = self
            .bookmark_source_lines()
            .into_iter()
            .filter_map(|line| bucket_for_zero_based((line - 1) as usize, total, bucket_count))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut error_counts: BTreeMap<usize, u32> = BTreeMap::new();
        for idx in &self.error_lines {
            if let Some(bucket) = bucket_for_zero_based(*idx as usize, total, bucket_count) {
                *error_counts.entry(bucket).or_insert(0) += 1;
            }
        }
        let errors = collect_minimap_buckets(error_counts);
        Minimap {
            bucket_count,
            bookmarks,
            errors,
        }
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
                self.write_line_range(0, self.stable_lines(), &mut buf, &mut writer, &mut summary)?;
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
        let total = self.stable_lines() as u64;
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
                total: self.stable_lines(),
            },
            RowsView::Filtered => {
                if self.filter_active {
                    ExportPlan::Indices(self.filtered.clone())
                } else {
                    ExportPlan::AllLines {
                        total: self.stable_lines(),
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
        if source_idx >= self.stable_lines() {
            return 0;
        }
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
            last.saturating_add(1).min(self.stable_lines()),
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
        let total = self.stable_lines();
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
        let matches = self.filter_indexed_range(&matcher, 0, self.stable_lines());
        Ok(self.apply_filter_results(spec, matches))
    }

    pub fn set_filter_pending(&mut self, spec: &FilterSpec) -> Result<Option<usize>, FilterError> {
        if !spec.is_active() {
            self.pending_filter_spec = None;
            self.filter_spec = spec.clone();
            self.filtered.clear();
            self.filter_active = false;
            return Ok(Some(self.stable_lines()));
        }
        FilterMatcher::new(spec)?;
        self.pending_filter_spec = Some(spec.clone());
        Ok(None)
    }

    /// Return the filter whose hit vector is currently visible.
    pub fn active_filter_spec(&self) -> Option<FilterSpec> {
        (self.filter_active && self.filter_spec.is_active()).then(|| self.filter_spec.clone())
    }

    /// Return the latest requested filter, including one still being scanned.
    pub fn desired_filter_spec(&self) -> Option<FilterSpec> {
        self.pending_filter_spec.clone().or_else(|| {
            self.filter_spec
                .is_active()
                .then(|| self.filter_spec.clone())
        })
    }

    pub fn filter_indexed_range(
        &self,
        matcher: &FilterMatcher,
        start: usize,
        end: usize,
    ) -> Vec<u32> {
        let frontier = self.indexed_frontier();
        let end = end.min(self.stable_lines());
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
        if self.pending_filter_spec.as_ref() == Some(spec) {
            self.pending_filter_spec = None;
        }
        if !spec.is_active() {
            self.filtered.clear();
            self.filter_active = false;
            return self.stable_lines();
        }
        let count = matches.len();
        self.filtered = matches;
        self.filter_active = true;
        count
    }

    pub fn append_filter_results(&mut self, spec: &FilterSpec, matches: Vec<u32>) -> usize {
        // Incremental live appends may only extend the already published
        // dataset. A pending replacement spec must not mix into old results.
        if !self.filter_active || self.filter_spec != *spec {
            return self.filtered_count();
        }
        append_sorted_unique(&mut self.filtered, matches);
        self.filtered.len()
    }

    pub fn filtered_count(&self) -> usize {
        if self.filter_active {
            self.filtered.len()
        } else {
            self.stable_lines()
        }
    }

    pub fn search(&mut self, spec: &SearchSpec) -> Result<SearchSummary, SearchError> {
        if !self.set_search_pending(spec)? {
            return Ok(SearchSummary::from_matches(&self.search_matches));
        }
        let matcher = SearchMatcher::new(spec)?;
        let matches = self.search_indexed_range(&matcher, 0, self.stable_lines());
        Ok(self.apply_search_results(spec, matches))
    }

    pub fn set_search_pending(&mut self, spec: &SearchSpec) -> Result<bool, SearchError> {
        if spec.query.is_empty() {
            self.pending_search_spec = None;
            self.search_spec = None;
            self.search_matches.clear();
            return Ok(false);
        }
        SearchMatcher::new(spec)?;
        self.pending_search_spec = Some(spec.clone());
        Ok(true)
    }

    /// Return the search represented by `search_matches`.
    pub fn active_search_spec(&self) -> Option<SearchSpec> {
        self.search_spec
            .clone()
            .filter(|spec| !spec.query.is_empty())
    }

    /// Return the latest requested search, including one still being scanned.
    pub fn desired_search_spec(&self) -> Option<SearchSpec> {
        self.pending_search_spec
            .clone()
            .or_else(|| self.active_search_spec())
    }

    pub fn search_indexed_range(
        &self,
        matcher: &SearchMatcher,
        start: usize,
        end: usize,
    ) -> Vec<u32> {
        let frontier = self.indexed_frontier();
        let end = end.min(self.stable_lines());
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
        if self.pending_search_spec.as_ref() == Some(spec) {
            self.pending_search_spec = None;
        }
        self.search_matches = matches;
        SearchSummary {
            count: self.search_matches.len(),
            first: self.search_matches.first().map(|idx| u64::from(*idx) + 1),
        }
    }

    pub fn append_search_results(&mut self, spec: &SearchSpec, matches: Vec<u32>) -> SearchSummary {
        if self.search_spec.as_ref() == Some(spec) {
            append_sorted_unique(&mut self.search_matches, matches);
        }
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
            RowsView::All => self.stable_lines(),
            RowsView::Filtered => self.filtered.len(),
            RowsView::Bookmarks => bookmark_lines.len(),
            RowsView::Errors => self.error_lines.len(),
        };
        let end = start.saturating_add(count).min(view_len);
        let mut out = Vec::with_capacity(end.saturating_sub(start));
        if effective_view == RowsView::All {
            let bytes = self.source.bytes();
            self.indexer.for_each_line_span(
                bytes,
                start,
                end,
                frontier,
                |source_idx, span_start, span_end| {
                    out.push((
                        source_idx as u64 + 1,
                        self.parse_source_span(span_start, span_end),
                    ));
                },
            );
            return out;
        }
        for view_idx in start..end {
            let source_idx = match effective_view {
                RowsView::All => unreachable!("all rows use the contiguous window fast path"),
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
        let total = self.stable_lines();
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
        let total = self.stable_lines();
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
        let max = self.stable_lines() as u64;
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

/// Append an already sorted hit batch without re-sorting the full 10GB-scale
/// history on every live chunk. The normal incremental path is strictly after
/// the existing frontier and therefore O(new hits); the merge fallback keeps
/// public callers deterministic when ranges overlap.
fn append_sorted_unique(existing: &mut Vec<u32>, mut incoming: Vec<u32>) {
    if incoming.is_empty() {
        return;
    }
    incoming.dedup();
    if existing
        .last()
        .is_none_or(|last| incoming.first().is_some_and(|first| *last < *first))
    {
        existing.extend(incoming);
        return;
    }

    let previous = std::mem::take(existing);
    let mut merged = Vec::with_capacity(previous.len().saturating_add(incoming.len()));
    let (mut left, mut right) = (0, 0);
    while left < previous.len() && right < incoming.len() {
        match previous[left].cmp(&incoming[right]) {
            std::cmp::Ordering::Less => {
                merged.push(previous[left]);
                left += 1;
            }
            std::cmp::Ordering::Greater => {
                merged.push(incoming[right]);
                right += 1;
            }
            std::cmp::Ordering::Equal => {
                merged.push(previous[left]);
                left += 1;
                right += 1;
            }
        }
    }
    merged.extend_from_slice(&previous[left..]);
    merged.extend_from_slice(&incoming[right..]);
    *existing = merged;
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
    fn static_and_growing_inputs_commit_unterminated_tail_differently() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "04-20 12:06:02.125   146   179 E Crash: unterminated").unwrap();
        f.flush().unwrap();

        let mut static_session = Session::open(f.path()).unwrap();
        static_session.index_step(8);
        assert_eq!(static_session.total_lines(), 1);
        assert_eq!(static_session.stable_lines(), 0);
        static_session.index_all();
        assert_eq!(static_session.stable_lines(), 1);

        let mut growing_session = Session::open_growing(f.path()).unwrap();
        growing_session.index_all();
        assert_eq!(growing_session.total_lines(), 1);
        assert_eq!(growing_session.stable_lines(), 0);
    }

    #[test]
    fn growing_input_pause_resume_and_seal_have_distinct_tail_semantics() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "04-20 12:06:02.125   146   179 E Crash: unterminated").unwrap();
        f.flush().unwrap();

        let mut session = Session::open_growing(f.path()).unwrap();
        session.index_all();
        assert_eq!(session.stable_lines(), 0);
        assert_eq!(session.error_count(), 0);

        session.pause_growing_input().unwrap();
        assert_eq!(session.stable_lines(), 0, "pause must not seal the tail");
        session.resume_paused_input().unwrap();
        assert_eq!(session.stable_lines(), 0);

        session.seal_growing_input().unwrap();
        assert_eq!(session.stable_lines(), 1);
        assert_eq!(session.error_count(), 1);
        assert!(session.resume_paused_input().is_err());
        assert!(session.pause_growing_input().is_err());

        let mut static_session = Session::open(f.path()).unwrap();
        assert!(static_session.pause_growing_input().is_err());
        assert!(static_session.resume_paused_input().is_err());
        assert!(static_session.seal_growing_input().is_err());
    }

    #[test]
    fn growing_filter_waits_for_the_tail_newline_and_matches_once() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "04-20 12:06:02.125   146   179 E Crash: incomplete hit").unwrap();
        f.flush().unwrap();

        let mut session = Session::open_growing(f.path()).unwrap();
        session.index_all();
        let spec = FilterSpec {
            word_include: FilterField::plain(true, "incomplete hit"),
            ..Default::default()
        };
        assert_eq!(session.set_filter(&spec).unwrap(), 0);

        let previous_stable = session.stable_lines();
        writeln!(f).unwrap();
        f.flush().unwrap();
        session.remap_and_index_step(usize::MAX).unwrap();
        let matcher = FilterMatcher::new(&spec).unwrap();
        let matches =
            session.filter_indexed_range(&matcher, previous_stable, session.stable_lines());
        assert_eq!(session.append_filter_results(&spec, matches), 1);

        let duplicate_scan =
            session.filter_indexed_range(&matcher, session.stable_lines(), session.stable_lines());
        assert_eq!(session.append_filter_results(&spec, duplicate_scan), 1);
    }

    #[test]
    fn growing_search_waits_for_the_tail_newline_and_matches_once() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "04-20 12:06:02.125   146   179 E Crash: searchable tail").unwrap();
        f.flush().unwrap();

        let mut session = Session::open_growing(f.path()).unwrap();
        session.index_all();
        let spec = SearchSpec::plain("searchable tail");
        assert_eq!(session.search(&spec).unwrap().count, 0);

        let previous_stable = session.stable_lines();
        writeln!(f).unwrap();
        f.flush().unwrap();
        session.remap_and_index_step(usize::MAX).unwrap();
        let matcher = SearchMatcher::new(&spec).unwrap();
        let matches =
            session.search_indexed_range(&matcher, previous_stable, session.stable_lines());
        let summary = session.append_search_results(&spec, matches);
        assert_eq!(summary.count, 1);
        assert_eq!(session.search_next(1, SearchDirection::Next), Some(1));

        let duplicate_scan =
            session.search_indexed_range(&matcher, session.stable_lines(), session.stable_lines());
        assert_eq!(
            session.append_search_results(&spec, duplicate_scan).count,
            1
        );
    }

    #[test]
    fn growing_tail_is_hidden_from_rows_counts_minimap_and_export_until_sealed() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "04-20 12:06:02.125   146   179 E Crash: private tail").unwrap();
        f.flush().unwrap();

        let mut session = Session::open_growing(f.path()).unwrap();
        session.index_all();
        assert_eq!(session.total_lines(), 1, "diagnostic line start exists");
        assert_eq!(session.stable_lines(), 0);
        assert_eq!(session.filtered_count(), 0);
        assert!(session.get_rows(0, 1).is_empty());
        assert_eq!(
            session.export_plan_snapshot(RowsView::All),
            ExportPlan::AllLines { total: 0 }
        );
        assert_eq!(
            session.minimap(10),
            Minimap {
                bucket_count: 0,
                bookmarks: Vec::new(),
                errors: Vec::new(),
            }
        );
        let mut bytes = Vec::new();
        assert_eq!(session.append_line_bytes(0, &mut bytes), 0);
        assert_eq!(session.append_line_range_bytes(0, 1, &mut bytes), (0, 0));
        assert!(bytes.is_empty());

        session.seal_growing_input().unwrap();
        assert_eq!(session.filtered_count(), 1);
        assert_eq!(session.get_rows(0, 1).len(), 1);
        assert_eq!(
            session.export_plan_snapshot(RowsView::All),
            ExportPlan::AllLines { total: 1 }
        );
    }

    #[test]
    fn paused_input_requires_resume_and_sealed_input_rejects_growth() {
        let mut paused_file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            paused_file,
            "04-20 12:06:02.125   146   179 I Stream: first"
        )
        .unwrap();
        paused_file.flush().unwrap();
        let mut paused = Session::open_growing(paused_file.path()).unwrap();
        paused.index_all();
        paused.pause_growing_input().unwrap();

        writeln!(
            paused_file,
            "04-20 12:06:02.225   146   179 I Stream: second"
        )
        .unwrap();
        paused_file.flush().unwrap();
        assert!(paused.remap_and_index_step(usize::MAX).is_err());
        assert_eq!(paused.stable_lines(), 1);

        paused.resume_paused_input().unwrap();
        paused.remap_and_index_step(usize::MAX).unwrap();
        assert_eq!(paused.stable_lines(), 2);
        paused.seal_growing_input().unwrap();

        writeln!(
            paused_file,
            "04-20 12:06:02.325   146   179 I Stream: forbidden"
        )
        .unwrap();
        paused_file.flush().unwrap();
        assert!(paused.remap_and_index_step(usize::MAX).is_err());
        assert_eq!(paused.stable_lines(), 2);
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
    fn nearest_filtered_mapping_prefers_the_preceding_line_on_a_tie() {
        let f = temp_filter_log();
        let mut session = Session::open(f.path()).unwrap();
        session.index_all();
        let spec = FilterSpec {
            tag_include: FilterField::plain(true, "synthetic-active-filter"),
            ..Default::default()
        };
        session.apply_filter_results(&spec, vec![0, 2]);

        assert_eq!(
            session.nearest_result_for_line_no(2),
            Some(ResultTarget {
                line_no: 1,
                result_index: 0
            })
        );
        assert_eq!(
            session.nearest_result_for_line_no(3),
            Some(ResultTarget {
                line_no: 3,
                result_index: 1
            })
        );
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
        assert_eq!(map.bucket_count, 4);
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
        assert_eq!(map.bucket_count, 4);
        assert_eq!(
            map.errors,
            vec![MinimapBucket {
                bucket: 0,
                count: 2
            }]
        );
    }

    #[test]
    fn minimap_uses_effective_bucket_count_for_short_results() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for i in 0..13 {
            writeln!(
                file,
                "04-20 12:06:02.{i:03}   300   330 E AndroidRuntime: error {i}"
            )
            .unwrap();
        }
        let mut session = Session::open(file.path()).unwrap();
        session.index_all();

        let map = session.minimap(180);

        assert_eq!(map.bucket_count, 13);
        assert_eq!(
            map.errors
                .iter()
                .map(|entry| entry.bucket)
                .collect::<Vec<_>>(),
            (0..13).collect::<Vec<_>>()
        );
        assert!(map.errors.iter().all(|entry| entry.count == 1));
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
        assert_eq!(s.active_filter_spec(), None);
        assert_eq!(s.desired_filter_spec(), Some(spec.clone()));

        let matcher = FilterMatcher::new(&spec).unwrap();
        let matches = s.filter_indexed_range(&matcher, 0, s.total_lines());
        assert_eq!(s.apply_filter_results(&spec, matches), 2);
        assert_eq!(s.active_filter_spec(), Some(spec));
    }

    #[test]
    fn pending_filter_does_not_mix_new_spec_into_applied_results() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let applied = FilterSpec {
            tag_include: FilterField::plain(true, "Network"),
            ..Default::default()
        };
        let pending = FilterSpec {
            tag_include: FilterField::plain(true, "Payment"),
            ..Default::default()
        };

        assert_eq!(s.set_filter(&applied).unwrap(), 2);
        assert_eq!(s.set_filter_pending(&pending).unwrap(), None);

        assert_eq!(s.active_filter_spec(), Some(applied));
        assert_eq!(s.desired_filter_spec(), Some(pending));
        assert_eq!(
            s.get_rows_for_view(RowsView::Filtered, 0, 10)
                .into_iter()
                .map(|(_, row)| row.tag)
                .collect::<Vec<_>>(),
            vec!["Network", "Network"]
        );
    }

    #[test]
    fn invalid_pending_filter_preserves_the_applied_dataset() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let applied = FilterSpec {
            tag_include: FilterField::plain(true, "Network"),
            ..Default::default()
        };
        let invalid = FilterSpec {
            tag_include: FilterField {
                enabled: true,
                pattern: "(".to_string(),
                regex: true,
            },
            ..Default::default()
        };

        assert_eq!(s.set_filter(&applied).unwrap(), 2);
        assert!(s.set_filter_pending(&invalid).is_err());
        assert_eq!(s.active_filter_spec(), Some(applied.clone()));
        assert_eq!(s.desired_filter_spec(), Some(applied));
        assert_eq!(s.filtered_count(), 2);
    }

    #[test]
    fn empty_search_clears_active_search_spec_and_matches() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let spec = SearchSpec::plain("Network");
        assert!(s.set_search_pending(&spec).unwrap());
        assert_eq!(s.active_search_spec(), None);
        assert_eq!(s.desired_search_spec(), Some(spec));

        let empty = SearchSpec::plain("");
        assert!(!s.set_search_pending(&empty).unwrap());
        assert_eq!(s.active_search_spec(), None);
        assert_eq!(s.desired_search_spec(), None);
        assert_eq!(s.search_next(1, SearchDirection::Next), None);
    }

    #[test]
    fn pending_search_does_not_mix_new_spec_into_applied_matches() {
        let f = temp_filter_log();
        let mut s = Session::open(f.path()).unwrap();
        s.index_all();
        let applied = SearchSpec::plain("Network");
        let pending = SearchSpec::plain("Payment");

        assert_eq!(s.search(&applied).unwrap().count, 2);
        assert!(s.set_search_pending(&pending).unwrap());

        assert_eq!(s.active_search_spec(), Some(applied));
        assert_eq!(s.desired_search_spec(), Some(pending));
        assert_eq!(s.search_next(1, SearchDirection::Next), Some(2));
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
        assert_eq!(map.bucket_count, 4);
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
        // 过滤后结果为行 2(书签, result 0)与行 7(错误, result 1);
        // 请求 4 桶时实际量程收缩为 2 桶。
        s.set_filter(&FilterSpec {
            word_include: FilterField::plain(true, "m1|m6"),
            ..Default::default()
        })
        .unwrap();

        let map = s.minimap(4);
        assert_eq!(map.bucket_count, 2);
        assert_eq!(map.bookmarks, vec![0]);
        assert_eq!(
            map.errors,
            vec![MinimapBucket {
                bucket: 1,
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
    fn unstable_frontier_row_is_hidden_while_indexing() {
        // Gate 0 回归:索引未完成时,已经发现行首但尚未见到行尾的前沿行不可见。
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for _ in 0..2000 {
            writeln!(f, "04-20 12:06:02.125   146   179 D T: msg").unwrap();
        }
        let mut s = Session::open(f.path()).unwrap();
        s.index_step(100); // 只索引一小段,未完成
        assert!(!s.is_indexing_done());
        assert!(s.total_lines() > s.stable_lines());
        let rows = s.get_rows(s.stable_lines(), 1);
        assert!(rows.is_empty());
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

    #[cfg(unix)]
    #[test]
    fn growing_truncation_resets_all_stable_and_derived_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing-reset.log");
        std::fs::write(
            &path,
            "04-20 12:06:02.125   146   179 E Old: reset-marker padding-padding-padding-padding\n",
        )
        .unwrap();
        let mut session = Session::open_growing(&path).unwrap();
        session.index_all();
        let filter = FilterSpec {
            word_include: FilterField::plain(true, "reset-marker"),
            ..Default::default()
        };
        let search = SearchSpec::plain("reset-marker");
        assert_eq!(session.set_filter(&filter).unwrap(), 1);
        assert_eq!(session.search(&search).unwrap().count, 1);
        assert_eq!(session.error_count(), 1);

        std::fs::write(
            &path,
            "04-20 12:06:03.000   200   220 I Fresh: reset-marker",
        )
        .unwrap();
        let outcome = session.remap_and_index_step(usize::MAX).unwrap();
        assert!(outcome.reset);
        assert_eq!(session.total_lines(), 1);
        assert_eq!(session.stable_lines(), 0);
        assert_eq!(session.filtered_count(), 0);
        assert_eq!(session.search_next(1, SearchDirection::Next), None);
        assert_eq!(session.error_count(), 0);
        assert!(session.get_rows(0, 1).is_empty());

        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(writer).unwrap();
        writer.flush().unwrap();
        let previous_stable = session.stable_lines();
        session.remap_and_index_step(usize::MAX).unwrap();
        let filter_matcher = FilterMatcher::new(&filter).unwrap();
        let filter_matches =
            session.filter_indexed_range(&filter_matcher, previous_stable, session.stable_lines());
        assert_eq!(session.append_filter_results(&filter, filter_matches), 1);
        let search_matcher = SearchMatcher::new(&search).unwrap();
        let search_matches =
            session.search_indexed_range(&search_matcher, previous_stable, session.stable_lines());
        assert_eq!(
            session.append_search_results(&search, search_matches).count,
            1
        );
        assert_eq!(session.error_count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn growing_truncation_clears_bookmarks_and_their_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("growing-bookmark-reset.log");
        std::fs::write(
            &path,
            "04-20 12:06:02.125   146   179 I Old: first\n\
             04-20 12:06:02.225   146   179 I Old: second\n",
        )
        .unwrap();
        let mut session = Session::open_growing(&path).unwrap();
        session.index_all();
        assert!(session.toggle_bookmark(2).unwrap());
        let sidecar = crate::bookmarks::sidecar_path_for(&path);
        assert!(sidecar.exists());

        std::fs::write(
            &path,
            "04-20 12:06:03.000   200   220 I Fresh: replacement\n",
        )
        .unwrap();
        assert!(session.remap_and_index_step(usize::MAX).unwrap().reset);

        assert!(session.list_bookmarks().is_empty());
        assert!(!sidecar.exists());
        drop(session);
        let reopened = Session::open_growing(&path).unwrap();
        assert!(reopened.list_bookmarks().is_empty());
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

    #[test]
    fn incremental_hit_batches_append_or_merge_without_duplicates() {
        let mut hits = vec![1, 3, 5];
        append_sorted_unique(&mut hits, vec![7, 8, 9]);
        assert_eq!(hits, vec![1, 3, 5, 7, 8, 9]);

        append_sorted_unique(&mut hits, vec![3, 4, 4, 8, 10]);
        assert_eq!(hits, vec![1, 3, 4, 5, 7, 8, 9, 10]);
    }

    #[test]
    fn encoding_change_invalidates_decoded_filter_and_search_results_until_rescanned() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "04-20 12:06:02.125   146   179 E Example: matching payload"
        )
        .unwrap();
        let mut session = Session::open(file.path()).unwrap();
        session.index_all();
        let filter = FilterSpec {
            word_include: FilterField::plain(true, "matching"),
            ..Default::default()
        };
        let search = SearchSpec::plain("matching");
        assert_eq!(session.set_filter(&filter).unwrap(), 1);
        assert_eq!(session.search(&search).unwrap().count, 1);

        session.set_encoding(TextEncoding::Local);

        assert_eq!(session.filtered_count(), 0);
        assert_eq!(session.search_next(0, SearchDirection::Next), None);
        assert_eq!(session.desired_filter_spec(), Some(filter));
        assert_eq!(session.desired_search_spec(), Some(search));
    }

    fn java_problem_log() -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "07-26 12:00:00.000   42   42 E AndroidRuntime: FATAL EXCEPTION: main"
        )
        .unwrap();
        writeln!(
            file,
            "07-26 12:00:00.001   42   42 E AndroidRuntime: Process: com.example.app, PID: 42"
        )
        .unwrap();
        writeln!(
            file,
            "07-26 12:00:00.002   42   42 E AndroidRuntime: java.lang.IllegalStateException: sample"
        )
        .unwrap();
        file
    }

    #[test]
    fn problem_scans_advance_in_bounded_stable_line_steps_and_finish_once() {
        let file = java_problem_log();
        let mut session = Session::open(file.path()).unwrap();
        session.index_all();

        let first = session.scan_problems_step(2);
        assert_eq!(first.scanned_lines, 2);
        assert_eq!(first.stable_lines, 3);
        assert!(!first.finished);
        assert_eq!(session.problem_stats().stored_occurrence_count, 0);

        let second = session.scan_problems_step(usize::MAX);
        assert_eq!(second.scanned_lines, 3);
        assert!(second.caught_up);
        assert!(!second.finished);

        let finished = session.finish_problem_input();
        assert!(finished.finished);
        assert_eq!(session.problem_stats().stored_occurrence_count, 1);
        let revision = session.problem_stats().revision;

        let repeated = session.finish_problem_input();
        assert!(repeated.finished);
        assert_eq!(session.problem_stats().revision, revision);
    }

    #[test]
    fn problem_scan_step_has_a_deterministic_byte_budget() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let payload = "x".repeat(8 * 1024);
        for index in 0..100 {
            writeln!(
                file,
                "07-26 12:00:00.{index:03}   42   42 I Example: {payload}"
            )
            .unwrap();
        }
        let mut session = Session::open(file.path()).unwrap();
        session.index_all();

        let first = session.scan_problems_step(usize::MAX);

        assert!(first.scanned_lines > 0);
        assert!(first.scanned_lines < first.stable_lines);
        let (_, accepted_end) = session
            .indexer
            .line_span(
                session.source.bytes(),
                first.scanned_lines - 1,
                session.indexed_frontier(),
            )
            .expect("accepted line span");
        assert!(accepted_end <= PROBLEM_SCAN_MAX_BYTES);
        while !session.scan_problems_step(usize::MAX).caught_up {}
        assert_eq!(session.problem_scanned_lines(), session.stable_lines());
    }

    #[test]
    fn session_exposes_the_unified_problem_memory_ledger() {
        let file = java_problem_log();
        let session = Session::open(file.path()).unwrap();

        let memory = session.problem_memory_stats();

        assert_eq!(
            memory.limit_bytes,
            crate::problems::DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES
        );
        assert_eq!(memory.charged_bytes, 0);
        assert_eq!(memory.retained_heap_bytes, 0);
    }

    #[test]
    fn growing_partial_tail_is_not_seen_by_problem_analysis_until_sealed() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "07-26 12:00:00.000   42   42 E AndroidRuntime: FATAL EXCEPTION: main"
        )
        .unwrap();
        writeln!(
            file,
            "07-26 12:00:00.001   42   42 E AndroidRuntime: Process: com.example.app, PID: 42"
        )
        .unwrap();
        write!(
            file,
            "07-26 12:00:00.002   42   42 E AndroidRuntime: java.lang.IllegalStateException: sample"
        )
        .unwrap();
        file.flush().unwrap();

        let mut session = Session::open_growing(file.path()).unwrap();
        session.index_all();
        assert_eq!(session.stable_lines(), 2);
        let step = session.scan_problems_step(4096);
        assert_eq!(step.scanned_lines, 2);
        assert_eq!(session.problem_stats().stored_occurrence_count, 0);
        assert!(!session.finish_problem_input().finished);

        writeln!(file).unwrap();
        file.flush().unwrap();
        session.remap_and_index_step(usize::MAX).unwrap();
        assert_eq!(session.stable_lines(), 3);
        session.scan_problems_step(4096);
        assert_eq!(session.problem_stats().stored_occurrence_count, 0);

        session.seal_growing_input().unwrap();
        assert!(session.finish_problem_input().finished);
        assert_eq!(session.problem_stats().stored_occurrence_count, 1);
    }

    #[test]
    fn static_divider_text_does_not_authenticate_eventlog_source() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "--------- beginning of main").unwrap();
        writeln!(
            file,
            "07-26 12:00:00.000  100  100 I Example: ordinary payload"
        )
        .unwrap();
        writeln!(file, "--------- beginning of events").unwrap();
        writeln!(
            file,
            "07-26 12:00:01.000  100  100 I am_crash: [321,com.example.app,0,java.lang.IllegalStateException,bad state,Example.kt,42]"
        )
        .unwrap();

        let mut session = Session::open(file.path()).unwrap();
        session.index_all();
        session.scan_problems_step(4096);
        session.finish_problem_input();

        assert_eq!(session.problem_stats().stored_occurrence_count, 0);
    }

    #[test]
    fn reliable_source_span_survives_bounded_problem_scan_chunks() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "--------- beginning of events").unwrap();
        writeln!(
            file,
            "07-26 12:00:01.000  100  100 I am_crash: [321,com.example.app,0,java.lang.IllegalStateException,bad state,Example.kt,42]"
        )
        .unwrap();

        let mut session = Session::open(file.path()).unwrap();
        session
            .add_problem_source_span(
                SourceSpan::new(1, 1, crate::problems::LogBuffer::Events).unwrap(),
            )
            .unwrap();
        session.index_all();
        assert_eq!(session.scan_problems_step(1).scanned_lines, 1);
        assert_eq!(session.problem_stats().stored_occurrence_count, 0);
        assert_eq!(session.scan_problems_step(1).scanned_lines, 2);
        session.finish_problem_input();

        assert_eq!(session.problem_stats().stored_occurrence_count, 1);
    }

    #[test]
    fn growing_partial_divider_never_authenticates_live_append() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "--------- beginning of events").unwrap();
        file.flush().unwrap();

        let mut session = Session::open_growing(file.path()).unwrap();
        session.set_problem_input_coverage(InputCoverage::adb_live(
            crate::problems::BufferSet::EVENTS,
            RangeCompleteness::StartTruncated,
        ));
        session.index_all();
        assert_eq!(session.stable_lines(), 0);
        assert_eq!(session.scan_problems_step(4096).scanned_lines, 0);

        writeln!(file).unwrap();
        writeln!(
            file,
            "07-26 12:00:01.000  100  100 I am_anr: [321,com.example.app,7,Input dispatching timed out]"
        )
        .unwrap();
        file.flush().unwrap();
        session.remap_and_index_step(usize::MAX).unwrap();
        assert_eq!(session.stable_lines(), 2);
        session.scan_problems_step(4096);
        session.seal_growing_input().unwrap();
        session.finish_problem_input();

        assert_eq!(session.problem_stats().stored_occurrence_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn growing_truncation_resets_divider_provenance_to_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provenance-reset.log");
        std::fs::write(
            &path,
            "--------- beginning of kernel\n\
             filler filler filler filler filler filler filler filler filler filler\n\
             filler filler filler filler filler filler filler filler filler filler\n",
        )
        .unwrap();
        let mut session = Session::open_growing(&path).unwrap();
        session.set_problem_input_coverage(InputCoverage::adb_live(
            crate::problems::BufferSet::KERNEL,
            RangeCompleteness::StartTruncated,
        ));
        session.index_all();
        session.scan_problems_step(4096);

        std::fs::write(
            &path,
            "07-26 12:00:02.000  0  0 E kernel: Out of memory: Killed process 333 (com.kernel.app) total-vm:42kB\n",
        )
        .unwrap();
        let remap = session.remap_and_index_step(usize::MAX).unwrap();
        assert!(remap.reset);
        session.scan_problems_step(4096);
        session.seal_growing_input().unwrap();
        session.finish_problem_input();

        assert_eq!(
            session.problem_stats().stored_occurrence_count,
            0,
            "the replacement source must not inherit the old kernel divider"
        );
    }

    #[test]
    fn kernel_divider_text_does_not_admit_kernel_oom_kill() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "--------- beginning of kernel").unwrap();
        writeln!(
            file,
            "07-26 12:00:02.000  0  0 E kernel: Out of memory: Killed process 333 (com.kernel.app) total-vm:42kB"
        )
        .unwrap();

        let mut session = Session::open(file.path()).unwrap();
        session.index_all();
        session.scan_problems_step(4096);
        session.finish_problem_input();

        assert_eq!(session.problem_stats().stored_occurrence_count, 0);
    }

    #[test]
    fn explicit_source_span_wins_divider_conflict_only_on_its_rows() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "--------- beginning of events").unwrap();
        writeln!(
            file,
            "07-26 12:00:01.000  100  100 I am_crash: [321,blocked.app,0,java.lang.IllegalStateException,blocked,Blocked.kt,42]"
        )
        .unwrap();
        writeln!(
            file,
            "07-26 12:00:02.000  100  100 I am_crash: [654,accepted.app,0,java.lang.IllegalStateException,accepted,Accepted.kt,42]"
        )
        .unwrap();

        let mut session = Session::open(file.path()).unwrap();
        session
            .add_problem_source_span(
                SourceSpan::new(1, 1, crate::problems::LogBuffer::Main).unwrap(),
            )
            .unwrap();
        session
            .add_problem_source_span(
                SourceSpan::new(2, 2, crate::problems::LogBuffer::Events).unwrap(),
            )
            .unwrap();
        session.index_all();
        session.scan_problems_step(4096);
        session.finish_problem_input();

        assert_eq!(session.problem_stats().stored_occurrence_count, 1);
        assert_eq!(session.problem_event(ProblemEventId(0)).unwrap().pid(), 654);
    }

    #[test]
    fn problem_query_snapshots_are_reachable_only_through_compact_ids() {
        let file = java_problem_log();
        let mut session = Session::open(file.path()).unwrap();
        session.index_all();
        session.scan_problems_step(4096);
        session.finish_problem_input();

        let snapshot = session
            .create_problem_group_snapshot(&crate::problems::GroupQuery::default())
            .unwrap();
        let groups = session
            .problem_group_snapshot_page(snapshot, crate::problems::PageSpec::new(0, 100).unwrap())
            .unwrap();
        assert_eq!(groups.items.len(), 1);

        let occurrences = session
            .create_problem_occurrence_snapshot(groups.items[0].id)
            .unwrap();
        let page = session
            .problem_occurrence_snapshot_page(
                occurrences,
                crate::problems::PageSpec::new(0, 100).unwrap(),
            )
            .unwrap();
        assert_eq!(page.items.len(), 1);
        let event = session.problem_event(page.items[0]).unwrap();
        assert_eq!(event.anchor_line(), 0);
        assert!(!session
            .problem_event_observations(page.items[0])
            .unwrap()
            .is_empty());
    }
}
