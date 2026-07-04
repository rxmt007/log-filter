use crate::dto::{Row, Status};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 每步 8MB
const MAX_ROWS: usize = 512;

fn status_from(session: &logcore::session::Session) -> Status {
    Status {
        total_lines: session.total_lines(),
        indexed_bytes: session.indexed_bytes() as u64,
        total_bytes: session.total_bytes() as u64,
        indexing: !session.is_indexing_done(),
    }
}

#[tauri::command]
pub fn open_file(path: String, state: State<AppState>, app: AppHandle) -> Result<Status, String> {
    let session =
        logcore::session::Session::open(&PathBuf::from(&path)).map_err(|e| e.to_string())?;
    let status = status_from(&session);
    // 递增代号:上一个文件遗留的索引线程会在下一次循环检测到并自退。
    let my_gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    *state.session.lock().unwrap() = Some(session);

    // 后台索引:小预算步进,步间释放锁,保证浏览不被阻塞。
    let session_arc = state.session.clone();
    let gen_arc = state.generation.clone();
    std::thread::spawn(move || loop {
        if gen_arc.load(Ordering::SeqCst) != my_gen {
            break; // 已被更晚的 open 取代
        }
        let snapshot = {
            let mut guard = session_arc.lock().unwrap();
            match guard.as_mut() {
                Some(s) => {
                    let done = s.index_step(INDEX_BUDGET);
                    Some((status_from(s), done))
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
    let guard = state.session.lock().unwrap();
    match guard.as_ref() {
        Some(s) => status_from(s),
        None => Status {
            total_lines: 0,
            indexed_bytes: 0,
            total_bytes: 0,
            indexing: false,
        },
    }
}

#[tauri::command]
pub fn get_rows(view: String, start: usize, count: usize, state: State<AppState>) -> Vec<Row> {
    // M1 只支持 all 视图;filtered 属于 M2。
    if view != "all" {
        return Vec::new();
    }
    let count = count.min(MAX_ROWS);
    let guard = state.session.lock().unwrap();
    match guard.as_ref() {
        Some(s) => s
            .get_rows(start, count)
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
