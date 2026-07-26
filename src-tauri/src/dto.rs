use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 传给前端的一行(camelCase 对齐 TS 类型)。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub line_no: u64,
    pub date: String,
    pub time: String,
    pub level: String,
    pub pid: String,
    pub tid: String,
    pub tag: String,
    pub message: String,
    pub marked: bool,
}

/// 会话状态快照。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub total_lines: usize,
    pub stable_lines: usize,
    pub filtered_lines: usize,
    pub bookmark_lines: usize,
    pub error_lines: usize,
    pub indexed_bytes: u64,
    pub total_bytes: u64,
    pub indexing: bool,
    pub generation: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AdbDeviceDto {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
    pub product: Option<String>,
    pub online: bool,
}

impl From<logcore::adb::AdbDevice> for AdbDeviceDto {
    fn from(value: logcore::adb::AdbDevice) -> Self {
        let online = value.online();
        Self {
            serial: value.serial,
            state: value.state,
            model: value.model,
            product: value.product,
            online,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListDto {
    pub adb_path: Option<String>,
    pub devices: Vec<AdbDeviceDto>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StartLogcatRequest {
    pub device_serial: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub buffers: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamAppendDto {
    pub appended_bytes: u64,
    pub status: Status,
    pub device_serial: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamControlDto {
    pub status: Status,
    pub running: bool,
    pub paused: bool,
    pub device_serial: Option<String>,
    pub session_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FilterFieldDto {
    pub enabled: bool,
    pub pattern: String,
    pub regex: bool,
}

impl From<FilterFieldDto> for logcore::filter::FilterField {
    fn from(value: FilterFieldDto) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HighlightRuleDto {
    pub enabled: bool,
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub color: String,
}

impl From<HighlightRuleDto> for logcore::filter::HighlightRule {
    fn from(value: HighlightRuleDto) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
            case_sensitive: value.case_sensitive,
            color: value.color,
        }
    }
}

impl From<logcore::filter::HighlightRule> for HighlightRuleDto {
    fn from(value: logcore::filter::HighlightRule) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
            case_sensitive: value.case_sensitive,
            color: value.color,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FilterSpecDto {
    pub levels: u8,
    pub marked_only: bool,
    pub pid: FilterFieldDto,
    pub tid: FilterFieldDto,
    pub tag_include: FilterFieldDto,
    pub tag_exclude: FilterFieldDto,
    pub word_include: FilterFieldDto,
    pub word_exclude: FilterFieldDto,
    #[serde(default)]
    pub highlights: Vec<HighlightRuleDto>,
}

impl From<FilterSpecDto> for logcore::filter::FilterSpec {
    fn from(value: FilterSpecDto) -> Self {
        Self {
            levels: logcore::filter::LevelMask::from_bits(value.levels),
            marked_only: value.marked_only,
            pid: value.pid.into(),
            tid: value.tid.into(),
            tag_include: value.tag_include.into(),
            tag_exclude: value.tag_exclude.into(),
            word_include: value.word_include.into(),
            word_exclude: value.word_exclude.into(),
            highlights: value.highlights.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<logcore::filter::FilterSpec> for FilterSpecDto {
    fn from(value: logcore::filter::FilterSpec) -> Self {
        Self {
            levels: value.levels.bits(),
            marked_only: value.marked_only,
            pid: FilterFieldDto::from(value.pid),
            tid: FilterFieldDto::from(value.tid),
            tag_include: FilterFieldDto::from(value.tag_include),
            tag_exclude: FilterFieldDto::from(value.tag_exclude),
            word_include: FilterFieldDto::from(value.word_include),
            word_exclude: FilterFieldDto::from(value.word_exclude),
            highlights: value.highlights.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<logcore::filter::FilterField> for FilterFieldDto {
    fn from(value: logcore::filter::FilterField) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchSpecDto {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

impl From<SearchSpecDto> for logcore::search::SearchSpec {
    fn from(value: SearchSpecDto) -> Self {
        Self {
            query: value.query,
            regex: value.regex,
            case_sensitive: value.case_sensitive,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub count: usize,
    pub first_line: Option<u64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FilterDoneDto {
    pub filtered_lines: usize,
    pub generation: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgressDto {
    pub scanned: usize,
    pub matches: usize,
    pub first_line: Option<u64>,
    pub done: bool,
    pub generation: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MinimapBucketDto {
    pub bucket: usize,
    pub count: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MinimapDto {
    pub bookmarks: Vec<usize>,
    pub errors: Vec<MinimapBucketDto>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NavigationTargetDto {
    pub line_no: u64,
    pub result_index: usize,
}

impl From<logcore::session::ResultTarget> for NavigationTargetDto {
    fn from(value: logcore::session::ResultTarget) -> Self {
        Self {
            line_no: value.line_no,
            result_index: value.result_index,
        }
    }
}

// This DTO slice is intentionally landed before its command consumers.
#[allow(dead_code)]
mod problem_dtos {
    use super::*;

    /// Identifies one Problems analysis inside one input session.
    ///
    /// Every Problems query carries this token so an event or snapshot from an old
    /// file/analysis cannot be resolved against the current session.
    #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct AnalysisTokenDto {
        pub session_generation: u64,
        pub analysis_generation: u64,
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemStatsDto {
        pub observed_occurrence_count: u64,
        pub stored_occurrence_count: u64,
        pub dropped_occurrence_count: u64,
        pub provisional_occurrence_count: u32,
        pub stored_group_count: u32,
        pub ungrouped_dropped_occurrence_count: u64,
        pub dropped_recent_observation_count: u64,
        pub revision: u64,
        pub limited: bool,
        pub correlation_limited: bool,
    }

    impl ProblemStatsDto {
        /// Adds the bounded engine/session counters which intentionally do not live
        /// in the compact persisted index statistics.
        pub const fn from_compact(
            stats: logcore::problems::ProblemStats,
            provisional_occurrence_count: u32,
            dropped_recent_observation_count: u64,
            correlation_limited: bool,
        ) -> Self {
            Self {
                observed_occurrence_count: stats.observed_occurrence_count,
                stored_occurrence_count: stats.stored_occurrence_count,
                dropped_occurrence_count: stats.dropped_occurrence_count,
                provisional_occurrence_count,
                stored_group_count: stats.stored_group_count,
                ungrouped_dropped_occurrence_count: stats.ungrouped_dropped_occurrence_count,
                dropped_recent_observation_count,
                revision: stats.revision,
                limited: stats.limited || correlation_limited,
                correlation_limited,
            }
        }
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemsStatusDto {
        pub analysis_token: AnalysisTokenDto,
        pub scanned_lines: u64,
        pub stable_lines: u64,
        pub scanning: bool,
        pub finished: bool,
        pub stats: ProblemStatsDto,
    }

    #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum ProblemKindDto {
        JavaCrash,
        JavaOom,
        Anr,
        NativeCrash,
        ProcessRestart,
        SignalExit,
        LmkKill,
        KernelOomKill,
    }

    #[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum ProblemGroupSortDto {
        #[default]
        LastSeenDesc,
        CountDesc,
    }

    #[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemGroupQueryRequest {
        pub expected_analysis_token: AnalysisTokenDto,
        #[serde(default)]
        pub kind: Option<ProblemKindDto>,
        #[serde(default)]
        pub sort: ProblemGroupSortDto,
        #[serde(default)]
        pub query_snapshot_id: Option<u64>,
        #[serde(default)]
        pub offset: Option<usize>,
        #[serde(default)]
        pub limit: Option<usize>,
    }

    #[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemOccurrenceQueryRequest {
        pub expected_analysis_token: AnalysisTokenDto,
        pub group_id: u32,
        #[serde(default)]
        pub query_snapshot_id: Option<u64>,
        #[serde(default)]
        pub offset: Option<usize>,
        #[serde(default)]
        pub limit: Option<usize>,
    }

    #[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemDetailRequest {
        pub event_id: u32,
        pub expected_analysis_token: AnalysisTokenDto,
    }

    #[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemReleaseSnapshotRequest {
        pub query_snapshot_id: u64,
        pub expected_analysis_token: AnalysisTokenDto,
    }

    impl From<logcore::problems::ProblemKind> for ProblemKindDto {
        fn from(value: logcore::problems::ProblemKind) -> Self {
            use logcore::problems::ProblemKind;
            match value {
                ProblemKind::JavaCrash => Self::JavaCrash,
                ProblemKind::JavaOom => Self::JavaOom,
                ProblemKind::Anr => Self::Anr,
                ProblemKind::NativeCrash => Self::NativeCrash,
                ProblemKind::ProcessRestart => Self::ProcessRestart,
                ProblemKind::SignalExit => Self::SignalExit,
                ProblemKind::LmkKill => Self::LmkKill,
                ProblemKind::KernelOomKill => Self::KernelOomKill,
            }
        }
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum SignatureQualityDto {
        FullStack,
        TypeFile,
        TypeOnly,
        SignalOnly,
        StructuredFields,
        Minimal,
    }

    impl From<logcore::problems::SignatureQuality> for SignatureQualityDto {
        fn from(value: logcore::problems::SignatureQuality) -> Self {
            use logcore::problems::SignatureQuality;
            match value {
                SignatureQuality::FullStack => Self::FullStack,
                SignatureQuality::TypeFile => Self::TypeFile,
                SignatureQuality::TypeOnly => Self::TypeOnly,
                SignatureQuality::SignalOnly => Self::SignalOnly,
                SignatureQuality::StructuredFields => Self::StructuredFields,
                SignatureQuality::Minimal => Self::Minimal,
            }
        }
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum IdentityQualityDto {
        KnownProcess,
        UnknownProcess,
    }

    impl From<logcore::problems::IdentityQuality> for IdentityQualityDto {
        fn from(value: logcore::problems::IdentityQuality) -> Self {
            match value {
                logcore::problems::IdentityQuality::KnownProcess => Self::KnownProcess,
                logcore::problems::IdentityQuality::UnknownProcess => Self::UnknownProcess,
            }
        }
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemGroupSummaryDto {
        pub id: u32,
        pub kind: ProblemKindDto,
        pub fingerprint_version: u16,
        pub signature_quality: SignatureQualityDto,
        pub identity_quality: IdentityQualityDto,
        /// A fixed-size BLAKE3-128 display value. The frontend must not interpret it.
        pub fingerprint: String,
        pub observed_occurrence_count: u64,
        pub stored_occurrence_count: u64,
        pub dropped_occurrence_count: u64,
        pub first_line: u64,
        pub first_timestamp: Option<String>,
        pub last_line: u64,
        pub last_timestamp: Option<String>,
        pub first_event_id: Option<u32>,
        pub last_event_id: Option<u32>,
        pub representative_event_id: Option<u32>,
    }

    impl From<logcore::problems::ProblemGroupSummary> for ProblemGroupSummaryDto {
        fn from(value: logcore::problems::ProblemGroupSummary) -> Self {
            Self {
                id: value.id.raw(),
                kind: value.key.kind().into(),
                fingerprint_version: value.key.fingerprint_version(),
                signature_quality: value.key.signature_quality().into(),
                identity_quality: value.key.identity_quality().into(),
                fingerprint: value.key.fingerprint().to_hex(),
                observed_occurrence_count: value.observed_occurrence_count,
                stored_occurrence_count: value.stored_occurrence_count,
                dropped_occurrence_count: value.dropped_occurrence_count,
                first_line: logcore::problems::public_line_number(value.first_observed_line),
                first_timestamp: format_problem_timestamp(value.first_observed_timestamp),
                last_line: logcore::problems::public_line_number(value.last_observed_line),
                last_timestamp: format_problem_timestamp(value.last_observed_timestamp),
                first_event_id: value.first_stored_event_id.map(|id| id.0),
                last_event_id: value.last_stored_event_id.map(|id| id.0),
                representative_event_id: value.representative_stored_event_id.map(|id| id.0),
            }
        }
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemGroupPageDto {
        pub analysis_token: AnalysisTokenDto,
        pub query_snapshot_id: u64,
        pub revision: u64,
        pub total: u64,
        pub items: Vec<ProblemGroupSummaryDto>,
        pub next_offset: Option<u64>,
    }

    impl ProblemGroupPageDto {
        /// Page size is validated by the command boundary. This conversion
        /// deliberately preserves all compact page items and does not silently
        /// clamp or fetch any log text.
        pub fn from_compact(
            analysis_token: AnalysisTokenDto,
            page: logcore::problems::GroupPage,
        ) -> Self {
            Self {
                analysis_token,
                query_snapshot_id: page.snapshot_id.raw(),
                revision: page.revision,
                total: usize_to_u64(page.total),
                items: page.items.into_iter().map(Into::into).collect(),
                next_offset: page.next_offset.map(usize_to_u64),
            }
        }
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum EvidenceFlagDto {
        Primary,
        Structured,
        Multiline,
        Correlated,
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum OutcomeFlagDto {
        KillRequested,
        KillIssued,
        DeathObserved,
        StartAfterDeathObserved,
        ExplicitlyRecoverable,
        Conflict,
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum BoundaryFlagDto {
        TruncatedByInput,
        ObservationRefsTruncated,
        ObservationCountLimited,
        LineIndexOverflow,
        CorrelationLimited,
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemOccurrenceDto {
        pub event_id: u32,
        pub group_id: u32,
        pub kind: ProblemKindDto,
        pub start_line: u64,
        pub end_line: u64,
        pub anchor_line: u64,
        pub timestamp: Option<String>,
        pub pid: Option<u32>,
        pub process_instance_id: u32,
        pub evidence_flags: Vec<EvidenceFlagDto>,
        pub outcome_flags: Vec<OutcomeFlagDto>,
        pub boundary_flags: Vec<BoundaryFlagDto>,
    }

    impl ProblemOccurrenceDto {
        pub fn from_compact(
            event_id: logcore::problems::ProblemEventId,
            event: logcore::problems::ProblemEvent,
        ) -> Self {
            Self {
                event_id: event_id.0,
                group_id: event.group_id_raw(),
                kind: event.kind().into(),
                start_line: logcore::problems::public_line_number(event.start_line()),
                end_line: logcore::problems::public_line_number(event.end_line()),
                anchor_line: logcore::problems::public_line_number(event.anchor_line()),
                timestamp: format_problem_timestamp(event.anchor_timestamp()),
                pid: (event.pid() != 0).then_some(event.pid()),
                process_instance_id: event.process_instance().0,
                evidence_flags: evidence_flag_dtos(event.evidence()),
                outcome_flags: outcome_flag_dtos(event.outcome()),
                boundary_flags: boundary_flag_dtos(event.boundary()),
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MissingProblemEventDto {
        pub event_id: u32,
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemOccurrencePageDto {
        pub analysis_token: AnalysisTokenDto,
        pub query_snapshot_id: u64,
        pub revision: u64,
        pub total: u64,
        pub items: Vec<ProblemOccurrenceDto>,
        pub next_offset: Option<u64>,
    }

    impl ProblemOccurrencePageDto {
        /// Resolves only compact event metadata. A missing opaque event id is an
        /// explicit error rather than a silently shortened page.
        pub fn try_from_compact(
            analysis_token: AnalysisTokenDto,
            page: logcore::problems::OccurrencePage,
            mut event_by_id: impl FnMut(
                logcore::problems::ProblemEventId,
            ) -> Option<logcore::problems::ProblemEvent>,
        ) -> Result<Self, MissingProblemEventDto> {
            let mut items = Vec::with_capacity(page.items.len());
            for event_id in page.items {
                let event = event_by_id(event_id).ok_or(MissingProblemEventDto {
                    event_id: event_id.0,
                })?;
                items.push(ProblemOccurrenceDto::from_compact(event_id, event));
            }
            Ok(Self {
                analysis_token,
                query_snapshot_id: page.snapshot_id.raw(),
                revision: page.revision,
                total: usize_to_u64(page.total),
                items,
                next_offset: page.next_offset.map(usize_to_u64),
            })
        }
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum ProblemFactCodeDto {
        JavaUncaughtException,
        JavaOutOfMemoryError,
        ManagedCrashRecord,
        AnrDetected,
        NativeCrashDetected,
        SignalExitDetected,
        ProcessStarted,
        ProcessDied,
        ProcessRestarted,
        LmkKillIssued,
        KernelOomKillIssued,
        KillRequested,
        ProcessIdentityRecorded,
        ExceptionTypeRecorded,
        StackFrameRecorded,
        AnrReasonRecorded,
        FatalSignalRecorded,
        NativeFrameRecorded,
        ProcessDeathObserved,
        StartAfterDeathObserved,
        NativeRecoveryRecorded,
        SupportingEvidenceRecorded,
    }

    impl From<logcore::problems::FactCode> for ProblemFactCodeDto {
        fn from(value: logcore::problems::FactCode) -> Self {
            use logcore::problems::FactCode;
            match value {
                FactCode::JavaUncaughtException => Self::JavaUncaughtException,
                FactCode::JavaOutOfMemoryError => Self::JavaOutOfMemoryError,
                FactCode::ManagedCrashRecord => Self::ManagedCrashRecord,
                FactCode::AnrDetected => Self::AnrDetected,
                FactCode::NativeCrashDetected => Self::NativeCrashDetected,
                FactCode::SignalExitDetected => Self::SignalExitDetected,
                FactCode::ProcessStarted => Self::ProcessStarted,
                FactCode::ProcessDied => Self::ProcessDied,
                FactCode::ProcessRestarted => Self::ProcessRestarted,
                FactCode::LmkKillIssued => Self::LmkKillIssued,
                FactCode::KernelOomKillIssued => Self::KernelOomKillIssued,
                FactCode::KillRequested => Self::KillRequested,
                FactCode::ProcessIdentityRecorded => Self::ProcessIdentityRecorded,
                FactCode::ExceptionTypeRecorded => Self::ExceptionTypeRecorded,
                FactCode::StackFrameRecorded => Self::StackFrameRecorded,
                FactCode::AnrReasonRecorded => Self::AnrReasonRecorded,
                FactCode::FatalSignalRecorded => Self::FatalSignalRecorded,
                FactCode::NativeFrameRecorded => Self::NativeFrameRecorded,
                FactCode::ProcessDeathObserved => Self::ProcessDeathObserved,
                FactCode::StartAfterDeathObserved => Self::StartAfterDeathObserved,
                FactCode::NativeRecoveryRecorded => Self::NativeRecoveryRecorded,
                FactCode::SupportingEvidenceRecorded => Self::SupportingEvidenceRecorded,
            }
        }
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum ObservationRoleDto {
        Primary,
        ProcessIdentity,
        ExceptionType,
        StackFrame,
        Reason,
        Signal,
        BacktraceFrame,
        Start,
        Death,
        Restart,
        KillRequest,
        KillIssued,
        Supporting,
        Recovery,
    }

    impl From<logcore::problems::ObservationRole> for ObservationRoleDto {
        fn from(value: logcore::problems::ObservationRole) -> Self {
            use logcore::problems::ObservationRole;
            match value {
                ObservationRole::Primary => Self::Primary,
                ObservationRole::ProcessIdentity => Self::ProcessIdentity,
                ObservationRole::ExceptionType => Self::ExceptionType,
                ObservationRole::StackFrame => Self::StackFrame,
                ObservationRole::Reason => Self::Reason,
                ObservationRole::Signal => Self::Signal,
                ObservationRole::BacktraceFrame => Self::BacktraceFrame,
                ObservationRole::Start => Self::Start,
                ObservationRole::Death => Self::Death,
                ObservationRole::Restart => Self::Restart,
                ObservationRole::KillRequest => Self::KillRequest,
                ObservationRole::KillIssued => Self::KillIssued,
                ObservationRole::Supporting => Self::Supporting,
                ObservationRole::Recovery => Self::Recovery,
            }
        }
    }

    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum EvidenceFormatDto {
        #[serde(rename = "aosp-text")]
        Aosp,
        #[serde(rename = "event-log-shaped-text")]
        EventLogShaped,
        #[serde(rename = "tombstone-shaped-text")]
        TombstoneShaped,
        #[serde(rename = "kernel-shaped-text")]
        KernelShaped,
    }

    impl From<logcore::problems::EvidenceFormat> for EvidenceFormatDto {
        fn from(value: logcore::problems::EvidenceFormat) -> Self {
            match value {
                logcore::problems::EvidenceFormat::AospText => Self::Aosp,
                logcore::problems::EvidenceFormat::EventLogShapedText => Self::EventLogShaped,
                logcore::problems::EvidenceFormat::TombstoneShapedText => Self::TombstoneShaped,
                logcore::problems::EvidenceFormat::KernelShapedText => Self::KernelShaped,
            }
        }
    }

    /// A flattened, finite provenance vocabulary keeps the wire value stable and
    /// prevents arbitrary buffer names from leaking into the API.
    #[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum LineProvenanceDto {
        Unknown,
        InferredMain,
        InferredSystem,
        InferredEvents,
        InferredCrash,
        InferredRadio,
        InferredKernel,
        KnownMain,
        KnownSystem,
        KnownEvents,
        KnownCrash,
        KnownRadio,
        KnownKernel,
    }

    impl From<logcore::problems::LineProvenance> for LineProvenanceDto {
        fn from(value: logcore::problems::LineProvenance) -> Self {
            use logcore::problems::{LineProvenance, LogBuffer};
            match value {
                LineProvenance::Unknown => Self::Unknown,
                LineProvenance::Inferred(LogBuffer::Main) => Self::InferredMain,
                LineProvenance::Inferred(LogBuffer::System) => Self::InferredSystem,
                LineProvenance::Inferred(LogBuffer::Events) => Self::InferredEvents,
                LineProvenance::Inferred(LogBuffer::Crash) => Self::InferredCrash,
                LineProvenance::Inferred(LogBuffer::Radio) => Self::InferredRadio,
                LineProvenance::Inferred(LogBuffer::Kernel) => Self::InferredKernel,
                LineProvenance::Known(LogBuffer::Main) => Self::KnownMain,
                LineProvenance::Known(LogBuffer::System) => Self::KnownSystem,
                LineProvenance::Known(LogBuffer::Events) => Self::KnownEvents,
                LineProvenance::Known(LogBuffer::Crash) => Self::KnownCrash,
                LineProvenance::Known(LogBuffer::Radio) => Self::KnownRadio,
                LineProvenance::Known(LogBuffer::Kernel) => Self::KnownKernel,
            }
        }
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemFactDto {
        pub code: ProblemFactCodeDto,
        pub source_line: u64,
        /// Versioned detector identifier, for example `aosp.java-uncaught.v1`.
        pub rule_id: String,
        pub role: ObservationRoleDto,
        pub evidence_format: EvidenceFormatDto,
        pub provenance: LineProvenanceDto,
    }

    impl From<logcore::problems::ObservationRef> for ProblemFactDto {
        fn from(value: logcore::problems::ObservationRef) -> Self {
            Self {
                code: value.fact().into(),
                source_line: logcore::problems::public_line_number(value.line()),
                rule_id: problem_rule_id(value.rule()).to_owned(),
                role: value.role().into(),
                evidence_format: value.format().into(),
                provenance: value.provenance().into(),
            }
        }
    }

    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemDetailDto {
        pub analysis_token: AnalysisTokenDto,
        pub revision: u64,
        pub occurrence: ProblemOccurrenceDto,
        pub facts: Vec<ProblemFactDto>,
        pub facts_truncated: bool,
        pub observation_total: u16,
    }

    impl ProblemDetailDto {
        pub fn from_compact(
            analysis_token: AnalysisTokenDto,
            revision: u64,
            event_id: logcore::problems::ProblemEventId,
            event: logcore::problems::ProblemEvent,
            observations: &[logcore::problems::ObservationRef],
        ) -> Self {
            const MAX_FACTS: usize = 8;
            let materialized_len = usize::from(event.observation_len()).min(MAX_FACTS);
            let facts = observations
                .iter()
                .take(materialized_len)
                .copied()
                .map(Into::into)
                .collect::<Vec<_>>();
            let facts_truncated = event
                .boundary()
                .contains(logcore::problems::BoundaryFlags::OBSERVATION_REFS_TRUNCATED)
                || usize::from(event.observation_total()) > facts.len();
            Self {
                analysis_token,
                revision,
                occurrence: ProblemOccurrenceDto::from_compact(event_id, event),
                facts,
                facts_truncated,
                observation_total: event.observation_total(),
            }
        }
    }

    #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case")]
    pub enum ProblemExportModeDto {
        EventRange,
        Context,
    }

    #[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemExportRequest {
        pub event_id: u32,
        pub expected_analysis_token: AnalysisTokenDto,
        pub mode: ProblemExportModeDto,
        #[serde(default)]
        pub radius: Option<u32>,
        pub path: String,
    }

    /// Counter-only progress payload. Event/group arrays are deliberately absent.
    #[derive(Serialize, Clone, Debug, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub struct ProblemsProgressDto {
        pub scanned_lines: u64,
        pub stable_lines: u64,
        pub observed_occurrence_count: u64,
        pub stored_occurrence_count: u64,
        pub dropped_occurrence_count: u64,
        pub provisional_occurrence_count: u32,
        pub stored_group_count: u32,
        pub ungrouped_dropped_occurrence_count: u64,
        pub dropped_recent_observation_count: u64,
        pub correlation_limited: bool,
        pub revision: u64,
        pub done: bool,
        pub limited: bool,
        pub session_generation: u64,
        pub analysis_generation: u64,
    }

    fn usize_to_u64(value: usize) -> u64 {
        u64::try_from(value).expect("supported targets have usize no wider than u64")
    }

    fn format_problem_timestamp(
        timestamp: logcore::problems::PackedLogTimestamp,
    ) -> Option<String> {
        if !timestamp.is_known() {
            return None;
        }
        let mut value = timestamp.raw() - 1;
        let millis = value % 1_000;
        value /= 1_000;
        let second = value % 60;
        value /= 60;
        let minute = value % 60;
        value /= 60;
        let hour = value % 24;
        value /= 24;
        let day = value % 32;
        let month = value / 32;
        Some(format!(
            "{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03}"
        ))
    }

    fn evidence_flag_dtos(flags: logcore::problems::EvidenceFlags) -> Vec<EvidenceFlagDto> {
        use logcore::problems::EvidenceFlags;
        let mut values = Vec::with_capacity(4);
        if flags.contains(EvidenceFlags::PRIMARY) {
            values.push(EvidenceFlagDto::Primary);
        }
        if flags.contains(EvidenceFlags::STRUCTURED) {
            values.push(EvidenceFlagDto::Structured);
        }
        if flags.contains(EvidenceFlags::MULTILINE) {
            values.push(EvidenceFlagDto::Multiline);
        }
        if flags.contains(EvidenceFlags::CORRELATED) {
            values.push(EvidenceFlagDto::Correlated);
        }
        values
    }

    fn outcome_flag_dtos(flags: logcore::problems::OutcomeFlags) -> Vec<OutcomeFlagDto> {
        use logcore::problems::OutcomeFlags;
        let mut values = Vec::with_capacity(6);
        if flags.contains(OutcomeFlags::KILL_REQUESTED) {
            values.push(OutcomeFlagDto::KillRequested);
        }
        if flags.contains(OutcomeFlags::KILL_ISSUED) {
            values.push(OutcomeFlagDto::KillIssued);
        }
        if flags.contains(OutcomeFlags::DEATH_OBSERVED) {
            values.push(OutcomeFlagDto::DeathObserved);
        }
        if flags.contains(OutcomeFlags::START_AFTER_DEATH_OBSERVED) {
            values.push(OutcomeFlagDto::StartAfterDeathObserved);
        }
        if flags.contains(OutcomeFlags::EXPLICITLY_RECOVERABLE) {
            values.push(OutcomeFlagDto::ExplicitlyRecoverable);
        }
        if flags.contains(OutcomeFlags::CONFLICT) {
            values.push(OutcomeFlagDto::Conflict);
        }
        values
    }

    fn boundary_flag_dtos(flags: logcore::problems::BoundaryFlags) -> Vec<BoundaryFlagDto> {
        use logcore::problems::BoundaryFlags;
        let mut values = Vec::with_capacity(5);
        if flags.contains(BoundaryFlags::TRUNCATED_BY_INPUT) {
            values.push(BoundaryFlagDto::TruncatedByInput);
        }
        if flags.contains(BoundaryFlags::OBSERVATION_REFS_TRUNCATED) {
            values.push(BoundaryFlagDto::ObservationRefsTruncated);
        }
        if flags.contains(BoundaryFlags::OBSERVATION_COUNT_LIMITED) {
            values.push(BoundaryFlagDto::ObservationCountLimited);
        }
        if flags.contains(BoundaryFlags::LINE_INDEX_OVERFLOW) {
            values.push(BoundaryFlagDto::LineIndexOverflow);
        }
        if flags.contains(BoundaryFlags::CORRELATION_LIMITED) {
            values.push(BoundaryFlagDto::CorrelationLimited);
        }
        values
    }

    fn problem_rule_id(rule: logcore::problems::RuleId) -> &'static str {
        use logcore::problems::RuleId;
        match rule {
            RuleId::JavaUncaughtV1 => "aosp.java-uncaught.v1",
            RuleId::JavaOomV1 => "aosp.java-oom.v1",
            RuleId::ManagedAmCrashV1 => "aosp.am-crash.v1",
            RuleId::AnrActivityManagerV1 => "aosp.anr-activity-manager.v1",
            RuleId::NativeTombstoneV1 => "aosp.native-tombstone.v1",
            RuleId::NativeLibcSignalV1 => "aosp.native-libc-signal.v1",
            RuleId::ProcessStartV1 => "aosp.process-start.v1",
            RuleId::ProcessDiedV1 => "aosp.process-died.v1",
            RuleId::ProcessRestartV1 => "aosp.process-restart.v1",
            RuleId::SignalExitV1 => "aosp.signal-exit.v1",
            RuleId::LmkdKillV1 => "aosp.lmkd-kill.v1",
            RuleId::KernelOomKillV1 => "aosp.kernel-oom-kill.v1",
            RuleId::AmKillRequestV1 => "aosp.am-kill-request.v1",
        }
    }
}

#[allow(unused_imports)]
pub use problem_dtos::*;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub mode: String,
    pub view: Option<String>,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub path: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummaryDto {
    pub written_lines: usize,
    pub written_bytes: u64,
    pub cancelled: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgressDto {
    pub written_lines: usize,
    pub written_bytes: u64,
    pub done: bool,
    /// 输出文件路径:仅最终成功事件(done=true 且未取消)携带,进度中为 None。
    pub path: Option<String>,
    /// 取消标记:仅最终取消事件携带 true,进度中为 false。
    pub cancelled: bool,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SplitRequest {
    pub path: String,
    pub out_dir: String,
    pub mode: String,
    pub value: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SplitProgressDto {
    pub parts: usize,
    pub bytes_processed: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SplitSummaryDto {
    pub parts: Vec<String>,
    pub total_bytes: u64,
}

impl From<logcore::split::SplitSummary> for SplitSummaryDto {
    fn from(value: logcore::split::SplitSummary) -> Self {
        Self {
            parts: value
                .parts
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            total_bytes: value.total_bytes,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TableColumnConfigDto {
    pub id: String,
    pub width: u16,
    pub visible: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TableConfigDto {
    pub columns: Vec<TableColumnConfigDto>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WindowConfigDto {
    pub width: u16,
    pub height: u16,
}

impl From<logcore::config::WindowConfig> for WindowConfigDto {
    fn from(value: logcore::config::WindowConfig) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

impl From<WindowConfigDto> for logcore::config::WindowConfig {
    fn from(value: WindowConfigDto) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

impl Default for TableConfigDto {
    fn default() -> Self {
        logcore::config::TableConfig::default().into()
    }
}

impl From<logcore::config::TableConfig> for TableConfigDto {
    fn from(value: logcore::config::TableConfig) -> Self {
        Self {
            columns: value
                .columns
                .into_iter()
                .map(|column| TableColumnConfigDto {
                    id: column.id,
                    width: column.width,
                    visible: column.visible,
                })
                .collect(),
        }
    }
}

impl From<TableConfigDto> for logcore::config::TableConfig {
    fn from(value: TableConfigDto) -> Self {
        Self {
            columns: value
                .columns
                .into_iter()
                .map(|column| logcore::config::TableColumnConfig {
                    id: column.id,
                    width: column.width,
                    visible: column.visible,
                })
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub theme: String,
    pub adb_path: Option<String>,
    pub storage_dir: Option<String>,
    pub encoding: String,
    pub font_size: u16,
    pub row_height: u16,
    #[serde(default)]
    pub table: TableConfigDto,
    #[serde(default)]
    pub recent_files: Vec<String>,
    #[serde(default)]
    pub last_filter: Option<FilterSpecDto>,
    #[serde(default)]
    pub command_buffers: Vec<String>,
    #[serde(default)]
    pub current_command: String,
    #[serde(default)]
    pub command_presets: Vec<String>,
    #[serde(default = "default_window_config")]
    pub window: WindowConfigDto,
    pub config_path: String,
}

fn default_window_config() -> WindowConfigDto {
    logcore::config::WindowConfig::default().into()
}

impl AppConfigDto {
    pub fn from_config(config: logcore::config::AppConfig, config_path: PathBuf) -> Self {
        Self {
            theme: match config.theme {
                logcore::config::ThemeMode::Light => "light".to_string(),
                logcore::config::ThemeMode::Dark => "dark".to_string(),
            },
            adb_path: config
                .adb_path
                .map(|path| path.to_string_lossy().to_string()),
            storage_dir: config
                .storage_dir
                .map(|path| path.to_string_lossy().to_string()),
            encoding: config.encoding,
            font_size: config.font_size,
            row_height: config.row_height,
            table: config.table.into(),
            recent_files: config
                .recent_files
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            last_filter: Some(config.last_filter.into()),
            command_buffers: config.command_buffers,
            current_command: config.current_command,
            command_presets: config.command_presets,
            window: config.window.into(),
            config_path: config_path.to_string_lossy().to_string(),
        }
    }
}

impl TryFrom<AppConfigDto> for logcore::config::AppConfig {
    type Error = String;

    fn try_from(value: AppConfigDto) -> Result<Self, Self::Error> {
        let theme = match value.theme.as_str() {
            "dark" => logcore::config::ThemeMode::Dark,
            "light" => logcore::config::ThemeMode::Light,
            other => return Err(format!("unsupported theme: {other}")),
        };
        Ok(Self {
            theme,
            adb_path: value
                .adb_path
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            storage_dir: value
                .storage_dir
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            encoding: value.encoding,
            font_size: value.font_size,
            row_height: value.row_height,
            table: value.table.into(),
            recent_files: value
                .recent_files
                .into_iter()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect(),
            last_filter: value.last_filter.map(Into::into).unwrap_or_default(),
            command_buffers: value.command_buffers,
            current_command: value.current_command,
            command_presets: value.command_presets,
            window: value.window.into(),
        }
        .normalized())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filter_done_event_serializes_with_camel_case_fields() {
        let payload = FilterDoneDto {
            filtered_lines: 12,
            generation: 7,
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({"filteredLines": 12, "generation": 7})
        );
    }

    #[test]
    fn search_progress_event_serializes_with_first_line() {
        let payload = SearchProgressDto {
            scanned: 100,
            matches: 3,
            first_line: Some(42),
            done: true,
            generation: 9,
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "scanned": 100,
                "matches": 3,
                "firstLine": 42,
                "done": true,
                "generation": 9
            })
        );
    }

    #[test]
    fn stream_append_event_serializes_nested_status() {
        let payload = StreamAppendDto {
            appended_bytes: 128,
            status: Status {
                total_lines: 2,
                stable_lines: 2,
                filtered_lines: 2,
                bookmark_lines: 0,
                error_lines: 1,
                indexed_bytes: 128,
                total_bytes: 128,
                indexing: false,
                generation: 4,
            },
            device_serial: "usb".to_string(),
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "appendedBytes": 128,
                "status": {
                    "totalLines": 2,
                    "stableLines": 2,
                    "filteredLines": 2,
                    "bookmarkLines": 0,
                    "errorLines": 1,
                    "indexedBytes": 128,
                    "totalBytes": 128,
                    "indexing": false,
                    "generation": 4
                },
                "deviceSerial": "usb"
            })
        );
    }

    #[test]
    fn app_config_dto_rejects_unknown_theme_and_normalizes_numbers() {
        let config = AppConfigDto {
            theme: "dark".to_string(),
            adb_path: Some(String::new()),
            storage_dir: Some(String::new()),
            encoding: String::new(),
            font_size: 99,
            row_height: 1,
            table: TableConfigDto::default(),
            recent_files: Vec::new(),
            last_filter: None,
            command_buffers: vec!["kernel".to_string(), "events".to_string()],
            current_command: String::new(),
            command_presets: vec![
                "logcat -v threadtime -b radio".to_string(),
                "logcat -v time".to_string(),
            ],
            window: WindowConfigDto {
                width: 1,
                height: 9999,
            },
            config_path: String::new(),
        };

        let converted = logcore::config::AppConfig::try_from(config).unwrap();
        assert_eq!(converted.theme, logcore::config::ThemeMode::Dark);
        assert_eq!(converted.adb_path, None);
        assert_eq!(converted.storage_dir, None);
        assert_eq!(converted.encoding, "UTF-8");
        assert_eq!(converted.font_size, 20);
        assert_eq!(converted.row_height, 16);
        assert_eq!(converted.command_buffers, vec!["events"]);
        assert_eq!(converted.current_command, "logcat -v threadtime -b events");
        assert!(converted
            .command_presets
            .contains(&"logcat -v threadtime -b radio".to_string()));
        assert_eq!(converted.window.width, 960);
        assert_eq!(converted.window.height, 2160);

        let bad = AppConfigDto {
            theme: "system".to_string(),
            ..AppConfigDto::from_config(logcore::config::AppConfig::default(), PathBuf::new())
        };
        assert!(logcore::config::AppConfig::try_from(bad).is_err());
    }

    #[test]
    fn app_config_dto_round_trips_command_presets() {
        let config = logcore::config::AppConfig {
            current_command: "logcat -v threadtime -b radio".to_string(),
            command_presets: vec!["logcat -v threadtime -b radio".to_string()],
            ..Default::default()
        }
        .normalized();
        let dto = AppConfigDto::from_config(config.clone(), PathBuf::new());
        assert_eq!(dto.current_command, "logcat -v threadtime -b radio");
        assert!(dto
            .command_presets
            .contains(&"logcat -v threadtime -b radio".to_string()));

        let converted = logcore::config::AppConfig::try_from(dto).unwrap();
        assert_eq!(converted.current_command, config.current_command);
    }

    #[test]
    fn problems_status_and_progress_have_frozen_camel_case_fields() {
        let token = AnalysisTokenDto {
            session_generation: 7,
            analysis_generation: 3,
        };
        let stats = ProblemStatsDto {
            observed_occurrence_count: 11,
            stored_occurrence_count: 9,
            dropped_occurrence_count: 2,
            provisional_occurrence_count: 1,
            stored_group_count: 4,
            ungrouped_dropped_occurrence_count: 1,
            dropped_recent_observation_count: 5,
            revision: 13,
            limited: true,
            correlation_limited: true,
        };

        assert_eq!(
            serde_json::to_value(ProblemsStatusDto {
                analysis_token: token,
                scanned_lines: 4_096,
                stable_lines: 8_192,
                scanning: true,
                finished: false,
                stats: stats.clone(),
            })
            .unwrap(),
            json!({
                "analysisToken": {
                    "sessionGeneration": 7,
                    "analysisGeneration": 3
                },
                "scannedLines": 4_096,
                "stableLines": 8_192,
                "scanning": true,
                "finished": false,
                "stats": {
                    "observedOccurrenceCount": 11,
                    "storedOccurrenceCount": 9,
                    "droppedOccurrenceCount": 2,
                    "provisionalOccurrenceCount": 1,
                    "storedGroupCount": 4,
                    "ungroupedDroppedOccurrenceCount": 1,
                    "droppedRecentObservationCount": 5,
                    "revision": 13,
                    "limited": true,
                    "correlationLimited": true
                }
            })
        );

        assert_eq!(
            serde_json::to_value(ProblemsProgressDto {
                scanned_lines: 4_096,
                stable_lines: 8_192,
                observed_occurrence_count: 11,
                stored_occurrence_count: 9,
                dropped_occurrence_count: 2,
                provisional_occurrence_count: 1,
                stored_group_count: 4,
                ungrouped_dropped_occurrence_count: 1,
                dropped_recent_observation_count: 5,
                correlation_limited: true,
                revision: 13,
                done: false,
                limited: true,
                session_generation: 7,
                analysis_generation: 3,
            })
            .unwrap(),
            json!({
                "scannedLines": 4_096,
                "stableLines": 8_192,
                "observedOccurrenceCount": 11,
                "storedOccurrenceCount": 9,
                "droppedOccurrenceCount": 2,
                "provisionalOccurrenceCount": 1,
                "storedGroupCount": 4,
                "ungroupedDroppedOccurrenceCount": 1,
                "droppedRecentObservationCount": 5,
                "correlationLimited": true,
                "revision": 13,
                "done": false,
                "limited": true,
                "sessionGeneration": 7,
                "analysisGeneration": 3
            })
        );
    }

    #[test]
    fn problem_enums_and_export_mode_use_frozen_kebab_case_values() {
        let kinds = logcore::problems::ProblemKind::ALL
            .into_iter()
            .map(|kind| serde_json::to_value(ProblemKindDto::from(kind)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                json!("java-crash"),
                json!("java-oom"),
                json!("anr"),
                json!("native-crash"),
                json!("process-restart"),
                json!("signal-exit"),
                json!("lmk-kill"),
                json!("kernel-oom-kill"),
            ]
        );

        let request: ProblemExportRequest = serde_json::from_value(json!({
            "eventId": 17,
            "expectedAnalysisToken": {
                "sessionGeneration": 9,
                "analysisGeneration": 2
            },
            "mode": "event-range",
            "radius": 50,
            "path": "/tmp/problem.log"
        }))
        .unwrap();
        assert_eq!(request.event_id, 17);
        assert_eq!(request.mode, ProblemExportModeDto::EventRange);
        assert_eq!(request.radius, Some(50));
        assert_eq!(request.expected_analysis_token.session_generation, 9);

        assert!(serde_json::from_value::<ProblemExportRequest>(json!({
            "eventId": 17,
            "expectedAnalysisToken": {
                "sessionGeneration": 9,
                "analysisGeneration": 2
            },
            "mode": "guess",
            "path": "/tmp/problem.log"
        }))
        .is_err());
    }

    #[test]
    fn problem_query_requests_bind_pagination_to_analysis_and_snapshot() {
        let groups: ProblemGroupQueryRequest = serde_json::from_value(json!({
            "expectedAnalysisToken": {
                "sessionGeneration": 9,
                "analysisGeneration": 2
            },
            "kind": "anr",
            "sort": "count-desc",
            "querySnapshotId": 17,
            "offset": 100,
            "limit": 100
        }))
        .unwrap();
        assert_eq!(groups.kind, Some(ProblemKindDto::Anr));
        assert_eq!(groups.sort, ProblemGroupSortDto::CountDesc);
        assert_eq!(groups.query_snapshot_id, Some(17));

        let occurrences: ProblemOccurrenceQueryRequest = serde_json::from_value(json!({
            "expectedAnalysisToken": {
                "sessionGeneration": 9,
                "analysisGeneration": 2
            },
            "groupId": 3,
            "querySnapshotId": null
        }))
        .unwrap();
        assert_eq!(occurrences.group_id, 3);
        assert_eq!(occurrences.offset, None);
        assert_eq!(occurrences.limit, None);

        let detail: ProblemDetailRequest = serde_json::from_value(json!({
            "eventId": 7,
            "expectedAnalysisToken": {
                "sessionGeneration": 9,
                "analysisGeneration": 2
            }
        }))
        .unwrap();
        assert_eq!(detail.event_id, 7);
    }

    #[test]
    fn compact_group_page_conversion_is_one_based_and_contains_group_identity() {
        use logcore::problems::{
            EvidenceFlags, EvidenceFormat, EvidencePriority, FingerprintBuilder,
            FingerprintTokenKind, GroupQuery, IdentityQuality, LineProvenance,
            ObservationCandidate, ObservationRef, ObservationRole, PackedLogTimestamp, PageSpec,
            ProblemEventDraft, ProblemIndex, ProblemKind, ProcessFingerprintKey,
            ProcessInstanceKey, RuleId, SignatureQuality,
        };

        let process = ProcessFingerprintKey::new(Some("com.example.app"));
        let mut fingerprint = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::TypeOnly,
            IdentityQuality::KnownProcess,
            &process,
        );
        fingerprint.token(
            FingerprintTokenKind::ExceptionType,
            b"java.lang.IllegalStateException",
        );
        let key = logcore::problems::GroupKey::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::TypeOnly,
            IdentityQuality::KnownProcess,
            fingerprint.finish(),
        );
        let draft = ProblemEventDraft {
            start_line: 9,
            end_line: 11,
            anchor_line: 10,
            anchor_timestamp: PackedLogTimestamp::new(7, 26, 12, 34, 56, 789).unwrap(),
            pid: 42,
            process_instance: ProcessInstanceKey(2),
            kind: ProblemKind::JavaCrash,
            evidence: EvidenceFlags::PRIMARY,
            outcome: Default::default(),
            boundary: Default::default(),
        };
        let observation = ObservationCandidate::new(
            ObservationRef::new(
                10,
                RuleId::JavaUncaughtV1,
                ObservationRole::Primary,
                EvidenceFormat::AospText,
                LineProvenance::Unknown,
            )
            .unwrap(),
            EvidencePriority::MinimumGrammar,
        );
        let mut index = ProblemIndex::new();
        index.append(draft, key, &[observation]).unwrap();
        let snapshot = index.create_group_snapshot(&GroupQuery::default()).unwrap();
        let page = index
            .group_snapshot_page(snapshot, PageSpec::new(0, 100).unwrap())
            .unwrap();
        let dto = ProblemGroupPageDto::from_compact(
            AnalysisTokenDto {
                session_generation: 1,
                analysis_generation: 1,
            },
            page,
        );

        assert_eq!(dto.items.len(), 1);
        let group = &dto.items[0];
        assert_eq!(group.first_line, 11);
        assert_eq!(group.last_line, 11);
        assert_eq!(group.first_timestamp.as_deref(), Some("07-26 12:34:56.789"));
        assert_eq!(group.fingerprint.len(), 32);
        assert_eq!(group.fingerprint_version, 1);
        assert_eq!(group.signature_quality, SignatureQualityDto::TypeOnly);
        assert_eq!(group.identity_quality, IdentityQualityDto::KnownProcess);
        assert_eq!(dto.analysis_token.session_generation, 1);
        assert_eq!(dto.query_snapshot_id, snapshot.raw());
    }

    #[test]
    fn compact_occurrence_and_fact_conversion_is_bounded_and_contains_no_raw_text() {
        use logcore::problems::{
            BoundaryFlags, EvidenceFlags, EvidenceFormat, LineProvenance, LogBuffer,
            ObservationRef, ObservationRole, OutcomeFlags, PackedLogTimestamp, ProblemEvent,
            ProblemEventDraft, ProblemEventId, ProblemKind, ProcessInstanceKey, RuleId,
        };

        let event = ProblemEvent::new(
            ProblemEventDraft {
                start_line: 99,
                end_line: 108,
                anchor_line: 101,
                anchor_timestamp: PackedLogTimestamp::new(7, 26, 1, 2, 3, 4).unwrap(),
                pid: 123,
                process_instance: ProcessInstanceKey(8),
                kind: ProblemKind::NativeCrash,
                evidence: EvidenceFlags::PRIMARY
                    | EvidenceFlags::MULTILINE
                    | EvidenceFlags::CORRELATED,
                outcome: OutcomeFlags::DEATH_OBSERVED | OutcomeFlags::CONFLICT,
                boundary: BoundaryFlags::OBSERVATION_REFS_TRUNCATED
                    | BoundaryFlags::CORRELATION_LIMITED,
            },
            4,
            0,
            2,
            9,
        )
        .unwrap();
        let observations = [
            ObservationRef::new(
                101,
                RuleId::NativeTombstoneV1,
                ObservationRole::Primary,
                EvidenceFormat::TombstoneShapedText,
                LineProvenance::Known(LogBuffer::Crash),
            )
            .unwrap(),
            ObservationRef::new(
                107,
                RuleId::NativeTombstoneV1,
                ObservationRole::Signal,
                EvidenceFormat::TombstoneShapedText,
                LineProvenance::Known(LogBuffer::Crash),
            )
            .unwrap(),
        ];
        let detail = ProblemDetailDto::from_compact(
            AnalysisTokenDto {
                session_generation: 2,
                analysis_generation: 5,
            },
            19,
            ProblemEventId(6),
            event,
            &observations,
        );

        assert_eq!(detail.occurrence.start_line, 100);
        assert_eq!(detail.occurrence.end_line, 109);
        assert_eq!(detail.occurrence.anchor_line, 102);
        assert_eq!(detail.occurrence.pid, Some(123));
        assert_eq!(
            detail.occurrence.timestamp.as_deref(),
            Some("07-26 01:02:03.004")
        );
        assert_eq!(
            detail.occurrence.evidence_flags,
            vec![
                EvidenceFlagDto::Primary,
                EvidenceFlagDto::Multiline,
                EvidenceFlagDto::Correlated,
            ]
        );
        assert_eq!(
            detail.occurrence.outcome_flags,
            vec![OutcomeFlagDto::DeathObserved, OutcomeFlagDto::Conflict]
        );
        assert_eq!(
            detail.occurrence.boundary_flags,
            vec![
                BoundaryFlagDto::ObservationRefsTruncated,
                BoundaryFlagDto::CorrelationLimited,
            ]
        );
        assert_eq!(detail.observation_total, 9);
        assert!(detail.facts_truncated);
        assert_eq!(detail.facts.len(), 2);
        assert_eq!(detail.facts[0].source_line, 102);
        assert_eq!(detail.facts[0].rule_id, "aosp.native-tombstone.v1");
        assert_eq!(detail.facts[0].provenance, LineProvenanceDto::KnownCrash);

        let serialized = serde_json::to_string(&detail).unwrap();
        assert!(!serialized.contains("\"raw\""));
        assert!(!serialized.contains("\"message\""));
        assert!(!serialized.contains("\"text\""));
    }
}
