use super::provenance::{EvidenceFormat, LineProvenance};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum RuleId {
    JavaUncaughtV1 = 1,
    JavaOomV1 = 2,
    ManagedAmCrashV1 = 3,
    AnrActivityManagerV1 = 4,
    NativeTombstoneV1 = 5,
    NativeLibcSignalV1 = 6,
    ProcessStartV1 = 7,
    ProcessDiedV1 = 8,
    ProcessRestartV1 = 9,
    SignalExitV1 = 10,
    LmkdKillV1 = 11,
    KernelOomKillV1 = 12,
    AmKillRequestV1 = 13,
}

impl RuleId {
    pub const ALL: [Self; 13] = [
        Self::JavaUncaughtV1,
        Self::JavaOomV1,
        Self::ManagedAmCrashV1,
        Self::AnrActivityManagerV1,
        Self::NativeTombstoneV1,
        Self::NativeLibcSignalV1,
        Self::ProcessStartV1,
        Self::ProcessDiedV1,
        Self::ProcessRestartV1,
        Self::SignalExitV1,
        Self::LmkdKillV1,
        Self::KernelOomKillV1,
        Self::AmKillRequestV1,
    ];

    pub const fn supported_roles(self) -> &'static [ObservationRole] {
        use ObservationRole::{
            BacktraceFrame, Death, ExceptionType, KillIssued, KillRequest, Primary,
            ProcessIdentity, Reason, Recovery, Restart, Signal, StackFrame, Start, Supporting,
        };
        match self {
            Self::JavaUncaughtV1 | Self::JavaOomV1 => &[
                Primary,
                ProcessIdentity,
                ExceptionType,
                StackFrame,
                Death,
                Supporting,
            ],
            Self::ManagedAmCrashV1 => &[Primary, ProcessIdentity, Death, Supporting],
            Self::AnrActivityManagerV1 => &[Primary, ProcessIdentity, Reason, Death, Supporting],
            Self::NativeTombstoneV1 => &[
                Primary,
                ProcessIdentity,
                Signal,
                BacktraceFrame,
                Recovery,
                Death,
                Supporting,
            ],
            Self::NativeLibcSignalV1 => &[Primary, ProcessIdentity, Signal, Death, Supporting],
            Self::ProcessStartV1 => &[Primary, ProcessIdentity, Start],
            Self::ProcessDiedV1 => &[Primary, ProcessIdentity, Death],
            Self::ProcessRestartV1 => &[Primary, ProcessIdentity, Death, Restart],
            Self::SignalExitV1 => &[Primary, ProcessIdentity, Signal, Death],
            Self::LmkdKillV1 | Self::KernelOomKillV1 => {
                &[Primary, ProcessIdentity, KillIssued, Death, Supporting]
            }
            Self::AmKillRequestV1 => &[Primary, ProcessIdentity, KillRequest, Supporting],
        }
    }

    const fn from_raw(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::JavaUncaughtV1),
            2 => Some(Self::JavaOomV1),
            3 => Some(Self::ManagedAmCrashV1),
            4 => Some(Self::AnrActivityManagerV1),
            5 => Some(Self::NativeTombstoneV1),
            6 => Some(Self::NativeLibcSignalV1),
            7 => Some(Self::ProcessStartV1),
            8 => Some(Self::ProcessDiedV1),
            9 => Some(Self::ProcessRestartV1),
            10 => Some(Self::SignalExitV1),
            11 => Some(Self::LmkdKillV1),
            12 => Some(Self::KernelOomKillV1),
            13 => Some(Self::AmKillRequestV1),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ObservationRole {
    Primary = 0,
    ProcessIdentity = 1,
    ExceptionType = 2,
    StackFrame = 3,
    Reason = 4,
    Signal = 5,
    BacktraceFrame = 6,
    Start = 7,
    Death = 8,
    Restart = 9,
    KillRequest = 10,
    KillIssued = 11,
    Supporting = 12,
    Recovery = 13,
}

impl ObservationRole {
    pub const ALL: [Self; 14] = [
        Self::Primary,
        Self::ProcessIdentity,
        Self::ExceptionType,
        Self::StackFrame,
        Self::Reason,
        Self::Signal,
        Self::BacktraceFrame,
        Self::Start,
        Self::Death,
        Self::Restart,
        Self::KillRequest,
        Self::KillIssued,
        Self::Supporting,
        Self::Recovery,
    ];

    const fn from_packed(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Primary),
            1 => Some(Self::ProcessIdentity),
            2 => Some(Self::ExceptionType),
            3 => Some(Self::StackFrame),
            4 => Some(Self::Reason),
            5 => Some(Self::Signal),
            6 => Some(Self::BacktraceFrame),
            7 => Some(Self::Start),
            8 => Some(Self::Death),
            9 => Some(Self::Restart),
            10 => Some(Self::KillRequest),
            11 => Some(Self::KillIssued),
            12 => Some(Self::Supporting),
            13 => Some(Self::Recovery),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum FactCode {
    JavaUncaughtException = 1,
    JavaOutOfMemoryError = 2,
    ManagedCrashRecord = 3,
    AnrDetected = 4,
    NativeCrashDetected = 5,
    SignalExitDetected = 6,
    ProcessStarted = 7,
    ProcessDied = 8,
    ProcessRestarted = 9,
    LmkKillIssued = 10,
    KernelOomKillIssued = 11,
    KillRequested = 12,
    ProcessIdentityRecorded = 13,
    ExceptionTypeRecorded = 14,
    StackFrameRecorded = 15,
    AnrReasonRecorded = 16,
    FatalSignalRecorded = 17,
    NativeFrameRecorded = 18,
    ProcessDeathObserved = 19,
    StartAfterDeathObserved = 20,
    NativeRecoveryRecorded = 21,
    SupportingEvidenceRecorded = 22,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactMappingError {
    DetectorBug { rule: RuleId, role: ObservationRole },
}

pub fn fact_code(rule: RuleId, role: ObservationRole) -> Result<FactCode, FactMappingError> {
    if !rule.supported_roles().contains(&role) {
        return Err(FactMappingError::DetectorBug { rule, role });
    }

    use FactCode::{
        AnrDetected, AnrReasonRecorded, ExceptionTypeRecorded, FatalSignalRecorded,
        JavaOutOfMemoryError, JavaUncaughtException, KernelOomKillIssued, KillRequested,
        LmkKillIssued, ManagedCrashRecord, NativeCrashDetected, NativeFrameRecorded,
        NativeRecoveryRecorded, ProcessDeathObserved, ProcessDied, ProcessIdentityRecorded,
        ProcessRestarted, ProcessStarted, SignalExitDetected, StackFrameRecorded,
        StartAfterDeathObserved, SupportingEvidenceRecorded,
    };
    use ObservationRole::{
        BacktraceFrame, Death, ExceptionType, KillIssued, KillRequest, Primary, ProcessIdentity,
        Reason, Recovery, Restart, Signal, StackFrame, Start, Supporting,
    };

    let code = match (rule, role) {
        (_, ProcessIdentity) => ProcessIdentityRecorded,
        (_, Death) => ProcessDeathObserved,
        (_, Supporting) => SupportingEvidenceRecorded,
        (RuleId::JavaUncaughtV1, Primary) => JavaUncaughtException,
        (RuleId::JavaOomV1, Primary) => JavaOutOfMemoryError,
        (RuleId::ManagedAmCrashV1, Primary) => ManagedCrashRecord,
        (RuleId::AnrActivityManagerV1, Primary) => AnrDetected,
        (RuleId::NativeTombstoneV1, Primary) => NativeCrashDetected,
        (RuleId::NativeLibcSignalV1 | RuleId::SignalExitV1, Primary) => SignalExitDetected,
        (RuleId::ProcessStartV1, Primary | Start) => ProcessStarted,
        (RuleId::ProcessDiedV1, Primary) => ProcessDied,
        (RuleId::ProcessRestartV1, Primary) => ProcessRestarted,
        (RuleId::LmkdKillV1, Primary | KillIssued) => LmkKillIssued,
        (RuleId::KernelOomKillV1, Primary | KillIssued) => KernelOomKillIssued,
        (RuleId::AmKillRequestV1, Primary | KillRequest) => KillRequested,
        (RuleId::JavaUncaughtV1 | RuleId::JavaOomV1, ExceptionType) => ExceptionTypeRecorded,
        (RuleId::JavaUncaughtV1 | RuleId::JavaOomV1, StackFrame) => StackFrameRecorded,
        (RuleId::AnrActivityManagerV1, Reason) => AnrReasonRecorded,
        (RuleId::NativeTombstoneV1 | RuleId::NativeLibcSignalV1 | RuleId::SignalExitV1, Signal) => {
            FatalSignalRecorded
        }
        (RuleId::NativeTombstoneV1, BacktraceFrame) => NativeFrameRecorded,
        (RuleId::NativeTombstoneV1, Recovery) => NativeRecoveryRecorded,
        (RuleId::ProcessRestartV1, Restart) => StartAfterDeathObserved,
        // The validity guard above makes this branch unreachable for published combinations.
        _ => return Err(FactMappingError::DetectorBug { rule, role }),
    };
    Ok(code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum EvidencePriority {
    MinimumGrammar = 0,
    Outcome = 1,
    Correlation = 2,
    Supporting = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationCandidate {
    pub reference: ObservationRef,
    pub priority: EvidencePriority,
}

impl ObservationCandidate {
    pub const fn new(reference: ObservationRef, priority: EvidencePriority) -> Self {
        Self {
            reference,
            priority,
        }
    }
}

/// Compact pointer into the original source. Fact text is derived from rule + role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ObservationRef {
    line: u32,
    rule: u16,
    role_and_format: u8,
    source_and_provenance: u8,
}

impl ObservationRef {
    pub fn new(
        line: u32,
        rule: RuleId,
        role: ObservationRole,
        format: EvidenceFormat,
        provenance: LineProvenance,
    ) -> Result<Self, FactMappingError> {
        fact_code(rule, role)?;
        Ok(Self {
            line,
            rule: rule as u16,
            role_and_format: (format as u8) << 4 | role as u8,
            source_and_provenance: provenance.pack(),
        })
    }

    pub const fn line(self) -> u32 {
        self.line
    }

    pub fn rule(self) -> RuleId {
        RuleId::from_raw(self.rule).expect("ObservationRef is only constructed from a valid RuleId")
    }

    pub fn role(self) -> ObservationRole {
        ObservationRole::from_packed(self.role_and_format & 0x0f)
            .expect("ObservationRef is only constructed from a valid ObservationRole")
    }

    pub fn format(self) -> EvidenceFormat {
        EvidenceFormat::from_packed(self.role_and_format >> 4)
            .expect("ObservationRef is only constructed from a valid EvidenceFormat")
    }

    pub fn provenance(self) -> LineProvenance {
        LineProvenance::unpack(self.source_and_provenance)
            .expect("ObservationRef is only constructed from a valid LineProvenance")
    }

    pub fn fact(self) -> FactCode {
        fact_code(self.rule(), self.role())
            .expect("ObservationRef constructor already validated the fact mapping")
    }

    pub fn dedup_key(self) -> (u32, RuleId, ObservationRole) {
        (self.line, self.rule(), self.role())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::{EvidenceFormat, LineProvenance, LogBuffer};
    use std::mem::size_of;

    #[test]
    fn observation_ref_has_the_frozen_eight_byte_layout() {
        assert_eq!(size_of::<ObservationRef>(), 8);
    }

    #[test]
    fn observation_ref_round_trips_compact_fields_and_derives_fact() {
        let reference = ObservationRef::new(
            123,
            RuleId::NativeTombstoneV1,
            ObservationRole::Signal,
            EvidenceFormat::TombstoneShapedText,
            LineProvenance::Known(LogBuffer::Crash),
        )
        .unwrap();

        assert_eq!(reference.line(), 123);
        assert_eq!(reference.rule(), RuleId::NativeTombstoneV1);
        assert_eq!(reference.role(), ObservationRole::Signal);
        assert_eq!(reference.format(), EvidenceFormat::TombstoneShapedText);
        assert_eq!(
            reference.provenance(),
            LineProvenance::Known(LogBuffer::Crash)
        );
        assert_eq!(reference.fact(), FactCode::FatalSignalRecorded);
    }

    #[test]
    fn every_published_rule_role_has_a_total_fact_mapping() {
        for rule in RuleId::ALL {
            for role in ObservationRole::ALL {
                assert_eq!(
                    fact_code(rule, role).is_ok(),
                    rule.supported_roles().contains(&role),
                    "mapping contract drift for {rule:?}/{role:?}"
                );
            }
        }
    }

    #[test]
    fn unpublished_rule_role_combination_is_a_detector_bug() {
        assert_eq!(
            fact_code(RuleId::JavaUncaughtV1, ObservationRole::Reason),
            Err(FactMappingError::DetectorBug {
                rule: RuleId::JavaUncaughtV1,
                role: ObservationRole::Reason,
            })
        );
    }

    #[test]
    fn deduplication_key_is_line_rule_and_role_only() {
        let first = ObservationRef::new(
            9,
            RuleId::AnrActivityManagerV1,
            ObservationRole::Primary,
            EvidenceFormat::AospText,
            LineProvenance::Unknown,
        )
        .unwrap();
        let second = ObservationRef::new(
            9,
            RuleId::AnrActivityManagerV1,
            ObservationRole::Primary,
            EvidenceFormat::EventLogShapedText,
            LineProvenance::Inferred(LogBuffer::Events),
        )
        .unwrap();

        assert_eq!(first.dedup_key(), second.dedup_key());
    }
}
