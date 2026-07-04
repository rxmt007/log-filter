use crate::dto::{FilterSpecDto, Row, SearchResult, SearchSpecDto, Status};
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
        indexed_bytes: 0,
        total_bytes: 0,
        indexing: false,
        generation,
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
    let view = match view.as_str() {
        "all" => logcore::session::RowsView::All,
        "filtered" => logcore::session::RowsView::Filtered,
        _ => return Vec::new(),
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
                marked: false,
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
