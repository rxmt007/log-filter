use logcore::session::Session;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, MutexGuard};

/// 全局应用状态:当前会话 + 打开代号(generation)。
/// 代号用于让被后续 open 取代的旧索引线程自行退出。
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
    pub generation: Arc<AtomicU64>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            generation: Arc::new(AtomicU64::new(0)),
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
}
