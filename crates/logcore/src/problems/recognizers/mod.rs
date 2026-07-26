mod anr;
mod java;
mod kernel_oom;
mod lmk;
mod native;
mod process;

pub(super) use anr::AnrRecognizer;
pub(super) use java::JavaRecognizer;
pub(super) use kernel_oom::{KernelOomOccurrence, KernelOomRecognizer};
pub(super) use lmk::{LmkMechanism, LmkOccurrence, LmkRecognizer};
pub(super) use native::NativeRecognizer;
pub(super) use process::{
    parse_zygote_signal_exit, LifecycleOccurrence, LifecycleRecognizer, LifecycleRecognizerError,
    LifecycleRelation, LifecycleTime,
};

use super::engine::{ObservedLine, RecognizedProblem};

pub(super) const MAX_PHYSICAL_LINE_BYTES: usize = 1024 * 1024;

pub(super) trait ProblemRecognizer {
    fn observe(&mut self, line: &ObservedLine<'_>);
    fn finish_input(&mut self);
    fn reset(&mut self);
    fn pop_ready(&mut self) -> Option<RecognizedProblem>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FixedText<const N: usize> {
    bytes: [u8; N],
    len: u16,
}

impl<const N: usize> Default for FixedText<N> {
    fn default() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> FixedText<N> {
    pub(super) fn set(&mut self, value: &str) -> bool {
        let bytes = value.as_bytes();
        let Ok(len) = u16::try_from(bytes.len()) else {
            return false;
        };
        if bytes.len() > N {
            return false;
        }
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        self.len = len;
        true
    }

    pub(super) fn clear(&mut self) {
        self.len = 0;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("FixedText only copies complete UTF-8 strings")
    }
}

pub(super) fn parse_pid(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok().filter(|pid| *pid != 0)
}

pub(super) fn trim_ascii(value: &str) -> &str {
    value.trim_matches(char::is_whitespace)
}
