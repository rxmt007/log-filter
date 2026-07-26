use crate::problems::{LineProvenance, LogBuffer};
use std::str;

pub const MAX_LMK_INPUT_BYTES: usize = 16 * 1024;
pub const MAX_LMK_PROCESS_NAME_BYTES: usize = 256;
pub const MAX_PENDING_LMK_KILLS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LmkMechanism {
    UserspaceLmkd,
    LegacyKernelLowMemoryKiller,
}

impl LmkMechanism {
    pub const fn token(self) -> &'static str {
        match self {
            Self::UserspaceLmkd => "userspace-lmkd",
            Self::LegacyKernelLowMemoryKiller => "legacy-kernel-lowmemorykiller",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LmkPolicyReason {
    LowWatermark,
    Thrashing,
    MemoryPressure,
    SwapLow,
    DirectReclaim,
    LegacyPolicy,
}

impl LmkPolicyReason {
    pub const fn token(self) -> &'static str {
        match self {
            Self::LowWatermark => "low-watermark",
            Self::Thrashing => "thrashing",
            Self::MemoryPressure => "memory-pressure",
            Self::SwapLow => "swap-low",
            Self::DirectReclaim => "direct-reclaim",
            Self::LegacyPolicy => "legacy-policy",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct LmkProcessToken([u8; MAX_LMK_PROCESS_NAME_BYTES]);

impl LmkProcessToken {
    pub fn new(value: &[u8]) -> Option<Self> {
        if value.is_empty()
            || value.len() > MAX_LMK_PROCESS_NAME_BYTES
            || value.contains(&0)
            || str::from_utf8(value).is_err()
            || !value.iter().copied().all(is_process_name_byte)
        {
            return None;
        }
        let mut token = Self([0; MAX_LMK_PROCESS_NAME_BYTES]);
        token.0[..value.len()].copy_from_slice(value);
        Some(token)
    }

    pub fn as_str(&self) -> &str {
        let len = self
            .0
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(MAX_LMK_PROCESS_NAME_BYTES);
        str::from_utf8(&self.0[..len]).expect("LMK process names are validated UTF-8")
    }
}

impl std::fmt::Debug for LmkProcessToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("LmkProcessToken")
            .field(&self.as_str())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LmkFingerprintInput {
    pub process: LmkProcessToken,
    pub mechanism: LmkMechanism,
    pub reason: LmkPolicyReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LmkOutcome {
    pub kill_issued: bool,
    pub death_observed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LmkOccurrence {
    pub line: u32,
    pub victim_pid: u32,
    pub victim_uid: Option<u32>,
    pub process: LmkProcessToken,
    pub mechanism: LmkMechanism,
    pub reason: LmkPolicyReason,
    pub outcome: LmkOutcome,
    pub fingerprint: LmkFingerprintInput,
}

#[derive(Debug)]
pub struct LmkRecognizer {
    pending: [Option<LmkOccurrence>; MAX_PENDING_LMK_KILLS],
    pending_eviction_count: u64,
}

impl Default for LmkRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl LmkRecognizer {
    pub const fn new() -> Self {
        Self {
            pending: [None; MAX_PENDING_LMK_KILLS],
            pending_eviction_count: 0,
        }
    }

    pub fn observe(
        &mut self,
        line: u32,
        tag: &str,
        message: &[u8],
        provenance: LineProvenance,
    ) -> Option<LmkOccurrence> {
        if message.len() > MAX_LMK_INPUT_BYTES || str::from_utf8(message).is_err() {
            return None;
        }
        let parsed = match tag {
            "lmkd" => parse_lmkd(message),
            "lowmemorykiller" if provenance == LineProvenance::Known(LogBuffer::Kernel) => {
                parse_legacy_lowmemorykiller(message)
            }
            _ => None,
        }?;
        let fingerprint = LmkFingerprintInput {
            process: parsed.process,
            mechanism: parsed.mechanism,
            reason: parsed.reason,
        };
        let occurrence = LmkOccurrence {
            line,
            victim_pid: parsed.pid,
            victim_uid: parsed.uid,
            process: parsed.process,
            mechanism: parsed.mechanism,
            reason: parsed.reason,
            outcome: LmkOutcome {
                kill_issued: true,
                death_observed: false,
            },
            fingerprint,
        };
        self.insert_pending(occurrence);
        Some(occurrence)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.iter().flatten().count()
    }

    #[cfg(test)]
    pub const fn pending_eviction_count(&self) -> u64 {
        self.pending_eviction_count
    }

    pub fn finish_input(&mut self) -> u8 {
        let count = self.pending_count().min(usize::from(u8::MAX)) as u8;
        self.pending.fill(None);
        count
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn insert_pending(&mut self, occurrence: LmkOccurrence) {
        if let Some(slot) = self.pending.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(occurrence);
            return;
        }
        let oldest = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, pending)| pending.map(|pending| pending.line).unwrap_or(u32::MAX))
            .map(|(index, _)| index)
            .expect("fixed LMK pending table is non-empty");
        self.pending[oldest] = Some(occurrence);
        self.pending_eviction_count = self.pending_eviction_count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedLmk {
    pid: u32,
    uid: Option<u32>,
    process: LmkProcessToken,
    mechanism: LmkMechanism,
    reason: LmkPolicyReason,
}

fn parse_lmkd(message: &[u8]) -> Option<ParsedLmk> {
    let value = trim_ascii(message).strip_prefix(b"Kill '")?;
    let quote = value.iter().position(|byte| *byte == b'\'')?;
    let process = LmkProcessToken::new(&value[..quote])?;
    let after_process = value[quote + 1..].strip_prefix(b" (")?;
    let (pid, remainder) = parse_unsigned(after_process, false)?;
    let remainder = remainder.strip_prefix(b"), uid ")?;
    let (uid, remainder) = parse_unsigned(remainder, true)?;
    let remainder = remainder.strip_prefix(b", oom_score_adj ")?;
    let (_, remainder) = parse_signed_i32(remainder)?;
    let remainder = remainder.strip_prefix(b" to free ")?;
    let reason_marker = find_subslice(remainder, b"; reason: ")?;
    if reason_marker == 0 {
        return None;
    }
    let reason = parse_policy_reason(&remainder[reason_marker + b"; reason: ".len()..])?;
    Some(ParsedLmk {
        pid,
        uid: Some(uid),
        process,
        mechanism: LmkMechanism::UserspaceLmkd,
        reason,
    })
}

fn parse_legacy_lowmemorykiller(message: &[u8]) -> Option<ParsedLmk> {
    let value = trim_ascii(message).strip_prefix(b"Killing '")?;
    let quote = value.iter().position(|byte| *byte == b'\'')?;
    let process = LmkProcessToken::new(&value[..quote])?;
    let after_process = value[quote + 1..].strip_prefix(b" (")?;
    let (pid, remainder) = parse_unsigned(after_process, false)?;
    let remainder = remainder.strip_prefix(b"), adj ")?;
    let (_, remainder) = parse_signed_i32(remainder)?;
    find_subslice(remainder, b"to free ")?;
    Some(ParsedLmk {
        pid,
        uid: None,
        process,
        mechanism: LmkMechanism::LegacyKernelLowMemoryKiller,
        reason: LmkPolicyReason::LegacyPolicy,
    })
}

fn parse_policy_reason(value: &[u8]) -> Option<LmkPolicyReason> {
    let value = trim_ascii(value);
    if has_prefix_boundary(value, b"low watermark") {
        Some(LmkPolicyReason::LowWatermark)
    } else if has_prefix_boundary(value, b"process is thrashing")
        || has_prefix_boundary(value, b"thrashing")
    {
        Some(LmkPolicyReason::Thrashing)
    } else if has_prefix_boundary(value, b"memory pressure") || has_prefix_boundary(value, b"psi") {
        Some(LmkPolicyReason::MemoryPressure)
    } else if has_prefix_boundary(value, b"swap is low") || has_prefix_boundary(value, b"swap low")
    {
        Some(LmkPolicyReason::SwapLow)
    } else if has_prefix_boundary(value, b"direct reclaim") {
        Some(LmkPolicyReason::DirectReclaim)
    } else {
        None
    }
}

fn parse_unsigned(value: &[u8], allow_zero: bool) -> Option<(u32, &[u8])> {
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
    if !allow_zero && number == 0 {
        return None;
    }
    Some((number, &value[end..]))
}

fn parse_signed_i32(value: &[u8]) -> Option<(i32, &[u8])> {
    let (negative, digits) = value
        .strip_prefix(b"-")
        .map_or((false, value), |digits| (true, digits));
    let (magnitude, remainder) = parse_unsigned(digits, true)?;
    let signed = if negative {
        i32::try_from(magnitude).ok()?.checked_neg()?
    } else {
        i32::try_from(magnitude).ok()?
    };
    Some((signed, remainder))
}

fn has_prefix_boundary(value: &[u8], prefix: &[u8]) -> bool {
    value.strip_prefix(prefix).is_some_and(|remainder| {
        remainder
            .first()
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
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
    use std::mem::{needs_drop, size_of};

    fn modern(process: &str, pid: u32, uid: u32, adj: i32, rss: u64, reason: &str) -> String {
        format!(
            "Kill '{process}' ({pid}), uid {uid}, oom_score_adj {adj} to free {rss}kB rss, 0kB swap; reason: {reason}"
        )
    }

    #[test]
    fn lmkd_uses_message_victim_and_does_not_claim_death() {
        let mut recognizer = LmkRecognizer::new();
        let occurrence = recognizer
            .observe(
                10,
                "lmkd",
                modern(
                    "com.example.app",
                    4242,
                    10_123,
                    900,
                    54_321,
                    "low watermark",
                )
                .as_bytes(),
                LineProvenance::Unknown,
            )
            .unwrap();
        assert_eq!(occurrence.victim_pid, 4242);
        assert_eq!(occurrence.victim_uid, Some(10_123));
        assert_eq!(occurrence.process.as_str(), "com.example.app");
        assert_eq!(occurrence.reason, LmkPolicyReason::LowWatermark);
        assert_eq!(
            occurrence.outcome,
            LmkOutcome {
                kill_issued: true,
                death_observed: false,
            }
        );
    }

    #[test]
    fn pid_adj_rss_and_uid_changes_do_not_change_fingerprint() {
        let mut recognizer = LmkRecognizer::new();
        let first = recognizer
            .observe(
                1,
                "lmkd",
                modern(
                    "com.example.app",
                    1,
                    10_001,
                    900,
                    1,
                    "process is thrashing 90%",
                )
                .as_bytes(),
                LineProvenance::Unknown,
            )
            .unwrap();
        let second = recognizer
            .observe(
                2,
                "lmkd",
                modern(
                    "com.example.app",
                    9999,
                    19_999,
                    0,
                    999_999,
                    "process is thrashing 1%",
                )
                .as_bytes(),
                LineProvenance::Unknown,
            )
            .unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_ne!(first.victim_pid, second.victim_pid);
    }

    #[test]
    fn exact_tag_and_strict_kill_grammar_reject_selection_skip_and_pressure_noise() {
        let mut recognizer = LmkRecognizer::new();
        for (tag, message) in [
            ("LMKD", modern("app", 1, 1, 1, 1, "low watermark")),
            ("lmkd", "Selecting process 1 with oom_score_adj 900".into()),
            (
                "lmkd",
                "pressure 80, skip killing because swap is available".into(),
            ),
            ("lmkd", "Kill 'app' (1), uid 2, oom_score_adj 3".into()),
            ("lmkd", modern("app", 1, 2, 3, 4, "OEM magic threshold 7")),
        ] {
            assert!(recognizer
                .observe(1, tag, message.as_bytes(), LineProvenance::Unknown)
                .is_none());
        }
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn legacy_lowmemorykiller_requires_exact_tag_and_known_kernel_source() {
        let message = b"Killing 'legacy.app' (321), adj 906, to free 12345kB";
        for provenance in [
            LineProvenance::Unknown,
            LineProvenance::Inferred(LogBuffer::Kernel),
        ] {
            assert!(LmkRecognizer::new()
                .observe(1, "lowmemorykiller", message, provenance)
                .is_none());
        }
        assert!(LmkRecognizer::new()
            .observe(
                1,
                "kernel",
                b"lowmemorykiller: Killing 'legacy.app' (321), adj 906, to free 12345kB",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .is_none());
        let occurrence = LmkRecognizer::new()
            .observe(
                1,
                "lowmemorykiller",
                message,
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .unwrap();
        assert_eq!(
            occurrence.mechanism,
            LmkMechanism::LegacyKernelLowMemoryKiller
        );
    }

    #[test]
    fn pending_is_fixed_capacity_and_truncation_never_invents_death() {
        assert_eq!(size_of::<LmkProcessToken>(), MAX_LMK_PROCESS_NAME_BYTES);
        assert!(!needs_drop::<LmkProcessToken>());
        let mut recognizer = LmkRecognizer::new();
        for index in 1..=MAX_PENDING_LMK_KILLS + 1 {
            let message = modern(
                &format!("com.example.app{index}"),
                index as u32,
                index as u32,
                0,
                1,
                "low watermark",
            );
            recognizer
                .observe(
                    index as u32,
                    "lmkd",
                    message.as_bytes(),
                    LineProvenance::Unknown,
                )
                .unwrap();
        }
        assert_eq!(recognizer.pending_count(), MAX_PENDING_LMK_KILLS);
        assert_eq!(recognizer.pending_eviction_count(), 1);
        assert_eq!(recognizer.finish_input(), MAX_PENDING_LMK_KILLS as u8);
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn invalid_utf8_overlong_input_and_other_oom_mechanisms_are_rejected() {
        let mut recognizer = LmkRecognizer::new();
        assert!(recognizer
            .observe(
                1,
                "lmkd",
                b"Kill 'bad\xff' (1), uid 1, oom_score_adj 1 to free 1kB; reason: low watermark",
                LineProvenance::Unknown,
            )
            .is_none());
        let overlong = vec![b'a'; MAX_LMK_INPUT_BYTES + 1];
        assert!(recognizer
            .observe(1, "lmkd", &overlong, LineProvenance::Unknown,)
            .is_none());
        assert!(recognizer
            .observe(
                1,
                "kernel",
                b"Out of memory: Killed process 42 (app)",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .is_none());
        assert!(recognizer
            .observe(
                1,
                "AndroidRuntime",
                b"java.lang.OutOfMemoryError",
                LineProvenance::Unknown,
            )
            .is_none());
    }
}
