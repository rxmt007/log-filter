use crate::dto::{
    AppConfigDto, DeviceListDto, ExportRequest, ExportSummaryDto, FilterDoneDto, FilterSpecDto,
    MinimapDto, NavigationTargetDto, Row, SearchProgressDto, SearchResult, SearchSpecDto,
    SplitRequest, SplitSummaryDto, StartLogcatRequest, Status, StreamAppendDto, StreamControlDto,
};
use crate::state::AppState;
use crate::state::{StreamRequestState, StreamTask};
use logcore::filter::{FilterMatcher, FilterSpec};
use logcore::search::{SearchMatcher, SearchSpec};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 每步 8MB
const SCAN_CHUNK_LINES: usize = 4096;
const STREAM_READ_BUF: usize = 64 * 1024;
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

fn load_app_config() -> Result<logcore::config::AppConfig, String> {
    let path = logcore::config::default_config_path();
    logcore::config::load_config(&path).map_err(|err| err.to_string())
}

fn resolve_adb_from_config(config: &logcore::config::AppConfig) -> Result<PathBuf, String> {
    logcore::adb::resolve_adb_path(config.adb_path.as_deref())
        .ok_or_else(|| "adb executable was not found".to_string())
}

fn parse_buffers(buffers: &[String]) -> Result<Vec<logcore::adb::LogcatBuffer>, String> {
    if buffers.is_empty() {
        return Ok(vec![logcore::adb::LogcatBuffer::Main]);
    }
    buffers
        .iter()
        .map(|buffer| logcore::adb::LogcatBuffer::try_from(buffer.as_str()))
        .collect()
}

fn stream_session_path(config: &logcore::config::AppConfig) -> Result<PathBuf, String> {
    let base_dir = config
        .storage_dir
        .clone()
        .unwrap_or_else(|| logcore::config::default_config_dir().join("sessions"));
    fs::create_dir_all(&base_dir).map_err(|err| err.to_string())?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_millis();
    Ok(base_dir.join(format!("logcat-{millis}.log")))
}

fn stream_status(state: &AppState) -> StreamControlDto {
    let status = {
        let generation = state.generation.load(Ordering::SeqCst);
        let guard = state.lock_session();
        match guard.as_ref() {
            Some(session) => status_from(session, generation),
            None => empty_status(generation),
        }
    };
    let stream = state.lock_stream();
    StreamControlDto {
        status,
        running: stream.task.is_some(),
        paused: stream.paused,
        device_serial: stream
            .task
            .as_ref()
            .map(|task| task.serial.clone())
            .or_else(|| {
                stream
                    .last_request
                    .as_ref()
                    .and_then(|request| request.requested_serial.clone())
            }),
        session_path: stream
            .last_request
            .as_ref()
            .map(|request| request.session_path.to_string_lossy().to_string()),
    }
}

fn lock_child(child: &Arc<Mutex<std::process::Child>>) -> MutexGuard<'_, std::process::Child> {
    match child.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn take_stream_task(
    state: &AppState,
    paused: bool,
    clear_last_request: bool,
) -> Option<StreamTask> {
    state.next_stream_generation();
    let mut runtime = state.lock_stream();
    runtime.paused = paused;
    if clear_last_request {
        runtime.last_request = None;
    }
    runtime.task.take()
}

fn stop_stream_task(state: &AppState, paused: bool, clear_last_request: bool) {
    let Some(task) = take_stream_task(state, paused, clear_last_request) else {
        return;
    };
    {
        let mut child = lock_child(&task.child);
        let _ = child.kill();
    }
    let _ = task.handle.join();
}

fn spawn_logcat_stream(
    app_state: AppState,
    app: AppHandle,
    request: StreamRequestState,
) -> Result<String, String> {
    let devices = logcore::adb::list_devices(&request.adb_path).map_err(|err| err.to_string())?;
    let device = logcore::adb::select_online_device(&devices, request.requested_serial.as_deref())?;
    let buffers = parse_buffers(&request.buffers)?;
    let command =
        logcore::adb::build_logcat_command(request.adb_path.clone(), &device.serial, &buffers);
    let mut child = Command::new(&command.adb_path)
        .args(&command.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("failed to start adb logcat: {err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture adb stdout".to_string())?;
    let child = Arc::new(Mutex::new(child));
    let stream_generation = app_state.next_stream_generation();
    let serial = device.serial.clone();
    let (start_tx, start_rx) = mpsc::channel();
    let handle = spawn_stream_reader(
        app_state.clone(),
        app,
        request.session_path.clone(),
        request.session_generation,
        stream_generation,
        serial.clone(),
        stdout,
        child.clone(),
        start_rx,
    );

    let mut runtime = app_state.lock_stream();
    runtime.task = Some(StreamTask {
        generation: stream_generation,
        child,
        handle,
        serial: serial.clone(),
    });
    runtime.last_request = Some(request);
    runtime.paused = false;
    let _ = start_tx.send(());
    Ok(serial)
}

fn spawn_stream_reader(
    app_state: AppState,
    app: AppHandle,
    session_path: PathBuf,
    session_generation: u64,
    stream_generation: u64,
    device_serial: String,
    stdout: std::process::ChildStdout,
    child: Arc<Mutex<std::process::Child>>,
    start_rx: mpsc::Receiver<()>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        if start_rx.recv().is_ok() {
            if let Ok(mut writer) = OpenOptions::new().append(true).open(&session_path) {
                let mut reader = BufReader::new(stdout);
                let mut buf = vec![0_u8; STREAM_READ_BUF];

                loop {
                    if app_state.generation.load(Ordering::SeqCst) != session_generation
                        || !app_state.is_current_stream_task(stream_generation)
                    {
                        break;
                    }

                    let read = match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(read) => read,
                        Err(_) => break,
                    };
                    if writer.write_all(&buf[..read]).is_err() || writer.flush().is_err() {
                        break;
                    }

                    let update = {
                        let mut guard = app_state.lock_session();
                        let Some(session) = guard.as_mut() else {
                            break;
                        };
                        let previous_total = session.total_lines();
                        if session.remap_and_index_step(INDEX_BUDGET).is_err() {
                            break;
                        }
                        let total_lines = session.total_lines();
                        let filter_done =
                            append_filter_for_range(session, previous_total, total_lines);
                        let search_progress =
                            append_search_for_range(session, previous_total, total_lines);
                        let status = status_from(session, session_generation);
                        (status, filter_done, search_progress)
                    };

                    let (status, filter_done, search_progress) = update;
                    if let Some(filtered_lines) = filter_done {
                        let _ = app.emit(
                            "filter:done",
                            FilterDoneDto {
                                filtered_lines,
                                generation: session_generation,
                            },
                        );
                    }
                    if let Some(summary) = search_progress {
                        let _ = app.emit(
                            "search:progress",
                            SearchProgressDto {
                                scanned: status.total_lines,
                                matches: summary.count,
                                first_line: summary.first,
                                done: true,
                                generation: session_generation,
                            },
                        );
                    }
                    let _ = app.emit(
                        "stream:append",
                        StreamAppendDto {
                            appended_bytes: read as u64,
                            status,
                            device_serial: device_serial.clone(),
                        },
                    );
                }
            }
        }

        {
            let mut child = lock_child(&child);
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut runtime = app_state.lock_stream();
        if runtime
            .task
            .as_ref()
            .is_some_and(|task| task.generation == stream_generation)
        {
            runtime.task = None;
        }
    })
}

fn append_filter_for_range(
    session: &mut logcore::session::Session,
    start: usize,
    end: usize,
) -> Option<usize> {
    let spec = session.active_filter_spec()?;
    let matcher = FilterMatcher::new(&spec).ok()?;
    let matches = session.filter_indexed_range(&matcher, start, end);
    Some(session.append_filter_results(&spec, matches))
}

fn append_search_for_range(
    session: &mut logcore::session::Session,
    start: usize,
    end: usize,
) -> Option<logcore::search::SearchSummary> {
    let spec = session.active_search_spec()?;
    let matcher = SearchMatcher::new(&spec).ok()?;
    let matches = session.search_indexed_range(&matcher, start, end);
    Some(session.append_search_results(&spec, matches))
}

#[tauri::command]
pub fn open_file(path: String, state: State<AppState>, app: AppHandle) -> Result<Status, String> {
    stop_stream_task(state.inner(), false, true);
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

#[tauri::command]
pub fn list_devices() -> Result<DeviceListDto, String> {
    let config = load_app_config()?;
    let adb_path = resolve_adb_from_config(&config)?;
    let devices = logcore::adb::list_devices(&adb_path).map_err(|err| err.to_string())?;
    Ok(DeviceListDto {
        adb_path: Some(adb_path.to_string_lossy().to_string()),
        devices: devices.into_iter().map(Into::into).collect(),
    })
}

#[tauri::command]
pub fn start_logcat(
    request: StartLogcatRequest,
    state: State<AppState>,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    stop_stream_task(state.inner(), false, true);
    let config = load_app_config()?;
    let adb_path = resolve_adb_from_config(&config)?;
    let buffers = parse_buffers(&request.buffers)?;
    let session_path = stream_session_path(&config)?;
    File::create(&session_path).map_err(|err| err.to_string())?;
    let session = logcore::session::Session::open(&session_path).map_err(|err| err.to_string())?;
    let session_generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.next_filter_task_generation();
    state.next_search_task_generation();
    *state.lock_session() = Some(session);

    let request_state = StreamRequestState {
        adb_path,
        requested_serial: request
            .device_serial
            .filter(|serial| !serial.trim().is_empty()),
        buffers: buffers
            .iter()
            .map(|buffer| buffer.as_arg().to_string())
            .collect(),
        session_path,
        session_generation,
    };
    spawn_logcat_stream(state.inner().clone(), app, request_state)?;
    Ok(stream_status(state.inner()))
}

#[tauri::command]
pub fn pause_logcat(state: State<AppState>) -> StreamControlDto {
    stop_stream_task(state.inner(), true, false);
    stream_status(state.inner())
}

#[tauri::command]
pub fn resume_logcat(state: State<AppState>, app: AppHandle) -> Result<StreamControlDto, String> {
    let request = {
        let runtime = state.lock_stream();
        runtime
            .last_request
            .clone()
            .ok_or_else(|| "no paused logcat session to resume".to_string())?
    };
    stop_stream_task(state.inner(), true, false);
    spawn_logcat_stream(state.inner().clone(), app, request)?;
    Ok(stream_status(state.inner()))
}

#[tauri::command]
pub fn stop_logcat(state: State<AppState>) -> StreamControlDto {
    stop_stream_task(state.inner(), false, false);
    stream_status(state.inner())
}

#[tauri::command]
pub fn clear_logcat(state: State<AppState>) -> Result<StreamControlDto, String> {
    let session_path = {
        let runtime = state.lock_stream();
        runtime
            .last_request
            .as_ref()
            .map(|request| request.session_path.clone())
    };
    stop_stream_task(state.inner(), false, false);
    if let Some(path) = session_path {
        File::create(&path).map_err(|err| err.to_string())?;
        let session = logcore::session::Session::open(&path).map_err(|err| err.to_string())?;
        let session_generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        state.next_filter_task_generation();
        state.next_search_task_generation();
        *state.lock_session() = Some(session);
        let mut runtime = state.lock_stream();
        if let Some(request) = runtime.last_request.as_mut() {
            request.session_generation = session_generation;
        }
    }
    Ok(stream_status(state.inner()))
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
                Some(session) => {
                    let mut count = session.apply_filter_results(&spec, matches);
                    let current_total = session.total_lines();
                    if current_total > total_lines {
                        let extra =
                            session.filter_indexed_range(&matcher, total_lines, current_total);
                        count = session.append_filter_results(&spec, extra);
                    }
                    count
                }
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
                Some(session) => {
                    let mut summary = session.apply_search_results(&spec, matches);
                    let current_total = session.total_lines();
                    if current_total > total_lines {
                        let extra =
                            session.search_indexed_range(&matcher, total_lines, current_total);
                        summary = session.append_search_results(&spec, extra);
                    }
                    summary
                }
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

    #[test]
    fn parses_logcat_buffers_and_rejects_unknown_values() {
        let buffers = parse_buffers(&["main".to_string(), "crash".to_string()]).unwrap();
        assert_eq!(
            buffers,
            vec![
                logcore::adb::LogcatBuffer::Main,
                logcore::adb::LogcatBuffer::Crash
            ]
        );
        assert_eq!(
            parse_buffers(&[]).unwrap(),
            vec![logcore::adb::LogcatBuffer::Main]
        );
        assert!(parse_buffers(&["kernel".to_string()]).is_err());
    }
}
