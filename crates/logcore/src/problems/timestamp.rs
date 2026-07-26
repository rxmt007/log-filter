// Strict Android log timestamp parsing and deterministic comparability segments.

use super::PackedLogTimestamp;

pub fn parse_log_timestamp(date: &str, time: &str) -> Option<PackedLogTimestamp> {
    parse_log_timestamp_bytes(date.as_bytes(), time.as_bytes())
}

/// 从完整原始行前两个 ASCII token 解析 Android 时间戳,无需先解码正文。
pub fn parse_log_timestamp_prefix(raw: &[u8]) -> Option<PackedLogTimestamp> {
    let raw = trim_ascii_start(raw);
    let date = raw.get(..5)?;
    if !raw.get(5)?.is_ascii_whitespace() {
        return None;
    }
    let remainder = trim_ascii_start(&raw[5..]);
    let time = remainder.get(..12)?;
    if remainder
        .get(12)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    parse_log_timestamp_bytes(date, time)
}

fn parse_log_timestamp_bytes(date: &[u8], time: &[u8]) -> Option<PackedLogTimestamp> {
    if date.len() != 5
        || time.len() != 12
        || date[2] != b'-'
        || time[2] != b':'
        || time[5] != b':'
        || time[8] != b'.'
    {
        return None;
    }

    let month = parse_two_digits(date, 0)?;
    let day = parse_two_digits(date, 3)?;
    let hour = parse_two_digits(time, 0)?;
    let minute = parse_two_digits(time, 3)?;
    let second = parse_two_digits(time, 6)?;
    let millis = parse_three_digits(time, 9)?;
    if day == 0 || day > days_in_month(month)? {
        return None;
    }

    PackedLogTimestamp::new(month, day, hour, minute, second, millis)
}

fn parse_two_digits(bytes: &[u8], start: usize) -> Option<u8> {
    let tens = bytes[start];
    let ones = bytes[start + 1];
    if !tens.is_ascii_digit() || !ones.is_ascii_digit() {
        return None;
    }
    Some((tens - b'0') * 10 + (ones - b'0'))
}

fn parse_three_digits(bytes: &[u8], start: usize) -> Option<u16> {
    let hundreds = bytes[start];
    let tens = bytes[start + 1];
    let ones = bytes[start + 2];
    if !hundreds.is_ascii_digit() || !tens.is_ascii_digit() || !ones.is_ascii_digit() {
        return None;
    }
    Some(u16::from(hundreds - b'0') * 100 + u16::from(tens - b'0') * 10 + u16::from(ones - b'0'))
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

fn days_in_month(month: u8) -> Option<u8> {
    const DAYS: [u8; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    month
        .checked_sub(1)
        .and_then(|index| DAYS.get(usize::from(index)).copied())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct TimestampSegmentId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentedTimestamp {
    timestamp: PackedLogTimestamp,
    segment: TimestampSegmentId,
    timeline_ms: u64,
}

impl SegmentedTimestamp {
    pub const fn timestamp(self) -> PackedLogTimestamp {
        self.timestamp
    }

    pub const fn segment(self) -> TimestampSegmentId {
        self.segment
    }

    /// Returns `other - self` in milliseconds when both points are comparable.
    pub fn delta_ms(self, other: Self) -> Option<i64> {
        if self.segment != other.segment {
            return None;
        }
        let left = i64::try_from(self.timeline_ms).ok()?;
        let right = i64::try_from(other.timeline_ms).ok()?;
        right.checked_sub(left)
    }
}

#[derive(Debug, Clone, Copy)]
struct DecodedTimestamp {
    month: u8,
    day: u8,
    timeline_ms: u64,
}

#[derive(Debug, Default)]
pub struct TimestampSegmentTracker {
    last: Option<DecodedTimestamp>,
    segment: u64,
    has_segment: bool,
    boundary_pending: bool,
    exhausted: bool,
}

impl TimestampSegmentTracker {
    pub const fn new() -> Self {
        Self {
            last: None,
            segment: 0,
            has_segment: false,
            boundary_pending: false,
            exhausted: false,
        }
    }

    pub fn observe(&mut self, timestamp: Option<PackedLogTimestamp>) -> Option<SegmentedTimestamp> {
        if self.exhausted {
            return None;
        }
        let Some(timestamp) = timestamp.filter(|value| value.is_known()) else {
            self.last = None;
            self.boundary_pending |= self.has_segment;
            return None;
        };
        let Some(decoded) = decode_timestamp(timestamp) else {
            self.last = None;
            self.boundary_pending |= self.has_segment;
            return None;
        };

        let starts_new_segment = self.boundary_pending
            || self.last.is_some_and(|last| {
                decoded.timeline_ms < last.timeline_ms || leap_year_is_ambiguous(last, decoded)
            });
        if !self.has_segment {
            self.has_segment = true;
        } else if starts_new_segment {
            let Some(segment) = self.segment.checked_add(1) else {
                self.exhausted = true;
                self.last = None;
                return None;
            };
            self.segment = segment;
        }

        self.last = Some(decoded);
        self.boundary_pending = false;
        Some(SegmentedTimestamp {
            timestamp,
            segment: TimestampSegmentId(self.segment),
            timeline_ms: decoded.timeline_ms,
        })
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

fn decode_timestamp(timestamp: PackedLogTimestamp) -> Option<DecodedTimestamp> {
    let mut packed = timestamp.raw().checked_sub(1)?;
    let millis = packed % 1_000;
    packed /= 1_000;
    let second = packed % 60;
    packed /= 60;
    let minute = packed % 60;
    packed /= 60;
    let hour = packed % 24;
    packed /= 24;
    let month = u8::try_from(packed / 32).ok()?;
    let day = u8::try_from(packed % 32).ok()?;
    if day == 0 || day > days_in_month(month)? {
        return None;
    }

    const DAYS_BEFORE_MONTH: [u16; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
    let day_of_year = u64::from(DAYS_BEFORE_MONTH[usize::from(month - 1)]) + u64::from(day - 1);
    let timeline_ms = ((((day_of_year * 24 + hour) * 60 + minute) * 60 + second) * 1_000) + millis;
    Some(DecodedTimestamp {
        month,
        day,
        timeline_ms,
    })
}

fn leap_year_is_ambiguous(previous: DecodedTimestamp, current: DecodedTimestamp) -> bool {
    previous.month <= 2 && current.month >= 3 && !(previous.month == 2 && previous.day == 29)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(date: &str, time: &str) -> PackedLogTimestamp {
        parse_log_timestamp(date, time).expect("valid timestamp")
    }

    #[test]
    fn parses_only_exact_ascii_timestamp_format_and_real_calendar_dates() {
        assert!(parse_log_timestamp("07-26", "09:08:07.006").is_some());
        assert!(parse_log_timestamp("02-29", "23:59:59.999").is_some());

        for (date, time) in [
            ("7-26", "09:08:07.006"),
            ("07/26", "09:08:07.006"),
            ("０7-26", "09:08:07.006"),
            ("07-26", "9:08:07.006"),
            ("07-26", "09-08-07.006"),
            ("07-26", "09:08:07,006"),
            ("07-26", "09:08:07.06"),
            ("00-01", "09:08:07.006"),
            ("13-01", "09:08:07.006"),
            ("02-30", "09:08:07.006"),
            ("04-31", "09:08:07.006"),
            ("07-26", "24:08:07.006"),
            ("07-26", "09:60:07.006"),
            ("07-26", "09:08:60.006"),
        ] {
            assert_eq!(parse_log_timestamp(date, time), None, "{date} {time}");
        }
    }

    #[test]
    fn byte_prefix_parser_matches_the_string_parser_without_decoding_the_line() {
        for raw in [
            b"07-26 09:08:07.006  1  1 I Tag: value".as_slice(),
            b"07-26 09:08:07.006 D/Tag(  1): value".as_slice(),
            b"--------- beginning of main".as_slice(),
            b"07-26 invalid \xff".as_slice(),
        ] {
            let decoded = String::from_utf8_lossy(raw);
            let mut tokens = decoded.split_whitespace();
            let expected = tokens
                .next()
                .zip(tokens.next())
                .and_then(|(date, time)| parse_log_timestamp(date, time));
            assert_eq!(parse_log_timestamp_prefix(raw), expected, "raw: {raw:?}");
        }
    }

    #[test]
    fn computes_59_60_61_second_deltas_and_same_millisecond() {
        let mut tracker = TimestampSegmentTracker::new();
        let base = tracker
            .observe(Some(timestamp("07-26", "09:00:00.000")))
            .unwrap();
        let at_59 = tracker
            .observe(Some(timestamp("07-26", "09:00:59.000")))
            .unwrap();
        let at_60 = tracker
            .observe(Some(timestamp("07-26", "09:01:00.000")))
            .unwrap();
        let at_61 = tracker
            .observe(Some(timestamp("07-26", "09:01:01.000")))
            .unwrap();
        let same = tracker
            .observe(Some(timestamp("07-26", "09:01:01.000")))
            .unwrap();

        assert_eq!(base.delta_ms(at_59), Some(59_000));
        assert_eq!(base.delta_ms(at_60), Some(60_000));
        assert_eq!(base.delta_ms(at_61), Some(61_000));
        assert_eq!(at_61.delta_ms(same), Some(0));
        assert_eq!(at_60.delta_ms(base), Some(-60_000));
    }

    #[test]
    fn rollback_missing_timestamp_and_cross_year_split_segments() {
        let mut tracker = TimestampSegmentTracker::new();
        let first = tracker
            .observe(Some(timestamp("07-26", "09:00:01.000")))
            .unwrap();
        let rollback = tracker
            .observe(Some(timestamp("07-26", "08:59:59.000")))
            .unwrap();
        assert_ne!(first.segment(), rollback.segment());
        assert_eq!(first.delta_ms(rollback), None);

        assert_eq!(tracker.observe(None), None);
        let after_missing = tracker
            .observe(Some(timestamp("07-26", "09:00:02.000")))
            .unwrap();
        assert_ne!(rollback.segment(), after_missing.segment());
        assert_eq!(rollback.delta_ms(after_missing), None);

        tracker.reset();
        let year_end = tracker
            .observe(Some(timestamp("12-31", "23:59:59.999")))
            .unwrap();
        let year_start = tracker
            .observe(Some(timestamp("01-01", "00:00:00.000")))
            .unwrap();
        assert_ne!(year_end.segment(), year_start.segment());
        assert_eq!(year_end.delta_ms(year_start), None);
    }

    #[test]
    fn leap_year_ambiguity_is_not_guessed() {
        let mut tracker = TimestampSegmentTracker::new();
        let feb_28 = tracker
            .observe(Some(timestamp("02-28", "23:59:59.000")))
            .unwrap();
        let march_1 = tracker
            .observe(Some(timestamp("03-01", "00:00:00.000")))
            .unwrap();
        assert_eq!(feb_28.delta_ms(march_1), None);

        tracker.reset();
        let feb_29 = tracker
            .observe(Some(timestamp("02-29", "23:59:59.000")))
            .unwrap();
        let march_1 = tracker
            .observe(Some(timestamp("03-01", "00:00:00.000")))
            .unwrap();
        assert_eq!(feb_29.delta_ms(march_1), Some(1_000));
    }
}
