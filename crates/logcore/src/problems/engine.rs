use super::facts::ObservationCandidate;
use super::index::{
    AppendOutcome, GroupId, GroupKey, GroupPage, GroupQuery, OccurrencePage, PageSpec,
    ProblemGroupSummary, ProblemIndex, ProblemIndexError, ProblemIndexLimits, ProblemStats,
    QuerySnapshotId, SnapshotError,
};
use super::model::{
    BoundaryFlags, ProblemEvent, ProblemEventDraft, ProblemEventId, MAX_MATERIALIZED_OBSERVATIONS,
};
use super::provenance::{InputCoverage, LineProvenance};
use super::recognizers::{AnrRecognizer, JavaRecognizer, NativeRecognizer, ProblemRecognizer};
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
        self.java.observe(&line);
        self.anr.observe(&line);
        self.native.observe(&line);
        self.drain_recognizers()
    }

    pub fn finish_input(&mut self) -> ProblemDelta {
        self.java.finish_input();
        self.anr.finish_input();
        self.native.finish_input();
        self.drain_recognizers()
    }

    pub fn reset(&mut self) {
        self.java.reset();
        self.anr.reset();
        self.native.reset();
        self.index.reset();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::{
        EvidenceFlags, EvidenceFormat, EvidencePriority, FingerprintBuilder, IdentityQuality,
        LineProvenance, ObservationRef, ObservationRole, PackedLogTimestamp, ProblemFingerprint,
        ProblemKind, ProcessFingerprintKey, ProcessInstanceKey, RuleId, SignatureQuality,
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
}
