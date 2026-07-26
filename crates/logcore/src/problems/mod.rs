mod classifier;
mod eventlog;
mod facts;
mod fingerprint;
mod index;
mod model;
mod process_instance;
mod provenance;

pub use classifier::{classify_candidate, CandidateKinds};
pub use eventlog::{
    parse_event_log, AmAnr, AmCrash, AmKill, AmProcDied, AmProcStart, AmbiguousSchemaMatches,
    EventLogParseError, EventLogRecord, EventLogSchemaId, MalformedEventLog, SchemaMatch,
};
pub use facts::{
    fact_code, EvidencePriority, FactCode, FactMappingError, ObservationCandidate, ObservationRef,
    ObservationRole, RuleId,
};
pub use fingerprint::{
    FingerprintBuilder, FingerprintTokenKind, ProblemFingerprint, ProcessFingerprintKey,
};
pub use index::{
    AppendDropReason, AppendOutcome, GroupId, GroupKey, GroupPage, GroupQuery, GroupSort,
    OccurrencePage, PageSpec, PageSpecError, ProblemGroupSummary, ProblemIndex, ProblemIndexError,
    ProblemIndexLimits, ProblemIndexLimitsError, ProblemStats, QuerySnapshotId, SnapshotError,
};
pub use model::{
    internal_line_index, public_line_number, BoundaryFlags, EvidenceFlags, IdentityQuality,
    LineNumberError, OutcomeFlags, PackedLogTimestamp, ProblemEvent, ProblemEventDraft,
    ProblemEventError, ProblemEventId, ProblemKind, ProcessInstanceKey, SignatureQuality,
};
pub use process_instance::{
    ProcessEpochOrigin, ProcessIdentity, ProcessIdentityError, ProcessInstance,
    ProcessInstanceTracker, ProcessTrackerError, ProcessTrackerLimitsError, ProcessTrackerStats,
    TerminatedProcessInstance, TrackedProcessInstance, MAX_ACTIVE_PROCESS_INSTANCES,
    MAX_RECENT_TERMINATED_INSTANCES,
};
pub use provenance::{
    BufferSet, CaptureOrigin, EvidenceAdmission, EvidenceFormat, InputCoverage, LineProvenance,
    LogBuffer, RangeCompleteness, SourceSpan, SourceSpanError, SourceSpanIndex,
};
