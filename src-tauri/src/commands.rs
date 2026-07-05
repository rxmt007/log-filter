use crate::dto::{
    AppConfigDto, ExportRequest, ExportSummaryDto, FilterSpecDto, MinimapDto, NavigationTargetDto,
    Row, SearchResult, SearchSpecDto, SplitRequest, SplitSummaryDto, Status,
};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 每步 8MB
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

#[tauri::command]
pub fn open_file(path: String, state: State<AppState>, app: AppHandle) -> Result<Status, String> {
    let session =
        logcore::session::Session::open(&PathBuf::from(&path)).map_err(|e| e.to_string())?;
    // 递增代号:上一个文件遗留的索引线程会在下一次循环检测到并自退。
    let my_gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
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
                    break;
                }
            }
            None => break,
        }
        std::thread::yield_now(); // 让出,减少与 get_rows 的锁争用
    });

    Ok(status)
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
    let count = count.min(MAX_ROWS);
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
pub fn set_filter(filter: FilterSpecDto, state: State<AppState>) -> Result<usize, String> {
    let mut guard = state.lock_session();
    let Some(session) = guard.as_mut() else {
        return Ok(0);
    };
    session
        .set_filter(&filter.into())
        .map_err(|err| err.message)
}

#[tauri::command]
pub fn get_filtered_count(state: State<AppState>) -> usize {
    let guard = state.lock_session();
    guard.as_ref().map_or(0, |session| session.filtered_count())
}

#[tauri::command]
pub fn search(spec: SearchSpecDto, state: State<AppState>) -> Result<SearchResult, String> {
    let mut guard = state.lock_session();
    let Some(session) = guard.as_mut() else {
        return Ok(SearchResult {
            count: 0,
            first_line: None,
        });
    };
    let summary = session.search(&spec.into()).map_err(|err| err.message)?;
    Ok(SearchResult {
        count: summary.count,
        first_line: summary.first,
    })
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
