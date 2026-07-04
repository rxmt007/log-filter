#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn from_char(c: char) -> Option<LogLevel> {
        match c {
            'V' => Some(LogLevel::Verbose),
            'D' => Some(LogLevel::Debug),
            'I' => Some(LogLevel::Info),
            'W' => Some(LogLevel::Warn),
            'E' => Some(LogLevel::Error),
            'F' => Some(LogLevel::Fatal),
            _ => None,
        }
    }
}

/// 一条日志的解析结果。行号由 session 赋值,不在此结构里。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogEntry {
    pub date: String,
    pub time: String,
    pub level: String,
    pub pid: String,
    pub tid: String,
    pub tag: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_level_from_char_maps_known() {
        assert_eq!(LogLevel::from_char('E'), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_char('V'), Some(LogLevel::Verbose));
        assert_eq!(LogLevel::from_char('X'), None);
    }

    #[test]
    fn log_entry_default_is_empty() {
        let e = LogEntry::default();
        assert_eq!(e.message, "");
        assert_eq!(e.level, "");
    }
}
