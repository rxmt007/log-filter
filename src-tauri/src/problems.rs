use crate::dto::{
    AnalysisTokenDto, ProblemDetailDto, ProblemDetailRequest, ProblemGroupPageDto,
    ProblemGroupQueryRequest, ProblemGroupSortDto, ProblemKindDto, ProblemOccurrencePageDto,
    ProblemOccurrenceQueryRequest, ProblemReleaseSnapshotRequest, ProblemStatsDto,
    ProblemsProgressDto, ProblemsStatusDto,
};
use crate::state::{
    AppState, ProblemAnalysisIdentity, ProblemCursorError, ProblemCursorRegistry, ProblemPageQuery,
    ProblemSnapshotLease,
};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

pub(crate) const PROBLEM_CATCH_UP_STEPS_PER_INDEX: usize = 32;
const PROBLEM_SCAN_CHUNK_LINES: usize = 4096;
const PROBLEM_PROGRESS_STRIDE: u64 = 65_536;
const PROBLEM_SNAPSHOT_RECORDS_PER_LOCK: usize = 4096;

fn problem_page_spec(
    position: usize,
    limit: Option<usize>,
) -> Result<logcore::problems::PageSpec, String> {
    logcore::problems::PageSpec::new(position, limit.unwrap_or(100))
        .map_err(|_| "problem page limit must be between 1 and 200".to_string())
}

fn problem_kind_from_dto(kind: ProblemKindDto) -> logcore::problems::ProblemKind {
    match kind {
        ProblemKindDto::JavaCrash => logcore::problems::ProblemKind::JavaCrash,
        ProblemKindDto::JavaOom => logcore::problems::ProblemKind::JavaOom,
        ProblemKindDto::Anr => logcore::problems::ProblemKind::Anr,
        ProblemKindDto::NativeCrash => logcore::problems::ProblemKind::NativeCrash,
        ProblemKindDto::ProcessRestart => logcore::problems::ProblemKind::ProcessRestart,
        ProblemKindDto::SignalExit => logcore::problems::ProblemKind::SignalExit,
        ProblemKindDto::LmkKill => logcore::problems::ProblemKind::LmkKill,
        ProblemKindDto::KernelOomKill => logcore::problems::ProblemKind::KernelOomKill,
    }
}

fn problem_kind_code(kind: ProblemKindDto) -> u8 {
    match kind {
        ProblemKindDto::JavaCrash => 0,
        ProblemKindDto::JavaOom => 1,
        ProblemKindDto::Anr => 2,
        ProblemKindDto::NativeCrash => 3,
        ProblemKindDto::ProcessRestart => 4,
        ProblemKindDto::SignalExit => 5,
        ProblemKindDto::LmkKill => 6,
        ProblemKindDto::KernelOomKill => 7,
    }
}

fn problem_group_sort_code(sort: ProblemGroupSortDto) -> u8 {
    match sort {
        ProblemGroupSortDto::LastSeenDesc => 0,
        ProblemGroupSortDto::CountDesc => 1,
    }
}

fn problem_analysis_identity(token: AnalysisTokenDto) -> ProblemAnalysisIdentity {
    ProblemAnalysisIdentity {
        session_generation: token.session_generation,
        analysis_generation: token.analysis_generation,
    }
}

fn problem_group_page_query(
    kind: Option<ProblemKindDto>,
    sort: ProblemGroupSortDto,
) -> ProblemPageQuery {
    ProblemPageQuery::Groups {
        kind: kind.map(problem_kind_code),
        sort: problem_group_sort_code(sort),
    }
}

fn problem_cursor_error(error: ProblemCursorError) -> String {
    error.code().to_string()
}

fn ensure_current_analysis(state: &AppState, token: AnalysisTokenDto) -> Result<(), String> {
    let Some(guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return Err("stale-analysis-token".to_string());
    };
    if guard.is_none() {
        return Err("no-active-session".to_string());
    }
    Ok(())
}

fn release_problem_snapshot_ids(
    state: &AppState,
    token: AnalysisTokenDto,
    snapshot_ids: impl IntoIterator<Item = u64>,
) {
    let Some(mut guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return;
    };
    let Some(session) = guard.as_mut() else {
        return;
    };
    for raw in snapshot_ids {
        if let Some(snapshot) = logcore::problems::QuerySnapshotId::from_raw(raw) {
            session.release_problem_snapshot(snapshot);
        }
    }
}

fn drain_problem_snapshot_releases(state: &AppState, token: AnalysisTokenDto) {
    release_problem_snapshot_ids(state, token, state.problem_cursors.drain_core_releases());
}

fn register_problem_snapshot_capability(
    registry: &ProblemCursorRegistry,
    snapshot_id: u64,
    analysis: ProblemAnalysisIdentity,
    query: ProblemPageQuery,
    mut release_core: impl FnMut(u64),
) -> Result<ProblemSnapshotLease, ProblemCursorError> {
    let registered = registry.register_snapshot(snapshot_id, analysis, query);
    for retired in registry.drain_core_releases() {
        release_core(retired);
    }
    if registered.is_err() {
        release_core(snapshot_id);
    }
    registered
}

fn rollback_problem_page(state: &AppState, token: AnalysisTokenDto, lease: &ProblemSnapshotLease) {
    if lease.is_initial_page() {
        state.problem_cursors.abandon_page(lease);
    } else {
        state.problem_cursors.rollback_page(lease);
    }
    drain_problem_snapshot_releases(state, token);
}

fn problem_snapshot_error(error: logcore::problems::SnapshotError) -> String {
    use logcore::problems::SnapshotError;
    match error {
        SnapshotError::GroupNotFound => "problem-group-not-found",
        SnapshotError::NotFound => "snapshot-not-found",
        SnapshotError::Expired => "snapshot-expired",
        SnapshotError::Evicted => "snapshot-evicted",
        SnapshotError::Released => "snapshot-released",
        SnapshotError::Reset => "snapshot-reset",
        SnapshotError::WrongKind => "snapshot-wrong-kind",
        SnapshotError::QueryMismatch => "snapshot-query-mismatch",
        SnapshotError::IdVectorLimit => "snapshot-capacity",
        SnapshotError::Allocation => "snapshot-allocation-failed",
        SnapshotError::IdExhausted => "snapshot-id-exhausted",
    }
    .to_string()
}

fn create_problem_group_snapshot_batched(
    state: &AppState,
    token: AnalysisTokenDto,
    query: &logcore::problems::GroupQuery,
) -> Result<logcore::problems::QuerySnapshotId, String> {
    let capture = {
        let Some(guard) =
            state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
        else {
            return Err("stale-analysis-token".to_string());
        };
        guard
            .as_ref()
            .ok_or_else(|| "no-active-session".to_string())?
            .problem_group_snapshot_capture()
    };
    let mut records = Vec::new();
    records
        .try_reserve_exact(capture.group_count)
        .map_err(|_| "snapshot-allocation-failed".to_string())?;
    let mut offset = 0;
    while offset < capture.group_count {
        let batch = {
            let Some(guard) =
                state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
            else {
                return Err("stale-analysis-token".to_string());
            };
            guard
                .as_ref()
                .ok_or_else(|| "no-active-session".to_string())?
                .problem_group_sort_records(
                    query,
                    capture,
                    offset,
                    PROBLEM_SNAPSHOT_RECORDS_PER_LOCK,
                )
                .map_err(problem_snapshot_error)?
        };
        records.extend(batch);
        offset = offset.saturating_add(PROBLEM_SNAPSHOT_RECORDS_PER_LOCK);
        std::thread::yield_now();
    }
    records.sort_by(|left, right| {
        logcore::problems::GroupSortRecord::compare(left, right, query.sort)
    });
    let mut ids = Vec::new();
    ids.try_reserve_exact(records.len())
        .map_err(|_| "snapshot-allocation-failed".to_string())?;
    ids.extend(records.into_iter().map(|record| record.id));

    let Some(mut guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return Err("stale-analysis-token".to_string());
    };
    guard
        .as_mut()
        .ok_or_else(|| "no-active-session".to_string())?
        .install_problem_group_snapshot(ids, capture.revision, *query)
        .map_err(problem_snapshot_error)
}

fn problem_stats_from(session: &logcore::session::Session) -> ProblemStatsDto {
    ProblemStatsDto::from_compact(session.problem_stats())
}

fn problems_status_from(
    session: &logcore::session::Session,
    token: AnalysisTokenDto,
) -> ProblemsStatusDto {
    ProblemsStatusDto {
        analysis_token: token,
        scanned_lines: session.problem_scanned_lines() as u64,
        stable_lines: session.stable_lines() as u64,
        scanning: !session.problem_analysis_finished()
            && session.problem_scanned_lines() < session.stable_lines(),
        finished: session.problem_analysis_finished(),
        coverage: session.problem_input_coverage().into(),
        stats: problem_stats_from(session),
    }
}

pub(crate) fn problems_progress_from(
    session: &logcore::session::Session,
    token: AnalysisTokenDto,
) -> ProblemsProgressDto {
    let stats = problem_stats_from(session);
    ProblemsProgressDto {
        scanned_lines: session.problem_scanned_lines() as u64,
        stable_lines: session.stable_lines() as u64,
        coverage: session.problem_input_coverage().into(),
        observed_occurrence_count: stats.observed_occurrence_count,
        stored_occurrence_count: stats.stored_occurrence_count,
        dropped_occurrence_count: stats.dropped_occurrence_count,
        provisional_occurrence_count: stats.provisional_occurrence_count,
        stored_group_count: stats.stored_group_count,
        ungrouped_dropped_occurrence_count: stats.ungrouped_dropped_occurrence_count,
        dropped_recent_observation_count: stats.dropped_recent_observation_count,
        correlation_limited: stats.correlation_limited,
        revision: stats.revision,
        done: session.problem_analysis_finished(),
        limited: stats.limited,
        session_generation: token.session_generation,
        analysis_generation: token.analysis_generation,
    }
}

pub(crate) fn step_problem_analysis(
    state: &AppState,
    session_generation: u64,
    analysis_generation: u64,
    finish_if_terminal: bool,
) -> Option<ProblemsProgressDto> {
    let mut guard = state.lock_analysis_if_current(session_generation, analysis_generation)?;
    let session = guard.as_mut()?;
    let step = session.scan_problems_step(PROBLEM_SCAN_CHUNK_LINES);
    if finish_if_terminal && step.caught_up {
        session.finish_problem_input();
    }
    Some(problems_progress_from(
        session,
        AnalysisTokenDto {
            session_generation,
            analysis_generation,
        },
    ))
}

#[derive(Default)]
pub(crate) struct ProblemProgressGate {
    last_scanned_lines: u64,
    last_revision: u64,
    last_limited: bool,
    last_correlation_limited: bool,
    last_emit: Option<Instant>,
    pending: bool,
    deferred_scheduled: bool,
}

impl ProblemProgressGate {
    pub(crate) fn should_emit(&mut self, progress: &ProblemsProgressDto) -> bool {
        self.should_emit_at(progress, Instant::now())
    }

    fn should_emit_at(&mut self, progress: &ProblemsProgressDto, now: Instant) -> bool {
        const MIN_INTERVAL: Duration = Duration::from_millis(100);
        let first_result = self.last_revision == 0 && progress.revision > 0;
        let limit_transition = progress.limited != self.last_limited
            || progress.correlation_limited != self.last_correlation_limited;
        let work_changed = progress.revision != self.last_revision
            || progress
                .scanned_lines
                .saturating_sub(self.last_scanned_lines)
                >= PROBLEM_PROGRESS_STRIDE;
        let cadence_ready = self
            .last_emit
            .is_none_or(|last_emit| now.duration_since(last_emit) >= MIN_INTERVAL);
        let emit =
            progress.done || first_result || limit_transition || (work_changed && cadence_ready);
        if emit {
            self.last_scanned_lines = progress.scanned_lines;
            self.last_revision = progress.revision;
            self.last_limited = progress.limited;
            self.last_correlation_limited = progress.correlation_limited;
            self.last_emit = Some(now);
            self.pending = false;
        } else if work_changed {
            self.pending = true;
        }
        emit
    }

    pub(crate) fn schedule_deferred(&mut self, now: Instant) -> Option<Duration> {
        const MIN_INTERVAL: Duration = Duration::from_millis(100);
        if !self.pending || self.deferred_scheduled {
            return None;
        }
        self.deferred_scheduled = true;
        let elapsed = self
            .last_emit
            .map_or(MIN_INTERVAL, |last_emit| now.duration_since(last_emit));
        Some(MIN_INTERVAL.saturating_sub(elapsed) + Duration::from_millis(1))
    }

    pub(crate) fn clear_deferred(&mut self) {
        self.deferred_scheduled = false;
    }
}

pub(crate) fn spawn_problem_catchup(
    state: AppState,
    app: AppHandle,
    session_generation: u64,
    analysis_generation: u64,
) {
    std::thread::spawn(move || {
        let mut progress_gate = ProblemProgressGate::default();
        while let Some(progress) =
            step_problem_analysis(&state, session_generation, analysis_generation, true)
        {
            let caught_up = progress.scanned_lines >= progress.stable_lines;
            if progress_gate.should_emit(&progress) {
                let _ = app.emit("problems:progress", progress.clone());
            }
            if caught_up {
                break;
            }
            std::thread::yield_now();
        }
    });
}

pub(crate) fn problem_export_range(
    event: logcore::problems::ProblemEvent,
    with_context: bool,
    radius: u32,
    stable_lines: u64,
) -> (u64, u64) {
    let start = logcore::problems::public_line_number(event.start_line());
    let end = logcore::problems::public_line_number(event.end_line()).min(stable_lines);
    if !with_context {
        return (start.min(stable_lines.saturating_add(1)), end);
    }
    (
        start.saturating_sub(u64::from(radius)).max(1),
        end.saturating_add(u64::from(radius)).min(stable_lines),
    )
}

#[tauri::command]
pub fn get_problems_status(state: State<AppState>) -> ProblemsStatusDto {
    let (session_generation, analysis_generation, guard) = state.lock_session_with_generations();
    let token = AnalysisTokenDto {
        session_generation,
        analysis_generation,
    };
    match guard.as_ref() {
        Some(session) => problems_status_from(session, token),
        None => ProblemsStatusDto {
            analysis_token: token,
            scanned_lines: 0,
            stable_lines: 0,
            scanning: false,
            finished: false,
            coverage: logcore::problems::InputCoverage::static_file(
                logcore::problems::RangeCompleteness::Unknown,
            )
            .into(),
            stats: ProblemStatsDto::from_compact(logcore::problems::ProblemStats::default()),
        },
    }
}

#[tauri::command]
pub async fn get_problem_groups(
    request: ProblemGroupQueryRequest,
    state: State<'_, AppState>,
) -> Result<ProblemGroupPageDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || get_problem_groups_blocking(request, &app_state))
        .await
        .map_err(|error| format!("problem group worker failed: {error}"))?
}

fn get_problem_groups_blocking(
    request: ProblemGroupQueryRequest,
    state: &AppState,
) -> Result<ProblemGroupPageDto, String> {
    let token = request.expected_analysis_token;
    let analysis = problem_analysis_identity(token);
    let (lease, query) = match request.cursor.as_deref() {
        Some(cursor) => {
            ensure_current_analysis(state, token)?;
            if request.kind.is_some() && request.sort.is_none() {
                return Err("problem-cursor-query-incomplete".to_string());
            }
            let expected_query = request
                .sort
                .map(|sort| problem_group_page_query(request.kind, sort));
            let lease = state
                .problem_cursors
                .reserve_cursor(cursor, analysis, expected_query);
            drain_problem_snapshot_releases(state, token);
            let lease = lease.map_err(problem_cursor_error)?;
            let ProblemPageQuery::Groups { .. } = lease.query else {
                state.problem_cursors.rollback_page(&lease);
                return Err("problem-cursor-query-mismatch".to_string());
            };
            let query = match lease.query {
                ProblemPageQuery::Groups { kind, sort } => logcore::problems::GroupQuery {
                    kind: kind.and_then(|code| {
                        [
                            ProblemKindDto::JavaCrash,
                            ProblemKindDto::JavaOom,
                            ProblemKindDto::Anr,
                            ProblemKindDto::NativeCrash,
                            ProblemKindDto::ProcessRestart,
                            ProblemKindDto::SignalExit,
                            ProblemKindDto::LmkKill,
                            ProblemKindDto::KernelOomKill,
                        ]
                        .get(usize::from(code))
                        .copied()
                        .map(problem_kind_from_dto)
                    }),
                    sort: if sort == problem_group_sort_code(ProblemGroupSortDto::CountDesc) {
                        logcore::problems::GroupSort::ObservedCountDesc
                    } else {
                        logcore::problems::GroupSort::LastOccurrenceDesc
                    },
                },
                ProblemPageQuery::Occurrences { .. } => unreachable!(),
            };
            (lease, query)
        }
        None => {
            let sort = request
                .sort
                .ok_or_else(|| "problem group first page requires sort".to_string())?;
            let query = logcore::problems::GroupQuery {
                kind: request.kind.map(problem_kind_from_dto),
                sort: match sort {
                    ProblemGroupSortDto::LastSeenDesc => {
                        logcore::problems::GroupSort::LastOccurrenceDesc
                    }
                    ProblemGroupSortDto::CountDesc => {
                        logcore::problems::GroupSort::ObservedCountDesc
                    }
                },
            };
            let snapshot = create_problem_group_snapshot_batched(state, token, &query)?;
            let mut releases = Vec::new();
            let lease = register_problem_snapshot_capability(
                &state.problem_cursors,
                snapshot.raw(),
                analysis,
                problem_group_page_query(request.kind, sort),
                |raw| releases.push(raw),
            );
            release_problem_snapshot_ids(state, token, releases);
            let lease = lease.map_err(problem_cursor_error)?;
            if let Err(error) = ensure_current_analysis(state, token) {
                state.problem_cursors.abandon_page(&lease);
                drain_problem_snapshot_releases(state, token);
                return Err(error);
            }
            (lease, query)
        }
    };
    let page_spec = match problem_page_spec(lease.position, request.limit) {
        Ok(page_spec) => page_spec,
        Err(error) => {
            rollback_problem_page(state, token, &lease);
            return Err(error);
        }
    };
    let snapshot = logcore::problems::QuerySnapshotId::from_raw(lease.snapshot_id)
        .ok_or_else(|| "problem-cursor-invalid".to_string())?;
    let Some(mut guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        rollback_problem_page(state, token, &lease);
        return Err("stale-analysis-token".to_string());
    };
    let page = match guard.as_mut() {
        Some(session) => session.problem_group_snapshot_page_for_query(snapshot, page_spec, query),
        None => {
            drop(guard);
            rollback_problem_page(state, token, &lease);
            return Err("no-active-session".to_string());
        }
    };
    drop(guard);
    let page = match page {
        Ok(page) => page,
        Err(error) => {
            rollback_problem_page(state, token, &lease);
            return Err(problem_snapshot_error(error));
        }
    };
    let next_cursor = match state.problem_cursors.commit_page(&lease, page.next_offset) {
        Ok(cursor) => cursor,
        Err(error) => {
            rollback_problem_page(state, token, &lease);
            return Err(problem_cursor_error(error));
        }
    };
    Ok(ProblemGroupPageDto::from_compact(
        token,
        page,
        lease.snapshot_handle.clone(),
        next_cursor,
    ))
}

#[tauri::command]
pub fn get_problem_occurrences(
    request: ProblemOccurrenceQueryRequest,
    state: State<AppState>,
) -> Result<ProblemOccurrencePageDto, String> {
    let token = request.expected_analysis_token;
    let analysis = problem_analysis_identity(token);
    ensure_current_analysis(state.inner(), token)?;
    let lease = match request.cursor.as_deref() {
        Some(cursor) => {
            let expected_query = request
                .group_id
                .map(|group_id| ProblemPageQuery::Occurrences { group_id });
            let lease = state
                .problem_cursors
                .reserve_cursor(cursor, analysis, expected_query);
            drain_problem_snapshot_releases(state.inner(), token);
            lease.map_err(problem_cursor_error)?
        }
        None => {
            let group_id = request
                .group_id
                .ok_or_else(|| "problem occurrence first page requires groupId".to_string())?;
            let group_id = logcore::problems::GroupId::from_raw(group_id);
            let mut guard = state
                .lock_analysis_if_current(token.session_generation, token.analysis_generation)
                .ok_or_else(|| "stale-analysis-token".to_string())?;
            let session = guard
                .as_mut()
                .ok_or_else(|| "no-active-session".to_string())?;
            let snapshot = session
                .create_problem_occurrence_snapshot(group_id)
                .map_err(problem_snapshot_error)?;
            let lease = register_problem_snapshot_capability(
                &state.problem_cursors,
                snapshot.raw(),
                analysis,
                ProblemPageQuery::Occurrences {
                    group_id: group_id.raw(),
                },
                |raw| {
                    if let Some(retired) = logcore::problems::QuerySnapshotId::from_raw(raw) {
                        session.release_problem_snapshot(retired);
                    }
                },
            );
            lease.map_err(problem_cursor_error)?
        }
    };
    let ProblemPageQuery::Occurrences { group_id } = lease.query else {
        state.problem_cursors.rollback_page(&lease);
        return Err("problem-cursor-query-mismatch".to_string());
    };
    let page_spec = match problem_page_spec(lease.position, request.limit) {
        Ok(page_spec) => page_spec,
        Err(error) => {
            rollback_problem_page(state.inner(), token, &lease);
            return Err(error);
        }
    };
    let group_id = logcore::problems::GroupId::from_raw(group_id);
    let snapshot = logcore::problems::QuerySnapshotId::from_raw(lease.snapshot_id)
        .ok_or_else(|| "problem-cursor-invalid".to_string())?;
    let Some(mut guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        rollback_problem_page(state.inner(), token, &lease);
        return Err("stale-analysis-token".to_string());
    };
    let session = match guard.as_mut() {
        Some(session) => session,
        None => {
            drop(guard);
            rollback_problem_page(state.inner(), token, &lease);
            return Err("no-active-session".to_string());
        }
    };
    let page =
        match session.problem_occurrence_snapshot_page_for_group(snapshot, page_spec, group_id) {
            Ok(page) => page,
            Err(error) => {
                drop(guard);
                rollback_problem_page(state.inner(), token, &lease);
                return Err(problem_snapshot_error(error));
            }
        };
    let next_position = page.next_offset;
    let dto = ProblemOccurrencePageDto::try_from_compact(
        token,
        page,
        lease.snapshot_handle.clone(),
        None,
        |id| session.problem_event(id),
    );
    drop(guard);
    let mut dto = match dto {
        Ok(dto) => dto,
        Err(_) => {
            rollback_problem_page(state.inner(), token, &lease);
            return Err("problem-event-not-found".to_string());
        }
    };
    dto.next_cursor = match state.problem_cursors.commit_page(&lease, next_position) {
        Ok(cursor) => cursor,
        Err(error) => {
            rollback_problem_page(state.inner(), token, &lease);
            return Err(problem_cursor_error(error));
        }
    };
    Ok(dto)
}

#[tauri::command]
pub fn get_problem_detail(
    request: ProblemDetailRequest,
    state: State<AppState>,
) -> Result<ProblemDetailDto, String> {
    let token = request.expected_analysis_token;
    let Some(guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return Err("stale-analysis-token".to_string());
    };
    let session = guard
        .as_ref()
        .ok_or_else(|| "no-active-session".to_string())?;
    let event_id = logcore::problems::ProblemEventId(request.event_id);
    let event = session
        .problem_event(event_id)
        .ok_or_else(|| "problem-event-not-found".to_string())?;
    let observations = session
        .problem_event_observations(event_id)
        .ok_or_else(|| "problem-event-not-found".to_string())?;
    Ok(ProblemDetailDto::from_compact(
        token,
        session.problem_stats().revision,
        event_id,
        event,
        observations,
    ))
}

#[tauri::command]
pub fn release_problem_snapshot(
    request: ProblemReleaseSnapshotRequest,
    state: State<AppState>,
) -> Result<bool, String> {
    let token = request.expected_analysis_token;
    ensure_current_analysis(state.inner(), token)?;
    let lease = state
        .problem_cursors
        .resolve_snapshot_handle(&request.snapshot_handle, problem_analysis_identity(token));
    drain_problem_snapshot_releases(state.inner(), token);
    let lease = lease.map_err(problem_cursor_error)?;
    let Some(mut guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return Err("stale-analysis-token".to_string());
    };
    let session = guard
        .as_mut()
        .ok_or_else(|| "no-active-session".to_string())?;
    let snapshot = logcore::problems::QuerySnapshotId::from_raw(lease.snapshot_id)
        .ok_or_else(|| "problem-snapshot-handle-invalid".to_string())?;
    let released = session.release_problem_snapshot(snapshot);
    state.problem_cursors.mark_snapshot_released(&lease);
    Ok(released)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn problems_pages_default_to_one_hundred_and_reject_unbounded_requests() {
        let page = problem_page_spec(0, None).unwrap();
        assert_eq!(page.offset(), 0);
        assert_eq!(page.limit(), 100);
        assert_eq!(problem_page_spec(5, Some(200)).unwrap().limit(), 200);
        assert_eq!(
            problem_page_spec(0, Some(0)).unwrap_err(),
            "problem page limit must be between 1 and 200"
        );
        assert_eq!(
            problem_page_spec(0, Some(201)).unwrap_err(),
            "problem page limit must be between 1 and 200"
        );
    }

    #[test]
    fn failed_first_page_capability_registration_releases_new_core_snapshot() {
        let registry = ProblemCursorRegistry::new();
        let analysis = ProblemAnalysisIdentity {
            session_generation: 7,
            analysis_generation: 2,
        };
        let query = ProblemPageQuery::Groups {
            kind: None,
            sort: 0,
        };
        for snapshot_id in 1..=8 {
            registry
                .register_snapshot(snapshot_id, analysis, query)
                .unwrap();
        }
        let mut released = Vec::new();
        assert_eq!(
            register_problem_snapshot_capability(&registry, 99, analysis, query, |raw| released
                .push(raw)),
            Err(ProblemCursorError::Capacity)
        );
        assert_eq!(released, vec![99]);
    }

    #[test]
    fn problem_context_export_range_is_clamped_to_stable_source_lines() {
        let event = logcore::problems::ProblemEvent::new(
            logcore::problems::ProblemEventDraft {
                start_line: 10,
                end_line: 20,
                anchor_line: 12,
                anchor_timestamp: logcore::problems::PackedLogTimestamp::UNKNOWN,
                pid: 42,
                process_instance: logcore::problems::ProcessInstanceKey(0),
                kind: logcore::problems::ProblemKind::JavaCrash,
                evidence: logcore::problems::EvidenceFlags::PRIMARY,
                outcome: logcore::problems::OutcomeFlags::NONE,
                boundary: logcore::problems::BoundaryFlags::NONE,
            },
            0,
            0,
            0,
            0,
        )
        .unwrap();

        assert_eq!(problem_export_range(event, false, 50, 1_000), (11, 21));
        assert_eq!(problem_export_range(event, true, 50, 30), (1, 30));
        assert_eq!(problem_export_range(event, true, 5, 30), (6, 26));
    }

    #[test]
    fn dense_problem_revisions_are_rate_limited_but_terminal_states_emit() {
        fn progress(revision: u64) -> ProblemsProgressDto {
            ProblemsProgressDto {
                scanned_lines: revision * 4_096,
                stable_lines: 1_000_000,
                coverage: logcore::problems::InputCoverage::static_file(
                    logcore::problems::RangeCompleteness::Bounded,
                )
                .into(),
                observed_occurrence_count: revision,
                stored_occurrence_count: revision,
                dropped_occurrence_count: 0,
                provisional_occurrence_count: 0,
                stored_group_count: 1,
                ungrouped_dropped_occurrence_count: 0,
                dropped_recent_observation_count: 0,
                correlation_limited: false,
                revision,
                done: false,
                limited: false,
                session_generation: 1,
                analysis_generation: 1,
            }
        }

        let base = Instant::now();
        let mut gate = ProblemProgressGate::default();
        assert!(gate.should_emit_at(&progress(1), base));
        for revision in 2..=24 {
            assert!(
                !gate.should_emit_at(&progress(revision), base + Duration::from_millis(revision))
            );
        }
        assert!(gate.should_emit_at(&progress(25), base + Duration::from_millis(100)));

        let mut terminal = progress(26);
        terminal.done = true;
        assert!(gate.should_emit_at(&terminal, base + Duration::from_millis(101)));

        let mut idle_gate = ProblemProgressGate::default();
        assert!(idle_gate.should_emit_at(&progress(1), base));
        assert!(!idle_gate.should_emit_at(&progress(2), base + Duration::from_millis(1)));
        assert!(idle_gate
            .schedule_deferred(base + Duration::from_millis(1))
            .is_some());
        idle_gate.clear_deferred();
        assert!(idle_gate.should_emit_at(&progress(2), base + Duration::from_millis(101)));
    }
}
