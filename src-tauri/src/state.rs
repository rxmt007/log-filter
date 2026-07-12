use logcore::session::Session;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// 全局应用状态:当前会话 + 打开代号(generation)。
/// 代号用于让被后续 open 取代的旧索引线程自行退出。
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
    pub generation: Arc<AtomicU64>,
    pub filter_task_generation: Arc<AtomicU64>,
    pub search_task_generation: Arc<AtomicU64>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
            filter_task_generation: Arc::new(AtomicU64::new(0)),
            search_task_generation: Arc::new(AtomicU64::new(0)),
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
}
