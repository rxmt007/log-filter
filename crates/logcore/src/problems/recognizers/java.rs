use super::{parse_pid, trim_ascii, FixedText, ProblemRecognizer, MAX_PHYSICAL_LINE_BYTES};
use crate::problems::engine::{ObservedLine, RecognizedProblem};
use crate::problems::EventLogRecord;
use crate::problems::{
    normalize_java_frame, normalize_java_throwable, parse_event_log, parse_log_timestamp,
    BoundaryFlags, EvidenceAdmission, EvidenceFlags, EvidenceFormat, EvidencePriority,
    FingerprintBuilder, FingerprintTokenKind, GroupKey, LineProvenance, NormalizedToken,
    ObservationCandidate, ObservationRef, ObservationRole, OutcomeFlags, PackedLogTimestamp,
    ProblemEventDraft, ProblemKind, ProcessFingerprintKey, ProcessInstanceKey, RuleId,
    SignatureQuality,
};
use std::collections::VecDeque;

const MAX_ACTIVE_JAVA: usize = 32;
const MAX_JAVA_LINES: u32 = 512;
const MAX_JAVA_BYTES: usize = 128 * 1024;
const MAX_UNMATCHED: u8 = 32;
const MAX_PROCESS_NAME: usize = 256;
const MAX_EXCEPTION_TYPE: usize = 256;
const MAX_FRAME: usize = 256;
const MAX_FRAMES: usize = 3;
const FINGERPRINT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JavaEnvelope {
    Normal,
    System,
}

#[derive(Debug, Clone, Copy)]
struct EvidencePoint {
    line: u32,
    provenance: LineProvenance,
}

#[derive(Debug)]
struct JavaPending {
    candidate_id: u64,
    envelope: JavaEnvelope,
    producer_pid: u32,
    start: EvidencePoint,
    anchor_timestamp: PackedLogTimestamp,
    last_evidence_line: u32,
    last_touched_line: u32,
    bytes_seen: usize,
    unmatched: u8,
    process_name: FixedText<MAX_PROCESS_NAME>,
    process_point: Option<EvidencePoint>,
    payload_pid_matches: bool,
    malformed_pid: bool,
    exception_type: FixedText<MAX_EXCEPTION_TYPE>,
    exception_point: Option<EvidencePoint>,
    oom_support_point: Option<EvidencePoint>,
    saw_oome: bool,
    frames: [FixedText<MAX_FRAME>; MAX_FRAMES],
    frame_points: [Option<EvidencePoint>; MAX_FRAMES],
    frame_count: u8,
}

impl JavaPending {
    fn new(
        candidate_id: u64,
        envelope: JavaEnvelope,
        producer_pid: u32,
        line: &ObservedLine<'_>,
    ) -> Self {
        let point = EvidencePoint {
            line: line.line,
            provenance: line.provenance,
        };
        Self {
            candidate_id,
            envelope,
            producer_pid,
            start: point,
            anchor_timestamp: parse_log_timestamp(line.parsed.date, line.parsed.time)
                .unwrap_or_default(),
            last_evidence_line: line.line,
            last_touched_line: line.line,
            bytes_seen: line.raw.len(),
            unmatched: 0,
            process_name: FixedText::default(),
            process_point: None,
            payload_pid_matches: false,
            malformed_pid: false,
            exception_type: FixedText::default(),
            exception_point: None,
            oom_support_point: None,
            saw_oome: false,
            frames: [FixedText::default(); MAX_FRAMES],
            frame_points: [None; MAX_FRAMES],
            frame_count: 0,
        }
    }

    fn minimum_grammar_met(&self) -> bool {
        if self.malformed_pid || self.exception_point.is_none() {
            return false;
        }
        match self.envelope {
            JavaEnvelope::Normal => {
                self.payload_pid_matches
                    && self.process_point.is_some()
                    && !self.process_name.is_empty()
            }
            JavaEnvelope::System => true,
        }
    }

    fn record_message(&mut self, line: &ObservedLine<'_>, message: &str) -> bool {
        if let Some((name, payload_pid)) = parse_process_identity(message) {
            if self.envelope == JavaEnvelope::Normal {
                if payload_pid != self.producer_pid {
                    self.malformed_pid = true;
                } else if self.process_name.set(name) {
                    self.payload_pid_matches = true;
                    self.process_point = Some(EvidencePoint {
                        line: line.line,
                        provenance: line.provenance,
                    });
                }
            }
            self.touch_evidence(line.line);
            return true;
        }

        if let Some(throwable) = parse_throwable(message) {
            let adopt = !throwable.suppressed || self.exception_type.is_empty();
            if adopt && self.exception_type.set(throwable.class_name.as_str()) {
                self.exception_point = Some(EvidencePoint {
                    line: line.line,
                    provenance: line.provenance,
                });
                self.saw_oome |= is_out_of_memory_error(throwable.class_name.as_str());
                if throwable.resets_frames {
                    self.frame_count = 0;
                    self.frames.iter_mut().for_each(FixedText::clear);
                    self.frame_points.fill(None);
                }
            }
            self.touch_evidence(line.line);
            return true;
        }

        if let Some(frame) = normalize_frame(message) {
            if usize::from(self.frame_count) < MAX_FRAMES {
                let index = usize::from(self.frame_count);
                if self.frames[index].set(frame.as_str()) {
                    self.frame_points[index] = Some(EvidencePoint {
                        line: line.line,
                        provenance: line.provenance,
                    });
                    self.frame_count += 1;
                }
            }
            self.touch_evidence(line.line);
            return true;
        }

        if is_elided_frame(message) {
            self.touch_evidence(line.line);
            return true;
        }
        false
    }

    fn touch_evidence(&mut self, line: u32) {
        self.last_evidence_line = line;
        self.last_touched_line = line;
        self.unmatched = 0;
    }

    fn into_problem(self, limited: bool) -> Option<RecognizedProblem> {
        if !self.minimum_grammar_met() {
            return None;
        }

        let kind = if self.saw_oome {
            ProblemKind::JavaOom
        } else {
            ProblemKind::JavaCrash
        };
        let rule = if kind == ProblemKind::JavaOom {
            RuleId::JavaOomV1
        } else {
            RuleId::JavaUncaughtV1
        };
        let process_name = match self.envelope {
            JavaEnvelope::Normal => Some(self.process_name.as_str()),
            JavaEnvelope::System => None,
        };
        let process = ProcessFingerprintKey::new(process_name);
        let identity_quality = process.identity_quality();
        let signature_quality = if self.frame_count > 0 {
            SignatureQuality::FullStack
        } else {
            SignatureQuality::TypeOnly
        };
        let mut fingerprint = FingerprintBuilder::new(
            kind,
            FINGERPRINT_VERSION,
            signature_quality,
            identity_quality,
            &process,
        );
        fingerprint.token(
            FingerprintTokenKind::ExceptionType,
            self.exception_type.as_str().as_bytes(),
        );
        for frame in self.frames.iter().take(usize::from(self.frame_count)) {
            fingerprint.token(FingerprintTokenKind::Frame, frame.as_str().as_bytes());
        }
        let fingerprint = fingerprint.finish();
        let group_key = GroupKey::new(
            kind,
            FINGERPRINT_VERSION,
            signature_quality,
            identity_quality,
            fingerprint,
        );

        let mut boundary = BoundaryFlags::NONE;
        if limited {
            // The current compact model has one generic truncation bit. It
            // records that source evidence ended before a natural boundary.
            boundary.insert(BoundaryFlags::TRUNCATED_BY_INPUT);
        }
        let mut evidence = EvidenceFlags::PRIMARY;
        if self.last_evidence_line > self.start.line {
            evidence.insert(EvidenceFlags::MULTILINE);
        }
        let draft = ProblemEventDraft {
            start_line: self.start.line,
            end_line: self.last_evidence_line,
            anchor_line: self.start.line,
            anchor_timestamp: self.anchor_timestamp,
            pid: self.producer_pid,
            process_instance: ProcessInstanceKey(0),
            kind,
            evidence,
            outcome: OutcomeFlags::NONE,
            boundary,
        };
        let primary = observation(
            self.start,
            rule,
            ObservationRole::Primary,
            EvidencePriority::MinimumGrammar,
            EvidenceFormat::AospText,
        );
        let mut problem = RecognizedProblem::new(draft, group_key, primary);
        if let Some(point) = self.process_point {
            problem.push_observation(observation(
                point,
                rule,
                ObservationRole::ProcessIdentity,
                EvidencePriority::MinimumGrammar,
                EvidenceFormat::AospText,
            ));
        }
        if let Some(point) = self.exception_point {
            problem.push_observation(observation(
                point,
                rule,
                ObservationRole::ExceptionType,
                EvidencePriority::MinimumGrammar,
                EvidenceFormat::AospText,
            ));
        }
        for point in self
            .frame_points
            .iter()
            .take(usize::from(self.frame_count))
            .flatten()
            .copied()
        {
            problem.push_observation(observation(
                point,
                rule,
                ObservationRole::StackFrame,
                EvidencePriority::Supporting,
                EvidenceFormat::AospText,
            ));
        }
        if let Some(point) = self.oom_support_point {
            problem.push_observation(observation(
                point,
                rule,
                ObservationRole::Supporting,
                EvidencePriority::Supporting,
                EvidenceFormat::AospText,
            ));
        }
        Some(problem)
    }
}

#[derive(Debug)]
pub(crate) struct JavaRecognizer {
    pending: Vec<JavaPending>,
    ready: VecDeque<RecognizedProblem>,
    next_candidate_id: u64,
}

impl Default for JavaRecognizer {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(MAX_ACTIVE_JAVA),
            ready: VecDeque::with_capacity(MAX_ACTIVE_JAVA),
            next_candidate_id: 1,
        }
    }
}

impl ProblemRecognizer for JavaRecognizer {
    fn observe(&mut self, line: &ObservedLine<'_>) {
        self.expire_before(line);
        if line.raw.len() > MAX_PHYSICAL_LINE_BYTES {
            self.mark_unmatched_all();
            return;
        }

        if let Some(problem) = recognize_am_crash(line) {
            self.ready.push_back(problem);
            return;
        }

        if let Some(producer_pid) = runtime_oom_support(line) {
            if let Some(pending) = self
                .pending
                .iter_mut()
                .find(|pending| pending.producer_pid == producer_pid)
            {
                pending.oom_support_point = Some(EvidencePoint {
                    line: line.line,
                    provenance: line.provenance,
                });
                pending.saw_oome = true;
                pending.touch_evidence(line.line);
                return;
            }
        }

        if let Some((envelope, producer_pid)) = java_start(line) {
            if let Some(index) = self
                .pending
                .iter()
                .position(|pending| pending.producer_pid == producer_pid)
            {
                self.finalize(index, false);
            } else if self.pending.len() == MAX_ACTIVE_JAVA {
                let index = self.oldest_pending_index();
                self.finalize(index, true);
            }
            let candidate_id = self.next_candidate_id;
            self.next_candidate_id = self.next_candidate_id.wrapping_add(1).max(1);
            self.pending
                .push(JavaPending::new(candidate_id, envelope, producer_pid, line));
            return;
        }

        let message = line.parsed.message;
        if line.parsed.tag == "AndroidRuntime" {
            let Some(producer_pid) = parse_pid(line.parsed.pid) else {
                self.mark_unmatched_all();
                return;
            };
            if let Some(pending) = self
                .pending
                .iter_mut()
                .find(|pending| pending.producer_pid == producer_pid)
            {
                if !pending.record_message(line, message) {
                    pending.unmatched = pending.unmatched.saturating_add(1);
                }
                self.finalize_exhausted_unmatched();
                return;
            }
        } else if line.parsed.tag.is_empty() && is_java_continuation(message) {
            let mut compatible_index = None;
            let mut ambiguous = false;
            for (index, pending) in self.pending.iter().enumerate() {
                if !message_compatible(pending, message) {
                    continue;
                }
                if compatible_index.is_some() {
                    ambiguous = true;
                    break;
                }
                compatible_index = Some(index);
            }
            if !ambiguous {
                if let Some(index) = compatible_index {
                    self.pending[index].record_message(line, message);
                    return;
                }
            }
        }

        self.mark_unmatched_all();
        self.finalize_exhausted_unmatched();
    }

    fn finish_input(&mut self) {
        while !self.pending.is_empty() {
            let index = self.earliest_pending_index();
            self.finalize(index, true);
        }
    }

    fn reset(&mut self) {
        self.pending.clear();
        self.ready.clear();
        self.next_candidate_id = 1;
    }

    fn pop_ready(&mut self) -> Option<RecognizedProblem> {
        self.ready.pop_front()
    }
}

impl JavaRecognizer {
    fn expire_before(&mut self, line: &ObservedLine<'_>) {
        let mut index = 0;
        while index < self.pending.len() {
            let pending = &mut self.pending[index];
            pending.bytes_seen = pending.bytes_seen.saturating_add(line.raw.len());
            let line_limit = line.line.saturating_sub(pending.start.line) >= MAX_JAVA_LINES;
            let byte_limit = pending.bytes_seen > MAX_JAVA_BYTES;
            if line_limit || byte_limit {
                self.finalize(index, true);
            } else {
                index += 1;
            }
        }
    }

    fn mark_unmatched_all(&mut self) {
        for pending in &mut self.pending {
            pending.unmatched = pending.unmatched.saturating_add(1);
        }
    }

    fn finalize_exhausted_unmatched(&mut self) {
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending[index].unmatched > MAX_UNMATCHED {
                self.finalize(index, true);
            } else {
                index += 1;
            }
        }
    }

    fn oldest_pending_index(&self) -> usize {
        self.pending
            .iter()
            .enumerate()
            .min_by_key(|(_, pending)| (pending.last_touched_line, pending.candidate_id))
            .map(|(index, _)| index)
            .expect("oldest_pending_index is called only for a non-empty set")
    }

    fn earliest_pending_index(&self) -> usize {
        self.pending
            .iter()
            .enumerate()
            .min_by_key(|(_, pending)| (pending.start.line, pending.candidate_id))
            .map(|(index, _)| index)
            .expect("earliest_pending_index is called only for a non-empty set")
    }

    fn finalize(&mut self, index: usize, limited: bool) {
        let pending = self.pending.remove(index);
        if let Some(problem) = pending.into_problem(limited) {
            self.ready.push_back(problem);
        }
    }
}

fn java_start(line: &ObservedLine<'_>) -> Option<(JavaEnvelope, u32)> {
    if line.parsed.tag != "AndroidRuntime" || !matches!(line.parsed.level, "E" | "F") {
        return None;
    }
    let producer_pid = parse_pid(line.parsed.pid)?;
    let message = line.parsed.message;
    if message
        .strip_prefix("*** FATAL EXCEPTION IN SYSTEM PROCESS:")
        .is_some_and(|thread| !trim_ascii(thread).is_empty())
    {
        return Some((JavaEnvelope::System, producer_pid));
    }
    if message
        .strip_prefix("FATAL EXCEPTION:")
        .is_some_and(|thread| !trim_ascii(thread).is_empty())
    {
        return Some((JavaEnvelope::Normal, producer_pid));
    }
    None
}

fn parse_process_identity(message: &str) -> Option<(&str, u32)> {
    let rest = message.strip_prefix("Process: ")?;
    let (name, pid) = rest.rsplit_once(", PID: ")?;
    let name = trim_ascii(name);
    if name.is_empty() {
        return None;
    }
    Some((name, parse_pid(trim_ascii(pid))?))
}

#[derive(Debug, Clone, Copy)]
struct Throwable {
    class_name: NormalizedToken,
    resets_frames: bool,
    suppressed: bool,
}

fn parse_throwable(message: &str) -> Option<Throwable> {
    let message = trim_ascii(message);
    let resets_frames = message.starts_with("Caused by: ");
    let suppressed = message.starts_with("Suppressed: ");
    let class_name = normalize_java_throwable(message.as_bytes())?;
    Some(Throwable {
        class_name,
        resets_frames,
        suppressed,
    })
}

fn is_out_of_memory_error(class_name: &str) -> bool {
    class_name
        .rsplit('.')
        .next()
        .is_some_and(|name| name == "OutOfMemoryError")
}

fn normalize_frame(message: &str) -> Option<NormalizedToken> {
    normalize_java_frame(message.as_bytes())
}

fn is_elided_frame(message: &str) -> bool {
    let value = trim_ascii(message);
    let Some(rest) = value.strip_prefix("... ") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(" more") else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_java_continuation(message: &str) -> bool {
    parse_process_identity(message).is_some()
        || parse_throwable(message).is_some()
        || normalize_frame(message).is_some()
        || is_elided_frame(message)
}

fn message_compatible(pending: &JavaPending, message: &str) -> bool {
    if let Some((_, pid)) = parse_process_identity(message) {
        return pending.envelope == JavaEnvelope::Normal && pid == pending.producer_pid;
    }
    parse_throwable(message).is_some()
        || normalize_frame(message).is_some()
        || is_elided_frame(message)
}

fn runtime_oom_support(line: &ObservedLine<'_>) -> Option<u32> {
    if !matches!(line.parsed.tag, "art" | "dalvikvm") {
        return None;
    }
    let message = trim_ascii(line.parsed.message);
    let strict = message
        .strip_prefix("Throwing OutOfMemoryError")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', ':']))
        || message
            .strip_prefix("java.lang.OutOfMemoryError")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', ':']));
    strict.then(|| parse_pid(line.parsed.pid)).flatten()
}

fn recognize_am_crash(line: &ObservedLine<'_>) -> Option<RecognizedProblem> {
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
    if crash.exception == "Native crash" {
        return None;
    }
    let exception = normalize_java_throwable(crash.exception.as_bytes())?;
    let kind = if is_out_of_memory_error(exception.as_str()) {
        ProblemKind::JavaOom
    } else {
        ProblemKind::JavaCrash
    };
    let process = ProcessFingerprintKey::new(Some(crash.process_name));
    let identity_quality = process.identity_quality();
    let signature_quality = if crash.file.is_empty() {
        SignatureQuality::TypeOnly
    } else {
        SignatureQuality::TypeFile
    };
    let mut fingerprint = FingerprintBuilder::new(
        kind,
        FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        &process,
    );
    fingerprint.token(FingerprintTokenKind::ExceptionType, exception.as_bytes());
    if !crash.file.is_empty() {
        let file = crash
            .file
            .rsplit(['/', '\\'])
            .next()
            .filter(|file| !file.is_empty())?;
        fingerprint.token(FingerprintTokenKind::Frame, file.as_bytes());
    }
    let group_key = GroupKey::new(
        kind,
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
        anchor_timestamp: PackedLogTimestamp::UNKNOWN,
        pid: crash.pid,
        process_instance: ProcessInstanceKey(0),
        kind,
        evidence: EvidenceFlags::PRIMARY | EvidenceFlags::STRUCTURED,
        outcome,
        boundary: BoundaryFlags::NONE,
    };
    let point = EvidencePoint {
        line: line.line,
        provenance: line.provenance,
    };
    let mut problem = RecognizedProblem::new(
        draft,
        group_key,
        observation(
            point,
            RuleId::ManagedAmCrashV1,
            ObservationRole::Primary,
            EvidencePriority::MinimumGrammar,
            EvidenceFormat::EventLogShapedText,
        ),
    );
    problem.push_observation(observation(
        point,
        RuleId::ManagedAmCrashV1,
        ObservationRole::ProcessIdentity,
        EvidencePriority::MinimumGrammar,
        EvidenceFormat::EventLogShapedText,
    ));
    Some(problem)
}

fn observation(
    point: EvidencePoint,
    rule: RuleId,
    role: ObservationRole,
    priority: EvidencePriority,
    format: EvidenceFormat,
) -> ObservationCandidate {
    ObservationCandidate::new(
        ObservationRef::new(point.line, rule, role, format, point.provenance)
            .expect("recognizer rule/role pairs are compile-time contracts"),
        priority,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_line_ref;
    use crate::problems::{BufferSet, InputCoverage, LogBuffer, ProblemEngine, RangeCompleteness};

    fn static_line<'a>(line: u32, raw: &'a str) -> ObservedLine<'a> {
        ObservedLine::new(
            line,
            raw.as_bytes(),
            parse_line_ref(raw),
            LineProvenance::Unknown,
            InputCoverage::static_file(RangeCompleteness::Bounded),
        )
    }

    fn events_line<'a>(line: u32, raw: &'a str, known: bool) -> ObservedLine<'a> {
        ObservedLine::new(
            line,
            raw.as_bytes(),
            parse_line_ref(raw),
            if known {
                LineProvenance::Known(LogBuffer::Events)
            } else {
                LineProvenance::Inferred(LogBuffer::Events)
            },
            InputCoverage::adb_live(BufferSet::EVENTS, RangeCompleteness::Bounded),
        )
    }

    fn feed(engine: &mut ProblemEngine, line: u32, raw: &str) {
        engine.observe(static_line(line, raw));
    }

    fn normal_crash(engine: &mut ProblemEngine, base: u32, pid: u32, exception: &str) {
        feed(
            engine,
            base,
            &format!("07-26 12:00:00.000  {pid}  {pid} E AndroidRuntime: FATAL EXCEPTION: main"),
        );
        feed(
            engine,
            base + 1,
            &format!(
                "07-26 12:00:00.001  {pid}  {pid} E AndroidRuntime: Process: com.example.app, PID: {pid}"
            ),
        );
        feed(
            engine,
            base + 2,
            &format!(
                "07-26 12:00:00.002  {pid}  {pid} E AndroidRuntime: {exception}: changing message 123"
            ),
        );
    }

    #[test]
    fn normal_fatal_requires_matching_process_pid_and_throwable() {
        let mut engine = ProblemEngine::new();
        normal_crash(&mut engine, 0, 111, "java.lang.IllegalStateException");
        feed(
            &mut engine,
            3,
            "07-26 12:00:00.003  111  111 E AndroidRuntime:     at com.example.MainKt.run(Main.kt:42)",
        );
        assert_eq!(engine.stats().stored_occurrence_count, 0);
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(crate::problems::ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::JavaCrash);
        assert_eq!(event.pid(), 111);
        assert_eq!((event.start_line(), event.end_line()), (0, 3));
        assert!(!event.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
    }

    #[test]
    fn incomplete_and_pid_conflicting_normal_envelopes_do_not_commit() {
        let mut missing_process = ProblemEngine::new();
        feed(
            &mut missing_process,
            0,
            "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
        );
        feed(
            &mut missing_process,
            1,
            "07-26 12:00:00.001  111  111 E AndroidRuntime: java.lang.RuntimeException: boom",
        );
        missing_process.finish_input();
        assert_eq!(missing_process.stats().observed_occurrence_count, 0);

        let mut conflict = ProblemEngine::new();
        feed(
            &mut conflict,
            0,
            "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
        );
        feed(
            &mut conflict,
            1,
            "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 222",
        );
        feed(
            &mut conflict,
            2,
            "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.RuntimeException: boom",
        );
        conflict.finish_input();
        assert_eq!(conflict.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn exact_system_envelope_needs_throwable_but_not_process_line() {
        let mut engine = ProblemEngine::new();
        feed(
            &mut engine,
            0,
            "07-26 12:00:00.000  100  100 F AndroidRuntime: *** FATAL EXCEPTION IN SYSTEM PROCESS: main",
        );
        feed(
            &mut engine,
            1,
            "07-26 12:00:00.001  100  100 E AndroidRuntime: java.lang.AssertionError: bad",
        );
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 1);
        assert_eq!(
            engine
                .event(crate::problems::ProblemEventId(0))
                .unwrap()
                .pid(),
            100
        );
    }

    #[test]
    fn oome_is_one_java_oom_occurrence_not_a_second_java_crash() {
        let mut engine = ProblemEngine::new();
        normal_crash(&mut engine, 0, 111, "java.lang.OutOfMemoryError");
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 1);
        assert_eq!(
            engine
                .event(crate::problems::ProblemEventId(0))
                .unwrap()
                .kind(),
            ProblemKind::JavaOom
        );
    }

    #[test]
    fn two_interleaved_android_runtime_producers_do_not_cross_attach() {
        let mut engine = ProblemEngine::new();
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
            ),
            (
                1,
                "07-26 12:00:00.001  222  222 E AndroidRuntime: FATAL EXCEPTION: worker",
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: Process: one.app, PID: 111",
            ),
            (
                3,
                "07-26 12:00:00.003  222  222 E AndroidRuntime: Process: two.app, PID: 222",
            ),
            (
                4,
                "07-26 12:00:00.004  222  222 E AndroidRuntime: java.lang.IllegalArgumentException: two",
            ),
            (
                5,
                "07-26 12:00:00.005  111  111 E AndroidRuntime: java.lang.IllegalStateException: one",
            ),
        ] {
            feed(&mut engine, line, raw);
        }
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 2);
        assert_eq!(
            engine
                .event(crate::problems::ProblemEventId(0))
                .unwrap()
                .pid(),
            111
        );
        assert_eq!(
            engine
                .event(crate::problems::ProblemEventId(1))
                .unwrap()
                .pid(),
            222
        );
    }

    #[test]
    fn known_events_am_crash_commits_but_inferred_events_is_supporting_only() {
        let raw = "07-26 12:00:00.000  55  55 I am_crash: [321,com.example.app,0,java.lang.IllegalStateException,bad state,Example.kt,42]";
        let mut known = ProblemEngine::new();
        known.observe(events_line(0, raw, true));
        assert_eq!(known.stats().stored_occurrence_count, 1);
        let observations = known
            .event_observations(crate::problems::ProblemEventId(0))
            .unwrap();
        assert!(observations
            .iter()
            .all(|fact| fact.provenance() == LineProvenance::Known(LogBuffer::Events)));

        let mut inferred = ProblemEngine::new();
        inferred.observe(events_line(0, raw, false));
        assert_eq!(inferred.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn frame_source_lines_and_messages_do_not_split_the_group() {
        let mut engine = ProblemEngine::new();
        normal_crash(&mut engine, 0, 111, "java.lang.IllegalStateException");
        feed(
            &mut engine,
            3,
            "07-26 12:00:00.003  111  111 E AndroidRuntime: at com.example.MainKt.run(Main.kt:42)",
        );
        normal_crash(&mut engine, 10, 111, "java.lang.IllegalStateException");
        feed(
            &mut engine,
            13,
            "07-26 12:00:00.013  111  111 E AndroidRuntime: at com.example.MainKt.run(Main.kt:999)",
        );
        engine.finish_input();

        let first = engine.event(crate::problems::ProblemEventId(0)).unwrap();
        let second = engine.event(crate::problems::ProblemEventId(1)).unwrap();
        assert_eq!(first.group_id_raw(), second.group_id_raw());
    }

    #[test]
    fn complete_candidate_commits_at_line_limit_but_incomplete_candidate_drops() {
        let mut complete = ProblemEngine::new();
        normal_crash(&mut complete, 0, 111, "java.lang.IllegalStateException");
        feed(
            &mut complete,
            512,
            "07-26 12:00:01.000  7  7 I Other: boundary",
        );
        assert_eq!(complete.stats().stored_occurrence_count, 1);

        let mut incomplete = ProblemEngine::new();
        feed(
            &mut incomplete,
            0,
            "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
        );
        feed(
            &mut incomplete,
            512,
            "07-26 12:00:01.000  7  7 I Other: boundary",
        );
        assert_eq!(incomplete.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn pending_java_state_is_a_bounded_fixed_summary_without_owned_log_text() {
        assert!(!std::mem::needs_drop::<JavaPending>());
        assert!(std::mem::size_of::<JavaPending>() < 4 * 1024);
        assert_eq!(MAX_ACTIVE_JAVA, 32);
        assert_eq!(MAX_JAVA_BYTES, 128 * 1024);
    }

    #[test]
    fn runtime_oom_is_supporting_only_and_upgrades_only_a_verified_fatal() {
        let support =
            "07-26 12:00:00.000  111  111 E art: Throwing OutOfMemoryError \"Failed to allocate\"";
        let mut standalone = ProblemEngine::new();
        feed(&mut standalone, 0, support);
        standalone.finish_input();
        assert_eq!(standalone.stats().observed_occurrence_count, 0);

        let mut verified = ProblemEngine::new();
        feed(
            &mut verified,
            0,
            "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
        );
        feed(
            &mut verified,
            1,
            "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
        );
        feed(
            &mut verified,
            2,
            "07-26 12:00:00.002  111  111 E art: Throwing OutOfMemoryError \"Failed to allocate\"",
        );
        feed(
            &mut verified,
            3,
            "07-26 12:00:00.003  111  111 E AndroidRuntime: java.lang.RuntimeException: allocation failed",
        );
        verified.finish_input();
        assert_eq!(verified.stats().stored_occurrence_count, 1);
        assert_eq!(
            verified
                .event(crate::problems::ProblemEventId(0))
                .unwrap()
                .kind(),
            ProblemKind::JavaOom
        );
    }
}
