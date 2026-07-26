use super::facts::{ObservationCandidate, ObservationRef};
use super::fingerprint::ProblemFingerprint;
use super::model::{
    BoundaryFlags, IdentityQuality, PackedLogTimestamp, ProblemEvent, ProblemEventDraft,
    ProblemEventError, ProblemEventId, ProblemKind, SignatureQuality, MAX_ADOPTED_OBSERVATIONS,
    MAX_MATERIALIZED_OBSERVATIONS,
};
use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::time::{Duration, Instant};

const DEFAULT_MAX_EVENTS: usize = 1_000_000;
const DEFAULT_MAX_OBSERVATION_REFS: usize = 4_000_000;
const DEFAULT_MAX_GROUPS: usize = 100_000;
const DEFAULT_MAX_SNAPSHOTS: usize = 8;
const DEFAULT_SNAPSHOT_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MAX_SNAPSHOT_ID_BYTES: usize = 16 * 1024 * 1024;

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
        draft: ProblemEventDraft,
        event_id: ProblemEventId,
        event_ids: Vec<ProblemEventId>,
    ) -> Self {
        Self {
            summary: ProblemGroupSummary {
                id,
                key,
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
    pub stored_group_count: u32,
    pub ungrouped_dropped_occurrence_count: u64,
    pub revision: u64,
    pub limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendDropReason {
    EventLimit,
    ObservationRefLimit,
    GroupLimit,
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
    IdVectorLimit,
    Allocation,
    IdExhausted,
}

#[derive(Debug)]
enum SnapshotData {
    Groups {
        ids: Vec<GroupId>,
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
    group_lookup: HashMap<GroupKey, GroupId>,
    stats: ProblemStats,
    snapshots: Vec<QuerySnapshot>,
    retired_snapshots: VecDeque<(QuerySnapshotId, SnapshotError)>,
    snapshot_id_bytes: usize,
    next_snapshot_id: u64,
}

impl Default for ProblemIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl ProblemIndex {
    pub fn new() -> Self {
        Self {
            limits: ProblemIndexLimits::default(),
            events: Vec::new(),
            observation_refs: Vec::new(),
            groups: Vec::new(),
            group_lookup: HashMap::new(),
            stats: ProblemStats::default(),
            snapshots: Vec::new(),
            retired_snapshots: VecDeque::new(),
            snapshot_id_bytes: 0,
            next_snapshot_id: 1,
        }
    }

    pub fn with_limits(limits: ProblemIndexLimits) -> Result<Self, ProblemIndexError> {
        Ok(Self {
            limits: limits
                .validate()
                .map_err(ProblemIndexError::InvalidLimits)?,
            events: Vec::new(),
            observation_refs: Vec::new(),
            groups: Vec::new(),
            group_lookup: HashMap::new(),
            stats: ProblemStats::default(),
            snapshots: Vec::new(),
            retired_snapshots: VecDeque::new(),
            snapshot_id_bytes: 0,
            next_snapshot_id: 1,
        })
    }

    pub fn append(
        &mut self,
        mut draft: ProblemEventDraft,
        group_key: GroupKey,
        candidates: &[ObservationCandidate],
    ) -> Result<AppendOutcome, ProblemIndexError> {
        ProblemEvent::new(draft, 0, 0, 0, 0)?;
        if draft.kind != group_key.kind {
            return Err(ProblemIndexError::GroupKindMismatch {
                event_kind: draft.kind,
                group_kind: group_key.kind,
            });
        }

        let selected = select_observations(candidates)?;
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

        saturating_increment(&mut self.stats.observed_occurrence_count);
        bump_revision(&mut self.stats);

        let existing_group = self.group_lookup.get(&group_key).copied();
        if let Some(group_id) = existing_group {
            self.groups[group_id.0 as usize].record_observed(draft);
        }

        let drop_reason = if existing_group.is_none() && self.groups.len() >= self.limits.max_groups
        {
            Some(AppendDropReason::GroupLimit)
        } else if self.events.len() >= self.limits.max_events {
            Some(AppendDropReason::EventLimit)
        } else if self
            .observation_refs
            .len()
            .checked_add(selected.materialized.len())
            .is_none_or(|required| required > self.limits.max_observation_refs)
        {
            Some(AppendDropReason::ObservationRefLimit)
        } else {
            None
        };
        if let Some(reason) = drop_reason {
            return Ok(self.record_drop(existing_group, reason));
        }

        let mut new_group_membership = if existing_group.is_none() {
            let mut membership = Vec::new();
            if membership.try_reserve_exact(1).is_err() {
                return Ok(self.record_drop(existing_group, AppendDropReason::Allocation));
            }
            Some(membership)
        } else {
            None
        };
        if !self.reserve_append(existing_group, selected.materialized.len()) {
            return Ok(self.record_drop(existing_group, AppendDropReason::Allocation));
        }

        let group_id = existing_group.unwrap_or(GroupId(self.groups.len() as u32));
        let event_id = ProblemEventId(self.events.len() as u32);
        let observation_start = self.observation_refs.len() as u32;
        let observation_len = selected.materialized.len() as u8;
        let event = ProblemEvent::new(
            draft,
            group_id.0,
            observation_start,
            observation_len,
            selected.total,
        )?;

        self.observation_refs
            .extend(selected.materialized.iter().copied());
        self.events.push(event);

        let created_group = existing_group.is_none();
        if let Some(existing) = existing_group {
            self.groups[existing.0 as usize].record_stored(draft, event_id);
        } else {
            let mut event_ids = new_group_membership
                .take()
                .expect("new groups reserve their membership before committing");
            event_ids.push(event_id);
            self.groups.push(ProblemGroup::new(
                group_id, group_key, draft, event_id, event_ids,
            ));
            self.group_lookup.insert(group_key, group_id);
            self.stats.stored_group_count = self.groups.len() as u32;
        }
        saturating_increment(&mut self.stats.stored_occurrence_count);

        Ok(AppendOutcome::Stored {
            event_id,
            group_id,
            created_group,
        })
    }

    pub const fn stats(&self) -> ProblemStats {
        self.stats
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
        let mut ids = Vec::new();
        ids.try_reserve_exact(self.groups.len())
            .map_err(|_| SnapshotError::Allocation)?;
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
        self.install_snapshot(now, id_bytes, SnapshotData::Groups { ids })
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

    pub fn group_snapshot_page_at(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
        now: Instant,
    ) -> Result<GroupPage, SnapshotError> {
        let snapshot_index = self.active_snapshot(snapshot_id, SnapshotKind::Groups, now)?;
        let (revision, total, selected_ids) = {
            let snapshot = &self.snapshots[snapshot_index];
            let SnapshotData::Groups { ids } = &snapshot.data else {
                unreachable!("active_snapshot checked the snapshot kind");
            };
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

    pub fn occurrence_snapshot_page_at(
        &mut self,
        snapshot_id: QuerySnapshotId,
        page: PageSpec,
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
        self.events.clear();
        self.observation_refs.clear();
        self.groups.clear();
        self.group_lookup.clear();
        self.stats = ProblemStats::default();
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
        if id_bytes > self.limits.max_snapshot_id_bytes {
            return Err(SnapshotError::IdVectorLimit);
        }
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
        self.snapshots
            .try_reserve(1)
            .map_err(|_| SnapshotError::Allocation)?;
        let id = QuerySnapshotId(self.next_snapshot_id);
        self.next_snapshot_id = self
            .next_snapshot_id
            .checked_add(1)
            .ok_or(SnapshotError::IdExhausted)?;
        self.snapshot_id_bytes += id_bytes;
        self.snapshots.push(QuerySnapshot {
            id,
            revision: self.stats.revision,
            last_access: now,
            id_bytes,
            data,
        });
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
        if self.retired_snapshots.len() == MAX_RETIRED_SNAPSHOTS {
            self.retired_snapshots.pop_front();
        }
        self.retired_snapshots.push_back((removed.id, reason));
    }

    fn retired_reason(&self, id: QuerySnapshotId) -> Option<SnapshotError> {
        self.retired_snapshots
            .iter()
            .rev()
            .find_map(|(retired_id, reason)| (*retired_id == id).then_some(*reason))
    }

    fn reserve_append(&mut self, group: Option<GroupId>, observation_count: usize) -> bool {
        if self.events.try_reserve(1).is_err()
            || self
                .observation_refs
                .try_reserve(observation_count)
                .is_err()
        {
            return false;
        }
        if let Some(group_id) = group {
            self.groups[group_id.0 as usize]
                .event_ids
                .try_reserve(1)
                .is_ok()
        } else {
            self.groups.try_reserve(1).is_ok() && self.group_lookup.try_reserve(1).is_ok()
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
    materialized: Vec<ObservationRef>,
    total: u16,
    count_limited: bool,
}

fn select_observations(
    candidates: &[ObservationCandidate],
) -> Result<SelectedObservations, ProblemIndexError> {
    let adopted_limit = usize::from(MAX_ADOPTED_OBSERVATIONS);
    let reserve = candidates.len().min(adopted_limit);
    let mut by_key = HashMap::new();
    by_key
        .try_reserve(reserve)
        .map_err(|_| ProblemIndexError::PreparationAllocation)?;
    let mut unique = Vec::<(ObservationCandidate, usize)>::new();
    unique
        .try_reserve(reserve)
        .map_err(|_| ProblemIndexError::PreparationAllocation)?;
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
    let materialized = unique
        .iter()
        .take(usize::from(MAX_MATERIALIZED_OBSERVATIONS))
        .map(|(candidate, _)| candidate.reference)
        .collect();

    Ok(SelectedObservations {
        materialized,
        total: unique.len() as u16,
        count_limited,
    })
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

        assert_eq!(
            index.stats(),
            ProblemStats {
                observed_occurrence_count: 3,
                stored_occurrence_count: 1,
                dropped_occurrence_count: 2,
                stored_group_count: 1,
                ungrouped_dropped_occurrence_count: 1,
                revision: 3,
                limited: true,
            }
        );
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
}
