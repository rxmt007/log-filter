use crate::dto::{
    AnalysisTokenDto, AppConfigDto, CheckedRowsDto, CheckedRowsRequest, DeviceListDto,
    ExportProgressDto, ExportRequest, ExportSummaryDto, FilterDoneDto, FilterSpecDto,
    LineMappingBiasDto, LineMappingRequest, LineMappingResponseDto, LineMappingStatusDto,
    MinimapBucketDto, MinimapDto, NavigationTargetDto, ProblemExportModeDto, ProblemExportRequest,
    ProblemsProgressDto, Row, SearchProgressDto, SearchResult, SearchSpecDto, SplitProgressDto,
    SplitRequest, SplitSummaryDto, StartLogcatRequest, Status, StreamAppendDto, StreamControlDto,
};
use crate::problems::{
    problem_export_range, problems_progress_from, spawn_problem_catchup, step_problem_analysis,
    ProblemProgressGate, PROBLEM_CATCH_UP_STEPS_PER_INDEX,
};
use crate::state::AppState;
use crate::state::{StreamLifecycle, StreamRequestState, StreamTask};
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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

const INDEX_BUDGET: usize = 1024 * 1024; // 控制大文件索引期的 Session 锁尾延迟
const INDEX_PROGRESS_STRIDE: usize = 8 * 1024 * 1024; // 保持既有 UI 进度事件频率
const SCAN_CHUNK_LINES: usize = 4096;
const SEARCH_PROGRESS_STRIDE: usize = 65_536; // 搜索进度事件节流阈值(约 16 个扫描块)
const EXPORT_CHUNK_LINES: usize = 4096;
const EXPORT_PROGRESS_STRIDE: usize = 65_536; // 与 SEARCH_PROGRESS_STRIDE 同数量级
const STREAM_READ_BUF: usize = 64 * 1024;
const STREAM_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const STREAM_STOP_POLL: Duration = Duration::from_millis(5);
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

#[derive(Debug, PartialEq, Eq)]
enum StreamReadOutcome {
    Bytes(usize),
    NaturalEof,
    ControlledCancellation,
}

fn classify_stream_read(
    result: std::io::Result<usize>,
    stream_task_is_current: bool,
) -> Result<StreamReadOutcome, String> {
    match result {
        Ok(0) => Ok(StreamReadOutcome::NaturalEof),
        Ok(read) => Ok(StreamReadOutcome::Bytes(read)),
        Err(_) if !stream_task_is_current => Ok(StreamReadOutcome::ControlledCancellation),
        Err(error) => Err(format!("failed to read adb logcat: {error}")),
    }
}

fn status_from(session: &logcore::session::Session, generation: u64, state: &AppState) -> Status {
    Status {
        // stableLines DTO 在 Task 10/11 贯通前，现有 totalLines 继续作为前端可见 rowCount；
        // 诊断用“已发现行首数”只保留在 Session::total_lines。
        total_lines: session.stable_lines(),
        stable_lines: session.stable_lines(),
        filtered_lines: session.filtered_count(),
        bookmark_lines: session.bookmark_count(),
        error_lines: session.error_count(),
        indexed_bytes: session.indexed_bytes() as u64,
        total_bytes: session.total_bytes() as u64,
        indexing: !session.is_indexing_done(),
        generation,
        analysis_generation: state.current_analysis_generation(),
        filter_input_revision: state.filter_input_revision.load(Ordering::SeqCst),
        applied_filter_input_revision: state.applied_filter_input_revision.load(Ordering::SeqCst),
        filter_result_revision: state.filter_result_revision.load(Ordering::SeqCst),
        decode_revision: state.decode_revision.load(Ordering::SeqCst),
        source_data_revision: state.source_data_revision.load(Ordering::SeqCst),
    }
}

fn empty_status(generation: u64, state: &AppState) -> Status {
    Status {
        total_lines: 0,
        stable_lines: 0,
        filtered_lines: 0,
        bookmark_lines: 0,
        error_lines: 0,
        indexed_bytes: 0,
        total_bytes: 0,
        indexing: false,
        generation,
        analysis_generation: state.current_analysis_generation(),
        filter_input_revision: state.filter_input_revision.load(Ordering::SeqCst),
        applied_filter_input_revision: state.applied_filter_input_revision.load(Ordering::SeqCst),
        filter_result_revision: state.filter_result_revision.load(Ordering::SeqCst),
        decode_revision: state.decode_revision.load(Ordering::SeqCst),
        source_data_revision: state.source_data_revision.load(Ordering::SeqCst),
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

fn load_app_config(state: &AppState) -> Result<logcore::config::AppConfig, String> {
    let _control = state.lock_config_control();
    let path = logcore::config::default_config_path();
    logcore::config::load_config(&path).map_err(|err| err.to_string())
}

fn config_encoding(config: &logcore::config::AppConfig) -> logcore::encoding::TextEncoding {
    logcore::encoding::TextEncoding::from_config(&config.encoding)
}

struct ActiveSessionEncodingUpdate {
    session_generation: u64,
    changed: bool,
    catchup_analysis_generation: Option<u64>,
    indexing_done: bool,
    dataset_status: Option<Status>,
}

fn update_active_session_encoding(
    state: &AppState,
    requested_encoding: logcore::encoding::TextEncoding,
) -> ActiveSessionEncodingUpdate {
    let (session_generation, _, mut guard) = state.lock_session_with_generations();
    let Some(session) = guard.as_mut() else {
        return ActiveSessionEncodingUpdate {
            session_generation,
            changed: false,
            catchup_analysis_generation: None,
            indexing_done: false,
            dataset_status: None,
        };
    };
    if session.encoding_config_label() == requested_encoding.config_label() {
        return ActiveSessionEncodingUpdate {
            session_generation,
            changed: false,
            catchup_analysis_generation: None,
            indexing_done: false,
            dataset_status: None,
        };
    }

    let filter_needs_rescan = session.desired_filter_spec().is_some();
    let search_needs_rescan = session.desired_search_spec().is_some();
    let analysis_generation = state.next_analysis_generation();
    state.bump_decode_revision();
    if filter_needs_rescan {
        state
            .applied_filter_input_revision
            .store(0, Ordering::SeqCst);
        state.bump_filter_result_revision();
    }
    if search_needs_rescan {
        state
            .applied_search_input_revision
            .store(0, Ordering::SeqCst);
    }
    let indexing_done = session.is_indexing_done();
    // The Session lock is the publication boundary for the decoder, invalidated
    // derived datasets, and new Problems analysis identity.
    session.set_encoding(requested_encoding);
    ActiveSessionEncodingUpdate {
        session_generation,
        changed: true,
        catchup_analysis_generation: indexing_done.then_some(analysis_generation),
        indexing_done,
        dataset_status: Some(status_from(session, session_generation, state)),
    }
}

fn resolve_adb_from_config(config: &logcore::config::AppConfig) -> Result<PathBuf, String> {
    logcore::adb::resolve_adb_path(config.adb_path.as_deref())
        .ok_or_else(|| "adb executable was not found".to_string())
}

fn parse_buffers(buffers: &[String]) -> Result<Vec<logcore::adb::LogcatBuffer>, String> {
    if buffers.is_empty() {
        return Ok(vec![logcore::adb::LogcatBuffer::Main]);
    }
    let mut parsed = Vec::new();
    for buffer in buffers {
        let buffer = logcore::adb::LogcatBuffer::try_from(buffer.as_str())?;
        if !parsed.contains(&buffer) {
            parsed.push(buffer);
        }
    }
    Ok(parsed)
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
        return Ok(spec.buffers);
    }
    if !request.buffers.is_empty() {
        return parse_buffers(&request.buffers);
    }
    let spec = logcore::adb::LogcatSpec::parse(&config.current_command)?;
    Ok(spec.buffers)
}

fn problem_buffer_set(buffers: &[logcore::adb::LogcatBuffer]) -> logcore::problems::BufferSet {
    buffers
        .iter()
        .fold(logcore::problems::BufferSet::NONE, |set, buffer| {
            set | match buffer {
                logcore::adb::LogcatBuffer::Main => logcore::problems::BufferSet::MAIN,
                logcore::adb::LogcatBuffer::System => logcore::problems::BufferSet::SYSTEM,
                logcore::adb::LogcatBuffer::Radio => logcore::problems::BufferSet::RADIO,
                logcore::adb::LogcatBuffer::Events => logcore::problems::BufferSet::EVENTS,
                logcore::adb::LogcatBuffer::Crash => logcore::problems::BufferSet::CRASH,
            }
        })
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
                .is_some_and(|millis| {
                    !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit())
                })
        })
        .collect();
    sessions.sort();
    for stale in sessions.iter().rev().skip(keep) {
        let _ = fs::remove_file(stale);
        let _ = fs::remove_file(logcore::bookmarks::sidecar_path_for(stale));
    }
}

fn stream_status(state: &AppState) -> StreamControlDto {
    let (status, input_paused) = {
        let (generation, _, guard) = state.lock_session_with_generations();
        match guard.as_ref() {
            Some(session) => (
                status_from(session, generation, state),
                session.input_lifecycle() == Some(logcore::session::InputLifecycle::Paused),
            ),
            None => (empty_status(generation, state), false),
        }
    };
    let stream = state.lock_stream();
    StreamControlDto {
        status,
        lifecycle: stream.lifecycle.into(),
        running: stream.task.is_some(),
        paused: input_paused || stream.lifecycle == StreamLifecycle::Paused,
        error: stream.control_error.clone(),
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

fn confirm_child_terminal(
    child: &Arc<Mutex<std::process::Child>>,
    terminate_if_running: bool,
) -> Result<(), String> {
    {
        let mut child = lock_child(child);
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if terminate_if_running => {
                if let Err(kill_error) = child.kill() {
                    return match child.try_wait() {
                        Ok(Some(_)) => Ok(()),
                        _ => Err(format!("failed to terminate adb logcat: {kill_error}")),
                    };
                }
            }
            Ok(None) => {}
            Err(error) => return Err(format!("failed to inspect adb logcat: {error}")),
        }
    }

    let deadline = Instant::now() + STREAM_STOP_TIMEOUT;
    loop {
        {
            let mut child = lock_child(child);
            match child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(error) => return Err(format!("failed to confirm adb logcat exit: {error}")),
            }
        }
        if Instant::now() >= deadline {
            return Err("timed out confirming adb logcat exit".to_string());
        }
        std::thread::sleep(STREAM_STOP_POLL);
    }
}

/// 停止当前流式任务的语义模式,决定 `paused` 标记与 `last_request` 的去留。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamStop {
    /// 挂起:保留 last_request,标记 paused,可 resume。
    Pause,
    /// 停止:保留 last_request(供 clear 复用路径),不标记 paused。
    Stop,
    /// 遗弃:丢弃 last_request(切换到新会话前)。
    Forget,
}

impl StreamStop {
    fn clears_last_request(self) -> bool {
        matches!(self, Self::Forget)
    }
}

fn take_stream_task(state: &AppState, mode: StreamStop) -> Option<StreamTask> {
    let mut runtime = state.lock_stream();
    let task = runtime.task.take();
    if task.is_some() {
        state.next_stream_generation();
        runtime.lifecycle = match mode {
            StreamStop::Pause => StreamLifecycle::Pausing,
            StreamStop::Stop | StreamStop::Forget => StreamLifecycle::Finishing,
        };
        runtime.control_error = None;
        runtime.eof_confirmed = false;
    }
    task
}

fn publish_stream_stop(state: &AppState, mode: StreamStop) {
    let mut runtime = state.lock_stream();
    runtime.lifecycle = match mode {
        StreamStop::Pause => StreamLifecycle::Paused,
        StreamStop::Stop | StreamStop::Forget => StreamLifecycle::Stopped,
    };
    runtime.control_error = None;
    runtime.eof_confirmed = true;
    runtime.orphan_child = None;
    if mode.clears_last_request() {
        runtime.last_request = None;
    }
}

fn publish_stream_error(
    state: &AppState,
    error: String,
    eof_confirmed: bool,
    orphan_child: Option<Arc<Mutex<std::process::Child>>>,
) {
    let mut runtime = state.lock_stream();
    runtime.lifecycle = StreamLifecycle::ControlError;
    runtime.control_error = Some(error);
    runtime.eof_confirmed = eof_confirmed;
    runtime.orphan_child = orphan_child;
}

fn publish_paused_stream_error(state: &AppState, error: String) {
    let mut runtime = state.lock_stream();
    runtime.lifecycle = StreamLifecycle::Paused;
    runtime.control_error = Some(error);
    runtime.eof_confirmed = true;
    runtime.orphan_child = None;
}

fn emit_stream_control_error(app: &AppHandle, state: &AppState, error: &str) {
    let _ = app.emit("stream:error", error.to_string());
    let _ = app.emit("stream:control", stream_status(state));
}

fn stop_stream_task(
    state: &AppState,
    mode: StreamStop,
) -> Result<Option<ProblemsProgressDto>, String> {
    let task = take_stream_task(state, mode);
    if matches!(mode, StreamStop::Pause) && task.is_none() {
        return Err("no running logcat session to pause".to_string());
    }
    if task.is_none() {
        let control_error = {
            let runtime = state.lock_stream();
            (runtime.lifecycle == StreamLifecycle::ControlError).then(|| {
                (
                    runtime
                        .control_error
                        .clone()
                        .unwrap_or_else(|| "logcat transport control failed".to_string()),
                    runtime.orphan_child.clone(),
                )
            })
        };
        if let Some((error, orphan_child)) = control_error {
            if !matches!(mode, StreamStop::Forget) {
                return Err(error);
            }
            // Start/Open recover from ControlError by abandoning the old input
            // identity, never by pretending it reached a clean seal.
            if let Some(child) = orphan_child {
                confirm_child_terminal(&child, true)?;
            }
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::Stopped;
            runtime.control_error = None;
            runtime.eof_confirmed = false;
            runtime.orphan_child = None;
            runtime.last_request = None;
            return Ok(None);
        }
    }
    if let Some(task) = task {
        let child = task.child.clone();
        if let Err(error) = confirm_child_terminal(&child, true) {
            // If EOF cannot be confirmed, joining the reader can block forever
            // on a still-open pipe. Detach it and retain the child handle so a
            // later Forget can retry termination without faking a clean seal.
            drop(task.handle);
            let error = format!("failed to confirm logcat EOF: {error}");
            publish_stream_error(state, error.clone(), false, Some(child));
            return Err(error);
        }
        let reader_result = task.handle.join();
        // A successful wait confirms the pipe-producing process is terminal. If the
        // reader observed a write/read/remap failure, keep the Session unsealed.
        let terminal_error = match reader_result {
            Err(_) => Some("logcat reader thread panicked".to_string()),
            Ok(Err(error)) => Some(error),
            Ok(Ok(())) => None,
        };
        if let Some(error) = terminal_error {
            publish_stream_error(state, error.clone(), true, None);
            return Err(error);
        }
    }
    let progress = match transition_stream_session(state, mode) {
        Ok(progress) => progress,
        Err(error) => {
            publish_stream_error(state, error.clone(), true, None);
            return Err(error);
        }
    };
    publish_stream_stop(state, mode);
    Ok(progress)
}

fn transition_stream_session(
    state: &AppState,
    mode: StreamStop,
) -> Result<Option<ProblemsProgressDto>, String> {
    #[derive(Clone, Copy)]
    enum TailStep {
        Continue {
            reset_identity: Option<(u64, u64)>,
        },
        Finished {
            session_generation: u64,
            analysis_generation: u64,
            reset_identity: Option<(u64, u64)>,
        },
    }

    let (session_generation, analysis_generation) = loop {
        let step = {
            let (session_generation, analysis_generation, mut guard) =
                state.lock_session_with_generations();
            let Some(session) = guard.as_mut() else {
                break (session_generation, analysis_generation);
            };
            match (mode, session.input_lifecycle()) {
                (_, Some(logcore::session::InputLifecycle::Growing)) => {
                    let previous_stable = session.stable_lines();
                    let outcome = session
                        .remap_and_index_step(INDEX_BUDGET)
                        .map_err(|error| format!("failed to index logcat tail: {error}"))?;
                    let (current_session_generation, current_analysis_generation, reset_identity) =
                        if outcome.reset {
                            let identity = state.advance_input_identity_while_session_locked();
                            (
                                identity.0,
                                identity.1,
                                Some((session_generation, identity.0)),
                            )
                        } else {
                            (session_generation, analysis_generation, None)
                        };
                    let stable_lines = session.stable_lines();
                    if outcome.reset || stable_lines != previous_stable {
                        state.bump_source_data_revision();
                    }
                    let scan_start = if outcome.reset { 0 } else { previous_stable };
                    if append_filter_for_range(session, scan_start, stable_lines).is_some() {
                        state.bump_filter_result_revision();
                    }
                    let _ = append_search_for_range(session, scan_start, stable_lines);

                    if !outcome.done {
                        TailStep::Continue { reset_identity }
                    } else {
                        let before_transition = session.stable_lines();
                        match mode {
                            StreamStop::Pause => {
                                session.pause_growing_input().map_err(|error| {
                                    format!("failed to pause logcat input: {error}")
                                })?
                            }
                            StreamStop::Stop | StreamStop::Forget => {
                                session.seal_growing_input().map_err(|error| {
                                    format!("failed to seal logcat input: {error}")
                                })?
                            }
                        }
                        let after_transition = session.stable_lines();
                        if after_transition != before_transition {
                            state.bump_source_data_revision();
                            if append_filter_for_range(session, before_transition, after_transition)
                                .is_some()
                            {
                                state.bump_filter_result_revision();
                            }
                            let _ = append_search_for_range(
                                session,
                                before_transition,
                                after_transition,
                            );
                        }
                        TailStep::Finished {
                            session_generation: current_session_generation,
                            analysis_generation: current_analysis_generation,
                            reset_identity,
                        }
                    }
                }
                (
                    StreamStop::Stop | StreamStop::Forget,
                    Some(logcore::session::InputLifecycle::Paused),
                ) => {
                    // Pause already caught up every persisted byte. Stop only exposes
                    // the known terminal partial line and seals the input.
                    let previous_stable = session.stable_lines();
                    session
                        .seal_growing_input()
                        .map_err(|error| format!("failed to seal paused logcat input: {error}"))?;
                    let stable_lines = session.stable_lines();
                    if stable_lines != previous_stable {
                        state.bump_source_data_revision();
                        if append_filter_for_range(session, previous_stable, stable_lines).is_some()
                        {
                            state.bump_filter_result_revision();
                        }
                        let _ = append_search_for_range(session, previous_stable, stable_lines);
                    }
                    TailStep::Finished {
                        session_generation,
                        analysis_generation,
                        reset_identity: None,
                    }
                }
                (StreamStop::Pause, Some(logcore::session::InputLifecycle::Paused)) => {
                    return Err("logcat session is already paused".to_string());
                }
                (StreamStop::Pause, _) => {
                    return Err("active session cannot be paused".to_string());
                }
                _ => TailStep::Finished {
                    session_generation,
                    analysis_generation,
                    reset_identity: None,
                },
            }
        };

        let reset_identity = match step {
            TailStep::Continue { reset_identity } | TailStep::Finished { reset_identity, .. } => {
                reset_identity
            }
        };
        if let Some((previous, current)) = reset_identity {
            let mut runtime = state.lock_stream();
            if let Some(request) = runtime.last_request.as_mut() {
                if request.session_generation == previous {
                    request.session_generation = current;
                }
            }
        }
        match step {
            TailStep::Continue { .. } => std::thread::yield_now(),
            TailStep::Finished {
                session_generation,
                analysis_generation,
                ..
            } => break (session_generation, analysis_generation),
        }
    };

    if matches!(mode, StreamStop::Forget) {
        return Ok(None);
    }
    let mut last_progress = None;
    while let Some(progress) = step_problem_analysis(
        state,
        session_generation,
        analysis_generation,
        matches!(mode, StreamStop::Stop),
    ) {
        last_progress = Some(progress.clone());
        if progress.scanned_lines >= progress.stable_lines {
            break;
        }
        std::thread::yield_now();
    }
    Ok(last_progress)
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
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("failed to capture adb stdout".to_string());
        }
    };
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
    runtime.lifecycle = StreamLifecycle::Running;
    runtime.control_error = None;
    runtime.eof_confirmed = false;
    runtime.orphan_child = None;
    let _ = start_tx.send(());
    Ok(serial)
}

fn spawn_stream_reader(args: StreamReaderArgs) -> JoinHandle<Result<(), String>> {
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
        let mut active_session_generation = session_generation;
        let mut natural_eof = false;
        let mut result = (|| -> Result<(), String> {
            start_rx
                .recv()
                .map_err(|_| "logcat reader start was cancelled".to_string())?;
            let mut writer = OpenOptions::new()
                .append(true)
                .open(&session_path)
                .map_err(|error| format!("failed to open logcat session file: {error}"))?;
            let mut reader = BufReader::new(stdout);
            let mut buf = vec![0_u8; STREAM_READ_BUF];
            let mut problem_progress_gate = Arc::new(Mutex::new(ProblemProgressGate::default()));
            let mut last_analysis_generation = app_state.current_analysis_generation();

            loop {
                let read = match classify_stream_read(
                    reader.read(&mut buf),
                    app_state.is_current_stream_task(stream_generation),
                )? {
                    StreamReadOutcome::NaturalEof => {
                        natural_eof = true;
                        break;
                    }
                    StreamReadOutcome::ControlledCancellation => break,
                    StreamReadOutcome::Bytes(read) => read,
                };
                writer
                    .write_all(&buf[..read])
                    .and_then(|_| writer.flush())
                    .map_err(|error| format!("failed to persist adb logcat: {error}"))?;

                let update = {
                    let Some(mut guard) =
                        app_state.lock_session_if_current(active_session_generation)
                    else {
                        break;
                    };
                    if !app_state.is_current_stream_task(stream_generation) {
                        break;
                    }
                    let Some(session) = guard.as_mut() else {
                        break;
                    };
                    let previous_stable = session.stable_lines();
                    let outcome = session
                        .remap_and_index_step(INDEX_BUDGET)
                        .map_err(|error| format!("failed to remap adb logcat: {error}"))?;
                    let reset_generation = if outcome.reset {
                        // The source bytes and compact event ids now describe a new analysis
                        // dataset. Publish both input and analysis identities while the Session
                        // lock is still held, so old row/export/event requests are rejected.
                        let (session_generation, _) =
                            app_state.advance_input_identity_while_session_locked();
                        Some(session_generation)
                    } else {
                        None
                    };
                    let stable_lines = session.stable_lines();
                    if stable_lines != previous_stable {
                        app_state.bump_source_data_revision();
                    }
                    // 外部截断触发重建后,派生命中数组已清空,须从 0 起做一次完整重扫;
                    // 否则沿用增量的 previous_stable；尾半行不会提前进入任何派生扫描。
                    let scan_start = if outcome.reset { 0 } else { previous_stable };
                    if append_filter_for_range(session, scan_start, stable_lines).is_some() {
                        app_state.bump_filter_result_revision();
                    }
                    let search_progress =
                        append_search_for_range(session, scan_start, stable_lines).map(|summary| {
                            (
                                summary,
                                app_state
                                    .applied_search_input_revision
                                    .load(Ordering::SeqCst),
                            )
                        });
                    let status = status_from(
                        session,
                        reset_generation.unwrap_or(active_session_generation),
                        &app_state,
                    );
                    (status, search_progress, reset_generation)
                };

                let (status, search_progress, reset_generation) = update;
                if let Some(generation) = reset_generation {
                    active_session_generation = generation;
                    let mut runtime = app_state.lock_stream();
                    if let Some(request) = runtime.last_request.as_mut() {
                        if request.session_generation != generation
                            && request.session_path == session_path
                        {
                            request.session_generation = generation;
                        }
                    }
                }
                loop {
                    let analysis_generation = app_state.current_analysis_generation();
                    if analysis_generation != last_analysis_generation {
                        last_analysis_generation = analysis_generation;
                        problem_progress_gate =
                            Arc::new(Mutex::new(ProblemProgressGate::default()));
                    }
                    let Some(progress) = step_problem_analysis(
                        &app_state,
                        active_session_generation,
                        analysis_generation,
                        false,
                    ) else {
                        if app_state.generation.load(Ordering::SeqCst) != active_session_generation
                            || !app_state.is_current_stream_task(stream_generation)
                        {
                            break;
                        }
                        std::thread::yield_now();
                        continue;
                    };
                    let caught_up = progress.scanned_lines >= progress.stable_lines;
                    let emit_now = match problem_progress_gate.lock() {
                        Ok(mut gate) => gate.should_emit(&progress),
                        Err(poisoned) => poisoned.into_inner().should_emit(&progress),
                    };
                    if emit_now {
                        let _ = app.emit("problems:progress", progress.clone());
                    } else if caught_up {
                        let delay = match problem_progress_gate.lock() {
                            Ok(mut gate) => gate.schedule_deferred(Instant::now()),
                            Err(poisoned) => {
                                poisoned.into_inner().schedule_deferred(Instant::now())
                            }
                        };
                        if let Some(delay) = delay {
                            let deferred_gate = problem_progress_gate.clone();
                            let deferred_state = app_state.clone();
                            let deferred_app = app.clone();
                            let deferred_session_generation = active_session_generation;
                            let deferred_analysis_generation = analysis_generation;
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                let latest = {
                                    let guard = deferred_state.lock_analysis_if_current(
                                        deferred_session_generation,
                                        deferred_analysis_generation,
                                    );
                                    guard.as_ref().and_then(|guard| {
                                        guard.as_ref().map(|session| {
                                            problems_progress_from(
                                                session,
                                                AnalysisTokenDto {
                                                    session_generation: deferred_session_generation,
                                                    analysis_generation:
                                                        deferred_analysis_generation,
                                                },
                                            )
                                        })
                                    })
                                };
                                let should_emit = match deferred_gate.lock() {
                                    Ok(mut gate) => {
                                        gate.clear_deferred();
                                        latest
                                            .as_ref()
                                            .is_some_and(|progress| gate.should_emit(progress))
                                    }
                                    Err(poisoned) => {
                                        let mut gate = poisoned.into_inner();
                                        gate.clear_deferred();
                                        latest
                                            .as_ref()
                                            .is_some_and(|progress| gate.should_emit(progress))
                                    }
                                };
                                if should_emit
                                    && deferred_state.is_current_stream_task(stream_generation)
                                {
                                    if let Some(progress) = latest {
                                        let _ = deferred_app.emit("problems:progress", progress);
                                    }
                                }
                            });
                        }
                    }
                    if caught_up {
                        break;
                    }
                    std::thread::yield_now();
                }
                if let Some((summary, request_id)) = search_progress {
                    let _ = app.emit(
                        "search:progress",
                        SearchProgressDto {
                            scanned: status.total_lines,
                            matches: summary.count,
                            first_line: summary.first,
                            done: true,
                            generation: active_session_generation,
                            request_id,
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
            Ok(())
        })();

        let mut eof_confirmed = false;
        if natural_eof {
            match confirm_child_terminal(&child, false) {
                Ok(()) => eof_confirmed = true,
                Err(error) if app_state.is_current_stream_task(stream_generation) => {
                    result = Err(format!("failed to confirm natural logcat EOF: {error}"));
                }
                Err(_) => {}
            }
        }
        if result.is_err() && !eof_confirmed {
            let cleanup = confirm_child_terminal(&child, true);
            eof_confirmed = cleanup.is_ok();
            if let Err(error) = cleanup {
                result = Err(format!(
                    "{}; failed to terminate adb logcat: {error}",
                    result
                        .err()
                        .unwrap_or_else(|| "logcat reader failed".to_string())
                ));
            }
        }
        let finalize_natural_eof = result.is_ok()
            && natural_eof
            && app_state.generation.load(Ordering::SeqCst) == active_session_generation
            && app_state.is_current_stream_task(stream_generation);
        let owned_runtime = {
            let mut runtime = app_state.lock_stream();
            if runtime
                .task
                .as_ref()
                .is_some_and(|task| task.generation == stream_generation)
            {
                runtime.task = None;
                runtime.eof_confirmed = eof_confirmed;
                if finalize_natural_eof {
                    runtime.lifecycle = StreamLifecycle::Finishing;
                    runtime.control_error = None;
                    runtime.orphan_child = None;
                } else {
                    let error = result.as_ref().err().cloned().unwrap_or_else(|| {
                        "logcat reader stopped before input finalization".to_string()
                    });
                    runtime.lifecycle = StreamLifecycle::ControlError;
                    runtime.control_error = Some(error);
                    runtime.orphan_child = (!eof_confirmed).then(|| child.clone());
                }
                true
            } else {
                false
            }
        };
        if owned_runtime && result.is_err() {
            let error = result
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_else(|| "logcat reader failed".to_string());
            let _ = app.emit("stream:error", error);
            let _ = app.emit("stream:control", stream_status(&app_state));
        }
        if finalize_natural_eof {
            let finalize_state = app_state.clone();
            let finalize_app = app.clone();
            std::thread::spawn(move || {
                let _control = finalize_state.lock_stream_control();
                if finalize_state.generation.load(Ordering::SeqCst) != active_session_generation
                    || !finalize_state.is_current_stream_task(stream_generation)
                {
                    return;
                }
                match transition_stream_session(&finalize_state, StreamStop::Stop) {
                    Ok(progress) => {
                        publish_stream_stop(&finalize_state, StreamStop::Stop);
                        if let Some(progress) = progress {
                            let _ = finalize_app.emit("problems:progress", progress);
                        }
                        let _ = finalize_app.emit("stream:control", stream_status(&finalize_state));
                    }
                    Err(error) => {
                        publish_stream_error(&finalize_state, error.clone(), true, None);
                        let _ = finalize_app.emit("stream:error", error);
                        let _ = finalize_app.emit("stream:control", stream_status(&finalize_state));
                    }
                }
            });
        }
        result
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
pub async fn open_file(
    path: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Status, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || open_file_blocking(path, &app_state, app))
        .await
        .map_err(|err| err.to_string())?
}

fn open_file_blocking(path: String, state: &AppState, app: AppHandle) -> Result<Status, String> {
    let _control = state.lock_stream_control();
    let config = load_app_config(state)?;
    let session = logcore::session::Session::open_with_encoding(
        &PathBuf::from(&path),
        config_encoding(&config),
    )
    .map_err(|e| e.to_string())?;
    // Stage every fallible resource before abandoning the current input. Once
    // Forget succeeds, replacing the Session is an in-memory publication only.
    if let Err(error) = stop_stream_task(state, StreamStop::Forget) {
        emit_stream_control_error(&app, state, &error);
        return Err(error);
    }
    // 递增代号:上一个文件遗留的索引线程会在下一次循环检测到并自退。
    let (my_gen, _) = state.replace_session(session);
    let status = {
        let guard = state
            .lock_session_if_current(my_gen)
            .ok_or_else(|| "session changed while opening file".to_string())?;
        status_from(
            guard
                .as_ref()
                .ok_or_else(|| "session missing while opening file".to_string())?,
            my_gen,
            state,
        )
    };

    // 后台索引:小预算步进,步间释放锁,保证浏览不被阻塞。
    let app_state = state.clone();
    std::thread::spawn(move || {
        let mut problem_progress_gate = ProblemProgressGate::default();
        let mut last_analysis_generation = app_state.current_analysis_generation();
        let mut last_index_progress_bytes = 0usize;
        'indexing: loop {
            let snapshot = {
                let Some(mut guard) = app_state.lock_session_if_current(my_gen) else {
                    break; // 已被更晚的 open 取代
                };
                match guard.as_mut() {
                    Some(session) => {
                        let previous_stable = session.stable_lines();
                        let done = session.index_step(INDEX_BUDGET);
                        if session.stable_lines() != previous_stable {
                            app_state.bump_source_data_revision();
                        }
                        let indexed_bytes = session.indexed_bytes();
                        let status = should_emit_index_progress(
                            last_index_progress_bytes,
                            indexed_bytes,
                            done,
                        )
                        .then(|| status_from(session, my_gen, &app_state));
                        Some((status, indexed_bytes, done))
                    }
                    None => None, // 会话被清空,退出
                }
            };
            let Some((status, indexed_bytes, index_done)) = snapshot else {
                break;
            };
            if let Some(status) = status {
                last_index_progress_bytes = indexed_bytes;
                let _ = app.emit("index:progress", status);
            }

            // 索引与 Problems 使用两个独立短临界区；encoding 切换时从当前
            // analysis generation 继续，旧 generation 的批次会被校验拒绝。
            let analysis_generation = app_state.current_analysis_generation();
            if analysis_generation != last_analysis_generation {
                last_analysis_generation = analysis_generation;
                problem_progress_gate = ProblemProgressGate::default();
            }
            // Catch up the stable lines exposed by this index slice while their mmap
            // pages are still hot. Every Problems step still owns its own bounded
            // critical section and yields between steps.
            for _ in 0..PROBLEM_CATCH_UP_STEPS_PER_INDEX {
                let Some(progress) =
                    step_problem_analysis(&app_state, my_gen, analysis_generation, false)
                else {
                    break;
                };
                let caught_up = progress.scanned_lines >= progress.stable_lines;
                if problem_progress_gate.should_emit(&progress) {
                    let _ = app.emit("problems:progress", progress);
                }
                if caught_up {
                    break;
                }
                std::thread::yield_now();
            }

            if index_done {
                loop {
                    if app_state.generation.load(Ordering::SeqCst) != my_gen {
                        break 'indexing;
                    }
                    let analysis_generation = app_state.current_analysis_generation();
                    if analysis_generation != last_analysis_generation {
                        last_analysis_generation = analysis_generation;
                        problem_progress_gate = ProblemProgressGate::default();
                    }
                    let Some(progress) =
                        step_problem_analysis(&app_state, my_gen, analysis_generation, true)
                    else {
                        std::thread::yield_now();
                        continue;
                    };
                    let finished = progress.done;
                    if problem_progress_gate.should_emit(&progress) {
                        let _ = app.emit("problems:progress", progress);
                    }
                    if finished {
                        rerun_scans_after_index_done(&app_state, &app, my_gen);
                        break 'indexing;
                    }
                    std::thread::yield_now();
                }
            }
            std::thread::yield_now(); // 让出,减少与 get_rows 的锁争用
        }
    });

    Ok(status)
}

fn should_emit_index_progress(last_emitted: usize, indexed_bytes: usize, done: bool) -> bool {
    done || indexed_bytes.saturating_sub(last_emitted) >= INDEX_PROGRESS_STRIDE
}

// adb 挂起时 `list_devices` 可能阻塞数秒(引擎侧 5s 超时),放到阻塞线程池避免冻结命令窗口。
#[tauri::command]
pub async fn list_devices(state: State<'_, AppState>) -> Result<DeviceListDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || list_devices_blocking(&app_state))
        .await
        .map_err(|err| err.to_string())?
}

fn list_devices_blocking(state: &AppState) -> Result<DeviceListDto, String> {
    let config = load_app_config(state)?;
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
    let _control = state.lock_stream_control();
    let config = load_app_config(state)?;
    let adb_path = resolve_adb_from_config(&config)?;
    let buffers = parse_logcat_request_buffers(&config, &request)?;
    // Fail fast while the old input and its analysis worker are still intact.
    // The transport is checked again when spawned because device state can race.
    let devices = logcore::adb::list_devices(&adb_path).map_err(|err| err.to_string())?;
    logcore::adb::select_online_device(
        &devices,
        request
            .device_serial
            .as_deref()
            .filter(|serial| !serial.trim().is_empty()),
    )?;
    let session_path = stream_session_path(&config)?;
    File::create(&session_path).map_err(|err| err.to_string())?;
    let mut session = logcore::session::Session::open_growing_with_encoding(
        &session_path,
        config_encoding(&config),
    )
    .map_err(|err| err.to_string())?;
    session.set_problem_input_coverage(logcore::problems::InputCoverage::adb_live(
        problem_buffer_set(&buffers),
        logcore::problems::RangeCompleteness::StartTruncated,
    ));
    if let Err(error) = stop_stream_task(state, StreamStop::Forget) {
        drop(session);
        let _ = fs::remove_file(&session_path);
        emit_stream_control_error(&app, state, &error);
        return Err(error);
    }
    prune_stream_sessions(
        session_path.parent().unwrap_or(&session_path),
        STREAM_SESSION_KEEP,
    );
    let (session_generation, _) = state.replace_session(session);

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
    {
        let mut runtime = state.lock_stream();
        runtime.lifecycle = StreamLifecycle::Starting;
        runtime.control_error = None;
        runtime.eof_confirmed = false;
        runtime.orphan_child = None;
        runtime.last_request = Some(request_state.clone());
    }
    if let Err(error) = spawn_logcat_stream(state.clone(), app.clone(), request_state) {
        let _ = transition_stream_session(state, StreamStop::Stop);
        publish_stream_error(state, error.clone(), true, None);
        let _ = app.emit("stream:control", stream_status(state));
        return Err(error);
    }
    Ok(stream_status(state))
}

#[tauri::command]
pub async fn pause_logcat(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || pause_logcat_blocking(&app_state, app))
        .await
        .map_err(|err| err.to_string())?
}

fn pause_logcat_blocking(state: &AppState, app: AppHandle) -> Result<StreamControlDto, String> {
    let _control = state.lock_stream_control();
    match stop_stream_task(state, StreamStop::Pause) {
        Ok(Some(progress)) => {
            let _ = app.emit("problems:progress", progress);
        }
        Ok(None) => {}
        Err(error) => {
            emit_stream_control_error(&app, state, &error);
            return Err(error);
        }
    }
    Ok(stream_status(state))
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
    let _control = state.lock_stream_control();
    let mut request = prepare_paused_stream_resume(state)?;
    // 续抓时用最后一条日志时间戳做 `logcat -T`,避免 ring buffer 重放造成重复;
    // 尾部无可解析时间戳时 since_timestamp 保持 None,退化为全量重放。
    request.since_timestamp = read_session_tail(&request.session_path, 64 * 1024)
        .as_deref()
        .and_then(logcore::adb::last_log_timestamp);
    {
        let mut runtime = state.lock_stream();
        runtime.lifecycle = StreamLifecycle::Starting;
        runtime.control_error = None;
        runtime.eof_confirmed = false;
        runtime.orphan_child = None;
    }
    if let Err(error) = spawn_logcat_stream(state.clone(), app.clone(), request.clone()) {
        if let Some(mut guard) = state.lock_session_if_current(request.session_generation) {
            if let Some(session) = guard.as_mut() {
                let _ = session.pause_growing_input();
            }
        }
        // No reader owns the input yet, so the existing Session is safely
        // paused and may be retried or stopped. This is not an orphaned
        // transport/control failure.
        publish_paused_stream_error(state, error.clone());
        let _ = app.emit("stream:control", stream_status(state));
        return Err(error);
    }
    Ok(stream_status(state))
}

fn prepare_paused_stream_resume(state: &AppState) -> Result<StreamRequestState, String> {
    let request = {
        let runtime = state.lock_stream();
        if runtime.lifecycle != StreamLifecycle::Paused {
            return Err("no paused logcat session to resume".to_string());
        }
        runtime
            .last_request
            .clone()
            .ok_or_else(|| "no paused logcat session to resume".to_string())?
    };
    let Some(mut guard) = state.lock_session_if_current(request.session_generation) else {
        return Err("logcat session changed before resume".to_string());
    };
    let session = guard
        .as_mut()
        .ok_or_else(|| "no paused logcat session to resume".to_string())?;
    session
        .resume_paused_input()
        .map_err(|error| format!("cannot resume logcat session: {error}"))?;
    Ok(request)
}

/// 读会话文件末尾至多 max_bytes 的内容(lossy 解码),供 resume 提取最后时间戳。
fn read_session_tail(path: &std::path::Path, max_bytes: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))
        .ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

#[tauri::command]
pub async fn stop_logcat(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || stop_logcat_blocking(&app_state, app))
        .await
        .map_err(|err| err.to_string())?
}

fn stop_logcat_blocking(state: &AppState, app: AppHandle) -> Result<StreamControlDto, String> {
    let _control = state.lock_stream_control();
    match stop_stream_task(state, StreamStop::Stop) {
        Ok(Some(progress)) => {
            let _ = app.emit("problems:progress", progress);
        }
        Ok(None) => {}
        Err(error) => {
            emit_stream_control_error(&app, state, &error);
            return Err(error);
        }
    }
    Ok(stream_status(state))
}

/// 重建流式会话文件。顺序不可变:必须先 drop 旧 Session(释放 mmap),再截断文件——
/// Windows 上截断带活动映射的文件报 ERROR_USER_MAPPED_FILE;Unix 上并发读旧 mmap 会 SIGBUS。
fn reset_stream_session_file(
    state: &AppState,
    path: &std::path::Path,
    encoding: logcore::encoding::TextEncoding,
    problem_buffers: logcore::problems::BufferSet,
) -> Result<u64, String> {
    let (session_generation, _) = state.invalidate_session();
    File::create(path).map_err(|err| err.to_string())?;
    let _ = fs::remove_file(logcore::bookmarks::sidecar_path_for(path));
    let mut session = logcore::session::Session::open_growing_with_encoding(path, encoding)
        .map_err(|err| err.to_string())?;
    session.set_problem_input_coverage(logcore::problems::InputCoverage::adb_live(
        problem_buffers,
        logcore::problems::RangeCompleteness::StartTruncated,
    ));
    if !state.install_session_if_current(session_generation, session) {
        return Err("session changed while clearing logcat".to_string());
    }
    Ok(session_generation)
}

struct StreamClearPlan {
    session_path: PathBuf,
    problem_buffers: logcore::problems::BufferSet,
    stop_mode: StreamStop,
    restart_request: Option<StreamRequestState>,
}

fn plan_stream_clear(state: &AppState) -> Result<Option<StreamClearPlan>, String> {
    let runtime = state.lock_stream();
    let Some(request) = runtime.last_request.as_ref() else {
        return Ok(None);
    };
    let problem_buffers = problem_buffer_set(&parse_buffers(&request.buffers)?);
    let restart_request = (runtime.lifecycle == StreamLifecycle::Running).then(|| request.clone());
    let stop_mode =
        if restart_request.is_some() || runtime.lifecycle == StreamLifecycle::ControlError {
            // Clear must join the current reader before replacing/truncating its
            // mapped session. ControlError can only be abandoned through
            // Forget, but must not be restarted automatically.
            StreamStop::Forget
        } else {
            StreamStop::Stop
        };
    Ok(Some(StreamClearPlan {
        session_path: request.session_path.clone(),
        problem_buffers,
        stop_mode,
        restart_request,
    }))
}

fn retarget_cleared_stream_request(
    mut request: StreamRequestState,
    session_generation: u64,
    since_timestamp: Option<String>,
) -> StreamRequestState {
    request.session_generation = session_generation;
    request.since_timestamp = since_timestamp;
    request
}

fn settle_cleared_stream_without_restart(
    state: &AppState,
    session_generation: u64,
) -> Result<Option<ProblemsProgressDto>, String> {
    {
        let Some(mut guard) = state.lock_session_if_current(session_generation) else {
            return Err("session changed while finishing logcat clear".to_string());
        };
        let session = guard
            .as_mut()
            .ok_or_else(|| "no active session after clearing logcat".to_string())?;
        session
            .seal_growing_input()
            .map_err(|error| format!("failed to seal cleared logcat input: {error}"))?;
    }
    {
        let mut runtime = state.lock_stream();
        runtime.lifecycle = StreamLifecycle::Stopped;
        runtime.control_error = None;
        runtime.eof_confirmed = true;
        runtime.orphan_child = None;
        if let Some(request) = runtime.last_request.as_mut() {
            request.session_generation = session_generation;
        }
    }
    Ok(step_problem_analysis(
        state,
        session_generation,
        state.current_analysis_generation(),
        true,
    ))
}

#[tauri::command]
pub async fn clear_logcat(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<StreamControlDto, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || clear_logcat_blocking(&app_state, app))
        .await
        .map_err(|err| err.to_string())?
}

fn clear_logcat_blocking(state: &AppState, app: AppHandle) -> Result<StreamControlDto, String> {
    let _control = state.lock_stream_control();
    let Some(clear_plan) = plan_stream_clear(state)? else {
        return Ok(stream_status(state));
    };
    // Configuration IO is staged before terminating the current input. A
    // failure leaves the live reader and its Session untouched.
    let encoding = config_encoding(&load_app_config(state)?);
    if let Err(error) = stop_stream_task(state, clear_plan.stop_mode) {
        emit_stream_control_error(&app, state, &error);
        return Err(error);
    }

    // `stop_stream_task` has confirmed child termination and joined the reader,
    // so the tail is complete and no writer can race the following truncation.
    let since_timestamp = clear_plan.restart_request.as_ref().and_then(|_| {
        read_session_tail(&clear_plan.session_path, 64 * 1024)
            .as_deref()
            .and_then(logcore::adb::last_log_timestamp)
    });
    let session_generation = match reset_stream_session_file(
        state,
        &clear_plan.session_path,
        encoding,
        clear_plan.problem_buffers,
    ) {
        Ok(generation) => generation,
        Err(error) => {
            publish_stream_error(state, error.clone(), true, None);
            emit_stream_control_error(&app, state, &error);
            return Err(error);
        }
    };

    if let Some(request) = clear_plan.restart_request {
        let request = retarget_cleared_stream_request(request, session_generation, since_timestamp);
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::Starting;
            runtime.control_error = None;
            runtime.eof_confirmed = false;
            runtime.orphan_child = None;
            runtime.last_request = Some(request.clone());
        }
        if let Err(error) = spawn_logcat_stream(state.clone(), app.clone(), request) {
            let _ = transition_stream_session(state, StreamStop::Stop);
            publish_stream_error(state, error.clone(), true, None);
            let _ = app.emit("stream:control", stream_status(state));
            return Err(error);
        }
    } else {
        match settle_cleared_stream_without_restart(state, session_generation) {
            Ok(Some(progress)) => {
                let _ = app.emit("problems:progress", progress);
            }
            Ok(None) => {}
            Err(error) => {
                publish_stream_error(state, error.clone(), true, None);
                emit_stream_control_error(&app, state, &error);
                return Err(error);
            }
        }
    }
    Ok(stream_status(state))
}

fn rerun_scans_after_index_done(app_state: &AppState, app: &AppHandle, session_generation: u64) {
    let (filter_task, search_task) = {
        let Some(guard) = app_state.lock_session_if_current(session_generation) else {
            return;
        };
        let Some(session) = guard.as_ref() else {
            return;
        };
        let filter_task = session.desired_filter_spec().map(|spec| {
            (
                spec,
                app_state.next_filter_task_generation(),
                app_state.filter_input_revision.load(Ordering::SeqCst),
            )
        });
        let search_task = session.desired_search_spec().map(|spec| {
            (
                spec,
                app_state.next_search_task_generation(),
                app_state.search_input_revision.load(Ordering::SeqCst),
            )
        });
        (filter_task, search_task)
    };
    if let Some((spec, task_generation, filter_input_revision)) = filter_task {
        spawn_filter_task(
            app_state.clone(),
            app.clone(),
            spec,
            session_generation,
            task_generation,
            filter_input_revision,
        );
    }
    if let Some((spec, task_generation, search_request_id)) = search_task {
        spawn_search_task(
            app_state.clone(),
            app.clone(),
            spec,
            session_generation,
            task_generation,
            search_request_id,
        );
    }
}

#[tauri::command]
pub fn get_status(state: State<AppState>) -> Status {
    let (generation, _, guard) = state.lock_session_with_generations();
    match guard.as_ref() {
        Some(s) => status_from(s, generation, state.inner()),
        None => empty_status(generation, state.inner()),
    }
}

#[tauri::command]
pub async fn export_problem_logs(
    request: ProblemExportRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<ExportSummaryDto, String> {
    let app_state = state.inner().clone();
    let export_generation = app_state.next_export_generation();
    tauri::async_runtime::spawn_blocking(move || {
        let token = request.expected_analysis_token;
        let (start_line, end_line) = {
            let Some(guard) = app_state
                .lock_analysis_if_current(token.session_generation, token.analysis_generation)
            else {
                return Err("stale-analysis-token".to_string());
            };
            let session = guard
                .as_ref()
                .ok_or_else(|| "no-active-session".to_string())?;
            let event = session
                .problem_event(logcore::problems::ProblemEventId(request.event_id))
                .ok_or_else(|| "problem-event-not-found".to_string())?;
            problem_export_range(
                event,
                matches!(request.mode, ProblemExportModeDto::Context),
                request.radius.unwrap_or(50).min(4_096),
                session.stable_lines() as u64,
            )
        };
        let export = ExportRequest {
            mode: "range".to_string(),
            view: None,
            start_line: Some(start_line),
            end_line: Some(end_line),
            path: request.path,
        };
        export_logs_blocking_for_analysis(&app_state, token, export_generation, export, app)
    })
    .await
    .map_err(|err| err.to_string())?
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
pub fn get_rows_checked(
    request: CheckedRowsRequest,
    state: State<AppState>,
) -> Result<CheckedRowsDto, String> {
    get_rows_checked_for_state(request, state.inner())
}

fn get_rows_checked_for_state(
    request: CheckedRowsRequest,
    state: &AppState,
) -> Result<CheckedRowsDto, String> {
    if request.count == 0 || request.count > MAX_ROWS {
        return Err("row-window-limit".to_string());
    }
    let view = rows_view_from_str(&request.view).ok_or_else(|| "unknown-rows-view".to_string())?;
    let token = request.expected_analysis_token;
    let Some(guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return Err("stale-analysis-token".to_string());
    };
    let session = guard
        .as_ref()
        .ok_or_else(|| "no-active-session".to_string())?;
    let filter_result_revision = state.filter_result_revision.load(Ordering::SeqCst);
    if view == logcore::session::RowsView::Filtered
        && request.expected_filter_result_revision != Some(filter_result_revision)
    {
        return Ok(CheckedRowsDto::StaleFilterResult {
            analysis_token: token,
            request_nonce: request.request_nonce,
            actual_filter_result_revision: filter_result_revision,
        });
    }
    let rows = session
        .get_rows_for_view(view, request.start, request.count)
        .into_iter()
        .map(|(line_no, entry)| Row {
            line_no,
            date: entry.date,
            time: entry.time,
            level: entry.level,
            pid: entry.pid,
            tid: entry.tid,
            tag: entry.tag,
            message: entry.message,
            marked: session.is_bookmarked(line_no),
        })
        .collect();
    Ok(CheckedRowsDto::Ok {
        analysis_token: token,
        request_nonce: request.request_nonce,
        decode_revision: state.decode_revision.load(Ordering::SeqCst),
        source_data_revision: state.source_data_revision.load(Ordering::SeqCst),
        filter_result_revision,
        rows,
    })
}

#[tauri::command]
pub fn map_source_line(
    request: LineMappingRequest,
    state: State<AppState>,
) -> Result<LineMappingResponseDto, String> {
    map_source_line_for_state(request, state.inner())
}

fn map_source_line_for_state(
    request: LineMappingRequest,
    state: &AppState,
) -> Result<LineMappingResponseDto, String> {
    let token = request.expected_analysis_token;
    let Some(guard) =
        state.lock_analysis_if_current(token.session_generation, token.analysis_generation)
    else {
        return Err("stale-analysis-token".to_string());
    };
    let session = guard
        .as_ref()
        .ok_or_else(|| "no-active-session".to_string())?;
    let actual_revision = state.filter_result_revision.load(Ordering::SeqCst);
    if request.expected_filter_result_revision != actual_revision {
        return Ok(LineMappingResponseDto {
            status: LineMappingStatusDto::StaleFilterResult,
            analysis_token: token,
            filter_result_revision: actual_revision,
            request_nonce: request.request_nonce,
            target: None,
        });
    }
    let target = match request.bias {
        LineMappingBiasDto::Exact => {
            session
                .result_index_for_line_no(request.line_no)
                .map(|result_index| NavigationTargetDto {
                    line_no: request.line_no,
                    result_index,
                })
        }
        LineMappingBiasDto::Nearest => session
            .nearest_result_for_line_no(request.line_no)
            .map(Into::into),
    };
    Ok(LineMappingResponseDto {
        status: LineMappingStatusDto::Ok,
        analysis_token: token,
        filter_result_revision: actual_revision,
        request_nonce: request.request_nonce,
        target,
    })
}

#[tauri::command]
pub fn set_filter(
    filter: FilterSpecDto,
    filter_input_revision: Option<u64>,
    state: State<AppState>,
    app: AppHandle,
) -> Result<usize, String> {
    let spec: FilterSpec = filter.into();
    // Reject an invalid regex without cancelling or mutating the currently
    // published filter dataset.
    FilterMatcher::new(&spec).map_err(|error| error.message)?;
    let (session_generation, _, mut guard) = state.lock_session_with_generations();
    let Some(session) = guard.as_mut() else {
        return Ok(0);
    };
    let filter_input_revision = state
        .publish_filter_input_revision(filter_input_revision)
        .map_err(|current| format!("stale-filter-input:{current}"))?;
    let task_generation = state.next_filter_task_generation();
    let immediate = session
        .set_filter_pending(&spec)
        .map_err(|err| err.message)?;
    let current_count = session.filtered_count();

    if let Some(filtered_lines) = immediate {
        let filter_result_revision = state.complete_filter_result(filter_input_revision);
        drop(guard);
        let _ = app.emit(
            "filter:done",
            FilterDoneDto {
                filtered_lines,
                generation: session_generation,
                filter_input_revision,
                filter_result_revision,
            },
        );
        return Ok(filtered_lines);
    }

    drop(guard);
    spawn_filter_task(
        state.inner().clone(),
        app,
        spec,
        session_generation,
        task_generation,
        filter_input_revision,
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
    request_id: Option<u64>,
    state: State<AppState>,
    app: AppHandle,
) -> Result<SearchResult, String> {
    let spec: SearchSpec = spec.into();
    SearchMatcher::new(&spec).map_err(|error| error.message)?;
    let (session_generation, _, mut guard) = state.lock_session_with_generations();
    let Some(session) = guard.as_mut() else {
        return Ok(SearchResult {
            count: 0,
            first_line: None,
            request_id: 0,
        });
    };
    let request_id = state
        .publish_search_input_revision(request_id)
        .map_err(|current| format!("stale-search-input:{current}"))?;
    let task_generation = state.next_search_task_generation();
    let active = session
        .set_search_pending(&spec)
        .map_err(|err| err.message)?;

    if !active {
        state.complete_search_result(request_id);
        drop(guard);
        let _ = app.emit(
            "search:progress",
            SearchProgressDto {
                scanned: 0,
                matches: 0,
                first_line: None,
                done: true,
                generation: session_generation,
                request_id,
            },
        );
        return Ok(SearchResult {
            count: 0,
            first_line: None,
            request_id,
        });
    }

    drop(guard);
    spawn_search_task(
        state.inner().clone(),
        app,
        spec,
        session_generation,
        task_generation,
        request_id,
    );
    Ok(SearchResult {
        count: 0,
        first_line: None,
        request_id,
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
        guard.as_ref().map(|session| session.stable_lines())?
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
    filter_input_revision: u64,
) {
    let Ok(matcher) = FilterMatcher::new(&spec) else {
        return;
    };
    std::thread::spawn(move || {
        let mut total_lines = 0;
        let Some(mut matches) = run_chunked_scan(
            &app_state,
            session_generation,
            || app_state.is_current_filter_task(task_generation),
            |session, start, end| session.filter_indexed_range(&matcher, start, end),
            |scanned, _matches_len| total_lines = scanned,
        ) else {
            return;
        };

        let (filtered_lines, filter_result_revision) = loop {
            let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
                return;
            };
            if !app_state.is_current_filter_task(task_generation) {
                return;
            }
            if app_state.filter_input_revision.load(Ordering::SeqCst) != filter_input_revision {
                return;
            }
            let Some(session) = guard.as_mut() else {
                return;
            };
            let current_total = session.stable_lines();
            if total_lines < current_total {
                let chunk_end = total_lines
                    .saturating_add(SCAN_CHUNK_LINES)
                    .min(current_total);
                let extra = session.filter_indexed_range(&matcher, total_lines, chunk_end);
                drop(guard);
                matches.extend(extra);
                total_lines = chunk_end;
                std::thread::yield_now();
                continue;
            }
            let filtered_lines = session.apply_filter_results(&spec, matches);
            if !app_state.is_current_filter_task(task_generation)
                || app_state.filter_input_revision.load(Ordering::SeqCst) != filter_input_revision
            {
                return;
            }
            let filter_result_revision = app_state.complete_filter_result(filter_input_revision);
            break (filtered_lines, filter_result_revision);
        };
        let _ = app.emit(
            "filter:done",
            FilterDoneDto {
                filtered_lines,
                generation: session_generation,
                filter_input_revision,
                filter_result_revision,
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
    request_id: u64,
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
        let Some(mut matches) = run_chunked_scan(
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
                            request_id,
                        },
                    );
                }
            },
        ) else {
            return;
        };

        let summary = loop {
            let Some(mut guard) = app_state.lock_session_if_current(session_generation) else {
                return;
            };
            if !app_state.is_current_search_task(task_generation) {
                return;
            }
            let Some(session) = guard.as_mut() else {
                return;
            };
            let current_total = session.stable_lines();
            if total_lines < current_total {
                let chunk_end = total_lines
                    .saturating_add(SCAN_CHUNK_LINES)
                    .min(current_total);
                let extra = session.search_indexed_range(&matcher, total_lines, chunk_end);
                drop(guard);
                matches.extend(extra);
                total_lines = chunk_end;
                std::thread::yield_now();
                continue;
            }
            let summary = session.apply_search_results(&spec, matches);
            if !app_state.is_current_search_task(task_generation)
                || app_state.search_input_revision.load(Ordering::SeqCst) != request_id
            {
                return;
            }
            app_state.complete_search_result(request_id);
            break summary;
        };
        let _ = app.emit(
            "search:progress",
            SearchProgressDto {
                scanned: total_lines,
                matches: summary.count,
                first_line: summary.first,
                done: true,
                generation: session_generation,
                request_id,
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
            bucket_count: 0,
            bookmarks: Vec::new(),
            errors: Vec::new(),
        };
    };
    let minimap = session.minimap(buckets);
    MinimapDto {
        bucket_count: minimap.bucket_count,
        bookmarks: minimap.bookmarks,
        errors: minimap
            .errors
            .into_iter()
            .map(|entry| MinimapBucketDto {
                bucket: entry.bucket,
                count: entry.count,
            })
            .collect(),
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
    // 起始即领取新导出代号:这也会取消任何仍在跑的旧导出(cancel_export 同此机制)。
    let export_generation = app_state.next_export_generation();
    let session_generation = {
        let (generation, _, guard) = app_state.lock_session_with_generations();
        if guard.is_none() {
            return Err("open a log file before exporting".to_string());
        }
        generation
    };
    tauri::async_runtime::spawn_blocking(move || {
        export_logs_blocking(
            &app_state,
            session_generation,
            export_generation,
            request,
            app,
        )
    })
    .await
    .map_err(|err| err.to_string())?
}

/// 用户主动取消导出:递增导出代号,让在跑的 run_chunked_export 在下一批检测到并中止。
#[tauri::command]
pub fn cancel_export(state: State<AppState>) {
    state.next_export_generation();
}

fn export_logs_blocking(
    app_state: &AppState,
    session_generation: u64,
    export_generation: u64,
    request: ExportRequest,
    app: AppHandle,
) -> Result<ExportSummaryDto, String> {
    export_logs_blocking_bound(
        app_state,
        session_generation,
        None,
        export_generation,
        request,
        app,
    )
}

fn export_logs_blocking_for_analysis(
    app_state: &AppState,
    token: AnalysisTokenDto,
    export_generation: u64,
    request: ExportRequest,
    app: AppHandle,
) -> Result<ExportSummaryDto, String> {
    export_logs_blocking_bound(
        app_state,
        token.session_generation,
        Some(token.analysis_generation),
        export_generation,
        request,
        app,
    )
}

fn export_logs_blocking_bound(
    app_state: &AppState,
    session_generation: u64,
    expected_analysis_generation: Option<u64>,
    export_generation: u64,
    request: ExportRequest,
    app: AppHandle,
) -> Result<ExportSummaryDto, String> {
    // 导出全程使用调用边界原子捕获或 Problems token 已验证的 generation；
    // 不得在这里重读“当前”会话，否则 event range 可能落到替换后的文件。
    let output_path = request.path.clone();
    let mut on_progress = |written_lines: usize, written_bytes: u64, done: bool| {
        let _ = app.emit(
            "export:progress",
            ExportProgressDto {
                written_lines,
                written_bytes,
                done,
                // 仅最终成功事件携带输出路径,供前端 toast「打开所在目录」。
                path: done.then(|| output_path.clone()),
                cancelled: false,
            },
        );
    };
    let summary = match expected_analysis_generation {
        Some(analysis_generation) => run_chunked_export_for_analysis(
            app_state,
            AnalysisTokenDto {
                session_generation,
                analysis_generation,
            },
            export_generation,
            &request,
            &mut on_progress,
        ),
        None => run_chunked_export(
            app_state,
            session_generation,
            export_generation,
            &request,
            &mut on_progress,
        ),
    }?;
    // 取消不由 on_progress 上报(它不知道 cancelled),这里补发一个终态事件。
    if summary.cancelled {
        let _ = app.emit(
            "export:progress",
            ExportProgressDto {
                written_lines: summary.written_lines,
                written_bytes: summary.written_bytes,
                done: true,
                path: None,
                cancelled: true,
            },
        );
    }
    Ok(summary)
}

/// 分段导出编排。进度回调 on_progress(written_lines, written_bytes, done) 在锁外调用;
/// 事件发送由 export_logs_blocking 注入闭包完成,本函数不依赖 Tauri,可直接单测。
fn lock_export_session_if_current(
    state: &AppState,
    session_generation: u64,
    expected_analysis_generation: Option<u64>,
) -> Result<MutexGuard<'_, Option<logcore::session::Session>>, String> {
    let guard = state.lock_session();
    if state.generation.load(Ordering::SeqCst) != session_generation {
        return Err("session changed during export".to_string());
    }
    if expected_analysis_generation
        .is_some_and(|expected| state.current_analysis_generation() != expected)
    {
        return Err("analysis changed during export".to_string());
    }
    Ok(guard)
}

fn run_chunked_export(
    app_state: &AppState,
    session_generation: u64,
    export_generation: u64,
    request: &ExportRequest,
    on_progress: &mut dyn FnMut(usize, u64, bool),
) -> Result<ExportSummaryDto, String> {
    run_chunked_export_bound(
        app_state,
        session_generation,
        None,
        export_generation,
        request,
        on_progress,
    )
}

fn run_chunked_export_for_analysis(
    app_state: &AppState,
    token: AnalysisTokenDto,
    export_generation: u64,
    request: &ExportRequest,
    on_progress: &mut dyn FnMut(usize, u64, bool),
) -> Result<ExportSummaryDto, String> {
    run_chunked_export_bound(
        app_state,
        token.session_generation,
        Some(token.analysis_generation),
        export_generation,
        request,
        on_progress,
    )
}

fn run_chunked_export_bound(
    app_state: &AppState,
    session_generation: u64,
    expected_analysis_generation: Option<u64>,
    export_generation: u64,
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
        let guard = lock_export_session_if_current(
            app_state,
            session_generation,
            expected_analysis_generation,
        )?;
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
        // Phase A 的输出文件尚未创建,取消时无半成品可删。
        if !app_state.is_current_export(export_generation) {
            return Ok(cancelled_summary(0, 0));
        }
        let done = {
            let mut guard = lock_export_session_if_current(
                app_state,
                session_generation,
                expected_analysis_generation,
            )?;
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
        let guard = lock_export_session_if_current(
            app_state,
            session_generation,
            expected_analysis_generation,
        )?;
        let Some(session) = guard.as_ref() else {
            return Err("open a log file before exporting".to_string());
        };
        let total = session.stable_lines() as u64;
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
            let guard = lock_export_session_if_current(
                app_state,
                session_generation,
                expected_analysis_generation,
            )?;
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
                    || app_state.is_current_export(export_generation),
                    |session, start, end| session.filter_indexed_range(&matcher, start, end),
                    |_, _| {},
                ) else {
                    // None 可能是会话换掉或用户取消:代号已失效 ⇒ 取消(输出文件尚未创建)。
                    if !app_state.is_current_export(export_generation) {
                        return Ok(cancelled_summary(0, 0));
                    }
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

    // The plan may have been assembled over several lock windows (notably a
    // filtered export). Revalidate the Problems analysis identity immediately
    // before creating any output file.
    drop(lock_export_session_if_current(
        app_state,
        session_generation,
        expected_analysis_generation,
    )?);

    // Phase C:锁外建文件,分批"锁内拷字节、锁外写盘"。
    let file = File::create(&output).map_err(|err| err.to_string())?;
    let mut writer = std::io::BufWriter::new(file);
    let mut buf: Vec<u8> = Vec::new();
    let mut written_lines = 0usize;
    let mut written_bytes = 0u64;
    let mut last_emitted = 0usize;

    let total_len = plan.len();
    let mut cursor = 0usize;
    let mut first_batch = true;
    while cursor < total_len {
        // 每批开工前校验导出代号:失效即取消 —— 丢弃 writer、删半成品文件、返回 cancelled。
        // 已知窄 TOCTOU(评审定级 Low,接受):同路径快速连开两次导出时,旧任务此处的删除
        // 理论上可能落在新任务 File::create 之后;实际上旧任务下一批即检测到失效并删除,
        // 而新任务需走完 Phase A/B 才创建文件,时序上几乎不可能交叉,且新任务 create 会截断重建。
        if !app_state.is_current_export(export_generation) {
            drop(writer);
            let _ = fs::remove_file(&output);
            return Ok(cancelled_summary(written_lines, written_bytes));
        }
        let batch_end = cursor.saturating_add(EXPORT_CHUNK_LINES).min(total_len);
        buf.clear();
        let batch_lines;
        {
            let guard = match lock_export_session_if_current(
                app_state,
                session_generation,
                expected_analysis_generation,
            ) {
                Ok(guard) => guard,
                Err(error) => {
                    drop(writer);
                    let _ = fs::remove_file(&output);
                    return Err(error);
                }
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
        // 首批总是上报一次(即时反馈,也让取消得以在下一批生效),之后按 stride 节流。
        if first_batch || written_lines - last_emitted >= EXPORT_PROGRESS_STRIDE {
            first_batch = false;
            last_emitted = written_lines;
            on_progress(written_lines, written_bytes, false);
        }
        cursor = batch_end;
        std::thread::yield_now();
    }

    if let Err(error) =
        lock_export_session_if_current(app_state, session_generation, expected_analysis_generation)
    {
        drop(writer);
        let _ = fs::remove_file(&output);
        return Err(error);
    }
    writer.flush().map_err(|err| err.to_string())?;
    on_progress(written_lines, written_bytes, true);
    Ok(ExportSummaryDto {
        written_lines,
        written_bytes,
        cancelled: false,
    })
}

/// 取消导出的终态 summary(cancelled=true)。半成品文件已由调用点删除。
fn cancelled_summary(written_lines: usize, written_bytes: u64) -> ExportSummaryDto {
    ExportSummaryDto {
        written_lines,
        written_bytes,
        cancelled: true,
    }
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
pub fn get_config(state: State<AppState>) -> Result<AppConfigDto, String> {
    let _control = state.lock_config_control();
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
    // Whole-config writes and decoder publication form one serial transaction.
    // This prevents two autosave callers from publishing a decoder that no
    // longer matches the persisted TOML.
    let _config_control = state.lock_config_control();
    let path = logcore::config::default_config_path();
    let config = logcore::config::AppConfig::try_from(config)?;
    logcore::config::save_config(&path, &config).map_err(|err| err.to_string())?;
    let ActiveSessionEncodingUpdate {
        session_generation,
        changed: session_encoding_changed,
        catchup_analysis_generation: catchup,
        indexing_done,
        dataset_status,
    } = update_active_session_encoding(state.inner(), config_encoding(&config));
    if let Some(analysis_generation) = catchup {
        spawn_problem_catchup(
            state.inner().clone(),
            app.clone(),
            session_generation,
            analysis_generation,
        );
    }
    if session_encoding_changed && indexing_done {
        rerun_scans_after_index_done(state.inner(), &app, session_generation);
    }
    if let Some(status) = dataset_status {
        let _ = app.emit("index:progress", status);
    }
    Ok(AppConfigDto::from_config(config, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_progress_is_throttled_but_always_emits_terminal_state() {
        assert!(!should_emit_index_progress(
            0,
            INDEX_PROGRESS_STRIDE - 1,
            false
        ));
        assert!(should_emit_index_progress(0, INDEX_PROGRESS_STRIDE, false));
        assert!(!should_emit_index_progress(
            INDEX_PROGRESS_STRIDE,
            INDEX_PROGRESS_STRIDE * 2 - 1,
            false
        ));
        assert!(should_emit_index_progress(usize::MAX, usize::MAX, true));
    }

    fn checked_state_with_session(
        contents: &str,
    ) -> (tempfile::TempDir, AppState, AnalysisTokenDto) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("checked.log");
        std::fs::write(&source, contents).unwrap();
        let mut session = logcore::session::Session::open(&source).unwrap();
        session.index_all();
        let state = AppState::new();
        let (session_generation, analysis_generation) = state.replace_session(session);
        (
            dir,
            state,
            AnalysisTokenDto {
                session_generation,
                analysis_generation,
            },
        )
    }

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
    fn status_total_lines_uses_the_stable_frontier() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "04-20 12:06:02.125   146   179 E Crash: unfinished").unwrap();
        file.flush().unwrap();
        let mut session = logcore::session::Session::open_growing(file.path()).unwrap();
        session.index_all();

        let state = AppState::new();
        let status = status_from(&session, 7, &state);

        assert_eq!(session.total_lines(), 1);
        assert_eq!(session.stable_lines(), 0);
        assert_eq!(status.total_lines, 0);
    }

    #[test]
    fn chunked_scan_snapshot_stops_at_the_stable_frontier() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "04-20 12:06:02.125   146   179 E Crash: unfinished").unwrap();
        file.flush().unwrap();
        let mut session = logcore::session::Session::open_growing(file.path()).unwrap();
        session.index_all();

        let state = AppState::new();
        state.generation.store(1, Ordering::SeqCst);
        *state.lock_session() = Some(session);
        let scanned = std::cell::Cell::new(0);

        let matches = run_chunked_scan(
            &state,
            1,
            || true,
            |_session, _start, _end| Vec::new(),
            |end, _matches| scanned.set(end),
        )
        .unwrap();

        assert!(matches.is_empty());
        assert_eq!(scanned.get(), 0);
    }

    #[test]
    fn finalizer_transitions_pause_and_stop_without_a_live_reader() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            "04-20 12:06:02.125   146   179 E Crash: final partial"
        )
        .unwrap();
        file.flush().unwrap();
        let mut session = logcore::session::Session::open_growing(file.path()).unwrap();
        session.index_all();
        let state = AppState::new();
        *state.lock_session() = Some(session);

        transition_stream_session(&state, StreamStop::Pause).unwrap();
        publish_stream_stop(&state, StreamStop::Pause);
        {
            let guard = state.lock_session();
            let session = guard.as_ref().unwrap();
            assert_eq!(
                session.input_lifecycle(),
                Some(logcore::session::InputLifecycle::Paused)
            );
            assert_eq!(session.stable_lines(), 0);
        }

        transition_stream_session(&state, StreamStop::Stop).unwrap();
        publish_stream_stop(&state, StreamStop::Stop);
        let guard = state.lock_session();
        let session = guard.as_ref().unwrap();
        assert_eq!(
            session.input_lifecycle(),
            Some(logcore::session::InputLifecycle::Sealed)
        );
        assert_eq!(session.stable_lines(), 1);
        assert!(session.problem_analysis_finished());
    }

    #[test]
    fn resume_requires_runtime_and_session_paused_and_rejects_sealed_input() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut session = logcore::session::Session::open_growing(file.path()).unwrap();
        session.index_all();
        let state = AppState::new();
        state.generation.store(3, Ordering::SeqCst);
        *state.lock_session() = Some(session);
        {
            let mut runtime = state.lock_stream();
            runtime.last_request = Some(StreamRequestState {
                adb_path: PathBuf::from("adb"),
                requested_serial: None,
                buffers: vec!["main".to_string()],
                session_path: file.path().to_path_buf(),
                session_generation: 3,
                since_timestamp: None,
            });
        }

        assert!(prepare_paused_stream_resume(&state).is_err());
        transition_stream_session(&state, StreamStop::Pause).unwrap();
        publish_stream_stop(&state, StreamStop::Pause);
        let request = prepare_paused_stream_resume(&state).unwrap();
        assert_eq!(request.session_generation, 3);
        assert_eq!(
            state.lock_session().as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Growing)
        );

        transition_stream_session(&state, StreamStop::Stop).unwrap();
        publish_stream_stop(&state, StreamStop::Stop);
        state.lock_stream().lifecycle = StreamLifecycle::Paused; // 模拟陈旧 runtime 标志，Session 仍是权威边界。
        assert!(prepare_paused_stream_resume(&state).is_err());
        assert_eq!(
            state.lock_session().as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Sealed)
        );
    }

    #[test]
    fn stopped_stream_cannot_resume_from_retained_request() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut session = logcore::session::Session::open_growing(file.path()).unwrap();
        session.index_all();
        let state = AppState::new();
        state.generation.store(3, Ordering::SeqCst);
        *state.lock_session() = Some(session);
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::Stopped;
            runtime.last_request = Some(StreamRequestState {
                adb_path: PathBuf::from("adb"),
                requested_serial: None,
                buffers: vec!["main".to_string()],
                session_path: file.path().to_path_buf(),
                session_generation: 3,
                since_timestamp: None,
            });
        }

        assert_eq!(
            prepare_paused_stream_resume(&state).unwrap_err(),
            "no paused logcat session to resume"
        );
        assert_eq!(state.lock_stream().lifecycle, StreamLifecycle::Stopped);
        assert_eq!(
            state.lock_session().as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Growing)
        );
    }

    #[test]
    fn checked_rows_reject_old_analysis_and_filtered_revision() {
        let (_dir, state, token) =
            checked_state_with_session("04-20 12:06:02.125   146   179 E Crash: one\n");
        let request = CheckedRowsRequest {
            view: "filtered".to_string(),
            start: 0,
            count: 1,
            expected_analysis_token: token,
            expected_filter_result_revision: Some(0),
            request_nonce: 41,
        };

        state.bump_filter_result_revision();
        match get_rows_checked_for_state(request.clone(), &state).unwrap() {
            CheckedRowsDto::StaleFilterResult {
                request_nonce,
                actual_filter_result_revision,
                ..
            } => {
                assert_eq!(request_nonce, 41);
                assert_eq!(actual_filter_result_revision, 1);
            }
            CheckedRowsDto::Ok { .. } => panic!("old filtered dataset must be rejected"),
        }

        state.next_analysis_generation();
        assert_eq!(
            get_rows_checked_for_state(request, &state).err().unwrap(),
            "stale-analysis-token"
        );
    }

    #[test]
    fn source_line_mapping_rejects_old_analysis_and_filtered_revision() {
        let (_dir, state, token) =
            checked_state_with_session("04-20 12:06:02.125   146   179 E Crash: one\n");
        let request = LineMappingRequest {
            line_no: 1,
            bias: LineMappingBiasDto::Nearest,
            expected_analysis_token: token,
            expected_filter_result_revision: 0,
            request_nonce: 42,
        };

        state.bump_filter_result_revision();
        let stale = map_source_line_for_state(request.clone(), &state).unwrap();
        assert_eq!(stale.status, LineMappingStatusDto::StaleFilterResult);
        assert_eq!(stale.filter_result_revision, 1);
        assert_eq!(stale.request_nonce, 42);
        assert!(stale.target.is_none());

        state.next_analysis_generation();
        assert_eq!(
            map_source_line_for_state(request, &state).err().unwrap(),
            "stale-analysis-token"
        );
    }

    #[test]
    fn encoding_change_invalidates_old_analysis_and_decoded_datasets_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("encoding.log");
        std::fs::write(
            &source,
            "04-20 12:06:02.125   146   179 E Crash: decoded match\n",
        )
        .unwrap();
        let mut session = logcore::session::Session::open(&source).unwrap();
        session.index_all();
        let filter = logcore::filter::FilterSpec {
            word_include: logcore::filter::FilterField::plain(true, "decoded"),
            ..Default::default()
        };
        session.set_filter(&filter).unwrap();
        session
            .search(&logcore::search::SearchSpec::plain("decoded"))
            .unwrap();

        let state = AppState::new();
        let (session_generation, analysis_generation) = state.replace_session(session);
        state.filter_input_revision.store(7, Ordering::SeqCst);
        state
            .applied_filter_input_revision
            .store(7, Ordering::SeqCst);
        state.filter_result_revision.store(3, Ordering::SeqCst);
        state.search_input_revision.store(9, Ordering::SeqCst);
        state
            .applied_search_input_revision
            .store(9, Ordering::SeqCst);
        state.decode_revision.store(4, Ordering::SeqCst);

        let update = update_active_session_encoding(&state, logcore::encoding::TextEncoding::Local);

        assert!(update.changed);
        assert!(update.indexing_done);
        assert_eq!(update.session_generation, session_generation);
        assert_eq!(
            update.catchup_analysis_generation,
            Some(analysis_generation + 1)
        );
        assert!(state
            .lock_analysis_if_current(session_generation, analysis_generation)
            .is_none());
        assert!(state
            .lock_analysis_if_current(session_generation, analysis_generation + 1)
            .is_some());
        assert_eq!(state.decode_revision.load(Ordering::SeqCst), 5);
        assert_eq!(state.filter_input_revision.load(Ordering::SeqCst), 7);
        assert_eq!(
            state.applied_filter_input_revision.load(Ordering::SeqCst),
            0
        );
        assert_eq!(state.filter_result_revision.load(Ordering::SeqCst), 4);
        assert_eq!(state.search_input_revision.load(Ordering::SeqCst), 9);
        assert_eq!(
            state.applied_search_input_revision.load(Ordering::SeqCst),
            0
        );
        let guard = state.lock_session();
        let session = guard.as_ref().unwrap();
        assert_eq!(session.filtered_count(), 0);
        assert!(session.desired_filter_spec().is_some());
        assert!(session.desired_search_spec().is_some());
    }

    #[test]
    fn pause_without_a_reader_does_not_cancel_natural_eof_finalization() {
        let state = AppState::new();
        state.stream_generation.store(7, Ordering::SeqCst);

        assert_eq!(
            stop_stream_task(&state, StreamStop::Pause),
            Err("no running logcat session to pause".to_string())
        );
        assert_eq!(state.stream_generation.load(Ordering::SeqCst), 7);
        assert_eq!(state.lock_stream().lifecycle, StreamLifecycle::Stopped);
    }

    #[test]
    fn read_error_after_stream_generation_cancel_is_not_a_transport_failure() {
        let cancelled = classify_stream_read(
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
            false,
        )
        .unwrap();
        assert_eq!(cancelled, StreamReadOutcome::ControlledCancellation);

        let active = classify_stream_read(
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
            true,
        );
        assert!(active.is_err());
    }

    #[test]
    fn control_error_requires_a_new_input_identity_instead_of_a_fake_stop() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut session = logcore::session::Session::open_growing(file.path()).unwrap();
        session.index_all();
        let state = AppState::new();
        *state.lock_session() = Some(session);
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::ControlError;
            runtime.control_error = Some("reader failed".to_string());
            runtime.eof_confirmed = true;
        }

        assert_eq!(
            stop_stream_task(&state, StreamStop::Stop),
            Err("reader failed".to_string())
        );
        assert_eq!(
            state.lock_session().as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Growing)
        );

        assert_eq!(stop_stream_task(&state, StreamStop::Forget), Ok(None));
        assert_eq!(
            state.lock_session().as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Growing)
        );
        assert_eq!(state.lock_stream().lifecycle, StreamLifecycle::Stopped);
    }

    #[test]
    fn parses_logcat_buffers_and_rejects_unknown_values() {
        let buffers =
            parse_buffers(&["main".to_string(), "crash".to_string(), "main".to_string()]).unwrap();
        assert_eq!(
            buffers,
            vec![
                logcore::adb::LogcatBuffer::Main,
                logcore::adb::LogcatBuffer::Crash
            ]
        );
        let coverage = problem_buffer_set(&buffers);
        assert!(coverage.contains(logcore::problems::LogBuffer::Main));
        assert!(coverage.contains(logcore::problems::LogBuffer::Crash));
        assert!(!coverage.contains(logcore::problems::LogBuffer::Events));
        assert_eq!(
            parse_buffers(&[]).unwrap(),
            vec![logcore::adb::LogcatBuffer::Main]
        );
        assert!(parse_buffers(&["kernel".to_string()]).is_err());
    }

    #[test]
    fn parses_repeated_buffers_from_logcat_command_in_request() {
        let config = logcore::config::AppConfig::default();
        let request = StartLogcatRequest {
            device_serial: None,
            command: Some(
                "logcat -v threadtime -b main -b system -b crash -b main -b events".to_string(),
            ),
            buffers: vec!["radio".to_string()],
        };

        let buffers = parse_logcat_request_buffers(&config, &request).unwrap();
        assert_eq!(
            buffers,
            vec![
                logcore::adb::LogcatBuffer::Main,
                logcore::adb::LogcatBuffer::System,
                logcore::adb::LogcatBuffer::Crash,
                logcore::adb::LogcatBuffer::Events,
            ]
        );
        let coverage = problem_buffer_set(&buffers);
        assert!(coverage.contains(logcore::problems::LogBuffer::Main));
        assert!(coverage.contains(logcore::problems::LogBuffer::System));
        assert!(coverage.contains(logcore::problems::LogBuffer::Crash));
        assert!(coverage.contains(logcore::problems::LogBuffer::Events));
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
        let export_generation = state.next_export_generation();
        let mut progress_calls = Vec::new();
        let summary = run_chunked_export(
            &state,
            generation,
            export_generation,
            &request,
            &mut |lines, bytes, done| {
                progress_calls.push((lines, bytes, done));
            },
        )
        .unwrap();

        let expected = std::fs::read(&expected_path).unwrap();
        let actual = std::fs::read(&out_path).unwrap();
        assert_eq!(actual, expected);
        assert!(!summary.cancelled);
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
        let export_generation = state.next_export_generation();
        let mut progress_calls = Vec::new();
        let summary = run_chunked_export(
            &state,
            generation,
            export_generation,
            &request,
            &mut |lines, bytes, done| {
                progress_calls.push((lines, bytes, done));
            },
        )
        .unwrap();

        // 9000 行 / 4096 每批 = 3 个写批次;输出必须与 oracle 完全一致
        assert_eq!(
            std::fs::read(&out_path).unwrap(),
            std::fs::read(&expected_path).unwrap()
        );
        assert!(!summary.cancelled);
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
        let export_generation = state.next_export_generation();
        let summary = run_chunked_export(
            &state,
            generation,
            export_generation,
            &request,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&out_path).unwrap(),
            std::fs::read(&expected_path).unwrap()
        );
        assert!(!summary.cancelled);
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
        let export_generation = state.next_export_generation();
        let err = run_chunked_export(
            &state,
            generation,
            export_generation,
            &request,
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert!(err.contains("session changed"), "{err}");
    }

    #[test]
    fn problem_export_aborts_and_removes_partial_output_when_analysis_changes() {
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
        let mut session = logcore::session::Session::open(&src).unwrap();
        session.index_all();
        let state = AppState::new();
        let (session_generation, analysis_generation) = state.replace_session(session);
        let token = AnalysisTokenDto {
            session_generation,
            analysis_generation,
        };
        let export_generation = state.next_export_generation();
        let out_path = dir.path().join("problem-context.log");
        let request = ExportRequest {
            mode: "range".to_string(),
            view: None,
            start_line: Some(1),
            end_line: Some(9000),
            path: out_path.to_string_lossy().to_string(),
        };
        let mut changed = false;

        let error = run_chunked_export_for_analysis(
            &state,
            token,
            export_generation,
            &request,
            &mut |_, _, _| {
                if !changed {
                    changed = true;
                    state.next_analysis_generation();
                }
            },
        )
        .unwrap_err();

        assert_eq!(error, "analysis changed during export");
        assert!(
            !out_path.exists(),
            "stale Problems exports must not leave a partial file"
        );
    }

    #[test]
    fn cancelled_export_deletes_partial_output_and_reports_cancelled() {
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

        let (state, generation) = export_state_with_session(&src);
        let export_generation = state.next_export_generation();
        let out_path = dir.path().join("chunked.log");
        let request = ExportRequest {
            mode: "view".to_string(),
            view: Some("all".to_string()),
            start_line: None,
            end_line: None,
            path: out_path.to_string_lossy().to_string(),
        };

        // 首次进度回调即模拟用户取消(递增导出代号),后续批次应检测到并中止。
        let mut fired = false;
        let summary = run_chunked_export(
            &state,
            generation,
            export_generation,
            &request,
            &mut |_, _, _| {
                if !fired {
                    fired = true;
                    state.next_export_generation();
                }
            },
        )
        .unwrap();

        assert!(summary.cancelled, "expected cancelled summary");
        assert!(
            !out_path.exists(),
            "cancelled export must delete the partial output file"
        );
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
        let analysis_before = state.current_analysis_generation();

        let generation = reset_stream_session_file(
            &state,
            &path,
            logcore::encoding::TextEncoding::Utf8,
            logcore::problems::BufferSet::MAIN,
        )
        .unwrap();

        assert_eq!(generation, before + 1);
        assert_eq!(
            state.current_analysis_generation(),
            analysis_before + 1,
            "new input must publish a new analysis identity"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert!(!logcore::bookmarks::sidecar_path_for(&path).exists());
        let guard = state.lock_session();
        assert_eq!(guard.as_ref().unwrap().total_lines(), 0);
        assert_eq!(
            guard.as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Growing)
        );
    }

    #[test]
    fn clear_plan_restarts_a_running_capture_and_preserves_its_request() {
        let state = AppState::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logcat-1.log");
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::Running;
            runtime.last_request = Some(StreamRequestState {
                adb_path: PathBuf::from("adb"),
                requested_serial: Some("test-device".to_string()),
                buffers: vec!["main".to_string(), "crash".to_string()],
                session_path: path.clone(),
                session_generation: 7,
                since_timestamp: None,
            });
        }

        let plan = plan_stream_clear(&state).unwrap().unwrap();
        assert_eq!(plan.stop_mode, StreamStop::Forget);
        assert_eq!(plan.session_path, path);
        assert!(plan
            .problem_buffers
            .contains(logcore::problems::LogBuffer::Main));
        assert!(plan
            .problem_buffers
            .contains(logcore::problems::LogBuffer::Crash));
        let restart = plan.restart_request.unwrap();
        assert_eq!(restart.adb_path, PathBuf::from("adb"));
        assert_eq!(restart.requested_serial.as_deref(), Some("test-device"));
        assert_eq!(restart.buffers, vec!["main", "crash"]);
        assert_eq!(restart.session_generation, 7);
    }

    #[test]
    fn clear_plan_keeps_a_non_running_capture_stopped() {
        let state = AppState::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logcat-1.log");
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::Stopped;
            runtime.last_request = Some(StreamRequestState {
                adb_path: PathBuf::from("adb"),
                requested_serial: None,
                buffers: vec!["main".to_string()],
                session_path: path.clone(),
                session_generation: 4,
                since_timestamp: None,
            });
        }

        let plan = plan_stream_clear(&state).unwrap().unwrap();
        assert_eq!(plan.stop_mode, StreamStop::Stop);
        assert_eq!(plan.session_path, path);
        assert!(plan.restart_request.is_none());
    }

    #[test]
    fn clear_plan_can_recover_a_transport_control_error_without_restarting() {
        let state = AppState::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logcat-1.log");
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::ControlError;
            runtime.control_error = Some("synthetic transport failure".to_string());
            runtime.last_request = Some(StreamRequestState {
                adb_path: PathBuf::from("adb"),
                requested_serial: None,
                buffers: vec!["main".to_string(), "crash".to_string()],
                session_path: path,
                session_generation: 4,
                since_timestamp: None,
            });
        }

        let plan = plan_stream_clear(&state).unwrap().unwrap();
        assert_eq!(plan.stop_mode, StreamStop::Forget);
        assert!(plan.restart_request.is_none());
    }

    #[test]
    fn cleared_non_running_session_is_sealed_and_reports_a_finished_input() {
        let state = AppState::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logcat-1.log");
        let session_generation = reset_stream_session_file(
            &state,
            &path,
            logcore::encoding::TextEncoding::Utf8,
            logcore::problems::BufferSet::MAIN,
        )
        .unwrap();
        {
            let mut runtime = state.lock_stream();
            runtime.lifecycle = StreamLifecycle::Stopped;
            runtime.last_request = Some(StreamRequestState {
                adb_path: PathBuf::from("adb"),
                requested_serial: None,
                buffers: vec!["main".to_string()],
                session_path: path,
                session_generation: 1,
                since_timestamp: None,
            });
        }

        settle_cleared_stream_without_restart(&state, session_generation).unwrap();

        let guard = state.lock_session();
        assert_eq!(
            guard.as_ref().unwrap().input_lifecycle(),
            Some(logcore::session::InputLifecycle::Sealed)
        );
        assert!(guard.as_ref().unwrap().problem_analysis_finished());
        drop(guard);
        let runtime = state.lock_stream();
        assert_eq!(runtime.lifecycle, StreamLifecycle::Stopped);
        assert_eq!(
            runtime.last_request.as_ref().unwrap().session_generation,
            session_generation
        );
    }

    #[test]
    fn clear_restart_request_uses_the_new_session_and_tail_timestamp() {
        let request = StreamRequestState {
            adb_path: PathBuf::from("adb"),
            requested_serial: Some("test-device".to_string()),
            buffers: vec!["main".to_string(), "system".to_string()],
            session_path: PathBuf::from("capture.log"),
            session_generation: 2,
            since_timestamp: Some("04-20 12:05:00.000".to_string()),
        };

        let request =
            retarget_cleared_stream_request(request, 9, Some("04-20 12:06:02.125".to_string()));

        assert_eq!(request.session_generation, 9);
        assert_eq!(
            request.since_timestamp.as_deref(),
            Some("04-20 12:06:02.125")
        );
        assert_eq!(request.requested_serial.as_deref(), Some("test-device"));
        assert_eq!(request.buffers, vec!["main", "system"]);
    }
}
