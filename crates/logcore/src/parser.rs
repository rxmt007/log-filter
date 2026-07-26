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

/// 借用式解析结果:七个字段全为源文本切片,过滤/搜索热路径零堆分配。
/// owned `LogEntry` 仅在 `get_rows` → IPC 边界处由 `From<ParsedLine>` 生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParsedLine<'a> {
    pub date: &'a str,
    pub time: &'a str,
    pub level: &'a str,
    pub pid: &'a str,
    pub tid: &'a str,
    pub tag: &'a str,
    pub message: &'a str,
}

/// `MM-DD HH:MM:SS.mmm  PID  TID L Tag: message`(借用式,禁止 collect)
pub fn parse_threadtime_ref(line: &str) -> Option<ParsedLine<'_>> {
    let mut tokens = line.split_whitespace();
    let date = tokens.next()?;
    let time = tokens.next()?;
    let pid = tokens.next()?;
    let tid = tokens.next()?;
    let level = tokens.next()?;
    if !is_all_ascii_digits(pid) || !is_all_ascii_digits(tid) {
        return None;
    }
    if level.len() != 1 || !"VDIWEF".contains(level) {
        return None;
    }
    let tail = rest_after_tokens(line, 5)?;
    let (tag, message) = if let Some(colon) = tail.find(':') {
        // 真机 threadtime 会把短 tag 填充到固定宽度(如 `vold    :`),去掉尾部填充。
        (tail[..colon].trim_end(), tail[colon + 1..].trim_start())
    } else if let Some(ws) = tail.find(char::is_whitespace) {
        (&tail[..ws], tail[ws..].trim_start())
    } else {
        (tail, "")
    };
    Some(ParsedLine {
        date,
        time,
        level,
        pid,
        tid,
        tag,
        message,
    })
}

/// `MM-DD HH:MM:SS.mmm L/Tag(  pid): message`(借用式)
pub fn parse_time_ref(line: &str) -> Option<ParsedLine<'_>> {
    let mut it = line.split_whitespace();
    let date = it.next()?;
    let time = it.next()?;
    // Example tail: "D/LightsService(  139): BKL : 106"
    let rest = rest_after_tokens(line, 2)?;
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
    // 级别字符为 ASCII,slash_idx 即其后单字节切片终点。
    let level = &rest[..slash_idx];
    let after = &rest[slash_idx + 1..]; // 斜杠为 ASCII,+1 是 char 边界
    let open = after.find('(')?;
    let close = after.find(')')?;
    if close < open {
        return None;
    }
    let tag = &after[..open];
    let pid = after[open + 1..close].trim();
    let message = after[close + 1..].trim_start_matches(':').trim_start();
    Some(ParsedLine {
        date,
        time,
        level,
        pid,
        tid: "",
        tag,
        message,
    })
}

/// 依次尝试 threadtime → time,失败则整行作为 message。(借用式)
pub fn parse_line_ref(line: &str) -> ParsedLine<'_> {
    let line = line.trim_end_matches(['\r', '\n']);
    parse_threadtime_ref(line)
        .or_else(|| parse_time_ref(line))
        .unwrap_or(ParsedLine {
            message: line,
            ..Default::default()
        })
}

/// `MM-DD HH:MM:SS.mmm  PID  TID L Tag: message`
pub fn parse_threadtime(line: &str) -> Option<LogEntry> {
    parse_threadtime_ref(line).map(Into::into)
}

/// `MM-DD HH:MM:SS.mmm L/Tag(  pid): message`
pub fn parse_time(line: &str) -> Option<LogEntry> {
    parse_time_ref(line).map(Into::into)
}

/// 依次尝试 threadtime → time,失败则整行作为 message。
pub fn parse_line(line: &str) -> LogEntry {
    LogEntry::from(parse_line_ref(line))
}

/// 零分配地判断一行的日志级别字节(b'V'..b'F'),语义与 parse_line(...).level 一致。
/// 仅供索引期错误行扫描等热路径使用;以 ASCII 空白分词,对 ASCII 兼容编码有效。
pub fn level_byte_of_line(line: &[u8]) -> Option<u8> {
    threadtime_level_byte(line).or_else(|| time_level_byte(line))
}

/// 零分配借用 ASCII tag,用于在解码与完整字段解析前做保守候选预筛。
///
/// 仅返回与 `parse_line_ref` 同样成立的 threadtime/time envelope；非 ASCII tag
/// 返回 `None`,让调用方回退到完整解析而不是猜测。
pub fn tag_bytes_of_line(line: &[u8]) -> Option<&[u8]> {
    ascii_log_envelope(line).map(|envelope| envelope.tag)
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AsciiLogEnvelope<'a> {
    pub(crate) date: &'a [u8],
    pub(crate) time: &'a [u8],
    pub(crate) tag: &'a [u8],
}

pub(crate) fn ascii_log_envelope(line: &[u8]) -> Option<AsciiLogEnvelope<'_>> {
    threadtime_ascii_envelope(line).or_else(|| time_ascii_envelope(line))
}

fn ascii_tokens(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|b| b.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
}

fn threadtime_ascii_envelope(line: &[u8]) -> Option<AsciiLogEnvelope<'_>> {
    let mut cursor = 0;
    let date = next_ascii_token(line, &mut cursor)?;
    let time = next_ascii_token(line, &mut cursor)?;
    let pid = next_ascii_token(line, &mut cursor)?;
    let tid = next_ascii_token(line, &mut cursor)?;
    let level = next_ascii_token(line, &mut cursor)?;
    if !pid.iter().all(u8::is_ascii_digit)
        || !tid.iter().all(u8::is_ascii_digit)
        || level.len() != 1
        || !b"VDIWEF".contains(&level[0])
    {
        return None;
    }
    let tail = trim_ascii_start(&line[cursor..]);
    if tail.is_empty() {
        return None;
    }
    let tag = if let Some(colon) = memchr::memchr(b':', tail) {
        trim_ascii_end(&tail[..colon])
    } else if let Some(ws) = tail.iter().position(|byte| byte.is_ascii_whitespace()) {
        &tail[..ws]
    } else {
        tail
    };
    (!tag.is_empty() && tag.is_ascii()).then_some(AsciiLogEnvelope { date, time, tag })
}

fn threadtime_level_byte(line: &[u8]) -> Option<u8> {
    let mut tokens = ascii_tokens(line);
    let _date = tokens.next()?;
    let _time = tokens.next()?;
    let pid = tokens.next()?;
    let tid = tokens.next()?;
    let level = tokens.next()?;
    let _tail = tokens.next()?; // 与 parse_threadtime 一致:至少 6 个 token
    if !pid.iter().all(u8::is_ascii_digit) || !tid.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if level.len() != 1 || !b"VDIWEF".contains(&level[0]) {
        return None;
    }
    Some(level[0])
}

fn time_ascii_envelope(line: &[u8]) -> Option<AsciiLogEnvelope<'_>> {
    let mut cursor = 0;
    let date = next_ascii_token(line, &mut cursor)?;
    let time = next_ascii_token(line, &mut cursor)?;
    let rest = trim_ascii_start(&line[cursor..]);
    let level = *rest.first()?;
    if !b"VDIWEF".contains(&level) || rest.get(1) != Some(&b'/') {
        return None;
    }
    let after = &rest[2..];
    let open = memchr::memchr(b'(', after)?;
    let close = memchr::memchr(b')', after)?;
    if close < open {
        return None;
    }
    let tag = &after[..open];
    (!tag.is_empty() && tag.is_ascii()).then_some(AsciiLogEnvelope { date, time, tag })
}

fn next_ascii_token<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    while bytes
        .get(*cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *cursor += 1;
    }
    let start = *cursor;
    while bytes
        .get(*cursor)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        *cursor += 1;
    }
    (start != *cursor).then_some(&bytes[start..*cursor])
}

fn time_level_byte(line: &[u8]) -> Option<u8> {
    let rest = rest_after_ascii_tokens(line, 2)?;
    let level = *rest.first()?;
    if !b"VDIWEF".contains(&level) || rest.get(1) != Some(&b'/') {
        return None;
    }
    let after = &rest[2..];
    let open = after.iter().position(|b| *b == b'(')?;
    let close = after.iter().position(|b| *b == b')')?;
    if close < open {
        return None;
    }
    Some(level)
}

fn rest_after_ascii_tokens(line: &[u8], n: usize) -> Option<&[u8]> {
    let mut rest = trim_ascii_start(line);
    for _ in 0..n {
        let ws = rest.iter().position(|b| b.is_ascii_whitespace())?;
        rest = trim_ascii_start(&rest[ws..]);
    }
    if rest.is_empty() {
        None
    } else {
        Some(rest)
    }
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

fn trim_ascii_end(mut bytes: &[u8]) -> &[u8] {
    while let [rest @ .., last] = bytes {
        if last.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
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
    fn threadtime_padded_tag_is_trimmed() {
        // 真机(Android 9 / MiTV)的 threadtime 输出会把短 tag 填充到固定宽度:`I vold    : msg`,
        // tag 必须去掉尾部填充空格。
        let e = parse_line("01-01 08:00:14.364  3176  3176 I vold    : Vold 3.0 firing up");
        assert_eq!(e.tag, "vold");
        assert_eq!(e.message, "Vold 3.0 firing up");
    }

    #[test]
    fn multibyte_line_does_not_panic() {
        // 时间戳后紧跟多字节字符(中文),旧实现会 byte 切片 panic;必须安全回退。
        let e = parse_line("01-01 00:00:00.000 中文消息 hello");
        assert_eq!(e.message, "01-01 00:00:00.000 中文消息 hello");
        // 级别位处即为多字节字符,也不能 panic。
        let _ = parse_line("01-01 00:00:00.000 中/x(1): y");
    }

    #[test]
    fn parse_line_ref_matches_owned_parse_line() {
        let corpus = [
            "04-20 12:06:02.125   146   179 D BatteryService: update start",
            "04-20 12:06:02.125   146   179 E NoColonTag message without delimiter",
            "04-17 09:01:18.910 D/LightsService(  139): BKL : 106",
            "--------- beginning of main",
            "01-01 00:00:00.000 中文消息 hello",
            "01-01 00:00:00.000 中/x(1): y",
            "",
        ];
        for line in corpus {
            let owned = parse_line(line);
            let parsed = parse_line_ref(line);
            assert_eq!(parsed.date, owned.date, "line: {line:?}");
            assert_eq!(parsed.time, owned.time, "line: {line:?}");
            assert_eq!(parsed.level, owned.level, "line: {line:?}");
            assert_eq!(parsed.pid, owned.pid, "line: {line:?}");
            assert_eq!(parsed.tid, owned.tid, "line: {line:?}");
            assert_eq!(parsed.tag, owned.tag, "line: {line:?}");
            assert_eq!(parsed.message, owned.message, "line: {line:?}");
        }
    }

    #[test]
    fn level_byte_matches_parse_line_level_on_corpus() {
        let corpus = [
            "04-20 12:06:02.125   146   179 D BatteryService: update start",
            "04-20 12:06:02.425   300   330 E Payment: SocketTimeoutException",
            "04-20 12:06:02.425   300   330 F Zygote: fatal",
            "04-20 12:06:02.125   146   179 E NoColonTag message without delimiter",
            "04-17 09:01:18.910 D/LightsService(  139): BKL : 106",
            "04-17 09:01:18.910 E/Crash(1): boom",
            "04-17 09:01:18.910 E/NoParen: message",
            "--------- beginning of main",
            "04-20 12:06:02.125   abc   179 D T: bad pid",
            "04-20 12:06:02.125   146   179 X T: bad level",
            "04-20 12:06:02.125 146 179 E",
            "01-01 00:00:00.000 中文消息 hello",
            "01-01 00:00:00.000 中/x(1): y",
            "",
            "   ",
            "04-20 12:06:02.425   300   330 E Payment: with newline\n",
            "04-17 09:01:18.910 F/Crash(1): crlf\r\n",
        ];
        for line in corpus {
            let expected = parse_line(line).level;
            let got = level_byte_of_line(line.as_bytes())
                .map(|b| (b as char).to_string())
                .unwrap_or_default();
            assert_eq!(got, expected, "line: {line:?}");
        }
    }

    #[test]
    fn borrowed_ascii_tag_matches_the_full_parser_without_decoding() {
        for line in [
            b"04-20 12:06:02.125   146   179 D BatteryService: update start".as_slice(),
            b"04-20 12:06:02.125   146   179 E NoColonTag message".as_slice(),
            b"04-17 09:01:18.910 D/LightsService(  139): BKL : 106".as_slice(),
            b"--------- beginning of main".as_slice(),
            b"01-01 00:00:00.000 \xff invalid".as_slice(),
        ] {
            let decoded = String::from_utf8_lossy(line);
            assert_eq!(
                tag_bytes_of_line(line),
                parse_line_ref(&decoded)
                    .tag
                    .is_ascii()
                    .then(|| parse_line_ref(&decoded).tag.as_bytes())
                    .filter(|tag| !tag.is_empty()),
                "line: {line:?}"
            );
        }
    }
}
