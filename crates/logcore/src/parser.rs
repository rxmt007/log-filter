use crate::model::LogEntry;

/// 返回跳过前 n 个空白分隔 token 后的剩余子串(保留其内部原始间隔)。
fn rest_after_tokens(line: &str, n: usize) -> Option<&str> {
    let mut rest = line.trim_start();
    for _ in 0..n {
        let ws = rest.find(char::is_whitespace)?;
        rest = rest[ws..].trim_start();
    }
    if rest.is_empty() { None } else { Some(rest) }
}

fn is_all_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `MM-DD HH:MM:SS.mmm  PID  TID L Tag: message`
pub fn parse_threadtime(line: &str) -> Option<LogEntry> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.len() < 6 {
        return None;
    }
    let (date, time, pid, tid, level) = (toks[0], toks[1], toks[2], toks[3], toks[4]);
    if !is_all_ascii_digits(pid) || !is_all_ascii_digits(tid) {
        return None;
    }
    if level.len() != 1 || !"VDIWEF".contains(level) {
        return None;
    }
    // tag+message 部分,保留原始间隔:跳过前 5 个 token
    let tail = rest_after_tokens(line, 5)?;
    let colon = tail.find(':')?;
    let tag = tail[..colon].to_string();
    let message = tail[colon + 1..].trim_start().to_string();
    Some(LogEntry {
        date: date.to_string(),
        time: time.to_string(),
        level: level.to_string(),
        pid: pid.to_string(),
        tid: tid.to_string(),
        tag,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_threadtime_line() {
        let line = "04-20 12:06:02.125   146   179 D BatteryService: update start";
        let e = parse_threadtime(line).expect("should parse");
        assert_eq!(e.date, "04-20");
        assert_eq!(e.time, "12:06:02.125");
        assert_eq!(e.pid, "146");
        assert_eq!(e.tid, "179");
        assert_eq!(e.level, "D");
        assert_eq!(e.tag, "BatteryService");
        assert_eq!(e.message, "update start");
    }

    #[test]
    fn rejects_non_threadtime() {
        let line = "04-17 09:01:18.910 D/LightsService(  139): BKL : 106";
        assert!(parse_threadtime(line).is_none());
    }
}
