use super::{IdentityQuality, ProcessInstanceKey};
use std::collections::{BTreeSet, HashMap, VecDeque};

pub const MAX_ACTIVE_PROCESS_INSTANCES: usize = 65_536;
pub const MAX_RECENT_TERMINATED_INSTANCES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessIdentityError {
    ZeroPid,
    EmptyProcessName,
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
    process_name: Box<str>,
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
    active_order: BTreeSet<(u32, u32, u32)>,
    recent_terminated: VecDeque<TerminatedProcess>,
    max_active_instances: usize,
    max_recent_terminated: usize,
    active_eviction_count: u64,
    recent_eviction_count: u64,
    next_key: u32,
    next_epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessTrackerStats {
    active_instances: usize,
    recent_terminated: usize,
    max_active_instances: usize,
    max_recent_terminated: usize,
    active_eviction_count: u64,
    recent_eviction_count: u64,
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

    pub const fn identity_coverage_limited(self) -> bool {
        self.active_eviction_count != 0 || self.recent_eviction_count != 0
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
        if !(1..=MAX_ACTIVE_PROCESS_INSTANCES).contains(&max_active_instances) {
            return Err(ProcessTrackerLimitsError::InvalidActiveLimit);
        }
        if !(1..=MAX_RECENT_TERMINATED_INSTANCES).contains(&max_recent_terminated) {
            return Err(ProcessTrackerLimitsError::InvalidRecentLimit);
        }
        Ok(Self {
            active: HashMap::new(),
            active_order: BTreeSet::new(),
            recent_terminated: VecDeque::new(),
            max_active_instances,
            max_recent_terminated,
            active_eviction_count: 0,
            recent_eviction_count: 0,
            next_key: 1,
            next_epoch: 1,
        })
    }

    pub fn observe_identity(
        &mut self,
        line: u32,
        identity: ProcessIdentity<'_>,
    ) -> Result<ProcessInstance, ProcessTrackerError> {
        let matches_active = self
            .active
            .get(&identity.pid)
            .is_some_and(|active| active.matches(identity));
        if matches_active {
            let mut active = self
                .remove_active(identity.pid)
                .expect("matching active process must still exist");
            active.enrich(identity);
            active.last_touched_line = active.last_touched_line.max(line);
            let instance = active.instance;
            self.insert_active(identity.pid, active);
            return Ok(instance);
        }

        let instance = self.allocate_instance(ProcessEpochOrigin::Provisional)?;
        self.insert_active(
            identity.pid,
            ActiveProcess {
                instance,
                pid: identity.pid,
                process_name: identity.process_name.into(),
                uid: identity.uid,
                user: identity.user,
                start_line: line,
                last_touched_line: line,
            },
        );
        Ok(instance)
    }

    pub fn observe_start(
        &mut self,
        line: u32,
        identity: ProcessIdentity<'_>,
    ) -> Result<ProcessInstance, ProcessTrackerError> {
        let instance = self.allocate_instance(ProcessEpochOrigin::ExplicitStart)?;
        self.insert_active(
            identity.pid,
            ActiveProcess {
                instance,
                pid: identity.pid,
                process_name: identity.process_name.into(),
                uid: identity.uid,
                user: identity.user,
                start_line: line,
                last_touched_line: line,
            },
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
        let mut process = if matching_active {
            self.remove_active(identity.pid)
                .expect("matching active process must still exist")
        } else {
            self.remove_active(identity.pid);
            let instance = self.allocate_instance(ProcessEpochOrigin::Provisional)?;
            ActiveProcess {
                instance,
                pid: identity.pid,
                process_name: identity.process_name.into(),
                uid: identity.uid,
                user: identity.user,
                start_line: line,
                last_touched_line: line,
            }
        };
        process.enrich(identity);
        process.last_touched_line = process.last_touched_line.max(line);
        let instance = process.instance;
        self.recent_terminated.push_back(TerminatedProcess {
            process,
            death_line: line,
        });
        while self.recent_terminated.len() > self.max_recent_terminated {
            self.recent_terminated.pop_front();
            self.recent_eviction_count = self.recent_eviction_count.saturating_add(1);
        }
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
                terminated.process.process_name.as_ref() == process_name
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
        }
    }

    fn insert_active(&mut self, pid: u32, active: ActiveProcess) {
        debug_assert_eq!(pid, active.pid);
        self.remove_active(pid);
        self.active_order.insert(active_order_key(pid, &active));
        self.active.insert(pid, active);
        while self.active.len() > self.max_active_instances {
            let oldest = self
                .active_order
                .iter()
                .next()
                .copied()
                .expect("an over-capacity active map must have an order entry");
            self.active_order.remove(&oldest);
            self.active.remove(&oldest.2);
            self.active_eviction_count = self.active_eviction_count.saturating_add(1);
        }
    }

    fn remove_active(&mut self, pid: u32) -> Option<ActiveProcess> {
        let active = self.active.remove(&pid)?;
        self.active_order.remove(&active_order_key(pid, &active));
        Some(active)
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

fn active_order_key(pid: u32, active: &ActiveProcess) -> (u32, u32, u32) {
    (active.last_touched_line, active.instance.key.0, pid)
}

impl ActiveProcess {
    fn matches(&self, identity: ProcessIdentity<'_>) -> bool {
        self.process_name.as_ref() == identity.process_name
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
}
