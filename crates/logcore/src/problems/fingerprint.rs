use super::model::{IdentityQuality, ProblemKind, SignatureQuality};
use std::fmt;

const FINGERPRINT_CONTEXT: &str = "LogFilter Problems fingerprint BLAKE3-128 v1";
const PROCESS_NAME_LIMIT: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProcessFingerprintKey {
    Known(Box<str>),
    Unknown,
}

impl ProcessFingerprintKey {
    pub fn new(process_name: Option<&str>) -> Self {
        let Some(normalized) = process_name.map(str::trim).filter(|name| !name.is_empty()) else {
            return Self::Unknown;
        };
        if normalized.len() > PROCESS_NAME_LIMIT {
            return Self::Unknown;
        }
        Self::Known(normalized.into())
    }

    pub const fn unknown() -> Self {
        Self::Unknown
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Known(name) => Some(name.as_bytes()),
            Self::Unknown => None,
        }
    }

    pub fn identity_quality(&self) -> IdentityQuality {
        match self {
            Self::Known(_) => IdentityQuality::KnownProcess,
            Self::Unknown => IdentityQuality::UnknownProcess,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ProblemFingerprint([u8; 16]);

impl ProblemFingerprint {
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        self.to_string()
    }
}

impl fmt::Display for ProblemFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FingerprintTokenKind {
    ExceptionType = 1,
    Frame = 2,
    Signal = 3,
    StructuredField = 4,
    Mechanism = 5,
    Relation = 6,
}

pub struct FingerprintBuilder {
    hasher: blake3::Hasher,
}

impl FingerprintBuilder {
    pub fn new(
        kind: ProblemKind,
        fingerprint_version: u16,
        signature_quality: SignatureQuality,
        identity_quality: IdentityQuality,
        process: &ProcessFingerprintKey,
    ) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key(FINGERPRINT_CONTEXT);
        hasher.update(&[1]); // Binary protocol version.
        hasher.update(&[kind as u8]);
        hasher.update(&fingerprint_version.to_le_bytes());
        hasher.update(&[signature_quality as u8, identity_quality as u8]);
        match process.as_bytes() {
            Some(name) => {
                hasher.update(&[1]);
                write_length_prefixed(&mut hasher, name);
            }
            None => {
                hasher.update(&[0]);
            }
        };
        Self { hasher }
    }

    pub fn token(&mut self, kind: FingerprintTokenKind, canonical: &[u8]) -> &mut Self {
        self.hasher.update(&[kind as u8]);
        write_length_prefixed(&mut self.hasher, canonical);
        self
    }

    pub fn finish(self) -> ProblemFingerprint {
        let digest = self.hasher.finalize();
        let mut compact = [0; 16];
        compact.copy_from_slice(&digest.as_bytes()[..16]);
        ProblemFingerprint(compact)
    }
}

fn write_length_prefixed(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problems::{IdentityQuality, ProblemKind, SignatureQuality};

    fn java_fingerprint(signature: SignatureQuality) -> ProblemFingerprint {
        let process = ProcessFingerprintKey::new(Some(" com.example.app "));
        let mut builder = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            signature,
            IdentityQuality::KnownProcess,
            &process,
        );
        builder
            .token(
                FingerprintTokenKind::ExceptionType,
                b"java.lang.IllegalStateException",
            )
            .token(
                FingerprintTokenKind::Frame,
                b"com.example.MainActivity#onCreate",
            );
        builder.finish()
    }

    #[test]
    fn process_fingerprint_key_normalizes_only_the_process_name() {
        assert_eq!(
            ProcessFingerprintKey::new(Some("  com.example.app:worker  ")).as_bytes(),
            Some(b"com.example.app:worker".as_slice())
        );
        assert!(ProcessFingerprintKey::new(Some("  ")).is_unknown());
        assert!(ProcessFingerprintKey::new(None).is_unknown());
    }

    #[test]
    fn fingerprint_has_a_frozen_blake3_128_golden_value() {
        assert_eq!(
            java_fingerprint(SignatureQuality::FullStack).to_hex(),
            "a17d382edecc4fb11c0d94b2c0791c3c"
        );
    }

    #[test]
    fn signature_and_identity_quality_are_orthogonal_domains() {
        let full = java_fingerprint(SignatureQuality::FullStack);
        let type_only = java_fingerprint(SignatureQuality::TypeOnly);
        assert_ne!(full, type_only);

        let process = ProcessFingerprintKey::new(Some("com.example.app"));
        let known = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::FullStack,
            IdentityQuality::KnownProcess,
            &process,
        )
        .finish();
        let unknown_identity = FingerprintBuilder::new(
            ProblemKind::JavaCrash,
            1,
            SignatureQuality::FullStack,
            IdentityQuality::UnknownProcess,
            &process,
        )
        .finish();
        assert_ne!(known, unknown_identity);
    }

    #[test]
    fn kind_version_and_process_name_each_split_groups() {
        fn bare(
            kind: ProblemKind,
            version: u16,
            process: &ProcessFingerprintKey,
        ) -> ProblemFingerprint {
            FingerprintBuilder::new(
                kind,
                version,
                SignatureQuality::Minimal,
                process.identity_quality(),
                process,
            )
            .finish()
        }

        let app = ProcessFingerprintKey::new(Some("com.example.app"));
        let worker = ProcessFingerprintKey::new(Some("com.example.app:worker"));
        let baseline = bare(ProblemKind::JavaCrash, 1, &app);
        assert_ne!(baseline, bare(ProblemKind::Anr, 1, &app));
        assert_ne!(baseline, bare(ProblemKind::JavaCrash, 2, &app));
        assert_ne!(baseline, bare(ProblemKind::JavaCrash, 1, &worker));
    }

    #[test]
    fn length_prefixes_make_token_boundaries_unambiguous() {
        let process = ProcessFingerprintKey::unknown();
        let mut left = FingerprintBuilder::new(
            ProblemKind::Anr,
            1,
            SignatureQuality::StructuredFields,
            IdentityQuality::UnknownProcess,
            &process,
        );
        left.token(FingerprintTokenKind::StructuredField, b"ab")
            .token(FingerprintTokenKind::StructuredField, b"c");

        let mut right = FingerprintBuilder::new(
            ProblemKind::Anr,
            1,
            SignatureQuality::StructuredFields,
            IdentityQuality::UnknownProcess,
            &process,
        );
        right
            .token(FingerprintTokenKind::StructuredField, b"a")
            .token(FingerprintTokenKind::StructuredField, b"bc");

        assert_ne!(left.finish(), right.finish());
    }
}
