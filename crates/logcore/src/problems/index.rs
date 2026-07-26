use super::budget::{
    aggregate_vec_usage, hash_map_usage, vec_usage, ProblemMemoryAccount, ProblemMemoryBudget,
    ProblemMemoryBudgetError, ProblemMemoryUsage,
};
use super::facts::{ObservationCandidate, ObservationRef};
use super::fingerprint::ProblemFingerprint;
use super::model::{
    BoundaryFlags, IdentityQuality, PackedLogTimestamp, ProblemEvent, ProblemEventDraft,
    ProblemEventError, ProblemEventId, ProblemKind, SignatureQuality, MAX_ADOPTED_OBSERVATIONS,
    MAX_MATERIALIZED_OBSERVATIONS,
};
use std::collections::HashMap;
use std::mem::size_of;
use std::time::{Duration, Instant};

const DEFAULT_MAX_EVENTS: usize = 1_000_000;
const DEFAULT_MAX_OBSERVATION_REFS: usize = 4_000_000;
const DEFAULT_MAX_GROUPS: usize = 100_000;
const DEFAULT_MAX_SNAPSHOTS: usize = 8;
const DEFAULT_SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MAX_SNAPSHOT_ID_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PROBLEM_PROCESS_SUMMARY_BYTES: usize = 48;
pub const MAX_PROBLEM_SIGNATURE_SUMMARY_BYTES: usize = 80;

/// Fixed-capacity display metadata derived only from normalized detector
/// tokens. It is deliberately separate from the grouping key: truncation can
/// affect presentation but can never merge or split groups.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BoundedProblemSummary<const N: usize> {
    bytes: [u8; N],
    len: u8,
    truncated: bool,
}

impl<const N: usize> BoundedProblemSummary<N> {
    pub fn from_normalized(value: Option<&str>) -> Self {
        let value = value.unwrap_or_default();
        let mut end = value.len().min(N).min(usize::from(u8::MAX));
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0; N];
        bytes[..end].copy_from_slice(&value.as_bytes()[..end]);
        Self {
            bytes,
            len: end as u8,
            truncated: end < value.len(),
        }
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("summary constructors retain complete UTF-8 code points")
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    pub const fn truncated(self) -> bool {
        self.truncated
    }
}

impl<const N: usize> Default for BoundedProblemSummary<N> {
    fn default() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            truncated: false,
        }
    }
}

impl<const N: usize> std::fmt::Debug for BoundedProblemSummary<N> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundedProblemSummary")
            .field("value", &self.as_str())
            .field("truncated", &self.truncated)
            .finish()
    }
}

pub type ProblemProcessSummary = BoundedProblemSummary<MAX_PROBLEM_PROCESS_SUMMARY_BYTES>;
pub type ProblemSignatureSummary = BoundedProblemSummary<MAX_PROBLEM_SIGNATURE_SUMMARY_BYTES>;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProblemDisplaySummary {
    pub process: ProblemProcessSummary,
    pub signature: ProblemSignatureSummary,
}

impl ProblemDisplaySummary {
    pub fn from_normalized(process: Option<&str>, signature: Option<&str>) -> Self {
        Self {
            process: ProblemProcessSummary::from_normalized(process),
            signature: ProblemSignatureSummary::from_normalized(signature),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemIndexLimits {
    pub max_events: usize,
    pub max_observation_refs: usize,
    pub max_groups: usize,
    pub max_snapshots: usize,
    pub snapshot_ttl: Duration,
    pub max_snapshot_id_bytes: usize,
}

impl Default for ProblemIndexLimits {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_MAX_EVENTS,
            max_observation_refs: DEFAULT_MAX_OBSERVATION_REFS,
            max_groups: DEFAULT_MAX_GROUPS,
            max_snapshots: DEFAULT_MAX_SNAPSHOTS,
            snapshot_ttl: DEFAULT_SNAPSHOT_TTL,
            max_snapshot_id_bytes: DEFAULT_MAX_SNAPSHOT_ID_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemIndexLimitsError {
    ExceedsStructuralCap(&'static str),
    SnapshotCountMustBePositive,
    SnapshotTtlMustBePositive,
}

impl ProblemIndexLimits {
    fn validate(self) -> Result<Self, ProblemIndexLimitsError> {
        let defaults = Self::default();
        for (name, actual, maximum) in [
            ("max_events", self.max_events, defaults.max_events),
            (
                "max_observation_refs",
                self.max_observation_refs,
                defaults.max_observation_refs,
            ),
            ("max_groups", self.max_groups, defaults.max_groups),
            ("max_snapshots", self.max_snapshots, defaults.max_snapshots),
            (
                "max_snapshot_id_bytes",
                self.max_snapshot_id_bytes,
                defaults.max_snapshot_id_bytes,
            ),
        ] {
            if actual > maximum {
                return Err(ProblemIndexLimitsError::ExceedsStructuralCap(name));
            }
        }
        if self.snapshot_ttl > defaults.snapshot_ttl {
            return Err(ProblemIndexLimitsError::ExceedsStructuralCap(
                "snapshot_ttl",
            ));
        }
        if self.max_snapshots == 0 {
            return Err(ProblemIndexLimitsError::SnapshotCountMustBePositive);
        }
        if self.snapshot_ttl.is_zero() {
            return Err(ProblemIndexLimitsError::SnapshotTtlMustBePositive);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct GroupId(u32);

impl GroupId {
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupKey {
    kind: ProblemKind,
    fingerprint_version: u16,
    signature_quality: SignatureQuality,
    identity_quality: IdentityQuality,
    fingerprint: ProblemFingerprint,
}

impl GroupKey {
    pub const fn new(
        kind: ProblemKind,
        fingerprint_version: u16,
        signature_quality: SignatureQuality,
        identity_quality: IdentityQuality,
        fingerprint: ProblemFingerprint,
    ) -> Self {
        Self {
            kind,
            fingerprint_version,
            signature_quality,
            identity_quality,
            fingerprint,
        }
    }

    pub const fn kind(self) -> ProblemKind {
        self.kind
    }

    pub const fn fingerprint_version(self) -> u16 {
        self.fingerprint_version
    }

    pub const fn signature_quality(self) -> SignatureQuality {
        self.signature_quality
    }

    pub const fn identity_quality(self) -> IdentityQuality {
        self.identity_quality
    }

    pub const fn fingerprint(self) -> ProblemFingerprint {
        self.fingerprint
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemGroupSummary {
    pub id: GroupId,
    pub key: GroupKey,
    pub process_summary: ProblemProcessSummary,
    pub signature_summary: ProblemSignatureSummary,
    pub observed_occurrence_count: u64,
    pub stored_occurrence_count: u64,
    pub dropped_occurrence_count: u64,
    pub first_observed_line: u32,
    pub first_observed_timestamp: PackedLogTimestamp,
    pub last_observed_line: u32,
    pub last_observed_timestamp: PackedLogTimestamp,
    pub first_stored_event_id: Option<ProblemEventId>,
    pub last_stored_event_id: Option<ProblemEventId>,
    pub representative_stored_event_id: Option<ProblemEventId>,
}

#[derive(Debug)]
struct ProblemGroup {
    summary: ProblemGroupSummary,
    event_ids: Vec<ProblemEventId>,
    first_stored_line: u32,
    last_stored_line: u32,
}

impl ProblemGroup {
    fn new(
        id: GroupId,
        key: GroupKey,
        display_summary: ProblemDisplaySummary,
        draft: ProblemEventDraft,
        event_id: ProblemEventId,
        event_ids: Vec<ProblemEventId>,
    ) -> Self {
        Self {
            summary: ProblemGroupSummary {
                id,
                key,
                process_summary: display_summary.process,
                signature_summary: display_summary.signature,
                observed_occurrence_count: 1,
                stored_occurrence_count: 1,
                dropped_occurrence_count: 0,
                first_observed_line: draft.anchor_line,
                first_observed_timestamp: draft.anchor_timestamp,
                last_observed_line: draft.anchor_line,
                last_observed_timestamp: draft.anchor_timestamp,
                first_stored_event_id: Some(event_id),
                last_stored_event_id: Some(event_id),
                representative_stored_event_id: Some(event_id),
            },
            event_ids,
            first_stored_line: draft.anchor_line,
            last_stored_line: draft.anchor_line,
        }
    }

    fn record_observed(&mut self, draft: ProblemEventDraft) {
        saturating_increment(&mut self.summary.observed_occurrence_count);
        if draft.anchor_line < self.summary.first_observed_line {
            self.summary.first_observed_line = draft.anchor_line;
            self.summary.first_observed_timestamp = draft.anchor_timestamp;
        }
        if draft.anchor_line > self.summary.last_observed_line {
            self.summary.last_observed_line = draft.anchor_line;
            self.summary.last_observed_timestamp = draft.anchor_timestamp;
        }
    }

    fn record_dropped(&mut self) {
        saturating_increment(&mut self.summary.dropped_occurrence_count);
    }

    fn record_stored(&mut self, draft: ProblemEventDraft, event_id: ProblemEventId) {
        saturating_increment(&mut self.summary.stored_occurrence_count);
        if draft.anchor_line < self.first_stored_line {
            self.first_stored_line = draft.anchor_line;
            self.summary.first_stored_event_id = Some(event_id);
        }
        if draft.anchor_line > self.last_stored_line {
            self.last_stored_line = draft.anchor_line;
            self.summary.last_stored_event_id = Some(event_id);
        }
        self.event_ids.push(event_id);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProblemStats {
    pub observed_occurrence_count: u64,
    pub stored_occurrence_count: u64,
    pub dropped_occurrence_count: u64,
    /// Occurrences which have met their minimum grammar but are still open for
    /// bounded, late supporting evidence. `ProblemIndex` itself always reports
    /// zero; `ProblemEngine` overlays the correlation-store count.
    pub provisional_occurrence_count: u32,
    pub stored_group_count: u32,
    pub ungrouped_dropped_occurrence_count: u64,
    /// Supporting observations evicted before their natural correlation
    /// expiry. `ProblemEngine` overlays the recent-observation store counter.
    pub dropped_recent_observation_count: u64,
    /// True when a correlation capacity limit may have hidden late evidence.
    pub correlation_limited: bool,
    /// True when process-identity retention was evicted and later correlation
    /// may therefore be incomplete.
    pub identity_coverage_limited: bool,
    /// True when a fixed pending recognizer table evicted an unfinished fact.
    pub pending_coverage_limited: bool,
    /// Number of process-identity records evicted under their structural cap.
    pub identity_eviction_count: u64,
    /// Number of unfinished recognizer candidates evicted under fixed caps.
    pub pending_eviction_count: u64,
    /// Conservative bytes charged to the unified Problems memory budget.
    pub charged_bytes: usize,
    /// Estimated live heap capacity retained by all unified-budget owners.
    pub retained_heap_bytes: usize,
    /// True after the unified budget denies any analysis retention.
    pub memory_limited: bool,
    pub revision: u64,
    pub limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendDropReason {
    EventLimit,
    ObservationRefLimit,
    GroupLimit,
    MemoryBudget,
    Allocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    Stored {
        event_id: ProblemEventId,
        group_id: GroupId,
        created_group: bool,
    },
    Dropped {
        group_id: Option<GroupId>,
        reason: AppendDropReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemIndexError {
    InvalidLimits(ProblemIndexLimitsError),
    InvalidEvent(ProblemEventError),
    GroupKindMismatch {
        event_kind: ProblemKind,
        group_kind: ProblemKind,
    },
    PreparationAllocation,
}

impl From<ProblemEventError> for ProblemIndexError {
    fn from(value: ProblemEventError) -> Self {
        Self::InvalidEvent(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct QuerySnapshotId(u64);

impl QuerySnapshotId {
    pub const fn from_raw(raw: u64) -> Option<Self> {
        if raw == 0 {
            None
        } else {
            Some(Self(raw))
        }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum GroupSort {
    #[default]
    LastOccurrenceDesc,
    FirstOccurrenceAsc,
    ObservedCountDesc,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct GroupQuery {
    pub kind: Option<ProblemKind>,
    pub sort: GroupSort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupSnapshotCapture {
    pub group_count: usize,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupSortRecord {
    pub id: GroupId,
    kind: ProblemKind,
    first_observed_line: u32,
    last_observed_line: u32,
    observed_occurrence_count: u64,
}

impl GroupSortRecord {
    pub fn compare(left: &Self, right: &Self, sort: GroupSort) -> std::cmp::Ordering {
        match sort {
            GroupSort::LastOccurrenceDesc => right
                .last_observed_line
                .cmp(&left.last_observed_line)
                .then_with(|| left.id.cmp(&right.id)),
            GroupSort::FirstOccurrenceAsc => left
                .first_observed_line
                .cmp(&right.first_observed_line)
                .then_with(|| left.id.cmp(&right.id)),
            GroupSort::ObservedCountDesc => right
                .observed_occurrence_count
                .cmp(&left.observed_occurrence_count)
                .then_with(|| right.last_observed_line.cmp(&left.last_observed_line))
                .then_with(|| left.id.cmp(&right.id)),
        }
    }

    pub fn matches(self, query: &GroupQuery) -> bool {
        match query.kind {
            Some(kind) => self.kind == kind,
            None => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSpecError {
    EmptyPage,
    PageTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSpec {
    offset: usize,
    limit: usize,
}

impl PageSpec {
    pub const MAX_LIMIT: usize = 200;

    pub fn new(offset: usize, limit: usize) -> Result<Self, PageSpecError> {
        if limit == 0 {
            return Err(PageSpecError::EmptyPage);
        }
        if limit > Self::MAX_LIMIT {
            return Err(PageSpecError::PageTooLarge);
        }
        Ok(Self { offset, limit })
    }

    pub const fn offset(self) -> usize {
        self.offset
    }

    pub const fn limit(self) -> usize {
        self.limit
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPage {
    pub snapshot_id: QuerySnapshotId,
    pub revision: u64,
    pub total: usize,
    pub items: Vec<ProblemGroupSummary>,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrencePage {
    pub snapshot_id: QuerySnapshotId,
    pub revision: u64,
    pub total: usize,
    pub items: Vec<ProblemEventId>,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    GroupNotFound,
    NotFound,
    Expired,
    Evicted,
    Released,
    Reset,
    WrongKind,
    QueryMismatch,
    IdVectorLimit,
    Allocation,
    IdExhausted,
}

#[derive(Debug)]
enum SnapshotData {
    Groups {
        ids: Vec<GroupId>,
        query: GroupQuery,
    },
    Occurrences {
        group_id: GroupId,
        frozen_len: usize,
        max_event_id: Option<ProblemEventId>,
    },
}

#[derive(Debug)]
struct QuerySnapshot {
    id: QuerySnapshotId,
    revision: u64,
    last_access: Instant,
    id_bytes: usize,
    data: SnapshotData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotKind {
    Groups,
    Occurrences,
}

const MAX_RETIRED_SNAPSHOTS: usize = 64;

#[derive(Debug)]
pub struct ProblemIndex {
    limits: ProblemIndexLimits,
    events: Vec<ProblemEvent>,
    observation_refs: Vec<ObservationRef>,
    groups: Vec<ProblemGroup>,
    group_membership_capacity: usize,
    group_lookup: HashMap<GroupKey, GroupId>,
    stats: ProblemStats,
    snapshots: Vec<QuerySnapshot>,
    retired_snapshots: [Option<(QuerySnapshotId, SnapshotError)>; MAX_RETIRED_SNAPSHOTS],
    retired_snapshot_len: usize,
    snapshot_id_bytes: usize,
    next_snapshot_id: u64,
    budget: ProblemMemoryBudget,
    memory: ProblemMemoryAccount,
}

impl Default for ProblemIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ProblemIndex {
    pub fn new() -> Self {
        Self::with_limits_and_budget(ProblemIndexLimits::default(), ProblemMemoryBudget::new())
            .expect("the documented Problems index limits are valid")
    }

    pub fn with_limits(limits: ProblemIndexLimits) -> Result<Self, ProblemIndexError> {
        Self::with_limits_and_budget(limits, ProblemMemoryBudget::new())
    }

    pub(crate) fn with_limits_and_budget(
        limits: ProblemIndexLimits,
        budget: ProblemMemoryBudget,
    ) -> Result<Self, ProblemIndexError> {
        let memory = budget.account();
        Ok(Self {
            limits: limits
                .validate()
                .map_err(ProblemIndexError::InvalidLimits)?,
            events: Vec::new(),
            observation_refs: Vec::new(),
            groups: Vec::new(),
            group_membership_capacity: 0,
            group_lookup: HashMap::new(),
            stats: ProblemStats::default(),
            snapshots: Vec::new(),
            retired_snapshots: [None; MAX_RETIRED_SNAPSHOTS],
            retired_snapshot_len: 0,
            snapshot_id_bytes: 0,
            next_snapshot_id: 1,
            budget,
            memory,
        })
    }

    pub fn append(
        &mut self,
        draft: ProblemEventDraft,
        group_key: GroupKey,
        candidates: &[ObservationCandidate],
    ) -> Result<AppendOutcome, ProblemIndexError> {
        self.append_with_summary(
            draft,
            group_key,
            ProblemDisplaySummary::default(),
            candidates,
        )
    }

    pub fn append_with_summary(
        &mut self,
        draft: ProblemEventDraft,
        group_key: GroupKey,
        display_summary: ProblemDisplaySummary,
        candidates: &[ObservationCandidate],
    ) -> Result<AppendOutcome, ProblemIndexError> {
        ProblemEvent::new(draft, 0, 0, 0, 0)?;
        if draft.kind != group_key.kind {
            return Err(ProblemIndexError::GroupKindMismatch {
                event_kind: draft.kind,
                group_kind: group_key.kind,
            });
        }

        let existing_group = self.record_observed_occurrence(draft, group_key);
        let selected = match select_observations(candidates, &self.budget) {
            Ok(selected) => selected,
            Err(reason) => return Ok(self.record_drop(existing_group, reason)),
        };
        self.append_selected_after_observed(
            draft,
            group_key,
            display_summary,
            existing_group,
            selected,
        )
    }

    pub(crate) fn append_with_summary_and_total(
        &mut self,
        draft: ProblemEventDraft,
        group_key: GroupKey,
        display_summary: ProblemDisplaySummary,
        candidates: &[ObservationCandidate],
        observation_total: u16,
        count_limited: bool,
    ) -> Result<AppendOutcome, ProblemIndexError> {
        let selected =
            SelectedObservations::from_materialized(candidates, observation_total, count_limited)?;
        ProblemEvent::new(draft, 0, 0, selected.materialized_len, selected.total)?;
        if draft.kind != group_key.kind {
            return Err(ProblemIndexError::GroupKindMismatch {
                event_kind: draft.kind,
                group_kind: group_key.kind,
            });
        }
        let existing_group = self.record_observed_occurrence(draft, group_key);
        self.append_selected_after_observed(
            draft,
            group_key,
            display_summary,
            existing_group,
            selected,
        )
    }

    fn append_selected_after_observed(
        &mut self,
        mut draft: ProblemEventDraft,
        group_key: GroupKey,
        display_summary: ProblemDisplaySummary,
        existing_group: Option<GroupId>,
        selected: SelectedObservations,
    ) -> Result<AppendOutcome, ProblemIndexError> {
        if selected.count_limited {
            draft
                .boundary
                .insert(BoundaryFlags::OBSERVATION_COUNT_LIMITED);
        }
        if selected.total > u16::from(MAX_MATERIALIZED_OBSERVATIONS) {
            draft
                .boundary
                .insert(BoundaryFlags::OBSERVATION_REFS_TRUNCATED);
        }

        let drop_reason = if existing_group.is_none() && self.groups.len() >= self.limits.max_groups
        {
            Some(AppendDropReason::GroupLimit)
        } else if self.events.len() >= self.limits.max_events {
            Some(AppendDropReason::EventLimit)
        } else if self
            .observation_refs
            .len()
            .checked_add(selected.len())
            .is_none_or(|required| required > self.limits.max_observation_refs)
        {
            Some(AppendDropReason::ObservationRefLimit)
        } else {
            None
        };
        if let Some(reason) = drop_reason {
            return Ok(self.record_drop(existing_group, reason));
        }

        let mut new_group_membership = match self.reserve_append(existing_group, selected.len()) {
            Ok(membership) => membership,
            Err(reason) => return Ok(self.record_drop(existing_group, reason)),
        };

        let group_id = existing_group.unwrap_or(GroupId(self.groups.len() as u32));
        let event_id = ProblemEventId(self.events.len() as u32);
        let observation_start = self.observation_refs.len() as u32;
        let observation_len = selected.materialized_len;
        let event = ProblemEvent::new(
            draft,
            group_id.0,
            observation_start,
            observation_len,
            selected.total,
        )?;

        self.observation_refs.extend(selected.iter().copied());
        self.events.push(event);

        let created_group = existing_group.is_none();
        if let Some(existing) = existing_group {
            self.groups[existing.0 as usize].record_stored(draft, event_id);
        } else {
            let mut event_ids = new_group_membership
                .take()
                .expect("new groups reserve their membership before committing");
            event_ids.push(event_id);
            self.group_membership_capacity = self
                .group_membership_capacity
                .saturating_add(event_ids.capacity());
            self.groups.push(ProblemGroup::new(
                group_id,
                group_key,
                display_summary,
                draft,
                event_id,
                event_ids,
            ));
            self.group_lookup.insert(group_key, group_id);
            self.stats.stored_group_count = self.groups.len() as u32;
        }
        saturating_increment(&mut self.stats.stored_occurrence_count);
        self.settle_index_memory();

        Ok(AppendOutcome::Stored {
            event_id,
            group_id,
            created_group,
        })
    }

    fn record_observed_occurrence(
        &mut self,
        draft: ProblemEventDraft,
        group_key: GroupKey,
    ) -> Option<GroupId> {
        saturating_increment(&mut self.stats.observed_occurrence_count);
        bump_revision(&mut self.stats);
        let existing_group = self.group_lookup.get(&group_key).copied();
        if let Some(group_id) = existing_group {
            self.groups[group_id.0 as usize].record_observed(draft);
        }
        existing_group
    }

    pub fn stats(&self) -> ProblemStats {
        let mut stats = self.stats;
        let memory = self.budget.stats();
        stats.charged_bytes = memory.charged_bytes;
        stats.retained_heap_bytes = memory.retained_heap_bytes;
        stats.memory_limited = memory.limited;
        stats.limited |= stats.memory_limited;
        stats
    }

    pub fn event(&self, id: ProblemEventId) -> Option<ProblemEvent> {
        self.events.get(id.0 as usize).copied()
    }

    pub fn event_observations(&self, id: ProblemEventId) -> Option<&[ObservationRef]> {
        let event = self.events.get(id.0 as usize)?;
        let start = event.observation_start() as usize;
        let end = start.checked_add(event.observation_len() as usize)?;
        self.observation_refs.get(start..end)
    }

    pub fn group(&self, id: GroupId) -> Option<ProblemGroupSummary> {
        self.groups.get(id.0 as usize).map(|group| group.summary)
    }

    pub fn group_event_ids(&self, id: GroupId) -> Option<&[ProblemEventId]> {
        self.groups
            .get(id.0 as usize)
            .map(|group| group.event_ids.as_slice())
    }

    pub const fn events_len(&self) -> usize {
        self.events.len()
    }

    pub const fn observation_refs_len(&self) -> usize {
        self.observation_refs.len()
    }

    pub fn create_group_snapshot(
        &mut self,
        query: &GroupQuery,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.create_group_snapshot_at(query, Instant::now())
    }

    pub fn create_group_snapshot_at(
        &mut self,
        query: &GroupQuery,
        now: Instant,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        let projected_capacity = projected_vec_capacity(0, 0, self.groups.len())
            .map_err(|_| SnapshotError::Allocation)?;
        let mut scratch = self.budget.account();
        scratch
            .try_set_usage_transient(
                vec_usage::<GroupId>(projected_capacity).map_err(|_| SnapshotError::Allocation)?,
            )
            .map_err(|_| SnapshotError::Allocation)?;
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.groups.len())
            .map_err(|_| SnapshotError::Allocation)?;
        let actual_usage =
            vec_usage::<GroupId>(ids.capacity()).map_err(|_| SnapshotError::Allocation)?;
        if actual_usage.charged_bytes <= scratch.usage().charged_bytes {
            scratch.settle_precharged(actual_usage);
        }
        ids.extend(
            self.groups
                .iter()
                .filter(|group| query.kind.is_none_or(|kind| group.summary.key.kind == kind))
                .map(|group| group.summary.id),
        );
        ids.sort_by(|left, right| {
            let left = self.groups[left.0 as usize].summary;
            let right = self.groups[right.0 as usize].summary;
            match query.sort {
                GroupSort::LastOccurrenceDesc => right
                    .last_observed_line
                    .cmp(&left.last_observed_line)
                    .then_with(|| left.id.cmp(&right.id)),
                GroupSort::FirstOccurrenceAsc => left
                    .first_observed_line
                    .cmp(&right.first_observed_line)
                    .then_with(|| left.id.cmp(&right.id)),
                GroupSort::ObservedCountDesc => right
                    .observed_occurrence_count
                    .cmp(&left.observed_occurrence_count)
                    .then_with(|| right.last_observed_line.cmp(&left.last_observed_line))
                    .then_with(|| left.id.cmp(&right.id)),
            }
        });

        let id_bytes = ids
            .capacity()
            .checked_mul(size_of::<GroupId>())
            .ok_or(SnapshotError::IdVectorLimit)?;
        if id_bytes > self.limits.max_snapshot_id_bytes {
            return Err(SnapshotError::IdVectorLimit);
        }
        scratch.release();
        self.install_snapshot(now, id_bytes, SnapshotData::Groups { ids, query: *query })
    }

    pub fn group_snapshot_capture(&self) -> GroupSnapshotCapture {
        GroupSnapshotCapture {
            group_count: self.groups.len(),
            revision: self.stats.revision,
        }
    }

    /// Copy at most `limit` compact sort records from a frozen group-id prefix.
    /// Callers can repeat this under short external lock sections and sort the
    /// resulting records after releasing the Session lock.
    pub fn group_sort_records(
        &self,
        query: &GroupQuery,
        capture: GroupSnapshotCapture,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<GroupSortRecord>, SnapshotError> {
        let end = offset
            .saturating_add(limit)
            .min(capture.group_count)
            .min(self.groups.len());
        let start = offset.min(end);
        let mut records = Vec::new();
        records
            .try_reserve_exact(end - start)
            .map_err(|_| SnapshotError::Allocation)?;
        records.extend(
            self.groups[start..end]
                .iter()
                .map(|group| {
                    let summary = group.summary;
                    GroupSortRecord {
                        id: summary.id,
                        kind: summary.key.kind,
                        first_observed_line: summary.first_observed_line,
                        last_observed_line: summary.last_observed_line,
                        observed_occurrence_count: summary.observed_occurrence_count,
                    }
                })
                .filter(|record| record.matches(query)),
        );
        Ok(records)
    }

    pub fn install_group_snapshot_ids(
        &mut self,
        ids: Vec<GroupId>,
        revision: u64,
        query: GroupQuery,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        let id_bytes = ids
            .capacity()
            .checked_mul(size_of::<GroupId>())
            .ok_or(SnapshotError::IdVectorLimit)?;
        self.install_snapshot_with_revision(
            Instant::now(),
            id_bytes,
            SnapshotData::Groups { ids, query },
            revision,
        )
    }

    pub fn create_occurrence_snapshot(
        &mut self,
        group_id: GroupId,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.create_occurrence_snapshot_at(group_id, Instant::now())
    }

    pub fn create_occurrence_snapshot_at(
        &mut self,
        group_id: GroupId,
        now: Instant,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        let group = self
            .groups
            .get(group_id.0 as usize)
            .ok_or(SnapshotError::GroupNotFound)?;
        let frozen_len = group.event_ids.len();
        let max_event_id = group.event_ids.last().copied();
        self.install_snapshot(
            now,
            0,
            SnapshotData::Occurrences {
                group_id,
                frozen_len,
                max_event_id,
            },
        )
    }

    pub fn group_snapshot_page(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<GroupPage, SnapshotError> {
        self.group_snapshot_page_at(snapshot_id, page, Instant::now())
    }

    pub fn group_snapshot_page_for_query(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        query: GroupQuery,
    ) -> Result<GroupPage, SnapshotError> {
        self.group_snapshot_page_at_expected(snapshot_id, page, Some(query), Instant::now())
    }

    pub fn group_snapshot_page_at(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        now: Instant,
    ) -> Result<GroupPage, SnapshotError> {
        self.group_snapshot_page_at_expected(snapshot_id, page, None, now)
    }

    fn group_snapshot_page_at_expected(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        expected_query: Option<GroupQuery>,
        now: Instant,
    ) -> Result<GroupPage, SnapshotError> {
        let snapshot_index = self.active_snapshot(snapshot_id, SnapshotKind::Groups, now)?;
        let (revision, total, selected_ids) = {
            let snapshot = &self.snapshots[snapshot_index];
            let SnapshotData::Groups { ids, query } = &snapshot.data else {
                unreachable!("active_snapshot checked the snapshot kind");
            };
            if expected_query.is_some_and(|expected| expected != *query) {
                return Err(SnapshotError::QueryMismatch);
            }
            let start = page.offset.min(ids.len());
            let end = start.saturating_add(page.limit).min(ids.len());
            let mut selected = Vec::new();
            selected
                .try_reserve_exact(end - start)
                .map_err(|_| SnapshotError::Allocation)?;
            selected.extend_from_slice(&ids[start..end]);
            (snapshot.revision, ids.len(), selected)
        };
        let mut items = Vec::new();
        items
            .try_reserve_exact(selected_ids.len())
            .map_err(|_| SnapshotError::Allocation)?;
        for group_id in selected_ids {
            if let Some(group) = self.groups.get(group_id.0 as usize) {
                items.push(group.summary);
            }
        }
        let consumed = page.offset.min(total).saturating_add(items.len());
        Ok(GroupPage {
            snapshot_id,
            revision,
            total,
            items,
            next_offset: (consumed < total).then_some(consumed),
        })
    }

    pub fn occurrence_snapshot_page(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.occurrence_snapshot_page_at(snapshot_id, page, Instant::now())
    }

    pub fn occurrence_snapshot_page_for_group(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        group_id: GroupId,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.occurrence_snapshot_page_at_expected(snapshot_id, page, Some(group_id), Instant::now())
    }

    pub fn occurrence_snapshot_page_at(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        now: Instant,
    ) -> Result<OccurrencePage, SnapshotError> {
        self.occurrence_snapshot_page_at_expected(snapshot_id, page, None, now)
    }

    fn occurrence_snapshot_page_at_expected(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        expected_group_id: Option<GroupId>,
        now: Instant,
    ) -> Result<OccurrencePage, SnapshotError> {
        let snapshot_index = self.active_snapshot(snapshot_id, SnapshotKind::Occurrences, now)?;
        let (revision, group_id, frozen_len, max_event_id) = {
            let snapshot = &self.snapshots[snapshot_index];
            let SnapshotData::Occurrences {
                group_id,
                frozen_len,
                max_event_id,
            } = snapshot.data
            else {
                unreachable!("active_snapshot checked the snapshot kind");
            };
            (snapshot.revision, group_id, frozen_len, max_event_id)
        };
        if expected_group_id.is_some_and(|expected| expected != group_id) {
            return Err(SnapshotError::QueryMismatch);
        }
        let group = self
            .groups
            .get(group_id.0 as usize)
            .ok_or(SnapshotError::GroupNotFound)?;
        let frozen_len = frozen_len.min(group.event_ids.len());
        debug_assert_eq!(
            max_event_id,
            frozen_len
                .checked_sub(1)
                .and_then(|last| group.event_ids.get(last))
                .copied()
        );
        let start = page.offset.min(frozen_len);
        let end = start.saturating_add(page.limit).min(frozen_len);
        let mut items = Vec::new();
        items
            .try_reserve_exact(end - start)
            .map_err(|_| SnapshotError::Allocation)?;
        items.extend_from_slice(&group.event_ids[start..end]);
        Ok(OccurrencePage {
            snapshot_id,
            revision,
            total: frozen_len,
            items,
            next_offset: (end < frozen_len).then_some(end),
        })
    }

    pub fn release_snapshot(&mut self, snapshot_id: QuerySnapshotId) -> bool {
        let Some(index) = self
            .snapshots
            .iter()
            .position(|snapshot| snapshot.id == snapshot_id)
        else {
            return false;
        };
        self.remove_snapshot(index, SnapshotError::Released);
        true
    }

    pub fn reset(&mut self) {
        while !self.snapshots.is_empty() {
            self.remove_snapshot(0, SnapshotError::Reset);
        }
        self.events = Vec::new();
        self.observation_refs = Vec::new();
        self.groups = Vec::new();
        self.group_membership_capacity = 0;
        self.group_lookup = HashMap::new();
        self.snapshots = Vec::new();
        self.snapshot_id_bytes = 0;
        self.stats = ProblemStats::default();
        self.memory.release();
        self.budget.clear_limit_state();
    }

    pub const fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    pub const fn snapshot_id_bytes(&self) -> usize {
        self.snapshot_id_bytes
    }

    fn install_snapshot(
        &mut self,
        now: Instant,
        id_bytes: usize,
        data: SnapshotData,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        self.install_snapshot_with_revision(now, id_bytes, data, self.stats.revision)
    }

    fn install_snapshot_with_revision(
        &mut self,
        now: Instant,
        id_bytes: usize,
        data: SnapshotData,
        revision: u64,
    ) -> Result<QuerySnapshotId, SnapshotError> {
        if id_bytes > self.limits.max_snapshot_id_bytes {
            return Err(SnapshotError::IdVectorLimit);
        }
        let next_snapshot_id = self
            .next_snapshot_id
            .checked_add(1)
            .ok_or(SnapshotError::IdExhausted)?;
        self.expire_snapshots(now);
        while self.snapshots.len() >= self.limits.max_snapshots
            || self
                .snapshot_id_bytes
                .checked_add(id_bytes)
                .is_none_or(|total| total > self.limits.max_snapshot_id_bytes)
        {
            if self.snapshots.is_empty() {
                return Err(SnapshotError::IdVectorLimit);
            }
            self.evict_lru();
        }
        loop {
            let mut shape = self.memory_shape();
            shape.snapshots_capacity =
                projected_vec_capacity(self.snapshots.len(), self.snapshots.capacity(), 1)
                    .map_err(|_| SnapshotError::Allocation)?;
            shape.snapshot_id_capacity = shape
                .snapshot_id_capacity
                .checked_add(id_bytes)
                .ok_or(SnapshotError::Allocation)?;
            if id_bytes != 0 {
                shape.snapshot_id_allocations = shape
                    .snapshot_id_allocations
                    .checked_add(1)
                    .ok_or(SnapshotError::Allocation)?;
            }
            let projection = index_memory_usage(shape).map_err(|_| SnapshotError::Allocation)?;
            if self.memory.try_set_usage_transient(projection).is_ok() {
                break;
            }
            if self.snapshots.is_empty() {
                return Err(SnapshotError::Allocation);
            }
            self.evict_lru();
        }
        if self.snapshots.try_reserve_exact(1).is_err() {
            drop(data);
            self.settle_index_memory();
            return Err(SnapshotError::Allocation);
        }
        let id = QuerySnapshotId(self.next_snapshot_id);
        self.next_snapshot_id = next_snapshot_id;
        self.snapshot_id_bytes += id_bytes;
        self.snapshots.push(QuerySnapshot {
            id,
            revision,
            last_access: now,
            id_bytes,
            data,
        });
        self.settle_index_memory();
        Ok(id)
    }

    fn active_snapshot(
        &mut self,
        id: QuerySnapshotId,
        expected: SnapshotKind,
        now: Instant,
    ) -> Result<usize, SnapshotError> {
        self.expire_snapshots(now);
        let Some(index) = self.snapshots.iter().position(|snapshot| snapshot.id == id) else {
            return Err(self.retired_reason(id).unwrap_or(SnapshotError::NotFound));
        };
        let actual = match self.snapshots[index].data {
            SnapshotData::Groups { .. } => SnapshotKind::Groups,
            SnapshotData::Occurrences { .. } => SnapshotKind::Occurrences,
        };
        if actual != expected {
            return Err(SnapshotError::WrongKind);
        }
        self.snapshots[index].last_access = now;
        Ok(index)
    }

    fn expire_snapshots(&mut self, now: Instant) {
        for index in (0..self.snapshots.len()).rev() {
            if now.saturating_duration_since(self.snapshots[index].last_access)
                >= self.limits.snapshot_ttl
            {
                self.remove_snapshot(index, SnapshotError::Expired);
            }
        }
    }

    fn evict_lru(&mut self) {
        let index = self
            .snapshots
            .iter()
            .enumerate()
            .min_by_key(|(_, snapshot)| (snapshot.last_access, snapshot.id))
            .map(|(index, _)| index)
            .expect("LRU eviction only runs with active snapshots");
        self.remove_snapshot(index, SnapshotError::Evicted);
    }

    fn remove_snapshot(&mut self, index: usize, reason: SnapshotError) {
        let removed = self.snapshots.remove(index);
        self.snapshot_id_bytes = self.snapshot_id_bytes.saturating_sub(removed.id_bytes);
        if self.retired_snapshot_len == MAX_RETIRED_SNAPSHOTS {
            self.retired_snapshots.copy_within(1.., 0);
            self.retired_snapshot_len -= 1;
        }
        self.retired_snapshots[self.retired_snapshot_len] = Some((removed.id, reason));
        self.retired_snapshot_len += 1;
        self.settle_index_memory();
    }

    fn retired_reason(&self, id: QuerySnapshotId) -> Option<SnapshotError> {
        self.retired_snapshots
            .iter()
            .take(self.retired_snapshot_len)
            .flatten()
            .rev()
            .find_map(|(retired_id, reason)| (*retired_id == id).then_some(*reason))
    }

    fn reserve_append(
        &mut self,
        group: Option<GroupId>,
        observation_count: usize,
    ) -> Result<Option<Vec<ProblemEventId>>, AppendDropReason> {
        loop {
            let projection = self
                .append_memory_projection(group, observation_count)
                .map_err(|_| AppendDropReason::MemoryBudget)?;
            if self.memory.try_set_usage_transient(projection).is_ok() {
                break;
            }
            if self.snapshots.is_empty() {
                let _ = self.memory.try_set_usage(projection);
                return Err(AppendDropReason::MemoryBudget);
            }
            self.evict_lru();
        }

        if self.events.try_reserve_exact(1).is_err()
            || self
                .observation_refs
                .try_reserve_exact(observation_count)
                .is_err()
        {
            self.settle_index_memory();
            return Err(AppendDropReason::Allocation);
        }

        if let Some(group_id) = group {
            let membership = &mut self.groups[group_id.0 as usize].event_ids;
            let old_capacity = membership.capacity();
            if membership.try_reserve_exact(1).is_err() {
                self.settle_index_memory();
                return Err(AppendDropReason::Allocation);
            }
            self.group_membership_capacity = self
                .group_membership_capacity
                .saturating_sub(old_capacity)
                .saturating_add(membership.capacity());
            Ok(None)
        } else {
            if self.groups.try_reserve_exact(1).is_err()
                || self.group_lookup.try_reserve(1).is_err()
            {
                self.settle_index_memory();
                return Err(AppendDropReason::Allocation);
            }
            let mut membership = Vec::new();
            if membership.try_reserve_exact(1).is_err() {
                self.settle_index_memory();
                return Err(AppendDropReason::Allocation);
            }
            Ok(Some(membership))
        }
    }

    fn append_memory_projection(
        &self,
        group: Option<GroupId>,
        observation_count: usize,
    ) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
        let mut shape = self.memory_shape();
        shape.events_capacity =
            projected_vec_capacity(self.events.len(), self.events.capacity(), 1)?;
        shape.observation_refs_capacity = projected_vec_capacity(
            self.observation_refs.len(),
            self.observation_refs.capacity(),
            observation_count,
        )?;
        if let Some(group_id) = group {
            let membership = &self.groups[group_id.0 as usize].event_ids;
            let projected = projected_vec_capacity(membership.len(), membership.capacity(), 1)?;
            shape.group_membership_capacity = shape
                .group_membership_capacity
                .checked_sub(membership.capacity())
                .and_then(|capacity| capacity.checked_add(projected))
                .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
        } else {
            shape.groups_capacity =
                projected_vec_capacity(self.groups.len(), self.groups.capacity(), 1)?;
            shape.group_lookup_capacity =
                projected_hash_capacity(self.group_lookup.len(), self.group_lookup.capacity(), 1)?;
            shape.group_membership_capacity = shape
                .group_membership_capacity
                .checked_add(1)
                .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
            shape.group_membership_allocations = shape
                .group_membership_allocations
                .checked_add(1)
                .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
        }
        index_memory_usage(shape)
    }

    fn memory_shape(&self) -> IndexMemoryShape {
        let snapshot_id_allocations = self
            .snapshots
            .iter()
            .filter(|snapshot| snapshot.id_bytes != 0)
            .count();
        IndexMemoryShape {
            events_capacity: self.events.capacity(),
            observation_refs_capacity: self.observation_refs.capacity(),
            groups_capacity: self.groups.capacity(),
            group_membership_capacity: self.group_membership_capacity,
            group_membership_allocations: self.groups.len(),
            group_lookup_capacity: self.group_lookup.capacity(),
            snapshots_capacity: self.snapshots.capacity(),
            snapshot_id_capacity: self.snapshot_id_bytes,
            snapshot_id_allocations,
        }
    }

    fn settle_index_memory(&mut self) {
        let Ok(usage) = index_memory_usage(self.memory_shape()) else {
            return;
        };
        if usage.charged_bytes <= self.memory.usage().charged_bytes {
            self.memory.settle_precharged(usage);
        } else {
            let _ = self.memory.try_set_usage(usage);
        }
    }

    fn record_drop(
        &mut self,
        group_id: Option<GroupId>,
        reason: AppendDropReason,
    ) -> AppendOutcome {
        self.stats.limited = true;
        saturating_increment(&mut self.stats.dropped_occurrence_count);
        if let Some(group_id) = group_id {
            self.groups[group_id.0 as usize].record_dropped();
        } else {
            saturating_increment(&mut self.stats.ungrouped_dropped_occurrence_count);
        }
        AppendOutcome::Dropped { group_id, reason }
    }
}

#[derive(Debug)]
struct SelectedObservations {
    materialized: [Option<ObservationRef>; MAX_MATERIALIZED_OBSERVATIONS as usize],
    materialized_len: u8,
    total: u16,
    count_limited: bool,
}

impl SelectedObservations {
    fn from_materialized(
        candidates: &[ObservationCandidate],
        total: u16,
        count_limited: bool,
    ) -> Result<Self, ProblemIndexError> {
        let mut selected = Self {
            materialized: [None; MAX_MATERIALIZED_OBSERVATIONS as usize],
            materialized_len: 0,
            total,
            count_limited,
        };
        for candidate in candidates {
            if selected
                .iter()
                .any(|known| known.dedup_key() == candidate.reference.dedup_key())
            {
                continue;
            }
            let index = usize::from(selected.materialized_len);
            if index == selected.materialized.len() {
                break;
            }
            selected.materialized[index] = Some(candidate.reference);
            selected.materialized_len += 1;
        }
        if total > MAX_ADOPTED_OBSERVATIONS || u16::from(selected.materialized_len) > total {
            return Err(ProblemIndexError::InvalidEvent(
                ProblemEventError::InvalidObservationTotal,
            ));
        }
        Ok(selected)
    }

    fn len(&self) -> usize {
        usize::from(self.materialized_len)
    }

    fn iter(&self) -> impl Iterator<Item = &ObservationRef> {
        self.materialized[..self.len()].iter().map(|reference| {
            reference
                .as_ref()
                .expect("the compact materialized prefix is contiguous")
        })
    }
}

fn select_observations(
    candidates: &[ObservationCandidate],
    budget: &ProblemMemoryBudget,
) -> Result<SelectedObservations, AppendDropReason> {
    let adopted_limit = usize::from(MAX_ADOPTED_OBSERVATIONS);
    let reserve = candidates.len().min(adopted_limit);
    let map_capacity =
        projected_hash_capacity(0, 0, reserve).map_err(|_| AppendDropReason::MemoryBudget)?;
    let vector_capacity =
        projected_vec_capacity(0, 0, reserve).map_err(|_| AppendDropReason::MemoryBudget)?;
    let scratch_usage = hash_map_usage::<
        (u32, super::facts::RuleId, super::facts::ObservationRole),
        usize,
    >(map_capacity)
    .and_then(|usage| {
        usage.checked_add(vec_usage::<(ObservationCandidate, usize)>(vector_capacity)?)
    })
    .map_err(|_| AppendDropReason::MemoryBudget)?;
    let mut scratch = budget.account();
    scratch
        .try_set_usage(scratch_usage)
        .map_err(|_| AppendDropReason::MemoryBudget)?;
    let mut by_key = HashMap::new();
    by_key
        .try_reserve(reserve)
        .map_err(|_| AppendDropReason::Allocation)?;
    let mut unique = Vec::<(ObservationCandidate, usize)>::new();
    unique
        .try_reserve(reserve)
        .map_err(|_| AppendDropReason::Allocation)?;
    let actual_scratch = hash_map_usage::<
        (u32, super::facts::RuleId, super::facts::ObservationRole),
        usize,
    >(by_key.capacity())
    .and_then(|usage| {
        usage.checked_add(vec_usage::<(ObservationCandidate, usize)>(
            unique.capacity(),
        )?)
    })
    .map_err(|_| AppendDropReason::MemoryBudget)?;
    if actual_scratch.charged_bytes <= scratch.usage().charged_bytes {
        scratch.settle_precharged(actual_scratch);
    }
    let mut count_limited = false;

    for (ordinal, candidate) in candidates.iter().copied().enumerate() {
        let key = candidate.reference.dedup_key();
        if let Some(existing) = by_key.get(&key).copied() {
            let entry: &mut (ObservationCandidate, usize) = &mut unique[existing];
            if candidate.priority < entry.0.priority {
                entry.0 = candidate;
            }
            continue;
        }
        if unique.len() == adopted_limit {
            count_limited = true;
            continue;
        }
        by_key.insert(key, unique.len());
        unique.push((candidate, ordinal));
    }

    unique.sort_by_key(|(candidate, ordinal)| (candidate.priority, *ordinal));
    let mut materialized = [None; MAX_MATERIALIZED_OBSERVATIONS as usize];
    let mut materialized_len = 0_u8;
    for (candidate, _) in unique
        .iter()
        .take(usize::from(MAX_MATERIALIZED_OBSERVATIONS))
    {
        materialized[usize::from(materialized_len)] = Some(candidate.reference);
        materialized_len += 1;
    }

    Ok(SelectedObservations {
        materialized,
        materialized_len,
        total: unique.len() as u16,
        count_limited,
    })
}

#[derive(Debug, Clone, Copy)]
struct IndexMemoryShape {
    events_capacity: usize,
    observation_refs_capacity: usize,
    groups_capacity: usize,
    group_membership_capacity: usize,
    group_membership_allocations: usize,
    group_lookup_capacity: usize,
    snapshots_capacity: usize,
    snapshot_id_capacity: usize,
    snapshot_id_allocations: usize,
}

fn index_memory_usage(
    shape: IndexMemoryShape,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    let mut usage = vec_usage::<ProblemEvent>(shape.events_capacity)?;
    usage = usage.checked_add(vec_usage::<ObservationRef>(
        shape.observation_refs_capacity,
    )?)?;
    usage = usage.checked_add(vec_usage::<ProblemGroup>(shape.groups_capacity)?)?;
    usage = usage.checked_add(aggregate_vec_usage::<ProblemEventId>(
        shape.group_membership_capacity,
        shape.group_membership_allocations,
    )?)?;
    usage = usage.checked_add(hash_map_usage::<GroupKey, GroupId>(
        shape.group_lookup_capacity,
    )?)?;
    usage = usage.checked_add(vec_usage::<QuerySnapshot>(shape.snapshots_capacity)?)?;
    usage = usage.checked_add(aggregate_vec_usage::<GroupId>(
        shape.snapshot_id_capacity / size_of::<GroupId>(),
        shape.snapshot_id_allocations,
    )?)?;
    Ok(usage)
}

fn projected_vec_capacity(
    len: usize,
    capacity: usize,
    additional: usize,
) -> Result<usize, ProblemMemoryBudgetError> {
    let required = len
        .checked_add(additional)
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    if required <= capacity {
        return Ok(capacity);
    }
    required
        .checked_next_power_of_two()
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)
}

fn projected_hash_capacity(
    len: usize,
    capacity: usize,
    additional: usize,
) -> Result<usize, ProblemMemoryBudgetError> {
    let required = len
        .checked_add(additional)
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)?;
    if required <= capacity {
        return Ok(capacity);
    }
    required
        .checked_next_power_of_two()
        .and_then(|capacity| capacity.checked_mul(2))
        .map(|capacity| capacity.max(4))
        .ok_or(ProblemMemoryBudgetError::SizeOverflow)
}

fn saturating_increment(value: &mut u64) {
    *value = value.saturating_add(1);
}

fn bump_revision(stats: &mut ProblemStats) {
    stats.revision = stats.revision.saturating_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::{
        EvidenceFormat, EvidencePriority, FingerprintBuilder, FingerprintTokenKind, LineProvenance,
        ObservationRole, ProcessFingerprintKey, RuleId,
    };
    use std::time::Instant;

    fn key(seed: &[u8]) -> GroupKey {
        let process = ProcessFingerprintKey::new(Some("com.example.app"));
        let mut builder = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::FullStack,
            IdentityQuality::KnownProcess,
            &process,
        );
        builder.token(FingerprintTokenKind::ExceptionType, seed);
        GroupKey::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::FullStack,
            IdentityQuality::KnownProcess,
            builder.finish(),
        )
    }

    fn draft(line: u32) -> ProblemEventDraft {
        ProblemEventDraft {
            start_line: line,
            end_line: line,
            anchor_line: line,
            ..ProblemEventDraft::minimal(ProblemKind::JavaCrash)
        }
    }

    fn timestamp(second: u8) -> PackedLogTimestamp {
        PackedLogTimestamp::new(7, 26, 12, 0, second, 0).unwrap()
    }

    fn candidate(
        line: u32,
        role: ObservationRole,
        priority: EvidencePriority,
    ) -> ObservationCandidate {
        ObservationCandidate::new(
            ObservationRef::new(
                line,
                RuleId::JavaUncaughtV1,
                role,
                EvidenceFormat::AospText,
                LineProvenance::Unknown,
            )
            .unwrap(),
            priority,
        )
    }

    fn stored(outcome: AppendOutcome) -> (ProblemEventId, GroupId) {
        match outcome {
            AppendOutcome::Stored {
                event_id, group_id, ..
            } => (event_id, group_id),
            other => panic!("expected stored occurrence, got {other:?}"),
        }
    }

    #[test]
    fn default_limits_freeze_the_structural_caps() {
        let limits = ProblemIndexLimits::default();
        assert_eq!(limits.max_events, 1_000_000);
        assert_eq!(limits.max_observation_refs, 4_000_000);
        assert_eq!(limits.max_groups, 100_000);
        assert_eq!(limits.max_snapshots, 8);
        assert_eq!(limits.snapshot_ttl, Duration::from_secs(5 * 60));
        assert_eq!(limits.max_snapshot_id_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn opaque_ids_have_checked_command_boundary_constructors() {
        assert_eq!(GroupId::from_raw(42).raw(), 42);
        assert_eq!(QuerySnapshotId::from_raw(0), None);
        assert_eq!(QuerySnapshotId::from_raw(9).unwrap().raw(), 9);
    }

    #[test]
    fn append_groups_occurrences_and_materializes_by_priority() {
        let mut index = ProblemIndex::new();
        let group_key = key(b"java.lang.IllegalStateException");
        let observations = [
            candidate(
                20,
                ObservationRole::Supporting,
                EvidencePriority::Supporting,
            ),
            candidate(
                10,
                ObservationRole::Primary,
                EvidencePriority::MinimumGrammar,
            ),
            candidate(11, ObservationRole::Death, EvidencePriority::Outcome),
            candidate(10, ObservationRole::Primary, EvidencePriority::Supporting),
        ];

        let first = index.append(draft(10), group_key, &observations).unwrap();
        let second = index.append(draft(30), group_key, &[]).unwrap();
        let (first_id, group_id) = match first {
            AppendOutcome::Stored {
                event_id,
                group_id,
                created_group: true,
            } => (event_id, group_id),
            outcome => panic!("unexpected first append: {outcome:?}"),
        };
        assert!(matches!(
            second,
            AppendOutcome::Stored {
                group_id: id,
                created_group: false,
                ..
            } if id == group_id
        ));

        let event = index.event(first_id).unwrap();
        assert_eq!(event.observation_total(), 3);
        assert_eq!(
            index
                .event_observations(first_id)
                .unwrap()
                .iter()
                .map(|reference| reference.line())
                .collect::<Vec<_>>(),
            vec![10, 11, 20]
        );
        let group = index.group(group_id).unwrap();
        assert_eq!(group.observed_occurrence_count, 2);
        assert_eq!(group.stored_occurrence_count, 2);
        assert_eq!(index.group_event_ids(group_id).unwrap().len(), 2);
        assert_eq!(index.stats().observed_occurrence_count, 2);
        assert_eq!(index.stats().stored_occurrence_count, 2);
    }

    #[test]
    fn capacity_drop_is_atomic_and_counts_existing_and_ungrouped_events() {
        let limits = ProblemIndexLimits {
            max_events: 2,
            max_observation_refs: 1,
            max_groups: 1,
            ..ProblemIndexLimits::default()
        };
        let mut index = ProblemIndex::with_limits(limits).unwrap();
        let existing = key(b"existing");
        let one_ref = [candidate(
            1,
            ObservationRole::Primary,
            EvidencePriority::MinimumGrammar,
        )];
        let two_refs = [
            candidate(
                2,
                ObservationRole::Primary,
                EvidencePriority::MinimumGrammar,
            ),
            candidate(
                3,
                ObservationRole::ExceptionType,
                EvidencePriority::MinimumGrammar,
            ),
        ];

        let group_id = match index.append(draft(1), existing, &one_ref).unwrap() {
            AppendOutcome::Stored { group_id, .. } => group_id,
            outcome => panic!("unexpected append: {outcome:?}"),
        };
        assert_eq!(
            index.append(draft(2), existing, &two_refs).unwrap(),
            AppendOutcome::Dropped {
                group_id: Some(group_id),
                reason: AppendDropReason::ObservationRefLimit,
            }
        );
        assert_eq!(
            index.append(draft(4), key(b"new"), &[]).unwrap(),
            AppendOutcome::Dropped {
                group_id: None,
                reason: AppendDropReason::GroupLimit,
            }
        );

        let stats = index.stats();
        assert_eq!(stats.observed_occurrence_count, 3);
        assert_eq!(stats.stored_occurrence_count, 1);
        assert_eq!(stats.dropped_occurrence_count, 2);
        assert_eq!(stats.stored_group_count, 1);
        assert_eq!(stats.ungrouped_dropped_occurrence_count, 1);
        assert_eq!(stats.revision, 3);
        assert!(stats.limited);
        assert!(stats.charged_bytes >= stats.retained_heap_bytes);
        assert_eq!(index.events_len(), 1);
        assert_eq!(index.observation_refs_len(), 1);
        assert_eq!(index.group_event_ids(group_id).unwrap().len(), 1);
        let group = index.group(group_id).unwrap();
        assert_eq!(group.observed_occurrence_count, 2);
        assert_eq!(group.stored_occurrence_count, 1);
        assert_eq!(group.dropped_occurrence_count, 1);
        assert_eq!(
            group.representative_stored_event_id,
            Some(ProblemEventId(0))
        );
    }

    #[test]
    fn group_first_and_last_follow_source_lines_despite_clock_rollback() {
        let mut index = ProblemIndex::new();
        let group_key = key(b"source-order");
        let mut middle = draft(100);
        middle.anchor_timestamp = timestamp(10);
        let mut first = draft(50);
        first.anchor_timestamp = timestamp(20);
        let mut last = draft(150);
        last.anchor_timestamp = timestamp(5);

        let first_stored = match index.append(middle, group_key, &[]).unwrap() {
            AppendOutcome::Stored {
                event_id, group_id, ..
            } => (event_id, group_id),
            outcome => panic!("unexpected append: {outcome:?}"),
        };
        let second_id = match index.append(first, group_key, &[]).unwrap() {
            AppendOutcome::Stored { event_id, .. } => event_id,
            outcome => panic!("unexpected append: {outcome:?}"),
        };
        let third_id = match index.append(last, group_key, &[]).unwrap() {
            AppendOutcome::Stored { event_id, .. } => event_id,
            outcome => panic!("unexpected append: {outcome:?}"),
        };

        let group = index.group(first_stored.1).unwrap();
        assert_eq!(group.first_observed_line, 50);
        assert_eq!(group.first_observed_timestamp, timestamp(20));
        assert_eq!(group.last_observed_line, 150);
        assert_eq!(group.last_observed_timestamp, timestamp(5));
        assert_eq!(group.representative_stored_event_id, Some(first_stored.0));
        assert_eq!(group.first_stored_event_id, Some(second_id));
        assert_eq!(group.last_stored_event_id, Some(third_id));
        assert_eq!(
            index.group_event_ids(first_stored.1).unwrap(),
            &[first_stored.0, second_id, third_id]
        );
    }

    #[test]
    fn adopted_observations_are_deduplicated_capped_and_materialized_to_eight() {
        let mut index = ProblemIndex::new();
        let mut observations = (0..4_100)
            .map(|line| {
                candidate(
                    line,
                    ObservationRole::Primary,
                    EvidencePriority::MinimumGrammar,
                )
            })
            .collect::<Vec<_>>();
        observations.push(candidate(
            0,
            ObservationRole::Primary,
            EvidencePriority::Supporting,
        ));

        let event_id = match index
            .append(draft(0), key(b"observation-cap"), &observations)
            .unwrap()
        {
            AppendOutcome::Stored { event_id, .. } => event_id,
            outcome => panic!("unexpected append: {outcome:?}"),
        };
        let event = index.event(event_id).unwrap();
        assert_eq!(event.observation_total(), 4_096);
        assert_eq!(event.observation_len(), 8);
        assert!(event
            .boundary()
            .contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED));
        assert!(event
            .boundary()
            .contains(BoundaryFlags::OBSERVATION_REFS_TRUNCATED));
        assert_eq!(index.event_observations(event_id).unwrap().len(), 8);
    }

    #[test]
    fn group_key_includes_kind_version_and_both_quality_dimensions() {
        let process = ProcessFingerprintKey::new(Some("com.example.app"));
        let fingerprint = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::FullStack,
            IdentityQuality::KnownProcess,
            &process,
        )
        .finish();
        let baseline = GroupKey::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::FullStack,
            IdentityQuality::KnownProcess,
            fingerprint,
        );
        assert_ne!(
            baseline,
            GroupKey::new(
                ProblemKind::Anr,
                1,
                SignatureQuality::FullStack,
                IdentityQuality::KnownProcess,
                fingerprint,
            )
        );
        assert_ne!(
            baseline,
            GroupKey::new(
                ProblemKind::JavaCrash,
                2,
                SignatureQuality::FullStack,
                IdentityQuality::KnownProcess,
                fingerprint,
            )
        );
        assert_ne!(
            baseline,
            GroupKey::new(
                ProblemKind::JavaCrash,
                1,
                SignatureQuality::TypeOnly,
                IdentityQuality::KnownProcess,
                fingerprint,
            )
        );
        assert_ne!(
            baseline,
            GroupKey::new(
                ProblemKind::JavaCrash,
                1,
                SignatureQuality::FullStack,
                IdentityQuality::UnknownProcess,
                fingerprint,
            )
        );
    }

    #[test]
    fn group_display_summaries_are_fixed_bounded_and_do_not_change_identity() {
        let process = "进程".repeat(40);
        let signature = "java.lang.IllegalStateException".repeat(8);
        let display = ProblemDisplaySummary::from_normalized(Some(&process), Some(&signature));
        assert!(display.process.truncated());
        assert!(display.signature.truncated());
        assert!(display.process.as_str().len() <= MAX_PROBLEM_PROCESS_SUMMARY_BYTES);
        assert!(display.signature.as_str().len() <= MAX_PROBLEM_SIGNATURE_SUMMARY_BYTES);
        assert!(std::str::from_utf8(display.process.as_str().as_bytes()).is_ok());

        let mut index = ProblemIndex::new();
        let group_key = key(b"bounded-summary");
        let first = index
            .append_with_summary(draft(1), group_key, display, &[])
            .unwrap();
        let AppendOutcome::Stored { group_id, .. } = first else {
            panic!("summary fixture must be stored");
        };
        index
            .append_with_summary(
                draft(2),
                group_key,
                ProblemDisplaySummary::from_normalized(
                    Some("different-presentation"),
                    Some("different-signature"),
                ),
                &[],
            )
            .unwrap();

        let group = index.group(group_id).unwrap();
        assert_eq!(group.process_summary, display.process);
        assert_eq!(group.signature_summary, display.signature);
        assert_eq!(group.observed_occurrence_count, 2);
        assert!(size_of::<ProblemGroupSummary>() <= 256);
    }

    #[test]
    fn snapshots_freeze_group_order_and_occurrence_prefix() {
        let mut index = ProblemIndex::new();
        let (_, group_10) = stored(index.append(draft(10), key(b"10"), &[]).unwrap());
        let (first_20, group_20) = stored(index.append(draft(20), key(b"20"), &[]).unwrap());
        let (_, group_30) = stored(index.append(draft(30), key(b"30"), &[]).unwrap());
        let now = Instant::now();
        let groups_snapshot = index
            .create_group_snapshot_at(&GroupQuery::default(), now)
            .unwrap();
        let occurrence_snapshot = index.create_occurrence_snapshot_at(group_20, now).unwrap();

        stored(index.append(draft(40), key(b"40"), &[]).unwrap());
        stored(index.append(draft(50), key(b"20"), &[]).unwrap());

        let first_page = index
            .group_snapshot_page_at(
                groups_snapshot,
                PageSpec::new(0, 2).unwrap(),
                now + Duration::from_secs(1),
            )
            .unwrap();
        let second_page = index
            .group_snapshot_page_at(
                groups_snapshot,
                PageSpec::new(2, 2).unwrap(),
                now + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(
            first_page
                .items
                .iter()
                .chain(&second_page.items)
                .map(|group| group.id)
                .collect::<Vec<_>>(),
            vec![group_30, group_20, group_10]
        );
        assert_eq!(
            index
                .occurrence_snapshot_page_at(
                    occurrence_snapshot,
                    PageSpec::new(0, 200).unwrap(),
                    now + Duration::from_secs(3),
                )
                .unwrap()
                .items,
            vec![first_20]
        );
    }

    #[test]
    fn batched_group_records_sort_outside_the_index_and_install_a_frozen_snapshot() {
        let mut index = ProblemIndex::new();
        for line in [10, 40, 20, 30, 50] {
            stored(
                index
                    .append(draft(line), key(format!("group-{line}").as_bytes()), &[])
                    .unwrap(),
            );
        }
        let query = GroupQuery {
            kind: None,
            sort: GroupSort::LastOccurrenceDesc,
        };
        let capture = index.group_snapshot_capture();
        let mut records = Vec::new();
        for offset in (0..capture.group_count).step_by(2) {
            records.extend(
                index
                    .group_sort_records(&query, capture, offset, 2)
                    .unwrap(),
            );
        }
        records.sort_by(|left, right| GroupSortRecord::compare(left, right, query.sort));
        let ids = records.into_iter().map(|record| record.id).collect();
        let snapshot = index
            .install_group_snapshot_ids(ids, capture.revision, query)
            .unwrap();
        let page = index
            .group_snapshot_page(snapshot, PageSpec::new(0, 10).unwrap())
            .unwrap();

        assert_eq!(page.revision, capture.revision);
        assert_eq!(
            page.items
                .iter()
                .map(|group| group.last_observed_line)
                .collect::<Vec<_>>(),
            vec![50, 40, 30, 20, 10]
        );
    }

    #[test]
    fn frozen_snapshots_reject_a_different_query_signature() {
        let mut index = ProblemIndex::new();
        let (_, first_group) = stored(index.append(draft(10), key(b"first"), &[]).unwrap());
        let (_, second_group) = stored(index.append(draft(20), key(b"second"), &[]).unwrap());
        let group_query = GroupQuery {
            kind: None,
            sort: GroupSort::LastOccurrenceDesc,
        };
        let group_snapshot = index.create_group_snapshot(&group_query).unwrap();
        assert_eq!(
            index.group_snapshot_page_for_query(
                group_snapshot,
                PageSpec::new(0, 10).unwrap(),
                GroupQuery {
                    sort: GroupSort::ObservedCountDesc,
                    ..group_query
                },
            ),
            Err(SnapshotError::QueryMismatch)
        );

        let occurrence_snapshot = index.create_occurrence_snapshot(first_group).unwrap();
        assert_eq!(
            index.occurrence_snapshot_page_for_group(
                occurrence_snapshot,
                PageSpec::new(0, 10).unwrap(),
                second_group,
            ),
            Err(SnapshotError::QueryMismatch)
        );
    }

    #[test]
    fn snapshot_ttl_lru_release_and_id_budget_have_explicit_outcomes() {
        let limits = ProblemIndexLimits {
            max_snapshots: 2,
            snapshot_ttl: Duration::from_secs(5),
            max_snapshot_id_bytes: 8,
            ..ProblemIndexLimits::default()
        };
        let mut index = ProblemIndex::with_limits(limits).unwrap();
        let (_, first_group) = stored(index.append(draft(1), key(b"one"), &[]).unwrap());
        stored(index.append(draft(2), key(b"two"), &[]).unwrap());
        let now = Instant::now();

        let expiring = index
            .create_occurrence_snapshot_at(first_group, now)
            .unwrap();
        assert_eq!(
            index
                .occurrence_snapshot_page_at(
                    expiring,
                    PageSpec::new(0, 1).unwrap(),
                    now + Duration::from_secs(5),
                )
                .unwrap_err(),
            SnapshotError::Expired
        );

        let group_snapshot = index
            .create_group_snapshot_at(&GroupQuery::default(), now + Duration::from_secs(6))
            .unwrap();
        let old_occurrence = index
            .create_occurrence_snapshot_at(first_group, now + Duration::from_secs(7))
            .unwrap();
        index
            .group_snapshot_page_at(
                group_snapshot,
                PageSpec::new(0, 1).unwrap(),
                now + Duration::from_secs(8),
            )
            .unwrap();
        let releasable = index
            .create_occurrence_snapshot_at(first_group, now + Duration::from_secs(9))
            .unwrap();
        assert_eq!(
            index
                .occurrence_snapshot_page_at(
                    old_occurrence,
                    PageSpec::new(0, 1).unwrap(),
                    now + Duration::from_secs(9),
                )
                .unwrap_err(),
            SnapshotError::Evicted
        );
        assert!(index.release_snapshot(releasable));
        assert!(!index.release_snapshot(releasable));
        assert_eq!(
            index
                .occurrence_snapshot_page_at(
                    releasable,
                    PageSpec::new(0, 1).unwrap(),
                    now + Duration::from_secs(9),
                )
                .unwrap_err(),
            SnapshotError::Released
        );

        stored(index.append(draft(3), key(b"three"), &[]).unwrap());
        assert_eq!(
            index
                .create_group_snapshot_at(&GroupQuery::default(), now + Duration::from_secs(10),)
                .unwrap_err(),
            SnapshotError::IdVectorLimit
        );
        assert!(index.snapshot_count() <= 2);
        assert!(index.snapshot_id_bytes() <= 8);
    }

    #[test]
    fn reset_clears_index_and_marks_old_snapshots_invalid() {
        let mut index = ProblemIndex::new();
        let (_, group_id) = stored(index.append(draft(1), key(b"reset"), &[]).unwrap());
        let snapshot = index.create_occurrence_snapshot(group_id).unwrap();

        index.reset();

        assert_eq!(index.stats(), ProblemStats::default());
        assert_eq!(index.events_len(), 0);
        assert_eq!(index.observation_refs_len(), 0);
        assert_eq!(index.snapshot_count(), 0);
        assert!(index.event(ProblemEventId(0)).is_none());
        assert_eq!(
            index
                .occurrence_snapshot_page(snapshot, PageSpec::new(0, 1).unwrap())
                .unwrap_err(),
            SnapshotError::Reset
        );
    }

    #[test]
    fn unified_budget_prevents_structural_caps_from_being_combined_into_an_oversized_heap() {
        let structural_maximum = IndexMemoryShape {
            events_capacity: DEFAULT_MAX_EVENTS,
            observation_refs_capacity: DEFAULT_MAX_OBSERVATION_REFS,
            groups_capacity: DEFAULT_MAX_GROUPS,
            group_membership_capacity: DEFAULT_MAX_EVENTS,
            group_membership_allocations: DEFAULT_MAX_GROUPS,
            group_lookup_capacity: DEFAULT_MAX_GROUPS,
            snapshots_capacity: DEFAULT_MAX_SNAPSHOTS,
            snapshot_id_capacity: DEFAULT_MAX_SNAPSHOT_ID_BYTES,
            snapshot_id_allocations: DEFAULT_MAX_SNAPSHOTS,
        };
        let usage = index_memory_usage(structural_maximum).unwrap();

        assert!(usage.charged_bytes > crate::problems::DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES);
    }

    #[test]
    fn event_and_high_distinct_group_storm_drops_one_whole_occurrence_at_budget_pressure() {
        let budget = ProblemMemoryBudget::with_limit_bytes(16 * 1024).unwrap();
        let mut index =
            ProblemIndex::with_limits_and_budget(ProblemIndexLimits::default(), budget.clone())
                .unwrap();
        let mut memory_drop = None;

        for line in 0..10_000 {
            let before_events = index.events_len();
            let before_refs = index.observation_refs_len();
            let outcome = index
                .append(draft(line), key(format!("distinct-{line}").as_bytes()), &[])
                .unwrap();
            if let AppendOutcome::Dropped {
                reason: AppendDropReason::MemoryBudget,
                ..
            } = outcome
            {
                assert_eq!(index.events_len(), before_events);
                assert_eq!(index.observation_refs_len(), before_refs);
                memory_drop = Some(outcome);
                break;
            }
        }

        assert!(memory_drop.is_some());
        let stats = index.stats();
        assert_eq!(
            stats.observed_occurrence_count,
            stats
                .stored_occurrence_count
                .saturating_add(stats.dropped_occurrence_count)
        );
        assert!(stats.memory_limited);
        assert!(stats.limited);
        assert!(stats.charged_bytes <= budget.stats().limit_bytes);
        assert!(stats.retained_heap_bytes <= stats.charged_bytes);

        index.reset();
        assert_eq!(index.stats(), ProblemStats::default());
        assert_eq!(budget.stats().charged_bytes, 0);
    }

    #[test]
    fn repeated_snapshots_remain_inside_the_same_budget_and_reset_reclaims_them() {
        let budget = ProblemMemoryBudget::with_limit_bytes(64 * 1024).unwrap();
        let mut index =
            ProblemIndex::with_limits_and_budget(ProblemIndexLimits::default(), budget.clone())
                .unwrap();
        for line in 0..20 {
            stored(
                index
                    .append(draft(line), key(format!("snapshot-{line}").as_bytes()), &[])
                    .unwrap(),
            );
        }
        for _ in 0..32 {
            index.create_group_snapshot(&GroupQuery::default()).unwrap();
        }

        assert!(index.snapshot_count() <= DEFAULT_MAX_SNAPSHOTS);
        assert!(index.stats().charged_bytes <= budget.stats().limit_bytes);
        assert!(index.stats().retained_heap_bytes <= index.stats().charged_bytes);

        index.reset();
        assert_eq!(budget.stats().charged_bytes, 0);
        assert_eq!(budget.stats().retained_heap_bytes, 0);
    }
}
