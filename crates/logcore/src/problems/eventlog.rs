//! Strict, allocation-free parsing for the small AOSP ActivityManager EventLog
//! schema subset used by the Problems engine.
//!
//! This parser only establishes that textual input matches an AOSP-shaped
//! schema. It deliberately does not assign source provenance.

pub const MAX_EVENTLOG_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_EVENTLOG_FIELD_BYTES: usize = 16 * 1024;
pub const MAX_EVENTLOG_FIELDS: usize = 32;
pub const MAX_AMBIGUOUS_SCHEMA_MATCHES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventLogSchemaId {
    AmAnrLegacyNoUser,
    AmAnrUserPrefixed,
    AmProcDiedLegacyNoUser,
    AmProcDiedUserPrefixed,
    AmProcDiedModernTailNoUser,
    AmProcDiedModernTail,
    AmProcStartLegacyNoUser,
    AmProcStartUserPrefixed,
    AmCrashLegacyNoUser,
    AmCrashUserPrefixed,
    AmCrashModernTailNoUser,
    AmCrashModernTail,
    AmKillLegacyNoUser,
    AmKillUserPrefixed,
    AmKillModernTailNoUser,
    AmKillModernTail,
}

impl EventLogSchemaId {
    pub const fn nominal_arity(self) -> u8 {
        match self {
            Self::AmAnrLegacyNoUser => 4,
            Self::AmAnrUserPrefixed => 5,
            Self::AmProcDiedLegacyNoUser => 2,
            Self::AmProcDiedUserPrefixed => 3,
            Self::AmProcDiedModernTailNoUser => 4,
            Self::AmProcDiedModernTail => 5,
            Self::AmProcStartLegacyNoUser => 5,
            Self::AmProcStartUserPrefixed => 6,
            Self::AmCrashLegacyNoUser => 7,
            Self::AmCrashUserPrefixed => 8,
            Self::AmCrashModernTailNoUser => 8,
            Self::AmCrashModernTail => 9,
            Self::AmKillLegacyNoUser => 4,
            Self::AmKillUserPrefixed => 5,
            Self::AmKillModernTailNoUser => 5,
            Self::AmKillModernTail => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaMatch {
    pub schema: EventLogSchemaId,
    pub nominal_arity: u8,
    pub observed_arity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmbiguousSchemaMatches {
    pub matches: [Option<SchemaMatch>; MAX_AMBIGUOUS_SCHEMA_MATCHES],
    pub count: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedEventLog {
    InvalidEnvelope,
    EmptyPayload,
    PayloadTooLong,
    TooManyFields,
    FieldTooLong,
    InvalidNumber,
    NumericOverflow,
    InvalidText,
    NoMatchingSchema,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLogParseError {
    UnsupportedTag,
    Malformed(MalformedEventLog),
    Ambiguous(AmbiguousSchemaMatches),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmAnr<'a> {
    pub schema: EventLogSchemaId,
    pub user_id: Option<i32>,
    pub pid: u32,
    pub package_name: &'a str,
    pub flags: i32,
    pub reason: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmProcDied<'a> {
    pub schema: EventLogSchemaId,
    pub user_id: Option<i32>,
    pub pid: u32,
    pub process_name: &'a str,
    pub oom_adj: Option<i32>,
    pub proc_state: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmProcStart<'a> {
    pub schema: EventLogSchemaId,
    pub user_id: Option<i32>,
    pub pid: u32,
    pub uid: u32,
    pub process_name: &'a str,
    pub start_type: &'a str,
    pub component: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmCrash<'a> {
    pub schema: EventLogSchemaId,
    pub user_id: Option<i32>,
    pub pid: u32,
    pub process_name: &'a str,
    pub flags: i32,
    pub exception: &'a str,
    pub message: &'a str,
    pub file: &'a str,
    pub line: i32,
    pub recoverable: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmKill<'a> {
    pub schema: EventLogSchemaId,
    pub user_id: Option<i32>,
    pub pid: u32,
    pub process_name: &'a str,
    pub oom_adj: i32,
    pub reason: &'a str,
    pub rss: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLogRecord<'a> {
    Anr(AmAnr<'a>),
    ProcDied(AmProcDied<'a>),
    ProcStart(AmProcStart<'a>),
    Crash(AmCrash<'a>),
    Kill(AmKill<'a>),
}

impl EventLogRecord<'_> {
    pub const fn schema(&self) -> EventLogSchemaId {
        match self {
            Self::Anr(record) => record.schema,
            Self::ProcDied(record) => record.schema,
            Self::ProcStart(record) => record.schema,
            Self::Crash(record) => record.schema,
            Self::Kill(record) => record.schema,
        }
    }
}

pub fn parse_event_log<'a>(
    tag: &str,
    bracket_payload: &'a str,
) -> Result<EventLogRecord<'a>, EventLogParseError> {
    if !matches!(
        tag,
        "am_anr" | "am_proc_died" | "am_proc_start" | "am_crash" | "am_kill"
    ) {
        return Err(EventLogParseError::UnsupportedTag);
    }

    let fields = PayloadFields::parse(bracket_payload)?;
    let mut candidates = CandidateSet::new(fields.len());

    match tag {
        "am_anr" => {
            candidates.add(parse_anr(&fields, EventLogSchemaId::AmAnrLegacyNoUser));
            candidates.add(parse_anr(&fields, EventLogSchemaId::AmAnrUserPrefixed));
        }
        "am_proc_died" => {
            candidates.add(parse_proc_died(
                &fields,
                EventLogSchemaId::AmProcDiedLegacyNoUser,
            ));
            candidates.add(parse_proc_died(
                &fields,
                EventLogSchemaId::AmProcDiedUserPrefixed,
            ));
            candidates.add(parse_proc_died(
                &fields,
                EventLogSchemaId::AmProcDiedModernTailNoUser,
            ));
            candidates.add(parse_proc_died(
                &fields,
                EventLogSchemaId::AmProcDiedModernTail,
            ));
        }
        "am_proc_start" => {
            candidates.add(parse_proc_start(
                &fields,
                EventLogSchemaId::AmProcStartLegacyNoUser,
            ));
            candidates.add(parse_proc_start(
                &fields,
                EventLogSchemaId::AmProcStartUserPrefixed,
            ));
        }
        "am_crash" => {
            candidates.add(parse_crash(&fields, EventLogSchemaId::AmCrashLegacyNoUser));
            candidates.add(parse_crash(&fields, EventLogSchemaId::AmCrashUserPrefixed));
            candidates.add(parse_crash(
                &fields,
                EventLogSchemaId::AmCrashModernTailNoUser,
            ));
            candidates.add(parse_crash(&fields, EventLogSchemaId::AmCrashModernTail));
        }
        "am_kill" => {
            candidates.add(parse_kill(&fields, EventLogSchemaId::AmKillLegacyNoUser));
            candidates.add(parse_kill(&fields, EventLogSchemaId::AmKillUserPrefixed));
            candidates.add(parse_kill(
                &fields,
                EventLogSchemaId::AmKillModernTailNoUser,
            ));
            candidates.add(parse_kill(&fields, EventLogSchemaId::AmKillModernTail));
        }
        _ => unreachable!("supported tags were checked above"),
    }

    candidates.finish()
}

#[derive(Debug, Clone, Copy)]
struct FieldSpan {
    start: usize,
    end: usize,
}

impl FieldSpan {
    const EMPTY: Self = Self { start: 0, end: 0 };
}

#[derive(Debug)]
struct PayloadFields<'a> {
    payload: &'a str,
    spans: [FieldSpan; MAX_EVENTLOG_FIELDS],
    len: u8,
}

impl<'a> PayloadFields<'a> {
    fn parse(bracket_payload: &'a str) -> Result<Self, EventLogParseError> {
        if bracket_payload.len() > MAX_EVENTLOG_PAYLOAD_BYTES {
            return Err(EventLogParseError::Malformed(
                MalformedEventLog::PayloadTooLong,
            ));
        }

        let envelope = trim_ascii(bracket_payload);
        if !envelope.starts_with('[') || !envelope.ends_with(']') {
            return Err(EventLogParseError::Malformed(
                MalformedEventLog::InvalidEnvelope,
            ));
        }
        let payload = &envelope[1..envelope.len() - 1];
        if payload.is_empty() {
            return Err(EventLogParseError::Malformed(
                MalformedEventLog::EmptyPayload,
            ));
        }

        let bytes = payload.as_bytes();
        let mut spans = [FieldSpan::EMPTY; MAX_EVENTLOG_FIELDS];
        let mut count = 0usize;
        let mut start = 0usize;

        for (index, byte) in bytes.iter().copied().enumerate() {
            if byte != b',' {
                continue;
            }
            push_field(payload, &mut spans, &mut count, start, index)?;
            start = index + 1;
        }
        push_field(payload, &mut spans, &mut count, start, bytes.len())?;

        Ok(Self {
            payload,
            spans,
            len: count as u8,
        })
    }

    fn len(&self) -> u8 {
        self.len
    }

    fn field(&self, index: usize) -> &'a str {
        let span = self.spans[index];
        &self.payload[span.start..span.end]
    }

    fn joined(&self, start: usize, end: usize) -> Result<&'a str, SchemaFailure> {
        if start >= end || end > usize::from(self.len) {
            return Err(SchemaFailure::Shape);
        }
        let span_start = self.spans[start].start;
        let span_end = self.spans[end - 1].end;
        let value = &self.payload[span_start..span_end];
        if value.len() > MAX_EVENTLOG_FIELD_BYTES {
            return Err(SchemaFailure::FieldTooLong);
        }
        Ok(value)
    }
}

fn push_field(
    payload: &str,
    spans: &mut [FieldSpan; MAX_EVENTLOG_FIELDS],
    count: &mut usize,
    start: usize,
    end: usize,
) -> Result<(), EventLogParseError> {
    if *count == MAX_EVENTLOG_FIELDS {
        return Err(EventLogParseError::Malformed(
            MalformedEventLog::TooManyFields,
        ));
    }

    let bytes = payload.as_bytes();
    let mut trimmed_start = start;
    let mut trimmed_end = end;
    while trimmed_start < trimmed_end && bytes[trimmed_start].is_ascii_whitespace() {
        trimmed_start += 1;
    }
    while trimmed_end > trimmed_start && bytes[trimmed_end - 1].is_ascii_whitespace() {
        trimmed_end -= 1;
    }
    if trimmed_end - trimmed_start > MAX_EVENTLOG_FIELD_BYTES {
        return Err(EventLogParseError::Malformed(
            MalformedEventLog::FieldTooLong,
        ));
    }

    spans[*count] = FieldSpan {
        start: trimmed_start,
        end: trimmed_end,
    };
    *count += 1;
    Ok(())
}

fn trim_ascii(value: &str) -> &str {
    value.trim_matches(|character: char| character.is_ascii_whitespace())
}

#[derive(Debug, Clone, Copy)]
enum SchemaFailure {
    Shape,
    InvalidNumber,
    NumericOverflow,
    InvalidText,
    FieldTooLong,
}

#[derive(Debug, Default)]
struct FailureSummary {
    invalid_number: bool,
    numeric_overflow: bool,
    invalid_text: bool,
    field_too_long: bool,
}

impl FailureSummary {
    fn observe(&mut self, failure: SchemaFailure) {
        match failure {
            SchemaFailure::Shape => {}
            SchemaFailure::InvalidNumber => self.invalid_number = true,
            SchemaFailure::NumericOverflow => self.numeric_overflow = true,
            SchemaFailure::InvalidText => self.invalid_text = true,
            SchemaFailure::FieldTooLong => self.field_too_long = true,
        }
    }

    fn malformed(&self) -> MalformedEventLog {
        if self.field_too_long {
            MalformedEventLog::FieldTooLong
        } else if self.numeric_overflow {
            MalformedEventLog::NumericOverflow
        } else if self.invalid_number {
            MalformedEventLog::InvalidNumber
        } else if self.invalid_text {
            MalformedEventLog::InvalidText
        } else {
            MalformedEventLog::NoMatchingSchema
        }
    }
}

struct CandidateSet<'a> {
    records: [Option<EventLogRecord<'a>>; MAX_AMBIGUOUS_SCHEMA_MATCHES],
    count: usize,
    observed_arity: u8,
    failures: FailureSummary,
}

impl<'a> CandidateSet<'a> {
    fn new(observed_arity: u8) -> Self {
        Self {
            records: [None; MAX_AMBIGUOUS_SCHEMA_MATCHES],
            count: 0,
            observed_arity,
            failures: FailureSummary::default(),
        }
    }

    fn add(&mut self, result: Result<EventLogRecord<'a>, SchemaFailure>) {
        match result {
            Ok(record) => {
                debug_assert!(self.count < MAX_AMBIGUOUS_SCHEMA_MATCHES);
                if self.count < MAX_AMBIGUOUS_SCHEMA_MATCHES {
                    self.records[self.count] = Some(record);
                    self.count += 1;
                }
            }
            Err(failure) => self.failures.observe(failure),
        }
    }

    fn finish(self) -> Result<EventLogRecord<'a>, EventLogParseError> {
        match self.count {
            0 => Err(EventLogParseError::Malformed(self.failures.malformed())),
            1 => Ok(self.records[0].expect("one candidate was stored")),
            count => {
                let mut matches = [None; MAX_AMBIGUOUS_SCHEMA_MATCHES];
                for (destination, record) in matches.iter_mut().zip(self.records).take(count) {
                    let schema = record.expect("successful candidate was stored").schema();
                    *destination = Some(SchemaMatch {
                        schema,
                        nominal_arity: schema.nominal_arity(),
                        observed_arity: self.observed_arity,
                    });
                }
                Err(EventLogParseError::Ambiguous(AmbiguousSchemaMatches {
                    matches,
                    count: count as u8,
                }))
            }
        }
    }
}

fn parse_anr<'a>(
    fields: &PayloadFields<'a>,
    schema: EventLogSchemaId,
) -> Result<EventLogRecord<'a>, SchemaFailure> {
    let (user_id, base, minimum_fields) = match schema {
        EventLogSchemaId::AmAnrLegacyNoUser => (None, 0, 4),
        EventLogSchemaId::AmAnrUserPrefixed => (Some(parse_i32(fields.field(0))?), 1, 5),
        _ => return Err(SchemaFailure::Shape),
    };
    if usize::from(fields.len()) < minimum_fields {
        return Err(SchemaFailure::Shape);
    }

    let pid = parse_pid(fields.field(base))?;
    let package_name = parse_name(fields.field(base + 1))?;
    let flags = parse_i32(fields.field(base + 2))?;
    let reason = parse_required_text(fields.joined(base + 3, usize::from(fields.len()))?)?;

    Ok(EventLogRecord::Anr(AmAnr {
        schema,
        user_id,
        pid,
        package_name,
        flags,
        reason,
    }))
}

fn parse_proc_died<'a>(
    fields: &PayloadFields<'a>,
    schema: EventLogSchemaId,
) -> Result<EventLogRecord<'a>, SchemaFailure> {
    let (user_id, base, expected_fields, has_modern_tail) = match schema {
        EventLogSchemaId::AmProcDiedLegacyNoUser => (None, 0, 2, false),
        EventLogSchemaId::AmProcDiedUserPrefixed => {
            (Some(parse_i32(fields.field(0))?), 1, 3, false)
        }
        EventLogSchemaId::AmProcDiedModernTailNoUser => (None, 0, 4, true),
        EventLogSchemaId::AmProcDiedModernTail => (Some(parse_i32(fields.field(0))?), 1, 5, true),
        _ => return Err(SchemaFailure::Shape),
    };
    require_arity(fields, expected_fields)?;

    let pid = parse_pid(fields.field(base))?;
    let process_name = parse_name(fields.field(base + 1))?;
    let (oom_adj, proc_state) = if has_modern_tail {
        (
            Some(parse_i32(fields.field(base + 2))?),
            Some(parse_i32(fields.field(base + 3))?),
        )
    } else {
        (None, None)
    };

    Ok(EventLogRecord::ProcDied(AmProcDied {
        schema,
        user_id,
        pid,
        process_name,
        oom_adj,
        proc_state,
    }))
}

fn parse_proc_start<'a>(
    fields: &PayloadFields<'a>,
    schema: EventLogSchemaId,
) -> Result<EventLogRecord<'a>, SchemaFailure> {
    let (user_id, base, expected_fields) = match schema {
        EventLogSchemaId::AmProcStartLegacyNoUser => (None, 0, 5),
        EventLogSchemaId::AmProcStartUserPrefixed => (Some(parse_i32(fields.field(0))?), 1, 6),
        _ => return Err(SchemaFailure::Shape),
    };
    require_arity(fields, expected_fields)?;

    let pid = parse_pid(fields.field(base))?;
    let uid = parse_uid(fields.field(base + 1))?;
    let process_name = parse_name(fields.field(base + 2))?;
    let start_type = parse_required_text(fields.field(base + 3))?;
    let component = parse_required_text(fields.field(base + 4))?;

    Ok(EventLogRecord::ProcStart(AmProcStart {
        schema,
        user_id,
        pid,
        uid,
        process_name,
        start_type,
        component,
    }))
}

fn parse_crash<'a>(
    fields: &PayloadFields<'a>,
    schema: EventLogSchemaId,
) -> Result<EventLogRecord<'a>, SchemaFailure> {
    let (user_id, base, minimum_fields, has_recoverable) = match schema {
        EventLogSchemaId::AmCrashLegacyNoUser => (None, 0, 7, false),
        EventLogSchemaId::AmCrashUserPrefixed => (Some(parse_i32(fields.field(0))?), 1, 8, false),
        EventLogSchemaId::AmCrashModernTailNoUser => (None, 0, 8, true),
        EventLogSchemaId::AmCrashModernTail => (Some(parse_i32(fields.field(0))?), 1, 9, true),
        _ => return Err(SchemaFailure::Shape),
    };
    let field_count = usize::from(fields.len());
    if field_count < minimum_fields {
        return Err(SchemaFailure::Shape);
    }

    let pid = parse_pid(fields.field(base))?;
    let process_name = parse_name(fields.field(base + 1))?;
    let flags = parse_i32(fields.field(base + 2))?;
    let exception = parse_name(fields.field(base + 3))?;
    let tail_fields = if has_recoverable { 3 } else { 2 };
    let file_index = field_count - tail_fields;
    let message = parse_optional_text(fields.joined(base + 4, file_index)?)?;
    let file = parse_source_file(fields.field(file_index))?;
    let line = parse_i32(fields.field(file_index + 1))?;
    let recoverable = if has_recoverable {
        Some(parse_bool_int(fields.field(file_index + 2))?)
    } else {
        None
    };

    Ok(EventLogRecord::Crash(AmCrash {
        schema,
        user_id,
        pid,
        process_name,
        flags,
        exception,
        message,
        file,
        line,
        recoverable,
    }))
}

fn parse_kill<'a>(
    fields: &PayloadFields<'a>,
    schema: EventLogSchemaId,
) -> Result<EventLogRecord<'a>, SchemaFailure> {
    let (user_id, base, minimum_fields, has_rss) = match schema {
        EventLogSchemaId::AmKillLegacyNoUser => (None, 0, 4, false),
        EventLogSchemaId::AmKillUserPrefixed => (Some(parse_i32(fields.field(0))?), 1, 5, false),
        EventLogSchemaId::AmKillModernTailNoUser => (None, 0, 5, true),
        EventLogSchemaId::AmKillModernTail => (Some(parse_i32(fields.field(0))?), 1, 6, true),
        _ => return Err(SchemaFailure::Shape),
    };
    let field_count = usize::from(fields.len());
    if field_count < minimum_fields {
        return Err(SchemaFailure::Shape);
    }

    let pid = parse_pid(fields.field(base))?;
    let process_name = parse_name(fields.field(base + 1))?;
    let oom_adj = parse_i32(fields.field(base + 2))?;
    let reason_end = if has_rss {
        field_count - 1
    } else {
        field_count
    };
    let reason = parse_required_text(fields.joined(base + 3, reason_end)?)?;
    let rss = if has_rss {
        Some(parse_non_negative_i64(fields.field(field_count - 1))?)
    } else {
        None
    };

    Ok(EventLogRecord::Kill(AmKill {
        schema,
        user_id,
        pid,
        process_name,
        oom_adj,
        reason,
        rss,
    }))
}

fn require_arity(fields: &PayloadFields<'_>, expected: usize) -> Result<(), SchemaFailure> {
    if usize::from(fields.len()) == expected {
        Ok(())
    } else {
        Err(SchemaFailure::Shape)
    }
}

fn parse_pid(value: &str) -> Result<u32, SchemaFailure> {
    let value = parse_i32(value)?;
    if value <= 0 {
        return Err(SchemaFailure::InvalidNumber);
    }
    Ok(value as u32)
}

fn parse_uid(value: &str) -> Result<u32, SchemaFailure> {
    let value = parse_i32(value)?;
    if value < 0 {
        return Err(SchemaFailure::InvalidNumber);
    }
    Ok(value as u32)
}

fn parse_i32(value: &str) -> Result<i32, SchemaFailure> {
    let value = parse_i64(value)?;
    i32::try_from(value).map_err(|_| SchemaFailure::NumericOverflow)
}

fn parse_non_negative_i64(value: &str) -> Result<u64, SchemaFailure> {
    let value = parse_i64(value)?;
    if value < 0 {
        return Err(SchemaFailure::InvalidNumber);
    }
    Ok(value as u64)
}

fn parse_bool_int(value: &str) -> Result<bool, SchemaFailure> {
    match parse_i64(value)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(SchemaFailure::InvalidNumber),
    }
}

fn parse_i64(value: &str) -> Result<i64, SchemaFailure> {
    let bytes = value.as_bytes();
    let digits = if let Some((&b'-', rest)) = bytes.split_first() {
        rest
    } else {
        bytes
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(SchemaFailure::InvalidNumber);
    }
    value.parse::<i64>().map_err(|error| match error.kind() {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            SchemaFailure::NumericOverflow
        }
        _ => SchemaFailure::InvalidNumber,
    })
}

fn parse_name(value: &str) -> Result<&str, SchemaFailure> {
    if !valid_text(value)
        || value.is_empty()
        || !value.bytes().any(|byte| byte.is_ascii_alphabetic())
        || value.contains(['[', ']'])
    {
        return Err(SchemaFailure::InvalidText);
    }
    Ok(value)
}

fn parse_required_text(value: &str) -> Result<&str, SchemaFailure> {
    if value.is_empty() || !valid_text(value) {
        return Err(SchemaFailure::InvalidText);
    }
    Ok(value)
}

fn parse_optional_text(value: &str) -> Result<&str, SchemaFailure> {
    if !valid_text(value) {
        return Err(SchemaFailure::InvalidText);
    }
    Ok(value)
}

fn parse_source_file(value: &str) -> Result<&str, SchemaFailure> {
    if !valid_text(value)
        || (!value.is_empty() && !value.bytes().any(|byte| byte.is_ascii_alphabetic()))
    {
        return Err(SchemaFailure::InvalidText);
    }
    Ok(value)
}

fn valid_text(value: &str) -> bool {
    value.len() <= MAX_EVENTLOG_FIELD_BYTES && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse<'a>(tag: &str, payload: &'a str) -> EventLogRecord<'a> {
        parse_event_log(tag, payload).expect("fixture should parse")
    }

    #[test]
    fn parses_legacy_no_user_variants_and_preserves_commas() {
        assert_eq!(
            parse(
                "am_anr",
                "[321,com.example.app,7,Input dispatching timed out, waiting for focus]"
            ),
            EventLogRecord::Anr(AmAnr {
                schema: EventLogSchemaId::AmAnrLegacyNoUser,
                user_id: None,
                pid: 321,
                package_name: "com.example.app",
                flags: 7,
                reason: "Input dispatching timed out, waiting for focus",
            })
        );
        assert_eq!(
            parse("am_proc_died", "[321,com.example.app]"),
            EventLogRecord::ProcDied(AmProcDied {
                schema: EventLogSchemaId::AmProcDiedLegacyNoUser,
                user_id: None,
                pid: 321,
                process_name: "com.example.app",
                oom_adj: None,
                proc_state: None,
            })
        );
        assert_eq!(
            parse(
                "am_proc_start",
                "[321,10001,com.example.app,activity,com.example.app/.MainActivity]"
            ),
            EventLogRecord::ProcStart(AmProcStart {
                schema: EventLogSchemaId::AmProcStartLegacyNoUser,
                user_id: None,
                pid: 321,
                uid: 10001,
                process_name: "com.example.app",
                start_type: "activity",
                component: "com.example.app/.MainActivity",
            })
        );
        assert_eq!(
            parse(
                "am_crash",
                "[321,com.example.app,0,java.lang.IllegalStateException,bad state, with comma,Example.kt,42]"
            ),
            EventLogRecord::Crash(AmCrash {
                schema: EventLogSchemaId::AmCrashLegacyNoUser,
                user_id: None,
                pid: 321,
                process_name: "com.example.app",
                flags: 0,
                exception: "java.lang.IllegalStateException",
                message: "bad state, with comma",
                file: "Example.kt",
                line: 42,
                recoverable: None,
            })
        );
        assert_eq!(
            parse(
                "am_kill",
                "[321,com.example.app,900,empty for 120s, cached]"
            ),
            EventLogRecord::Kill(AmKill {
                schema: EventLogSchemaId::AmKillLegacyNoUser,
                user_id: None,
                pid: 321,
                process_name: "com.example.app",
                oom_adj: 900,
                reason: "empty for 120s, cached",
                rss: None,
            })
        );
    }

    #[test]
    fn parses_user_prefixed_variants() {
        assert_eq!(
            parse(
                "am_anr",
                "[10,321,com.example.app,7,Input dispatching timed out, waiting]"
            )
            .schema(),
            EventLogSchemaId::AmAnrUserPrefixed
        );
        assert_eq!(
            parse("am_proc_died", "[10,321,com.example.app]").schema(),
            EventLogSchemaId::AmProcDiedUserPrefixed
        );
        assert_eq!(
            parse(
                "am_proc_start",
                "[10,321,10001,com.example.app,activity,com.example.app/.MainActivity]"
            )
            .schema(),
            EventLogSchemaId::AmProcStartUserPrefixed
        );
        assert_eq!(
            parse(
                "am_crash",
                "[10,321,com.example.app,0,java.lang.IllegalStateException,bad state, with comma,Example.java,77]"
            )
            .schema(),
            EventLogSchemaId::AmCrashUserPrefixed
        );
        assert_eq!(
            parse(
                "am_kill",
                "[10,321,com.example.app,900,empty for 120s, cached]"
            )
            .schema(),
            EventLogSchemaId::AmKillUserPrefixed
        );
    }

    #[test]
    fn parses_unambiguous_modern_tail_variants() {
        assert_eq!(
            parse("am_proc_died", "[321,com.example.app,900,15]"),
            EventLogRecord::ProcDied(AmProcDied {
                schema: EventLogSchemaId::AmProcDiedModernTailNoUser,
                user_id: None,
                pid: 321,
                process_name: "com.example.app",
                oom_adj: Some(900),
                proc_state: Some(15),
            })
        );
        assert_eq!(
            parse("am_proc_died", "[10,321,com.example.app,900,15]"),
            EventLogRecord::ProcDied(AmProcDied {
                schema: EventLogSchemaId::AmProcDiedModernTail,
                user_id: Some(10),
                pid: 321,
                process_name: "com.example.app",
                oom_adj: Some(900),
                proc_state: Some(15),
            })
        );
        assert_eq!(
            parse(
                "am_crash",
                "[321,com.example.app,0,java.lang.IllegalStateException,bad state, with comma,Example.kt,42,0]"
            ),
            EventLogRecord::Crash(AmCrash {
                schema: EventLogSchemaId::AmCrashModernTailNoUser,
                user_id: None,
                pid: 321,
                process_name: "com.example.app",
                flags: 0,
                exception: "java.lang.IllegalStateException",
                message: "bad state, with comma",
                file: "Example.kt",
                line: 42,
                recoverable: Some(false),
            })
        );
        assert_eq!(
            parse(
                "am_crash",
                "[10,321,com.example.app,0,java.lang.IllegalStateException,bad state, with comma,Example.kt,42,1]"
            ),
            EventLogRecord::Crash(AmCrash {
                schema: EventLogSchemaId::AmCrashModernTail,
                user_id: Some(10),
                pid: 321,
                process_name: "com.example.app",
                flags: 0,
                exception: "java.lang.IllegalStateException",
                message: "bad state, with comma",
                file: "Example.kt",
                line: 42,
                recoverable: Some(true),
            })
        );
    }

    #[test]
    fn rejects_raw_modern_kill_when_old_free_form_reason_also_matches() {
        let no_user_error =
            parse_event_log("am_kill", "[321,com.example.app,900,cached empty,2048]")
                .expect_err("raw text cannot prove whether 2048 is reason text or RSS");
        let EventLogParseError::Ambiguous(no_user_matches) = no_user_error else {
            panic!("expected ambiguous schemas, got {no_user_error:?}");
        };
        assert_eq!(no_user_matches.count, 2);
        assert_eq!(
            no_user_matches.matches[0],
            Some(SchemaMatch {
                schema: EventLogSchemaId::AmKillLegacyNoUser,
                nominal_arity: 4,
                observed_arity: 5,
            })
        );
        assert_eq!(
            no_user_matches.matches[1],
            Some(SchemaMatch {
                schema: EventLogSchemaId::AmKillModernTailNoUser,
                nominal_arity: 5,
                observed_arity: 5,
            })
        );

        let error = parse_event_log("am_kill", "[10,321,com.example.app,900,cached empty,2048]")
            .expect_err("raw text cannot prove whether 2048 is reason text or RSS");
        let EventLogParseError::Ambiguous(matches) = error else {
            panic!("expected ambiguous schemas, got {error:?}");
        };
        assert_eq!(matches.count, 2);
        assert_eq!(
            matches.matches[0],
            Some(SchemaMatch {
                schema: EventLogSchemaId::AmKillUserPrefixed,
                nominal_arity: 5,
                observed_arity: 6,
            })
        );
        assert_eq!(
            matches.matches[1],
            Some(SchemaMatch {
                schema: EventLogSchemaId::AmKillModernTail,
                nominal_arity: 6,
                observed_arity: 6,
            })
        );
    }

    #[test]
    fn reports_malformed_envelopes_numbers_and_limits() {
        assert_eq!(
            parse_event_log("am_anr", "[1,com.example.app,0,reason"),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::InvalidEnvelope
            ))
        );
        assert_eq!(
            parse_event_log("am_proc_died", "[999999999999999999999999,com.example.app]"),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::NumericOverflow
            ))
        );
        assert_eq!(
            parse_event_log("am_proc_died", "[3000000000,com.example.app]"),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::NumericOverflow
            ))
        );
        assert_eq!(
            parse_event_log(
                "am_crash",
                "[10,321,com.example.app,0,java.lang.Error,message,Example.kt,42,2]"
            ),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::InvalidNumber
            ))
        );

        let too_many = format!("[{}]", vec!["x"; MAX_EVENTLOG_FIELDS + 1].join(","));
        assert_eq!(
            parse_event_log("am_anr", &too_many),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::TooManyFields
            ))
        );

        let long_field = "x".repeat(MAX_EVENTLOG_FIELD_BYTES + 1);
        let oversized_field = format!("[1,{long_field}]");
        assert_eq!(
            parse_event_log("am_proc_died", &oversized_field),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::FieldTooLong
            ))
        );

        let oversized_payload = format!("[{}]", "x".repeat(MAX_EVENTLOG_PAYLOAD_BYTES));
        assert_eq!(
            parse_event_log("am_anr", &oversized_payload),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::PayloadTooLong
            ))
        );
    }

    #[test]
    fn unknown_tag_and_near_misses_are_not_guessed() {
        assert_eq!(
            parse_event_log("am_not_real", "[1,com.example.app]"),
            Err(EventLogParseError::UnsupportedTag)
        );
        assert_eq!(
            parse_event_log("am_proc_died", "[1,123]"),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::InvalidText
            ))
        );
        assert_eq!(
            parse_event_log("am_crash", "[1,com.example.app,0,Exception,msg,42,7]"),
            Err(EventLogParseError::Malformed(
                MalformedEventLog::InvalidText
            ))
        );
    }

    #[test]
    fn malformed_inputs_never_panic() {
        let samples = [
            "",
            "[]",
            "[",
            "]",
            "[,,,,]",
            "[\0]",
            "[1,2,3,4,5,6,7,8,9,10]",
            "[+,+,+]",
            "[1,\nprocess]",
            "prefix [1,process]",
            "[1,process] suffix",
        ];
        for tag in [
            "am_anr",
            "am_proc_died",
            "am_proc_start",
            "am_crash",
            "am_kill",
        ] {
            for payload in samples {
                assert!(
                    std::panic::catch_unwind(|| parse_event_log(tag, payload)).is_ok(),
                    "{tag} {payload:?}"
                );
            }
        }
    }
}
