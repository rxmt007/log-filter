use crate::dto::{
    AppConfigDto, ExportRequest, ExportSummaryDto, FilterDoneDto, FilterSpecDto, MinimapDto,
    NavigationTargetDto, Row, SearchProgressDto, SearchResult, SearchSpecDto, SplitRequest,
    SplitSummaryDto, Status,
};
use crate::state::AppState;
use logcore::filter::{FilterMatcher, FilterSpec};
use logcore::search::{SearchMatcher, SearchSpec};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 每步 8MB
const SCAN_CHUNK_LINES: usize = 4096;
const MAX_ROWS: usize = 512;

fn status_from(session: &logcore::session::Session, generation: u64) -> Status {
    Status {
        total_lines: session.total_lines(),
        filtered_lines: session.filtered_count(),
        bookmark_lines: session.bookmark_count(),
        error_lines: session.error_count(),
        indexed_bytes: session.indexed_bytes() as u64,
        total_bytes: session.total_bytes() as u64,
        indexing: !session.is_indexing_done(),
        generation,
    }
}

fn empty_status(generation: u64) -> Status {
    Status {
        total_lines: 0,
        filtered_lines: 0,
        bookmark_lines: 0,
        error_lines: 0,
        indexed_bytes: 0,
        total_bytes: 0,
        indexing: false,
        generation,
    }
}

fn rows_view_from_str(view: &str) -> Option<logcore::session::RowsView> {
    match view {
        "all" => Some(logcore::session::RowsView::All),
        "filtered" => Some(logcore::session::RowsView::Filtered),
        "bookmarks" => Some(logcore::session::RowsView::Bookmarks),
        "errors" => Some(logcore::session::RowsView::Errors),
        _ => None,
    }
}

fn clamp_row_count(count: usize) -> usize {
    count.min(MAX_ROWS)
}

#[tauri::command]
pub fn open_file(path: String, state: State<AppState>, app: AppHandle) -> Result<Status, String> {
    let session =
        logcore::session::Session::open(&PathBuf::from(&path)).map_err(|e| e.to_string())?;
    // 递增代号:上一个文件遗留的索引线程会在下一次循环检测到并自退。
    let my_gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.next_filter_task_generation();
    state.next_search_task_generation();
    let status = status_from(&session, my_gen);
    *state.lock_session() = Some(session);

    // 后台索引:小预算步进,步间释放锁,保证浏览不被阻塞。
    let app_state = state.inner().clone();
    let gen_arc = state.generation.clone();
    std::thread::spawn(move || loop {
        if gen_arc.load(Ordering::SeqCst) != my_gen {
            break; // 已被更晚的 open 取代
        }
        let snapshot = {
            let mut guard = app_state.lock_session();
            match guard.as_mut() {
                Some(s) => {
                    let done = s.index_step(INDEX_BUDGET);
                    Some((status_from(s, my_gen), done))
                }
                None => None, // 会话被清空,退出
            }
        };
        match snapshot {
            Some((st, done)) => {
                let _ = app.emit("index:progress", st);
                if done {
                    rerun_scans_after_index_done(&app_state, &app, my_gen);
                    break;
                }
            }
            None => break,
        }
        std::thread::yield_now(); // 让出,减少与 get_rows 的锁争用
    });

    Ok(status)
}

fn rerun_scans_after_index_done(app_state: &AppState, app: &AppHandle, session_generation: u64) {
    let (filter_spec, search_spec) = {
        let guard = app_state.lock_session();
        match guard.as_ref() {
            Some(session) => (session.active_filter_spec(), session.active_search_spec()),
            None => (None, None),
        }
    };
    if let Some(spec) = filter_spec {
        let task_generation = app_state.next_filter_task_generation();
        spawn_filter_task(
            app_state.clone(),
            app.clone(),
            spec,
            session_generation,
            task_generation,
        );
    }
    if let Some(spec) = search_spec {
        let task_generation = app_state.next_search_task_generation();
        spawn_search_task(
            app_state.clone(),
            app.clone(),
            spec,
            session_generation,
            task_generation,
        );
    }
}

#[tauri::command]
pub fn get_status(state: State<AppState>) -> Status {
    let generation = state.generation.load(Ordering::SeqCst);
    let guard = state.lock_session();
    match guard.as_ref() {
        Some(s) => status_from(s, generation),
        None => empty_status(generation),
    }
}

#[tauri::command]
pub fn get_rows(view: String, start: usize, count: usize, state: State<AppState>) -> Vec<Row> {
    let Some(view) = rows_view_from_str(&view) else {
        return Vec::new();
    };
    let count = clamp_row_count(count);
    let guard = state.lock_session();
    match guard.as_ref() {
        Some(s) => s
            .get_rows_for_view(view, start, count)
            .into_iter()
            .map(|(line_no, e)| Row {
                line_no,
                date: e.date,
                time: e.time,
                level: e.level,
                pid: e.pid,
                tid: e.tid,
                tag: e.tag,
                message: e.message,
                marked: s.is_bookmarked(line_no),
            })
            .collect(),
        None => Vec::new(),
    }
}

#[tauri::command]
pub fn set_filter(
    filter: FilterSpecDto,
    state: State<AppState>,
    app: AppHandle,
) -> Result<usize, String> {
    let spec: FilterSpec = filter.into();
    let task_generation = state.next_filter_task_generation();
    let session_generation = state.generation.load(Ordering::SeqCst);
    let (current_count, immediate) = {
        let mut guard = state.lock_session();
        let Some(session) = guard.as_mut() else {
            return Ok(0);
        };
        let immediate = session
            .set_filter_pending(&spec)
            .map_err(|err| err.message)?;
        (session.filtered_count(), immediate)
    };

    if let Some(filtered_lines) = immediate {
        let _ = app.emit(
            "filter:done",
            FilterDoneDto {
                filtered_lines,
                generation: session_generation,
            },
        );
        return Ok(filtered_lines);
    }

    spawn_filter_task(
        state.inner().clone(),
        app,
        spec,
        session_generation,
        task_generation,
    );
    Ok(current_count)
}

#[tauri::command]
pub fn get_filtered_count(state: State<AppState>) -> usize {
    let guard = state.lock_session();
    guard.as_ref().map_or(0, |session| session.filtered_count())
}

#[tauri::command]
pub fn search(
    spec: SearchSpecDto,
    state: State<AppState>,
    app: AppHandle,
) -> Result<SearchResult, String> {
    let spec: SearchSpec = spec.into();
    let task_generation = state.next_search_task_generation();
    let session_generation = state.generation.load(Ordering::SeqCst);
    let active = {
        let mut guard = state.lock_session();
        let Some(session) = guard.as_mut() else {
            return Ok(SearchResult {
                count: 0,
                first_line: None,
            });
        };
        session
            .set_search_pending(&spec)
            .map_err(|err| err.message)?
    };

    if !active {
        let _ = app.emit(
            "search:progress",
            SearchProgressDto {
                scanned: 0,
                matches: 0,
                first_line: None,
                done: true,
                generation: session_generation,
            },
        );
        return Ok(SearchResult {
            count: 0,
            first_line: None,
        });
    }

    spawn_search_task(
        state.inner().clone(),
        app,
        spec,
        session_generation,
        task_generation,
    );
    Ok(SearchResult {
        count: 0,
        first_line: None,
    })
}

fn spawn_filter_task(
    app_state: AppState,
    app: AppHandle,
    spec: FilterSpec,
    session_generation: u64,
    task_generation: u64,
) {
    let Ok(matcher) = FilterMatcher::new(&spec) else {
        return;
    };
    std::thread::spawn(move || {
        let total_lines = {
            let guard = app_state.lock_session();
            match guard.as_ref() {
                Some(session) => session.total_lines(),
                None => return,
            }
        };
        let mut matches = Vec::new();
        let mut start = 0;
        while start < total_lines {
            if app_state.generation.load(Ordering::SeqCst) != session_generation
                || !app_state.is_current_filter_task(task_generation)
            {
                return;
            }
            let end = start.saturating_add(SCAN_CHUNK_LINES).min(total_lines);
            let chunk = {
                let guard = app_state.lock_session();
                match guard.as_ref() {
                    Some(session) => session.filter_indexed_range(&matcher, start, end),
                    None => return,
                }
            };
            matches.extend(chunk);
            start = end;
            std::thread::yield_now();
        }

        if app_state.generation.load(Ordering::SeqCst) != session_generation
            || !app_state.is_current_filter_task(task_generation)
        {
            return;
        }
        let filtered_lines = {
            let mut guard = app_state.lock_session();
            match guard.as_mut() {
                Some(session) => session.apply_filter_results(&spec, matches),
                None => return,
            }
        };
        let _ = app.emit(
            "filter:done",
            FilterDoneDto {
                filtered_lines,
                generation: session_generation,
            },
        );
    });
}

fn spawn_search_task(
    app_state: AppState,
    app: AppHandle,
    spec: SearchSpec,
    session_generation: u64,
    task_generation: u64,
) {
    let Ok(matcher) = SearchMatcher::new(&spec) else {
        return;
    };
    std::thread::spawn(move || {
        let total_lines = {
            let guard = app_state.lock_session();
            match guard.as_ref() {
                Some(session) => session.total_lines(),
                None => return,
            }
        };
        let mut matches = Vec::new();
        let mut first_line = None;
        let mut start = 0;
        while start < total_lines {
            if app_state.generation.load(Ordering::SeqCst) != session_generation
                || !app_state.is_current_search_task(task_generation)
            {
                return;
            }
            let end = start.saturating_add(SCAN_CHUNK_LINES).min(total_lines);
            let chunk = {
                let guard = app_state.lock_session();
                match guard.as_ref() {
                    Some(session) => session.search_indexed_range(&matcher, start, end),
                    None => return,
                }
            };
            if first_line.is_none() {
                first_line = chunk.first().map(|idx| idx + 1);
            }
            matches.extend(chunk);
            let _ = app.emit(
                "search:progress",
                SearchProgressDto {
                    scanned: end,
                    matches: matches.len(),
                    first_line,
                    done: false,
                    generation: session_generation,
                },
            );
            start = end;
            std::thread::yield_now();
        }

        if app_state.generation.load(Ordering::SeqCst) != session_generation
            || !app_state.is_current_search_task(task_generation)
        {
            return;
        }
        let summary = {
            let mut guard = app_state.lock_session();
            match guard.as_mut() {
                Some(session) => session.apply_search_results(&spec, matches),
                None => return,
            }
        };
        let _ = app.emit(
            "search:progress",
            SearchProgressDto {
                scanned: total_lines,
                matches: summary.count,
                first_line: summary.first,
                done: true,
                generation: session_generation,
            },
        );
    });
}

#[tauri::command]
pub fn search_next(from_line_no: u64, direction: String, state: State<AppState>) -> Option<u64> {
    let direction = match direction.as_str() {
        "previous" => logcore::search::SearchDirection::Previous,
        _ => logcore::search::SearchDirection::Next,
    };
    let guard = state.lock_session();
    guard
        .as_ref()
        .and_then(|session| session.search_next(from_line_no, direction))
}

#[tauri::command]
pub fn toggle_bookmark(line_no: u64, state: State<AppState>) -> Result<bool, String> {
    let mut guard = state.lock_session();
    let Some(session) = guard.as_mut() else {
        return Ok(false);
    };
    session
        .toggle_bookmark(line_no)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn list_bookmarks(state: State<AppState>) -> Vec<u64> {
    let guard = state.lock_session();
    guard
        .as_ref()
        .map_or_else(Vec::new, |session| session.list_bookmarks())
}

#[tauri::command]
pub fn next_bookmark(
    from_line_no: u64,
    direction: String,
    state: State<AppState>,
) -> Option<NavigationTargetDto> {
    let direction = match direction.as_str() {
        "previous" => logcore::bookmarks::BookmarkDirection::Previous,
        _ => logcore::bookmarks::BookmarkDirection::Next,
    };
    let guard = state.lock_session();
    guard
        .as_ref()
        .and_then(|session| session.next_bookmark_in_current_result(from_line_no, direction))
        .map(Into::into)
}

#[tauri::command]
pub fn get_minimap(buckets: usize, state: State<AppState>) -> MinimapDto {
    let guard = state.lock_session();
    let Some(session) = guard.as_ref() else {
        return MinimapDto {
            bookmarks: Vec::new(),
            errors: Vec::new(),
        };
    };
    let minimap = session.minimap(buckets);
    MinimapDto {
        bookmarks: minimap.bookmarks,
        errors: minimap.errors,
    }
}

#[tauri::command]
pub fn export_logs(
    request: ExportRequest,
    state: State<AppState>,
) -> Result<ExportSummaryDto, String> {
    if request.path.trim().is_empty() {
        return Err("export path is required".to_string());
    }
    let output = PathBuf::from(&request.path);
    let mut guard = state.lock_session();
    let Some(session) = guard.as_mut() else {
        return Err("open a log file before exporting".to_string());
    };

    let summary = if request.mode == "range" {
        let start = request
            .start_line
            .ok_or_else(|| "range start line is required".to_string())?;
        let end = request
            .end_line
            .ok_or_else(|| "range end line is required".to_string())?;
        session.export_range(start, end, &output)
    } else {
        let view = request.view.as_deref().unwrap_or("all");
        let view =
            rows_view_from_str(view).ok_or_else(|| format!("unknown export view: {view}"))?;
        session.export_view(view, &output)
    }
    .map_err(|err| err.to_string())?;

    Ok(summary.into())
}

#[tauri::command]
pub fn split_log_file(request: SplitRequest) -> Result<SplitSummaryDto, String> {
    if request.path.trim().is_empty() {
        return Err("source path is required".to_string());
    }
    if request.out_dir.trim().is_empty() {
        return Err("output directory is required".to_string());
    }
    let mode = match request.mode.as_str() {
        "bytes" => logcore::split::SplitMode::Bytes(request.value),
        "lines" => logcore::split::SplitMode::Lines(request.value),
        other => return Err(format!("unknown split mode: {other}")),
    };
    let summary = logcore::split::split_file(
        &PathBuf::from(request.path),
        &PathBuf::from(request.out_dir),
        mode,
    )
    .map_err(|err| err.to_string())?;
    Ok(summary.into())
}

#[tauri::command]
pub fn get_config() -> Result<AppConfigDto, String> {
    let path = logcore::config::default_config_path();
    let config = logcore::config::load_config(&path).map_err(|err| err.to_string())?;
    Ok(AppConfigDto::from_config(config, path))
}

#[tauri::command]
pub fn set_config(config: AppConfigDto) -> Result<AppConfigDto, String> {
    let path = logcore::config::default_config_path();
    let config = logcore::config::AppConfig::try_from(config)?;
    logcore::config::save_config(&path, &config).map_err(|err| err.to_string())?;
    Ok(AppConfigDto::from_config(config, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_rows_views_and_rejects_unknown() {
        assert_eq!(
            rows_view_from_str("all"),
            Some(logcore::session::RowsView::All)
        );
        assert_eq!(
            rows_view_from_str("filtered"),
            Some(logcore::session::RowsView::Filtered)
        );
        assert_eq!(
            rows_view_from_str("bookmarks"),
            Some(logcore::session::RowsView::Bookmarks)
        );
        assert_eq!(
            rows_view_from_str("errors"),
            Some(logcore::session::RowsView::Errors)
        );
        assert_eq!(rows_view_from_str("everything"), None);
    }

    #[test]
    fn clamps_visible_row_requests_to_ipc_limit() {
        assert_eq!(clamp_row_count(32), 32);
        assert_eq!(clamp_row_count(MAX_ROWS + 1), MAX_ROWS);
    }
}
