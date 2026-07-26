mod budget;
mod classifier;
mod correlation;
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

pub use budget::{
    ProblemMemoryBudget, ProblemMemoryBudgetError, ProblemMemoryStats,
    DEFAULT_PROBLEM_MEMORY_BUDGET_BYTES,
};
pub use classifier::{classify_candidate, CandidateKinds};
pub use correlation::{
    CompactCorrelationPayload, CorrelationLimitsError, CorrelationSequenceExhausted,
    FinalizedProvisional, ProvisionalEntry, ProvisionalFinalizeReason, ProvisionalInsertOutcome,
    ProvisionalLimits, ProvisionalStats, ProvisionalStore, RecentInsertOutcome, RecentObservation,
    RecentObservationLimits, RecentObservationStats, RecentObservationStore, MAX_PROVISIONAL_BYTES,
    MAX_PROVISIONAL_OCCURRENCES, MAX_RECENT_OBSERVATIONS, MAX_RECENT_OBSERVATION_BYTES,
};
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
    AppendDropReason, AppendOutcome, BoundedProblemSummary, GroupId, GroupKey, GroupPage,
    GroupQuery, GroupSnapshotCapture, GroupSort, GroupSortRecord, OccurrencePage, PageSpec,
    PageSpecError, ProblemDisplaySummary, ProblemGroupSummary, ProblemIndex, ProblemIndexError,
    ProblemIndexLimits, ProblemIndexLimitsError, ProblemProcessSummary, ProblemSignatureSummary,
    ProblemStats, QuerySnapshotId, SnapshotError, MAX_PROBLEM_PROCESS_SUMMARY_BYTES,
    MAX_PROBLEM_SIGNATURE_SUMMARY_BYTES,
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
pub(crate) use provenance::BufferProvenanceTracker;
pub use provenance::{
    BufferSet, CaptureOrigin, EvidenceAdmission, EvidenceFormat, InputCoverage, LineProvenance,
    LogBuffer, RangeCompleteness, SourceSpan, SourceSpanError, SourceSpanIndex,
};
pub use timestamp::{
    parse_log_timestamp, SegmentedTimestamp, TimestampSegmentId, TimestampSegmentTracker,
};
