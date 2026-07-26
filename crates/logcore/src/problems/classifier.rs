use crate::parser::ParsedLine;

/// 廉价候选类别位集。分类只决定“哪些确定性 parser 值得看这一行”，不提交 Problem。
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CandidateKinds(u16);

impl CandidateKinds {
    pub const EMPTY: Self = Self(0);
    pub const JAVA_FATAL: Self = Self(1 << 0);
    pub const JAVA_OOM: Self = Self(1 << 1);
    pub const ANR: Self = Self(1 << 2);
    pub const EVENT_LOG: Self = Self(1 << 3);
    pub const NATIVE_CRASH: Self = Self(1 << 4);
    pub const LIFECYCLE: Self = Self(1 << 5);
    pub const LMK: Self = Self(1 << 6);
    pub const KERNEL_OOM: Self = Self(1 << 7);
    pub const CONTINUATION: Self = Self(1 << 8);

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for CandidateKinds {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for CandidateKinds {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// 无分配候选预筛。
///
/// 策略有意保守：
/// - 带 logcat prefix 的候选开始必须使用大小写敏感的精确平台 tag。
/// - 无 tag 的 raw 行只接受严格 tombstone/kernel/legacy-LMK 起始文本。
/// - allowlist continuation 只设置 `CONTINUATION`；它不能自行打开或提交事件，是否附着
///   仍由已有 recognizer 状态、producer 与歧义规则决定。
pub fn classify_candidate(line: &ParsedLine<'_>, raw_bytes: &[u8]) -> CandidateKinds {
    let tag = line.tag.as_bytes();
    let message = line.message.as_bytes();
    let mut kinds = CandidateKinds::EMPTY;

    match tag {
        b"AndroidRuntime" => {
            if is_error_or_fatal_level(line.level.as_bytes())
                && (starts_with_payload(message, b"FATAL EXCEPTION:")
                    || starts_with_payload(message, b"*** FATAL EXCEPTION IN SYSTEM PROCESS:"))
            {
                kinds |= CandidateKinds::JAVA_FATAL;
            }
            if is_java_oom_text(message) {
                kinds |= CandidateKinds::JAVA_OOM;
            }
            if kinds.is_empty() && is_tagged_continuation(message) {
                kinds |= CandidateKinds::CONTINUATION;
            }
        }
        b"art" | b"dalvikvm" if is_java_oom_text(message) => {
            kinds |= CandidateKinds::JAVA_OOM;
        }
        b"ActivityManager" => {
            if starts_with_payload(message, b"ANR in ") {
                kinds |= CandidateKinds::ANR;
            }
            if (starts_with_payload(message, b"Process ") && contains_ascii(message, b" has died"))
                || starts_with_payload(message, b"Start proc ")
            {
                kinds |= CandidateKinds::LIFECYCLE;
            }
            if kinds.is_empty() && is_tagged_continuation(message) {
                kinds |= CandidateKinds::CONTINUATION;
            }
        }
        b"am_crash" => {
            kinds |= CandidateKinds::EVENT_LOG;
        }
        b"am_anr" => {
            kinds |= CandidateKinds::EVENT_LOG | CandidateKinds::ANR;
        }
        b"am_proc_start" | b"am_proc_died" | b"am_kill" => {
            kinds |= CandidateKinds::EVENT_LOG | CandidateKinds::LIFECYCLE;
        }
        b"Zygote"
            if starts_with_payload(message, b"Process ")
                && contains_ascii(message, b" exited due to signal ") =>
        {
            kinds |= CandidateKinds::LIFECYCLE;
        }
        b"libc" => {
            if starts_with_payload(message, b"Fatal signal ") {
                kinds |= CandidateKinds::NATIVE_CRASH;
            }
            if kinds.is_empty() && is_tagged_continuation(message) {
                kinds |= CandidateKinds::CONTINUATION;
            }
        }
        b"DEBUG" | b"debuggerd" => {
            if is_tombstone_separator(message) {
                kinds |= CandidateKinds::NATIVE_CRASH;
            }
            if kinds.is_empty() && is_tagged_continuation(message) {
                kinds |= CandidateKinds::CONTINUATION;
            }
        }
        b"lmkd" if starts_with_payload(message, b"Kill '") => {
            kinds |= CandidateKinds::LMK;
        }
        b"lowmemorykiller"
            if starts_with_payload(message, b"Kill '")
                || starts_with_payload(message, b"Killing '") =>
        {
            kinds |= CandidateKinds::LMK;
        }
        b"kernel" => {
            if is_kernel_oom_text(message) {
                kinds |= CandidateKinds::KERNEL_OOM;
            }
            if contains_ascii(message, b"lowmemorykiller: Killing '") {
                kinds |= CandidateKinds::LMK;
            }
        }
        b"" => {
            let raw = trim_ascii_start(raw_bytes);
            if is_tombstone_separator(raw) {
                kinds |= CandidateKinds::NATIVE_CRASH;
            } else if is_kernel_oom_text(raw) {
                kinds |= CandidateKinds::KERNEL_OOM;
            } else if contains_ascii(raw, b"lowmemorykiller: Killing '") {
                kinds |= CandidateKinds::LMK;
            } else if is_raw_continuation(raw) {
                kinds |= CandidateKinds::CONTINUATION;
            }
        }
        _ => {}
    }

    kinds
}

fn starts_with_payload(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes
        .strip_prefix(prefix)
        .is_some_and(|remaining| !remaining.is_empty())
}

fn is_error_or_fatal_level(level: &[u8]) -> bool {
    matches!(level, b"E" | b"F")
}

fn is_java_oom_text(message: &[u8]) -> bool {
    contains_ascii(message, b"java.lang.OutOfMemoryError")
        || starts_with_payload(message, b"Throwing OutOfMemoryError")
        || starts_with_payload(message, b"Out of memory on a ")
}

fn is_tombstone_separator(bytes: &[u8]) -> bool {
    bytes.starts_with(b"*** *** *** *** *** *** *** ***")
}

fn is_kernel_oom_text(bytes: &[u8]) -> bool {
    contains_ascii(bytes, b"invoked oom-killer:")
        || contains_ascii(bytes, b"oom-kill:constraint=")
        || contains_ascii(bytes, b"Out of memory: Killed process ")
        || contains_ascii(bytes, b"Memory cgroup out of memory: Killed process ")
}

fn is_tagged_continuation(message: &[u8]) -> bool {
    is_raw_continuation(trim_ascii_start(message))
}

fn is_raw_continuation(bytes: &[u8]) -> bool {
    [
        b"at ".as_slice(),
        b"Caused by: ".as_slice(),
        b"Suppressed: ".as_slice(),
        b"... ".as_slice(),
        b"Process: ".as_slice(),
        b"PID: ".as_slice(),
        b"pid: ".as_slice(),
        b"Reason: ".as_slice(),
        b"Subject: ".as_slice(),
        b"Cmdline: ".as_slice(),
        b"Abort message: ".as_slice(),
        b"signal ".as_slice(),
        b"backtrace:".as_slice(),
        b">>> ".as_slice(),
        b"java.".as_slice(),
        b"javax.".as_slice(),
        b"kotlin.".as_slice(),
        b"android.".as_slice(),
    ]
    .iter()
    .any(|prefix| bytes.starts_with(prefix))
        || is_native_frame_prefix(bytes)
}

fn is_native_frame_prefix(bytes: &[u8]) -> bool {
    matches!(
        bytes,
        [b'#', first, second, b' ', ..]
            if first.is_ascii_digit() && second.is_ascii_digit()
    )
}

fn contains_ascii(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn trim_ascii_start(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes {
        if first.is_ascii_whitespace() {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{needs_drop, size_of};

    fn parsed<'a>(tag: &'a str, message: &'a str) -> ParsedLine<'a> {
        ParsedLine {
            level: "E",
            tag,
            message,
            ..Default::default()
        }
    }

    #[test]
    fn candidate_kinds_is_a_compact_copy_bitset() {
        assert_eq!(size_of::<CandidateKinds>(), 2);
        assert!(!needs_drop::<CandidateKinds>());
        let kinds = CandidateKinds::JAVA_FATAL | CandidateKinds::JAVA_OOM;
        let copied = kinds;
        assert_eq!(copied, kinds);
    }

    #[test]
    fn android_runtime_requires_exact_tag_and_strict_fatal_or_oom_text() {
        let fatal = parsed("AndroidRuntime", "FATAL EXCEPTION: main");
        assert_eq!(
            classify_candidate(&fatal, b"ignored"),
            CandidateKinds::JAVA_FATAL
        );

        let system = parsed(
            "AndroidRuntime",
            "*** FATAL EXCEPTION IN SYSTEM PROCESS: system_server",
        );
        assert_eq!(
            classify_candidate(&system, b"ignored"),
            CandidateKinds::JAVA_FATAL
        );

        let oom = parsed(
            "AndroidRuntime",
            "java.lang.OutOfMemoryError: Failed to allocate",
        );
        assert_eq!(
            classify_candidate(&oom, b"ignored"),
            CandidateKinds::JAVA_OOM
        );

        let fatal_oom = parsed(
            "AndroidRuntime",
            "FATAL EXCEPTION: main java.lang.OutOfMemoryError",
        );
        assert_eq!(
            classify_candidate(&fatal_oom, b"ignored"),
            CandidateKinds::JAVA_FATAL | CandidateKinds::JAVA_OOM
        );

        assert!(classify_candidate(
            &parsed("App", "FATAL EXCEPTION: copied text"),
            b"FATAL EXCEPTION: copied text"
        )
        .is_empty());
        assert!(classify_candidate(
            &parsed("androidruntime", "FATAL EXCEPTION: main"),
            b"FATAL EXCEPTION: main"
        )
        .is_empty());
        assert!(classify_candidate(
            &parsed("AndroidRuntime", "fatal exception: main"),
            b"fatal exception: main"
        )
        .is_empty());
        let mut info = parsed("AndroidRuntime", "FATAL EXCEPTION: copied text");
        info.level = "I";
        assert!(classify_candidate(&info, b"ignored").is_empty());
    }

    #[test]
    fn anr_and_eventlog_candidates_use_exact_platform_tags() {
        assert_eq!(
            classify_candidate(
                &parsed("ActivityManager", "ANR in com.example.app"),
                b"ignored"
            ),
            CandidateKinds::ANR
        );
        assert_eq!(
            classify_candidate(&parsed("am_anr", "[0,123,com.example,reason]"), b"ignored"),
            CandidateKinds::EVENT_LOG | CandidateKinds::ANR
        );
        assert_eq!(
            classify_candidate(&parsed("am_crash", "[0,123,com.example,...]"), b"ignored"),
            CandidateKinds::EVENT_LOG
        );
        for tag in ["am_proc_start", "am_proc_died", "am_kill"] {
            assert_eq!(
                classify_candidate(&parsed(tag, "[0,123,com.example]"), b"ignored"),
                CandidateKinds::EVENT_LOG | CandidateKinds::LIFECYCLE,
                "tag {tag}"
            );
        }
        assert_eq!(
            classify_candidate(
                &parsed("ActivityManager", "Process com.example (pid 123) has died"),
                b"ignored"
            ),
            CandidateKinds::LIFECYCLE
        );
        assert_eq!(
            classify_candidate(
                &parsed("ActivityManager", "Start proc 456:com.example/u0a1"),
                b"ignored"
            ),
            CandidateKinds::LIFECYCLE
        );
        assert!(classify_candidate(
            &parsed("MyActivityManager", "ANR in com.example"),
            b"ignored"
        )
        .is_empty());
        assert!(classify_candidate(&parsed("am_low_memory", "[0,123]"), b"ignored").is_empty());
    }

    #[test]
    fn native_candidates_require_libc_signal_or_debuggerd_tombstone_start() {
        assert_eq!(
            classify_candidate(
                &parsed("libc", "Fatal signal 11 (SIGSEGV), code 1, fault addr 0x0"),
                b"ignored"
            ),
            CandidateKinds::NATIVE_CRASH
        );
        let separator = "*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***";
        for tag in ["DEBUG", "debuggerd"] {
            assert_eq!(
                classify_candidate(&parsed(tag, separator), b"ignored"),
                CandidateKinds::NATIVE_CRASH,
                "tag {tag}"
            );
        }
        assert!(classify_candidate(
            &parsed("App", "Fatal signal 11 (SIGSEGV)"),
            b"Fatal signal 11 (SIGSEGV)"
        )
        .is_empty());
        assert_eq!(
            classify_candidate(
                &parsed("DEBUG", "pid: 123, tid: 124, name: worker"),
                b"ignored"
            ),
            CandidateKinds::CONTINUATION
        );
    }

    #[test]
    fn lmk_candidates_exclude_selection_and_pressure_chatter() {
        assert_eq!(
            classify_candidate(
                &parsed(
                    "lmkd",
                    "Kill 'com.example' (123), uid 10001, to free 123kB; reason: low watermark"
                ),
                b"ignored"
            ),
            CandidateKinds::LMK
        );
        assert_eq!(
            classify_candidate(
                &parsed("lowmemorykiller", "Killing 'com.example' (123), adj 900"),
                b"ignored"
            ),
            CandidateKinds::LMK
        );
        for message in [
            "Skipping kill because process is protected",
            "Selecting victim with oom_score_adj 900",
            "pressure stall detected",
        ] {
            assert!(
                classify_candidate(&parsed("lmkd", message), b"ignored").is_empty(),
                "{message}"
            );
        }
        assert!(classify_candidate(
            &parsed("ActivityManager", "Killing 123:com.example"),
            b"ignored"
        )
        .is_empty());
    }

    #[test]
    fn raw_kernel_and_legacy_lmk_starts_are_bounded_ascii_checks() {
        let raw = parsed("", "");
        assert_eq!(
            classify_candidate(&raw, b"<3>[ 12.345] task invoked oom-killer: gfp_mask=0x0"),
            CandidateKinds::KERNEL_OOM
        );
        assert_eq!(
            classify_candidate(
                &raw,
                b"[ 12.346] Out of memory: Killed process 123 (com.example)"
            ),
            CandidateKinds::KERNEL_OOM
        );
        assert_eq!(
            classify_candidate(
                &raw,
                b"[ 12.347] lowmemorykiller: Killing 'com.example' (123)"
            ),
            CandidateKinds::LMK
        );

        let userspace = parsed("App", "Out of memory: Killed process 123");
        assert!(classify_candidate(&userspace, b"Out of memory: Killed process 123").is_empty());
        assert!(classify_candidate(&raw, b"out of memory in image cache").is_empty());
    }

    #[test]
    fn raw_continuations_are_allowlisted_and_never_open_a_candidate() {
        let raw = parsed("", "");
        for bytes in [
            b"    at com.example.Main.run(Main.kt:42)".as_slice(),
            b"Caused by: java.lang.IllegalStateException".as_slice(),
            b"Reason: Input dispatching timed out".as_slice(),
            b"Cmdline: com.example".as_slice(),
            b"    #00 pc 000000000001234 libfoo.so".as_slice(),
            b"#17 pc 000000000004321 libbar.so".as_slice(),
            b"pid: 123, tid: 124, name: worker".as_slice(),
            b">>> com.example <<<".as_slice(),
            b"java.lang.IllegalStateException: boom".as_slice(),
            b"kotlin.KotlinNullPointerException".as_slice(),
        ] {
            assert_eq!(
                classify_candidate(&raw, bytes),
                CandidateKinds::CONTINUATION
            );
        }

        assert_eq!(
            classify_candidate(
                &parsed("AndroidRuntime", "at com.example.Main.run(Main.kt:42)"),
                b"ignored"
            ),
            CandidateKinds::CONTINUATION
        );
        assert!(classify_candidate(&raw, b"ordinary application output").is_empty());
        assert!(classify_candidate(
            &parsed("App", "at com.example.Main.run(Main.kt:42)"),
            b"ignored"
        )
        .is_empty());
    }

    #[test]
    fn raw_tombstone_separator_is_a_native_start_not_only_a_continuation() {
        let raw = parsed("", "");
        assert_eq!(
            classify_candidate(
                &raw,
                b"*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***"
            ),
            CandidateKinds::NATIVE_CRASH
        );
    }
}
