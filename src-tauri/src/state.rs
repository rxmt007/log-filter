use logcore::session::Session;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

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
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
