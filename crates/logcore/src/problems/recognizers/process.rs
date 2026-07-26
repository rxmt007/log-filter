use crate::problems::{
    EventLogRecord, EvidenceFormat, LineProvenance, LogBuffer, ProblemMemoryBudget,
    ProcessIdentity, ProcessInstance, ProcessInstanceKey, ProcessInstanceTracker,
    ProcessTrackerError,
};
use std::str;

pub const MAX_LIFECYCLE_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_PROCESS_NAME_BYTES: usize = 256;
pub const MAX_PENDING_DEATHS: usize = 64;
pub const QUICK_RESTART_THRESHOLD_MS: u64 = 30_000;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ProcessNameToken([u8; MAX_PROCESS_NAME_BYTES]);

impl ProcessNameToken {
    pub fn new(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_PROCESS_NAME_BYTES
            || bytes.contains(&0)
            || !bytes.iter().copied().all(is_process_name_byte)
        {
            return None;
        }
        let mut token = Self([0; MAX_PROCESS_NAME_BYTES]);
        token.0[..bytes.len()].copy_from_slice(bytes);
        Some(token)
    }

    pub fn as_str(&self) -> &str {
        let len = self
            .0
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(MAX_PROCESS_NAME_BYTES);
        str::from_utf8(&self.0[..len]).expect("process names are validated ASCII")
    }
}

impl std::fmt::Debug for ProcessNameToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ProcessNameToken")
            .field(&self.as_str())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LifecycleTime {
    pub segment: u64,
    pub millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleRelation {
    Restart,
    SignalExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RestartTimingFacet {
    WithinThirtySeconds,
    AfterThirtySeconds,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LifecycleFingerprintInput {
    pub process: ProcessNameToken,
    pub relation: LifecycleRelation,
    pub signal: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleSource {
    pub format: EvidenceFormat,
    pub provenance: LineProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleOccurrence {
    pub relation: LifecycleRelation,
    pub process: ProcessNameToken,
    pub uid: u32,
    pub user: Option<u32>,
    pub death_pid: u32,
    pub start_pid: Option<u32>,
    pub death_line: u32,
    pub start_line: Option<u32>,
    pub terminated_instance: ProcessInstance,
    pub started_instance: Option<ProcessInstance>,
    pub signal: Option<u8>,
    pub timing: RestartTimingFacet,
    pub fingerprint: LifecycleFingerprintInput,
    pub death_source: LifecycleSource,
    pub start_source: Option<LifecycleSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleObservationKind {
    Start,
    Death,
    KillRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationStrength {
    Independent,
    SupportingOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleObservation {
    pub kind: LifecycleObservationKind,
    pub line: u32,
    pub pid: u32,
    pub process: ProcessNameToken,
    pub uid: Option<u32>,
    pub user: Option<u32>,
    pub process_instance: ProcessInstanceKey,
    pub strength: ObservationStrength,
    pub source: LifecycleSource,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleDelta {
    pub observation: Option<LifecycleObservation>,
    pub occurrence: Option<LifecycleOccurrence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleRecognizerError {
    Tracker(ProcessTrackerError),
}

impl From<ProcessTrackerError> for LifecycleRecognizerError {
    fn from(value: ProcessTrackerError) -> Self {
        Self::Tracker(value)
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingDeath {
    line: u32,
    time: Option<LifecycleTime>,
    pid: u32,
    process: ProcessNameToken,
    uid: u32,
    user: Option<u32>,
    instance: ProcessInstance,
    source: LifecycleSource,
}

#[derive(Debug)]
pub struct LifecycleRecognizer {
    tracker: ProcessInstanceTracker,
    pending: [Option<PendingDeath>; MAX_PENDING_DEATHS],
    pending_eviction_count: u64,
}

impl Default for LifecycleRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleRecognizer {
    pub fn new() -> Self {
        Self::with_budget(ProblemMemoryBudget::new())
    }

    pub(crate) fn with_budget(memory_budget: ProblemMemoryBudget) -> Self {
        Self {
            tracker: ProcessInstanceTracker::with_limits_and_budget(
                crate::problems::MAX_ACTIVE_PROCESS_INSTANCES,
                crate::problems::MAX_RECENT_TERMINATED_INSTANCES,
                memory_budget,
            )
            .expect("documented process tracker limits must be valid"),
            pending: [None; MAX_PENDING_DEATHS],
            pending_eviction_count: 0,
        }
    }

    pub fn tracker(&self) -> &ProcessInstanceTracker {
        &self.tracker
    }

    pub(crate) const fn pending_eviction_count(&self) -> u64 {
        self.pending_eviction_count
    }

    pub fn pending_count(&self) -> usize {
        self.pending.iter().flatten().count()
    }

    pub(crate) fn observe_fault_identity(
        &mut self,
        line: u32,
        pid: u32,
        process: &str,
    ) -> Result<ProcessInstanceKey, LifecycleRecognizerError> {
        let identity = ProcessIdentity::new(pid, process, None, None)
            .expect("recognized fault identities have a non-zero pid and process name");
        Ok(self.tracker.observe_identity(line, identity)?.key())
    }

    #[cfg(test)]
    pub fn observe_activity_manager(
        &mut self,
        line: u32,
        message: &[u8],
        time: Option<LifecycleTime>,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        self.observe_activity_manager_with_provenance(line, message, LineProvenance::Unknown, time)
    }

    pub(crate) fn observe_activity_manager_with_provenance(
        &mut self,
        line: u32,
        message: &[u8],
        provenance: LineProvenance,
        time: Option<LifecycleTime>,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        if message.len() > MAX_LIFECYCLE_INPUT_BYTES || str::from_utf8(message).is_err() {
            return Ok(LifecycleDelta::default());
        }
        let source = LifecycleSource {
            format: EvidenceFormat::AospText,
            provenance,
        };
        if let Some(start) = parse_start_proc(message) {
            return self.record_start(line, start, source, time, ObservationStrength::Independent);
        }
        if let Some(death) = parse_process_died(message) {
            return self.record_death(line, death, source, time, ObservationStrength::Independent);
        }
        Ok(LifecycleDelta::default())
    }

    pub fn observe_event_log(
        &mut self,
        line: u32,
        record: EventLogRecord<'_>,
        provenance: LineProvenance,
        time: Option<LifecycleTime>,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        let Some(strength) = event_log_strength(provenance) else {
            return Ok(LifecycleDelta::default());
        };
        let source = LifecycleSource {
            format: EvidenceFormat::EventLogShapedText,
            provenance,
        };
        match record {
            EventLogRecord::ProcStart(start) => {
                let parsed = ParsedStart {
                    pid: start.pid,
                    uid: start.uid,
                    user: non_negative_user(start.user_id),
                    process: ProcessNameToken::new(start.process_name),
                };
                if !uid_matches_user(parsed.uid, parsed.user) {
                    return Ok(LifecycleDelta::default());
                }
                let Some(process) = parsed.process else {
                    return Ok(LifecycleDelta::default());
                };
                let parsed = ParsedStart {
                    process: Some(process),
                    ..parsed
                };
                if strength == ObservationStrength::SupportingOnly {
                    return Ok(LifecycleDelta {
                        observation: Some(start_observation(
                            line,
                            parsed,
                            ProcessInstanceKey(0),
                            strength,
                            source,
                        )),
                        occurrence: None,
                    });
                }
                self.record_start(line, parsed, source, time, strength)
            }
            EventLogRecord::ProcDied(death) => {
                let Some(process) = ProcessNameToken::new(death.process_name) else {
                    return Ok(LifecycleDelta::default());
                };
                let parsed = ParsedDeath {
                    pid: death.pid,
                    process,
                    user: non_negative_user(death.user_id),
                };
                if strength == ObservationStrength::SupportingOnly {
                    return Ok(LifecycleDelta {
                        observation: Some(death_observation(
                            line,
                            parsed,
                            None,
                            ProcessInstanceKey(0),
                            strength,
                            source,
                        )),
                        occurrence: None,
                    });
                }
                self.record_death(line, parsed, source, time, strength)
            }
            EventLogRecord::Kill(kill) => {
                let Some(process) = ProcessNameToken::new(kill.process_name) else {
                    return Ok(LifecycleDelta::default());
                };
                let process_instance = self
                    .tracker
                    .active_for_pid(kill.pid)
                    .filter(|active| active.process_name() == process.as_str())
                    .map_or(ProcessInstanceKey(0), |active| active.instance().key());
                Ok(LifecycleDelta {
                    observation: Some(LifecycleObservation {
                        kind: LifecycleObservationKind::KillRequest,
                        line,
                        pid: kill.pid,
                        process,
                        uid: None,
                        user: non_negative_user(kill.user_id),
                        process_instance,
                        strength: ObservationStrength::SupportingOnly,
                        source,
                    }),
                    occurrence: None,
                })
            }
            _ => Ok(LifecycleDelta::default()),
        }
    }

    #[cfg(test)]
    pub fn observe_signal_exit(
        &mut self,
        line: u32,
        pid: u32,
        signal: u8,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        self.observe_signal_exit_with_provenance(line, pid, signal, LineProvenance::Unknown)
    }

    pub(crate) fn observe_signal_exit_with_provenance(
        &mut self,
        line: u32,
        pid: u32,
        signal: u8,
        provenance: LineProvenance,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        if !(1..=64).contains(&signal) {
            return Ok(LifecycleDelta::default());
        }
        let Some(active) = self.tracker.active_for_pid(pid) else {
            return Ok(LifecycleDelta::default());
        };
        let Some(uid) = active.uid() else {
            return Ok(LifecycleDelta::default());
        };
        let process = ProcessNameToken::new(active.process_name())
            .expect("tracked process names already passed validation");
        let user = active.user();
        let identity = ProcessIdentity::new(pid, process.as_str(), Some(uid), user)
            .expect("tracked process identity is valid");
        let terminated_instance = self.tracker.observe_death(line, identity)?;
        let fingerprint = LifecycleFingerprintInput {
            process,
            relation: LifecycleRelation::SignalExit,
            signal: Some(signal),
        };
        let source = LifecycleSource {
            format: EvidenceFormat::AospText,
            provenance,
        };
        Ok(LifecycleDelta {
            observation: Some(LifecycleObservation {
                kind: LifecycleObservationKind::Death,
                line,
                pid,
                process,
                uid: Some(uid),
                user,
                process_instance: terminated_instance.key(),
                strength: ObservationStrength::Independent,
                source,
            }),
            occurrence: Some(LifecycleOccurrence {
                relation: LifecycleRelation::SignalExit,
                process,
                uid,
                user,
                death_pid: pid,
                start_pid: None,
                death_line: line,
                start_line: None,
                terminated_instance,
                started_instance: None,
                signal: Some(signal),
                timing: RestartTimingFacet::Unknown,
                fingerprint,
                death_source: source,
                start_source: None,
            }),
        })
    }

    /// Truncated input never upgrades an unmatched death into a Problem.
    pub fn finish_input(&mut self) -> u8 {
        let pending = self.pending_count().min(usize::from(u8::MAX)) as u8;
        self.pending.fill(None);
        pending
    }

    pub fn reset(&mut self) {
        self.tracker.reset();
        self.pending.fill(None);
        self.pending_eviction_count = 0;
    }

    fn record_start(
        &mut self,
        line: u32,
        start: ParsedStart,
        source: LifecycleSource,
        time: Option<LifecycleTime>,
        strength: ObservationStrength,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        let process = start
            .process
            .expect("parsers only return starts with a bounded process name");
        let matching_pending = self.take_next_matching(process, start.uid, start.user);
        let identity =
            ProcessIdentity::new(start.pid, process.as_str(), Some(start.uid), start.user)
                .expect("parsed start identity is valid");
        let started_instance = self.tracker.observe_start(line, identity)?;
        let occurrence = matching_pending.map(|death| {
            let timing = restart_timing(death.time, time);
            let fingerprint = LifecycleFingerprintInput {
                process,
                relation: LifecycleRelation::Restart,
                signal: None,
            };
            LifecycleOccurrence {
                relation: LifecycleRelation::Restart,
                process,
                uid: start.uid,
                user: merged_user(death.user, start.user),
                death_pid: death.pid,
                start_pid: Some(start.pid),
                death_line: death.line,
                start_line: Some(line),
                terminated_instance: death.instance,
                started_instance: Some(started_instance),
                signal: None,
                timing,
                fingerprint,
                death_source: death.source,
                start_source: Some(source),
            }
        });
        Ok(LifecycleDelta {
            observation: Some(start_observation(
                line,
                start,
                started_instance.key(),
                strength,
                source,
            )),
            occurrence,
        })
    }

    fn record_death(
        &mut self,
        line: u32,
        death: ParsedDeath,
        source: LifecycleSource,
        time: Option<LifecycleTime>,
        strength: ObservationStrength,
    ) -> Result<LifecycleDelta, LifecycleRecognizerError> {
        let active_identity = self.tracker.active_for_pid(death.pid).and_then(|active| {
            (active.process_name() == death.process.as_str()
                && optional_user_matches(active.user(), death.user))
            .then_some((active.uid()?, active.user()))
        });
        let identity = ProcessIdentity::new(
            death.pid,
            death.process.as_str(),
            active_identity.map(|identity| identity.0),
            merged_user(active_identity.and_then(|identity| identity.1), death.user),
        )
        .expect("parsed death identity is valid");
        let instance = self.tracker.observe_death(line, identity)?;
        if let Some((uid, active_user)) = active_identity {
            let pending = PendingDeath {
                line,
                time,
                pid: death.pid,
                process: death.process,
                uid,
                user: merged_user(active_user, death.user),
                instance,
                source,
            };
            self.insert_pending(pending);
        }
        Ok(LifecycleDelta {
            observation: Some(death_observation(
                line,
                death,
                active_identity.map(|identity| identity.0),
                instance.key(),
                strength,
                source,
            )),
            occurrence: None,
        })
    }

    fn take_next_matching(
        &mut self,
        process: ProcessNameToken,
        uid: u32,
        user: Option<u32>,
    ) -> Option<PendingDeath> {
        let index = self
            .pending
            .iter()
            .enumerate()
            .filter_map(|(index, pending)| pending.map(|pending| (index, pending)))
            .filter(|(_, pending)| {
                pending.process == process
                    && pending.uid == uid
                    && optional_user_matches(pending.user, user)
            })
            .min_by_key(|(_, pending)| pending.line)
            .map(|(index, _)| index)?;
        self.pending[index].take()
    }

    fn insert_pending(&mut self, pending: PendingDeath) {
        if let Some(slot) = self.pending.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(pending);
            return;
        }
        let oldest = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, candidate)| {
                candidate
                    .map(|candidate| candidate.line)
                    .unwrap_or(u32::MAX)
            })
            .map(|(index, _)| index)
            .expect("fixed pending table is non-empty");
        self.pending[oldest] = Some(pending);
        self.pending_eviction_count = self.pending_eviction_count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedStart {
    pid: u32,
    uid: u32,
    user: Option<u32>,
    process: Option<ProcessNameToken>,
}

#[derive(Debug, Clone, Copy)]
struct ParsedDeath {
    pid: u32,
    process: ProcessNameToken,
    user: Option<u32>,
}

fn parse_start_proc(message: &[u8]) -> Option<ParsedStart> {
    let value = trim_ascii(message).strip_prefix(b"Start proc ")?;
    let (pid, after_pid) = parse_decimal_prefix(value)?;
    let after_pid = after_pid.strip_prefix(b":")?;
    let slash = after_pid.iter().position(|byte| *byte == b'/')?;
    let process = ProcessNameToken::new(str::from_utf8(&after_pid[..slash]).ok()?)?;
    let identity_end = after_pid[slash + 1..]
        .iter()
        .position(u8::is_ascii_whitespace)?;
    let identity = &after_pid[slash + 1..slash + 1 + identity_end];
    let remainder = &after_pid[slash + 1 + identity_end..];
    if !remainder.starts_with(b" for ") {
        return None;
    }
    let (user, uid) = parse_android_uid(identity)?;
    Some(ParsedStart {
        pid,
        uid,
        user: Some(user),
        process: Some(process),
    })
}

fn parse_process_died(message: &[u8]) -> Option<ParsedDeath> {
    let value = trim_ascii(message).strip_prefix(b"Process ")?;
    let marker = find_subslice(value, b" (pid ")?;
    let process = ProcessNameToken::new(str::from_utf8(&value[..marker]).ok()?)?;
    let (pid, remainder) = parse_decimal_prefix(&value[marker + b" (pid ".len()..])?;
    let remainder = remainder.strip_prefix(b") has died")?;
    if !remainder.is_empty() && !remainder.starts_with(b":") {
        return None;
    }
    Some(ParsedDeath {
        pid,
        process,
        user: None,
    })
}

pub(crate) fn parse_zygote_signal_exit(message: &[u8]) -> Option<(u32, u8)> {
    let value = trim_ascii(message).strip_prefix(b"Process ")?;
    let (pid, remainder) = parse_decimal_prefix(value)?;
    let remainder = remainder.strip_prefix(b" exited due to signal ")?;
    let (signal, remainder) = parse_decimal_prefix(remainder)?;
    let signal = u8::try_from(signal).ok().filter(|signal| *signal <= 64)?;
    let name = remainder.strip_prefix(b" (")?.strip_suffix(b")")?;
    if name.is_empty()
        || !name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some((pid, signal))
}

fn parse_android_uid(value: &[u8]) -> Option<(u32, u32)> {
    let value = value.strip_prefix(b"u")?;
    let (user, rest) = parse_decimal_prefix_allow_zero(value)?;
    let kind = *rest.first()?;
    let (local, remainder) = parse_decimal_prefix_allow_zero(&rest[1..])?;
    if !remainder.is_empty() {
        return None;
    }
    let base = user.checked_mul(100_000)?;
    let uid = match kind {
        b'a' => base.checked_add(10_000)?.checked_add(local)?,
        b's' => base.checked_add(local)?,
        _ => return None,
    };
    Some((user, uid))
}

fn parse_decimal_prefix(value: &[u8]) -> Option<(u32, &[u8])> {
    let (number, remainder) = parse_decimal_prefix_allow_zero(value)?;
    (number != 0).then_some((number, remainder))
}

fn parse_decimal_prefix_allow_zero(value: &[u8]) -> Option<(u32, &[u8])> {
    let end = value
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(value.len());
    if end == 0 {
        return None;
    }
    let mut number = 0u32;
    for digit in &value[..end] {
        number = number
            .checked_mul(10)?
            .checked_add(u32::from(*digit - b'0'))?;
    }
    Some((number, &value[end..]))
}

fn start_observation(
    line: u32,
    start: ParsedStart,
    process_instance: ProcessInstanceKey,
    strength: ObservationStrength,
    source: LifecycleSource,
) -> LifecycleObservation {
    LifecycleObservation {
        kind: LifecycleObservationKind::Start,
        line,
        pid: start.pid,
        process: start.process.expect("parsed start has process"),
        uid: Some(start.uid),
        user: start.user,
        process_instance,
        strength,
        source,
    }
}

fn death_observation(
    line: u32,
    death: ParsedDeath,
    uid: Option<u32>,
    process_instance: ProcessInstanceKey,
    strength: ObservationStrength,
    source: LifecycleSource,
) -> LifecycleObservation {
    LifecycleObservation {
        kind: LifecycleObservationKind::Death,
        line,
        pid: death.pid,
        process: death.process,
        uid,
        user: death.user,
        process_instance,
        strength,
        source,
    }
}

fn event_log_strength(provenance: LineProvenance) -> Option<ObservationStrength> {
    match provenance {
        LineProvenance::Known(LogBuffer::Events) => Some(ObservationStrength::Independent),
        LineProvenance::Inferred(LogBuffer::Events) => Some(ObservationStrength::SupportingOnly),
        _ => None,
    }
}

fn non_negative_user(user: Option<i32>) -> Option<u32> {
    user.and_then(|user| u32::try_from(user).ok())
}

fn uid_matches_user(uid: u32, user: Option<u32>) -> bool {
    user.is_none_or(|user| uid / 100_000 == user)
}

fn restart_timing(
    death: Option<LifecycleTime>,
    start: Option<LifecycleTime>,
) -> RestartTimingFacet {
    match (death, start) {
        (Some(death), Some(start))
            if death.segment == start.segment && start.millis >= death.millis =>
        {
            if start.millis - death.millis <= QUICK_RESTART_THRESHOLD_MS {
                RestartTimingFacet::WithinThirtySeconds
            } else {
                RestartTimingFacet::AfterThirtySeconds
            }
        }
        _ => RestartTimingFacet::Unknown,
    }
}

fn optional_user_matches(left: Option<u32>, right: Option<u32>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn merged_user(left: Option<u32>, right: Option<u32>) -> Option<u32> {
    left.or(right)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn is_process_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'$' | b':' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::{AmKill, AmProcDied, AmProcStart, EventLogSchemaId, ProcessInstanceKey};
    use std::mem::{needs_drop, size_of};

    fn time(millis: u64) -> Option<LifecycleTime> {
        Some(LifecycleTime { segment: 1, millis })
    }

    #[test]
    fn text_death_is_internal_until_the_next_strict_matching_start() {
        let mut recognizer = LifecycleRecognizer::new();
        let start = recognizer
            .observe_activity_manager(
                10,
                b"Start proc 42:com.example.app/u0a123 for top-activity",
                time(1_000),
            )
            .unwrap();
        assert!(start.occurrence.is_none());
        let death = recognizer
            .observe_activity_manager(
                20,
                b"Process com.example.app (pid 42) has died: fg TOP",
                time(2_000),
            )
            .unwrap();
        assert!(death.occurrence.is_none());
        let restart = recognizer
            .observe_activity_manager(
                30,
                b"Start proc 77:com.example.app/u0a123 for activity",
                time(40_001),
            )
            .unwrap()
            .occurrence
            .unwrap();
        assert_eq!(restart.relation, LifecycleRelation::Restart);
        assert_eq!(restart.death_pid, 42);
        assert_eq!(restart.start_pid, Some(77));
        assert_eq!(restart.uid, 10_123);
        assert_eq!(restart.timing, RestartTimingFacet::AfterThirtySeconds);
        assert_eq!(
            restart.fingerprint,
            LifecycleFingerprintInput {
                process: ProcessNameToken::new("com.example.app").unwrap(),
                relation: LifecycleRelation::Restart,
                signal: None,
            }
        );
    }

    #[test]
    fn thirty_seconds_is_only_a_facet_and_never_a_relation_cutoff() {
        let mut recognizer = LifecycleRecognizer::new();
        recognizer
            .observe_activity_manager(
                1,
                b"Start proc 10:com.example.app/u0a1 for service",
                time(0),
            )
            .unwrap();
        recognizer
            .observe_activity_manager(2, b"Process com.example.app (pid 10) has died", time(1_000))
            .unwrap();
        let at_30s = recognizer
            .observe_activity_manager(
                3,
                b"Start proc 11:com.example.app/u0a1 for service",
                time(31_000),
            )
            .unwrap()
            .occurrence
            .unwrap();
        assert_eq!(at_30s.timing, RestartTimingFacet::WithinThirtySeconds);
    }

    #[test]
    fn uid_user_and_process_are_required_and_pid_reuse_does_not_name_match() {
        let mut recognizer = LifecycleRecognizer::new();
        assert!(recognizer
            .observe_activity_manager(1, b"Process com.example.app (pid 42) has died", None,)
            .unwrap()
            .occurrence
            .is_none());
        assert!(recognizer
            .observe_activity_manager(
                2,
                b"Start proc 42:com.example.app/u0a123 for activity",
                None,
            )
            .unwrap()
            .occurrence
            .is_none());

        recognizer
            .observe_activity_manager(3, b"Process com.example.app (pid 42) has died", None)
            .unwrap();
        assert!(recognizer
            .observe_activity_manager(
                4,
                b"Start proc 42:com.example.app/u0a124 for activity",
                None,
            )
            .unwrap()
            .occurrence
            .is_none());
        let matched = recognizer
            .observe_activity_manager(
                5,
                b"Start proc 99:com.example.app/u0a123 for activity",
                None,
            )
            .unwrap();
        assert!(matched.occurrence.is_some());
    }

    #[test]
    fn known_eventlog_can_establish_lifecycle_but_inferred_is_supporting_only() {
        let mut recognizer = LifecycleRecognizer::new();
        let start = EventLogRecord::ProcStart(AmProcStart {
            schema: EventLogSchemaId::AmProcStartUserPrefixed,
            user_id: Some(0),
            pid: 42,
            uid: 10_123,
            process_name: "com.example.app",
            start_type: "activity",
            component: "com.example/.Main",
        });
        let supporting = recognizer
            .observe_event_log(1, start, LineProvenance::Inferred(LogBuffer::Events), None)
            .unwrap();
        assert_eq!(
            supporting.observation.unwrap().strength,
            ObservationStrength::SupportingOnly
        );
        let death = EventLogRecord::ProcDied(AmProcDied {
            schema: EventLogSchemaId::AmProcDiedUserPrefixed,
            user_id: Some(0),
            pid: 42,
            process_name: "com.example.app",
            oom_adj: None,
            proc_state: None,
        });
        recognizer
            .observe_event_log(2, death, LineProvenance::Known(LogBuffer::Events), None)
            .unwrap();
        assert_eq!(recognizer.pending_count(), 0);

        recognizer
            .observe_event_log(3, start, LineProvenance::Known(LogBuffer::Events), None)
            .unwrap();
        recognizer
            .observe_event_log(4, death, LineProvenance::Known(LogBuffer::Events), None)
            .unwrap();
        assert_eq!(recognizer.pending_count(), 1);
    }

    #[test]
    fn am_kill_is_supporting_only_and_never_claims_death() {
        let mut recognizer = LifecycleRecognizer::new();
        let kill = EventLogRecord::Kill(AmKill {
            schema: EventLogSchemaId::AmKillUserPrefixed,
            user_id: Some(0),
            pid: 42,
            process_name: "com.example.app",
            oom_adj: 900,
            reason: "background",
            rss: None,
        });
        let delta = recognizer
            .observe_event_log(1, kill, LineProvenance::Known(LogBuffer::Events), None)
            .unwrap();
        assert_eq!(
            delta.observation.unwrap().kind,
            LifecycleObservationKind::KillRequest
        );
        assert_eq!(
            delta.observation.unwrap().strength,
            ObservationStrength::SupportingOnly
        );
        assert!(delta.occurrence.is_none());
    }

    #[test]
    fn conflicting_known_users_do_not_create_a_restart_pending_identity() {
        let mut recognizer = LifecycleRecognizer::new();
        let start = EventLogRecord::ProcStart(AmProcStart {
            schema: EventLogSchemaId::AmProcStartUserPrefixed,
            user_id: Some(0),
            pid: 42,
            uid: 10_123,
            process_name: "com.example.app",
            start_type: "activity",
            component: "com.example/.Main",
        });
        let conflicting_death = EventLogRecord::ProcDied(AmProcDied {
            schema: EventLogSchemaId::AmProcDiedUserPrefixed,
            user_id: Some(10),
            pid: 42,
            process_name: "com.example.app",
            oom_adj: None,
            proc_state: None,
        });
        recognizer
            .observe_event_log(1, start, LineProvenance::Known(LogBuffer::Events), None)
            .unwrap();
        recognizer
            .observe_event_log(
                2,
                conflicting_death,
                LineProvenance::Known(LogBuffer::Events),
                None,
            )
            .unwrap();
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn eventlog_start_uid_must_belong_to_the_declared_user() {
        let mut recognizer = LifecycleRecognizer::new();
        let inconsistent = EventLogRecord::ProcStart(AmProcStart {
            schema: EventLogSchemaId::AmProcStartUserPrefixed,
            user_id: Some(10),
            pid: 42,
            uid: 10_123,
            process_name: "com.example.app",
            start_type: "activity",
            component: "com.example/.Main",
        });
        assert_eq!(
            recognizer
                .observe_event_log(
                    1,
                    inconsistent,
                    LineProvenance::Known(LogBuffer::Events),
                    None,
                )
                .unwrap(),
            LifecycleDelta::default()
        );
        assert!(recognizer.tracker().active_for_pid(42).is_none());
    }

    #[test]
    fn explicit_signal_exit_requires_active_pid_identity() {
        let mut recognizer = LifecycleRecognizer::new();
        assert!(recognizer
            .observe_signal_exit(1, 42, 9)
            .unwrap()
            .occurrence
            .is_none());
        recognizer
            .observe_activity_manager(
                2,
                b"Start proc 42:com.example.app/u0a123 for activity",
                None,
            )
            .unwrap();
        assert!(recognizer
            .observe_signal_exit(3, 42, 65)
            .unwrap()
            .occurrence
            .is_none());
        let signal = recognizer
            .observe_signal_exit(4, 42, 9)
            .unwrap()
            .occurrence
            .unwrap();
        assert_eq!(signal.relation, LifecycleRelation::SignalExit);
        assert_eq!(signal.signal, Some(9));
        assert!(signal.started_instance.is_none());
    }

    #[test]
    fn zygote_signal_exit_grammar_is_exact_and_bounded() {
        assert_eq!(
            parse_zygote_signal_exit(b"Process 42 exited due to signal 9 (Killed)"),
            Some((42, 9))
        );
        for message in [
            b"Process 42 exited cleanly (0)".as_slice(),
            b"Process 42 exited due to signal 0 (Unknown)".as_slice(),
            b"Process 42 exited due to signal 65 (Unknown)".as_slice(),
            b"Process 42 exited due to signal 9".as_slice(),
            b"Process 42 exited due to signal 9 (Killed) trailing".as_slice(),
            b"process 42 exited due to signal 9 (Killed)".as_slice(),
        ] {
            assert_eq!(parse_zygote_signal_exit(message), None);
        }
    }

    #[test]
    fn malformed_truncated_and_near_match_lines_do_not_change_state() {
        let mut recognizer = LifecycleRecognizer::new();
        for message in [
            b"Start proc ".as_slice(),
            b"Start proc 42:com.example.app/u0a123".as_slice(),
            b"Process com.example.app (pid 42) has died unexpectedly".as_slice(),
            b"Process com.example.app has died".as_slice(),
            b"process com.example.app (pid 42) has died".as_slice(),
            b"\xffProcess com.example.app (pid 42) has died".as_slice(),
        ] {
            assert_eq!(
                recognizer
                    .observe_activity_manager(1, message, None)
                    .unwrap(),
                LifecycleDelta::default()
            );
        }
        assert_eq!(recognizer.pending_count(), 0);
        assert_eq!(recognizer.finish_input(), 0);
    }

    #[test]
    fn pending_table_is_fixed_and_evicts_oldest_without_heap_owned_payloads() {
        assert_eq!(size_of::<ProcessNameToken>(), MAX_PROCESS_NAME_BYTES);
        assert!(!needs_drop::<ProcessNameToken>());
        let mut recognizer = LifecycleRecognizer::new();
        for index in 1..=MAX_PENDING_DEATHS + 1 {
            let process = format!("com.example.app{index}");
            let start = format!("Start proc {}:{process}/u0a{} for service", index, index);
            let death = format!("Process {process} (pid {index}) has died");
            recognizer
                .observe_activity_manager(index as u32, start.as_bytes(), None)
                .unwrap();
            recognizer
                .observe_activity_manager((index + 100) as u32, death.as_bytes(), None)
                .unwrap();
        }
        assert_eq!(recognizer.pending_count(), MAX_PENDING_DEATHS);
        assert_eq!(recognizer.pending_eviction_count(), 1);
        assert_eq!(recognizer.finish_input(), MAX_PENDING_DEATHS as u8);
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn fingerprint_input_excludes_pid_epoch_and_elapsed_time() {
        let process = ProcessNameToken::new("com.example.app").unwrap();
        let first = LifecycleFingerprintInput {
            process,
            relation: LifecycleRelation::Restart,
            signal: None,
        };
        let second = LifecycleFingerprintInput {
            process,
            relation: LifecycleRelation::Restart,
            signal: None,
        };
        assert_eq!(first, second);
        let _opaque_key = ProcessInstanceKey(7);
    }
}
