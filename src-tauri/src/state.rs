use logcore::session::Session;
use std::sync::{Arc, Mutex};

/// 全局应用状态:当前打开的会话(可为空)。用 Arc 便于后台索引线程共享。
#[derive(Clone)]
pub struct AppState {
    pub session: Arc<Mutex<Option<Session>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
