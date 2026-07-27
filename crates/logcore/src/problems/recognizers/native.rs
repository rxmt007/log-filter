use super::super::engine::{ObservedLine, RecognizedProblem};
use super::{parse_pid, trim_ascii, FixedText, ProblemRecognizer};
use crate::problems::{
    parse_event_log, parse_log_timestamp, BoundaryFlags, EventLogRecord, EvidenceAdmission,
    EvidenceFlags, EvidenceFormat, EvidencePriority, FingerprintBuilder, FingerprintTokenKind,
    GroupKey, LineProvenance, ObservationCandidate, ObservationRef, ObservationRole, OutcomeFlags,
    PackedLogTimestamp, ProblemEventDraft, ProblemKind, ProcessFingerprintKey, ProcessInstanceKey,
    RuleId, SignatureQuality,
};
use std::collections::VecDeque;

pub(crate) const MAX_NATIVE_LINES: u32 = 256;
pub(crate) const MAX_NATIVE_BYTES: u32 = 64 * 1024;
pub(crate) const MAX_NATIVE_LINE_BYTES: usize = 16 * 1024;
pub(crate) const MAX_NATIVE_FRAMES: usize = 3;
const MAX_PROCESS_NAME: usize = 256;
const MAX_FRAME_TOKEN: usize = 256;
const MAX_SIGNAL_CODE_TOKEN: usize = 32;
const MAX_UNMATCHED: u8 = 8;
const FINGERPRINT_VERSION: u16 = 1;
const TOMBSTONE_SEPARATOR: &str = "*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***";

#[derive(Debug, Clone, Copy)]
struct EvidencePoint {
    line: u32,
    provenance: LineProvenance,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LibcSignalObservation {
    point: EvidencePoint,
    anchor_timestamp: PackedLogTimestamp,
    pid: u32,
    signal: FatalSignal,
    signal_code: FixedText<MAX_SIGNAL_CODE_TOKEN>,
    explicit_process: FixedText<MAX_PROCESS_NAME>,
}

impl LibcSignalObservation {
    pub(crate) const fn pid(self) -> u32 {
        self.pid
    }

    pub(crate) fn explicit_process_ref(&self) -> Option<&str> {
        (!self.explicit_process.is_empty()).then(|| self.explicit_process.as_str())
    }

    pub(crate) fn into_problem(
        self,
        process: &ProcessFingerprintKey,
        process_instance: ProcessInstanceKey,
    ) -> RecognizedProblem {
        let identity_quality = process.identity_quality();
        let signature_quality = SignatureQuality::SignalOnly;
        let mut fingerprint = FingerprintBuilder::new(
            ProblemKind::NativeCrash,
            FINGERPRINT_VERSION,
            signature_quality,
            identity_quality,
            process,
        );
        fingerprint.token(FingerprintTokenKind::Signal, self.signal.canonical());
        fingerprint.token(
            FingerprintTokenKind::StructuredField,
            self.signal_code.as_str().as_bytes(),
        );
        let group_key = GroupKey::new(
            ProblemKind::NativeCrash,
            FINGERPRINT_VERSION,
            signature_quality,
            identity_quality,
            fingerprint.finish(),
        );
        let draft = ProblemEventDraft {
            start_line: self.point.line,
            end_line: self.point.line,
            anchor_line: self.point.line,
            anchor_timestamp: self.anchor_timestamp,
            pid: self.pid,
            process_instance,
            kind: ProblemKind::NativeCrash,
            evidence: EvidenceFlags::PRIMARY,
            outcome: OutcomeFlags::NONE,
            boundary: BoundaryFlags::NONE,
        };
        let primary = libc_observation(
            self.point,
            ObservationRole::Primary,
            EvidencePriority::MinimumGrammar,
        );
        let mut problem = RecognizedProblem::new(draft, group_key, primary);
        problem.push_observation(libc_observation(
            self.point,
            ObservationRole::Signal,
            EvidencePriority::MinimumGrammar,
        ));
        if !self.explicit_process.is_empty() {
            problem.push_observation(libc_observation(
                self.point,
                ObservationRole::ProcessIdentity,
                EvidencePriority::MinimumGrammar,
            ));
        }
        if !process.is_unknown() {
            problem.set_correlation_identity(process, Some(self.signal.canonical()));
        }
        problem.set_display_summary(process, std::str::from_utf8(self.signal.canonical()).ok());
        problem
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FatalSignal {
    Ill,
    Trap,
    Abrt,
    Bus,
    Fpe,
    Segv,
    StkFlt,
    Sys,
}

impl FatalSignal {
    const fn canonical(self) -> &'static [u8] {
        match self {
            Self::Ill => b"SIGILL",
            Self::Trap => b"SIGTRAP",
            Self::Abrt => b"SIGABRT",
            Self::Bus => b"SIGBUS",
            Self::Fpe => b"SIGFPE",
            Self::Segv => b"SIGSEGV",
            Self::StkFlt => b"SIGSTKFLT",
            Self::Sys => b"SIGSYS",
        }
    }
}

#[derive(Debug)]
struct NativePending {
    start: EvidencePoint,
    producer_pid: u32,
    last_evidence_line: u32,
    bytes_seen: u32,
    unmatched: u8,
    victim_pid: u32,
    victim_tid: u32,
    identity_point: Option<EvidencePoint>,
    process_name: FixedText<MAX_PROCESS_NAME>,
    signal: Option<FatalSignal>,
    signal_code: FixedText<MAX_SIGNAL_CODE_TOKEN>,
    signal_point: Option<EvidencePoint>,
    frames: [FixedText<MAX_FRAME_TOKEN>; MAX_NATIVE_FRAMES],
    frame_points: [Option<EvidencePoint>; MAX_NATIVE_FRAMES],
    frame_count: u8,
    frame_limited: bool,
    saw_backtrace: bool,
    recoverable_point: Option<EvidencePoint>,
    forbidden: bool,
}

impl NativePending {
    fn new(line: &ObservedLine<'_>, producer_pid: u32) -> Self {
        let start = EvidencePoint {
            line: line.line,
            provenance: line.provenance,
        };
        Self {
            start,
            producer_pid,
            last_evidence_line: line.line,
            bytes_seen: u32::try_from(line.raw.len()).unwrap_or(u32::MAX),
            unmatched: 0,
            victim_pid: 0,
            victim_tid: 0,
            identity_point: None,
            process_name: FixedText::default(),
            signal: None,
            signal_code: FixedText::default(),
            signal_point: None,
            frames: [FixedText::default(); MAX_NATIVE_FRAMES],
            frame_points: [None; MAX_NATIVE_FRAMES],
            frame_count: 0,
            frame_limited: false,
            saw_backtrace: false,
            recoverable_point: None,
            forbidden: false,
        }
    }

    fn minimum_grammar_met(&self) -> bool {
        !self.forbidden
            && self.victim_pid != 0
            && self.victim_tid != 0
            && self.identity_point.is_some()
            && !self.process_name.is_empty()
            && self.signal.is_some()
            && !self.signal_code.is_empty()
            && self.signal_point.is_some()
    }

    fn compatible(&self, line: &ObservedLine<'_>) -> bool {
        if line.parsed.tag.is_empty() || self.producer_pid == 0 {
            return true;
        }
        parse_pid(line.parsed.pid).is_some_and(|pid| pid == self.producer_pid)
    }

    fn record(&mut self, line: &ObservedLine<'_>, message: &str) -> bool {
        let message = trim_ascii(message);
        let point = EvidencePoint {
            line: line.line,
            provenance: line.provenance,
        };
        let mut matched = false;

        if is_forbidden_dump(message) {
            self.forbidden = true;
            matched = true;
        }
        if let Some((pid, tid)) = parse_pid_tid(message) {
            self.victim_pid = pid;
            self.victim_tid = tid;
            matched = true;
        }
        if let Some(process) = parse_process_name(message) {
            if self.process_name.set(process) {
                self.identity_point = Some(point);
            }
            matched = true;
        }
        if message.starts_with("signal ") {
            match parse_fatal_signal(message) {
                Some(record) if self.signal_code.set(record.code_token) => {
                    self.signal = Some(record.signal);
                    self.signal_point = Some(point);
                }
                None => self.forbidden = true,
                Some(_) => self.forbidden = true,
            }
            matched = true;
        }
        if matches!(message, "recoverable: true" | "Recoverable: true") {
            self.recoverable_point = Some(point);
            matched = true;
        }
        if message == "backtrace:" {
            self.saw_backtrace = true;
            matched = true;
        } else if self.saw_backtrace {
            if let Some(frame) = normalize_native_frame(message) {
                let index = usize::from(self.frame_count);
                if index < MAX_NATIVE_FRAMES {
                    self.frames[index] = frame;
                    self.frame_points[index] = Some(point);
                    self.frame_count += 1;
                } else {
                    self.frame_limited = true;
                }
                matched = true;
            }
        }

        if matched {
            self.last_evidence_line = line.line;
            self.unmatched = 0;
        } else {
            self.unmatched = self.unmatched.saturating_add(1);
        }
        matched
    }

    fn into_problem(self, mut boundary: BoundaryFlags) -> Option<RecognizedProblem> {
        if !self.minimum_grammar_met() {
            return None;
        }
        let process = ProcessFingerprintKey::new(Some(self.process_name.as_str()));
        let identity_quality = process.identity_quality();
        let signature_quality = if self.frame_count > 0 {
            SignatureQuality::FullStack
        } else {
            SignatureQuality::SignalOnly
        };
        let signal = self.signal?;
        let mut fingerprint = FingerprintBuilder::new(
            ProblemKind::NativeCrash,
            FINGERPRINT_VERSION,
            signature_quality,
            identity_quality,
            &process,
        );
        fingerprint.token(FingerprintTokenKind::Signal, signal.canonical());
        fingerprint.token(
            FingerprintTokenKind::StructuredField,
            self.signal_code.as_str().as_bytes(),
        );
        for frame in self.frames.iter().take(usize::from(self.frame_count)) {
            fingerprint.token(FingerprintTokenKind::Frame, frame.as_str().as_bytes());
        }
        let group_key = GroupKey::new(
            ProblemKind::NativeCrash,
            FINGERPRINT_VERSION,
            signature_quality,
            identity_quality,
            fingerprint.finish(),
        );

        if self.frame_limited {
            boundary.insert(BoundaryFlags::OBSERVATION_COUNT_LIMITED);
        }
        let mut outcome = OutcomeFlags::NONE;
        if self.recoverable_point.is_some() {
            outcome.insert(OutcomeFlags::EXPLICITLY_RECOVERABLE);
        }
        let signal_point = self.signal_point?;
        let draft = ProblemEventDraft {
            start_line: self.start.line,
            end_line: self.last_evidence_line,
            anchor_line: signal_point.line,
            anchor_timestamp: PackedLogTimestamp::UNKNOWN,
            pid: self.victim_pid,
            process_instance: ProcessInstanceKey(0),
            kind: ProblemKind::NativeCrash,
            evidence: EvidenceFlags::PRIMARY | EvidenceFlags::STRUCTURED | EvidenceFlags::MULTILINE,
            outcome,
            boundary,
        };
        let mut problem = RecognizedProblem::new(
            draft,
            group_key,
            observation(
                self.start,
                ObservationRole::Primary,
                EvidencePriority::MinimumGrammar,
            ),
        );
        problem.push_observation(observation(
            self.identity_point?,
            ObservationRole::ProcessIdentity,
            EvidencePriority::MinimumGrammar,
        ));
        problem.push_observation(observation(
            signal_point,
            ObservationRole::Signal,
            EvidencePriority::MinimumGrammar,
        ));
        for point in self
            .frame_points
            .iter()
            .take(usize::from(self.frame_count))
            .flatten()
            .copied()
        {
            problem.push_observation(observation(
                point,
                ObservationRole::BacktraceFrame,
                EvidencePriority::Supporting,
            ));
        }
        if let Some(point) = self.recoverable_point {
            problem.push_observation(observation(
                point,
                ObservationRole::Recovery,
                EvidencePriority::Outcome,
            ));
        }
        problem.set_correlation_identity(&process, Some(signal.canonical()));
        problem.set_display_summary(&process, std::str::from_utf8(signal.canonical()).ok());
        Some(problem)
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeRecognizer {
    pending: Option<NativePending>,
    signal_ready: Option<LibcSignalObservation>,
    ready: VecDeque<RecognizedProblem>,
}

impl NativeRecognizer {
    pub(crate) const fn new() -> Self {
        Self {
            pending: None,
            signal_ready: None,
            ready: VecDeque::new(),
        }
    }

    pub(crate) const fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn pop_signal_ready(&mut self) -> Option<LibcSignalObservation> {
        self.signal_ready.take()
    }

    fn finalize(&mut self, boundary: BoundaryFlags) {
        if let Some(problem) = self
            .pending
            .take()
            .and_then(|pending| pending.into_problem(boundary))
        {
            self.ready.push_back(problem);
        }
    }
}

impl ProblemRecognizer for NativeRecognizer {
    fn observe(&mut self, line: &ObservedLine<'_>) {
        if let Some(signal) = recognize_libc_signal(line) {
            self.signal_ready = Some(signal);
            return;
        }
        if let Some(problem) = recognize_native_am_crash(line) {
            self.finalize(BoundaryFlags::NONE);
            self.ready.push_back(problem);
            return;
        }
        let message = line.parsed.message;
        let native_source = matches!(line.parsed.tag, "" | "DEBUG" | "debuggerd");

        if native_source && is_tombstone_separator(trim_ascii(message)) {
            self.finalize(BoundaryFlags::NONE);
            let producer_pid = parse_pid(line.parsed.pid).unwrap_or(0);
            self.pending = Some(NativePending::new(line, producer_pid));
            return;
        }

        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if !pending.compatible(line) {
            return;
        }
        if !native_source {
            let finalize = {
                let pending = self.pending.as_mut().expect("pending was checked above");
                pending.unmatched = pending.unmatched.saturating_add(1);
                (pending.frame_count > 0 || pending.unmatched >= MAX_UNMATCHED)
                    .then_some(pending.frame_count == 0)
            };
            if let Some(limited) = finalize {
                self.finalize(if limited {
                    BoundaryFlags::TRUNCATED_BY_LIMIT
                } else {
                    BoundaryFlags::NONE
                });
            }
            return;
        }
        if line.raw.len() > MAX_NATIVE_LINE_BYTES
            || line.line.saturating_sub(pending.start.line) > MAX_NATIVE_LINES
            || pending
                .bytes_seen
                .checked_add(u32::try_from(line.raw.len()).unwrap_or(u32::MAX))
                .is_none_or(|bytes| bytes > MAX_NATIVE_BYTES)
        {
            self.finalize(BoundaryFlags::TRUNCATED_BY_LIMIT);
            return;
        }

        let finalize_limited = {
            let pending = self.pending.as_mut().expect("pending was checked above");
            pending.bytes_seen += u32::try_from(line.raw.len()).unwrap_or(u32::MAX);
            let matched = pending.record(line, message);
            ((!matched && pending.frame_count > 0) || pending.unmatched >= MAX_UNMATCHED)
                .then_some(pending.frame_count == 0)
        };
        if let Some(limited) = finalize_limited {
            self.finalize(if limited {
                BoundaryFlags::TRUNCATED_BY_LIMIT
            } else {
                BoundaryFlags::NONE
            });
        }
    }

    fn finish_input(&mut self) {
        self.finalize(BoundaryFlags::TRUNCATED_BY_INPUT);
    }

    fn reset(&mut self) {
        self.pending = None;
        self.signal_ready = None;
        self.ready.clear();
    }

    fn pop_ready(&mut self) -> Option<RecognizedProblem> {
        self.ready.pop_front()
    }
}

fn recognize_libc_signal(line: &ObservedLine<'_>) -> Option<LibcSignalObservation> {
    if line.parsed.tag != "libc"
        || line.parsed.level != "F"
        || line.raw.len() > MAX_NATIVE_LINE_BYTES
    {
        return None;
    }
    let message = trim_ascii(line.parsed.message);
    let record = parse_fatal_signal(message.strip_prefix("Fatal ")?)?;
    let pid = parse_pid(line.parsed.pid)?;
    if record.explicit_pid.is_some_and(|explicit| explicit != pid) {
        return None;
    }
    let mut process = FixedText::default();
    if let Some(explicit_process) = record.explicit_process {
        process.set(explicit_process).then_some(())?;
    }
    let mut signal_code = FixedText::default();
    signal_code.set(record.code_token).then_some(())?;
    Some(LibcSignalObservation {
        point: EvidencePoint {
            line: line.line,
            provenance: line.provenance,
        },
        anchor_timestamp: parse_log_timestamp(line.parsed.date, line.parsed.time)
            .unwrap_or_default(),
        pid,
        signal: record.signal,
        signal_code,
        explicit_process: process,
    })
}

fn recognize_native_am_crash(line: &ObservedLine<'_>) -> Option<RecognizedProblem> {
    if line.parsed.tag != "am_crash"
        || line
            .coverage
            .admit(EvidenceFormat::EventLogShapedText, line.provenance)
            != EvidenceAdmission::CommitEligible
    {
        return None;
    }
    let EventLogRecord::Crash(crash) =
        parse_event_log(line.parsed.tag, line.parsed.message).ok()?
    else {
        return None;
    };
    if crash.exception != "Native crash" {
        return None;
    }
    let process = ProcessFingerprintKey::new(Some(crash.process_name));
    if process.is_unknown() {
        return None;
    }
    let identity_quality = process.identity_quality();
    let signature_quality = SignatureQuality::Minimal;
    let mut fingerprint = FingerprintBuilder::new(
        ProblemKind::NativeCrash,
        FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        &process,
    );
    fingerprint.token(FingerprintTokenKind::StructuredField, b"native-am-crash");
    let group_key = GroupKey::new(
        ProblemKind::NativeCrash,
        FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        fingerprint.finish(),
    );
    let mut outcome = OutcomeFlags::NONE;
    if crash.recoverable == Some(true) {
        outcome.insert(OutcomeFlags::EXPLICITLY_RECOVERABLE);
    }
    let draft = ProblemEventDraft {
        start_line: line.line,
        end_line: line.line,
        anchor_line: line.line,
        anchor_timestamp: parse_log_timestamp(line.parsed.date, line.parsed.time)
            .unwrap_or_default(),
        pid: crash.pid,
        process_instance: ProcessInstanceKey(0),
        kind: ProblemKind::NativeCrash,
        evidence: EvidenceFlags::PRIMARY | EvidenceFlags::STRUCTURED,
        outcome,
        boundary: BoundaryFlags::NONE,
    };
    let primary = ObservationCandidate::new(
        ObservationRef::new(
            line.line,
            RuleId::ManagedAmCrashV1,
            ObservationRole::Primary,
            EvidenceFormat::EventLogShapedText,
            line.provenance,
        )
        .expect("am_crash primary is part of the published fact contract"),
        EvidencePriority::MinimumGrammar,
    );
    let mut problem = RecognizedProblem::new(draft, group_key, primary);
    problem.push_observation(ObservationCandidate::new(
        ObservationRef::new(
            line.line,
            RuleId::ManagedAmCrashV1,
            ObservationRole::ProcessIdentity,
            EvidenceFormat::EventLogShapedText,
            line.provenance,
        )
        .expect("am_crash process identity is part of the published fact contract"),
        EvidencePriority::MinimumGrammar,
    ));
    problem.set_correlation_identity(&process, None);
    problem.set_display_summary(&process, Some("Native crash"));
    Some(problem)
}

fn libc_observation(
    point: EvidencePoint,
    role: ObservationRole,
    priority: EvidencePriority,
) -> ObservationCandidate {
    ObservationCandidate::new(
        ObservationRef::new(
            point.line,
            RuleId::NativeLibcSignalV1,
            role,
            EvidenceFormat::AospText,
            point.provenance,
        )
        .expect("libc fatal-signal roles are covered by the fact map"),
        priority,
    )
}

fn observation(
    point: EvidencePoint,
    role: ObservationRole,
    priority: EvidencePriority,
) -> ObservationCandidate {
    ObservationCandidate::new(
        ObservationRef::new(
            point.line,
            RuleId::NativeTombstoneV1,
            role,
            EvidenceFormat::TombstoneShapedText,
            point.provenance,
        )
        .expect("native tombstone roles are covered by the fact map"),
        priority,
    )
}

fn is_tombstone_separator(message: &str) -> bool {
    message == TOMBSTONE_SEPARATOR
}

fn is_forbidden_dump(message: &str) -> bool {
    message.contains("Requested dump")
        || message.contains("requested dump")
        || message.contains("Signal Catcher")
}

fn parse_pid_tid(message: &str) -> Option<(u32, u32)> {
    let rest = message.strip_prefix("pid: ")?;
    let comma = rest.find(", tid: ")?;
    let pid = parse_pid(&rest[..comma])?;
    let rest = &rest[comma + ", tid: ".len()..];
    let tid_end = rest.find(',').unwrap_or(rest.len());
    let tid = parse_pid(trim_ascii(&rest[..tid_end]))?;
    Some((pid, tid))
}

fn parse_process_name(message: &str) -> Option<&str> {
    if let Some(start) = message.find(">>> ") {
        let value = &message[start + 4..];
        let end = value.find(" <<<")?;
        return valid_process_name(&value[..end]).then_some(&value[..end]);
    }
    let value = trim_ascii(message.strip_prefix("Cmdline: ")?);
    valid_process_name(value).then_some(value)
}

fn valid_process_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_PROCESS_NAME
        && value.bytes().any(|byte| byte.is_ascii_alphabetic())
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-' | b'@')
        })
}

#[derive(Debug, Clone, Copy)]
struct FatalSignalRecord<'a> {
    signal: FatalSignal,
    code_token: &'a str,
    explicit_pid: Option<u32>,
    explicit_process: Option<&'a str>,
}

fn parse_fatal_signal(message: &str) -> Option<FatalSignalRecord<'_>> {
    // AOSP bionic emits these fields in a fixed order. Optional facets are
    // parsed below as complete shapes so a valid prefix never admits junk.
    let rest = message.strip_prefix("signal ")?;
    let (number, rest) = rest.split_once(" (")?;
    let number = number.parse::<u8>().ok()?;
    let (name, rest) = rest.split_once("), code ")?;
    let signal = match (number, name) {
        (4, "SIGILL") => Some(FatalSignal::Ill),
        (5, "SIGTRAP") => Some(FatalSignal::Trap),
        (6, "SIGABRT") => Some(FatalSignal::Abrt),
        (7, "SIGBUS") => Some(FatalSignal::Bus),
        (8, "SIGFPE") => Some(FatalSignal::Fpe),
        (11, "SIGSEGV") => Some(FatalSignal::Segv),
        (16, "SIGSTKFLT") => Some(FatalSignal::StkFlt),
        (31, "SIGSYS") => Some(FatalSignal::Sys),
        _ => None,
    }?;

    let (code, rest) = rest.split_once(" (")?;
    let code = parse_signed_decimal(code)?;
    let (code_description, mut rest) = rest.split_once(')')?;
    let code_token = parse_signal_code(signal, code, code_description)?;

    if let Some(after_address) = rest.strip_prefix(", fault addr ") {
        let address_end = after_address
            .find(" in tid ")
            .unwrap_or(after_address.len());
        let address = &after_address[..address_end];
        if !valid_fault_address(address) {
            return None;
        }
        rest = &after_address[address_end..];
    } else if let Some(after_syscall) = rest.strip_prefix(", syscall ") {
        if signal != FatalSignal::Sys || code_token != "SYS_SECCOMP" {
            return None;
        }
        let syscall_end = after_syscall.find(" in tid ")?;
        parse_signed_decimal(&after_syscall[..syscall_end])?;
        rest = &after_syscall[syscall_end..];
    }

    let (explicit_pid, explicit_process) = if rest.is_empty() {
        (None, None)
    } else {
        parse_signal_thread_suffix(rest)?
    };
    Some(FatalSignalRecord {
        signal,
        code_token,
        explicit_pid,
        explicit_process,
    })
}

fn parse_signed_decimal(value: &str) -> Option<i32> {
    if value.is_empty()
        || value.starts_with('+')
        || value == "-"
        || !value
            .strip_prefix('-')
            .unwrap_or(value)
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse::<i32>().ok()
}

fn parse_signal_code(signal: FatalSignal, code: i32, description: &str) -> Option<&str> {
    let (token, sender) = match description.split_once(" from pid ") {
        Some((token, sender)) => (token, Some(sender)),
        None => (description, None),
    };
    if !valid_signal_code_token(signal, token) || !signal_code_matches_number(token, code) {
        return None;
    }
    if let Some(sender) = sender {
        if !matches!(
            token,
            "SI_USER"
                | "SI_QUEUE"
                | "SI_TIMER"
                | "SI_MESGQ"
                | "SI_ASYNCIO"
                | "SI_SIGIO"
                | "SI_TKILL"
                | "SI_DETHREAD"
        ) {
            return None;
        }
        let (pid, uid) = sender.split_once(", uid ")?;
        parse_pid(pid)?;
        parse_unsigned_decimal(uid)?;
    }
    Some(token)
}

fn parse_unsigned_decimal(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn signal_code_matches_number(token: &str, code: i32) -> bool {
    // Linux UAPI si_code values consumed by AOSP debuggerd's get_sigcode().
    // The names and values are stable ABI; "?" deliberately represents a
    // platform value that this parser does not know yet.
    let expected = match token {
        "SI_USER" => 0,
        "SI_KERNEL" => 128,
        "SI_QUEUE" => -1,
        "SI_TIMER" => -2,
        "SI_MESGQ" => -3,
        "SI_ASYNCIO" => -4,
        "SI_SIGIO" => -5,
        "SI_TKILL" => -6,
        "SI_DETHREAD" => -7,
        "ILL_ILLOPC" | "FPE_INTDIV" | "SEGV_MAPERR" | "BUS_ADRALN" | "TRAP_BRKPT"
        | "SYS_SECCOMP" => 1,
        "ILL_ILLOPN" | "FPE_INTOVF" | "SEGV_ACCERR" | "BUS_ADRERR" | "TRAP_TRACE"
        | "SYS_USER_DISPATCH" => 2,
        "ILL_ILLADR" | "FPE_FLTDIV" | "SEGV_BNDERR" | "BUS_OBJERR" | "TRAP_BRANCH" => 3,
        "ILL_ILLTRP" | "FPE_FLTOVF" | "SEGV_PKUERR" | "BUS_MCEERR_AR" | "TRAP_HWBKPT" => 4,
        "ILL_PRVOPC" | "FPE_FLTUND" | "SEGV_ACCADI" | "BUS_MCEERR_AO" | "TRAP_UNDIAGNOSED" => 5,
        "ILL_PRVREG" | "FPE_FLTRES" | "SEGV_ADIDERR" | "TRAP_PERF" => 6,
        "ILL_COPROC" | "FPE_FLTINV" | "SEGV_ADIPERR" => 7,
        "ILL_BADSTK" | "FPE_FLTSUB" | "SEGV_MTEAERR" => 8,
        "ILL_BADIADDR" | "FPE_DECOVF" | "SEGV_MTESERR" => 9,
        "ILL_BREAK" | "FPE_DECDIV" | "SEGV_CPERR" => 10,
        "ILL_BNDMOD" | "FPE_DECERR" => 11,
        "FPE_INVASC" => 12,
        "FPE_INVDEC" => 13,
        "FPE_FLTUNK" => 14,
        "FPE_CONDTRAP" => 15,
        "PTRACE_EVENT_FORK" => 0x105,
        "PTRACE_EVENT_VFORK" => 0x205,
        "PTRACE_EVENT_CLONE" => 0x305,
        "PTRACE_EVENT_EXEC" => 0x405,
        "PTRACE_EVENT_VFORK_DONE" => 0x505,
        "PTRACE_EVENT_EXIT" => 0x605,
        "PTRACE_EVENT_SECCOMP" => 0x705,
        "PTRACE_EVENT_STOP" => 0x8005,
        "?" => return true,
        _ => return false,
    };
    code == expected
}

fn valid_signal_code_token(signal: FatalSignal, token: &str) -> bool {
    // Frozen names emitted by AOSP debuggerd's get_sigcode implementation.
    // Unknown platform codes use the exact token "?", not arbitrary text.
    if token == "?" {
        return true;
    }
    if matches!(
        token,
        "SI_USER"
            | "SI_KERNEL"
            | "SI_QUEUE"
            | "SI_TIMER"
            | "SI_MESGQ"
            | "SI_ASYNCIO"
            | "SI_SIGIO"
            | "SI_TKILL"
            | "SI_DETHREAD"
    ) {
        return true;
    }
    match signal {
        FatalSignal::Ill => matches!(
            token,
            "ILL_ILLOPC"
                | "ILL_ILLOPN"
                | "ILL_ILLADR"
                | "ILL_ILLTRP"
                | "ILL_PRVOPC"
                | "ILL_PRVREG"
                | "ILL_COPROC"
                | "ILL_BADSTK"
                | "ILL_BADIADDR"
                | "ILL_BREAK"
                | "ILL_BNDMOD"
        ),
        FatalSignal::Trap => matches!(
            token,
            "TRAP_BRKPT"
                | "TRAP_TRACE"
                | "TRAP_BRANCH"
                | "TRAP_HWBKPT"
                | "TRAP_UNDIAGNOSED"
                | "TRAP_PERF"
                | "PTRACE_EVENT_FORK"
                | "PTRACE_EVENT_VFORK"
                | "PTRACE_EVENT_CLONE"
                | "PTRACE_EVENT_EXEC"
                | "PTRACE_EVENT_VFORK_DONE"
                | "PTRACE_EVENT_EXIT"
                | "PTRACE_EVENT_SECCOMP"
                | "PTRACE_EVENT_STOP"
        ),
        FatalSignal::Abrt | FatalSignal::StkFlt => false,
        FatalSignal::Bus => matches!(
            token,
            "BUS_ADRALN" | "BUS_ADRERR" | "BUS_OBJERR" | "BUS_MCEERR_AR" | "BUS_MCEERR_AO"
        ),
        FatalSignal::Fpe => matches!(
            token,
            "FPE_INTDIV"
                | "FPE_INTOVF"
                | "FPE_FLTDIV"
                | "FPE_FLTOVF"
                | "FPE_FLTUND"
                | "FPE_FLTRES"
                | "FPE_FLTINV"
                | "FPE_FLTSUB"
                | "FPE_DECOVF"
                | "FPE_DECDIV"
                | "FPE_DECERR"
                | "FPE_INVASC"
                | "FPE_INVDEC"
                | "FPE_FLTUNK"
                | "FPE_CONDTRAP"
        ),
        FatalSignal::Segv => matches!(
            token,
            "SEGV_MAPERR"
                | "SEGV_ACCERR"
                | "SEGV_BNDERR"
                | "SEGV_PKUERR"
                | "SEGV_ACCADI"
                | "SEGV_ADIDERR"
                | "SEGV_ADIPERR"
                | "SEGV_MTEAERR"
                | "SEGV_MTESERR"
                | "SEGV_CPERR"
        ),
        FatalSignal::Sys => matches!(token, "SYS_SECCOMP" | "SYS_USER_DISPATCH"),
    }
}

fn valid_fault_address(address: &str) -> bool {
    if address == "--------" {
        return true;
    }
    let digits = address.strip_prefix("0x").unwrap_or(address);
    !digits.is_empty()
        && digits.len() <= 16
        && (address.starts_with("0x") || matches!(digits.len(), 8 | 16))
        && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_signal_thread_suffix(rest: &str) -> Option<(Option<u32>, Option<&str>)> {
    let rest = rest.strip_prefix(" in tid ")?;
    let (tid, rest) = rest.split_once(" (")?;
    parse_pid(tid)?;

    let (thread_name, pid_and_process) = match rest.split_once("), pid ") {
        Some((thread_name, pid_and_process)) => (thread_name, Some(pid_and_process)),
        None => (rest.strip_suffix(')')?, None),
    };
    valid_task_name(thread_name).then_some(())?;

    let Some(pid_and_process) = pid_and_process else {
        return Some((None, None));
    };
    let (pid, process) = pid_and_process.split_once(" (")?;
    let pid = parse_pid(pid)?;
    let process = process.strip_suffix(')')?;
    valid_parenthesized_name(process, MAX_PROCESS_NAME).then_some(())?;
    let process = valid_process_name(process).then_some(process);
    Some((Some(pid), process))
}

fn valid_task_name(value: &str) -> bool {
    valid_parenthesized_name(value, 16)
}

fn valid_parenthesized_name(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .chars()
            .all(|character| !character.is_control() && character != '(' && character != ')')
}

fn normalize_native_frame(message: &str) -> Option<FixedText<MAX_FRAME_TOKEN>> {
    let mut rest = trim_ascii(message);
    let frame_number = take_token(&mut rest)?;
    if frame_number.len() != 3
        || !frame_number.starts_with('#')
        || !frame_number[1..].bytes().all(|byte| byte.is_ascii_digit())
        || take_token(&mut rest)? != "pc"
    {
        return None;
    }
    let relative_pc = take_token(&mut rest)?;
    if relative_pc.is_empty() || !relative_pc.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let module = take_token(&mut rest)?;
    if module.is_empty() || module.starts_with('(') {
        return None;
    }

    if let Some(symbol) = first_symbol(rest) {
        let symbol = strip_symbol_offset(symbol);
        if !symbol.is_empty() {
            return build_frame_token(&[(module, false), ("#", false), (symbol, false)]);
        }
    }
    let build_id = parse_build_id(rest)?;
    build_frame_token(&[
        (build_id, true),
        ("+", false),
        (module, false),
        ("+", false),
        (relative_pc, true),
    ])
}

fn take_token<'a>(value: &mut &'a str) -> Option<&'a str> {
    *value = value.trim_start();
    if value.is_empty() {
        return None;
    }
    let end = value.find(char::is_whitespace).unwrap_or(value.len());
    let token = &value[..end];
    *value = &value[end..];
    Some(token)
}

fn first_symbol(mut value: &str) -> Option<&str> {
    loop {
        let start = value.find('(')?;
        value = &value[start + 1..];
        let end = value.find(')')?;
        let candidate = &value[..end];
        if !candidate.starts_with("offset ") && !candidate.starts_with("BuildId: ") {
            return Some(candidate);
        }
        value = &value[end + 1..];
    }
}

fn strip_symbol_offset(symbol: &str) -> &str {
    let Some((base, offset)) = symbol.rsplit_once('+') else {
        return symbol;
    };
    let offset = offset.strip_prefix("0x").unwrap_or(offset);
    if !offset.is_empty() && offset.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        base
    } else {
        symbol
    }
}

fn parse_build_id(value: &str) -> Option<&str> {
    let value = value.split_once("(BuildId: ")?.1;
    let build_id = value.split_once(')')?.0;
    (!build_id.is_empty() && build_id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(build_id)
}

fn build_frame_token(parts: &[(&str, bool)]) -> Option<FixedText<MAX_FRAME_TOKEN>> {
    let total = parts.iter().map(|(part, _)| part.len()).sum::<usize>();
    if total == 0 || total > MAX_FRAME_TOKEN {
        return None;
    }
    let mut bytes = [0u8; MAX_FRAME_TOKEN];
    let mut written = 0usize;
    for (part, lowercase) in parts {
        for byte in part.bytes() {
            bytes[written] = if *lowercase {
                byte.to_ascii_lowercase()
            } else {
                byte
            };
            written += 1;
        }
    }
    let value = std::str::from_utf8(&bytes[..written]).ok()?;
    let mut token = FixedText::default();
    token.set(value).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_line_ref;
    use crate::problems::{
        BoundaryFlags, InputCoverage, LineProvenance, OutcomeFlags, RangeCompleteness,
    };

    const SEPARATOR: &str = "*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***";

    fn observed(line: u32, raw: &str) -> ObservedLine<'_> {
        ObservedLine::new(
            line,
            raw.as_bytes(),
            parse_line_ref(raw),
            LineProvenance::Unknown,
            InputCoverage::static_file(RangeCompleteness::Bounded),
        )
    }

    fn feed(recognizer: &mut NativeRecognizer, line: u32, raw: &str) -> Option<RecognizedProblem> {
        recognizer.observe(&observed(line, raw));
        recognizer.pop_ready()
    }

    fn finish(recognizer: &mut NativeRecognizer) -> Option<RecognizedProblem> {
        recognizer.finish_input();
        recognizer.pop_ready()
    }

    #[test]
    fn libc_signal_accepts_only_enumerated_aosp_suffix_shapes() {
        let cases = [
            (
                "07-26 12:00:00.001  321  322 F libc: Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0 in tid 322 (RenderThread)",
                None,
            ),
            (
                "07-26 12:00:00.001  777  778 F libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 778 (worker)",
                None,
            ),
            (
                "07-26 12:00:00.001  777  778 F libc: Fatal signal 6 (SIGABRT), code -6 (SI_TKILL from pid 42, uid 0) in tid 778 (worker), pid 777 (com.example.native)",
                Some("com.example.native"),
            ),
            (
                "07-26 12:00:00.001  901  902 F libc: Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall 172 in tid 902 (seccomp-worker), pid 901 (com.example.seccomp)",
                Some("com.example.seccomp"),
            ),
            (
                "07-26 12:00:00.001  901  902 F libc: Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall -2147483648 in tid 902 (seccomp-worker), pid 901 (com.example.seccomp)",
                Some("com.example.seccomp"),
            ),
            (
                "07-26 12:00:00.001  901  902 F libc: Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall 0 in tid 902 (seccomp-worker), pid 901 (com.example.seccomp)",
                Some("com.example.seccomp"),
            ),
            (
                "07-26 12:00:00.001  901  902 F libc: Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall 2147483647 in tid 902 (seccomp-worker), pid 901 (com.example.seccomp)",
                Some("com.example.seccomp"),
            ),
            (
                "07-26 12:00:00.001  902  902 F libc: Fatal signal 7 (SIGBUS), code 1 (BUS_ADRALN), fault addr 00000000",
                None,
            ),
            (
                "07-26 12:00:00.001  903  904 F libc: Fatal signal 8 (SIGFPE), code 1 (FPE_INTDIV) in tid 904 (math-worker), pid 903 (com.example.math)",
                Some("com.example.math"),
            ),
            (
                "07-26 12:00:00.001  905  906 F libc: Fatal signal 4 (SIGILL), code 1 (ILL_ILLOPC), fault addr 0x1234 in tid 906 (Jit thread pool), pid 905 (com.example.jit)",
                Some("com.example.jit"),
            ),
        ];

        for (raw, expected_process) in cases {
            let signal = recognize_libc_signal(&observed(0, raw))
                .unwrap_or_else(|| panic!("expected strict AOSP libc signal: {raw}"));
            assert_eq!(signal.explicit_process_ref(), expected_process);
        }
    }

    #[test]
    fn libc_signal_rejects_incomplete_or_unconsumed_suffixes() {
        let malformed = [
            "Fatal signal 11 (SIGSEGV)",
            "Fatal signal 11 (SIGSEGV), code 1",
            "Fatal signal 11 (SIGSEGV), code 1 SEGV_MAPERR",
            "Fatal signal 11 (SIGSEGV), code nope (SEGV_MAPERR)",
            "Fatal signal 11 (SIGSEGV), code 2147483648 (SEGV_MAPERR)",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) arbitrary garbage",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x1 trailing",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) in tid nope (worker)",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) in tid 322",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) in tid 322 (worker) junk",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), pid 321 (com.example)",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) in tid 322 (worker), pid 321",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) in tid 322 (worker), pid 321 (com.example) junk",
            "Fatal signal 11 (SIGABRT), code 1 (SEGV_MAPERR)",
            "Fatal signal 11 (SIGSEGV), code 1 (BUS_ADRALN)",
            "Fatal signal 6 (SIGABRT), code -6 (SI_TKILL from pid nope, uid 1000)",
            "Fatal signal 6 (SIGABRT), code -6 (SI_TKILL from pid 42, uid nope)",
            "Fatal signal 6 (SIGABRT), code -6 (anything goes)",
            "Fatal signal 11 (SIGSEGV), code 999 (SEGV_MAPERR)",
            "Fatal signal 11 (SIGSEGV), code 1 (SI_TKILL)",
            "Fatal signal 31 (SIGSYS), code 2 (SYS_SECCOMP), syscall 172 in tid 322 (worker)",
            "Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall",
            "Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall -2147483649 in tid 322 (worker)",
            "Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall 2147483648 in tid 322 (worker)",
            "Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall 172",
            "Fatal signal 31 (SIGSYS), code 1 (SYS_SECCOMP), syscall 172 in tid 322 (worker) junk",
            "Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), syscall 172 in tid 322 (worker)",
        ];

        for message in malformed {
            let raw = format!("07-26 12:00:00.001  321  322 F libc: {message}");
            assert!(
                recognize_libc_signal(&observed(0, &raw)).is_none(),
                "malformed suffix must not be partially accepted: {message}"
            );
        }
    }

    #[test]
    fn libc_signal_rejects_explicit_pid_that_disagrees_with_header() {
        let raw = "07-26 12:00:00.001  321  322 F libc: Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR) in tid 322 (worker), pid 999 (com.example)";
        assert!(recognize_libc_signal(&observed(0, raw)).is_none());
    }

    #[test]
    fn libc_signal_fingerprint_includes_si_code_but_not_dynamic_suffix_fields() {
        let process = ProcessFingerprintKey::new(Some("com.example.native"));
        let fingerprint = |raw: &str| {
            recognize_libc_signal(&observed(0, raw))
                .expect("valid libc fatal signal")
                .into_problem(&process, ProcessInstanceKey(1))
                .group_key
                .fingerprint()
        };
        let base = fingerprint(
            "07-26 12:00:00.001  777  778 F libc: Fatal signal 6 (SIGABRT), code -6 (SI_TKILL) in tid 778 (worker)",
        );
        let same_code_different_facets = fingerprint(
            "07-26 12:00:00.001  777  779 F libc: Fatal signal 6 (SIGABRT), code -6 (SI_TKILL from pid 42, uid 1000) in tid 779 (other), pid 777 (com.example.native)",
        );
        let different_code = fingerprint(
            "07-26 12:00:00.001  777  778 F libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 778 (worker)",
        );

        assert_eq!(base, same_code_different_facets);
        assert_ne!(base, different_code);
    }

    fn complete_raw(
        recognizer: &mut NativeRecognizer,
        pid: u32,
        symbol_offset: &str,
        fault_address: &str,
    ) -> RecognizedProblem {
        let pid_line = format!(
            "pid: {pid}, tid: {}, name: worker  >>> com.example.native <<<",
            pid + 1
        );
        let signal_line =
            format!("signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr {fault_address}");
        let frame = format!(
            "#00 pc 0000000000012345  /data/app/libfoo.so (native_crash+{symbol_offset}) (BuildId: aabbccdd)"
        );
        for (line, text) in [
            (0, SEPARATOR),
            (1, pid_line.as_str()),
            (2, signal_line.as_str()),
            (3, "backtrace:"),
            (4, frame.as_str()),
        ] {
            assert!(feed(recognizer, line, text).is_none());
        }
        feed(recognizer, 5, "memory map:").expect("complete tombstone should commit")
    }

    #[test]
    fn recognizes_complete_raw_tombstone_and_keeps_death_unknown() {
        let mut recognizer = NativeRecognizer::new();
        let problem = complete_raw(&mut recognizer, 123, "16", "0xdeadbeef");

        assert_eq!(problem.draft.pid, 123);
        assert_eq!(problem.draft.start_line, 0);
        assert_eq!(problem.draft.end_line, 4);
        assert_eq!(problem.draft.anchor_line, 2);
        assert!(!problem.draft.outcome.contains(OutcomeFlags::DEATH_OBSERVED));
    }

    #[test]
    fn recognizes_every_line_with_threadtime_prefix_and_cmdline_identity() {
        let mut recognizer = NativeRecognizer::new();
        for (line, payload) in [
            (0, SEPARATOR),
            (1, "pid: 321, tid: 322, name: worker"),
            (2, "Cmdline: com.example.native"),
            (
                3,
                "signal 6 (SIGABRT), code -1 (SI_QUEUE), fault addr --------",
            ),
            (4, "backtrace:"),
            (
                5,
                "#00 pc 0000000000001234  /system/lib64/libfoo.so (abort_now+24)",
            ),
        ] {
            let raw = format!("07-26 12:00:00.00{line}   99   99 F DEBUG: {payload}");
            assert!(feed(&mut recognizer, line, &raw).is_none());
        }
        let stop = "07-26 12:00:00.009   99   99 F DEBUG: memory map:";
        let problem = feed(&mut recognizer, 6, stop).expect("prefixed tombstone should commit");
        assert_eq!(problem.draft.pid, 321);
    }

    #[test]
    fn explicit_recoverable_is_an_outcome_fact_not_a_death_claim() {
        let mut recognizer = NativeRecognizer::new();
        for (line, text) in [
            (0, SEPARATOR),
            (
                1,
                "pid: 123, tid: 124, name: worker  >>> com.example.native <<<",
            ),
            (
                2,
                "signal 11 (SIGSEGV), code 2 (SEGV_ACCERR), fault addr 0x1",
            ),
            (3, "recoverable: true"),
        ] {
            assert!(feed(&mut recognizer, line, text).is_none());
        }
        let problem = finish(&mut recognizer).expect("minimum grammar is met");
        assert!(problem
            .draft
            .outcome
            .contains(OutcomeFlags::EXPLICITLY_RECOVERABLE));
        assert!(!problem.draft.outcome.contains(OutcomeFlags::DEATH_OBSERVED));
    }

    #[test]
    fn requested_dump_signal_catcher_nonfatal_and_missing_process_do_not_commit() {
        for forbidden in [
            Some("Requested dump of process 123"),
            Some("Signal Catcher"),
            None,
        ] {
            let mut recognizer = NativeRecognizer::new();
            for (line, text) in [
                (0, SEPARATOR),
                (1, "pid: 123, tid: 124, name: worker"),
                (2, "Cmdline: com.example.native"),
                (
                    3,
                    if forbidden.is_none() {
                        "signal 35 (BIONIC_SIGNAL_DEBUGGER), code -1 (SI_QUEUE)"
                    } else {
                        "signal 11 (SIGSEGV), code 1 (SEGV_MAPERR)"
                    },
                ),
            ] {
                assert!(feed(&mut recognizer, line, text).is_none());
            }
            if let Some(marker) = forbidden {
                assert!(feed(&mut recognizer, 4, marker).is_none());
            }
            assert!(finish(&mut recognizer).is_none());
        }

        let mut missing_process = NativeRecognizer::new();
        for (line, text) in [
            (0, SEPARATOR),
            (1, "pid: 123, tid: 124, name: com.example.native"),
            (2, "signal 11 (SIGSEGV), code 1 (SEGV_MAPERR)"),
        ] {
            assert!(feed(&mut missing_process, line, text).is_none());
        }
        assert!(finish(&mut missing_process).is_none());
    }

    #[test]
    fn absolute_addresses_pid_and_symbol_offsets_do_not_change_group() {
        let left = complete_raw(&mut NativeRecognizer::new(), 123, "0x10", "0x11111111");
        let right = complete_raw(&mut NativeRecognizer::new(), 987, "0x88", "0x99999999");
        assert_eq!(left.group_key.fingerprint(), right.group_key.fingerprint());
    }

    #[test]
    fn input_end_commits_only_satisfied_minimum_grammar_as_truncated() {
        let mut complete = NativeRecognizer::new();
        for (line, text) in [
            (0, SEPARATOR),
            (
                1,
                "pid: 123, tid: 124, name: worker  >>> com.example.native <<<",
            ),
            (2, "signal 7 (SIGBUS), code 1 (BUS_ADRALN)"),
        ] {
            assert!(feed(&mut complete, line, text).is_none());
        }
        let problem = finish(&mut complete).expect("minimum grammar is met");
        assert!(problem
            .draft
            .boundary
            .contains(BoundaryFlags::TRUNCATED_BY_INPUT));

        let mut incomplete = NativeRecognizer::new();
        assert!(feed(&mut incomplete, 0, SEPARATOR).is_none());
        assert!(feed(
            &mut incomplete,
            1,
            "pid: 123, tid: 124, name: worker  >>> com.example.native <<<"
        )
        .is_none());
        assert!(finish(&mut incomplete).is_none());
    }

    #[test]
    fn frame_and_line_limits_are_bounded_and_deterministic() {
        let mut recognizer = NativeRecognizer::new();
        for (line, text) in [
            (0, SEPARATOR),
            (
                1,
                "pid: 123, tid: 124, name: worker  >>> com.example.native <<<",
            ),
            (2, "signal 5 (SIGTRAP), code 1 (TRAP_BRKPT)"),
            (3, "backtrace:"),
            (4, "#00 pc 00000001 /system/lib64/liba.so (first+1)"),
            (5, "#01 pc 00000002 /system/lib64/libb.so (second+2)"),
            (6, "#02 pc 00000003 /system/lib64/libc.so (third+3)"),
            (7, "#03 pc 00000004 /system/lib64/libd.so (fourth+4)"),
        ] {
            assert!(feed(&mut recognizer, line, text).is_none());
        }
        let problem = feed(&mut recognizer, 8, "memory map:").expect("bounded frames still commit");
        assert!(problem
            .draft
            .boundary
            .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED));

        let mut oversized = NativeRecognizer::new();
        assert!(feed(&mut oversized, 0, SEPARATOR).is_none());
        let huge = "x".repeat(MAX_NATIVE_LINE_BYTES + 1);
        assert!(feed(&mut oversized, 1, &huge).is_none());
        assert!(finish(&mut oversized).is_none());

        let mut line_limited = NativeRecognizer::new();
        for (line, text) in [
            (0, SEPARATOR),
            (
                1,
                "pid: 123, tid: 124, name: worker  >>> com.example.native <<<",
            ),
            (2, "signal 5 (SIGTRAP), code 1 (TRAP_BRKPT)"),
        ] {
            assert!(feed(&mut line_limited, line, text).is_none());
        }
        let problem = feed(&mut line_limited, MAX_NATIVE_LINES + 1, "limit boundary")
            .expect("satisfied grammar commits at the line limit");
        assert!(problem
            .draft
            .boundary
            .contains(BoundaryFlags::TRUNCATED_BY_LIMIT));
        assert!(!problem
            .draft
            .boundary
            .contains(BoundaryFlags::TRUNCATED_BY_INPUT));
    }

    #[test]
    fn frame_canonicalization_prefers_symbol_and_preserves_module_case() {
        let symbolized = normalize_native_frame(
            "#00 pc 0000000000001234 /data/app/libFoo.so (DoWork+0x88) (BuildId: AABBCC)",
        )
        .unwrap();
        assert_eq!(symbolized.as_str(), "/data/app/libFoo.so#DoWork");

        let unsymbolized =
            normalize_native_frame("#00 pc 000000000000ABCD /data/app/libFoo.so (BuildId: AABBCC)")
                .unwrap();
        assert_eq!(
            unsymbolized.as_str(),
            "aabbcc+/data/app/libFoo.so+000000000000abcd"
        );
    }

    #[test]
    fn recognizer_pending_state_has_no_heap_owned_fields() {
        assert!(!std::mem::needs_drop::<NativePending>());
        assert!(std::mem::size_of::<NativeRecognizer>() < 2 * 1024);
    }
}
