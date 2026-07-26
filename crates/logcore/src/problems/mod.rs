mod classifier;
mod engine;
mod eventlog;
mod facts;
mod fingerprint;
mod index;
mod model;
mod normalization;
mod process_instance;
mod provenance;
mod recognizers;
mod timestamp;

pub use classifier::{classify_candidate, CandidateKinds};
pub use engine::{ObservedLine, ProblemDelta, ProblemEngine};
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
pub use normalization::{
    normalize_anr_reason, normalize_java_frame, normalize_java_throwable,
    normalize_kernel_oom_mechanism, normalize_lmk_reason, normalize_native_frame,
    AnrReasonCategory, NormalizedAnrReason, NormalizedToken, MAX_NORMALIZATION_INPUT_BYTES,
    MAX_NORMALIZED_TOKEN_BYTES,
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
pub use timestamp::{
    parse_log_timestamp, SegmentedTimestamp, TimestampSegmentId, TimestampSegmentTracker,
};
