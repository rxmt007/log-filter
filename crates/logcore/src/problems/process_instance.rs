use super::budget::{
    aggregate_vec_usage, hash_map_usage, vec_deque_usage, ProblemMemoryAccount,
    ProblemMemoryBudget, ProblemMemoryBudgetError, ProblemMemoryUsage,
};
use super::{IdentityQuality, ProcessInstanceKey};
use std::collections::{HashMap, VecDeque};

pub const MAX_ACTIVE_PROCESS_INSTANCES: usize = 65_536;
pub const MAX_RECENT_TERMINATED_INSTANCES: usize = 4_096;
pub const MAX_TRACKED_PROCESS_NAME_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessIdentityError {
    ZeroPid,
    EmptyProcessName,
    ProcessNameTooLong,
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessIdentity<'a> {
    pid: u32,
    process_name: &'a str,
    uid: Option<u32>,
    user: Option<u32>,
}

impl<'a> ProcessIdentity<'a> {
    pub fn new(
        pid: u32,
        process_name: &'a str,
        uid: Option<u32>,
        user: Option<u32>,
    ) -> Result<Self, ProcessIdentityError> {
        if pid == 0 {
            return Err(ProcessIdentityError::ZeroPid);
        }
        if process_name.trim().is_empty() {
            return Err(ProcessIdentityError::EmptyProcessName);
        }
        if process_name.len() > MAX_TRACKED_PROCESS_NAME_BYTES {
            return Err(ProcessIdentityError::ProcessNameTooLong);
        }
        Ok(Self {
            pid,
            process_name,
            uid,
            user,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessEpochOrigin {
    ExplicitStart,
    Provisional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTrackerError {
    KeySpaceExhausted,
    EpochSpaceExhausted,
    MemoryBudget,
    Allocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTrackerLimitsError {
    InvalidActiveLimit,
    InvalidRecentLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessInstance {
    key: ProcessInstanceKey,
    epoch: u64,
    epoch_origin: ProcessEpochOrigin,
}

impl ProcessInstance {
    pub const fn key(self) -> ProcessInstanceKey {
        self.key
    }

    pub const fn epoch(self) -> u64 {
        self.epoch
    }

    pub const fn epoch_origin(self) -> ProcessEpochOrigin {
        self.epoch_origin
    }

    pub const fn identity_quality(self) -> IdentityQuality {
        IdentityQuality::KnownProcess
    }
}

#[derive(Debug)]
struct ActiveProcess {
    instance: ProcessInstance,
    pid: u32,
    process_name: String,
    uid: Option<u32>,
    user: Option<u32>,
    start_line: u32,
    last_touched_line: u32,
}

#[derive(Debug)]
struct TerminatedProcess {
    process: ActiveProcess,
    death_line: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct TrackedProcessInstance<'a> {
    active: &'a ActiveProcess,
}

impl<'a> TrackedProcessInstance<'a> {
    pub const fn instance(self) -> ProcessInstance {
        self.active.instance
    }

    pub fn process_name(self) -> &'a str {
        &self.active.process_name
    }

    pub const fn pid(self) -> u32 {
        self.active.pid
    }

    pub const fn uid(self) -> Option<u32> {
        self.active.uid
    }

    pub const fn user(self) -> Option<u32> {
        self.active.user
    }

    pub const fn start_line(self) -> u32 {
        self.active.start_line
    }

    pub const fn last_touched_line(self) -> u32 {
        self.active.last_touched_line
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TerminatedProcessInstance<'a> {
    terminated: &'a TerminatedProcess,
}

impl<'a> TerminatedProcessInstance<'a> {
    pub const fn instance(self) -> ProcessInstance {
        self.terminated.process.instance
    }

    pub fn process_name(self) -> &'a str {
        &self.terminated.process.process_name
    }

    pub const fn pid(self) -> u32 {
        self.terminated.process.pid
    }

    pub const fn uid(self) -> Option<u32> {
        self.terminated.process.uid
    }

    pub const fn user(self) -> Option<u32> {
        self.terminated.process.user
    }

    pub const fn death_line(self) -> u32 {
        self.terminated.death_line
    }
}

#[derive(Debug)]
pub struct ProcessInstanceTracker {
    active: HashMap<u32, ActiveProcess>,
    recent_terminated: VecDeque<TerminatedProcess>,
    process_name_capacity: usize,
    max_active_instances: usize,
    max_recent_terminated: usize,
    active_eviction_count: u64,
    recent_eviction_count: u64,
    budget_drop_count: u64,
    next_key: u32,
    next_epoch: u64,
    memory_budget: ProblemMemoryBudget,
    memory: ProblemMemoryAccount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessTrackerStats {
    active_instances: usize,
    recent_terminated: usize,
    max_active_instances: usize,
    max_recent_terminated: usize,
    active_eviction_count: u64,
    recent_eviction_count: u64,
    budget_drop_count: u64,
}

impl ProcessTrackerStats {
    pub const fn active_instances(self) -> usize {
        self.active_instances
    }

    pub const fn recent_terminated(self) -> usize {
        self.recent_terminated
    }

    pub const fn max_active_instances(self) -> usize {
        self.max_active_instances
    }

    pub const fn max_recent_terminated(self) -> usize {
        self.max_recent_terminated
    }

    pub const fn active_eviction_count(self) -> u64 {
        self.active_eviction_count
    }

    pub const fn recent_eviction_count(self) -> u64 {
        self.recent_eviction_count
    }

    pub const fn budget_drop_count(self) -> u64 {
        self.budget_drop_count
    }

    pub const fn identity_coverage_limited(self) -> bool {
        self.active_eviction_count != 0
            || self.recent_eviction_count != 0
            || self.budget_drop_count != 0
    }
}

impl Default for ProcessInstanceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessInstanceTracker {
    pub fn new() -> Self {
        Self::with_limits(
            MAX_ACTIVE_PROCESS_INSTANCES,
            MAX_RECENT_TERMINATED_INSTANCES,
        )
        .expect("documented process tracker limits must be valid")
    }

    pub fn with_limits(
        max_active_instances: usize,
        max_recent_terminated: usize,
    ) -> Result<Self, ProcessTrackerLimitsError> {
        Self::with_limits_and_budget(
            max_active_instances,
            max_recent_terminated,
            ProblemMemoryBudget::new(),
        )
    }

    pub(crate) fn with_limits_and_budget(
        max_active_instances: usize,
        max_recent_terminated: usize,
        memory_budget: ProblemMemoryBudget,
    ) -> Result<Self, ProcessTrackerLimitsError> {
        if !(1..=MAX_ACTIVE_PROCESS_INSTANCES).contains(&max_active_instances) {
            return Err(ProcessTrackerLimitsError::InvalidActiveLimit);
        }
        if !(1..=MAX_RECENT_TERMINATED_INSTANCES).contains(&max_recent_terminated) {
            return Err(ProcessTrackerLimitsError::InvalidRecentLimit);
        }
        let memory = memory_budget.account();
        Ok(Self {
            active: HashMap::new(),
            recent_terminated: VecDeque::new(),
            process_name_capacity: 0,
            max_active_instances,
            max_recent_terminated,
            active_eviction_count: 0,
            recent_eviction_count: 0,
            budget_drop_count: 0,
            next_key: 1,
            next_epoch: 1,
            memory_budget,
            memory,
        })
    }

    pub fn observe_identity(
        &mut self,
        line: u32,
        identity: ProcessIdentity<'_>,
    ) -> Result<ProcessInstance, ProcessTrackerError> {
        if let Some(active) = self
            .active
            .get_mut(&identity.pid)
            .filter(|active| active.matches(identity))
        {
            active.enrich(identity);
            active.last_touched_line = active.last_touched_line.max(line);
            return Ok(active.instance);
        }

        let (process_name, victim) = match self.prepare_active_insert(identity) {
            Ok(prepared) => prepared,
            Err(error) => return Err(self.record_retention_failure(error)),
        };
        let instance = match self.allocate_instance(ProcessEpochOrigin::Provisional) {
            Ok(instance) => instance,
            Err(error) => {
                self.settle_memory();
                return Err(error);
            }
        };
        self.commit_active_insert(
            identity.pid,
            ActiveProcess {
                instance,
                pid: identity.pid,
                process_name,
                uid: identity.uid,
                user: identity.user,
                start_line: line,
                last_touched_line: line,
            },
            victim,
        );
        Ok(instance)
    }

    pub fn observe_start(
        &mut self,
        line: u32,
        identity: ProcessIdentity<'_>,
    ) -> Result<ProcessInstance, ProcessTrackerError> {
        let (process_name, victim) = match self.prepare_active_insert(identity) {
            Ok(prepared) => prepared,
            Err(error) => return Err(self.record_retention_failure(error)),
        };
        let instance = match self.allocate_instance(ProcessEpochOrigin::ExplicitStart) {
            Ok(instance) => instance,
            Err(error) => {
                self.settle_memory();
                return Err(error);
            }
        };
        self.commit_active_insert(
            identity.pid,
            ActiveProcess {
                instance,
                pid: identity.pid,
                process_name,
                uid: identity.uid,
                user: identity.user,
                start_line: line,
                last_touched_line: line,
            },
            victim,
        );
        Ok(instance)
    }

    pub fn observe_death(
        &mut self,
        line: u32,
        identity: ProcessIdentity<'_>,
    ) -> Result<ProcessInstance, ProcessTrackerError> {
        let matching_active = self
            .active
            .get(&identity.pid)
            .is_some_and(|active| active.matches(identity));
        let prepared_name_bytes = (!matching_active).then_some(identity.process_name.len());
        if let Err(error) = self.prepare_death_retention(prepared_name_bytes) {
            return Err(self.record_retention_failure(error));
        }
        let prepared_name = if matching_active {
            None
        } else {
            match self.prepare_name(identity.process_name) {
                Ok(name) => Some(name),
                Err(error) => {
                    self.settle_memory();
                    return Err(self.record_retention_failure(error));
                }
            }
        };
        let new_instance = if matching_active {
            None
        } else {
            match self.allocate_instance(ProcessEpochOrigin::Provisional) {
                Ok(instance) => Some(instance),
                Err(error) => {
                    self.settle_memory();
                    return Err(error);
                }
            }
        };
        let removed_active = self.remove_active(identity.pid);
        let mut process = if matching_active {
            removed_active.expect("matching active process must still exist")
        } else {
            ActiveProcess {
                instance: new_instance.expect("a new death identity allocates an instance"),
                pid: identity.pid,
                process_name: prepared_name.expect("a new death identity prepares its name"),
                uid: identity.uid,
                user: identity.user,
                start_line: line,
                last_touched_line: line,
            }
        };
        process.enrich(identity);
        process.last_touched_line = process.last_touched_line.max(line);
        let instance = process.instance;
        if self.recent_terminated.len() == self.max_recent_terminated {
            if let Some(evicted) = self.recent_terminated.pop_front() {
                self.process_name_capacity = self
                    .process_name_capacity
                    .saturating_sub(evicted.process.process_name.capacity());
                self.recent_eviction_count = self.recent_eviction_count.saturating_add(1);
            }
        }
        self.process_name_capacity = self
            .process_name_capacity
            .saturating_add(process.process_name.capacity());
        self.recent_terminated.push_back(TerminatedProcess {
            process,
            death_line: line,
        });
        self.settle_memory();
        Ok(instance)
    }

    pub fn active_for_pid(&self, pid: u32) -> Option<TrackedProcessInstance<'_>> {
        self.active
            .get(&pid)
            .map(|active| TrackedProcessInstance { active })
    }

    pub fn active_matching(
        &self,
        identity: ProcessIdentity<'_>,
    ) -> Option<TrackedProcessInstance<'_>> {
        self.active
            .get(&identity.pid)
            .filter(|active| active.matches(identity))
            .map(|active| TrackedProcessInstance { active })
    }

    pub fn recent_terminated_matching(
        &self,
        process_name: &str,
        uid: Option<u32>,
        user: Option<u32>,
    ) -> Option<TerminatedProcessInstance<'_>> {
        let uid = uid?;
        self.recent_terminated
            .iter()
            .rev()
            .find(|terminated| {
                terminated.process.process_name.as_str() == process_name
                    && terminated.process.uid == Some(uid)
                    && optional_field_matches(terminated.process.user, user)
            })
            .map(|terminated| TerminatedProcessInstance { terminated })
    }

    pub fn stats(&self) -> ProcessTrackerStats {
        ProcessTrackerStats {
            active_instances: self.active.len(),
            recent_terminated: self.recent_terminated.len(),
            max_active_instances: self.max_active_instances,
            max_recent_terminated: self.max_recent_terminated,
            active_eviction_count: self.active_eviction_count,
            recent_eviction_count: self.recent_eviction_count,
            budget_drop_count: self.budget_drop_count,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.active = HashMap::new();
        self.recent_terminated = VecDeque::new();
        self.process_name_capacity = 0;
        self.active_eviction_count = 0;
        self.recent_eviction_count = 0;
        self.budget_drop_count = 0;
        self.next_key = 1;
        self.next_epoch = 1;
        self.memory.release();
        self.memory_budget.clear_limit_state();
    }

    fn prepare_active_insert(
        &mut self,
        identity: ProcessIdentity<'_>,
    ) -> Result<(String, Option<u32>), ProcessTrackerError> {
        let replacing = self.active.contains_key(&identity.pid);
        let victim = (!replacing && self.active.len() >= self.max_active_instances)
            .then(|| self.oldest_active_pid())
            .flatten();
        let map_additional = usize::from(!replacing && victim.is_none());
        let projected_active_capacity =
            projected_hash_capacity(self.active.len(), self.active.capacity(), map_additional)
                .ok_or(ProcessTrackerError::MemoryBudget)?;
        let preparation_name_capacity = self
            .process_name_capacity
            .checked_add(identity.process_name.len())
            .ok_or(ProcessTrackerError::MemoryBudget)?;
        let projection = tracker_memory_usage(
            projected_active_capacity,
            self.recent_terminated.capacity(),
            preparation_name_capacity,
            self.active
                .len()
                .saturating_add(self.recent_terminated.len())
                .saturating_add(1),
        )
        .map_err(|_| ProcessTrackerError::MemoryBudget)?;
        if self.memory.try_set_usage(projection).is_err() {
            return Err(ProcessTrackerError::MemoryBudget);
        }
        if map_additional != 0 && self.active.try_reserve(1).is_err() {
            self.settle_memory();
            return Err(ProcessTrackerError::Allocation);
        }
        match self.prepare_name(identity.process_name) {
            Ok(name) => Ok((name, victim)),
            Err(error) => {
                self.settle_memory();
                Err(error)
            }
        }
    }

    fn commit_active_insert(&mut self, pid: u32, active: ActiveProcess, victim: Option<u32>) {
        debug_assert_eq!(pid, active.pid);
        self.remove_active(pid);
        if let Some(victim) = victim {
            if self.remove_active(victim).is_some() {
                self.active_eviction_count = self.active_eviction_count.saturating_add(1);
            }
        }
        self.process_name_capacity = self
            .process_name_capacity
            .saturating_add(active.process_name.capacity());
        self.active.insert(pid, active);
        self.settle_memory();
    }

    fn remove_active(&mut self, pid: u32) -> Option<ActiveProcess> {
        let active = self.active.remove(&pid)?;
        self.process_name_capacity = self
            .process_name_capacity
            .saturating_sub(active.process_name.capacity());
        Some(active)
    }

    fn oldest_active_pid(&self) -> Option<u32> {
        self.active
            .iter()
            .min_by_key(|(pid, active)| (active.last_touched_line, active.instance.key.0, **pid))
            .map(|(pid, _)| *pid)
    }

    fn prepare_name(&self, value: &str) -> Result<String, ProcessTrackerError> {
        let mut prepared = String::new();
        prepared
            .try_reserve_exact(value.len())
            .map_err(|_| ProcessTrackerError::Allocation)?;
        prepared.push_str(value);
        Ok(prepared)
    }

    fn prepare_death_retention(
        &mut self,
        prepared_name_bytes: Option<usize>,
    ) -> Result<(), ProcessTrackerError> {
        let needs_growth = self.recent_terminated.len() < self.max_recent_terminated;
        let projected_recent_capacity = if needs_growth {
            projected_vec_capacity(
                self.recent_terminated.len(),
                self.recent_terminated.capacity(),
                1,
            )
            .ok_or(ProcessTrackerError::MemoryBudget)?
        } else {
            self.recent_terminated.capacity()
        };
        let preparation_name_capacity = self
            .process_name_capacity
            .checked_add(prepared_name_bytes.unwrap_or(0))
            .ok_or(ProcessTrackerError::MemoryBudget)?;
        let projection = tracker_memory_usage(
            self.active.capacity(),
            projected_recent_capacity,
            preparation_name_capacity,
            self.active
                .len()
                .saturating_add(self.recent_terminated.len())
                .saturating_add(usize::from(prepared_name_bytes.is_some())),
        )
        .map_err(|_| ProcessTrackerError::MemoryBudget)?;
        self.memory
            .try_set_usage(projection)
            .map_err(|_| ProcessTrackerError::MemoryBudget)?;
        if needs_growth && self.recent_terminated.try_reserve_exact(1).is_err() {
            self.settle_memory();
            return Err(ProcessTrackerError::Allocation);
        }
        Ok(())
    }

    fn record_retention_failure(&mut self, error: ProcessTrackerError) -> ProcessTrackerError {
        self.budget_drop_count = self.budget_drop_count.saturating_add(1);
        error
    }

    fn settle_memory(&mut self) {
        let Ok(usage) = tracker_memory_usage(
            self.active.capacity(),
            self.recent_terminated.capacity(),
            self.process_name_capacity,
            self.active
                .len()
                .saturating_add(self.recent_terminated.len()),
        ) else {
            return;
        };
        if usage.charged_bytes <= self.memory.usage().charged_bytes {
            self.memory.settle_precharged(usage);
        } else {
            let _ = self.memory.try_set_usage(usage);
        }
    }

    fn allocate_instance(
        &mut self,
        epoch_origin: ProcessEpochOrigin,
    ) -> Result<ProcessInstance, ProcessTrackerError> {
        let key = self.next_key;
        self.next_key = self
            .next_key
            .checked_add(1)
            .ok_or(ProcessTrackerError::KeySpaceExhausted)?;
        let epoch = self.next_epoch;
        self.next_epoch = self
            .next_epoch
            .checked_add(1)
            .ok_or(ProcessTrackerError::EpochSpaceExhausted)?;
        Ok(ProcessInstance {
            key: ProcessInstanceKey(key),
            epoch,
            epoch_origin,
        })
    }
}

fn tracker_memory_usage(
    active_capacity: usize,
    recent_capacity: usize,
    process_name_capacity: usize,
    process_name_allocations: usize,
) -> Result<ProblemMemoryUsage, ProblemMemoryBudgetError> {
    hash_map_usage::<u32, ActiveProcess>(active_capacity)?
        .checked_add(vec_deque_usage::<TerminatedProcess>(recent_capacity)?)?
        .checked_add(aggregate_vec_usage::<u8>(
            process_name_capacity,
            process_name_allocations,
        )?)
}

fn projected_hash_capacity(len: usize, capacity: usize, additional: usize) -> Option<usize> {
    let required = len.checked_add(additional)?;
    if required <= capacity {
        Some(capacity)
    } else {
        required
            .checked_next_power_of_two()?
            .checked_mul(2)
            .map(|capacity| capacity.max(4))
    }
}

fn projected_vec_capacity(len: usize, capacity: usize, additional: usize) -> Option<usize> {
    let required = len.checked_add(additional)?;
    if required <= capacity {
        Some(capacity)
    } else {
        required.checked_next_power_of_two()
    }
}

impl ActiveProcess {
    fn matches(&self, identity: ProcessIdentity<'_>) -> bool {
        self.process_name.as_str() == identity.process_name
            && optional_field_matches(self.uid, identity.uid)
            && optional_field_matches(self.user, identity.user)
    }

    fn enrich(&mut self, identity: ProcessIdentity<'_>) {
        if self.uid.is_none() {
            self.uid = identity.uid;
        }
        if self.user.is_none() {
            self.user = identity.user;
        }
    }
}

fn optional_field_matches<T: Eq>(known: Option<T>, observed: Option<T>) -> bool {
    match (known, observed) {
        (Some(known), Some(observed)) => known == observed,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strong_mid_log_identity_starts_a_provisional_epoch() {
        let mut tracker = ProcessInstanceTracker::new();
        let identity =
            ProcessIdentity::new(42, "com.example.player", Some(10_042), Some(0)).unwrap();

        let first = tracker.observe_identity(100, identity).unwrap();
        let repeated = tracker.observe_identity(101, identity).unwrap();

        assert_ne!(first.key(), ProcessInstanceKey(0));
        assert_eq!(first.epoch_origin(), ProcessEpochOrigin::Provisional);
        assert_eq!(first.identity_quality(), IdentityQuality::KnownProcess);
        assert_eq!(repeated.key(), first.key());
        assert_eq!(repeated.epoch(), first.epoch());

        let active = tracker.active_for_pid(42).unwrap();
        assert_eq!(active.pid(), 42);
        assert_eq!(active.process_name(), "com.example.player");
        assert_eq!(active.start_line(), 100);
        assert_eq!(active.last_touched_line(), 101);
    }

    #[test]
    fn every_explicit_start_advances_the_epoch_for_a_reused_pid() {
        let mut tracker = ProcessInstanceTracker::new();
        let identity =
            ProcessIdentity::new(42, "com.example.player", Some(10_042), Some(0)).unwrap();

        let first = tracker.observe_start(10, identity).unwrap();
        let second = tracker.observe_start(20, identity).unwrap();

        assert_eq!(first.epoch_origin(), ProcessEpochOrigin::ExplicitStart);
        assert_eq!(second.epoch_origin(), ProcessEpochOrigin::ExplicitStart);
        assert_ne!(first.key(), second.key());
        assert!(second.epoch() > first.epoch());
        assert_eq!(tracker.active_for_pid(42).unwrap().instance(), second);
    }

    #[test]
    fn conflicting_identity_on_a_reused_pid_starts_a_new_provisional_epoch() {
        let mut tracker = ProcessInstanceTracker::new();
        let first_identity =
            ProcessIdentity::new(42, "com.example.first", Some(10_042), Some(0)).unwrap();
        let reused_identity =
            ProcessIdentity::new(42, "com.example.second", Some(10_043), Some(0)).unwrap();

        let first = tracker.observe_identity(10, first_identity).unwrap();
        let reused = tracker.observe_identity(20, reused_identity).unwrap();

        assert_ne!(first.key(), reused.key());
        assert!(reused.epoch() > first.epoch());
        assert_eq!(reused.epoch_origin(), ProcessEpochOrigin::Provisional);
        assert_eq!(
            tracker.active_for_pid(42).unwrap().process_name(),
            "com.example.second"
        );
    }

    #[test]
    fn active_matching_requires_pid_and_process_and_rejects_known_field_conflicts() {
        let mut tracker = ProcessInstanceTracker::new();
        let exact = ProcessIdentity::new(42, "com.example.player", Some(10_042), Some(0)).unwrap();
        let instance = tracker.observe_start(10, exact).unwrap();

        let missing_optional = ProcessIdentity::new(42, "com.example.player", None, None).unwrap();
        let wrong_pid =
            ProcessIdentity::new(43, "com.example.player", Some(10_042), Some(0)).unwrap();
        let wrong_process =
            ProcessIdentity::new(42, "com.example.other", Some(10_042), Some(0)).unwrap();
        let wrong_uid =
            ProcessIdentity::new(42, "com.example.player", Some(10_043), Some(0)).unwrap();
        let wrong_user =
            ProcessIdentity::new(42, "com.example.player", Some(10_042), Some(10)).unwrap();

        assert_eq!(
            tracker
                .active_matching(exact)
                .map(|tracked| tracked.instance()),
            Some(instance)
        );
        assert_eq!(
            tracker
                .active_matching(missing_optional)
                .map(|tracked| tracked.instance()),
            Some(instance)
        );
        assert!(tracker.active_matching(wrong_pid).is_none());
        assert!(tracker.active_matching(wrong_process).is_none());
        assert!(tracker.active_matching(wrong_uid).is_none());
        assert!(tracker.active_matching(wrong_user).is_none());
    }

    #[test]
    fn recent_death_matching_requires_uid_and_process_and_never_uses_name_only() {
        let mut tracker = ProcessInstanceTracker::new();
        let started =
            ProcessIdentity::new(42, "com.example.player", Some(10_042), Some(0)).unwrap();
        let death_without_optional_fields =
            ProcessIdentity::new(42, "com.example.player", None, None).unwrap();
        let instance = tracker.observe_start(10, started).unwrap();

        let terminated = tracker
            .observe_death(20, death_without_optional_fields)
            .unwrap();

        assert_eq!(terminated, instance);
        assert!(tracker.active_for_pid(42).is_none());
        assert!(tracker
            .recent_terminated_matching("com.example.player", None, None)
            .is_none());
        assert!(tracker
            .recent_terminated_matching("com.example.player", Some(10_043), None)
            .is_none());
        assert!(tracker
            .recent_terminated_matching("com.example.other", Some(10_042), None)
            .is_none());
        assert!(tracker
            .recent_terminated_matching("com.example.player", Some(10_042), Some(10))
            .is_none());

        let historical = tracker
            .recent_terminated_matching("com.example.player", Some(10_042), None)
            .unwrap();
        assert_eq!(historical.instance(), instance);
        assert_eq!(historical.pid(), 42);
        assert_eq!(historical.death_line(), 20);
        assert_eq!(historical.uid(), Some(10_042));
        assert_eq!(historical.user(), Some(0));
    }

    #[test]
    fn tracker_limits_are_fixed_at_the_documented_hard_caps() {
        let tracker = ProcessInstanceTracker::new();

        assert_eq!(
            tracker.stats().max_active_instances(),
            MAX_ACTIVE_PROCESS_INSTANCES
        );
        assert_eq!(
            tracker.stats().max_recent_terminated(),
            MAX_RECENT_TERMINATED_INSTANCES
        );
        assert!(matches!(
            ProcessInstanceTracker::with_limits(MAX_ACTIVE_PROCESS_INSTANCES + 1, 1),
            Err(ProcessTrackerLimitsError::InvalidActiveLimit)
        ));
        assert!(matches!(
            ProcessInstanceTracker::with_limits(1, MAX_RECENT_TERMINATED_INSTANCES + 1),
            Err(ProcessTrackerLimitsError::InvalidRecentLimit)
        ));
        assert!(matches!(
            ProcessInstanceTracker::with_limits(0, 1),
            Err(ProcessTrackerLimitsError::InvalidActiveLimit)
        ));
        assert!(matches!(
            ProcessInstanceTracker::with_limits(1, 0),
            Err(ProcessTrackerLimitsError::InvalidRecentLimit)
        ));
    }

    #[test]
    fn active_capacity_evicts_by_last_touched_line_then_instance_key() {
        let mut tracker = ProcessInstanceTracker::with_limits(2, 2).unwrap();
        let first = ProcessIdentity::new(1, "com.example.first", Some(10_001), Some(0)).unwrap();
        let second = ProcessIdentity::new(2, "com.example.second", Some(10_002), Some(0)).unwrap();
        let third = ProcessIdentity::new(3, "com.example.third", Some(10_003), Some(0)).unwrap();
        let fourth = ProcessIdentity::new(4, "com.example.fourth", Some(10_004), Some(0)).unwrap();

        tracker.observe_start(10, first).unwrap();
        tracker.observe_start(10, second).unwrap();
        tracker.observe_start(10, third).unwrap();

        assert!(tracker.active_for_pid(1).is_none());
        assert!(tracker.active_for_pid(2).is_some());
        assert!(tracker.active_for_pid(3).is_some());

        tracker.observe_identity(11, second).unwrap();
        tracker.observe_start(11, fourth).unwrap();

        assert!(tracker.active_for_pid(2).is_some());
        assert!(tracker.active_for_pid(3).is_none());
        assert!(tracker.active_for_pid(4).is_some());
        assert_eq!(tracker.stats().active_instances(), 2);
        assert_eq!(tracker.stats().active_eviction_count(), 2);
        assert!(tracker.stats().identity_coverage_limited());
    }

    #[test]
    fn recent_terminated_capacity_evicts_oldest_death_deterministically() {
        let mut tracker = ProcessInstanceTracker::with_limits(3, 2).unwrap();
        let first = ProcessIdentity::new(1, "com.example.first", Some(10_001), Some(0)).unwrap();
        let second = ProcessIdentity::new(2, "com.example.second", Some(10_002), Some(0)).unwrap();
        let third = ProcessIdentity::new(3, "com.example.third", Some(10_003), Some(0)).unwrap();

        for identity in [first, second, third] {
            tracker.observe_start(10, identity).unwrap();
            tracker.observe_death(20, identity).unwrap();
        }

        assert!(tracker
            .recent_terminated_matching("com.example.first", Some(10_001), None)
            .is_none());
        assert!(tracker
            .recent_terminated_matching("com.example.second", Some(10_002), None)
            .is_some());
        assert!(tracker
            .recent_terminated_matching("com.example.third", Some(10_003), None)
            .is_some());
        assert_eq!(tracker.stats().recent_terminated(), 2);
        assert_eq!(tracker.stats().recent_eviction_count(), 1);
        assert!(tracker.stats().identity_coverage_limited());
    }

    #[test]
    fn public_identity_rejects_names_that_could_bypass_the_tracker_budget() {
        let oversized = "x".repeat(MAX_TRACKED_PROCESS_NAME_BYTES + 1);
        assert!(matches!(
            ProcessIdentity::new(1, &oversized, None, None),
            Err(ProcessIdentityError::ProcessNameTooLong)
        ));
    }

    #[test]
    fn identity_storm_is_budgeted_and_reset_reclaims_all_tracker_heap() {
        let budget = ProblemMemoryBudget::with_limit_bytes(4 * 1024).unwrap();
        let mut tracker =
            ProcessInstanceTracker::with_limits_and_budget(100, 10, budget.clone()).unwrap();
        let mut denied = false;
        for pid in 1..=100 {
            let name = format!("com.example.process{pid:03}.{}", "x".repeat(96));
            let identity = ProcessIdentity::new(pid, &name, Some(10_000 + pid), Some(0)).unwrap();
            match tracker.observe_start(pid, identity) {
                Ok(_) => {}
                Err(ProcessTrackerError::MemoryBudget) => {
                    denied = true;
                    break;
                }
                Err(error) => panic!("unexpected tracker error: {error:?}"),
            }
        }

        assert!(denied);
        assert!(tracker.stats().budget_drop_count() > 0);
        assert!(tracker.stats().identity_coverage_limited());
        assert!(budget.stats().charged_bytes <= budget.stats().limit_bytes);
        assert!(budget.stats().retained_heap_bytes <= budget.stats().charged_bytes);

        tracker.reset();
        assert_eq!(budget.stats().charged_bytes, 0);
        assert_eq!(budget.stats().retained_heap_bytes, 0);
        assert!(!budget.stats().limited);
    }
}
