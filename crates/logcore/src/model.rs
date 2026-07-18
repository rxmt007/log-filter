use crate::parser::ParsedLine;

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

impl LogEntry {
    /// 借用视图:零拷贝地把 owned 字段转成 `ParsedLine`,供匹配器统一走借用式路径。
    pub fn as_parsed(&self) -> ParsedLine<'_> {
        ParsedLine {
            date: &self.date,
            time: &self.time,
            level: &self.level,
            pid: &self.pid,
            tid: &self.tid,
            tag: &self.tag,
            message: &self.message,
        }
    }
}

impl From<ParsedLine<'_>> for LogEntry {
    fn from(parsed: ParsedLine<'_>) -> Self {
        LogEntry {
            date: parsed.date.to_string(),
            time: parsed.time.to_string(),
            level: parsed.level.to_string(),
            pid: parsed.pid.to_string(),
            tid: parsed.tid.to_string(),
            tag: parsed.tag.to_string(),
            message: parsed.message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_entry_default_is_empty() {
        let e = LogEntry::default();
        assert_eq!(e.message, "");
        assert_eq!(e.level, "");
    }
}
