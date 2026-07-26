use super::budget::{
    btree_map_usage, vec_deque_usage, vec_usage, ProblemMemoryAccount, ProblemMemoryBudget,
    ProblemMemoryBudgetError, ProblemMemoryUsage,
};
use std::collections::{BTreeMap, VecDeque};

pub const MAX_RECENT_OBSERVATIONS: usize = 16_384;
pub const MAX_RECENT_OBSERVATION_BYTES: usize = 256 * 1024;
pub const MAX_PROVISIONAL_OCCURRENCES: usize = 4_096;
pub const MAX_PROVISIONAL_BYTES: usize = 4 * 1024 * 1024;

/// Payloads retained by the correlation stores must be compact metadata. The
/// reported logical size must include every retained byte and must not change
/// while the payload is stored. Raw log text belongs in the source file, not in
/// a correlation payload.
pub trait CompactCorrelationPayload {
    fn logical_bytes(&self) -> u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationLimitsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentObservationLimits {
    max_observations: usize,
    max_logical_bytes: usize,
}

impl RecentObservationLimits {
    pub fn new(
        max_observations: usize,
        max_logical_bytes: usize,
    ) -> Result<Self, CorrelationLimitsError> {
        if max_observations == 0 || max_logical_bytes == 0 {
            return Err(CorrelationLimitsError);
        }
        Ok(Self {
            max_observations,
            max_logical_bytes,
        })
    }

    pub fn max_observations(self) -> usize {
        self.max_observations
    }

    pub fn max_logical_bytes(self) -> usize {
        self.max_logical_bytes
    }
}

impl Default for RecentObservationLimits {
    fn default() -> Self {
        Self {
            max_observations: MAX_RECENT_OBSERVATIONS,
            max_logical_bytes: MAX_RECENT_OBSERVATION_BYTES,
        }
    }
}

#[derive(Debug)]
pub struct RecentObservation<P> {
    sequence: u64,
    expiry_line: u64,
    logical_bytes: u32,
    payload: P,
}

impl<P> RecentObservation<P> {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn expiry_line(&self) -> u64 {
        self.expiry_line
    }

    pub fn logical_bytes(&self) -> u32 {
        self.logical_bytes
    }

    pub fn payload(&self) -> &P {
        &self.payload
    }

    pub fn payload_mut(&mut self) -> &mut P {
        &mut self.payload
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentObservationStats {
    pub retained_observation_count: usize,
    pub retained_logical_bytes: usize,
    pub dropped_recent_observation_count: u64,
    pub correlation_limited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentInsertOutcome {
    pub sequence: u64,
    pub naturally_expired: usize,
    pub forcibly_evicted: usize,
    pub retained: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationSequenceExhausted;

#[derive(Debug)]
pub struct RecentObservationStore<P> {
    limits: RecentObservationLimits,
    observations: VecDeque<RecentObservation<P>>,
    retained_logical_bytes: usize,
    next_sequence: u64,
    dropped_recent_observation_count: u64,
    correlation_limited: bool,
    memory: ProblemMemoryAccount,
}

impl<P: CompactCorrelationPayload> RecentObservationStore<P> {
    pub fn new() -> Self {
        Self::with_limits(RecentObservationLimits::default())
    }

    pub fn with_limits(limits: RecentObservationLimits) -> Self {
        Self::with_limits_and_budget(limits, ProblemMemoryBudget::new())
    }

    pub(crate) fn with_limits_and_budget(
        limits: RecentObservationLimits,
        budget: ProblemMemoryBudget,
    ) -> Self {
        let memory = budget.account();
        Self {
            limits,
            observations: VecDeque::new(),
            retained_logical_bytes: 0,
            next_sequence: 0,
            dropped_recent_observation_count: 0,
            correlation_limited: false,
            memory,
        }
    }

    pub fn insert(
        &mut self,
        watermark: u64,
        expiry_line: u64,
        payload: P,
    ) -> Result<RecentInsertOutcome, CorrelationSequenceExhausted> {
        let naturally_expired = self.advance_watermark(watermark);
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(CorrelationSequenceExhausted)?;
        let logical_bytes = payload.logical_bytes();
        let mut forcibly_evicted = 0;
        while self.observations.len() >= self.limits.max_observations
            || logical_bytes as usize > self.limits.max_logical_bytes - self.retained_logical_bytes
        {
            let Some(evicted) = self.observations.pop_front() else {
                self.dropped_recent_observation_count =
                    self.dropped_recent_observation_count.saturating_add(1);
                self.correlation_limited = true;
                return Ok(RecentInsertOutcome {
                    sequence,
                    naturally_expired,
                    forcibly_evicted: forcibly_evicted + 1,
                    retained: false,
                });
            };
            self.retained_logical_bytes -= evicted.logical_bytes as usize;
            self.dropped_recent_observation_count =
                self.dropped_recent_observation_count.saturating_add(1);
            self.correlation_limited = true;
            forcibly_evicted += 1;
        }

        loop {
            let projected_capacity =
                projected_deque_capacity(self.observations.len(), self.observations.capacity(), 1);
            let projected_logical = self
                .retained_logical_bytes
                .checked_add(logical_bytes as usize)
                .ok_or(CorrelationSequenceExhausted)?;
            let projection = recent_memory_usage::<P>(projected_capacity, projected_logical)
                .map_err(|_| CorrelationSequenceExhausted)?;
            if self.memory.try_set_usage(projection).is_ok() {
                break;
            }
            let Some(evicted) = self.observations.pop_front() else {
                self.dropped_recent_observation_count =
                    self.dropped_recent_observation_count.saturating_add(1);
                self.correlation_limited = true;
                self.settle_memory();
                return Ok(RecentInsertOutcome {
                    sequence,
                    naturally_expired,
                    forcibly_evicted: forcibly_evicted.saturating_add(1),
                    retained: false,
                });
            };
            self.retained_logical_bytes -= evicted.logical_bytes as usize;
            self.dropped_recent_observation_count =
                self.dropped_recent_observation_count.saturating_add(1);
            self.correlation_limited = true;
            forcibly_evicted = forcibly_evicted.saturating_add(1);
            self.settle_memory();
        }
        if self.observations.try_reserve_exact(1).is_err() {
            self.dropped_recent_observation_count =
                self.dropped_recent_observation_count.saturating_add(1);
            self.correlation_limited = true;
            self.settle_memory();
            return Ok(RecentInsertOutcome {
                sequence,
                naturally_expired,
                forcibly_evicted: forcibly_evicted.saturating_add(1),
                retained: false,
            });
        }
        self.retained_logical_bytes += logical_bytes as usize;
        self.observations.push_back(RecentObservation {
            sequence,
            expiry_line,
            logical_bytes,
            payload,
        });
        self.settle_memory();
        Ok(RecentInsertOutcome {
            sequence,
            naturally_expired,
            forcibly_evicted,
            retained: true,
        })
    }

    pub fn advance_watermark(&mut self, watermark: u64) -> usize {
        let mut expired = 0;
        while self
            .observations
            .front()
            .is_some_and(|observation| watermark > observation.expiry_line)
        {
            let observation = self
                .observations
                .pop_front()
                .expect("the recent-observation head was present");
            self.retained_logical_bytes -= observation.logical_bytes as usize;
            expired += 1;
        }
        self.settle_memory();
        expired
    }

    pub fn iter(&self) -> impl Iterator<Item = &RecentObservation<P>> {
        self.observations.iter()
    }

    /// Mutates compact claim metadata in place. The payload's reported logical
    /// size must remain unchanged while retained.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut RecentObservation<P>> {
        self.observations.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.observations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }

    pub fn next_expiry_line(&self) -> Option<u64> {
        self.observations
            .front()
            .map(|observation| observation.expiry_line)
    }

    /// Releases observations after input finish while preserving session-level
    /// evidence-loss statistics for the query UI.
    pub fn finish(&mut self) -> usize {
        let discarded = self.observations.len();
        self.observations = VecDeque::new();
        self.retained_logical_bytes = 0;
        self.memory.release();
        discarded
    }

    pub fn reset(&mut self) {
        self.observations = VecDeque::new();
        self.retained_logical_bytes = 0;
        self.next_sequence = 0;
        self.dropped_recent_observation_count = 0;
        self.correlation_limited = false;
        self.memory.release();
    }

    pub fn stats(&self) -> RecentObservationStats {
        RecentObservationStats {
            retained_observation_count: self.observations.len(),
            retained_logical_bytes: self.retained_logical_bytes,
            dropped_recent_observation_count: self.dropped_recent_observation_count,
            correlation_limited: self.correlation_limited,
        }
    }

    fn settle_memory(&mut self) {
        let Ok(usage) =
            recent_memory_usage::<P>(self.observations.capacity(), self.retained_logical_bytes)
        else {
            return;
        };
        if usage.charged_bytes <= self.memory.usage().charged_bytes {
            self.memory.settle_precharged(usage);
        } else {
            let _ = self.memory.try_set_usage(usage);
        }
    }
}

impl<P: CompactCorrelationPayload> Default for RecentObservationStore<P> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionalLimits {
    max_occurrences: usize,
    max_logical_bytes: usize,
}

impl ProvisionalLimits {
    pub fn new(
        max_occurrences: usize,
        max_logical_bytes: usize,
    ) -> Result<Self, CorrelationLimitsError> {
        if max_occurrences == 0 || max_logical_bytes == 0 {
            return Err(CorrelationLimitsError);
        }
        Ok(Self {
            max_occurrences,
            max_logical_bytes,
        })
    }

    pub fn max_occurrences(self) -> usize {
        self.max_occurrences
    }

    pub fn max_logical_bytes(self) -> usize {
        self.max_logical_bytes
    }
}

impl Default for ProvisionalLimits {
    fn default() -> Self {
        Self {
            max_occurrences: MAX_PROVISIONAL_OCCURRENCES,
            max_logical_bytes: MAX_PROVISIONAL_BYTES,
        }
    }
}

#[derive(Debug)]
pub struct ProvisionalEntry<P> {
    sequence: u64,
    finalize_after_line: u64,
    logical_bytes: u32,
    payload: P,
}

impl<P> ProvisionalEntry<P> {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn finalize_after_line(&self) -> u64 {
        self.finalize_after_line
    }

    pub fn logical_bytes(&self) -> u32 {
        self.logical_bytes
    }

    pub fn payload(&self) -> &P {
        &self.payload
    }

    pub fn payload_mut(&mut self) -> &mut P {
        &mut self.payload
    }

    pub fn into_payload(self) -> P {
        self.payload
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionalFinalizeReason {
    Watermark,
    Capacity,
    Finish,
}

#[derive(Debug)]
pub struct FinalizedProvisional<P> {
    pub entry: ProvisionalEntry<P>,
    pub reason: ProvisionalFinalizeReason,
}

#[derive(Debug)]
pub struct ProvisionalInsertOutcome<P> {
    pub sequence: u64,
    pub finalized: Vec<FinalizedProvisional<P>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionalStats {
    pub provisional_occurrence_count: usize,
    pub retained_logical_bytes: usize,
    pub capacity_finalized_count: u64,
}

#[derive(Debug)]
pub struct ProvisionalStore<P> {
    limits: ProvisionalLimits,
    entries: BTreeMap<(u64, u64), ProvisionalEntry<P>>,
    retained_logical_bytes: usize,
    next_sequence: u64,
    capacity_finalized_count: u64,
    memory: ProblemMemoryAccount,
}

impl<P: CompactCorrelationPayload> ProvisionalStore<P> {
    pub fn new() -> Self {
        Self::with_limits(ProvisionalLimits::default())
    }

    pub fn with_limits(limits: ProvisionalLimits) -> Self {
        Self::with_limits_and_budget(limits, ProblemMemoryBudget::new())
    }

    pub(crate) fn with_limits_and_budget(
        limits: ProvisionalLimits,
        budget: ProblemMemoryBudget,
    ) -> Self {
        let memory = budget.account();
        Self {
            limits,
            entries: BTreeMap::new(),
            retained_logical_bytes: 0,
            next_sequence: 0,
            capacity_finalized_count: 0,
            memory,
        }
    }

    pub fn insert(
        &mut self,
        finalize_after_line: u64,
        payload: P,
    ) -> Result<ProvisionalInsertOutcome<P>, CorrelationSequenceExhausted> {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(CorrelationSequenceExhausted)?;
        let logical_bytes = payload.logical_bytes();
        let key = (finalize_after_line, sequence);
        let mut pending = Some(ProvisionalEntry {
            sequence,
            finalize_after_line,
            logical_bytes,
            payload,
        });
        let mut finalized = Vec::new();
        while self.entries.len() >= self.limits.max_occurrences
            || logical_bytes as usize > self.limits.max_logical_bytes - self.retained_logical_bytes
        {
            let entry = match self.entries.first_key_value() {
                Some((earliest_key, _)) if *earliest_key <= key => self
                    .pop_earliest()
                    .expect("the selected provisional entry must still exist"),
                _ => pending
                    .take()
                    .expect("the incoming provisional entry was not yet finalized"),
            };
            self.capacity_finalized_count = self.capacity_finalized_count.saturating_add(1);
            finalized.push(FinalizedProvisional {
                entry,
                reason: ProvisionalFinalizeReason::Capacity,
            });
            if pending.is_none() {
                break;
            }
        }
        while pending.is_some() {
            let projected_logical = self
                .retained_logical_bytes
                .checked_add(logical_bytes as usize)
                .ok_or(CorrelationSequenceExhausted)?;
            let projection =
                provisional_memory_usage::<P>(self.entries.len() + 1, projected_logical)
                    .map_err(|_| CorrelationSequenceExhausted)?;
            if self.memory.try_set_usage(projection).is_ok() {
                break;
            }
            let entry = match self.entries.first_key_value() {
                Some((earliest_key, _)) if *earliest_key <= key => self
                    .pop_earliest()
                    .expect("the selected provisional entry must still exist"),
                _ => pending
                    .take()
                    .expect("the incoming provisional entry was not yet finalized"),
            };
            self.capacity_finalized_count = self.capacity_finalized_count.saturating_add(1);
            finalized.push(FinalizedProvisional {
                entry,
                reason: ProvisionalFinalizeReason::Capacity,
            });
            if pending.is_none() {
                break;
            }
        }
        if let Some(entry) = pending {
            self.retained_logical_bytes += logical_bytes as usize;
            self.entries.insert(key, entry);
        }
        self.settle_memory();
        Ok(ProvisionalInsertOutcome {
            sequence,
            finalized,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProvisionalEntry<P>> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn next_finalize_after_line(&self) -> Option<u64> {
        self.entries
            .first_key_value()
            .map(|((finalize_after_line, _), _)| *finalize_after_line)
    }

    /// Mutates compact evidence in place without changing deadline ordering.
    ///
    /// `CompactCorrelationPayload::logical_bytes` is required to remain
    /// invariant while the item is retained.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ProvisionalEntry<P>> {
        self.entries.values_mut()
    }

    /// Removes one retained entry by its stable sequence number.
    ///
    /// Correlation uses this only after a delayed, one-to-one match has become
    /// deterministic. Keeping removal inside the store preserves exact logical
    /// byte accounting.
    pub fn remove(&mut self, sequence: u64) -> Option<ProvisionalEntry<P>> {
        let key = self
            .entries
            .iter()
            .find_map(|(key, entry)| (entry.sequence == sequence).then_some(*key))?;
        let entry = self
            .entries
            .remove(&key)
            .expect("the selected provisional entry was present");
        self.retained_logical_bytes -= entry.logical_bytes as usize;
        self.settle_memory();
        Some(entry)
    }

    /// Extends an entry's strict source-line finalization deadline while
    /// preserving its original deterministic sequence tie-breaker.
    ///
    /// Correlation can discover a later member of the same occurrence. That
    /// member may itself admit forward evidence, so retaining the old deadline
    /// would finalize the occurrence too early.
    pub fn extend_finalize_after_line(&mut self, sequence: u64, new_deadline: u64) -> bool {
        let Some(old_key) = self
            .entries
            .iter()
            .find_map(|(key, entry)| (entry.sequence == sequence).then_some(*key))
        else {
            return false;
        };
        if new_deadline <= old_key.0 {
            return true;
        }
        let mut entry = self
            .entries
            .remove(&old_key)
            .expect("the selected provisional entry was present");
        entry.finalize_after_line = new_deadline;
        self.entries.insert((new_deadline, sequence), entry);
        true
    }

    pub fn advance_watermark(&mut self, watermark: u64) -> Vec<FinalizedProvisional<P>> {
        let mut finalized = Vec::new();
        while self
            .entries
            .first_key_value()
            .is_some_and(|((finalize_after_line, _), _)| watermark > *finalize_after_line)
        {
            let entry = self
                .pop_earliest()
                .expect("the provisional entry selected by watermark was present");
            finalized.push(FinalizedProvisional {
                entry,
                reason: ProvisionalFinalizeReason::Watermark,
            });
        }
        self.settle_memory();
        finalized
    }

    pub fn finish(&mut self) -> Vec<FinalizedProvisional<P>> {
        let mut finalized = Vec::with_capacity(self.entries.len());
        while let Some(entry) = self.pop_earliest() {
            finalized.push(FinalizedProvisional {
                entry,
                reason: ProvisionalFinalizeReason::Finish,
            });
        }
        self.settle_memory();
        finalized
    }

    pub fn reset(&mut self) {
        self.entries = BTreeMap::new();
        self.retained_logical_bytes = 0;
        self.next_sequence = 0;
        self.capacity_finalized_count = 0;
        self.memory.release();
    }

    pub fn stats(&self) -> ProvisionalStats {
        ProvisionalStats {
            provisional_occurrence_count: self.entries.len(),
            retained_logical_bytes: self.retained_logical_bytes,
            capacity_finalized_count: self.capacity_finalized_count,
        }
    }

    fn pop_earliest(&mut self) -> Option<ProvisionalEntry<P>> {
        let key = self.entries.first_key_value().map(|(key, _)| *key)?;
        let entry = self
            .entries
            .remove(&key)
            .expect("the selected provisional entry must still exist");
        self.retained_logical_bytes -= entry.logical_bytes as usize;
        Some(entry)
    }

    fn settle_memory(&mut self) {
        let Ok(usage) =
            provisional_memory_usage::<P>(self.entries.len(), self.retained_logical_bytes)
        else {
            return;
        };
        if usage.charged_bytes <= self.memory.usage().charged_bytes {
            self.memory.settle_precharged(usage);
        } else {
            let _ = self.memory.try_set_usage(usage);
        }
    }
}

fn projected_deque_capacity(len: usize, capacity: usize, additional: usize) -> usize {
    let required = len.saturating_add(additional);
    if required <= capacity {
        capacity
    } else {
        required.checked_next_power_of_two().unwrap_or(usize::MAX)
    }
}

fn recent_memory_usage<P>(
    capacity: usize,
    logical_bytes: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    vec_deque_usage::<RecentObservation<P>>(capacity)?.checked_add(vec_usage::<u8>(logical_bytes)?)
}

fn provisional_memory_usage<P>(
    len: usize,
    logical_bytes: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    btree_map_usage::<(u64, u64), ProvisionalEntry<P>>(len)?
        .checked_add(vec_usage::<u8>(logical_bytes)?)
}

impl<P: CompactCorrelationPayload> Default for ProvisionalStore<P> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct TestPayload {
        id: u16,
        bytes: u32,
    }

    impl CompactCorrelationPayload for TestPayload {
        fn logical_bytes(&self) -> u32 {
            self.bytes
        }
    }

    fn payload(id: u16, bytes: u32) -> TestPayload {
        TestPayload { id, bytes }
    }

    #[test]
    fn recent_store_accepts_the_exact_count_and_byte_boundaries() {
        let limits = RecentObservationLimits::new(2, 10).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);

        let first = store.insert(1, 10, payload(1, 4)).unwrap();
        let second = store.insert(2, 10, payload(2, 6)).unwrap();

        assert_eq!(first.forcibly_evicted, 0);
        assert_eq!(second.forcibly_evicted, 0);
        assert_eq!(store.stats().retained_observation_count, 2);
        assert_eq!(store.stats().retained_logical_bytes, 10);
        assert_eq!(
            store
                .iter()
                .map(|item| item.payload().id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn recent_store_forcibly_evicts_oldest_at_count_plus_one() {
        let limits = RecentObservationLimits::new(2, 100).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);
        store.insert(1, 20, payload(1, 1)).unwrap();
        store.insert(2, 20, payload(2, 1)).unwrap();

        let outcome = store.insert(3, 20, payload(3, 1)).unwrap();

        assert_eq!(outcome.forcibly_evicted, 1);
        assert_eq!(
            store
                .iter()
                .map(|item| item.payload().id)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(store.stats().dropped_recent_observation_count, 1);
        assert!(store.stats().correlation_limited);
    }

    #[test]
    fn recent_store_forcibly_evicts_oldest_at_byte_limit_plus_one() {
        let limits = RecentObservationLimits::new(10, 10).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);
        store.insert(1, 20, payload(1, 5)).unwrap();

        let outcome = store.insert(2, 20, payload(2, 6)).unwrap();

        assert_eq!(outcome.forcibly_evicted, 1);
        assert_eq!(
            store
                .iter()
                .map(|item| item.payload().id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(store.stats().retained_logical_bytes, 6);
        assert_eq!(store.stats().dropped_recent_observation_count, 1);
    }

    #[test]
    fn an_individually_oversized_recent_observation_is_not_retained() {
        let limits = RecentObservationLimits::new(2, 10).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);

        let outcome = store.insert(1, 10, payload(1, 11)).unwrap();

        assert!(!outcome.retained);
        assert_eq!(outcome.forcibly_evicted, 1);
        assert!(store.is_empty());
        assert_eq!(store.stats().retained_logical_bytes, 0);
        assert_eq!(store.stats().dropped_recent_observation_count, 1);
        assert!(store.stats().correlation_limited);
    }

    #[test]
    fn recent_store_expires_the_fifo_head_before_insert_without_counting_a_drop() {
        let limits = RecentObservationLimits::new(2, 10).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);
        store.insert(1, 5, payload(1, 5)).unwrap();

        let outcome = store.insert(6, 10, payload(2, 5)).unwrap();

        assert_eq!(outcome.naturally_expired, 1);
        assert_eq!(outcome.forcibly_evicted, 0);
        assert_eq!(
            store
                .iter()
                .map(|item| item.payload().id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(store.stats().dropped_recent_observation_count, 0);
        assert!(!store.stats().correlation_limited);
    }

    #[test]
    fn recent_store_limit_state_is_sticky_until_reset() {
        let limits = RecentObservationLimits::new(1, 10).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);
        store.insert(1, 5, payload(1, 1)).unwrap();
        store.insert(2, 5, payload(2, 1)).unwrap();

        assert_eq!(store.advance_watermark(5), 0);
        assert_eq!(store.advance_watermark(6), 1);
        assert!(store.stats().correlation_limited);
        assert_eq!(store.stats().dropped_recent_observation_count, 1);

        store.reset();

        assert_eq!(
            store.stats(),
            RecentObservationStats {
                retained_observation_count: 0,
                retained_logical_bytes: 0,
                dropped_recent_observation_count: 0,
                correlation_limited: false,
            }
        );
        assert_eq!(store.insert(7, 8, payload(3, 1)).unwrap().sequence, 0);
    }

    #[test]
    fn recent_finish_discards_retained_items_but_preserves_limit_statistics() {
        let limits = RecentObservationLimits::new(1, 10).unwrap();
        let mut store = RecentObservationStore::with_limits(limits);
        store.insert(1, 10, payload(1, 1)).unwrap();
        store.insert(2, 10, payload(2, 1)).unwrap();

        assert_eq!(store.finish(), 1);

        assert!(store.is_empty());
        assert_eq!(store.stats().retained_logical_bytes, 0);
        assert_eq!(store.stats().dropped_recent_observation_count, 1);
        assert!(store.stats().correlation_limited);
    }

    #[test]
    fn provisional_store_accepts_the_exact_count_and_byte_boundaries() {
        let limits = ProvisionalLimits::new(2, 10).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);

        let first = store.insert(20, payload(1, 4)).unwrap();
        let second = store.insert(10, payload(2, 6)).unwrap();

        assert!(first.finalized.is_empty());
        assert!(second.finalized.is_empty());
        assert_eq!(
            store.stats(),
            ProvisionalStats {
                provisional_occurrence_count: 2,
                retained_logical_bytes: 10,
                capacity_finalized_count: 0,
            }
        );
    }

    #[test]
    fn provisional_store_finalizes_the_earliest_deadline_at_count_plus_one() {
        let limits = ProvisionalLimits::new(2, 100).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(20, payload(1, 1)).unwrap();
        store.insert(10, payload(2, 1)).unwrap();

        let outcome = store.insert(30, payload(3, 1)).unwrap();

        assert_eq!(outcome.finalized.len(), 1);
        assert_eq!(
            outcome.finalized[0].reason,
            ProvisionalFinalizeReason::Capacity
        );
        assert_eq!(outcome.finalized[0].entry.payload().id, 2);
        assert_eq!(
            store
                .iter()
                .map(|entry| entry.payload().id)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(store.stats().capacity_finalized_count, 1);
    }

    #[test]
    fn provisional_watermark_must_strictly_cross_the_finalize_line() {
        let limits = ProvisionalLimits::new(3, 100).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(10, payload(1, 1)).unwrap();
        store.insert(11, payload(2, 1)).unwrap();

        assert!(store.advance_watermark(10).is_empty());
        let finalized = store.advance_watermark(11);

        assert_eq!(finalized.len(), 1);
        assert_eq!(finalized[0].reason, ProvisionalFinalizeReason::Watermark);
        assert_eq!(finalized[0].entry.payload().id, 1);
        assert_eq!(
            store
                .iter()
                .map(|entry| entry.payload().id)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn provisional_finish_flushes_every_entry_in_deadline_then_sequence_order() {
        let limits = ProvisionalLimits::new(3, 100).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(20, payload(1, 1)).unwrap();
        store.insert(10, payload(2, 1)).unwrap();
        store.insert(10, payload(3, 1)).unwrap();

        let finalized = store.finish();

        assert_eq!(
            finalized
                .iter()
                .map(|item| (item.entry.payload().id, item.reason))
                .collect::<Vec<_>>(),
            vec![
                (2, ProvisionalFinalizeReason::Finish),
                (3, ProvisionalFinalizeReason::Finish),
                (1, ProvisionalFinalizeReason::Finish),
            ]
        );
        assert_eq!(
            store.stats(),
            ProvisionalStats {
                provisional_occurrence_count: 0,
                retained_logical_bytes: 0,
                capacity_finalized_count: 0,
            }
        );
    }

    #[test]
    fn provisional_reset_discards_pending_state_and_restarts_statistics() {
        let limits = ProvisionalLimits::new(1, 10).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(20, payload(1, 1)).unwrap();
        store.insert(30, payload(2, 1)).unwrap();
        assert_eq!(store.stats().capacity_finalized_count, 1);

        store.reset();

        assert_eq!(
            store.stats(),
            ProvisionalStats {
                provisional_occurrence_count: 0,
                retained_logical_bytes: 0,
                capacity_finalized_count: 0,
            }
        );
        assert_eq!(store.insert(40, payload(3, 1)).unwrap().sequence, 0);
    }

    #[test]
    fn provisional_payload_can_be_updated_without_rekeying_the_store() {
        let limits = ProvisionalLimits::new(2, 10).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(20, payload(1, 4)).unwrap();

        store.iter_mut().next().unwrap().payload_mut().id = 7;

        let finalized = store.finish();
        assert_eq!(finalized[0].entry.payload().id, 7);
        assert_eq!(finalized[0].entry.logical_bytes(), 4);
    }

    #[test]
    fn frozen_default_limits_match_the_correlation_contract() {
        let recent = RecentObservationLimits::default();
        assert_eq!(recent.max_observations(), 16_384);
        assert_eq!(recent.max_logical_bytes(), 256 * 1024);

        let provisional = ProvisionalLimits::default();
        assert_eq!(provisional.max_occurrences(), 4_096);
        assert_eq!(provisional.max_logical_bytes(), 4 * 1024 * 1024);
    }

    #[test]
    fn provisional_byte_pressure_uses_deadline_order_not_insertion_order() {
        let limits = ProvisionalLimits::new(10, 10).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(20, payload(1, 4)).unwrap();

        let outcome = store.insert(10, payload(2, 7)).unwrap();

        assert_eq!(outcome.finalized.len(), 1);
        assert_eq!(outcome.finalized[0].entry.payload().id, 2);
        assert_eq!(
            outcome.finalized[0].reason,
            ProvisionalFinalizeReason::Capacity
        );
        assert_eq!(
            store
                .iter()
                .map(|entry| entry.payload().id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(store.stats().retained_logical_bytes, 4);
    }

    #[test]
    fn an_individually_oversized_provisional_is_returned_for_capacity_finalization() {
        let limits = ProvisionalLimits::new(2, 10).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);

        let outcome = store.insert(10, payload(1, 11)).unwrap();

        assert!(store.is_empty());
        assert_eq!(outcome.finalized.len(), 1);
        assert_eq!(outcome.finalized[0].entry.payload().id, 1);
        assert_eq!(
            outcome.finalized[0].reason,
            ProvisionalFinalizeReason::Capacity
        );
        assert_eq!(store.stats().capacity_finalized_count, 1);
    }

    #[test]
    fn provisional_equal_deadlines_are_finalized_by_sequence() {
        let limits = ProvisionalLimits::new(2, 100).unwrap();
        let mut store = ProvisionalStore::with_limits(limits);
        store.insert(10, payload(1, 1)).unwrap();
        store.insert(10, payload(2, 1)).unwrap();

        let outcome = store.insert(10, payload(3, 1)).unwrap();

        assert_eq!(outcome.finalized[0].entry.sequence(), 0);
        assert_eq!(outcome.finalized[0].entry.payload().id, 1);
    }

    #[test]
    fn zero_capacity_is_rejected_for_both_stores() {
        assert_eq!(
            RecentObservationLimits::new(0, 1),
            Err(CorrelationLimitsError)
        );
        assert_eq!(
            RecentObservationLimits::new(1, 0),
            Err(CorrelationLimitsError)
        );
        assert_eq!(ProvisionalLimits::new(0, 1), Err(CorrelationLimitsError));
        assert_eq!(ProvisionalLimits::new(1, 0), Err(CorrelationLimitsError));
    }

    #[test]
    fn recent_and_provisional_stores_share_one_non_bypassable_budget() {
        let budget = ProblemMemoryBudget::with_limit_bytes(2 * 1024).unwrap();
        let provisional_limits = ProvisionalLimits::new(100, 1024 * 1024).unwrap();
        let recent_limits = RecentObservationLimits::new(100, 1024 * 1024).unwrap();
        let mut provisional =
            ProvisionalStore::with_limits_and_budget(provisional_limits, budget.clone());
        let mut recent =
            RecentObservationStore::with_limits_and_budget(recent_limits, budget.clone());

        for id in 0..100 {
            let _ = provisional
                .insert(u64::from(id) + 100, payload(id, 32))
                .unwrap();
        }
        for id in 0..100 {
            let _ = recent
                .insert(u64::from(id), u64::from(id) + 100, payload(id, 32))
                .unwrap();
        }

        assert!(
            provisional.stats().capacity_finalized_count > 0
                || recent.stats().dropped_recent_observation_count > 0
        );
        assert!(budget.stats().limited);
        assert!(budget.stats().charged_bytes <= budget.stats().limit_bytes);
        assert!(budget.stats().retained_heap_bytes <= budget.stats().charged_bytes);

        provisional.reset();
        recent.reset();
        budget.clear_limit_state();
        assert_eq!(budget.stats().charged_bytes, 0);
        assert_eq!(budget.stats().retained_heap_bytes, 0);
        assert!(!budget.stats().limited);
    }
}
