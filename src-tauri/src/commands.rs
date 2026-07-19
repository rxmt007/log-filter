use crate::dto::{
    AppConfigDto, DeviceListDto, ExportProgressDto, ExportRequest, ExportSummaryDto, FilterDoneDto,
    FilterSpecDto, MinimapDto, NavigationTargetDto, Row, SearchProgressDto, SearchResult,
    SearchSpecDto, SplitProgressDto, SplitRequest, SplitSummaryDto, StartLogcatRequest, Status,
    StreamAppendDto, StreamControlDto,
};
use crate::state::AppState;
use crate::state::{StreamRequestState, StreamTask};
use logcore::filter::{FilterMatcher, FilterSpec};
use logcore::search::{SearchMatcher, SearchSpec};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 8 * 1024 * 1024; // 每步 8MB
const SCAN_CHUNK_LINES: usize = 4096;
const SEARCH_PROGRESS_STRIDE: usize = 65_536; // 搜索进度事件节流阈值(约 16 个扫描块)
const EXPORT_CHUNK_LINES: usize = 4096;
const EXPORT_PROGRESS_STRIDE: usize = 65_536; // 与 SEARCH_PROGRESS_STRIDE 同数量级
const STREAM_READ_BUF: usize = 64 * 1024;
const MAX_ROWS: usize = 512;
const STREAM_SESSION_KEEP: usize = 10; // 流式会话文件最多保留的份数

struct StreamReaderArgs {
    app_state: AppState,
    app: AppHandle,
    session_path: PathBuf,
    session_generation: u64,
    stream_generation: u64,
    device_serial: String,
    stdout: std::process::ChildStdout,
    child: Arc<Mutex<std::process::Child>>,
    start_rx: mpsc::Receiver<()>,
}

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

fn config_encoding(config: &logcore::config::AppConfig) -> logcore::encoding::TextEncoding {
    logcore::encoding::TextEncoding::from_config(&config.encoding)
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

fn parse_logcat_request_buffers(
    config: &logcore::config::AppConfig,
    request: &StartLogcatRequest,
) -> Result<Vec<logcore::adb::LogcatBuffer>, String> {
    if let Some(command) = request
        .command
        .as_deref()
        .filter(|command| !command.trim().is_empty())
    {
        let spec = logcore::adb::LogcatSpec::parse(command)?;
        return Ok(vec![spec.buffer]);
    }
    if !request.buffers.is_empty() {
        return parse_buffers(&request.buffers);
    }
    let spec = logcore::adb::LogcatSpec::parse(&config.current_command)?;
    Ok(vec![spec.buffer])
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

/// 只识别本应用生成的 `logcat-<millis>.log`,按文件名倒序保留最新 keep 个,
/// 其余连同书签 sidecar 一起删除;所有 IO 失败静默忽略(清理是尽力而为)。
fn prune_stream_sessions(dir: &std::path::Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut sessions: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("logcat-"))
                .and_then(|rest| rest.strip_suffix(".log"))
                .is_some_and(|millis| !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    sessions.sort();
    for stale in sessions.iter().rev().skip(keep) {
        let _ = fs::remove_file(stale);
        let _ = fs::remove_file(logcore::bookmarks::sidecar_path_for(stale));
    }
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

/// 停止当前流式任务的语义模式,决定 `paused` 标记与 `last_request` 的去留。
#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamStop {
    /// 挂起:保留 last_request,标记 paused,可 resume。
    Pause,
    /// 停止:保留 last_request(供 clear 复用路径),不标记 paused。
    Stop,
    /// 遗弃:丢弃 last_request(切换到新会话前)。
    Forget,
}

impl StreamStop {
    fn paused(self) -> bool {
        matches!(self, Self::Pause)
    }

    fn clears_last_request(self) -> bool {
        matches!(self, Self::Forget)
    }
}

fn take_stream_task(state: &AppState, mode: StreamStop) -> Option<StreamTask> {
    state.next_stream_generation();
    let mut runtime = state.lock_stream();
    runtime.paused = mode.paused();
    if mode.clears_last_request() {
        runtime.last_request = None;
    }
    runtime.task.take()
}

fn stop_stream_task(state: &AppState, mode: StreamStop) {
    let Some(task) = take_stream_task(state, mode) else {
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
    let command = logcore::adb::build_logcat_command(
        request.adb_path.clone(),
        &device.serial,
        &buffers,
        request.since_timestamp.as_deref(),
    );
    let mut child = logcore::adb::adb_command(&command.adb_path)
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
    let handle = spawn_stream_reader(StreamReaderArgs {
        app_state: app_state.clone(),
        app,
        session_path: request.session_path.clone(),
        session_generation: request.session_generation,
        stream_generation,
        device_serial: serial.clone(),
        stdout,
        child: child.clone(),
        start_rx,
    });

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

fn spawn_stream_reader(args: StreamReaderArgs) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let StreamReaderArgs {
            app_state,
            app,
            session_path,
            session_generation,
            stream_generation,
            device_serial,
            stdout,
            child,
            start_rx,
        } = args;
        if start_rx.recv().is_ok() {
            if let Ok(mut writer) = OpenOptions::new().append(true).open(&session_path) {
                let mut reader = BufReader::new(stdout);
                let mut buf = vec![0_u8; STREAM_READ_BUF];

                loop {
                    let read = match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(read) => read,
                        Err(_) => break,
                    };
                    if writer.write_all(&buf[..read]).is_err() || writer.flush().is_err() {
                        break;
                    }

                    let update = {
                        let Some(mut guard) = app_state.lock_session_if_current(session_generation)
                        else {
                            break;
                        };
                        if !app_state.is_current_stream_task(stream_generation) {
                            break;
                        }
                        let Some(session) = guard.as_mut() else {
                            break;
                        };
                        let previous_total = session.total_lines();
                        let Ok(outcome) = session.remap_and_index_step(INDEX_BUDGET) else {
                            break;
                        };
                        let total_lines = session.total_lines();
                        // 外部截断触发重建后,派生命中数组已清空,须从 0 起做一次完整重扫;
                        // 否则沿用增量的 previous_total(截断后它反而大于新总行数,会漏扫)。
                        let scan_start = if outcome.reset { 0 } else { previous_total };
                        let _filtered_count =
                            append_filter_for_range(session, scan_start, total_lines);
                        let search_progress =
                            append_search_for_range(session, scan_start, total_lines);
                        let status = status_from(session, session_generation);
                        (status, search_progress)
                    };

                    let (status, search_progress) = update;
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
    stop_stream_task(state.inner(), StreamStop::Forget);
    let config = load_app_config()?;
    let session = logcore::session::Session::open_with_encoding(
        &PathBuf::from(&path),
        config_encoding(&config),
    )
    .map_err(|e| e.to_string())?;
    // 递增代号:上一个文件遗留的索引线程会在下一次循环检测到并自退。
    let my_gen = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.next_filter_task_generation();
    state.next_search_task_generation();
    let status = status_from(&session, my_gen);
    *state.lock_session() = Some(session);

    // 后台索引:小预算步进,步间释放锁,保证浏览不被阻塞。
    let app_state = state.inner().clone();
    std::thread::spawn(move || loop {
        let snapshot = {
            let Some(mut guard) = app_state.lock_session_if_current(my_gen) else {
                break; // 已被更晚的 open 取代
            };
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

// adb 挂起时 `list_devices` 可能阻塞数秒(引擎侧 5s 超时),放到阻塞线程池避免冻结命令窗口。
#[tauri::command]
pub async fn list_devices() -> Result<DeviceListDto, String> {
    tauri::async_runtime::spawn_blocking(list_devices_blocking)
        .await
        .map_err(|err| err.to_string())?
}

fn list_devices_blocking() -> Result<DeviceListDto, String> {
    let config = load_app_config()?;
    let adb_path = resolve_adb_from_config(&config)?;
    let devices = logcore::adb::list_devices(&adb_path).map_err(|err| err.to_string())?;
    Ok(DeviceListDto {
        adb_path: Some(adb_path.to_string_lossy().to_string()),
        devices: devices.into_iter().map(Into::into).collect(),
    })
}

// 启动流会 join 上一个 reader 线程(stop_stream_task)且要拉起 adb 子进程,放到阻塞线程池。
#[tauri::command]
pub async fn start_logcat(
    request: StartLogcatRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || start_logcat_blocking(request, &app_state, app))
        .await
        .map_err(|err| err.to_string())?
}

fn start_logcat_blocking(
    request: StartLogcatRequest,
    state: &AppState,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    stop_stream_task(state, StreamStop::Forget);
    let config = load_app_config()?;
    let adb_path = resolve_adb_from_config(&config)?;
    let buffers = parse_logcat_request_buffers(&config, &request)?;
    let session_path = stream_session_path(&config)?;
    File::create(&session_path).map_err(|err| err.to_string())?;
    prune_stream_sessions(
        session_path.parent().unwrap_or(&session_path),
        STREAM_SESSION_KEEP,
    );
    let session =
        logcore::session::Session::open_with_encoding(&session_path, config_encoding(&config))
            .map_err(|err| err.to_string())?;
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
        since_timestamp: None,
    };
    spawn_logcat_stream(state.clone(), app, request_state)?;
    Ok(stream_status(state))
}

#[tauri::command]
pub fn pause_logcat(state: State<AppState>) -> StreamControlDto {
    stop_stream_task(state.inner(), StreamStop::Pause);
    stream_status(state.inner())
}

// 恢复流会 join 上一个 reader 线程并重新拉起 adb,放到阻塞线程池。
#[tauri::command]
pub async fn resume_logcat(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || resume_logcat_blocking(&app_state, app))
        .await
        .map_err(|err| err.to_string())?
}

fn resume_logcat_blocking(state: &AppState, app: AppHandle) -> Result<StreamControlDto, String> {
    let request = {
        let runtime = state.lock_stream();
        runtime
            .last_request
            .clone()
            .ok_or_else(|| "no paused logcat session to resume".to_string())?
    };
    stop_stream_task(state, StreamStop::Pause);
    // 续抓时用最后一条日志时间戳做 `logcat -T`,避免 ring buffer 重放造成重复;
    // 尾部无可解析时间戳时 since_timestamp 保持 None,退化为全量重放。
    let mut request = request;
    request.since_timestamp = read_session_tail(&request.session_path, 64 * 1024)
        .as_deref()
        .and_then(logcore::adb::last_log_timestamp);
    spawn_logcat_stream(state.clone(), app, request)?;
    Ok(stream_status(state))
}

/// 读会话文件末尾至多 max_bytes 的内容(lossy 解码),供 resume 提取最后时间戳。
fn read_session_tail(path: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max_bytes))).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[tauri::command]
pub fn stop_logcat(state: State<AppState>) -> StreamControlDto {
    stop_stream_task(state.inner(), StreamStop::Stop);
    stream_status(state.inner())
}

/// 重建流式会话文件。顺序不可变:必须先 drop 旧 Session(释放 mmap),再截断文件——
/// Windows 上截断带活动映射的文件报 ERROR_USER_MAPPED_FILE;Unix 上并发读旧 mmap 会 SIGBUS。
fn reset_stream_session_file(
    state: &AppState,
    path: &std::path::Path,
    encoding: logcore::encoding::TextEncoding,
) -> Result<u64, String> {
    *state.lock_session() = None;
    File::create(path).map_err(|err| err.to_string())?;
    let _ = fs::remove_file(logcore::bookmarks::sidecar_path_for(path));
    let session = logcore::session::Session::open_with_encoding(path, encoding)
        .map_err(|err| err.to_string())?;
    let session_generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.next_filter_task_generation();
    state.next_search_task_generation();
    *state.lock_session() = Some(session);
    Ok(session_generation)
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
    stop_stream_task(state.inner(), StreamStop::Stop);
    if let Some(path) = session_path {
        let config = load_app_config()?;
        let session_generation =
            reset_stream_session_file(state.inner(), &path, config_encoding(&config))?;
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

/// 分块扫描 [0, 快照总行数);每块持锁校验会话代号与 `is_current_task`,任一失效即放弃(返回 None)。
/// `scan` 在持锁状态下执行(返回本块命中的行号);`on_chunk(scanned, matches_len)` 在锁外执行,
/// 供进度事件使用。收尾 apply 段由各调用方在本函数返回后自行完成(仍需重新持锁校验)。
fn run_chunked_scan(
    app_state: &AppState,
    session_generation: u64,
    is_current_task: impl Fn() -> bool,
    scan: impl Fn(&logcore::session::Session, usize, usize) -> Vec<u32>,
    mut on_chunk: impl FnMut(usize, usize),
) -> Option<Vec<u32>> {
    let total_lines = {
        let guard = app_state.lock_session_if_current(session_generation)?;
        guard.as_ref().map(|session| session.total_lines())?
    };
    let mut matches = Vec::new();
    let mut start = 0;
    while start < total_lines {
        let end = start.saturating_add(SCAN_CHUNK_LINES).min(total_lines);
        let chunk = {
            let guard = app_state.lock_session_if_current(session_generation)?;
            if !is_current_task() {
                return None;
            }
            scan(guard.as_ref()?, start, end)
        };
        matches.extend(chunk);
        on_chunk(end, matches.len());
        start = end;
        std::thread::yield_now();
    }
    Some(matches)
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
        let mut total_lines = 0;
        let Some(matches) = run_chunked_scan(
            &app_state,
            session_generation,
            || app_state.is_current_filter_task(task_generation),
            |session, start, end| session.filter_indexed_range(&matcher, start, end),
            |scanned, _matches_len| total_lines = scanned,
        ) else {
            return;
        };

        let filtered_lines = {
            let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
                return;
            };
            if !app_state.is_current_filter_task(task_generation) {
                return;
            }
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
        // matches 随 chunk 前向扫描升序累积,故一旦非空,`matches.first()` 即最终首命中,可提前上报。
        let first_line = std::cell::Cell::new(None);
        // 节流 search:progress(done=false):约每 16 块(65_536 行)或首命中出现时才发一次,
        // 避免 1 亿行日志产生数万个 IPC 事件;最终 done=true 事件不受影响。
        let mut last_emitted = 0_usize;
        let mut surfaced_first_match = false;
        let mut total_lines = 0;
        let Some(matches) = run_chunked_scan(
            &app_state,
            session_generation,
            || app_state.is_current_search_task(task_generation),
            |session, start, end| {
                let chunk = session.search_indexed_range(&matcher, start, end);
                if first_line.get().is_none() {
                    first_line.set(chunk.first().map(|idx| u64::from(*idx) + 1));
                }
                chunk
            },
            |scanned, matches_len| {
                total_lines = scanned;
                let first_match_now = matches_len > 0 && !surfaced_first_match;
                if scanned - last_emitted >= SEARCH_PROGRESS_STRIDE || first_match_now {
                    surfaced_first_match |= matches_len > 0;
                    last_emitted = scanned;
                    let _ = app.emit(
                        "search:progress",
                        SearchProgressDto {
                            scanned,
                            matches: matches_len,
                            first_line: first_line.get(),
                            done: false,
                            generation: session_generation,
                        },
                    );
                }
            },
        ) else {
            return;
        };

        let summary = {
            let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
                return;
            };
            if !app_state.is_current_search_task(task_generation) {
                return;
            }
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
pub fn line_to_result_index(line_no: u64, state: State<AppState>) -> Option<NavigationTargetDto> {
    let guard = state.lock_session();
    let session = guard.as_ref()?;
    let result_index = session.result_index_for_line_no(line_no)?;
    Some(NavigationTargetDto {
        line_no,
        result_index,
    })
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

// 导出可能处理 10GB+ 文件,必须放到阻塞线程池,避免冻结主线程(命令窗口)。
#[tauri::command]
pub async fn export_logs(
    request: ExportRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ExportSummaryDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || export_logs_blocking(&app_state, request, app))
        .await
        .map_err(|err| err.to_string())?
}

fn export_logs_blocking(
    app_state: &AppState,
    request: ExportRequest,
    app: AppHandle,
) -> Result<ExportSummaryDto, String> {
    // 导出全程用同一 generation 快照;发现代号失效即中止(见 run_chunked_export)。
    let session_generation = app_state.generation.load(Ordering::SeqCst);
    let mut on_progress = |written_lines: usize, written_bytes: u64, done: bool| {
        let _ = app.emit(
            "export:progress",
            ExportProgressDto {
                written_lines,
                written_bytes,
                done,
            },
        );
    };
    run_chunked_export(app_state, session_generation, &request, &mut on_progress)
}

/// 分段导出编排。进度回调 on_progress(written_lines, written_bytes, done) 在锁外调用;
/// 事件发送由 export_logs_blocking 注入闭包完成,本函数不依赖 Tauri,可直接单测。
fn run_chunked_export(
    app_state: &AppState,
    session_generation: u64,
    request: &ExportRequest,
    on_progress: &mut dyn FnMut(usize, u64, bool),
) -> Result<ExportSummaryDto, String> {
    if request.path.trim().is_empty() {
        return Err("export path is required".to_string());
    }
    let output = PathBuf::from(&request.path);
    let is_range = request.mode == "range";

    // Phase 0:首次持锁——校验导出目标 + range 参数(沿用 export_range 文案)。
    let range = if is_range {
        let start = request
            .start_line
            .ok_or_else(|| "range start line is required".to_string())?;
        let end = request
            .end_line
            .ok_or_else(|| "range end line is required".to_string())?;
        Some((start, end))
    } else {
        None
    };
    {
        let Some(guard) = app_state.lock_session_if_current(session_generation) else {
            return Err("session changed during export".to_string());
        };
        let Some(session) = guard.as_ref() else {
            return Err("open a log file before exporting".to_string());
        };
        session
            .validate_export_target(&output)
            .map_err(|err| err.to_string())?;
        if let Some((start, end)) = range {
            if start == 0 || end < start {
                return Err("export range must be 1-based and ascending".to_string());
            }
        }
    }

    // Phase A:分块驱动索引补完;每轮锁外让出,避免饿死 get_rows/get_status。
    loop {
        let done = {
            let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
                return Err("session changed during export".to_string());
            };
            let Some(session) = guard.as_mut() else {
                return Err("open a log file before exporting".to_string());
            };
            session.index_step(INDEX_BUDGET)
        };
        std::thread::yield_now();
        if done {
            break;
        }
    }

    // Phase B:确定导出对象(0-based 源行号来源)。
    let plan = if let Some((start_line, end_line)) = range {
        // range 模式:不重建过滤,输出行号区间 [start-1, end)。
        let Some(guard) = app_state.lock_session_if_current(session_generation) else {
            return Err("session changed during export".to_string());
        };
        let Some(session) = guard.as_ref() else {
            return Err("open a log file before exporting".to_string());
        };
        let total = session.total_lines() as u64;
        let start = start_line.min(total + 1);
        let end = end_line.min(total);
        // 空区间输出 0 行;转 0-based AllLines 风格区间(下面统一切片)。
        let count = end.saturating_sub(start.saturating_sub(1));
        ExportSource::Range {
            first: start.saturating_sub(1) as usize,
            len: count as usize,
        }
    } else {
        let view_str = request.view.as_deref().unwrap_or("all");
        let view = rows_view_from_str(view_str)
            .ok_or_else(|| format!("unknown export view: {view_str}"))?;
        let active_spec = {
            let Some(guard) = app_state.lock_session_if_current(session_generation) else {
                return Err("session changed during export".to_string());
            };
            let Some(session) = guard.as_ref() else {
                return Err("open a log file before exporting".to_string());
            };
            session.active_filter_spec()
        };
        if view == logcore::session::RowsView::Filtered {
            if let Some(spec) = active_spec {
                // Filtered 激活:按当前 spec 分块重算出**局部**命中数组(不写回 session)。
                let matcher = FilterMatcher::new(&spec).map_err(|err| err.message)?;
                let Some(indices) = run_chunked_scan(
                    app_state,
                    session_generation,
                    || true,
                    |session, start, end| session.filter_indexed_range(&matcher, start, end),
                    |_, _| {},
                ) else {
                    return Err("session changed during export".to_string());
                };
                ExportSource::Indices(indices)
            } else {
                // 过滤未激活:退化为 All(与 export_view 一致)。
                ExportSource::from_plan(plan_snapshot(app_state, session_generation, view)?)
            }
        } else {
            ExportSource::from_plan(plan_snapshot(app_state, session_generation, view)?)
        }
    };

    // Phase C:锁外建文件,分批"锁内拷字节、锁外写盘"。
    let file = File::create(&output).map_err(|err| err.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let mut buf: Vec<u8> = Vec::new();
    let mut written_lines = 0usize;
    let mut written_bytes = 0u64;
    let mut last_emitted = 0usize;

    let total_len = plan.len();
    let mut cursor = 0usize;
    while cursor < total_len {
        let batch_end = cursor.saturating_add(EXPORT_CHUNK_LINES).min(total_len);
        buf.clear();
        let batch_lines;
        {
            let Some(guard) = app_state.lock_session_if_current(session_generation) else {
                return Err("session changed during export".to_string());
            };
            let Some(session) = guard.as_ref() else {
                return Err("open a log file before exporting".to_string());
            };
            // 单次持锁批量拷贝:一次前向扫描解析该批 span,取代逐行检查点回退。
            let (lines, bytes) = match &plan {
                ExportSource::Range { first, .. } => {
                    session.append_line_range_bytes(first + cursor, batch_end - cursor, &mut buf)
                }
                ExportSource::Indices(indices) => {
                    session.append_sorted_lines_bytes(&indices[cursor..batch_end], &mut buf)
                }
            };
            batch_lines = lines;
            written_bytes += bytes;
        }
        writer.write_all(&buf).map_err(|err| err.to_string())?;
        written_lines += batch_lines;
        if written_lines - last_emitted >= EXPORT_PROGRESS_STRIDE {
            last_emitted = written_lines;
            on_progress(written_lines, written_bytes, false);
        }
        cursor = batch_end;
        std::thread::yield_now();
    }

    writer.flush().map_err(|err| err.to_string())?;
    on_progress(written_lines, written_bytes, true);
    Ok(ExportSummaryDto {
        written_lines,
        written_bytes,
    })
}

/// 导出对象:统一成"按 view_idx 取 0-based 源行号"的迭代抽象。
enum ExportSource {
    /// 连续区间 [first, first+len)(All / Range 复用)。
    Range { first: usize, len: usize },
    /// 离散 0-based 源行号数组(Filtered / Bookmarks / Errors)。
    Indices(Vec<u32>),
}

impl ExportSource {
    fn from_plan(plan: logcore::session::ExportPlan) -> Self {
        match plan {
            logcore::session::ExportPlan::AllLines { total } => ExportSource::Range {
                first: 0,
                len: total,
            },
            logcore::session::ExportPlan::Indices(indices) => ExportSource::Indices(indices),
        }
    }

    fn len(&self) -> usize {
        match self {
            ExportSource::Range { len, .. } => *len,
            ExportSource::Indices(indices) => indices.len(),
        }
    }
}

/// 持锁取一次 export_plan_snapshot(非 Filtered 或过滤未激活分支)。
fn plan_snapshot(
    app_state: &AppState,
    session_generation: u64,
    view: logcore::session::RowsView,
) -> Result<logcore::session::ExportPlan, String> {
    let Some(guard) = app_state.lock_session_if_current(session_generation) else {
        return Err("session changed during export".to_string());
    };
    let Some(session) = guard.as_ref() else {
        return Err("open a log file before exporting".to_string());
    };
    Ok(session.export_plan_snapshot(view))
}

// 切分同样可能处理超大文件,放到阻塞线程池;分片进度经 split:progress 事件回传。
#[tauri::command]
pub async fn split_log_file(
    request: SplitRequest,
    app: AppHandle,
) -> Result<SplitSummaryDto, String> {
    tauri::async_runtime::spawn_blocking(move || split_log_file_blocking(request, &app))
        .await
        .map_err(|err| err.to_string())?
}

fn split_log_file_blocking(
    request: SplitRequest,
    app: &AppHandle,
) -> Result<SplitSummaryDto, String> {
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
    let summary = logcore::split::split_file_with_progress(
        &PathBuf::from(request.path),
        &PathBuf::from(request.out_dir),
        mode,
        &mut |parts, bytes_processed| {
            let _ = app.emit(
                "split:progress",
                SplitProgressDto {
                    parts,
                    bytes_processed,
                },
            );
        },
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
pub fn set_config(
    config: AppConfigDto,
    state: State<AppState>,
    app: AppHandle,
) -> Result<AppConfigDto, String> {
    let path = logcore::config::default_config_path();
    let config = logcore::config::AppConfig::try_from(config)?;
    logcore::config::save_config(&path, &config).map_err(|err| err.to_string())?;
    {
        let mut guard = state.lock_session();
        if let Some(session) = guard.as_mut() {
            session.set_encoding(config_encoding(&config));
        }
    }
    let session_generation = state.generation.load(Ordering::SeqCst);
    rerun_scans_after_index_done(state.inner(), &app, session_generation);
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

    #[test]
    fn prunes_old_stream_sessions_keeping_newest() {
        let dir = tempfile::tempdir().unwrap();
        for millis in [1, 2, 3, 4] {
            std::fs::write(dir.path().join(format!("logcat-{millis}.log")), b"x").unwrap();
        }
        std::fs::write(dir.path().join("user-notes.log"), b"keep me").unwrap();

        prune_stream_sessions(dir.path(), 2);

        assert!(!dir.path().join("logcat-1.log").exists());
        assert!(!dir.path().join("logcat-2.log").exists());
        assert!(dir.path().join("logcat-3.log").exists());
        assert!(dir.path().join("logcat-4.log").exists());
        assert!(dir.path().join("user-notes.log").exists());
    }

    fn export_state_with_session(path: &std::path::Path) -> (AppState, u64) {
        let mut session = logcore::session::Session::open(path).unwrap();
        session.index_all();
        let state = AppState::new();
        let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *state.lock_session() = Some(session);
        (state, generation)
    }

    #[test]
    fn chunked_export_matches_export_view_output_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.log");
        let mut content = String::new();
        for i in 0..9000 {
            let level = if i % 7 == 0 { "E" } else { "I" };
            content.push_str(&format!(
                "04-20 12:06:02.{:03}   200   220 {level} Net: msg {i}\n",
                i % 1000
            ));
        }
        std::fs::write(&src, &content).unwrap();

        // 期望输出:独立会话跑旧路径 export_view(Filtered)
        let expected_path = dir.path().join("expected.log");
        let spec = logcore::filter::FilterSpec {
            levels: logcore::filter::LevelMask::from_levels(&["E", "F"]),
            ..Default::default()
        };
        {
            let mut oracle = logcore::session::Session::open(&src).unwrap();
            oracle.index_all();
            oracle.set_filter(&spec).unwrap();
            oracle
                .export_view(logcore::session::RowsView::Filtered, &expected_path)
                .unwrap();
        }

        // 实际输出:分段导出(9000 行 > EXPORT_CHUNK_LINES,保证跨批)
        let (state, generation) = export_state_with_session(&src);
        {
            let mut guard = state.lock_session();
            guard.as_mut().unwrap().set_filter(&spec).unwrap();
        }
        let out_path = dir.path().join("chunked.log");
        let request = ExportRequest {
            mode: "view".to_string(),
            view: Some("filtered".to_string()),
            start_line: None,
            end_line: None,
            path: out_path.to_string_lossy().to_string(),
        };
        let mut progress_calls = Vec::new();
        let summary = run_chunked_export(&state, generation, &request, &mut |lines, bytes, done| {
            progress_calls.push((lines, bytes, done));
        })
        .unwrap();

        let expected = std::fs::read(&expected_path).unwrap();
        let actual = std::fs::read(&out_path).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(summary.written_bytes as usize, actual.len());
        assert_eq!(progress_calls.last().map(|c| c.2), Some(true)); // 最终 done 事件
    }

    #[test]
    fn chunked_export_all_view_crosses_write_batches() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.log");
        let mut content = String::new();
        for i in 0..9000 {
            content.push_str(&format!(
                "04-20 12:06:02.{:03}   200   220 I Net: msg {i}\n",
                i % 1000
            ));
        }
        std::fs::write(&src, &content).unwrap();

        let expected_path = dir.path().join("expected.log");
        {
            let mut oracle = logcore::session::Session::open(&src).unwrap();
            oracle.index_all();
            oracle
                .export_view(logcore::session::RowsView::All, &expected_path)
                .unwrap();
        }

        let (state, generation) = export_state_with_session(&src);
        let out_path = dir.path().join("chunked.log");
        let request = ExportRequest {
            mode: "view".to_string(),
            view: Some("all".to_string()),
            start_line: None,
            end_line: None,
            path: out_path.to_string_lossy().to_string(),
        };
        let mut progress_calls = Vec::new();
        let summary = run_chunked_export(&state, generation, &request, &mut |lines, bytes, done| {
            progress_calls.push((lines, bytes, done));
        })
        .unwrap();

        // 9000 行 / 4096 每批 = 3 个写批次;输出必须与 oracle 完全一致
        assert_eq!(
            std::fs::read(&out_path).unwrap(),
            std::fs::read(&expected_path).unwrap()
        );
        assert_eq!(summary.written_lines, 9000);
        assert_eq!(progress_calls.last().map(|c| c.2), Some(true));
    }

    #[test]
    fn chunked_export_range_matches_export_range_output_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.log");
        std::fs::write(
            &src,
            "04-20 12:06:02.125   146   179 D A: one\n04-20 12:06:02.225   200   220 I B: two\n04-20 12:06:02.325   200   221 W C: three\n",
        )
        .unwrap();
        let expected_path = dir.path().join("expected.log");
        {
            let mut oracle = logcore::session::Session::open(&src).unwrap();
            oracle.index_all();
            oracle.export_range(2, 3, &expected_path).unwrap();
        }
        let (state, generation) = export_state_with_session(&src);
        let out_path = dir.path().join("chunked.log");
        let request = ExportRequest {
            mode: "range".to_string(),
            view: None,
            start_line: Some(2),
            end_line: Some(3),
            path: out_path.to_string_lossy().to_string(),
        };
        let summary =
            run_chunked_export(&state, generation, &request, &mut |_, _, _| {}).unwrap();
        assert_eq!(
            std::fs::read(&out_path).unwrap(),
            std::fs::read(&expected_path).unwrap()
        );
        assert_eq!(summary.written_lines, 2);
    }

    #[test]
    fn chunked_export_aborts_when_session_generation_changes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.log");
        std::fs::write(&src, "04-20 12:06:02.125   146   179 D A: one\n").unwrap();
        let (state, generation) = export_state_with_session(&src);
        state.generation.fetch_add(1, Ordering::SeqCst); // 模拟导出期间 open 了新文件
        let request = ExportRequest {
            mode: "view".to_string(),
            view: Some("all".to_string()),
            start_line: None,
            end_line: None,
            path: dir.path().join("out.log").to_string_lossy().to_string(),
        };
        let err = run_chunked_export(&state, generation, &request, &mut |_, _, _| {}).unwrap_err();
        assert!(err.contains("session changed"), "{err}");
    }

    #[test]
    fn reset_stream_session_file_drops_old_mmap_then_truncates() {
        let state = AppState::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logcat-1.log");
        std::fs::write(&path, "04-20 12:06:02.125   146   179 D T: one\n").unwrap();
        let mut session = logcore::session::Session::open(&path).unwrap();
        session.index_all();
        session.toggle_bookmark(1).unwrap();
        *state.lock_session() = Some(session);
        let before = state.generation.load(Ordering::SeqCst);

        let generation =
            reset_stream_session_file(&state, &path, logcore::encoding::TextEncoding::Utf8)
                .unwrap();

        assert_eq!(generation, before + 1);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert!(!logcore::bookmarks::sidecar_path_for(&path).exists());
        let guard = state.lock_session();
        assert_eq!(guard.as_ref().unwrap().total_lines(), 0);
    }
}
