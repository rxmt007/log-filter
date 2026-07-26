use super::{parse_pid, trim_ascii, FixedText, ProblemRecognizer, MAX_PHYSICAL_LINE_BYTES};
use crate::problems::engine::{ObservedLine, RecognizedProblem};
use crate::problems::{
    normalize_anr_reason, parse_event_log, parse_log_timestamp, AnrReasonCategory, BoundaryFlags,
    EventLogRecord, EvidenceAdmission, EvidenceFlags, EvidenceFormat, EvidencePriority,
    FingerprintBuilder, FingerprintTokenKind, GroupKey, LineProvenance, NormalizedAnrReason,
    ObservationCandidate, ObservationRef, ObservationRole, OutcomeFlags, ProblemEventDraft,
    ProblemKind, ProcessFingerprintKey, ProcessInstanceKey, RuleId, SignatureQuality,
};
use std::collections::VecDeque;

const MAX_ACTIVE_ANR: usize = 16;
const MAX_ANR_LINES: u32 = 512;
const MAX_ANR_BYTES: usize = 256 * 1024;
const MAX_UNMATCHED: u8 = 16;
const MAX_PROCESS_NAME: usize = 256;
const FINGERPRINT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy)]
struct EvidencePoint {
    line: u32,
    provenance: LineProvenance,
}

#[derive(Debug)]
struct AnrPending {
    candidate_id: u64,
    producer_pid: u32,
    victim_process: FixedText<MAX_PROCESS_NAME>,
    victim_pid: Option<u32>,
    victim_pid_point: Option<EvidencePoint>,
    reason: Option<NormalizedAnrReason>,
    reason_point: Option<EvidencePoint>,
    conflicting_victim_pid: bool,
    start: EvidencePoint,
    anchor_timestamp: crate::problems::PackedLogTimestamp,
    last_evidence_line: u32,
    last_touched_line: u32,
    bytes_seen: usize,
    unmatched: u8,
}

impl AnrPending {
    fn new(
        candidate_id: u64,
        producer_pid: u32,
        victim_process: &str,
        line: &ObservedLine<'_>,
    ) -> Option<Self> {
        let mut process = FixedText::default();
        if !process.set(victim_process) {
            return None;
        }
        let start = EvidencePoint {
            line: line.line,
            provenance: line.provenance,
        };
        Some(Self {
            candidate_id,
            producer_pid,
            victim_process: process,
            victim_pid: None,
            victim_pid_point: None,
            reason: None,
            reason_point: None,
            conflicting_victim_pid: false,
            start,
            anchor_timestamp: parse_log_timestamp(line.parsed.date, line.parsed.time)
                .unwrap_or_default(),
            last_evidence_line: line.line,
            last_touched_line: line.line,
            bytes_seen: line.raw.len(),
            unmatched: 0,
        })
    }

    fn minimum_grammar_met(&self) -> bool {
        self.victim_pid.is_some() && !self.conflicting_victim_pid
    }

    fn record_message(&mut self, line: &ObservedLine<'_>, message: &str) -> bool {
        if let Some(pid) = parse_victim_pid(message) {
            if self.victim_pid.is_some_and(|known| known != pid) {
                self.conflicting_victim_pid = true;
            } else {
                self.victim_pid = Some(pid);
                self.victim_pid_point = Some(EvidencePoint {
                    line: line.line,
                    provenance: line.provenance,
                });
            }
            self.touch_evidence(line.line);
            return true;
        }
        if let Some(reason) = message.strip_prefix("Reason: ").map(trim_ascii) {
            if !reason.is_empty() {
                self.reason = normalize_anr_reason(reason.as_bytes());
                self.reason_point = Some(EvidencePoint {
                    line: line.line,
                    provenance: line.provenance,
                });
                self.touch_evidence(line.line);
                return true;
            }
        }
        false
    }

    fn touch_evidence(&mut self, line: u32) {
        self.last_evidence_line = line;
        self.last_touched_line = line;
        self.unmatched = 0;
    }

    fn into_problem(self, limited: bool) -> Option<RecognizedProblem> {
        let victim_pid = self.victim_pid.filter(|_| self.minimum_grammar_met())?;
        build_anr_problem(
            self.start,
            self.last_evidence_line,
            self.anchor_timestamp,
            victim_pid,
            self.victim_process.as_str(),
            self.victim_pid_point,
            self.reason,
            self.reason_point,
            EvidenceFormat::AospText,
            false,
            limited,
        )
    }
}

#[derive(Debug)]
pub(crate) struct AnrRecognizer {
    pending: Vec<AnrPending>,
    ready: VecDeque<RecognizedProblem>,
    next_candidate_id: u64,
}

impl Default for AnrRecognizer {
    fn default() -> Self {
        Self {
            pending: Vec::with_capacity(MAX_ACTIVE_ANR),
            ready: VecDeque::with_capacity(MAX_ACTIVE_ANR),
            next_candidate_id: 1,
        }
    }
}

impl ProblemRecognizer for AnrRecognizer {
    fn observe(&mut self, line: &ObservedLine<'_>) {
        self.expire_before(line);
        if line.raw.len() > MAX_PHYSICAL_LINE_BYTES {
            self.mark_unmatched_all();
            self.finalize_exhausted_unmatched();
            return;
        }

        if let Some(problem) = recognize_am_anr(line) {
            self.ready.push_back(problem);
            return;
        }

        if let Some((producer_pid, victim_process)) = anr_start(line) {
            if let Some(index) = self
                .pending
                .iter()
                .position(|pending| pending.producer_pid == producer_pid)
            {
                self.finalize(index, false);
            } else if self.pending.len() == MAX_ACTIVE_ANR {
                let index = self.oldest_pending_index();
                self.finalize(index, true);
            }
            let candidate_id = self.next_candidate_id;
            self.next_candidate_id = self.next_candidate_id.wrapping_add(1).max(1);
            if let Some(pending) = AnrPending::new(candidate_id, producer_pid, victim_process, line)
            {
                self.pending.push(pending);
            }
            return;
        }

        let message = line.parsed.message;
        if line.parsed.tag == "ActivityManager" {
            let Some(producer_pid) = parse_pid(line.parsed.pid) else {
                self.mark_unmatched_all();
                self.finalize_exhausted_unmatched();
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
        } else if line.parsed.tag.is_empty() && is_anr_continuation(message) {
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

impl AnrRecognizer {
    fn expire_before(&mut self, line: &ObservedLine<'_>) {
        let mut index = 0;
        while index < self.pending.len() {
            let pending = &mut self.pending[index];
            pending.bytes_seen = pending.bytes_seen.saturating_add(line.raw.len());
            let line_limit = line.line.saturating_sub(pending.start.line) >= MAX_ANR_LINES;
            let byte_limit = pending.bytes_seen > MAX_ANR_BYTES;
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

fn anr_start<'a>(line: &ObservedLine<'a>) -> Option<(u32, &'a str)> {
    if line.parsed.tag != "ActivityManager" {
        return None;
    }
    let process = trim_ascii(line.parsed.message.strip_prefix("ANR in ")?);
    let process = process.split_ascii_whitespace().next()?;
    if process.is_empty()
        || !process
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-'))
    {
        return None;
    }
    Some((parse_pid(line.parsed.pid)?, process))
}

fn parse_victim_pid(message: &str) -> Option<u32> {
    parse_pid(trim_ascii(message.strip_prefix("PID: ")?))
}

fn is_anr_continuation(message: &str) -> bool {
    parse_victim_pid(message).is_some()
        || message
            .strip_prefix("Reason: ")
            .is_some_and(|reason| !trim_ascii(reason).is_empty())
}

fn message_compatible(pending: &AnrPending, message: &str) -> bool {
    if let Some(pid) = parse_victim_pid(message) {
        return pending.victim_pid.is_none_or(|known| known == pid);
    }
    message
        .strip_prefix("Reason: ")
        .is_some_and(|reason| !trim_ascii(reason).is_empty())
}

fn recognize_am_anr(line: &ObservedLine<'_>) -> Option<RecognizedProblem> {
    if line.parsed.tag != "am_anr"
        || line
            .coverage
            .admit(EvidenceFormat::EventLogShapedText, line.provenance)
            != EvidenceAdmission::CommitEligible
    {
        return None;
    }
    let EventLogRecord::Anr(anr) = parse_event_log(line.parsed.tag, line.parsed.message).ok()?
    else {
        return None;
    };
    let point = EvidencePoint {
        line: line.line,
        provenance: line.provenance,
    };
    build_anr_problem(
        point,
        line.line,
        parse_log_timestamp(line.parsed.date, line.parsed.time).unwrap_or_default(),
        anr.pid,
        anr.package_name,
        Some(point),
        normalize_anr_reason(anr.reason.as_bytes()),
        Some(point),
        EvidenceFormat::EventLogShapedText,
        true,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_anr_problem(
    start: EvidencePoint,
    end_line: u32,
    anchor_timestamp: crate::problems::PackedLogTimestamp,
    victim_pid: u32,
    victim_process: &str,
    victim_pid_point: Option<EvidencePoint>,
    reason: Option<NormalizedAnrReason>,
    reason_point: Option<EvidencePoint>,
    format: EvidenceFormat,
    structured: bool,
    limited: bool,
) -> Option<RecognizedProblem> {
    let process = ProcessFingerprintKey::new(Some(victim_process));
    if process.is_unknown() {
        return None;
    }
    let identity_quality = process.identity_quality();
    let signature_quality = if reason.is_some() {
        SignatureQuality::StructuredFields
    } else {
        SignatureQuality::Minimal
    };
    let mut fingerprint = FingerprintBuilder::new(
        ProblemKind::Anr,
        FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        &process,
    );
    if let Some(reason) = reason {
        fingerprint
            .token(
                FingerprintTokenKind::StructuredField,
                reason_category_token(reason.category),
            )
            .token(
                FingerprintTokenKind::StructuredField,
                reason.canonical.as_bytes(),
            );
    }
    let group_key = GroupKey::new(
        ProblemKind::Anr,
        FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        fingerprint.finish(),
    );
    let mut evidence = EvidenceFlags::PRIMARY;
    if end_line > start.line {
        evidence.insert(EvidenceFlags::MULTILINE);
    }
    if structured {
        evidence.insert(EvidenceFlags::STRUCTURED);
    }
    let mut boundary = BoundaryFlags::NONE;
    if limited {
        boundary.insert(BoundaryFlags::TRUNCATED_BY_INPUT);
    }
    let draft = ProblemEventDraft {
        start_line: start.line,
        end_line,
        anchor_line: start.line,
        anchor_timestamp,
        pid: victim_pid,
        process_instance: ProcessInstanceKey(0),
        kind: ProblemKind::Anr,
        evidence,
        outcome: OutcomeFlags::NONE,
        boundary,
    };
    let mut problem = RecognizedProblem::new(
        draft,
        group_key,
        observation(
            start,
            ObservationRole::Primary,
            EvidencePriority::MinimumGrammar,
            format,
        ),
    );
    if let Some(point) = victim_pid_point {
        problem.push_observation(observation(
            point,
            ObservationRole::ProcessIdentity,
            EvidencePriority::MinimumGrammar,
            format,
        ));
    }
    if let Some(point) = reason_point {
        problem.push_observation(observation(
            point,
            ObservationRole::Reason,
            EvidencePriority::Supporting,
            format,
        ));
    }
    Some(problem)
}

fn reason_category_token(category: AnrReasonCategory) -> &'static [u8] {
    match category {
        AnrReasonCategory::InputDispatchTimeout => b"input-dispatch-timeout",
        AnrReasonCategory::BroadcastTimeout => b"broadcast-timeout",
        AnrReasonCategory::ServiceTimeout => b"service-timeout",
        AnrReasonCategory::ContentProviderTimeout => b"content-provider-timeout",
    }
}

fn observation(
    point: EvidencePoint,
    role: ObservationRole,
    priority: EvidencePriority,
    format: EvidenceFormat,
) -> ObservationCandidate {
    ObservationCandidate::new(
        ObservationRef::new(
            point.line,
            RuleId::AnrActivityManagerV1,
            role,
            format,
            point.provenance,
        )
        .expect("ANR recognizer rule/role pairs are compile-time contracts"),
        priority,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_line_ref;
    use crate::problems::{
        BufferSet, InputCoverage, LogBuffer, ProblemEngine, ProblemEventId, RangeCompleteness,
    };

    fn line<'a>(number: u32, raw: &'a str) -> ObservedLine<'a> {
        ObservedLine::new(
            number,
            raw.as_bytes(),
            parse_line_ref(raw),
            LineProvenance::Unknown,
            InputCoverage::static_file(RangeCompleteness::Bounded),
        )
    }

    fn events_line<'a>(number: u32, raw: &'a str, known: bool) -> ObservedLine<'a> {
        ObservedLine::new(
            number,
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

    fn feed(engine: &mut ProblemEngine, number: u32, raw: &str) {
        engine.observe(line(number, raw));
    }

    #[test]
    fn activity_manager_block_uses_victim_pid_not_header_producer_pid() {
        let mut engine = ProblemEngine::new();
        for (number, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
            ),
            (
                1,
                "07-26 12:00:00.001  100  101 E ActivityManager: PID: 321",
            ),
            (
                2,
                "07-26 12:00:00.002  100  101 E ActivityManager: Reason: Input dispatching timed out (server is not responding for 5000ms)",
            ),
            (
                3,
                "07-26 12:00:00.003  7  7 I Other: unrelated after block",
            ),
        ] {
            feed(&mut engine, number, raw);
        }
        engine.finish_input();
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::Anr);
        assert_eq!(event.pid(), 321);
        assert_ne!(event.pid(), 100);
        assert_eq!((event.start_line(), event.end_line()), (0, 2));
        assert!(event.anchor_timestamp().is_known());
    }

    #[test]
    fn victim_pid_is_required_and_reason_is_optional() {
        let mut missing_pid = ProblemEngine::new();
        feed(
            &mut missing_pid,
            0,
            "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
        );
        feed(
            &mut missing_pid,
            1,
            "07-26 12:00:00.001  100  101 E ActivityManager: Reason: Input dispatching timed out",
        );
        missing_pid.finish_input();
        assert_eq!(missing_pid.stats().observed_occurrence_count, 0);

        let mut no_reason = ProblemEngine::new();
        feed(
            &mut no_reason,
            0,
            "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
        );
        feed(
            &mut no_reason,
            1,
            "07-26 12:00:00.001  100  101 E ActivityManager: PID: 321",
        );
        no_reason.finish_input();
        assert_eq!(no_reason.stats().stored_occurrence_count, 1);
    }

    #[test]
    fn standalone_watchdog_and_input_timeout_chatter_are_not_anr_events() {
        let mut engine = ProblemEngine::new();
        for (number, raw) in [
            (
                0,
                "07-26 12:00:00.000  321  321 E ANR-WatchDog: ANR detected",
            ),
            (
                1,
                "07-26 12:00:00.001  100  100 W InputDispatcher: Application is not responding",
            ),
            (
                2,
                "07-26 12:00:00.002  100  100 W WindowManager: timeout waiting for draw",
            ),
        ] {
            feed(&mut engine, number, raw);
        }
        engine.finish_input();
        assert_eq!(engine.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn known_am_anr_commits_and_inferred_events_remains_supporting_only() {
        let raw = "07-26 12:00:00.000  55  55 I am_anr: [321,com.example.app,0,Input dispatching timed out, waiting for focus]";
        let mut known = ProblemEngine::new();
        known.observe(events_line(0, raw, true));
        assert_eq!(known.stats().stored_occurrence_count, 1);
        assert_eq!(known.event(ProblemEventId(0)).unwrap().pid(), 321);

        let mut inferred = ProblemEngine::new();
        inferred.observe(events_line(0, raw, false));
        assert_eq!(inferred.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn interleaved_producer_blocks_do_not_cross_victim_pids() {
        let mut engine = ProblemEngine::new();
        for (number, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  101 E ActivityManager: ANR in one.app",
            ),
            (
                1,
                "07-26 12:00:00.001  200  201 E ActivityManager: ANR in two.app",
            ),
            (
                2,
                "07-26 12:00:00.002  200  201 E ActivityManager: PID: 222",
            ),
            (
                3,
                "07-26 12:00:00.003  100  101 E ActivityManager: PID: 111",
            ),
        ] {
            feed(&mut engine, number, raw);
        }
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 2);
        assert_eq!(engine.event(ProblemEventId(0)).unwrap().pid(), 111);
        assert_eq!(engine.event(ProblemEventId(1)).unwrap().pid(), 222);
    }

    #[test]
    fn raw_pid_continuation_is_ignored_when_two_pending_blocks_are_compatible() {
        let mut engine = ProblemEngine::new();
        feed(
            &mut engine,
            0,
            "07-26 12:00:00.000  100  101 E ActivityManager: ANR in one.app",
        );
        feed(
            &mut engine,
            1,
            "07-26 12:00:00.001  200  201 E ActivityManager: ANR in two.app",
        );
        feed(&mut engine, 2, "PID: 999");
        engine.finish_input();
        assert_eq!(engine.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn volatile_input_timeout_details_do_not_split_anr_groups() {
        let mut engine = ProblemEngine::new();
        for (number, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
            ),
            (
                1,
                "07-26 12:00:00.001  100  101 E ActivityManager: PID: 321",
            ),
            (
                2,
                "07-26 12:00:00.002  100  101 E ActivityManager: Reason: Input dispatching timed out (5000ms)",
            ),
            (
                10,
                "07-26 12:00:10.000  100  101 E ActivityManager: ANR in com.example.app",
            ),
            (
                11,
                "07-26 12:00:10.001  100  101 E ActivityManager: PID: 321",
            ),
            (
                12,
                "07-26 12:00:10.002  100  101 E ActivityManager: Reason: Input dispatching timed out (9000ms)",
            ),
        ] {
            feed(&mut engine, number, raw);
        }
        engine.finish_input();
        let first = engine.event(ProblemEventId(0)).unwrap();
        let second = engine.event(ProblemEventId(1)).unwrap();
        assert_eq!(first.group_id_raw(), second.group_id_raw());
    }

    #[test]
    fn pending_anr_state_is_a_bounded_fixed_summary_without_owned_log_text() {
        assert!(!std::mem::needs_drop::<AnrPending>());
        assert!(std::mem::size_of::<AnrPending>() < 2 * 1024);
        assert_eq!(MAX_ACTIVE_ANR, 16);
        assert_eq!(MAX_ANR_BYTES, 256 * 1024);
    }

    #[test]
    fn conflicting_victim_pid_drops_the_activity_manager_candidate() {
        let mut engine = ProblemEngine::new();
        for (number, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
            ),
            (
                1,
                "07-26 12:00:00.001  100  101 E ActivityManager: PID: 321",
            ),
            (
                2,
                "07-26 12:00:00.002  100  101 E ActivityManager: PID: 654",
            ),
        ] {
            feed(&mut engine, number, raw);
        }
        engine.finish_input();
        assert_eq!(engine.stats().observed_occurrence_count, 0);
    }

    #[test]
    fn complete_anr_commits_at_limit_while_incomplete_anr_is_dropped() {
        let mut complete = ProblemEngine::new();
        feed(
            &mut complete,
            0,
            "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
        );
        feed(
            &mut complete,
            1,
            "07-26 12:00:00.001  100  101 E ActivityManager: PID: 321",
        );
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
            "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
        );
        feed(
            &mut incomplete,
            512,
            "07-26 12:00:01.000  7  7 I Other: boundary",
        );
        assert_eq!(incomplete.stats().observed_occurrence_count, 0);
    }
}
