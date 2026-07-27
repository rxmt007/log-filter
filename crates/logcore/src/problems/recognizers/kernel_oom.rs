use crate::problems::{LineProvenance, LogBuffer};
use std::str;

pub const MAX_KERNEL_OOM_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_KERNEL_PROCESS_NAME_BYTES: usize = 256;
pub const MAX_PENDING_KERNEL_OOM: usize = 8;
pub const MAX_KERNEL_OOM_SPAN_LINES: u32 = 512;

const OPENER_INVOKED: u8 = 1 << 0;
const OPENER_CONSTRAINT: u8 = 1 << 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KernelOomMechanism {
    Global,
    Memcg,
}

impl KernelOomMechanism {
    pub const fn token(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Memcg => "memcg",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct KernelProcessToken([u8; MAX_KERNEL_PROCESS_NAME_BYTES]);

impl KernelProcessToken {
    pub fn new(value: &[u8]) -> Option<Self> {
        if value.is_empty()
            || value.len() > MAX_KERNEL_PROCESS_NAME_BYTES
            || value.contains(&0)
            || str::from_utf8(value).is_err()
            || !value.iter().copied().all(is_process_name_byte)
        {
            return None;
        }
        let mut token = Self([0; MAX_KERNEL_PROCESS_NAME_BYTES]);
        token.0[..value.len()].copy_from_slice(value);
        Some(token)
    }

    pub fn as_str(&self) -> &str {
        let len = self
            .0
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(MAX_KERNEL_PROCESS_NAME_BYTES);
        str::from_utf8(&self.0[..len]).expect("kernel process names are validated UTF-8")
    }
}

impl std::fmt::Debug for KernelProcessToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("KernelProcessToken")
            .field(&self.as_str())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KernelOomFingerprintInput {
    pub process: KernelProcessToken,
    pub mechanism: KernelOomMechanism,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelOomOutcome {
    pub kill_issued: bool,
    pub death_observed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelOomOccurrence {
    pub start_line: u32,
    pub line: u32,
    pub victim_pid: u32,
    pub process: KernelProcessToken,
    pub mechanism: KernelOomMechanism,
    pub outcome: KernelOomOutcome,
    pub fingerprint: KernelOomFingerprintInput,
}

#[derive(Debug, Clone, Copy)]
struct PendingKernelOom {
    start_line: u32,
    last_opener_line: u32,
    mechanism: KernelOomMechanism,
    opener_kinds: u8,
}

#[derive(Debug)]
pub struct KernelOomRecognizer {
    pending: [Option<PendingKernelOom>; MAX_PENDING_KERNEL_OOM],
    pending_eviction_count: u64,
    ambiguity_count: u64,
}

impl Default for KernelOomRecognizer {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelOomRecognizer {
    pub const fn new() -> Self {
        Self {
            pending: [None; MAX_PENDING_KERNEL_OOM],
            pending_eviction_count: 0,
            ambiguity_count: 0,
        }
    }

    pub fn observe(
        &mut self,
        line: u32,
        tag: &str,
        message: &[u8],
        provenance: LineProvenance,
    ) -> Option<KernelOomOccurrence> {
        if provenance != LineProvenance::Known(LogBuffer::Kernel)
            || !matches!(tag, "kernel" | "")
            || message.len() > MAX_KERNEL_OOM_INPUT_BYTES
            || str::from_utf8(message).is_err()
        {
            return None;
        }
        self.expire_pending(line);
        let message = normalize_kernel_message(tag, message);

        if let Some(victim) =
            parse_prefixed_victim(message, b"Memory cgroup out of memory: Killed process ")
        {
            self.pending.fill(None);
            return Some(occurrence(line, line, victim, KernelOomMechanism::Memcg));
        }
        if let Some(victim) = parse_prefixed_victim(message, b"Out of memory: Killed process ") {
            self.pending.fill(None);
            return Some(occurrence(line, line, victim, KernelOomMechanism::Global));
        }
        if let Some(mechanism) = parse_constraint(message) {
            self.insert_pending(PendingKernelOom {
                start_line: line,
                last_opener_line: line,
                mechanism,
                opener_kinds: OPENER_CONSTRAINT,
            });
            return None;
        }
        if contains(message, b"invoked oom-killer:") {
            self.insert_pending(PendingKernelOom {
                start_line: line,
                last_opener_line: line,
                mechanism: KernelOomMechanism::Global,
                opener_kinds: OPENER_INVOKED,
            });
            return None;
        }
        if let Some(victim) = parse_prefixed_victim(message, b"Killed process ") {
            let mut matches = self.pending.iter().flatten().copied();
            let candidate = matches.next()?;
            if matches.next().is_some() {
                self.ambiguity_count = self.ambiguity_count.saturating_add(1);
                return None;
            }
            self.pending.fill(None);
            return Some(occurrence(
                candidate.start_line,
                line,
                victim,
                candidate.mechanism,
            ));
        }
        None
    }

    pub fn pending_count(&self) -> usize {
        self.pending.iter().flatten().count()
    }

    pub(crate) const fn pending_eviction_count(&self) -> u64 {
        self.pending_eviction_count
    }

    #[cfg(test)]
    pub const fn ambiguity_count(&self) -> u64 {
        self.ambiguity_count
    }

    pub fn finish_input(&mut self) -> u8 {
        let count = self.pending_count().min(usize::from(u8::MAX)) as u8;
        self.pending.fill(None);
        count
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn insert_pending(&mut self, pending: PendingKernelOom) {
        if pending.opener_kinds == OPENER_CONSTRAINT {
            if let Some(invoked) = self.pending.iter_mut().flatten().find(|candidate| {
                candidate.opener_kinds == OPENER_INVOKED
                    && candidate.last_opener_line.checked_add(1) == Some(pending.start_line)
            }) {
                invoked.last_opener_line = pending.last_opener_line;
                invoked.mechanism = pending.mechanism;
                invoked.opener_kinds |= OPENER_CONSTRAINT;
                return;
            }
        }
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
                    .map(|candidate| candidate.start_line)
                    .unwrap_or(u32::MAX)
            })
            .map(|(index, _)| index)
            .expect("fixed kernel OOM table is non-empty");
        self.pending[oldest] = Some(pending);
        self.pending_eviction_count = self.pending_eviction_count.saturating_add(1);
    }

    fn expire_pending(&mut self, line: u32) {
        for pending in &mut self.pending {
            if pending.is_some_and(|candidate| {
                line.saturating_sub(candidate.start_line) > MAX_KERNEL_OOM_SPAN_LINES
            }) {
                *pending = None;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedVictim {
    pid: u32,
    process: KernelProcessToken,
}

fn occurrence(
    start_line: u32,
    line: u32,
    victim: ParsedVictim,
    mechanism: KernelOomMechanism,
) -> KernelOomOccurrence {
    let fingerprint = KernelOomFingerprintInput {
        process: victim.process,
        mechanism,
    };
    KernelOomOccurrence {
        start_line,
        line,
        victim_pid: victim.pid,
        process: victim.process,
        mechanism,
        outcome: KernelOomOutcome {
            kill_issued: true,
            death_observed: false,
        },
        fingerprint,
    }
}

fn normalize_kernel_message<'a>(tag: &str, message: &'a [u8]) -> &'a [u8] {
    let message = trim_ascii(message);
    if tag.is_empty() {
        strip_printk_prefix(message).unwrap_or(message)
    } else {
        message
    }
}

fn strip_printk_prefix(message: &[u8]) -> Option<&[u8]> {
    let after_open = message.strip_prefix(b"<")?;
    let after_level = match after_open {
        [level @ b'0'..=b'7', b'>', remaining @ ..] => {
            let _ = level;
            remaining
        }
        _ => return None,
    };
    let timestamp_and_message = after_level.strip_prefix(b"[")?;
    let close = timestamp_and_message
        .iter()
        .position(|byte| *byte == b']')?;
    let timestamp = trim_ascii(&timestamp_and_message[..close]);
    if timestamp.is_empty()
        || !timestamp.iter().any(u8::is_ascii_digit)
        || !timestamp
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b'.')
    {
        return None;
    }
    let after_timestamp = &timestamp_and_message[close + 1..];
    if !after_timestamp.first().is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    Some(trim_ascii(after_timestamp))
}

fn parse_constraint(message: &[u8]) -> Option<KernelOomMechanism> {
    if contains_field_value(message, b"constraint=CONSTRAINT_MEMCG") {
        Some(KernelOomMechanism::Memcg)
    } else if contains_field_value(message, b"constraint=CONSTRAINT_NONE") {
        Some(KernelOomMechanism::Global)
    } else {
        None
    }
}

fn parse_prefixed_victim(message: &[u8], prefix: &[u8]) -> Option<ParsedVictim> {
    let value = message.strip_prefix(prefix)?;
    let (pid, remainder) = parse_pid(value)?;
    let remainder = remainder.strip_prefix(b" (")?;
    let close = remainder.iter().position(|byte| *byte == b')')?;
    let process = KernelProcessToken::new(&remainder[..close])?;
    if remainder
        .get(close + 1)
        .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(ParsedVictim { pid, process })
}

fn parse_pid(value: &[u8]) -> Option<(u32, &[u8])> {
    let end = value
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(value.len());
    if end == 0 {
        return None;
    }
    let mut pid = 0u32;
    for digit in &value[..end] {
        pid = pid.checked_mul(10)?.checked_add(u32::from(*digit - b'0'))?;
    }
    (pid != 0).then_some((pid, &value[end..]))
}

fn contains_field_value(message: &[u8], field: &[u8]) -> bool {
    let mut search_start = 0;
    while let Some(relative) = find_subslice(&message[search_start..], field) {
        let position = search_start + relative;
        let before_ok = position == 0
            || message[position - 1].is_ascii_whitespace()
            || matches!(message[position - 1], b':' | b',' | b';');
        let after = position + field.len();
        let after_ok = message
            .get(after)
            .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(*byte, b',' | b';' | b')'));
        if before_ok && after_ok {
            return true;
        }
        search_start = position + 1;
    }
    false
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find_subslice(haystack, needle).is_some()
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

    #[test]
    fn direct_global_and_memcg_victims_are_mutually_exclusive() {
        let mut recognizer = KernelOomRecognizer::new();
        let global = recognizer
            .observe(
                1,
                "kernel",
                b"Out of memory: Killed process 42 (com.example) total-vm:99999kB anon-rss:1234kB",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .unwrap();
        let memcg = recognizer
            .observe(
                2,
                "kernel",
                b"Memory cgroup out of memory: Killed process 43 (com.example) total-vm:1kB",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .unwrap();
        assert_eq!(global.mechanism, KernelOomMechanism::Global);
        assert_eq!(memcg.mechanism, KernelOomMechanism::Memcg);
        assert_ne!(global.fingerprint, memcg.fingerprint);
        assert!(!global.outcome.death_observed);
        assert!(!memcg.outcome.death_observed);
    }

    #[test]
    fn known_raw_printk_prefix_is_removed_before_exact_grammar_but_unknown_is_rejected() {
        let kernel = LineProvenance::Known(LogBuffer::Kernel);
        let direct = KernelOomRecognizer::new()
            .observe(
                1,
                "",
                b"<3>[  12.345678] Out of memory: Killed process 42 (com.example.raw) total-vm:99999kB",
                kernel,
            )
            .expect("known raw printk direct victim");
        assert_eq!(direct.mechanism, KernelOomMechanism::Global);
        assert_eq!(direct.process.as_str(), "com.example.raw");

        let mut paired = KernelOomRecognizer::new();
        assert!(paired
            .observe(
                10,
                "",
                b"<3>[  12.400000] oom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null)",
                kernel,
            )
            .is_none());
        let memcg = paired
            .observe(
                11,
                "",
                b"<3>[  12.400001] Killed process 43 (com.example.raw.memcg) total-vm:1kB",
                kernel,
            )
            .expect("known raw printk constraint and victim");
        assert_eq!(memcg.mechanism, KernelOomMechanism::Memcg);

        assert!(KernelOomRecognizer::new()
            .observe(
                1,
                "",
                b"<3>[  12.345678] Out of memory: Killed process 42 (com.example.raw)",
                LineProvenance::Unknown,
            )
            .is_none());
    }

    #[test]
    fn consecutive_invoked_and_constraint_openers_form_one_episode() {
        let kernel = LineProvenance::Known(LogBuffer::Kernel);
        let mut recognizer = KernelOomRecognizer::new();
        assert!(recognizer
            .observe(
                10,
                "kernel",
                b"synthetic-worker invoked oom-killer: gfp_mask=0x0",
                kernel,
            )
            .is_none());
        assert!(recognizer
            .observe(
                11,
                "kernel",
                b"oom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null)",
                kernel,
            )
            .is_none());
        assert_eq!(recognizer.pending_count(), 1);

        let occurrence = recognizer
            .observe(
                12,
                "kernel",
                b"Killed process 44 (com.example.combined) total-vm:42kB",
                kernel,
            )
            .expect("one victim resolves the combined episode");
        assert_eq!(occurrence.mechanism, KernelOomMechanism::Memcg);
        assert_eq!(recognizer.ambiguity_count(), 0);
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn pending_episode_accepts_511_and_512_lines_but_expires_at_513() {
        let kernel = LineProvenance::Known(LogBuffer::Kernel);
        for (victim_line, expected) in [(511, true), (512, true), (513, false)] {
            let mut recognizer = KernelOomRecognizer::new();
            assert!(recognizer
                .observe(
                    0,
                    "kernel",
                    b"oom-kill:constraint=CONSTRAINT_NONE,nodemask=(null)",
                    kernel,
                )
                .is_none());
            assert_eq!(
                recognizer
                    .observe(
                        victim_line,
                        "kernel",
                        b"Killed process 45 (com.example.boundary) total-vm:42kB",
                        kernel,
                    )
                    .is_some(),
                expected,
                "victim at source-line distance {victim_line}"
            );
        }
    }

    #[test]
    fn pid_and_dynamic_memory_counters_do_not_split_fingerprint() {
        let mut recognizer = KernelOomRecognizer::new();
        let first = recognizer
            .observe(
                1,
                "kernel",
                b"Out of memory: Killed process 1 (same.app) total-vm:1kB",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .unwrap();
        let second = recognizer
            .observe(
                2,
                "kernel",
                b"Out of memory: Killed process 9999 (same.app) total-vm:999999kB",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_ne!(first.victim_pid, second.victim_pid);
    }

    #[test]
    fn constraint_candidate_needs_a_later_victim_and_truncated_input_does_not_commit() {
        let mut recognizer = KernelOomRecognizer::new();
        assert!(recognizer
            .observe(
                10,
                "kernel",
                b"oom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null),cpuset=/uid_1000",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .is_none());
        assert_eq!(recognizer.pending_count(), 1);
        let occurrence = recognizer
            .observe(
                11,
                "kernel",
                b"Killed process 77 (com.example) total-vm:123kB",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .unwrap();
        assert_eq!(occurrence.mechanism, KernelOomMechanism::Memcg);
        assert_eq!(recognizer.pending_count(), 0);

        recognizer.observe(
            20,
            "kernel",
            b"oom-kill:constraint=CONSTRAINT_NONE,nodemask=(null)",
            LineProvenance::Known(LogBuffer::Kernel),
        );
        assert_eq!(recognizer.finish_input(), 1);
    }

    #[test]
    fn unknown_inferred_and_non_kernel_tags_never_open_or_commit() {
        let message = b"Out of memory: Killed process 42 (app)";
        for provenance in [
            LineProvenance::Unknown,
            LineProvenance::Inferred(LogBuffer::Kernel),
        ] {
            assert!(KernelOomRecognizer::new()
                .observe(1, "kernel", message, provenance)
                .is_none());
        }
        assert!(KernelOomRecognizer::new()
            .observe(
                1,
                "ActivityManager",
                message,
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .is_none());
    }

    #[test]
    fn legacy_lmk_java_oom_and_constraint_near_matches_are_not_kernel_oom() {
        let mut recognizer = KernelOomRecognizer::new();
        for message in [
            b"lowmemorykiller: Killing 'app' (42), adj 900, to free 1kB".as_slice(),
            b"java.lang.OutOfMemoryError".as_slice(),
            b"oom-kill:constraint=CONSTRAINT_MEMCG_FAKE,nodemask=(null)".as_slice(),
            b"Out of memory pressure is rising".as_slice(),
            b"Killed process 42 (app)".as_slice(),
        ] {
            assert!(recognizer
                .observe(
                    1,
                    "kernel",
                    message,
                    LineProvenance::Known(LogBuffer::Kernel),
                )
                .is_none());
        }
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn multiple_pending_candidates_make_an_unqualified_victim_ambiguous() {
        let mut recognizer = KernelOomRecognizer::new();
        for (line, constraint) in [
            (1, b"oom-kill:constraint=CONSTRAINT_NONE".as_slice()),
            (2, b"oom-kill:constraint=CONSTRAINT_MEMCG".as_slice()),
        ] {
            recognizer.observe(
                line,
                "kernel",
                constraint,
                LineProvenance::Known(LogBuffer::Kernel),
            );
        }
        assert!(recognizer
            .observe(
                3,
                "kernel",
                b"Killed process 42 (app)",
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .is_none());
        assert_eq!(recognizer.ambiguity_count(), 1);
    }

    #[test]
    fn pending_state_is_fixed_capacity_fifo_and_expires_by_source_line() {
        let mut recognizer = KernelOomRecognizer::new();
        for line in 1..=MAX_PENDING_KERNEL_OOM as u32 + 1 {
            recognizer.observe(
                line,
                "kernel",
                b"oom-kill:constraint=CONSTRAINT_NONE",
                LineProvenance::Known(LogBuffer::Kernel),
            );
        }
        assert_eq!(recognizer.pending_count(), MAX_PENDING_KERNEL_OOM);
        assert_eq!(recognizer.pending_eviction_count(), 1);
        let after_all_pending_windows =
            MAX_KERNEL_OOM_SPAN_LINES + MAX_PENDING_KERNEL_OOM as u32 + 2;
        recognizer.observe(
            after_all_pending_windows,
            "kernel",
            b"unrelated",
            LineProvenance::Known(LogBuffer::Kernel),
        );
        assert_eq!(recognizer.pending_count(), 0);
    }

    #[test]
    fn invalid_utf8_overlong_and_truncated_victim_lines_are_rejected() {
        assert_eq!(
            size_of::<KernelProcessToken>(),
            MAX_KERNEL_PROCESS_NAME_BYTES
        );
        assert!(!needs_drop::<KernelProcessToken>());
        let mut recognizer = KernelOomRecognizer::new();
        for message in [
            b"Out of memory: Killed process ".as_slice(),
            b"Out of memory: Killed process 42".as_slice(),
            b"Out of memory: Killed process 42 (bad\xff)".as_slice(),
            b"Out of memory: Killed process 42 (app)garbage".as_slice(),
        ] {
            assert!(recognizer
                .observe(
                    1,
                    "kernel",
                    message,
                    LineProvenance::Known(LogBuffer::Kernel),
                )
                .is_none());
        }
        let overlong = vec![b'a'; MAX_KERNEL_OOM_INPUT_BYTES + 1];
        assert!(recognizer
            .observe(
                1,
                "kernel",
                &overlong,
                LineProvenance::Known(LogBuffer::Kernel),
            )
            .is_none());
    }
}
