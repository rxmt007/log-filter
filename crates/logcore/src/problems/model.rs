use std::ops::{BitOr, BitOrAssign};

pub const MAX_MATERIALIZED_OBSERVATIONS: u8 = 8;
pub const MAX_ADOPTED_OBSERVATIONS: u16 = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ProblemKind {
    JavaCrash = 0,
    JavaOom = 1,
    Anr = 2,
    NativeCrash = 3,
    ProcessRestart = 4,
    SignalExit = 5,
    LmkKill = 6,
    KernelOomKill = 7,
}

impl ProblemKind {
    pub const ALL: [Self; 8] = [
        Self::JavaCrash,
        Self::JavaOom,
        Self::Anr,
        Self::NativeCrash,
        Self::ProcessRestart,
        Self::SignalExit,
        Self::LmkKill,
        Self::KernelOomKill,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SignatureQuality {
    FullStack = 0,
    TypeFile = 1,
    TypeOnly = 2,
    SignalOnly = 3,
    StructuredFields = 4,
    Minimal = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum IdentityQuality {
    KnownProcess = 0,
    UnknownProcess = 1,
}

macro_rules! flag_type {
    ($name:ident, $storage:ty) => {
        #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name($storage);

        impl $name {
            pub const NONE: Self = Self(0);

            pub const fn from_bits(bits: $storage) -> Self {
                Self(bits)
            }

            pub const fn bits(self) -> $storage {
                self.0
            }

            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            pub fn insert(&mut self, other: Self) {
                self.0 |= other.0;
            }
        }

        impl BitOr for $name {
            type Output = Self;

            fn bitor(self, rhs: Self) -> Self::Output {
                Self(self.0 | rhs.0)
            }
        }

        impl BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }
    };
}

flag_type!(EvidenceFlags, u16);
flag_type!(OutcomeFlags, u8);
flag_type!(BoundaryFlags, u8);

impl EvidenceFlags {
    pub const PRIMARY: Self = Self(1 << 0);
    pub const STRUCTURED: Self = Self(1 << 1);
    pub const MULTILINE: Self = Self(1 << 2);
    pub const CORRELATED: Self = Self(1 << 3);
}

impl OutcomeFlags {
    pub const KILL_REQUESTED: Self = Self(1 << 0);
    pub const KILL_ISSUED: Self = Self(1 << 1);
    pub const DEATH_OBSERVED: Self = Self(1 << 2);
    pub const START_AFTER_DEATH_OBSERVED: Self = Self(1 << 3);
    pub const EXPLICITLY_RECOVERABLE: Self = Self(1 << 4);
    pub const CONFLICT: Self = Self(1 << 5);
}

impl BoundaryFlags {
    pub const TRUNCATED_BY_INPUT: Self = Self(1 << 0);
    pub const OBSERVATION_REFS_TRUNCATED: Self = Self(1 << 1);
    pub const OBSERVATION_COUNT_LIMITED: Self = Self(1 << 2);
    pub const LINE_INDEX_OVERFLOW: Self = Self(1 << 3);
    pub const CORRELATION_LIMITED: Self = Self(1 << 4);
    pub const TRUNCATED_BY_LIMIT: Self = Self(1 << 5);
}

/// An optional packed Android log timestamp. Zero represents an unavailable timestamp.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PackedLogTimestamp(u64);

impl PackedLogTimestamp {
    pub const UNKNOWN: Self = Self(0);

    pub fn new(month: u8, day: u8, hour: u8, minute: u8, second: u8, millis: u16) -> Option<Self> {
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 59
            || millis > 999
        {
            return None;
        }
        let packed = (((((u64::from(month) * 32 + u64::from(day)) * 24 + u64::from(hour)) * 60
            + u64::from(minute))
            * 60
            + u64::from(second))
            * 1_000
            + u64::from(millis))
            + 1;
        Some(Self(packed))
    }

    pub const fn is_known(self) -> bool {
        self.0 != 0
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ProblemEventId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ProcessInstanceKey(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineNumberError {
    Zero,
    IndexOverflow,
}

/// Convert the public 1-based line convention to the compact internal 0-based index.
pub fn internal_line_index(public_line: u64) -> Result<u32, LineNumberError> {
    if public_line == 0 {
        return Err(LineNumberError::Zero);
    }
    u32::try_from(public_line - 1).map_err(|_| LineNumberError::IndexOverflow)
}

pub const fn public_line_number(internal_line: u32) -> u64 {
    internal_line as u64 + 1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemEventDraft {
    pub start_line: u32,
    pub end_line: u32,
    pub anchor_line: u32,
    pub anchor_timestamp: PackedLogTimestamp,
    pub pid: u32,
    pub process_instance: ProcessInstanceKey,
    pub kind: ProblemKind,
    pub evidence: EvidenceFlags,
    pub outcome: OutcomeFlags,
    pub boundary: BoundaryFlags,
}

impl ProblemEventDraft {
    pub const fn minimal(kind: ProblemKind) -> Self {
        Self {
            start_line: 0,
            end_line: 0,
            anchor_line: 0,
            anchor_timestamp: PackedLogTimestamp::UNKNOWN,
            pid: 0,
            process_instance: ProcessInstanceKey(0),
            kind,
            evidence: EvidenceFlags::NONE,
            outcome: OutcomeFlags::NONE,
            boundary: BoundaryFlags::NONE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemEventError {
    InvalidRange,
    InvalidAnchor,
    TooManyMaterializedObservations,
    InvalidObservationTotal,
}

/// Compact occurrence metadata. Raw log text and fingerprints live outside this record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct ProblemEvent {
    anchor_timestamp: PackedLogTimestamp,
    start_line: u32,
    end_line: u32,
    anchor_line: u32,
    pid: u32,
    process_instance: u32,
    group_id: u32,
    observation_start: u32,
    observation_total: u16,
    evidence: EvidenceFlags,
    kind: ProblemKind,
    observation_len: u8,
    outcome: OutcomeFlags,
    boundary: BoundaryFlags,
}

impl ProblemEvent {
    pub fn new(
        draft: ProblemEventDraft,
        group_id: u32,
        observation_start: u32,
        observation_len: u8,
        observation_total: u16,
    ) -> Result<Self, ProblemEventError> {
        if draft.start_line > draft.end_line {
            return Err(ProblemEventError::InvalidRange);
        }
        if !(draft.start_line..=draft.end_line).contains(&draft.anchor_line) {
            return Err(ProblemEventError::InvalidAnchor);
        }
        if observation_len > MAX_MATERIALIZED_OBSERVATIONS {
            return Err(ProblemEventError::TooManyMaterializedObservations);
        }
        if observation_total > MAX_ADOPTED_OBSERVATIONS
            || u16::from(observation_len) > observation_total
        {
            return Err(ProblemEventError::InvalidObservationTotal);
        }
        Ok(Self {
            anchor_timestamp: draft.anchor_timestamp,
            start_line: draft.start_line,
            end_line: draft.end_line,
            anchor_line: draft.anchor_line,
            pid: draft.pid,
            process_instance: draft.process_instance.0,
            group_id,
            observation_start,
            observation_total,
            evidence: draft.evidence,
            kind: draft.kind,
            observation_len,
            outcome: draft.outcome,
            boundary: draft.boundary,
        })
    }

    pub const fn start_line(self) -> u32 {
        self.start_line
    }

    pub const fn end_line(self) -> u32 {
        self.end_line
    }

    pub const fn anchor_line(self) -> u32 {
        self.anchor_line
    }

    pub const fn anchor_timestamp(self) -> PackedLogTimestamp {
        self.anchor_timestamp
    }

    pub const fn pid(self) -> u32 {
        self.pid
    }

    pub const fn process_instance(self) -> ProcessInstanceKey {
        ProcessInstanceKey(self.process_instance)
    }

    pub const fn group_id_raw(self) -> u32 {
        self.group_id
    }

    pub const fn observation_start(self) -> u32 {
        self.observation_start
    }

    pub const fn observation_len(self) -> u8 {
        self.observation_len
    }

    pub const fn observation_total(self) -> u16 {
        self.observation_total
    }

    pub const fn kind(self) -> ProblemKind {
        self.kind
    }

    pub const fn evidence(self) -> EvidenceFlags {
        self.evidence
    }

    pub const fn outcome(self) -> OutcomeFlags {
        self.outcome
    }

    pub const fn boundary(self) -> BoundaryFlags {
        self.boundary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{needs_drop, size_of};

    #[test]
    fn problem_event_is_compact_and_owns_no_heap_data() {
        assert!(size_of::<ProblemEvent>() <= 48);
        assert!(!needs_drop::<ProblemEvent>());
    }

    #[test]
    fn public_and_internal_line_numbers_have_explicit_bounds() {
        assert_eq!(internal_line_index(1), Ok(0));
        assert_eq!(internal_line_index(u32::MAX as u64 + 1), Ok(u32::MAX));
        assert_eq!(
            internal_line_index(u32::MAX as u64 + 2),
            Err(LineNumberError::IndexOverflow)
        );
        assert_eq!(internal_line_index(0), Err(LineNumberError::Zero));
        assert_eq!(public_line_number(u32::MAX), u32::MAX as u64 + 1);
    }

    #[test]
    fn event_constructor_rejects_an_invalid_range() {
        let draft = ProblemEventDraft {
            start_line: 12,
            end_line: 11,
            anchor_line: 12,
            ..ProblemEventDraft::minimal(ProblemKind::Anr)
        };

        assert_eq!(
            ProblemEvent::new(draft, 1, 0, 0, 0),
            Err(ProblemEventError::InvalidRange)
        );
    }

    #[test]
    fn observation_counts_enforce_materialized_and_adopted_limits() {
        let draft = ProblemEventDraft::minimal(ProblemKind::JavaCrash);
        assert!(ProblemEvent::new(draft, 0, 0, 8, 4_096).is_ok());
        assert_eq!(
            ProblemEvent::new(draft, 0, 0, 9, 9),
            Err(ProblemEventError::TooManyMaterializedObservations)
        );
        assert_eq!(
            ProblemEvent::new(draft, 0, 0, 8, 4_097),
            Err(ProblemEventError::InvalidObservationTotal)
        );
        assert_eq!(
            ProblemEvent::new(draft, 0, 0, 8, 7),
            Err(ProblemEventError::InvalidObservationTotal)
        );
    }

    #[test]
    fn input_and_detector_limit_truncation_have_distinct_bits() {
        assert_ne!(
            BoundaryFlags::TRUNCATED_BY_INPUT.bits(),
            BoundaryFlags::TRUNCATED_BY_LIMIT.bits()
        );
    }
}
