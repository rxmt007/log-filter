use logcore::session::Session;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

/// 全局应用状态:当前会话 + 打开代号(generation)。
/// 代号用于让被后续 open 取代的旧索引线程自行退出。
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
    pub generation: Arc<AtomicU64>,
    pub analysis_generation: Arc<AtomicU64>,
    pub filter_task_generation: Arc<AtomicU64>,
    pub search_task_generation: Arc<AtomicU64>,
    pub export_task_generation: Arc<AtomicU64>,
    pub stream_generation: Arc<AtomicU64>,
    pub stream: Arc<Mutex<StreamRuntime>>,
    pub stream_control: Arc<Mutex<()>>,
}

#[derive(Default)]
pub struct StreamRuntime {
    pub task: Option<StreamTask>,
    pub last_request: Option<StreamRequestState>,
    pub paused: bool,
}

pub struct StreamTask {
    pub generation: u64,
    pub child: Arc<Mutex<Child>>,
    pub handle: JoinHandle<()>,
    pub serial: String,
}

#[derive(Debug, Clone)]
pub struct StreamRequestState {
    pub adb_path: PathBuf,
    pub requested_serial: Option<String>,
    pub buffers: Vec<String>,
    pub session_path: PathBuf,
    pub session_generation: u64,
    pub since_timestamp: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            analysis_generation: Arc::new(AtomicU64::new(0)),
            filter_task_generation: Arc::new(AtomicU64::new(0)),
            search_task_generation: Arc::new(AtomicU64::new(0)),
            export_task_generation: Arc::new(AtomicU64::new(0)),
            stream_generation: Arc::new(AtomicU64::new(0)),
            stream: Arc::new(Mutex::new(StreamRuntime::default())),
            stream_control: Arc::new(Mutex::new(())),
        }
    }

    pub fn lock_session(&self) -> MutexGuard<'_, Option<Session>> {
        Self::lock_session_arc(&self.session)
    }

    pub fn lock_session_arc(
        session: &Arc<Mutex<Option<Session>>>,
    ) -> MutexGuard<'_, Option<Session>> {
        match session.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn next_filter_task_generation(&self) -> u64 {
        self.filter_task_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_filter_task(&self, task_generation: u64) -> bool {
        self.filter_task_generation.load(Ordering::SeqCst) == task_generation
    }

    pub fn next_search_task_generation(&self) -> u64 {
        self.search_task_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_search_task(&self, task_generation: u64) -> bool {
        self.search_task_generation.load(Ordering::SeqCst) == task_generation
    }

    pub fn next_export_generation(&self) -> u64 {
        self.export_task_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_export(&self, generation: u64) -> bool {
        self.export_task_generation.load(Ordering::SeqCst) == generation
    }

    pub fn lock_stream(&self) -> MutexGuard<'_, StreamRuntime> {
        match self.stream.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn lock_stream_control(&self) -> MutexGuard<'_, ()> {
        match self.stream_control.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn next_analysis_generation(&self) -> u64 {
        self.analysis_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_analysis_generation(&self) -> u64 {
        self.analysis_generation.load(Ordering::SeqCst)
    }

    pub fn next_stream_generation(&self) -> u64 {
        self.stream_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn is_current_stream_task(&self, task_generation: u64) -> bool {
        self.stream_generation.load(Ordering::SeqCst) == task_generation
    }

    /// 持锁校验会话代号。依赖写侧顺序"先递增 generation、后替换 session"
    /// (open_file / start_logcat / reset_stream_session_file),
    /// 因此持锁后代号仍匹配 ⇒ guard 里就是该代号对应的会话。
    pub fn lock_session_if_current(
        &self,
        session_generation: u64,
    ) -> Option<MutexGuard<'_, Option<Session>>> {
        let guard = self.lock_session();
        if self.generation.load(Ordering::SeqCst) == session_generation {
            Some(guard)
        } else {
            None
        }
    }

    pub fn lock_analysis_if_current(
        &self,
        session_generation: u64,
        analysis_generation: u64,
    ) -> Option<MutexGuard<'_, Option<Session>>> {
        let guard = self.lock_session();
        if self.generation.load(Ordering::SeqCst) == session_generation
            && self.analysis_generation.load(Ordering::SeqCst) == analysis_generation
        {
            Some(guard)
        } else {
            None
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_poisoned_session_lock() {
        let state = AppState::new();
        let session = state.session.clone();
        let _ = std::thread::spawn(move || {
            let _guard = session.lock().unwrap();
            panic!("poison session lock for regression test");
        })
        .join();

        let guard = state.lock_session();
        assert!(guard.is_none());
    }

    #[test]
    fn scan_task_generations_cancel_stale_filter_and_search_work() {
        let state = AppState::new();

        let first_filter = state.next_filter_task_generation();
        let second_filter = state.next_filter_task_generation();
        assert!(!state.is_current_filter_task(first_filter));
        assert!(state.is_current_filter_task(second_filter));

        let first_search = state.next_search_task_generation();
        let second_search = state.next_search_task_generation();
        assert!(!state.is_current_search_task(first_search));
        assert!(state.is_current_search_task(second_search));
    }

    #[test]
    fn export_generations_cancel_stale_export_work() {
        let state = AppState::new();

        let first_export = state.next_export_generation();
        let second_export = state.next_export_generation();
        assert!(!state.is_current_export(first_export));
        assert!(state.is_current_export(second_export));
    }

    #[test]
    fn stream_task_generation_cancels_stale_work() {
        let state = AppState::new();

        let first = state.next_stream_generation();
        let second = state.next_stream_generation();

        assert!(!state.is_current_stream_task(first));
        assert!(state.is_current_stream_task(second));
    }

    #[test]
    fn lock_session_if_current_rejects_stale_generation() {
        let state = AppState::new();
        let first = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(state.lock_session_if_current(first).is_some());

        let second = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(state.lock_session_if_current(first).is_none());
        assert!(state.lock_session_if_current(second).is_some());
    }

    #[test]
    fn analysis_lock_requires_both_session_and_analysis_generations() {
        let state = AppState::new();
        let session = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let first = state.next_analysis_generation();
        assert!(state.lock_analysis_if_current(session, first).is_some());

        let second = state.next_analysis_generation();
        assert!(state.lock_analysis_if_current(session, first).is_none());
        assert!(state.lock_analysis_if_current(session, second).is_some());

        let replacement = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(state.lock_analysis_if_current(session, second).is_none());
        assert!(state
            .lock_analysis_if_current(replacement, second)
            .is_some());
    }
}
