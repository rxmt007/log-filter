use std::ops::{BitOr, BitOrAssign};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LogBuffer {
    Main = 0,
    System = 1,
    Events = 2,
    Crash = 3,
    Radio = 4,
    Kernel = 5,
}

impl LogBuffer {
    const fn bit(self) -> u8 {
        1 << self as u8
    }

    pub(crate) const fn from_packed(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Main),
            1 => Some(Self::System),
            2 => Some(Self::Events),
            3 => Some(Self::Crash),
            4 => Some(Self::Radio),
            5 => Some(Self::Kernel),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct BufferSet(u8);

impl BufferSet {
    pub const NONE: Self = Self(0);
    pub const MAIN: Self = Self(LogBuffer::Main.bit());
    pub const SYSTEM: Self = Self(LogBuffer::System.bit());
    pub const EVENTS: Self = Self(LogBuffer::Events.bit());
    pub const CRASH: Self = Self(LogBuffer::Crash.bit());
    pub const RADIO: Self = Self(LogBuffer::Radio.bit());
    pub const KERNEL: Self = Self(LogBuffer::Kernel.bit());

    pub const fn contains(self, buffer: LogBuffer) -> bool {
        self.0 & buffer.bit() != 0
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl BitOr for BufferSet {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for BufferSet {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CaptureOrigin {
    StaticFile = 0,
    AdbLive = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RangeCompleteness {
    Unknown = 0,
    Bounded = 1,
    StartTruncated = 2,
    EndTruncated = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputCoverage {
    origin: CaptureOrigin,
    requested_buffers: BufferSet,
    requested_buffers_known: bool,
    range_completeness: RangeCompleteness,
}

impl InputCoverage {
    pub const fn static_file(range_completeness: RangeCompleteness) -> Self {
        Self {
            origin: CaptureOrigin::StaticFile,
            requested_buffers: BufferSet::NONE,
            requested_buffers_known: false,
            range_completeness,
        }
    }

    pub const fn adb_live(
        requested_buffers: BufferSet,
        range_completeness: RangeCompleteness,
    ) -> Self {
        Self {
            origin: CaptureOrigin::AdbLive,
            requested_buffers,
            requested_buffers_known: true,
            range_completeness,
        }
    }

    pub const fn origin(self) -> CaptureOrigin {
        self.origin
    }

    pub const fn requested_buffers(self) -> Option<BufferSet> {
        if self.requested_buffers_known {
            Some(self.requested_buffers)
        } else {
            None
        }
    }

    pub const fn range_completeness(self) -> RangeCompleteness {
        self.range_completeness
    }

    /// Add only source-specific inference that the capture contract permits.
    pub fn infer_format_provenance(
        self,
        format: EvidenceFormat,
        provenance: LineProvenance,
    ) -> LineProvenance {
        if provenance != LineProvenance::Unknown {
            return provenance;
        }
        match format {
            EvidenceFormat::EventLogShapedText => {
                if self
                    .requested_buffers()
                    .is_some_and(|buffers| !buffers.contains(LogBuffer::Events))
                {
                    LineProvenance::Unknown
                } else {
                    LineProvenance::Inferred(LogBuffer::Events)
                }
            }
            // Kernel source is never inferred from payload shape.
            EvidenceFormat::KernelShapedText
            | EvidenceFormat::AospText
            | EvidenceFormat::TombstoneShapedText => LineProvenance::Unknown,
        }
    }

    pub fn admit(self, format: EvidenceFormat, provenance: LineProvenance) -> EvidenceAdmission {
        let required_buffer = match format {
            EvidenceFormat::EventLogShapedText => Some(LogBuffer::Events),
            EvidenceFormat::KernelShapedText => Some(LogBuffer::Kernel),
            EvidenceFormat::AospText | EvidenceFormat::TombstoneShapedText => None,
        };
        if required_buffer.is_some_and(|required| {
            self.requested_buffers()
                .is_some_and(|requested| !requested.contains(required))
        }) {
            return EvidenceAdmission::Rejected;
        }
        evidence_admission(format, self.infer_format_provenance(format, provenance))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineProvenance {
    Unknown,
    Inferred(LogBuffer),
    Known(LogBuffer),
}

impl LineProvenance {
    pub(crate) const fn pack(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Inferred(buffer) => 0x40 | buffer as u8,
            Self::Known(buffer) => 0x80 | buffer as u8,
        }
    }

    pub(crate) const fn unpack(value: u8) -> Option<Self> {
        let buffer = value & 0x0f;
        match value & 0xf0 {
            0 if buffer == 0 => Some(Self::Unknown),
            0x40 => match LogBuffer::from_packed(buffer) {
                Some(buffer) => Some(Self::Inferred(buffer)),
                None => None,
            },
            0x80 => match LogBuffer::from_packed(buffer) {
                Some(buffer) => Some(Self::Known(buffer)),
                None => None,
            },
            _ => None,
        }
    }
}

/// Streaming fallback provenance derived from exact logcat buffer dividers.
///
/// The tracker owns only the active buffer, so memory use is constant regardless
/// of input size. A divider affects rows after itself. Provenance supplied by an
/// input adapter wins for the current row; a valid divider still advances the
/// fallback state used after that explicit span ends.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BufferProvenanceTracker {
    active_buffer: Option<LogBuffer>,
}

impl BufferProvenanceTracker {
    pub(crate) const fn new() -> Self {
        Self {
            active_buffer: None,
        }
    }

    pub(crate) fn observe_stable_line(
        &mut self,
        raw_line: &[u8],
        adapter_provenance: LineProvenance,
    ) -> LineProvenance {
        let fallback = self
            .active_buffer
            .map(LineProvenance::Known)
            .unwrap_or(LineProvenance::Unknown);
        let resolved = match adapter_provenance {
            LineProvenance::Unknown => fallback,
            explicit => explicit,
        };

        if let Some(buffer) = parse_logcat_buffer_divider(raw_line) {
            self.active_buffer = Some(buffer);
        }
        resolved
    }
}

fn parse_logcat_buffer_divider(raw_line: &[u8]) -> Option<LogBuffer> {
    let line = raw_line.strip_suffix(b"\n").unwrap_or(raw_line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    match line {
        b"--------- beginning of main" => Some(LogBuffer::Main),
        b"--------- beginning of system" => Some(LogBuffer::System),
        b"--------- beginning of events" => Some(LogBuffer::Events),
        b"--------- beginning of crash" => Some(LogBuffer::Crash),
        b"--------- beginning of radio" => Some(LogBuffer::Radio),
        b"--------- beginning of kernel" => Some(LogBuffer::Kernel),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum EvidenceFormat {
    AospText = 0,
    EventLogShapedText = 1,
    TombstoneShapedText = 2,
    KernelShapedText = 3,
}

impl EvidenceFormat {
    pub(crate) const fn from_packed(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::AospText),
            1 => Some(Self::EventLogShapedText),
            2 => Some(Self::TombstoneShapedText),
            3 => Some(Self::KernelShapedText),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceAdmission {
    CommitEligible,
    SupportingOnly,
    Rejected,
}

pub const fn evidence_admission(
    format: EvidenceFormat,
    provenance: LineProvenance,
) -> EvidenceAdmission {
    match format {
        EvidenceFormat::AospText | EvidenceFormat::TombstoneShapedText => {
            EvidenceAdmission::CommitEligible
        }
        EvidenceFormat::EventLogShapedText => match provenance {
            LineProvenance::Known(LogBuffer::Events) => EvidenceAdmission::CommitEligible,
            LineProvenance::Inferred(LogBuffer::Events) => EvidenceAdmission::SupportingOnly,
            _ => EvidenceAdmission::Rejected,
        },
        EvidenceFormat::KernelShapedText => match provenance {
            LineProvenance::Known(LogBuffer::Kernel) => EvidenceAdmission::CommitEligible,
            _ => EvidenceAdmission::Rejected,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    start_line: u32,
    end_line: u32,
    buffer: LogBuffer,
}

impl SourceSpan {
    pub fn new(start_line: u32, end_line: u32, buffer: LogBuffer) -> Result<Self, SourceSpanError> {
        if start_line > end_line {
            return Err(SourceSpanError::InvalidRange);
        }
        Ok(Self {
            start_line,
            end_line,
            buffer,
        })
    }

    pub const fn start_line(self) -> u32 {
        self.start_line
    }

    pub const fn end_line(self) -> u32 {
        self.end_line
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceSpanError {
    InvalidRange,
    Overlap,
    Capacity,
}

#[derive(Debug, Default, Clone)]
pub struct SourceSpanIndex {
    spans: Vec<SourceSpan>,
}

impl SourceSpanIndex {
    pub const fn new() -> Self {
        Self { spans: Vec::new() }
    }

    pub fn insert(&mut self, span: SourceSpan) -> Result<(), SourceSpanError> {
        let position = self
            .spans
            .partition_point(|existing| existing.start_line < span.start_line);
        if position > 0 && self.spans[position - 1].end_line >= span.start_line {
            return Err(SourceSpanError::Overlap);
        }
        if position < self.spans.len() && self.spans[position].start_line <= span.end_line {
            return Err(SourceSpanError::Overlap);
        }
        self.spans
            .try_reserve(1)
            .map_err(|_| SourceSpanError::Capacity)?;
        self.spans.insert(position, span);
        Ok(())
    }

    pub fn provenance_at(&self, line: u32) -> LineProvenance {
        let position = self.spans.partition_point(|span| span.start_line <= line);
        if position == 0 {
            return LineProvenance::Unknown;
        }
        let span = self.spans[position - 1];
        if line <= span.end_line {
            LineProvenance::Known(span.buffer)
        } else {
            LineProvenance::Unknown
        }
    }

    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_events_without_line_metadata_is_only_inferred() {
        let coverage = InputCoverage::adb_live(
            BufferSet::MAIN | BufferSet::SYSTEM | BufferSet::EVENTS,
            RangeCompleteness::Bounded,
        );

        let provenance = coverage
            .infer_format_provenance(EvidenceFormat::EventLogShapedText, LineProvenance::Unknown);

        assert_eq!(provenance, LineProvenance::Inferred(LogBuffer::Events));
        assert_eq!(
            evidence_admission(EvidenceFormat::EventLogShapedText, provenance),
            EvidenceAdmission::SupportingOnly
        );
    }

    #[test]
    fn main_only_capture_rejects_eventlog_shaped_text() {
        let coverage = InputCoverage::adb_live(BufferSet::MAIN, RangeCompleteness::StartTruncated);

        assert_eq!(
            coverage.infer_format_provenance(
                EvidenceFormat::EventLogShapedText,
                LineProvenance::Unknown
            ),
            LineProvenance::Unknown
        );
        assert_eq!(
            coverage.admit(EvidenceFormat::EventLogShapedText, LineProvenance::Unknown),
            EvidenceAdmission::Rejected
        );
        assert_eq!(
            coverage.admit(
                EvidenceFormat::EventLogShapedText,
                LineProvenance::Known(LogBuffer::Events)
            ),
            EvidenceAdmission::Rejected,
            "capture contract wins over contradictory per-line metadata"
        );
    }

    #[test]
    fn only_known_kernel_source_can_commit_kernel_shaped_text() {
        assert_eq!(
            evidence_admission(
                EvidenceFormat::KernelShapedText,
                LineProvenance::Known(LogBuffer::Kernel)
            ),
            EvidenceAdmission::CommitEligible
        );
        assert_eq!(
            evidence_admission(
                EvidenceFormat::KernelShapedText,
                LineProvenance::Inferred(LogBuffer::Kernel)
            ),
            EvidenceAdmission::Rejected
        );
    }

    #[test]
    fn source_span_index_requires_non_overlapping_proven_ranges() {
        let mut spans = SourceSpanIndex::new();
        spans
            .insert(SourceSpan::new(10, 19, LogBuffer::Events).unwrap())
            .unwrap();
        spans
            .insert(SourceSpan::new(20, 30, LogBuffer::Crash).unwrap())
            .unwrap();

        assert_eq!(
            spans.provenance_at(10),
            LineProvenance::Known(LogBuffer::Events)
        );
        assert_eq!(
            spans.provenance_at(30),
            LineProvenance::Known(LogBuffer::Crash)
        );
        assert_eq!(spans.provenance_at(31), LineProvenance::Unknown);
        assert_eq!(
            spans.insert(SourceSpan::new(18, 21, LogBuffer::Main).unwrap()),
            Err(SourceSpanError::Overlap)
        );
    }

    #[test]
    fn standard_logcat_dividers_change_only_subsequent_line_provenance() {
        let mut tracker = BufferProvenanceTracker::new();

        assert_eq!(
            tracker.observe_stable_line(b"--------- beginning of main\n", LineProvenance::Unknown),
            LineProvenance::Unknown
        );
        assert_eq!(
            tracker.observe_stable_line(b"main payload\n", LineProvenance::Unknown),
            LineProvenance::Known(LogBuffer::Main)
        );
        assert_eq!(
            tracker.observe_stable_line(
                b"--------- beginning of events\r\n",
                LineProvenance::Unknown
            ),
            LineProvenance::Known(LogBuffer::Main)
        );
        assert_eq!(
            tracker.observe_stable_line(b"events payload\n", LineProvenance::Unknown),
            LineProvenance::Known(LogBuffer::Events)
        );
        assert_eq!(
            tracker.observe_stable_line(b"--------- beginning of crash\n", LineProvenance::Unknown),
            LineProvenance::Known(LogBuffer::Events)
        );
        assert_eq!(
            tracker.observe_stable_line(b"crash payload\n", LineProvenance::Unknown),
            LineProvenance::Known(LogBuffer::Crash)
        );
    }

    #[test]
    fn every_supported_standard_divider_is_recognized_exactly() {
        for (divider, buffer) in [
            (b"--------- beginning of main\n".as_slice(), LogBuffer::Main),
            (
                b"--------- beginning of system\n".as_slice(),
                LogBuffer::System,
            ),
            (
                b"--------- beginning of events\n".as_slice(),
                LogBuffer::Events,
            ),
            (
                b"--------- beginning of crash\n".as_slice(),
                LogBuffer::Crash,
            ),
            (
                b"--------- beginning of radio\n".as_slice(),
                LogBuffer::Radio,
            ),
            (
                b"--------- beginning of kernel\n".as_slice(),
                LogBuffer::Kernel,
            ),
        ] {
            let mut tracker = BufferProvenanceTracker::new();
            assert_eq!(
                tracker.observe_stable_line(divider, LineProvenance::Unknown),
                LineProvenance::Unknown
            );
            assert_eq!(
                tracker.observe_stable_line(b"payload\n", LineProvenance::Unknown),
                LineProvenance::Known(buffer)
            );
        }
    }

    #[test]
    fn ordinary_messages_cannot_forge_a_logcat_divider() {
        for lookalike in [
            b"07-26 12:00:00.000  1  1 I App: --------- beginning of events\n".as_slice(),
            b"prefix --------- beginning of events\n".as_slice(),
            b" --------- beginning of events\n".as_slice(),
            b"--------- beginning of events trailing\n".as_slice(),
            b"--------- switch to events\n".as_slice(),
            b"---------- beginning of events\n".as_slice(),
        ] {
            let mut tracker = BufferProvenanceTracker::new();
            assert_eq!(
                tracker.observe_stable_line(lookalike, LineProvenance::Unknown),
                LineProvenance::Unknown
            );
            assert_eq!(
                tracker.observe_stable_line(b"next payload\n", LineProvenance::Unknown),
                LineProvenance::Unknown
            );
        }
    }

    #[test]
    fn explicit_adapter_provenance_wins_without_blocking_divider_progress() {
        let mut tracker = BufferProvenanceTracker::new();

        assert_eq!(
            tracker.observe_stable_line(
                b"--------- beginning of events\n",
                LineProvenance::Known(LogBuffer::Main)
            ),
            LineProvenance::Known(LogBuffer::Main),
            "the adapter owns attribution for the divider row itself"
        );
        assert_eq!(
            tracker.observe_stable_line(
                b"payload inside explicit span\n",
                LineProvenance::Known(LogBuffer::Crash)
            ),
            LineProvenance::Known(LogBuffer::Crash),
            "an explicit adapter span wins a conflict on its rows"
        );
        assert_eq!(
            tracker.observe_stable_line(b"payload after explicit span\n", LineProvenance::Unknown),
            LineProvenance::Known(LogBuffer::Events),
            "the divider remains the fallback after the explicit span ends"
        );
    }

    #[test]
    fn tracker_state_is_constant_size_and_unknown_without_a_divider() {
        assert!(std::mem::size_of::<BufferProvenanceTracker>() <= 2);
        let mut tracker = BufferProvenanceTracker::new();
        for _ in 0..10_000 {
            assert_eq!(
                tracker.observe_stable_line(b"ordinary payload\n", LineProvenance::Unknown),
                LineProvenance::Unknown
            );
        }
    }
}
