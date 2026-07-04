use crate::model::LogEntry;

/// 返回跳过前 n 个空白分隔 token 后的剩余子串(保留其内部原始间隔)。
fn rest_after_tokens(line: &str, n: usize) -> Option<&str> {
    let mut rest = line.trim_start();
    for _ in 0..n {
        let ws = rest.find(char::is_whitespace)?;
        rest = rest[ws..].trim_start();
    }
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
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
    let (tag, message) = if let Some(colon) = tail.find(':') {
        (
            tail[..colon].to_string(),
            tail[colon + 1..].trim_start().to_string(),
        )
    } else if let Some(ws) = tail.find(char::is_whitespace) {
        (
            tail[..ws].to_string(),
            tail[ws..].trim_start().to_string(),
        )
    } else {
        (tail.to_string(), String::new())
    };
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

/// `MM-DD HH:MM:SS.mmm L/Tag(  pid): message`
pub fn parse_time(line: &str) -> Option<LogEntry> {
    let mut it = line.split_whitespace();
    let date = it.next()?;
    let time = it.next()?;
    let rest = rest_after_tokens(line, 2)?; // "D/LightsService(  139): BKL : 106"
    // 用 char 边界安全地取"级别 + 斜杠",避免多字节(中文/emoji)行 byte 切片 panic。
    let mut chars = rest.char_indices();
    let (_, level_ch) = chars.next()?;
    if !matches!(level_ch, 'V' | 'D' | 'I' | 'W' | 'E' | 'F') {
        return None;
    }
    let (slash_idx, slash_ch) = chars.next()?;
    if slash_ch != '/' {
        return None;
    }
    let after = &rest[slash_idx + 1..]; // 斜杠为 ASCII,+1 是 char 边界
    let open = after.find('(')?;
    let close = after.find(')')?;
    if close < open {
        return None;
    }
    let tag = after[..open].to_string();
    let pid = after[open + 1..close].trim().to_string();
    let message = after[close + 1..]
        .trim_start_matches(':')
        .trim_start()
        .to_string();
    Some(LogEntry {
        date: date.to_string(),
        time: time.to_string(),
        level: level_ch.to_string(),
        pid,
        tid: String::new(),
        tag,
        message,
    })
}

/// 依次尝试 threadtime → time,失败则整行作为 message。
pub fn parse_line(line: &str) -> LogEntry {
    let line = line.trim_end_matches(['\r', '\n']);
    parse_threadtime(line)
        .or_else(|| parse_time(line))
        .unwrap_or_else(|| LogEntry {
            message: line.to_string(),
            ..Default::default()
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
    fn parses_threadtime_without_colon() {
        let line = "04-20 12:06:02.125   146   179 E NoColonTag message without delimiter";
        let e = parse_threadtime(line).expect("should parse threadtime fields");
        assert_eq!(e.date, "04-20");
        assert_eq!(e.time, "12:06:02.125");
        assert_eq!(e.pid, "146");
        assert_eq!(e.tid, "179");
        assert_eq!(e.level, "E");
        assert_eq!(e.tag, "NoColonTag");
        assert_eq!(e.message, "message without delimiter");
    }

    #[test]
    fn rejects_non_threadtime() {
        let line = "04-17 09:01:18.910 D/LightsService(  139): BKL : 106";
        assert!(parse_threadtime(line).is_none());
    }

    #[test]
    fn parses_time_line() {
        let line = "04-17 09:01:18.910 D/LightsService(  139): BKL : 106";
        let e = parse_time(line).expect("should parse");
        assert_eq!(e.date, "04-17");
        assert_eq!(e.time, "09:01:18.910");
        assert_eq!(e.level, "D");
        assert_eq!(e.tag, "LightsService");
        assert_eq!(e.pid, "139");
        assert_eq!(e.tid, "");
        assert_eq!(e.message, "BKL : 106");
    }

    #[test]
    fn parse_line_dispatches_and_falls_back() {
        let tt = parse_line("04-20 12:06:02.125   146   179 D BatteryService: update start");
        assert_eq!(tt.tag, "BatteryService");
        let tm = parse_line("04-17 09:01:18.910 D/LightsService(  139): BKL : 106");
        assert_eq!(tm.tag, "LightsService");
        let raw = parse_line("--------- beginning of main");
        assert_eq!(raw.message, "--------- beginning of main");
        assert_eq!(raw.tag, "");
    }

    #[test]
    fn multibyte_line_does_not_panic() {
        // 时间戳后紧跟多字节字符(中文),旧实现会 byte 切片 panic;必须安全回退。
        let e = parse_line("01-01 00:00:00.000 中文消息 hello");
        assert_eq!(e.message, "01-01 00:00:00.000 中文消息 hello");
        // 级别位处即为多字节字符,也不能 panic。
        let _ = parse_line("01-01 00:00:00.000 中/x(1): y");
    }
}
