use super::eventlog::parse_event_log;
use super::facts::{
    EvidencePriority, ObservationCandidate, ObservationRef, ObservationRole, RuleId,
};
use super::fingerprint::{FingerprintBuilder, FingerprintTokenKind, ProcessFingerprintKey};
use super::index::{
    AppendOutcome, GroupId, GroupKey, GroupPage, GroupQuery, OccurrencePage, PageSpec,
    ProblemGroupSummary, ProblemIndex, ProblemIndexError, ProblemIndexLimits, ProblemStats,
    QuerySnapshotId, SnapshotError,
};
use super::model::{
    BoundaryFlags, EvidenceFlags, OutcomeFlags, PackedLogTimestamp, ProblemEvent,
    ProblemEventDraft, ProblemEventId, ProblemKind, ProcessInstanceKey, SignatureQuality,
    MAX_MATERIALIZED_OBSERVATIONS,
};
use super::provenance::{EvidenceAdmission, EvidenceFormat, InputCoverage, LineProvenance};
use super::recognizers::{
    parse_zygote_signal_exit, AnrRecognizer, JavaRecognizer, KernelOomOccurrence,
    KernelOomRecognizer, LifecycleOccurrence, LifecycleRecognizer, LifecycleRecognizerError,
    LifecycleRelation, LifecycleTime, LmkMechanism, LmkOccurrence, LmkRecognizer, NativeRecognizer,
    ProblemRecognizer,
};
use super::timestamp::{parse_log_timestamp, SegmentedTimestamp, TimestampSegmentTracker};
use crate::parser::ParsedLine;

/// A borrowed stable source line presented to the Problems engine.
///
/// Recognizers may inspect `raw` and `parsed` only during `observe`; pending
/// state and indexed events must retain compact summaries and source pointers,
/// never either borrowed payload.
#[derive(Debug, Clone, Copy)]
pub struct ObservedLine<'a> {
    pub line: u32,
    pub raw: &'a [u8],
    pub parsed: ParsedLine<'a>,
    pub provenance: LineProvenance,
    pub coverage: InputCoverage,
}

impl<'a> ObservedLine<'a> {
    pub const fn new(
        line: u32,
        raw: &'a [u8],
        parsed: ParsedLine<'a>,
        provenance: LineProvenance,
        coverage: InputCoverage,
    ) -> Self {
        Self {
            line,
            raw,
            parsed,
            provenance,
            coverage,
        }
    }
}

/// Bounded summary of changes caused by one engine operation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProblemDelta {
    committed: u8,
    stored: u8,
    dropped: u8,
    last_stored_event: Option<ProblemEventId>,
    failed: u8,
}

impl ProblemDelta {
    pub const fn committed(self) -> u8 {
        self.committed
    }

    pub const fn stored(self) -> u8 {
        self.stored
    }

    pub const fn dropped(self) -> u8 {
        self.dropped
    }

    pub const fn last_stored_event(self) -> Option<ProblemEventId> {
        self.last_stored_event
    }

    pub const fn failed(self) -> u8 {
        self.failed
    }

    fn record(&mut self, outcome: AppendOutcome) {
        self.committed = self.committed.saturating_add(1);
        match outcome {
            AppendOutcome::Stored { event_id, .. } => {
                self.stored = self.stored.saturating_add(1);
                self.last_stored_event = Some(event_id);
            }
            AppendOutcome::Dropped { .. } => {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }
}

/// Internal recognizer-to-engine commit value.
///
/// Its observation storage has a fixed upper bound and contains only compact
/// source references. There is intentionally no raw-text field.
#[derive(Debug, Clone)]
pub(crate) struct RecognizedProblem {
    pub(crate) draft: ProblemEventDraft,
    pub(crate) group_key: GroupKey,
    observations: [ObservationCandidate; MAX_MATERIALIZED_OBSERVATIONS as usize],
    observation_len: u8,
}

impl RecognizedProblem {
    pub(crate) fn new(
        draft: ProblemEventDraft,
        group_key: GroupKey,
        primary: ObservationCandidate,
    ) -> Self {
        Self {
            draft,
            group_key,
            observations: [primary; MAX_MATERIALIZED_OBSERVATIONS as usize],
            observation_len: 1,
        }
    }

    pub(crate) fn push_observation(&mut self, observation: ObservationCandidate) -> bool {
        let index = usize::from(self.observation_len);
        if index == self.observations.len() {
            self.draft
                .boundary
                .insert(BoundaryFlags::OBSERVATION_REFS_TRUNCATED);
            return false;
        }
        self.observations[index] = observation;
        self.observation_len += 1;
        true
    }

    pub(crate) fn observations(&self) -> &[ObservationCandidate] {
        &self.observations[..usize::from(self.observation_len)]
    }
}

/// Deep-module owner for recognizer state and the compact query index.
#[derive(Debug, Default)]
pub struct ProblemEngine {
    index: ProblemIndex,
    java: JavaRecognizer,
    anr: AnrRecognizer,
    native: NativeRecognizer,
    lifecycle: LifecycleRecognizer,
    lmk: LmkRecognizer,
    kernel_oom: KernelOomRecognizer,
    timestamps: TimestampSegmentTracker,
    timestamp_origin: Option<SegmentedTimestamp>,
}

impl ProblemEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(limits: ProblemIndexLimits) -> Result<Self, ProblemIndexError> {
        Ok(Self {
            index: ProblemIndex::with_limits(limits)?,
            java: JavaRecognizer::default(),
            anr: AnrRecognizer::default(),
            native: NativeRecognizer::new(),
            lifecycle: LifecycleRecognizer::new(),
            lmk: LmkRecognizer::new(),
            kernel_oom: KernelOomRecognizer::new(),
            timestamps: TimestampSegmentTracker::new(),
            timestamp_origin: None,
        })
    }

    pub fn stats(&self) -> ProblemStats {
        self.index.stats()
    }

    pub fn event(&self, id: ProblemEventId) -> Option<ProblemEvent> {
        self.index.event(id)
    }

    pub fn event_observations(
        &self,
        id: ProblemEventId,
    ) -> Option<&[super::facts::ObservationRef]> {
        self.index.event_observations(id)
    }

    pub fn group(&self, id: GroupId) -> Option<ProblemGroupSummary> {
        self.index.group(id)
    }

    pub fn create_group_snapshot(
        &mut self,
        query: &GroupQuery,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.index.create_group_snapshot(query)
    }

    pub fn create_occurrence_snapshot(
        &mut self,
        group: GroupId,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.index.create_occurrence_snapshot(group)
    }

    pub fn group_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<GroupPage, SnapshotError> {
        self.index.group_snapshot_page(snapshot, page)
    }

    pub fn occurrence_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.index.occurrence_snapshot_page(snapshot, page)
    }

    pub fn release_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool {
        self.index.release_snapshot(snapshot)
    }

    pub fn observe(&mut self, line: ObservedLine<'_>) -> ProblemDelta {
        let (anchor_timestamp, lifecycle_time) = self.observe_timestamp(&line);
        let lifecycle = self.observe_lifecycle(&line, lifecycle_time);
        let kernel_oom = self.kernel_oom.observe(
            line.line,
            line.parsed.tag,
            line.parsed.message.as_bytes(),
            line.provenance,
        );
        let lmk = kernel_oom.is_none().then(|| {
            self.lmk.observe(
                line.line,
                line.parsed.tag,
                line.parsed.message.as_bytes(),
                line.provenance,
            )
        });

        self.java.observe(&line);
        self.anr.observe(&line);
        self.native.observe(&line);
        let mut delta = self.drain_recognizers();
        match lifecycle {
            Ok(Some(occurrence)) => {
                let problem = recognized_lifecycle(occurrence, anchor_timestamp);
                self.commit_or_count_failure(problem, &mut delta);
            }
            Ok(None) => {}
            Err(_) => delta.failed = delta.failed.saturating_add(1),
        }
        if let Some(occurrence) = lmk.flatten() {
            let process_instance = self.active_process_instance(
                occurrence.victim_pid,
                occurrence.process.as_str(),
                occurrence.victim_uid,
            );
            let problem = recognized_lmk(
                occurrence,
                anchor_timestamp,
                process_instance,
                line.provenance,
            );
            self.commit_or_count_failure(problem, &mut delta);
        }
        if let Some(occurrence) = kernel_oom {
            let process_instance = self.active_process_instance(
                occurrence.victim_pid,
                occurrence.process.as_str(),
                None,
            );
            let problem = recognized_kernel_oom(
                occurrence,
                anchor_timestamp,
                process_instance,
                line.provenance,
            );
            self.commit_or_count_failure(problem, &mut delta);
        }
        delta
    }

    pub fn finish_input(&mut self) -> ProblemDelta {
        self.java.finish_input();
        self.anr.finish_input();
        self.native.finish_input();
        self.lifecycle.finish_input();
        self.lmk.finish_input();
        self.kernel_oom.finish_input();
        self.drain_recognizers()
    }

    pub fn reset(&mut self) {
        self.java.reset();
        self.anr.reset();
        self.native.reset();
        self.lifecycle.reset();
        self.lmk.reset();
        self.kernel_oom.reset();
        self.timestamps.reset();
        self.timestamp_origin = None;
        self.index.reset();
    }

    fn observe_timestamp(
        &mut self,
        line: &ObservedLine<'_>,
    ) -> (PackedLogTimestamp, Option<LifecycleTime>) {
        let parsed = parse_log_timestamp(line.parsed.date, line.parsed.time);
        let Some(current) = self.timestamps.observe(parsed) else {
            self.timestamp_origin = None;
            return (PackedLogTimestamp::UNKNOWN, None);
        };
        let origin = match self.timestamp_origin {
            Some(origin) if origin.segment() == current.segment() => origin,
            _ => {
                self.timestamp_origin = Some(current);
                current
            }
        };
        let millis = origin
            .delta_ms(current)
            .and_then(|delta| u64::try_from(delta).ok())
            .unwrap_or(0);
        (
            current.timestamp(),
            Some(LifecycleTime {
                segment: current.segment().0,
                millis,
            }),
        )
    }

    fn observe_lifecycle(
        &mut self,
        line: &ObservedLine<'_>,
        time: Option<LifecycleTime>,
    ) -> Result<Option<LifecycleOccurrence>, LifecycleRecognizerError> {
        let delta = match line.parsed.tag {
            "ActivityManager" => self.lifecycle.observe_activity_manager_with_provenance(
                line.line,
                line.parsed.message.as_bytes(),
                line.provenance,
                time,
            )?,
            "am_proc_start" | "am_proc_died" | "am_kill" => {
                let provenance = line
                    .coverage
                    .infer_format_provenance(EvidenceFormat::EventLogShapedText, line.provenance);
                if line
                    .coverage
                    .admit(EvidenceFormat::EventLogShapedText, provenance)
                    == EvidenceAdmission::Rejected
                {
                    return Ok(None);
                }
                let Ok(record) = parse_event_log(line.parsed.tag, line.parsed.message) else {
                    return Ok(None);
                };
                self.lifecycle
                    .observe_event_log(line.line, record, provenance, time)?
            }
            "Zygote" => {
                let Some((pid, signal)) = parse_zygote_signal_exit(line.parsed.message.as_bytes())
                else {
                    return Ok(None);
                };
                self.lifecycle.observe_signal_exit_with_provenance(
                    line.line,
                    pid,
                    signal,
                    line.provenance,
                )?
            }
            _ => return Ok(None),
        };
        Ok(delta.occurrence)
    }

    fn active_process_instance(
        &self,
        pid: u32,
        process: &str,
        uid: Option<u32>,
    ) -> ProcessInstanceKey {
        let Some(active) = self.lifecycle.tracker().active_for_pid(pid) else {
            return ProcessInstanceKey(0);
        };
        if active.process_name() != process || uid.is_some_and(|uid| active.uid() != Some(uid)) {
            return ProcessInstanceKey(0);
        }
        active.instance().key()
    }

    fn commit_or_count_failure(&mut self, problem: RecognizedProblem, delta: &mut ProblemDelta) {
        if self.commit_recognized(problem, delta).is_err() {
            delta.failed = delta.failed.saturating_add(1);
        }
    }

    pub(crate) fn commit_recognized(
        &mut self,
        problem: RecognizedProblem,
        delta: &mut ProblemDelta,
    ) -> Result<(), ProblemIndexError> {
        let outcome =
            self.index
                .append(problem.draft, problem.group_key, problem.observations())?;
        delta.record(outcome);
        Ok(())
    }

    fn drain_recognizers(&mut self) -> ProblemDelta {
        let mut delta = ProblemDelta::default();
        while let Some(problem) = self.java.pop_ready() {
            if self.commit_recognized(problem, &mut delta).is_err() {
                delta.failed = delta.failed.saturating_add(1);
            }
        }
        while let Some(problem) = self.anr.pop_ready() {
            if self.commit_recognized(problem, &mut delta).is_err() {
                delta.failed = delta.failed.saturating_add(1);
            }
        }
        while let Some(problem) = self.native.pop_ready() {
            if self.commit_recognized(problem, &mut delta).is_err() {
                delta.failed = delta.failed.saturating_add(1);
            }
        }
        delta
    }
}

const LIFECYCLE_FINGERPRINT_VERSION: u16 = 1;
const LMK_FINGERPRINT_VERSION: u16 = 1;
const KERNEL_OOM_FINGERPRINT_VERSION: u16 = 1;

fn recognized_lifecycle(
    occurrence: LifecycleOccurrence,
    anchor_timestamp: PackedLogTimestamp,
) -> RecognizedProblem {
    let process = ProcessFingerprintKey::new(Some(occurrence.process.as_str()));
    let identity_quality = process.identity_quality();
    let (
        kind,
        signature_quality,
        relation_token,
        rule,
        anchor_line,
        pid,
        process_instance,
        primary_source,
        outcome,
    ) = match occurrence.relation {
        LifecycleRelation::Restart => (
            ProblemKind::ProcessRestart,
            SignatureQuality::StructuredFields,
            b"start-after-death".as_slice(),
            RuleId::ProcessRestartV1,
            occurrence
                .start_line
                .expect("restart occurrence has a start line"),
            occurrence
                .start_pid
                .expect("restart occurrence has a start pid"),
            occurrence
                .started_instance
                .expect("restart occurrence has a started instance")
                .key(),
            occurrence
                .start_source
                .expect("restart occurrence has a start source"),
            OutcomeFlags::DEATH_OBSERVED | OutcomeFlags::START_AFTER_DEATH_OBSERVED,
        ),
        LifecycleRelation::SignalExit => (
            ProblemKind::SignalExit,
            SignatureQuality::SignalOnly,
            b"signal-exit".as_slice(),
            RuleId::SignalExitV1,
            occurrence.death_line,
            occurrence.death_pid,
            occurrence.terminated_instance.key(),
            occurrence.death_source,
            OutcomeFlags::DEATH_OBSERVED,
        ),
    };
    let mut fingerprint = FingerprintBuilder::new(
        kind,
        LIFECYCLE_FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        &process,
    );
    fingerprint.token(FingerprintTokenKind::Relation, relation_token);
    if let Some(signal) = occurrence.signal {
        fingerprint.token(FingerprintTokenKind::Signal, &[signal]);
    }
    let group_key = GroupKey::new(
        kind,
        LIFECYCLE_FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        fingerprint.finish(),
    );
    let start_line = occurrence
        .start_line
        .map_or(occurrence.death_line, |start| {
            occurrence.death_line.min(start)
        });
    let end_line = occurrence
        .start_line
        .map_or(occurrence.death_line, |start| {
            occurrence.death_line.max(start)
        });
    let mut evidence = EvidenceFlags::PRIMARY | EvidenceFlags::CORRELATED;
    if start_line != end_line {
        evidence.insert(EvidenceFlags::MULTILINE);
    }
    if primary_source.format == EvidenceFormat::EventLogShapedText
        || occurrence.death_source.format == EvidenceFormat::EventLogShapedText
    {
        evidence.insert(EvidenceFlags::STRUCTURED);
    }
    let draft = ProblemEventDraft {
        start_line,
        end_line,
        anchor_line,
        anchor_timestamp,
        pid,
        process_instance,
        kind,
        evidence,
        outcome,
        boundary: BoundaryFlags::NONE,
    };
    let mut problem = RecognizedProblem::new(
        draft,
        group_key,
        compact_observation(
            anchor_line,
            rule,
            ObservationRole::Primary,
            primary_source.format,
            primary_source.provenance,
            EvidencePriority::MinimumGrammar,
        ),
    );
    problem.push_observation(compact_observation(
        anchor_line,
        rule,
        ObservationRole::ProcessIdentity,
        primary_source.format,
        primary_source.provenance,
        EvidencePriority::MinimumGrammar,
    ));
    match occurrence.relation {
        LifecycleRelation::Restart => {
            problem.push_observation(compact_observation(
                occurrence.death_line,
                rule,
                ObservationRole::Death,
                occurrence.death_source.format,
                occurrence.death_source.provenance,
                EvidencePriority::Correlation,
            ));
            problem.push_observation(compact_observation(
                anchor_line,
                rule,
                ObservationRole::Restart,
                primary_source.format,
                primary_source.provenance,
                EvidencePriority::Outcome,
            ));
        }
        LifecycleRelation::SignalExit => {
            problem.push_observation(compact_observation(
                anchor_line,
                rule,
                ObservationRole::Signal,
                primary_source.format,
                primary_source.provenance,
                EvidencePriority::MinimumGrammar,
            ));
            problem.push_observation(compact_observation(
                occurrence.death_line,
                rule,
                ObservationRole::Death,
                occurrence.death_source.format,
                occurrence.death_source.provenance,
                EvidencePriority::Outcome,
            ));
        }
    }
    problem
}

fn recognized_lmk(
    occurrence: LmkOccurrence,
    anchor_timestamp: PackedLogTimestamp,
    process_instance: ProcessInstanceKey,
    provenance: LineProvenance,
) -> RecognizedProblem {
    let process = ProcessFingerprintKey::new(Some(occurrence.process.as_str()));
    let identity_quality = process.identity_quality();
    let signature_quality = SignatureQuality::StructuredFields;
    let mut fingerprint = FingerprintBuilder::new(
        ProblemKind::LmkKill,
        LMK_FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        &process,
    );
    fingerprint
        .token(
            FingerprintTokenKind::Mechanism,
            occurrence.mechanism.token().as_bytes(),
        )
        .token(
            FingerprintTokenKind::StructuredField,
            occurrence.reason.token().as_bytes(),
        );
    let group_key = GroupKey::new(
        ProblemKind::LmkKill,
        LMK_FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        fingerprint.finish(),
    );
    let format = match occurrence.mechanism {
        LmkMechanism::UserspaceLmkd => EvidenceFormat::AospText,
        LmkMechanism::LegacyKernelLowMemoryKiller => EvidenceFormat::KernelShapedText,
    };
    let draft = ProblemEventDraft {
        start_line: occurrence.line,
        end_line: occurrence.line,
        anchor_line: occurrence.line,
        anchor_timestamp,
        pid: occurrence.victim_pid,
        process_instance,
        kind: ProblemKind::LmkKill,
        evidence: EvidenceFlags::PRIMARY | EvidenceFlags::STRUCTURED,
        outcome: OutcomeFlags::KILL_ISSUED,
        boundary: BoundaryFlags::NONE,
    };
    let mut problem = RecognizedProblem::new(
        draft,
        group_key,
        compact_observation(
            occurrence.line,
            RuleId::LmkdKillV1,
            ObservationRole::Primary,
            format,
            provenance,
            EvidencePriority::MinimumGrammar,
        ),
    );
    problem.push_observation(compact_observation(
        occurrence.line,
        RuleId::LmkdKillV1,
        ObservationRole::ProcessIdentity,
        format,
        provenance,
        EvidencePriority::MinimumGrammar,
    ));
    problem.push_observation(compact_observation(
        occurrence.line,
        RuleId::LmkdKillV1,
        ObservationRole::KillIssued,
        format,
        provenance,
        EvidencePriority::Outcome,
    ));
    problem
}

fn recognized_kernel_oom(
    occurrence: KernelOomOccurrence,
    anchor_timestamp: PackedLogTimestamp,
    process_instance: ProcessInstanceKey,
    provenance: LineProvenance,
) -> RecognizedProblem {
    let process = ProcessFingerprintKey::new(Some(occurrence.process.as_str()));
    let identity_quality = process.identity_quality();
    let signature_quality = SignatureQuality::StructuredFields;
    let mut fingerprint = FingerprintBuilder::new(
        ProblemKind::KernelOomKill,
        KERNEL_OOM_FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        &process,
    );
    fingerprint.token(
        FingerprintTokenKind::Mechanism,
        occurrence.mechanism.token().as_bytes(),
    );
    let group_key = GroupKey::new(
        ProblemKind::KernelOomKill,
        KERNEL_OOM_FINGERPRINT_VERSION,
        signature_quality,
        identity_quality,
        fingerprint.finish(),
    );
    let draft = ProblemEventDraft {
        start_line: occurrence.line,
        end_line: occurrence.line,
        anchor_line: occurrence.line,
        anchor_timestamp,
        pid: occurrence.victim_pid,
        process_instance,
        kind: ProblemKind::KernelOomKill,
        evidence: EvidenceFlags::PRIMARY | EvidenceFlags::STRUCTURED,
        outcome: OutcomeFlags::KILL_ISSUED,
        boundary: BoundaryFlags::NONE,
    };
    let mut problem = RecognizedProblem::new(
        draft,
        group_key,
        compact_observation(
            occurrence.line,
            RuleId::KernelOomKillV1,
            ObservationRole::Primary,
            EvidenceFormat::KernelShapedText,
            provenance,
            EvidencePriority::MinimumGrammar,
        ),
    );
    problem.push_observation(compact_observation(
        occurrence.line,
        RuleId::KernelOomKillV1,
        ObservationRole::ProcessIdentity,
        EvidenceFormat::KernelShapedText,
        provenance,
        EvidencePriority::MinimumGrammar,
    ));
    problem.push_observation(compact_observation(
        occurrence.line,
        RuleId::KernelOomKillV1,
        ObservationRole::KillIssued,
        EvidenceFormat::KernelShapedText,
        provenance,
        EvidencePriority::Outcome,
    ));
    problem
}

fn compact_observation(
    line: u32,
    rule: RuleId,
    role: ObservationRole,
    format: EvidenceFormat,
    provenance: LineProvenance,
    priority: EvidencePriority,
) -> ObservationCandidate {
    ObservationCandidate::new(
        ObservationRef::new(line, rule, role, format, provenance)
            .expect("engine conversion uses only published rule/role pairs"),
        priority,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::{
        EvidenceFlags, EvidenceFormat, EvidencePriority, FingerprintBuilder, IdentityQuality,
        LineProvenance, ObservationRef, ObservationRole, OutcomeFlags, PackedLogTimestamp,
        ProblemFingerprint, ProblemKind, ProcessFingerprintKey, ProcessInstanceKey, RuleId,
        SignatureQuality,
    };

    fn group_key() -> GroupKey {
        let process = ProcessFingerprintKey::new(Some("com.example.app"));
        let fingerprint = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::TypeOnly,
            IdentityQuality::KnownProcess,
            &process,
        )
        .finish();
        GroupKey::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::TypeOnly,
            IdentityQuality::KnownProcess,
            fingerprint,
        )
    }

    fn observation(line: u32) -> ObservationCandidate {
        ObservationCandidate::new(
            ObservationRef::new(
                line,
                RuleId::JavaUncaughtV1,
                ObservationRole::Primary,
                EvidenceFormat::AospText,
                LineProvenance::Unknown,
            )
            .unwrap(),
            EvidencePriority::MinimumGrammar,
        )
    }

    fn recognized() -> RecognizedProblem {
        let draft = ProblemEventDraft {
            start_line: 3,
            end_line: 12,
            anchor_line: 3,
            anchor_timestamp: PackedLogTimestamp::UNKNOWN,
            pid: 42,
            process_instance: ProcessInstanceKey(0),
            kind: ProblemKind::JavaCrash,
            evidence: EvidenceFlags::PRIMARY,
            outcome: Default::default(),
            boundary: Default::default(),
        };
        RecognizedProblem::new(draft, group_key(), observation(3))
    }

    #[test]
    fn recognized_problem_has_bounded_compact_observations() {
        let mut problem = recognized();
        for line in 4..=10 {
            assert!(problem.push_observation(observation(line)));
        }
        assert_eq!(problem.observations().len(), 8);
        assert!(!problem.push_observation(observation(11)));
        assert!(problem
            .draft
            .boundary
            .contains(BoundaryFlags::OBSERVATION_REFS_TRUNCATED));
    }

    #[test]
    fn engine_commit_seam_routes_a_recognized_problem_to_the_index() {
        let mut engine = ProblemEngine::new();
        let mut delta = ProblemDelta::default();
        engine.commit_recognized(recognized(), &mut delta).unwrap();

        assert_eq!(delta.committed(), 1);
        assert_eq!(delta.stored(), 1);
        let event_id = delta.last_stored_event().unwrap();
        let event = engine.event(event_id).unwrap();
        assert_eq!(event.kind(), ProblemKind::JavaCrash);
        assert_eq!(event.pid(), 42);
        assert_eq!(engine.event_observations(event_id).unwrap().len(), 1);
    }

    #[test]
    fn observed_line_is_borrowed_and_does_not_own_raw_text() {
        let raw = b"07-26 12:00:00.000  42  42 E AndroidRuntime: fatal";
        let text = std::str::from_utf8(raw).unwrap();
        let observed = ObservedLine::new(
            9,
            raw,
            crate::parser::parse_line_ref(text),
            LineProvenance::Unknown,
            InputCoverage::static_file(crate::problems::RangeCompleteness::Bounded),
        );
        assert_eq!(observed.raw.as_ptr(), raw.as_ptr());
        assert_eq!(observed.parsed.pid, "42");
    }

    #[test]
    fn test_helper_freezes_the_fingerprint_type() {
        let fingerprint: ProblemFingerprint = group_key().fingerprint();
        assert_eq!(fingerprint.as_bytes().len(), 16);
    }

    #[test]
    fn reset_discards_pending_recognizer_state_as_well_as_the_index() {
        let coverage = InputCoverage::static_file(crate::problems::RangeCompleteness::Bounded);
        let header = "07-26 12:00:00.000  42  42 E AndroidRuntime: FATAL EXCEPTION: main";
        let mut engine = ProblemEngine::new();
        engine.observe(ObservedLine::new(
            0,
            header.as_bytes(),
            crate::parser::parse_line_ref(header),
            LineProvenance::Unknown,
            coverage,
        ));
        engine.reset();
        for (line, raw) in [
            (
                1,
                "07-26 12:00:00.001  42  42 E AndroidRuntime: Process: com.example.app, PID: 42",
            ),
            (
                2,
                "07-26 12:00:00.002  42  42 E AndroidRuntime: java.lang.RuntimeException: boom",
            ),
        ] {
            engine.observe(ObservedLine::new(
                line,
                raw.as_bytes(),
                crate::parser::parse_line_ref(raw),
                LineProvenance::Unknown,
                coverage,
            ));
        }
        engine.finish_input();
        assert_eq!(engine.stats().observed_occurrence_count, 0);
    }

    fn feed_with_provenance(
        engine: &mut ProblemEngine,
        line: u32,
        raw: &str,
        provenance: LineProvenance,
    ) -> ProblemDelta {
        engine.observe(ObservedLine::new(
            line,
            raw.as_bytes(),
            crate::parser::parse_line_ref(raw),
            provenance,
            InputCoverage::static_file(crate::problems::RangeCompleteness::Bounded),
        ))
    }

    fn stored_events(engine: &ProblemEngine) -> Vec<ProblemEvent> {
        (0..engine.stats().stored_occurrence_count)
            .filter_map(|id| u32::try_from(id).ok())
            .filter_map(|id| engine.event(ProblemEventId(id)))
            .collect()
    }

    #[test]
    fn engine_commits_strict_restart_and_signal_exit_without_ordinary_death_noise() {
        let mut engine = ProblemEngine::new();
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I am_proc_start: [0,42,10123,com.example.app,activity,com.example/.Main]",
            ),
            (
                1,
                "07-26 12:00:01.000  100  100 I am_proc_died: [0,42,com.example.app]",
            ),
        ] {
            assert_eq!(
                feed_with_provenance(&mut engine, line, raw, events).committed(),
                0
            );
        }
        let restart = feed_with_provenance(
            &mut engine,
            2,
            "07-26 12:01:01.000  100  100 I am_proc_start: [0,77,10123,com.example.app,activity,com.example/.Main]",
            events,
        );
        assert_eq!(
            restart.committed(),
            1,
            "30 seconds is not a relation cutoff"
        );

        feed_with_provenance(
            &mut engine,
            3,
            "07-26 12:01:02.000  100  100 I ActivityManager: Start proc 88:com.signal.app/u0a124 for service",
            LineProvenance::Known(crate::problems::LogBuffer::System),
        );
        let signal = feed_with_provenance(
            &mut engine,
            4,
            "07-26 12:01:03.000  100  100 I Zygote: Process 88 exited due to signal 9 (Killed)",
            LineProvenance::Known(crate::problems::LogBuffer::System),
        );
        assert_eq!(signal.committed(), 1);

        let events = stored_events(&engine);
        assert_eq!(events.len(), 2);
        let restart = events
            .iter()
            .find(|event| event.kind() == ProblemKind::ProcessRestart)
            .copied()
            .unwrap();
        assert_eq!((restart.start_line(), restart.end_line()), (1, 2));
        assert_eq!(restart.anchor_line(), 2);
        assert!(restart.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
        assert!(restart
            .outcome()
            .contains(OutcomeFlags::START_AFTER_DEATH_OBSERVED));
        assert!(!restart.outcome().contains(OutcomeFlags::KILL_ISSUED));

        let signal = events
            .iter()
            .find(|event| event.kind() == ProblemKind::SignalExit)
            .copied()
            .unwrap();
        assert_eq!((signal.start_line(), signal.end_line()), (4, 4));
        assert_eq!(signal.pid(), 88);
        assert_eq!(signal.outcome(), OutcomeFlags::DEATH_OBSERVED);
        let observations = engine
            .event_observations(ProblemEventId(
                events.iter().position(|event| *event == signal).unwrap() as u32,
            ))
            .unwrap();
        assert!(observations
            .iter()
            .any(|fact| fact.role() == ObservationRole::Signal));
    }

    #[test]
    fn mixed_memory_lines_commit_one_exclusive_kind_with_kill_issued_only() {
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.java.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.OutOfMemoryError: allocation failed",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:01.000  900  900 I lmkd: Kill 'com.lmk.app' (222), uid 10222, oom_score_adj 900 to free 42kB rss, 0kB swap; reason: low watermark",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:02.000  0  0 E kernel: Out of memory: Killed process 333 (com.kernel.app) total-vm:42kB",
                LineProvenance::Known(crate::problems::LogBuffer::Kernel),
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        let events = stored_events(&engine);
        assert_eq!(events.len(), 3);
        for kind in [
            ProblemKind::JavaOom,
            ProblemKind::LmkKill,
            ProblemKind::KernelOomKill,
        ] {
            assert_eq!(
                events.iter().filter(|event| event.kind() == kind).count(),
                1,
                "{kind:?} must be mutually exclusive"
            );
        }
        for event in events.iter().filter(|event| {
            matches!(
                event.kind(),
                ProblemKind::LmkKill | ProblemKind::KernelOomKill
            )
        }) {
            assert_eq!(event.outcome(), OutcomeFlags::KILL_ISSUED);
            assert!(!event.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
        }
        let lmk = events
            .iter()
            .find(|event| event.kind() == ProblemKind::LmkKill)
            .unwrap();
        assert_eq!(lmk.pid(), 222, "victim pid comes from the message");
    }

    #[test]
    fn restart_group_ignores_pid_elapsed_time_and_thirty_second_facet() {
        let mut engine = ProblemEngine::new();
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I ActivityManager: Start proc 41:com.example.app/u0a123 for service",
            ),
            (
                1,
                "07-26 12:00:01.000  100  100 I ActivityManager: Process com.example.app (pid 41) has died",
            ),
            (
                2,
                "07-26 12:00:10.000  100  100 I ActivityManager: Start proc 42:com.example.app/u0a123 for service",
            ),
            (
                3,
                "07-26 12:00:11.000  100  100 I ActivityManager: Process com.example.app (pid 42) has died",
            ),
            (
                4,
                "07-26 12:01:11.000  100  100 I ActivityManager: Start proc 43:com.example.app/u0a123 for service",
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, LineProvenance::Unknown);
        }
        let first = engine.event(ProblemEventId(0)).unwrap();
        let second = engine.event(ProblemEventId(1)).unwrap();
        assert_eq!(first.kind(), ProblemKind::ProcessRestart);
        assert_eq!(second.kind(), ProblemKind::ProcessRestart);
        assert_eq!(first.group_id_raw(), second.group_id_raw());
        assert!(first.anchor_timestamp().is_known());
        assert!(second.anchor_timestamp().is_known());
    }

    #[test]
    fn finish_and_reset_clear_all_new_recognizer_state() {
        let kernel = LineProvenance::Known(crate::problems::LogBuffer::Kernel);
        let mut engine = ProblemEngine::new();
        feed_with_provenance(
            &mut engine,
            0,
            "07-26 12:00:00.000  0  0 E kernel: oom-kill:constraint=CONSTRAINT_MEMCG",
            kernel,
        );
        assert_eq!(engine.kernel_oom.pending_count(), 1);
        engine.finish_input();
        assert_eq!(engine.kernel_oom.pending_count(), 0);

        feed_with_provenance(
            &mut engine,
            1,
            "07-26 12:00:01.000  100  100 I ActivityManager: Start proc 42:com.example.app/u0a123 for service",
            LineProvenance::Unknown,
        );
        feed_with_provenance(
            &mut engine,
            2,
            "07-26 12:00:02.000  100  100 I ActivityManager: Process com.example.app (pid 42) has died",
            LineProvenance::Unknown,
        );
        assert_eq!(engine.lifecycle.pending_count(), 1);
        engine.reset();
        assert_eq!(engine.lifecycle.pending_count(), 0);
        assert_eq!(engine.lmk.pending_count(), 0);
        assert_eq!(engine.kernel_oom.pending_count(), 0);
        assert_eq!(engine.stats().stored_occurrence_count, 0);

        feed_with_provenance(
            &mut engine,
            3,
            "07-26 12:00:03.000  100  100 I ActivityManager: Start proc 77:com.example.app/u0a123 for service",
            LineProvenance::Unknown,
        );
        assert_eq!(engine.stats().stored_occurrence_count, 0);
    }
}
