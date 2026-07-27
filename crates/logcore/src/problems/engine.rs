use super::budget::{
    vec_deque_usage, ProblemMemoryAccount, ProblemMemoryBudget, ProblemMemoryStats,
};
use super::correlation::{
    CompactCorrelationPayload, FinalizedProvisional, ProvisionalFinalizeReason, ProvisionalLimits,
    ProvisionalStore, RecentObservationLimits, RecentObservationStore,
};
use super::eventlog::parse_event_log;
use super::facts::{
    EvidencePriority, ObservationCandidate, ObservationRef, ObservationRole, RuleId,
};
use super::fingerprint::{FingerprintBuilder, FingerprintTokenKind, ProcessFingerprintKey};
use super::index::{
    AppendOutcome, GroupId, GroupKey, GroupPage, GroupQuery, GroupSnapshotCapture, GroupSortRecord,
    OccurrencePage, PageSpec, ProblemDisplaySummary, ProblemGroupSummary, ProblemIndex,
    ProblemIndexError, ProblemIndexLimits, ProblemStats, QuerySnapshotId, SnapshotError,
};
use super::model::{
    BoundaryFlags, EvidenceFlags, OutcomeFlags, PackedLogTimestamp, ProblemEvent,
    ProblemEventDraft, ProblemEventId, ProblemKind, ProcessInstanceKey, SignatureQuality,
    MAX_MATERIALIZED_OBSERVATIONS,
};
use super::provenance::{EvidenceAdmission, EvidenceFormat, InputCoverage, LineProvenance};
use super::recognizers::{
    parse_zygote_signal_exit, AnrRecognizer, JavaRecognizer, KernelOomOccurrence,
    KernelOomRecognizer, LifecycleDelta, LifecycleObservation, LifecycleObservationKind,
    LifecycleOccurrence, LifecycleRecognizer, LifecycleRecognizerError, LifecycleRelation,
    LifecycleTime, LmkMechanism, LmkOccurrence, LmkRecognizer, NativeRecognizer, ProblemRecognizer,
};
use super::timestamp::{
    parse_log_timestamp, ParsedLogTimestamp, SegmentedTimestamp, TimestampSegmentTracker,
};
use super::{classify_candidate, CandidateKinds};
use crate::parser::ParsedLine;
use std::cell::Cell;
use std::collections::VecDeque;
use std::mem::size_of;

const MANAGED_CORRELATION_WINDOW_LINES: u32 = 512;
const FAULT_OUTCOME_WINDOW_LINES: u32 = 4_096;
const MAX_RECENT_CLOCKS: usize = FAULT_OUTCOME_WINDOW_LINES as usize + 1;
const MAX_CORRELATION_TIME_DELTA_MS: u64 = 60_000;
const MAX_CORRELATION_PROCESS_BYTES: usize = 256;
const MAX_CORRELATION_WORK_UNITS_PER_OBSERVE: usize = 65_536;
const MAX_CORRELATION_WORK_UNITS_PER_FINISH: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CorrelationToken([u8; 16]);

impl CorrelationToken {
    fn from_domain_bytes(domain: &'static str, value: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key(domain);
        hasher.update(value);
        let digest = hasher.finalize();
        let mut token = [0; 16];
        token.copy_from_slice(&digest.as_bytes()[..16]);
        Self(token)
    }

    fn process(process: &ProcessFingerprintKey) -> Option<Self> {
        process.as_bytes().map(|value| {
            Self::from_domain_bytes("LogFilter Problems correlation process v1", value)
        })
    }

    fn subject(value: &[u8]) -> Self {
        Self::from_domain_bytes("LogFilter Problems correlation subject v1", value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CorrelationIdentity {
    process: CorrelationToken,
    subject: Option<CorrelationToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CorrelationProcessName {
    bytes: [u8; MAX_CORRELATION_PROCESS_BYTES],
    len: u16,
}

impl CorrelationProcessName {
    fn from_process(process: &ProcessFingerprintKey) -> Option<Self> {
        let value = process.as_bytes()?;
        let len = u16::try_from(value.len()).ok()?;
        let mut bytes = [0; MAX_CORRELATION_PROCESS_BYTES];
        bytes[..value.len()].copy_from_slice(value);
        Some(Self { bytes, len })
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("process fingerprint keys preserve valid UTF-8")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CorrelationSource {
    Body,
    Structured,
    Signal,
    Direct,
}

impl CorrelationSource {
    const fn bit(self) -> u8 {
        match self {
            Self::Body => 1 << 0,
            Self::Structured => 1 << 1,
            Self::Signal => 1 << 2,
            Self::Direct => 1 << 3,
        }
    }

    const fn slot(self) -> usize {
        match self {
            Self::Body => 0,
            Self::Structured => 1,
            Self::Signal => 2,
            Self::Direct => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CorrelationClock {
    segment: u64,
    millis: u64,
}

impl From<LifecycleTime> for CorrelationClock {
    fn from(value: LifecycleTime) -> Self {
        Self {
            segment: value.segment,
            millis: value.millis,
        }
    }
}

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
    observation_total: u16,
    correlation_identity: Option<CorrelationIdentity>,
    correlation_process: Option<CorrelationProcessName>,
    display_summary: ProblemDisplaySummary,
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
            observation_total: 1,
            correlation_identity: None,
            correlation_process: None,
            display_summary: ProblemDisplaySummary::default(),
        }
    }

    /// Adds only normalized, fixed-size identity metadata. The source process
    /// name and signature text are hashed immediately and never retained.
    pub(crate) fn set_correlation_identity(
        &mut self,
        process: &ProcessFingerprintKey,
        subject: Option<&[u8]>,
    ) {
        self.correlation_identity =
            CorrelationToken::process(process).map(|process| CorrelationIdentity {
                process,
                subject: subject.map(CorrelationToken::subject),
            });
        self.correlation_process = CorrelationProcessName::from_process(process);
    }

    pub(crate) fn set_display_summary(
        &mut self,
        process: &ProcessFingerprintKey,
        signature: Option<&str>,
    ) {
        let process = process
            .as_bytes()
            .and_then(|value| std::str::from_utf8(value).ok());
        self.display_summary = ProblemDisplaySummary::from_normalized(process, signature);
    }

    pub(crate) fn push_observation(&mut self, observation: ObservationCandidate) -> bool {
        if self
            .observations()
            .iter()
            .any(|known| known.reference.dedup_key() == observation.reference.dedup_key())
        {
            return true;
        }
        if self.observation_total == super::model::MAX_ADOPTED_OBSERVATIONS {
            self.draft
                .boundary
                .insert(BoundaryFlags::OBSERVATION_COUNT_LIMITED);
            return false;
        }
        self.observation_total += 1;
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

    pub(crate) const fn observation_total(&self) -> u16 {
        self.observation_total
    }

    fn correlation_source(&self) -> CorrelationSource {
        let primary = self.observations[0].reference;
        if primary.rule() == RuleId::NativeLibcSignalV1 {
            CorrelationSource::Signal
        } else if primary.format() == EvidenceFormat::EventLogShapedText
            || primary.rule() == RuleId::ManagedAmCrashV1
        {
            CorrelationSource::Structured
        } else if matches!(
            self.draft.kind,
            ProblemKind::JavaCrash
                | ProblemKind::JavaOom
                | ProblemKind::Anr
                | ProblemKind::NativeCrash
        ) {
            CorrelationSource::Body
        } else {
            CorrelationSource::Direct
        }
    }

    fn merge_from(&mut self, other: Self) {
        let left_total = self.observation_total;
        let right_total = other.observation_total;
        let duplicate_materialized = self
            .observations()
            .iter()
            .filter(|left| {
                other
                    .observations()
                    .iter()
                    .any(|right| left.reference.dedup_key() == right.reference.dedup_key())
            })
            .count() as u16;
        let combined_total = left_total
            .saturating_add(right_total)
            .saturating_sub(duplicate_materialized);
        let total_limited = combined_total > super::model::MAX_ADOPTED_OBSERVATIONS
            || self
                .draft
                .boundary
                .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED)
            || other
                .draft
                .boundary
                .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED);
        let left_draft = self.draft;
        let right_draft = other.draft;
        let left_summary = self.display_summary;
        let right_summary = other.display_summary;
        let self_is_better = canonical_problem_precedes(self, &other);
        let (canonical_draft, canonical_group_key, secondary_observations, secondary_len) =
            if self_is_better {
                (
                    self.draft,
                    self.group_key,
                    other.observations,
                    other.observation_len,
                )
            } else {
                let previous_observations = self.observations;
                let previous_len = self.observation_len;
                self.observations = other.observations;
                self.observation_len = other.observation_len;
                (
                    other.draft,
                    other.group_key,
                    previous_observations,
                    previous_len,
                )
            };

        let start_line = left_draft.start_line.min(right_draft.start_line);
        let end_line = left_draft.end_line.max(right_draft.end_line);
        let mut evidence = left_draft.evidence | right_draft.evidence;
        evidence.insert(EvidenceFlags::CORRELATED);
        let outcome = left_draft.outcome | right_draft.outcome;
        let boundary = left_draft.boundary | right_draft.boundary;
        let process_instance = if canonical_draft.process_instance != ProcessInstanceKey(0) {
            canonical_draft.process_instance
        } else if left_draft.process_instance != ProcessInstanceKey(0) {
            left_draft.process_instance
        } else {
            right_draft.process_instance
        };

        self.draft = ProblemEventDraft {
            start_line,
            end_line,
            process_instance,
            evidence,
            outcome,
            boundary,
            ..canonical_draft
        };
        self.group_key = canonical_group_key;
        self.display_summary = if self_is_better {
            left_summary
        } else {
            right_summary
        };
        for observation in secondary_observations
            .iter()
            .take(usize::from(secondary_len))
            .copied()
        {
            if !self
                .observations()
                .iter()
                .any(|known| known.reference.dedup_key() == observation.reference.dedup_key())
            {
                self.push_observation(observation);
            }
        }
        self.sort_observations();
        self.observation_total = combined_total.min(super::model::MAX_ADOPTED_OBSERVATIONS);
        if total_limited {
            self.draft
                .boundary
                .insert(BoundaryFlags::OBSERVATION_COUNT_LIMITED);
        }
        if self.observation_total > u16::from(self.observation_len) {
            self.draft
                .boundary
                .insert(BoundaryFlags::OBSERVATION_REFS_TRUNCATED);
        }
    }

    fn sort_observations(&mut self) {
        self.observations[..usize::from(self.observation_len)].sort_by_key(|candidate| {
            (
                candidate.priority,
                candidate.reference.line(),
                candidate.reference.rule() as u16,
                candidate.reference.role() as u8,
            )
        });
    }
}

fn canonical_problem_precedes(left: &RecognizedProblem, right: &RecognizedProblem) -> bool {
    let left_quality = left.group_key.signature_quality() as u8;
    let right_quality = right.group_key.signature_quality() as u8;
    left_quality < right_quality
        || (left_quality == right_quality
            && left.group_key.fingerprint().as_bytes() <= right.group_key.fingerprint().as_bytes())
}

#[derive(Debug, Clone)]
struct ProvisionalOccurrence {
    problem: RecognizedProblem,
    identity: CorrelationIdentity,
    source_mask: u8,
    clocks: [Option<CorrelationClock>; 4],
    ambiguity_recorded: bool,
}

impl ProvisionalOccurrence {
    fn new(
        problem: RecognizedProblem,
        identity: CorrelationIdentity,
        source: CorrelationSource,
        clock: Option<CorrelationClock>,
    ) -> Self {
        let mut clocks = [None; 4];
        clocks[source.slot()] = clock;
        Self {
            problem,
            identity,
            source_mask: source.bit(),
            clocks,
            ambiguity_recorded: false,
        }
    }

    fn max_line(&self) -> u32 {
        self.problem.draft.end_line
    }

    fn merge_occurrence(&mut self, other: Self) {
        if self.identity.subject.is_none() {
            self.identity.subject = other.identity.subject;
        }
        self.source_mask |= other.source_mask;
        for (slot, clock) in other.clocks.into_iter().enumerate() {
            if self.clocks[slot].is_none() {
                self.clocks[slot] = clock;
            }
        }
        self.ambiguity_recorded |= other.ambiguity_recorded;
        self.problem.merge_from(other.problem);
    }

    fn time_compatible(&self, clock: Option<CorrelationClock>) -> bool {
        let mut known = self.clocks.iter().flatten().copied();
        let Some(first) = known.next() else {
            return true;
        };
        correlation_time_compatible(Some(first), clock)
            || known.any(|candidate| correlation_time_compatible(Some(candidate), clock))
    }
}

impl CompactCorrelationPayload for ProvisionalOccurrence {
    fn logical_bytes(&self) -> u32 {
        size_of::<Self>() as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecentLifecycleObservation {
    kind: LifecycleObservationKind,
    line: u32,
    pid: u32,
    process: CorrelationToken,
    process_instance: ProcessInstanceKey,
    clock: Option<CorrelationClock>,
    reference: ObservationCandidate,
    claimed_by: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CorrelationTarget {
    None,
    Unique(u64),
    Ambiguous,
}

impl CompactCorrelationPayload for RecentLifecycleObservation {
    fn logical_bytes(&self) -> u32 {
        size_of::<Self>() as u32
    }
}

/// Deep-module owner for recognizer state and the compact query index.
#[derive(Debug)]
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
    provisional: ProvisionalStore<ProvisionalOccurrence>,
    recent: RecentObservationStore<RecentLifecycleObservation>,
    recent_clocks: VecDeque<(u32, CorrelationClock)>,
    memory_budget: ProblemMemoryBudget,
    recent_clocks_memory: ProblemMemoryAccount,
    correlation_capacity_limited: bool,
    correlation_ambiguity_count: u64,
    correlation_work_remaining: Cell<usize>,
    correlation_work_used: Cell<usize>,
    correlation_work_exhausted: Cell<bool>,
    #[cfg(test)]
    recognizer_observe_counts: [u64; 6],
}

impl Default for ProblemEngine {
    fn default() -> Self {
        Self::with_limits_and_correlation(
            ProblemIndexLimits::default(),
            ProvisionalLimits::default(),
            RecentObservationLimits::default(),
        )
        .expect("the documented Problems engine limits are valid")
    }
}

impl ProblemEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(limits: ProblemIndexLimits) -> Result<Self, ProblemIndexError> {
        Self::with_limits_and_correlation(
            limits,
            ProvisionalLimits::default(),
            RecentObservationLimits::default(),
        )
    }

    pub fn with_limits_and_correlation(
        limits: ProblemIndexLimits,
        provisional_limits: ProvisionalLimits,
        recent_limits: RecentObservationLimits,
    ) -> Result<Self, ProblemIndexError> {
        Self::with_limits_correlation_and_budget(
            limits,
            provisional_limits,
            recent_limits,
            ProblemMemoryBudget::new(),
        )
    }

    pub(crate) fn with_limits_correlation_and_budget(
        limits: ProblemIndexLimits,
        provisional_limits: ProvisionalLimits,
        recent_limits: RecentObservationLimits,
        memory_budget: ProblemMemoryBudget,
    ) -> Result<Self, ProblemIndexError> {
        let recent_clocks_memory = memory_budget.account();
        Ok(Self {
            index: ProblemIndex::with_limits_and_budget(limits, memory_budget.clone())?,
            java: JavaRecognizer::default(),
            anr: AnrRecognizer::default(),
            native: NativeRecognizer::new(),
            lifecycle: LifecycleRecognizer::with_budget(memory_budget.clone()),
            lmk: LmkRecognizer::new(),
            kernel_oom: KernelOomRecognizer::new(),
            timestamps: TimestampSegmentTracker::new(),
            timestamp_origin: None,
            provisional: ProvisionalStore::with_limits_and_budget(
                provisional_limits,
                memory_budget.clone(),
            ),
            recent: RecentObservationStore::with_limits_and_budget(
                recent_limits,
                memory_budget.clone(),
            ),
            recent_clocks: VecDeque::new(),
            memory_budget,
            recent_clocks_memory,
            correlation_capacity_limited: false,
            correlation_ambiguity_count: 0,
            correlation_work_remaining: Cell::new(MAX_CORRELATION_WORK_UNITS_PER_OBSERVE),
            correlation_work_used: Cell::new(0),
            correlation_work_exhausted: Cell::new(false),
            #[cfg(test)]
            recognizer_observe_counts: [0; 6],
        })
    }

    pub fn stats(&self) -> ProblemStats {
        let mut stats = self.index.stats();
        let provisional = self.provisional.stats();
        let recent = self.recent.stats();
        stats.provisional_occurrence_count =
            u32::try_from(provisional.provisional_occurrence_count).unwrap_or(u32::MAX);
        stats.dropped_recent_observation_count = recent.dropped_recent_observation_count;
        stats.correlation_limited = self.correlation_capacity_limited
            || self.correlation_work_exhausted.get()
            || recent.correlation_limited;
        let identity = self.lifecycle.tracker().stats();
        stats.identity_eviction_count = identity
            .active_eviction_count()
            .saturating_add(identity.recent_eviction_count())
            .saturating_add(identity.budget_drop_count());
        stats.identity_coverage_limited = identity.identity_coverage_limited();
        stats.pending_eviction_count = self
            .lifecycle
            .pending_eviction_count()
            .saturating_add(self.lmk.pending_eviction_count())
            .saturating_add(self.kernel_oom.pending_eviction_count());
        stats.pending_coverage_limited = stats.pending_eviction_count != 0;
        stats.limited |= stats.correlation_limited
            || stats.identity_coverage_limited
            || stats.pending_coverage_limited;
        stats
    }

    pub fn memory_stats(&self) -> ProblemMemoryStats {
        self.memory_budget.stats()
    }

    pub fn event(&self, id: ProblemEventId) -> Option<ProblemEvent> {
        self.index.event(id)
    }

    pub const fn correlation_ambiguity_count(&self) -> u64 {
        self.correlation_ambiguity_count
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

    pub fn group_snapshot_capture(&self) -> GroupSnapshotCapture {
        self.index.group_snapshot_capture()
    }

    pub fn group_sort_records(
        &self,
        query: &GroupQuery,
        capture: GroupSnapshotCapture,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<GroupSortRecord>, SnapshotError> {
        self.index.group_sort_records(query, capture, offset, limit)
    }

    pub fn install_group_snapshot_ids(
        &mut self,
        ids: Vec<GroupId>,
        revision: u64,
        query: GroupQuery,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.index.install_group_snapshot_ids(ids, revision, query)
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

    pub fn group_snapshot_page_for_query(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
        query: GroupQuery,
    ) -> Result<GroupPage, SnapshotError> {
        self.index
            .group_snapshot_page_for_query(snapshot, page, query)
    }

    pub fn occurrence_snapshot_page(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.index.occurrence_snapshot_page(snapshot, page)
    }

    pub fn occurrence_snapshot_page_for_group(
        &mut self,
        snapshot: QuerySnapshotId,
        page: PageSpec,
        group: GroupId,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.index
            .occurrence_snapshot_page_for_group(snapshot, page, group)
    }

    pub fn release_snapshot(&mut self, snapshot: QuerySnapshotId) -> bool {
        self.index.release_snapshot(snapshot)
    }

    pub(crate) fn requires_full_line(&self) -> bool {
        self.java.has_pending()
            || self.anr.has_pending()
            || self.native.has_pending()
            || self.kernel_oom.pending_count() != 0
            || self.lmk.pending_count() != 0
    }

    /// Advance timestamp/correlation state for a line proven by the byte gate not to
    /// contain a candidate start. This keeps every physical line in watermark and
    /// timestamp-segment semantics without decoding or fully parsing ordinary text.
    pub(crate) fn observe_non_candidate(
        &mut self,
        line: u32,
        timestamp: Option<ParsedLogTimestamp>,
    ) -> ProblemDelta {
        if self.timestamps.observe_probe(timestamp).is_none() {
            self.timestamp_origin = None;
        }
        if self.provisional.next_finalize_after_line().is_none()
            && self.recent.next_expiry_line().is_none()
        {
            return ProblemDelta::default();
        }
        self.begin_correlation_work(MAX_CORRELATION_WORK_UNITS_PER_OBSERVE);
        let mut delta = ProblemDelta::default();
        self.advance_correlation(u64::from(line), &mut delta);
        self.finish_correlation_work();
        delta
    }

    pub fn observe(&mut self, line: ObservedLine<'_>) -> ProblemDelta {
        self.begin_correlation_work(MAX_CORRELATION_WORK_UNITS_PER_OBSERVE);
        let candidates = classify_candidate(&line.parsed, line.raw);
        let (anchor_timestamp, lifecycle_time) = self.observe_timestamp(&line);
        let clock = lifecycle_time.map(CorrelationClock::from);
        self.remember_clock(
            line.line,
            (!candidates.is_empty()).then_some(clock).flatten(),
        );
        let mut delta = ProblemDelta::default();
        self.advance_correlation(u64::from(line.line), &mut delta);
        let lifecycle = if candidates.contains(CandidateKinds::LIFECYCLE) {
            #[cfg(test)]
            {
                self.recognizer_observe_counts[3] =
                    self.recognizer_observe_counts[3].saturating_add(1);
            }
            self.observe_lifecycle(&line, lifecycle_time)
        } else {
            Ok(LifecycleDelta::default())
        };
        let kernel_oom = if candidates.contains(CandidateKinds::KERNEL_OOM)
            || self.kernel_oom.pending_count() != 0
        {
            #[cfg(test)]
            {
                self.recognizer_observe_counts[4] =
                    self.recognizer_observe_counts[4].saturating_add(1);
            }
            self.kernel_oom.observe(
                line.line,
                line.parsed.tag,
                line.parsed.message.as_bytes(),
                line.provenance,
            )
        } else {
            None
        };
        let lmk = (kernel_oom.is_none() && candidates.contains(CandidateKinds::LMK)).then(|| {
            #[cfg(test)]
            {
                self.recognizer_observe_counts[5] =
                    self.recognizer_observe_counts[5].saturating_add(1);
            }
            self.lmk.observe(
                line.line,
                line.parsed.tag,
                line.parsed.message.as_bytes(),
                line.provenance,
            )
        });

        let mut lifecycle_occurrence = None;
        match lifecycle {
            Ok(lifecycle_delta) => {
                if let Some(observation) = lifecycle_delta.observation {
                    self.record_lifecycle_observation(observation, clock);
                }
                lifecycle_occurrence = lifecycle_delta.occurrence;
            }
            Err(_) => delta.failed = delta.failed.saturating_add(1),
        }
        if candidates.contains(CandidateKinds::JAVA_FATAL)
            || candidates.contains(CandidateKinds::JAVA_OOM)
            || candidates.contains(CandidateKinds::EVENT_LOG)
            || candidates.contains(CandidateKinds::CONTINUATION)
            || self.java.has_pending()
        {
            #[cfg(test)]
            {
                self.recognizer_observe_counts[0] =
                    self.recognizer_observe_counts[0].saturating_add(1);
            }
            self.java.observe(&line);
        }
        if candidates.contains(CandidateKinds::ANR)
            || candidates.contains(CandidateKinds::CONTINUATION)
            || self.anr.has_pending()
        {
            #[cfg(test)]
            {
                self.recognizer_observe_counts[1] =
                    self.recognizer_observe_counts[1].saturating_add(1);
            }
            self.anr.observe(&line);
        }
        if candidates.contains(CandidateKinds::NATIVE_CRASH)
            || candidates.contains(CandidateKinds::EVENT_LOG)
            || candidates.contains(CandidateKinds::CONTINUATION)
            || self.native.has_pending()
        {
            #[cfg(test)]
            {
                self.recognizer_observe_counts[2] =
                    self.recognizer_observe_counts[2].saturating_add(1);
            }
            self.native.observe(&line);
        }
        self.drain_recognizers(clock, &mut delta);
        if let Some(occurrence) = lifecycle_occurrence {
            let problem = recognized_lifecycle(occurrence, anchor_timestamp);
            self.admit_provisional(problem, clock, &mut delta);
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
            self.admit_provisional(problem, clock, &mut delta);
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
            self.admit_provisional(problem, clock, &mut delta);
        }
        self.finish_correlation_work();
        delta
    }

    pub fn finish_input(&mut self) -> ProblemDelta {
        self.begin_correlation_work(MAX_CORRELATION_WORK_UNITS_PER_FINISH);
        self.java.finish_input();
        self.anr.finish_input();
        self.native.finish_input();
        self.lifecycle.finish_input();
        self.lmk.finish_input();
        self.kernel_oom.finish_input();
        let mut delta = ProblemDelta::default();
        self.drain_recognizers(None, &mut delta);
        self.resolve_safe_fault_pairs(u64::MAX);
        self.resolve_all_unclaimed_recent();
        for finalized in self.provisional.finish() {
            self.commit_finalized(finalized, &mut delta);
        }
        self.recent.finish();
        self.finish_correlation_work();
        delta
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
        self.provisional.reset();
        self.recent.reset();
        self.recent_clocks = VecDeque::new();
        self.recent_clocks_memory.release();
        self.correlation_capacity_limited = false;
        self.correlation_ambiguity_count = 0;
        self.begin_correlation_work(MAX_CORRELATION_WORK_UNITS_PER_OBSERVE);
        #[cfg(test)]
        {
            self.recognizer_observe_counts = [0; 6];
        }
        self.index.reset();
        self.memory_budget.clear_limit_state();
    }

    fn begin_correlation_work(&self, limit: usize) {
        self.correlation_work_remaining.set(limit);
        self.correlation_work_used.set(0);
        self.correlation_work_exhausted.set(false);
    }

    fn consume_correlation_work(&self) -> bool {
        let remaining = self.correlation_work_remaining.get();
        if remaining == 0 {
            self.correlation_work_exhausted.set(true);
            return false;
        }
        self.correlation_work_remaining.set(remaining - 1);
        self.correlation_work_used
            .set(self.correlation_work_used.get().saturating_add(1));
        true
    }

    fn finish_correlation_work(&mut self) {
        if self.correlation_work_exhausted.get() {
            self.correlation_capacity_limited = true;
        }
    }

    fn advance_correlation(&mut self, watermark: u64, delta: &mut ProblemDelta) {
        if self
            .provisional
            .next_finalize_after_line()
            .is_some_and(|deadline| watermark > deadline)
        {
            self.resolve_safe_fault_pairs(watermark);
            if self
                .provisional
                .next_finalize_after_line()
                .is_some_and(|deadline| watermark > deadline)
            {
                for finalized in self.provisional.advance_watermark(watermark) {
                    self.commit_finalized(finalized, delta);
                }
            }
        }
        if self
            .recent
            .next_expiry_line()
            .is_some_and(|expiry| watermark > expiry)
        {
            let expiring_recent = self
                .recent
                .iter()
                .take_while(|observation| watermark > observation.expiry_line())
                .take_while(|_| self.consume_correlation_work())
                .filter(|observation| observation.payload().claimed_by.is_none())
                .map(|observation| observation.sequence())
                .collect::<Vec<_>>();
            for sequence in expiring_recent {
                self.attach_recent_observation(sequence);
            }
            self.recent.advance_watermark(watermark);
        }
    }

    fn remember_clock(&mut self, line: u32, clock: Option<CorrelationClock>) {
        let oldest = line.saturating_sub(FAULT_OUTCOME_WINDOW_LINES);
        while self
            .recent_clocks
            .front()
            .is_some_and(|(known_line, _)| *known_line < oldest)
        {
            self.recent_clocks.pop_front();
        }
        if let Some(clock) = clock {
            if let Some((known_line, known_clock)) = self.recent_clocks.back_mut() {
                if *known_line == line {
                    *known_clock = clock;
                    return;
                }
            }
            while self.recent_clocks.len() >= MAX_RECENT_CLOCKS {
                self.recent_clocks.pop_front();
                self.correlation_capacity_limited = true;
            }
            if self.recent_clocks.capacity() < MAX_RECENT_CLOCKS {
                let Ok(projection) = vec_deque_usage::<(u32, CorrelationClock)>(MAX_RECENT_CLOCKS)
                else {
                    self.correlation_capacity_limited = true;
                    return;
                };
                if self.recent_clocks_memory.try_set_usage(projection).is_err() {
                    self.correlation_capacity_limited = true;
                    return;
                }
                let additional = MAX_RECENT_CLOCKS.saturating_sub(self.recent_clocks.len());
                if self.recent_clocks.try_reserve_exact(additional).is_err() {
                    self.correlation_capacity_limited = true;
                    self.settle_recent_clock_memory();
                    return;
                }
                self.settle_recent_clock_memory();
            }
            self.recent_clocks.push_back((line, clock));
        }
    }

    fn settle_recent_clock_memory(&mut self) {
        let Ok(usage) = vec_deque_usage::<(u32, CorrelationClock)>(self.recent_clocks.capacity())
        else {
            return;
        };
        if usage.charged_bytes <= self.recent_clocks_memory.usage().charged_bytes {
            self.recent_clocks_memory.settle_precharged(usage);
        } else {
            let _ = self.recent_clocks_memory.try_set_usage(usage);
        }
    }

    fn clock_for_line(&self, line: u32) -> Option<CorrelationClock> {
        self.recent_clocks
            .iter()
            .rev()
            .find_map(|(known_line, clock)| (*known_line == line).then_some(*clock))
    }

    fn commit_finalized(
        &mut self,
        mut finalized: FinalizedProvisional<ProvisionalOccurrence>,
        delta: &mut ProblemDelta,
    ) {
        if finalized.reason == ProvisionalFinalizeReason::Capacity
            || self.correlation_work_exhausted.get()
        {
            finalized
                .entry
                .payload_mut()
                .problem
                .draft
                .boundary
                .insert(BoundaryFlags::CORRELATION_LIMITED);
            self.correlation_capacity_limited = true;
        }
        let problem = finalized.entry.into_payload().problem;
        self.commit_or_count_failure(problem, delta);
    }

    fn drain_recognizers(&mut self, clock: Option<CorrelationClock>, delta: &mut ProblemDelta) {
        while let Some(problem) = self.java.pop_ready() {
            self.admit_provisional(problem, clock, delta);
        }
        while let Some(problem) = self.anr.pop_ready() {
            self.admit_provisional(problem, clock, delta);
        }
        while let Some(problem) = self.native.pop_ready() {
            self.admit_provisional(problem, clock, delta);
        }
        while let Some(signal) = self.native.pop_signal_ready() {
            let explicit_process = signal.explicit_process_ref();
            let (process, process_instance) = match explicit_process {
                Some(explicit_process) => {
                    let instance = self
                        .lifecycle
                        .tracker()
                        .active_for_pid(signal.pid())
                        .filter(|active| active.process_name() == explicit_process)
                        .map_or(ProcessInstanceKey(0), |active| active.instance().key());
                    (ProcessFingerprintKey::new(Some(explicit_process)), instance)
                }
                None => self
                    .lifecycle
                    .tracker()
                    .active_for_pid(signal.pid())
                    .map(|active| {
                        (
                            ProcessFingerprintKey::new(Some(active.process_name())),
                            active.instance().key(),
                        )
                    })
                    .unwrap_or_else(|| (ProcessFingerprintKey::unknown(), ProcessInstanceKey(0))),
            };
            self.admit_provisional(
                signal.into_problem(&process, process_instance),
                clock,
                delta,
            );
        }
    }

    fn admit_provisional(
        &mut self,
        mut problem: RecognizedProblem,
        clock: Option<CorrelationClock>,
        delta: &mut ProblemDelta,
    ) {
        let clock = self.clock_for_line(problem.draft.anchor_line).or(clock);
        let Some(identity) = problem.correlation_identity else {
            self.commit_or_count_failure(problem, delta);
            return;
        };
        self.enrich_problem_instance(&mut problem, identity.process);
        if self.correlation_work_exhausted.get() {
            problem
                .draft
                .boundary
                .insert(BoundaryFlags::CORRELATION_LIMITED);
            self.correlation_capacity_limited = true;
        }
        let source = problem.correlation_source();
        let deadline = provisional_deadline(problem.draft.end_line);
        let payload = ProvisionalOccurrence::new(problem, identity, source, clock);
        match self.provisional.insert(deadline, payload.clone()) {
            Ok(outcome) => {
                self.extend_cross_source_candidates(outcome.sequence);
                let open_observation_deadline = self
                    .provisional
                    .iter()
                    .find(|entry| entry.sequence() == outcome.sequence)
                    .and_then(|entry| {
                        self.recent
                            .iter()
                            .filter(|observation| {
                                observation.payload().claimed_by.is_none()
                                    && can_attach_lifecycle(entry.payload(), observation.payload())
                            })
                            .map(|observation| observation.expiry_line())
                            .max()
                    });
                if let Some(open_observation_deadline) = open_observation_deadline {
                    self.provisional
                        .extend_finalize_after_line(outcome.sequence, open_observation_deadline);
                }
                for finalized in outcome.finalized {
                    self.commit_finalized(finalized, delta);
                }
            }
            Err(_) => {
                self.correlation_capacity_limited = true;
                // Sequence exhaustion is not recoverable within a session.
                // Preserve the already-proven occurrence and make the loss of
                // possible late evidence explicit.
                let mut payload = payload;
                payload
                    .problem
                    .draft
                    .boundary
                    .insert(BoundaryFlags::CORRELATION_LIMITED);
                self.commit_or_count_failure(payload.problem, delta);
            }
        }
    }

    fn merge_target(
        &self,
        problem: &RecognizedProblem,
        identity: CorrelationIdentity,
        source: CorrelationSource,
        clock: Option<CorrelationClock>,
    ) -> CorrelationTarget {
        let mut nearest: Option<(u32, u64)> = None;
        let mut tied = false;
        for entry in self.provisional.iter() {
            if !self.consume_correlation_work() {
                return CorrelationTarget::Ambiguous;
            }
            let candidate = entry.payload();
            if !can_merge_faults(candidate, problem, identity, source, clock)
                || self.has_start_barrier(
                    problem.draft.pid,
                    candidate.problem.draft.start_line,
                    problem.draft.end_line,
                )
            {
                continue;
            }
            let distance = range_distance(
                candidate.problem.draft.start_line,
                candidate.problem.draft.end_line,
                problem.draft.start_line,
                problem.draft.end_line,
            );
            match nearest {
                None => {
                    nearest = Some((distance, entry.sequence()));
                    tied = false;
                }
                Some((known, _)) if distance < known => {
                    nearest = Some((distance, entry.sequence()));
                    tied = false;
                }
                Some((known, _)) if distance == known => tied = true,
                Some(_) => {}
            }
        }
        if tied {
            CorrelationTarget::Ambiguous
        } else if let Some((_, sequence)) = nearest {
            CorrelationTarget::Unique(sequence)
        } else {
            CorrelationTarget::None
        }
    }

    fn merge_target_for_sequence(&self, sequence: u64) -> CorrelationTarget {
        let Some(candidate) = self
            .provisional
            .iter()
            .find(|entry| entry.sequence() == sequence)
            .map(|entry| entry.payload())
        else {
            return CorrelationTarget::None;
        };
        if candidate.source_mask.count_ones() != 1 {
            return CorrelationTarget::None;
        }
        let source = candidate.problem.correlation_source();
        self.merge_target(
            &candidate.problem,
            candidate.identity,
            source,
            candidate.clocks[source.slot()],
        )
    }

    fn extend_cross_source_candidates(&mut self, sequence: u64) {
        let Some(incoming) = self
            .provisional
            .iter()
            .find(|entry| entry.sequence() == sequence)
            .map(|entry| entry.payload().clone())
        else {
            return;
        };
        if incoming.source_mask.count_ones() != 1 {
            return;
        }
        let source = incoming.problem.correlation_source();
        if source == CorrelationSource::Direct {
            return;
        }
        let clock = incoming.clocks[source.slot()];
        let incoming_safe_after = cross_source_safe_after(&incoming);
        let matches = self
            .provisional
            .iter()
            .filter(|entry| entry.sequence() != sequence)
            .filter(|entry| {
                self.consume_correlation_work()
                    && can_merge_faults(
                        entry.payload(),
                        &incoming.problem,
                        incoming.identity,
                        source,
                        clock,
                    )
                    && !self.has_start_barrier(
                        incoming.problem.draft.pid,
                        entry.payload().problem.draft.start_line,
                        incoming.problem.draft.end_line,
                    )
            })
            .map(|entry| {
                (
                    entry.sequence(),
                    incoming_safe_after.max(cross_source_safe_after(entry.payload())),
                )
            })
            .collect::<Vec<_>>();
        let mut incoming_deadline = None;
        for (candidate, deadline) in matches {
            self.provisional
                .extend_finalize_after_line(candidate, deadline);
            incoming_deadline =
                Some(incoming_deadline.map_or(deadline, |known: u64| known.max(deadline)));
        }
        if let Some(deadline) = incoming_deadline {
            self.provisional
                .extend_finalize_after_line(sequence, deadline);
        }
        if self.correlation_work_exhausted.get() {
            self.correlation_capacity_limited = true;
            if let Some(entry) = self
                .provisional
                .iter_mut()
                .find(|entry| entry.sequence() == sequence)
            {
                entry
                    .payload_mut()
                    .problem
                    .draft
                    .boundary
                    .insert(BoundaryFlags::CORRELATION_LIMITED);
            }
        }
    }

    fn resolve_safe_fault_pairs(&mut self, watermark: u64) {
        let ready_sequences = self
            .provisional
            .iter()
            .filter(|entry| watermark > entry.finalize_after_line())
            .take_while(|_| self.consume_correlation_work())
            .map(|entry| entry.sequence())
            .collect::<Vec<_>>();
        if ready_sequences.is_empty() {
            return;
        }
        let mut pairs = Vec::new();
        let mut ambiguous = Vec::new();
        for sequence in ready_sequences {
            match self.merge_target_for_sequence(sequence) {
                CorrelationTarget::Ambiguous => ambiguous.push(sequence),
                CorrelationTarget::Unique(target) => {
                    let target_is_safe = self
                        .provisional
                        .iter()
                        .find(|entry| entry.sequence() == target)
                        .is_some_and(|entry| watermark > cross_source_safe_after(entry.payload()));
                    if target_is_safe {
                        match self.merge_target_for_sequence(target) {
                            CorrelationTarget::Unique(reverse) if reverse == sequence => {
                                if sequence < target {
                                    pairs.push((sequence, target));
                                }
                            }
                            CorrelationTarget::Ambiguous => ambiguous.push(target),
                            CorrelationTarget::None | CorrelationTarget::Unique(_) => {}
                        }
                    }
                }
                CorrelationTarget::None => {}
            }
            if self.correlation_work_exhausted.get() {
                self.correlation_capacity_limited = true;
                break;
            }
        }
        for sequence in ambiguous {
            let Some(entry) = self
                .provisional
                .iter_mut()
                .find(|entry| entry.sequence() == sequence)
            else {
                continue;
            };
            if !entry.payload().ambiguity_recorded {
                entry.payload_mut().ambiguity_recorded = true;
                self.correlation_ambiguity_count =
                    self.correlation_ambiguity_count.saturating_add(1);
            }
        }
        for (keep_sequence, remove_sequence) in pairs {
            let Some(removed) = self.provisional.remove(remove_sequence) else {
                continue;
            };
            let removed_deadline = removed.finalize_after_line();
            let removed = removed.into_payload();
            let Some(keep) = self
                .provisional
                .iter_mut()
                .find(|entry| entry.sequence() == keep_sequence)
            else {
                continue;
            };
            keep.payload_mut().merge_occurrence(removed);
            let new_deadline = removed_deadline
                .max(provisional_deadline(keep.payload().max_line()))
                .max(keep.finalize_after_line());
            self.provisional
                .extend_finalize_after_line(keep_sequence, new_deadline);
        }
    }

    fn enrich_problem_instance(
        &mut self,
        problem: &mut RecognizedProblem,
        process: CorrelationToken,
    ) {
        if problem.draft.process_instance != ProcessInstanceKey(0) {
            return;
        }
        let Some(process_name) = problem.correlation_process else {
            return;
        };
        if problem.draft.pid == 0 {
            return;
        }
        if let Some(active) = self.lifecycle.tracker().active_for_pid(problem.draft.pid) {
            let tracked = ProcessFingerprintKey::new(Some(active.process_name()));
            if active.start_line() <= problem.draft.end_line
                && CorrelationToken::process(&tracked) == Some(process)
            {
                problem.draft.process_instance = active.instance().key();
            }
            return;
        }

        let mut historical = None;
        let mut historical_ambiguous = false;
        for observation in self
            .recent
            .iter()
            .map(|entry| entry.payload())
            .filter(|observation| {
                self.consume_correlation_work()
                    && observation.pid == problem.draft.pid
                    && observation.process == process
                    && observation.process_instance != ProcessInstanceKey(0)
                    && !self.has_start_barrier(
                        problem.draft.pid,
                        problem.draft.end_line,
                        observation.line,
                    )
            })
        {
            match historical {
                None => historical = Some(observation.process_instance),
                Some(known) if known != observation.process_instance => {
                    historical_ambiguous = true;
                }
                Some(_) => {}
            }
        }
        if !historical_ambiguous {
            if let Some(instance) = historical {
                problem.draft.process_instance = instance;
                return;
            }
        }
        if historical_ambiguous
            || self.recent.iter().any(|entry| {
                if !self.consume_correlation_work() {
                    return true;
                }
                let observation = entry.payload();
                observation.pid == problem.draft.pid
                    && observation.line > problem.draft.end_line
                    && matches!(
                        observation.kind,
                        LifecycleObservationKind::Start | LifecycleObservationKind::Death
                    )
            })
        {
            return;
        }
        match self.lifecycle.observe_fault_identity(
            problem.draft.anchor_line,
            problem.draft.pid,
            process_name.as_str(),
        ) {
            Ok(instance) => problem.draft.process_instance = instance,
            Err(_) => {
                self.correlation_capacity_limited = true;
            }
        }
    }

    fn has_start_barrier(&self, pid: u32, left_line: u32, right_line: u32) -> bool {
        let start = left_line.min(right_line);
        let end = left_line.max(right_line);
        self.recent.iter().any(|observation| {
            if !self.consume_correlation_work() {
                return true;
            }
            let observation = observation.payload();
            observation.kind == LifecycleObservationKind::Start
                && observation.pid == pid
                && (start..=end).contains(&observation.line)
        })
    }

    fn record_lifecycle_observation(
        &mut self,
        observation: LifecycleObservation,
        clock: Option<CorrelationClock>,
    ) {
        let process = ProcessFingerprintKey::new(Some(observation.process.as_str()));
        let Some(process) = CorrelationToken::process(&process) else {
            return;
        };
        let Some(reference) = lifecycle_observation_candidate(observation) else {
            return;
        };
        let process_instance = self.lifecycle_observation_instance(observation, process);
        let payload = RecentLifecycleObservation {
            kind: observation.kind,
            line: observation.line,
            pid: observation.pid,
            process,
            process_instance,
            clock,
            reference,
            claimed_by: None,
        };
        let expiry = u64::from(observation.line.saturating_add(FAULT_OUTCOME_WINDOW_LINES));
        match self
            .recent
            .insert(u64::from(observation.line), expiry, payload)
        {
            Ok(outcome) if outcome.retained => {
                let candidates = self
                    .provisional
                    .iter()
                    .filter(|entry| {
                        self.consume_correlation_work()
                            && can_attach_lifecycle(entry.payload(), &payload)
                    })
                    .map(|entry| entry.sequence())
                    .collect::<Vec<_>>();
                for sequence in candidates {
                    self.provisional
                        .extend_finalize_after_line(sequence, expiry);
                }
            }
            Ok(_) => {}
            Err(_) => {
                // The only possible error is a session-long sequence exhaustion.
                // Surface it through the same conservative limited state.
                self.correlation_capacity_limited = true;
            }
        }
    }

    fn lifecycle_observation_instance(
        &self,
        observation: LifecycleObservation,
        process: CorrelationToken,
    ) -> ProcessInstanceKey {
        if observation.process_instance != ProcessInstanceKey(0) {
            return observation.process_instance;
        }
        match observation.kind {
            LifecycleObservationKind::Start => ProcessInstanceKey(0),
            LifecycleObservationKind::KillRequest => {
                let active = self
                    .lifecycle
                    .tracker()
                    .active_for_pid(observation.pid)
                    .filter(|active| {
                        let tracked = ProcessFingerprintKey::new(Some(active.process_name()));
                        CorrelationToken::process(&tracked) == Some(process)
                    })
                    .map(|active| active.instance().key());
                if let Some(active) = active {
                    return active;
                }
                let mut historical = None;
                for known in self
                    .recent
                    .iter()
                    .map(|entry| entry.payload())
                    .filter(|known| {
                        self.consume_correlation_work()
                            && known.pid == observation.pid
                            && known.process == process
                            && known.kind == LifecycleObservationKind::Death
                            && known.process_instance != ProcessInstanceKey(0)
                            && !self.has_start_barrier(
                                observation.pid,
                                known.line,
                                observation.line,
                            )
                    })
                {
                    match historical {
                        None => historical = Some(known.process_instance),
                        Some(instance) if instance == known.process_instance => {}
                        Some(_) => return ProcessInstanceKey(0),
                    }
                }
                historical.unwrap_or(ProcessInstanceKey(0))
            }
            LifecycleObservationKind::Death => ProcessInstanceKey(0),
        }
    }

    fn resolve_all_unclaimed_recent(&mut self) {
        let sequences = self
            .recent
            .iter()
            .take_while(|_| self.consume_correlation_work())
            .filter(|observation| observation.payload().claimed_by.is_none())
            .map(|observation| observation.sequence())
            .collect::<Vec<_>>();
        for sequence in sequences {
            self.attach_recent_observation(sequence);
        }
    }

    fn attach_recent_observation(&mut self, recent_sequence: u64) {
        match self.lifecycle_target(recent_sequence) {
            CorrelationTarget::Unique(target) => {
                self.claim_lifecycle_observation(recent_sequence, target);
            }
            CorrelationTarget::Ambiguous => {
                self.correlation_ambiguity_count =
                    self.correlation_ambiguity_count.saturating_add(1);
            }
            CorrelationTarget::None => {}
        }
    }

    fn lifecycle_target(&self, recent_sequence: u64) -> CorrelationTarget {
        let Some(observation) = self
            .recent
            .iter()
            .find(|observation| observation.sequence() == recent_sequence)
            .map(|observation| observation.payload())
        else {
            return CorrelationTarget::None;
        };
        if observation.claimed_by.is_some() || observation.kind == LifecycleObservationKind::Start {
            return CorrelationTarget::None;
        }
        let mut nearest: Option<(u32, u64)> = None;
        let mut tied = false;
        for entry in self.provisional.iter() {
            if !self.consume_correlation_work() {
                return CorrelationTarget::Ambiguous;
            }
            if !can_attach_lifecycle(entry.payload(), observation) {
                continue;
            }
            let distance = line_to_range_distance(
                observation.line,
                entry.payload().problem.draft.start_line,
                entry.payload().problem.draft.end_line,
            );
            match nearest {
                None => {
                    nearest = Some((distance, entry.sequence()));
                    tied = false;
                }
                Some((known, _)) if distance < known => {
                    nearest = Some((distance, entry.sequence()));
                    tied = false;
                }
                Some((known, _)) if distance == known => tied = true,
                Some(_) => {}
            }
        }
        if tied {
            CorrelationTarget::Ambiguous
        } else if let Some((_, sequence)) = nearest {
            CorrelationTarget::Unique(sequence)
        } else {
            CorrelationTarget::None
        }
    }

    fn claim_lifecycle_observation(&mut self, recent_sequence: u64, target_sequence: u64) {
        let Some(observation) = self
            .recent
            .iter()
            .find(|observation| observation.sequence() == recent_sequence)
            .map(|observation| *observation.payload())
        else {
            return;
        };
        let Some(target) = self
            .provisional
            .iter_mut()
            .find(|entry| entry.sequence() == target_sequence)
        else {
            return;
        };
        apply_lifecycle_observation(target.payload_mut(), observation);
        if let Some(recent) = self
            .recent
            .iter_mut()
            .find(|recent| recent.sequence() == recent_sequence)
        {
            recent.payload_mut().claimed_by = Some(target_sequence);
        }
    }

    fn observe_timestamp(
        &mut self,
        line: &ObservedLine<'_>,
    ) -> (PackedLogTimestamp, Option<LifecycleTime>) {
        let parsed = parse_log_timestamp(line.parsed.date, line.parsed.time);
        self.observe_timestamp_value(parsed)
    }

    fn observe_timestamp_value(
        &mut self,
        parsed: Option<PackedLogTimestamp>,
    ) -> (PackedLogTimestamp, Option<LifecycleTime>) {
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
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
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
                    return Ok(LifecycleDelta::default());
                }
                let Ok(record) = parse_event_log(line.parsed.tag, line.parsed.message) else {
                    return Ok(LifecycleDelta::default());
                };
                self.lifecycle
                    .observe_event_log(line.line, record, provenance, time)?
            }
            "Zygote" => {
                let Some((pid, signal)) = parse_zygote_signal_exit(line.parsed.message.as_bytes())
                else {
                    return Ok(LifecycleDelta::default());
                };
                self.lifecycle.observe_signal_exit_with_provenance(
                    line.line,
                    pid,
                    signal,
                    line.provenance,
                )?
            }
            _ => return Ok(LifecycleDelta::default()),
        };
        Ok(delta)
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
        let count_limited = problem
            .draft
            .boundary
            .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED);
        let outcome = self.index.append_with_summary_and_total(
            problem.draft,
            problem.group_key,
            problem.display_summary,
            problem.observations(),
            problem.observation_total(),
            count_limited,
        )?;
        delta.record(outcome);
        Ok(())
    }
}

fn can_merge_faults(
    candidate: &ProvisionalOccurrence,
    problem: &RecognizedProblem,
    identity: CorrelationIdentity,
    source: CorrelationSource,
    clock: Option<CorrelationClock>,
) -> bool {
    if source == CorrelationSource::Direct
        || candidate.source_mask & source.bit() != 0
        || candidate.problem.draft.kind != problem.draft.kind
        || candidate.problem.draft.pid == 0
        || candidate.problem.draft.pid != problem.draft.pid
        || candidate.identity.process != identity.process
        || matches!(
            (candidate.identity.subject, identity.subject),
            (Some(left), Some(right)) if left != right
        )
        || candidate.problem.draft.process_instance == ProcessInstanceKey(0)
        || problem.draft.process_instance == ProcessInstanceKey(0)
        || candidate.problem.draft.process_instance != problem.draft.process_instance
        || !candidate.time_compatible(clock)
    {
        return false;
    }
    let window = match problem.draft.kind {
        ProblemKind::JavaCrash | ProblemKind::JavaOom | ProblemKind::Anr => {
            MANAGED_CORRELATION_WINDOW_LINES
        }
        ProblemKind::NativeCrash => FAULT_OUTCOME_WINDOW_LINES,
        _ => return false,
    };
    range_distance(
        candidate.problem.draft.start_line,
        candidate.problem.draft.end_line,
        problem.draft.start_line,
        problem.draft.end_line,
    ) <= window
}

fn can_attach_lifecycle(
    candidate: &ProvisionalOccurrence,
    observation: &RecentLifecycleObservation,
) -> bool {
    if candidate.problem.draft.pid == 0
        || candidate.problem.draft.pid != observation.pid
        || candidate.identity.process != observation.process
        || candidate.problem.draft.process_instance == ProcessInstanceKey(0)
        || observation.process_instance == ProcessInstanceKey(0)
        || candidate.problem.draft.process_instance != observation.process_instance
        || !candidate.time_compatible(observation.clock)
    {
        return false;
    }
    match observation.kind {
        LifecycleObservationKind::Start => false,
        LifecycleObservationKind::Death => {
            matches!(
                candidate.problem.draft.kind,
                ProblemKind::JavaCrash
                    | ProblemKind::JavaOom
                    | ProblemKind::NativeCrash
                    | ProblemKind::LmkKill
                    | ProblemKind::KernelOomKill
            ) && observation.line >= candidate.problem.draft.start_line
                && observation
                    .line
                    .saturating_sub(candidate.problem.draft.end_line)
                    <= FAULT_OUTCOME_WINDOW_LINES
        }
        LifecycleObservationKind::KillRequest => {
            matches!(
                candidate.problem.draft.kind,
                ProblemKind::JavaCrash
                    | ProblemKind::JavaOom
                    | ProblemKind::Anr
                    | ProblemKind::NativeCrash
                    | ProblemKind::LmkKill
                    | ProblemKind::KernelOomKill
                    | ProblemKind::ProcessRestart
                    | ProblemKind::SignalExit
            ) && line_to_range_distance(
                observation.line,
                candidate.problem.draft.start_line,
                candidate.problem.draft.end_line,
            ) <= FAULT_OUTCOME_WINDOW_LINES
        }
    }
}

fn apply_lifecycle_observation(
    candidate: &mut ProvisionalOccurrence,
    observation: RecentLifecycleObservation,
) {
    candidate.problem.draft.start_line = candidate.problem.draft.start_line.min(observation.line);
    candidate.problem.draft.end_line = candidate.problem.draft.end_line.max(observation.line);
    candidate
        .problem
        .draft
        .evidence
        .insert(EvidenceFlags::CORRELATED);
    if candidate.problem.draft.process_instance == ProcessInstanceKey(0)
        && observation.process_instance != ProcessInstanceKey(0)
    {
        candidate.problem.draft.process_instance = observation.process_instance;
    }
    match observation.kind {
        LifecycleObservationKind::Start => {}
        LifecycleObservationKind::Death => {
            candidate
                .problem
                .draft
                .outcome
                .insert(OutcomeFlags::DEATH_OBSERVED);
            if candidate
                .problem
                .draft
                .outcome
                .contains(OutcomeFlags::EXPLICITLY_RECOVERABLE)
            {
                candidate
                    .problem
                    .draft
                    .outcome
                    .insert(OutcomeFlags::CONFLICT);
            }
        }
        LifecycleObservationKind::KillRequest => candidate
            .problem
            .draft
            .outcome
            .insert(OutcomeFlags::KILL_REQUESTED),
    }
    candidate.problem.push_observation(observation.reference);
    candidate.problem.sort_observations();
}

fn lifecycle_observation_candidate(
    observation: LifecycleObservation,
) -> Option<ObservationCandidate> {
    let (rule, role, priority) = match observation.kind {
        LifecycleObservationKind::Start => (
            RuleId::ProcessStartV1,
            ObservationRole::Start,
            EvidencePriority::Correlation,
        ),
        LifecycleObservationKind::Death => (
            RuleId::ProcessDiedV1,
            ObservationRole::Death,
            EvidencePriority::Outcome,
        ),
        LifecycleObservationKind::KillRequest => (
            RuleId::AmKillRequestV1,
            ObservationRole::KillRequest,
            EvidencePriority::Outcome,
        ),
    };
    Some(ObservationCandidate::new(
        ObservationRef::new(
            observation.line,
            rule,
            role,
            observation.source.format,
            observation.source.provenance,
        )
        .ok()?,
        priority,
    ))
}

fn provisional_deadline(last_evidence_line: u32) -> u64 {
    u64::from(last_evidence_line) + u64::from(FAULT_OUTCOME_WINDOW_LINES)
}

fn cross_source_safe_after(candidate: &ProvisionalOccurrence) -> u64 {
    let window = match candidate.problem.draft.kind {
        ProblemKind::JavaCrash | ProblemKind::JavaOom | ProblemKind::Anr => {
            MANAGED_CORRELATION_WINDOW_LINES
        }
        ProblemKind::NativeCrash => FAULT_OUTCOME_WINDOW_LINES,
        _ => return u64::MAX,
    };
    u64::from(candidate.problem.draft.end_line) + u64::from(window)
}

fn correlation_time_compatible(
    left: Option<CorrelationClock>,
    right: Option<CorrelationClock>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) if left.segment == right.segment => {
            left.millis.abs_diff(right.millis) <= MAX_CORRELATION_TIME_DELTA_MS
        }
        _ => true,
    }
}

fn range_distance(left_start: u32, left_end: u32, right_start: u32, right_end: u32) -> u32 {
    if left_end < right_start {
        right_start - left_end
    } else if right_end < left_start {
        left_start.saturating_sub(right_end)
    } else {
        0
    }
}

fn line_to_range_distance(line: u32, start: u32, end: u32) -> u32 {
    if line < start {
        start - line
    } else if line > end {
        line.saturating_sub(end)
    } else {
        0
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
            occurrence.death_pid,
            occurrence.terminated_instance.key(),
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
    problem.set_correlation_identity(&process, None);
    problem.set_display_summary(&process, std::str::from_utf8(relation_token).ok());
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
    problem.set_correlation_identity(&process, Some(occurrence.mechanism.token().as_bytes()));
    problem.set_display_summary(&process, Some(occurrence.mechanism.token()));
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
    let is_multiline = occurrence.start_line != occurrence.line;
    let mut evidence = EvidenceFlags::PRIMARY | EvidenceFlags::STRUCTURED;
    if is_multiline {
        evidence.insert(EvidenceFlags::MULTILINE);
    }
    let draft = ProblemEventDraft {
        start_line: occurrence.start_line,
        end_line: occurrence.line,
        anchor_line: occurrence.line,
        anchor_timestamp,
        pid: occurrence.victim_pid,
        process_instance,
        kind: ProblemKind::KernelOomKill,
        evidence,
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
    if is_multiline {
        problem.push_observation(compact_observation(
            occurrence.start_line,
            RuleId::KernelOomKillV1,
            ObservationRole::Supporting,
            EvidenceFormat::KernelShapedText,
            provenance,
            EvidencePriority::Supporting,
        ));
    }
    problem.set_correlation_identity(&process, Some(occurrence.mechanism.token().as_bytes()));
    problem.set_display_summary(&process, Some(occurrence.mechanism.token()));
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

    fn correlated_problem_at(line: u32, rule: RuleId) -> RecognizedProblem {
        let mut problem = recognized();
        problem.draft.start_line = line;
        problem.draft.end_line = line;
        problem.draft.anchor_line = line;
        problem.observations[0] = ObservationCandidate::new(
            ObservationRef::new(
                line,
                rule,
                ObservationRole::Primary,
                if rule == RuleId::ManagedAmCrashV1 {
                    EvidenceFormat::EventLogShapedText
                } else {
                    EvidenceFormat::AospText
                },
                LineProvenance::Unknown,
            )
            .unwrap(),
            EvidencePriority::MinimumGrammar,
        );
        let process = ProcessFingerprintKey::new(Some("com.example.app"));
        problem.set_correlation_identity(&process, Some(b"java.lang.RuntimeException"));
        problem
    }

    #[test]
    fn recognized_problem_has_bounded_compact_observations() {
        let mut problem = recognized();
        for line in 4..=10 {
            assert!(problem.push_observation(observation(line)));
        }
        assert_eq!(problem.observations().len(), 8);
        for line in 11..=4_098 {
            assert!(!problem.push_observation(observation(line)));
        }
        assert_eq!(problem.observation_total(), 4_096);
        assert!(!problem.push_observation(observation(4_099)));
        assert!(problem
            .draft
            .boundary
            .contains(BoundaryFlags::OBSERVATION_REFS_TRUNCATED));
        assert!(problem
            .draft
            .boundary
            .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED));

        problem.draft.end_line = 4_098;
        let mut engine = ProblemEngine::new();
        let mut delta = ProblemDelta::default();
        engine.commit_recognized(problem, &mut delta).unwrap();
        let event = engine.event(delta.last_stored_event().unwrap()).unwrap();
        assert_eq!(event.observation_len(), 8);
        assert_eq!(event.observation_total(), 4_096);
        assert!(event
            .boundary()
            .contains(BoundaryFlags::OBSERVATION_REFS_TRUNCATED));
        assert!(event
            .boundary()
            .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED));
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
    fn ordinary_lines_advance_watermarks_without_invoking_problem_recognizers() {
        let mut engine = ProblemEngine::new();
        let mut delta = ProblemDelta::default();
        engine.admit_provisional(
            correlated_problem_at(0, RuleId::JavaUncaughtV1),
            None,
            &mut delta,
        );
        feed_with_provenance(
            &mut engine,
            FAULT_OUTCOME_WINDOW_LINES + 1,
            "07-26 12:00:01.000  7  7 I Other: ordinary application output",
            LineProvenance::Unknown,
        );

        assert_eq!(engine.recognizer_observe_counts, [0; 6]);
        assert_eq!(engine.stats().stored_occurrence_count, 1);
        assert!(engine.recent_clocks.is_empty());
    }

    #[test]
    fn ordinary_line_storm_does_not_retain_or_charge_correlation_heap() {
        let mut engine = ProblemEngine::new();
        let raw = "07-26 12:00:01.000  7  7 I Other: ordinary application output";
        for line in 0..10_000 {
            feed_with_provenance(&mut engine, line, raw, LineProvenance::Unknown);
        }

        let memory = engine.memory_stats();
        assert_eq!(memory.charged_bytes, 0);
        assert_eq!(memory.retained_heap_bytes, 0);
        assert_eq!(memory.high_water_charged_bytes, 0);
        assert_eq!(memory.denied_reservation_count, 0);
        assert!(engine.recent_clocks.is_empty());
        assert!(engine.provisional.is_empty());
        assert!(engine.recent.is_empty());
    }

    #[test]
    fn ordinary_fast_path_preserves_timestamp_boundaries_and_pending_bypass() {
        let mut engine = ProblemEngine::new();
        assert!(!engine.requires_full_line());

        let first = super::super::timestamp::parse_log_timestamp_probe(b"07-26", b"12:00:01.000");
        engine.observe_non_candidate(0, first);
        assert!(engine.timestamp_origin.is_none());
        engine.observe_non_candidate(1, None);
        assert!(engine.timestamp_origin.is_none());

        feed_with_provenance(
            &mut engine,
            2,
            "07-26 12:00:02.000  42  42 E AndroidRuntime: FATAL EXCEPTION: main",
            LineProvenance::Known(crate::problems::LogBuffer::Main),
        );
        assert!(engine.requires_full_line());
        assert!(engine.timestamp_origin.is_some());
    }

    #[test]
    fn maximum_provisional_finish_has_a_conservative_work_limit() {
        let mut engine = ProblemEngine::new();
        for line in 0..super::super::correlation::MAX_PROVISIONAL_OCCURRENCES as u32 {
            let problem = correlated_problem_at(line, RuleId::JavaUncaughtV1);
            let identity = problem
                .correlation_identity
                .expect("the test problem has a correlation identity");
            let source = problem.correlation_source();
            let payload = ProvisionalOccurrence::new(problem, identity, source, None);
            let outcome = engine.provisional.insert(u64::MAX - 1, payload).unwrap();
            assert!(outcome.finalized.is_empty());
        }

        let delta = engine.finish_input();

        assert_eq!(
            engine.stats().stored_occurrence_count,
            super::super::correlation::MAX_PROVISIONAL_OCCURRENCES as u64
        );
        assert_eq!(usize::from(delta.committed()), usize::from(u8::MAX));
        assert!(engine.stats().correlation_limited);
        assert!(engine.correlation_work_exhausted.get());
        assert!(
            engine.correlation_work_used.get() <= MAX_CORRELATION_WORK_UNITS_PER_FINISH,
            "the fixed finish budget must cap adversarial pair comparisons"
        );
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
            0,
            "a lifecycle occurrence remains provisional for late am_kill evidence"
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
        assert_eq!(signal.committed(), 0);
        let finished = engine.finish_input();
        assert_eq!(finished.committed(), 2);

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
    fn kernel_oom_event_range_and_supporting_fact_include_the_earliest_opener() {
        let kernel = LineProvenance::Known(crate::problems::LogBuffer::Kernel);
        let mut engine = ProblemEngine::new();
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  0  0 E kernel: oom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null)",
            ),
            (
                1,
                "07-26 12:00:00.001  0  0 I kernel: synthetic kernel task dump",
            ),
            (
                2,
                "07-26 12:00:00.002  0  0 E kernel: Killed process 333 (com.example.kernel) total-vm:42kB",
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, kernel);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(
            (event.start_line(), event.end_line(), event.anchor_line()),
            (0, 2, 2)
        );
        assert!(event.evidence().contains(EvidenceFlags::MULTILINE));
        let observations = engine.event_observations(ProblemEventId(0)).unwrap();
        assert!(observations.iter().any(|fact| {
            fact.line() == 0
                && fact.rule() == RuleId::KernelOomKillV1
                && fact.role() == ObservationRole::Supporting
        }));
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
        engine.finish_input();
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

    #[test]
    fn late_structured_java_fact_merges_into_the_same_provisional_occurrence() {
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.IllegalStateException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:00.004  100  100 I am_crash: [111,com.example.app,0,java.lang.IllegalStateException,boom,Example.kt,42]",
                LineProvenance::Known(crate::problems::LogBuffer::Events),
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }

        assert_eq!(engine.stats().stored_occurrence_count, 0);
        assert_eq!(engine.stats().provisional_occurrence_count, 2);
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        assert_eq!(engine.stats().provisional_occurrence_count, 0);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!((event.start_line(), event.end_line()), (0, 4));
        let observations = engine.event_observations(ProblemEventId(0)).unwrap();
        assert!(observations
            .iter()
            .any(|fact| fact.rule() == RuleId::JavaUncaughtV1));
        assert!(observations
            .iter()
            .any(|fact| fact.rule() == RuleId::ManagedAmCrashV1));
        let group = engine
            .group(GroupId::from_raw(event.group_id_raw()))
            .unwrap();
        assert_eq!(group.process_summary.as_str(), "com.example.app");
        assert_eq!(
            group.signature_summary.as_str(),
            "java.lang.IllegalStateException"
        );
        assert!(!group.signature_summary.as_str().contains("boom"));
    }

    #[test]
    fn native_tombstone_and_native_am_crash_are_one_occurrence() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  99  99 F DEBUG: *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                1,
                "07-26 12:00:00.001  99  99 F DEBUG: pid: 321, tid: 322, name: worker  >>> com.example.native <<<",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                2,
                "07-26 12:00:00.002  99  99 F DEBUG: signal 11 (SIGSEGV), code 2 (SEGV_ACCERR), fault addr 0x1",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                3,
                "07-26 12:00:00.003  99  99 F DEBUG: backtrace:",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                4,
                "07-26 12:00:00.004  99  99 F DEBUG: #00 pc 0000000000001234 /system/lib64/libfoo.so (abort_now+24)",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                5,
                "07-26 12:00:00.005  100  100 I am_crash: [321,com.example.native,0,Native crash,signal 11,libfoo.so,0]",
                events,
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::NativeCrash);
        assert_eq!((event.start_line(), event.end_line()), (0, 5));
        let observations = engine.event_observations(ProblemEventId(0)).unwrap();
        assert!(observations
            .iter()
            .any(|fact| fact.rule() == RuleId::NativeTombstoneV1));
        assert!(observations
            .iter()
            .any(|fact| fact.rule() == RuleId::ManagedAmCrashV1));
    }

    #[test]
    fn libc_fatal_signal_uses_the_active_process_and_merges_with_its_tombstone() {
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I ActivityManager: Start proc 321:com.example.native/u0a123 for service",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                1,
                "07-26 12:00:00.001  321  322 F libc: Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0 in tid 322 (worker)",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                2,
                "07-26 12:00:00.002   99   99 F DEBUG: *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                3,
                "07-26 12:00:00.003   99   99 F DEBUG: pid: 321, tid: 322, name: worker  >>> com.example.native <<<",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
            (
                4,
                "07-26 12:00:00.004   99   99 F DEBUG: signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0",
                LineProvenance::Known(crate::problems::LogBuffer::Crash),
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::NativeCrash);
        assert_eq!((event.start_line(), event.end_line()), (1, 4));
        assert_ne!(event.process_instance(), ProcessInstanceKey(0));
        let group = engine
            .group(GroupId::from_raw(event.group_id_raw()))
            .unwrap();
        assert_eq!(group.key.signature_quality(), SignatureQuality::SignalOnly);
        assert_eq!(group.key.identity_quality(), IdentityQuality::KnownProcess);
        assert_eq!(group.process_summary.as_str(), "com.example.native");
        let observations = engine.event_observations(ProblemEventId(0)).unwrap();
        assert!(observations
            .iter()
            .any(|fact| fact.rule() == RuleId::NativeLibcSignalV1));
        assert!(observations
            .iter()
            .any(|fact| fact.rule() == RuleId::NativeTombstoneV1));
    }

    #[test]
    fn unmapped_libc_signals_are_unknown_separate_occurrences_and_spoofs_are_rejected() {
        let mut engine = ProblemEngine::new();
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  777  778 F libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 778 (worker)",
            ),
            (
                1,
                "07-26 12:00:00.001  777  778 F libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE) in tid 778 (worker)",
            ),
            (
                2,
                "07-26 12:00:00.002  777  778 E libc: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE)",
            ),
            (
                3,
                "07-26 12:00:00.003  777  778 F libc: Fatal signal 11 (SIGABRT), code 1 (SEGV_MAPERR)",
            ),
            (
                4,
                "07-26 12:00:00.004  777  778 F App: Fatal signal 6 (SIGABRT), code -1 (SI_QUEUE)",
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, LineProvenance::Unknown);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 2);
        assert_eq!(engine.stats().stored_group_count, 1);
        for event in stored_events(&engine) {
            assert_eq!(event.kind(), ProblemKind::NativeCrash);
            assert_eq!(event.process_instance(), ProcessInstanceKey(0));
        }
        let event = engine.event(ProblemEventId(0)).unwrap();
        let group = engine
            .group(GroupId::from_raw(event.group_id_raw()))
            .unwrap();
        assert_eq!(group.key.signature_quality(), SignatureQuality::SignalOnly);
        assert_eq!(
            group.key.identity_quality(),
            IdentityQuality::UnknownProcess
        );
        assert_eq!(group.observed_occurrence_count, 2);
        assert_eq!(group.process_summary.as_str(), "");
        assert!(engine
            .event_observations(ProblemEventId(0))
            .unwrap()
            .iter()
            .all(|fact| fact.rule() == RuleId::NativeLibcSignalV1));
    }

    #[test]
    fn activity_manager_anr_block_and_am_anr_are_one_occurrence() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  100  101 E ActivityManager: ANR in com.example.app",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                1,
                "07-26 12:00:00.001  100  101 E ActivityManager: PID: 321",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                2,
                "07-26 12:00:00.002  100  101 E ActivityManager: ANR in unfinished.app",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                3,
                "07-26 12:00:00.003  100  100 I am_anr: [321,com.example.app,0,Input dispatching timed out]",
                events,
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::Anr);
        assert_eq!((event.start_line(), event.end_line()), (0, 3));
        assert!(event.evidence().contains(EvidenceFlags::CORRELATED));
    }

    #[test]
    fn incompatible_signature_or_comparable_time_over_sixty_seconds_does_not_merge() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);

        let mut different_exception = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.IllegalStateException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:00.004  100  100 I am_crash: [111,com.example.app,0,java.lang.IllegalArgumentException,boom,Example.kt,42]",
                events,
            ),
        ] {
            feed_with_provenance(&mut different_exception, line, raw, provenance);
        }
        different_exception.finish_input();
        assert_eq!(different_exception.stats().stored_occurrence_count, 2);

        let mut too_late_in_time = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.IllegalStateException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:01:01.004  100  100 I am_crash: [111,com.example.app,0,java.lang.IllegalStateException,boom,Example.kt,42]",
                events,
            ),
        ] {
            feed_with_provenance(&mut too_late_in_time, line, raw, provenance);
        }
        too_late_in_time.finish_input();
        assert_eq!(too_late_in_time.stats().stored_occurrence_count, 2);
    }

    #[test]
    fn engine_watermark_must_strictly_cross_the_provisional_deadline() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        feed_with_provenance(
            &mut engine,
            0,
            "07-26 12:00:00.000  100  100 I am_crash: [111,com.example.app,0,java.lang.RuntimeException,boom,Example.kt,42]",
            events,
        );
        feed_with_provenance(
            &mut engine,
            FAULT_OUTCOME_WINDOW_LINES,
            "07-26 12:00:01.000  7  7 I Other: exact deadline",
            LineProvenance::Unknown,
        );
        assert_eq!(engine.stats().stored_occurrence_count, 0);
        assert_eq!(engine.stats().provisional_occurrence_count, 1);

        feed_with_provenance(
            &mut engine,
            FAULT_OUTCOME_WINDOW_LINES + 1,
            "07-26 12:00:01.001  7  7 I Other: crossed deadline",
            LineProvenance::Unknown,
        );
        assert_eq!(engine.stats().stored_occurrence_count, 1);
        assert_eq!(engine.stats().provisional_occurrence_count, 0);
    }

    #[test]
    fn correlation_does_not_merge_cross_process_or_outside_the_frozen_line_window() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);

        let mut cross_process = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.IllegalStateException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:00.004  100  100 I am_crash: [222,com.example.app,0,java.lang.IllegalStateException,boom,Example.kt,42]",
                events,
            ),
        ] {
            feed_with_provenance(&mut cross_process, line, raw, provenance);
        }
        cross_process.finish_input();
        assert_eq!(cross_process.stats().stored_occurrence_count, 2);

        let mut outside_window = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.IllegalStateException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                516,
                "07-26 12:00:01.000  100  100 I am_crash: [111,com.example.app,0,java.lang.IllegalStateException,boom,Example.kt,42]",
                events,
            ),
        ] {
            feed_with_provenance(&mut outside_window, line, raw, provenance);
        }
        outside_window.finish_input();
        assert_eq!(outside_window.stats().stored_occurrence_count, 2);
    }

    #[test]
    fn an_equidistant_cross_source_fact_is_ambiguous_and_does_not_merge() {
        let mut engine = ProblemEngine::new();
        let mut delta = ProblemDelta::default();
        engine.admit_provisional(
            correlated_problem_at(0, RuleId::JavaUncaughtV1),
            None,
            &mut delta,
        );
        engine.admit_provisional(
            correlated_problem_at(5, RuleId::ManagedAmCrashV1),
            None,
            &mut delta,
        );
        engine.admit_provisional(
            correlated_problem_at(10, RuleId::JavaUncaughtV1),
            None,
            &mut delta,
        );

        assert_eq!(engine.stats().provisional_occurrence_count, 3);
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 3);
        assert_eq!(engine.correlation_ambiguity_count(), 1);
        engine.reset();
        assert_eq!(engine.correlation_ambiguity_count(), 0);
    }

    #[test]
    fn lifecycle_observations_add_compact_outcomes_before_finish() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I am_proc_start: [0,111,10123,com.example.app,activity,com.example/.Main]",
                events,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: java.lang.RuntimeException: boom",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:00.004  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                5,
                "07-26 12:00:00.005  100  100 I am_kill: [111,com.example.app,900,cached empty]",
                events,
            ),
            (
                6,
                "07-26 12:00:00.006  100  100 I am_proc_died: [0,111,com.example.app]",
                events,
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_ne!(event.process_instance(), ProcessInstanceKey(0));
        assert!(event.outcome().contains(OutcomeFlags::KILL_REQUESTED));
        assert!(event.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
        assert_eq!(event.end_line(), 6);
        let observations = engine.event_observations(ProblemEventId(0)).unwrap();
        assert!(observations
            .iter()
            .any(|fact| fact.role() == ObservationRole::KillRequest));
        assert!(observations
            .iter()
            .any(|fact| fact.role() == ObservationRole::Death));
    }

    #[test]
    fn recoverable_native_crash_preserves_recovery_death_and_conflict_facts() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I am_proc_start: [321,10321,com.example.native,activity,com.example/.Main]",
            ),
            (
                1,
                "07-26 12:00:00.001  100  100 I am_crash: [321,com.example.native,0,Native crash,signal 11,libexample.so,60,1]",
            ),
            (
                2,
                "07-26 12:00:00.002  100  100 I am_proc_died: [321,com.example.native]",
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, events);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::NativeCrash);
        assert!(event
            .outcome()
            .contains(OutcomeFlags::EXPLICITLY_RECOVERABLE));
        assert!(event.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
        assert!(event.outcome().contains(OutcomeFlags::CONFLICT));
        assert!(engine
            .event_observations(ProblemEventId(0))
            .unwrap()
            .iter()
            .any(|fact| fact.role() == ObservationRole::Death));
    }

    #[test]
    fn lmk_and_kernel_oom_kills_keep_kill_issued_separate_from_observed_death() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        for (expected_kind, kill_line, kill_provenance, pid, process) in [
            (
                ProblemKind::LmkKill,
                "07-26 12:00:00.001  900  900 I lmkd: Kill 'com.example.lmk' (222), uid 10222, oom_score_adj 900 to free 42kB rss, 0kB swap; reason: low watermark",
                LineProvenance::Unknown,
                222,
                "com.example.lmk",
            ),
            (
                ProblemKind::KernelOomKill,
                "07-26 12:00:00.001    0    0 E kernel: Out of memory: Killed process 333 (com.example.kernel) total-vm:42kB",
                LineProvenance::Known(crate::problems::LogBuffer::Kernel),
                333,
                "com.example.kernel",
            ),
        ] {
            let mut engine = ProblemEngine::new();
            let start = format!(
                "07-26 12:00:00.000  100  100 I am_proc_start: [{pid},{},\
                 {process},activity,com.example/.Main]",
                10_000 + pid
            );
            let death = format!(
                "07-26 12:00:00.002  100  100 I am_proc_died: [{pid},{process}]"
            );
            feed_with_provenance(&mut engine, 0, &start, events);
            feed_with_provenance(&mut engine, 1, kill_line, kill_provenance);
            feed_with_provenance(&mut engine, 2, &death, events);
            engine.finish_input();

            assert_eq!(
                engine.stats().stored_occurrence_count,
                1,
                "{expected_kind:?}"
            );
            let event = engine.event(ProblemEventId(0)).unwrap();
            assert_eq!(event.kind(), expected_kind);
            assert!(event.outcome().contains(OutcomeFlags::KILL_ISSUED));
            assert!(event.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
            assert!(!event.outcome().contains(OutcomeFlags::CONFLICT));
            assert!(engine
                .event_observations(ProblemEventId(0))
                .unwrap()
                .iter()
                .any(|fact| fact.role() == ObservationRole::Death));
        }
    }

    #[test]
    fn an_intervening_explicit_start_prevents_pid_reuse_deduplication() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.RuntimeException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: second",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:00.004  100  100 I ActivityManager: Start proc 111:com.example.app/u0a123 for activity",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                5,
                "07-26 12:00:00.005  100  100 I am_crash: [111,com.example.app,0,java.lang.RuntimeException,boom,Example.kt,42]",
                events,
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 2);
    }

    #[test]
    fn fault_before_a_new_start_cannot_claim_that_instances_death() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  111  111 E AndroidRuntime: FATAL EXCEPTION: main",
                LineProvenance::Unknown,
            ),
            (
                1,
                "07-26 12:00:00.001  111  111 E AndroidRuntime: Process: com.example.app, PID: 111",
                LineProvenance::Unknown,
            ),
            (
                2,
                "07-26 12:00:00.002  111  111 E AndroidRuntime: java.lang.RuntimeException: boom",
                LineProvenance::Unknown,
            ),
            (
                3,
                "07-26 12:00:00.003  111  111 E AndroidRuntime: FATAL EXCEPTION: next",
                LineProvenance::Unknown,
            ),
            (
                4,
                "07-26 12:00:00.004  100  100 I am_proc_start: [0,111,10123,com.example.app,activity,com.example/.Main]",
                events,
            ),
            (
                5,
                "07-26 12:00:00.005  100  100 I am_proc_died: [0,111,com.example.app]",
                events,
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, provenance);
        }
        engine.finish_input();

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let event = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::JavaCrash);
        assert_eq!((event.start_line(), event.end_line()), (0, 2));
        assert!(!event.outcome().contains(OutcomeFlags::DEATH_OBSERVED));
    }

    #[test]
    fn am_kill_attaches_to_lifecycle_occurrences_before_or_after_minimum_grammar() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        for kill_before_signal in [true, false] {
            let mut engine = ProblemEngine::new();
            feed_with_provenance(
                &mut engine,
                0,
                "07-26 12:00:00.000  100  100 I ActivityManager: Start proc 88:com.signal.app/u0a124 for service",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            );
            if kill_before_signal {
                feed_with_provenance(
                    &mut engine,
                    1,
                    "07-26 12:00:00.001  100  100 I am_kill: [88,com.signal.app,900,cached empty]",
                    events,
                );
            }
            feed_with_provenance(
                &mut engine,
                if kill_before_signal { 2 } else { 1 },
                if kill_before_signal {
                    "07-26 12:00:00.002  100  100 I Zygote: Process 88 exited due to signal 9 (Killed)"
                } else {
                    "07-26 12:00:00.001  100  100 I Zygote: Process 88 exited due to signal 9 (Killed)"
                },
                LineProvenance::Known(crate::problems::LogBuffer::System),
            );
            if !kill_before_signal {
                feed_with_provenance(
                    &mut engine,
                    2,
                    "07-26 12:00:00.002  100  100 I am_kill: [88,com.signal.app,900,cached empty]",
                    events,
                );
            }
            assert_eq!(engine.stats().stored_occurrence_count, 0);
            assert_eq!(engine.stats().provisional_occurrence_count, 1);
            engine.finish_input();

            let event = engine.event(ProblemEventId(0)).unwrap();
            assert_eq!(event.kind(), ProblemKind::SignalExit);
            assert!(event.outcome().contains(OutcomeFlags::KILL_REQUESTED));
            assert!(engine
                .event_observations(ProblemEventId(0))
                .unwrap()
                .iter()
                .any(|observation| observation.role() == ObservationRole::KillRequest));
        }

        let mut restart = ProblemEngine::new();
        for (line, raw, provenance) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I ActivityManager: Start proc 42:com.restart.app/u0a123 for service",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                1,
                "07-26 12:00:00.001  100  100 I am_kill: [42,com.restart.app,900,cached empty]",
                events,
            ),
            (
                2,
                "07-26 12:00:00.002  100  100 I ActivityManager: Process com.restart.app (pid 42) has died",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
            (
                3,
                "07-26 12:00:00.003  100  100 I ActivityManager: Start proc 77:com.restart.app/u0a123 for service",
                LineProvenance::Known(crate::problems::LogBuffer::System),
            ),
        ] {
            feed_with_provenance(&mut restart, line, raw, provenance);
        }
        restart.finish_input();
        let event = restart.event(ProblemEventId(0)).unwrap();
        assert_eq!(event.kind(), ProblemKind::ProcessRestart);
        assert!(event.outcome().contains(OutcomeFlags::KILL_REQUESTED));
    }

    #[test]
    fn open_am_kill_window_delays_finalize_until_the_nearest_candidate_is_known() {
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        let mut engine = ProblemEngine::new();
        let mut delta = ProblemDelta::default();
        engine.admit_provisional(
            correlated_problem_at(0, RuleId::JavaUncaughtV1),
            None,
            &mut delta,
        );
        feed_with_provenance(
            &mut engine,
            4_000,
            "07-26 12:00:00.000  100  100 I am_kill: [42,com.example.app,900,cached empty]",
            events,
        );
        feed_with_provenance(
            &mut engine,
            5_000,
            "07-26 12:00:00.001  7  7 I Other: advance",
            LineProvenance::Unknown,
        );
        assert_eq!(engine.stats().stored_occurrence_count, 0);
        engine.admit_provisional(
            correlated_problem_at(5_000, RuleId::JavaUncaughtV1),
            None,
            &mut delta,
        );
        feed_with_provenance(
            &mut engine,
            8_097,
            "07-26 12:00:00.002  7  7 I Other: close kill window",
            LineProvenance::Unknown,
        );

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        let first = engine.event(ProblemEventId(0)).unwrap();
        assert_eq!(first.anchor_line(), 0);
        assert!(!first.outcome().contains(OutcomeFlags::KILL_REQUESTED));
        engine.finish_input();
        let second = engine.event(ProblemEventId(1)).unwrap();
        assert_eq!(second.anchor_line(), 5_000);
        assert!(second.outcome().contains(OutcomeFlags::KILL_REQUESTED));
    }

    #[test]
    fn correlation_capacity_forces_the_earliest_occurrence_with_a_limit_marker() {
        let provisional = super::super::correlation::ProvisionalLimits::new(1, 1024).unwrap();
        let recent = super::super::correlation::RecentObservationLimits::new(4, 1024).unwrap();
        let mut engine = ProblemEngine::with_limits_and_correlation(
            ProblemIndexLimits::default(),
            provisional,
            recent,
        )
        .unwrap();
        let events = LineProvenance::Known(crate::problems::LogBuffer::Events);
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I am_crash: [111,one.app,0,java.lang.RuntimeException,boom,One.kt,42]",
            ),
            (
                1,
                "07-26 12:00:00.001  100  100 I am_crash: [222,two.app,0,java.lang.RuntimeException,boom,Two.kt,42]",
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, events);
        }

        assert_eq!(engine.stats().stored_occurrence_count, 1);
        assert_eq!(engine.stats().provisional_occurrence_count, 1);
        assert!(engine.stats().correlation_limited);
        assert!(engine
            .event(ProblemEventId(0))
            .unwrap()
            .boundary()
            .contains(BoundaryFlags::CORRELATION_LIMITED));
        engine.finish_input();
        assert_eq!(engine.stats().stored_occurrence_count, 2);
    }

    #[test]
    fn recent_observation_pressure_reports_real_drop_statistics_until_reset() {
        let provisional = super::super::correlation::ProvisionalLimits::new(4, 4096).unwrap();
        let recent = super::super::correlation::RecentObservationLimits::new(1, 128).unwrap();
        let mut engine = ProblemEngine::with_limits_and_correlation(
            ProblemIndexLimits::default(),
            provisional,
            recent,
        )
        .unwrap();
        for (line, raw) in [
            (
                0,
                "07-26 12:00:00.000  100  100 I ActivityManager: Start proc 111:one.app/u0a123 for service",
            ),
            (
                1,
                "07-26 12:00:00.001  100  100 I ActivityManager: Start proc 222:two.app/u0a124 for service",
            ),
        ] {
            feed_with_provenance(&mut engine, line, raw, LineProvenance::Unknown);
        }

        assert_eq!(engine.stats().dropped_recent_observation_count, 1);
        assert!(engine.stats().correlation_limited);
        engine.reset();
        assert_eq!(engine.stats().dropped_recent_observation_count, 0);
        assert!(!engine.stats().correlation_limited);
    }
}
