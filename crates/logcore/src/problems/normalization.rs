use std::fmt;
use std::str;

pub const MAX_NORMALIZED_TOKEN_BYTES: usize = 256;
pub const MAX_NORMALIZATION_INPUT_BYTES: usize = 16 * 1024;

/// Fixed-size fingerprint input. Normalizers return `None` instead of truncating,
/// so two distinct overlong identities cannot silently collapse into one token.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct NormalizedToken([u8; MAX_NORMALIZED_TOKEN_BYTES]);

impl NormalizedToken {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0[..self.len()]
    }

    pub fn as_str(&self) -> &str {
        str::from_utf8(self.as_bytes())
            .expect("normalizers only construct tokens from validated UTF-8")
    }

    pub fn len(&self) -> usize {
        self.0
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(MAX_NORMALIZED_TOKEN_BYTES)
    }

    pub fn is_empty(&self) -> bool {
        self.0[0] == 0
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty()
            || bytes.len() > MAX_NORMALIZED_TOKEN_BYTES
            || bytes.contains(&0)
            || str::from_utf8(bytes).is_err()
        {
            return None;
        }
        let mut token = Self([0; MAX_NORMALIZED_TOKEN_BYTES]);
        token.0[..bytes.len()].copy_from_slice(bytes);
        Some(token)
    }

    #[cfg(test)]
    fn from_static(value: &'static str) -> Self {
        Self::from_bytes(value.as_bytes()).expect("test token must fit")
    }
}

impl fmt::Debug for NormalizedToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("NormalizedToken")
            .field(&self.as_str())
            .finish()
    }
}

impl fmt::Display for NormalizedToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub fn normalize_java_throwable(input: &[u8]) -> Option<NormalizedToken> {
    validate_input(input)?;
    let mut value = trim_ascii(input);
    for prefix in [b"Caused by:".as_slice(), b"Suppressed:".as_slice()] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = trim_ascii(rest);
            break;
        }
    }
    let end = value
        .iter()
        .position(|byte| byte.is_ascii_whitespace() || *byte == b':')
        .unwrap_or(value.len());
    let class = &value[..end];
    if !is_java_class(class)
        || ![
            b"Exception".as_slice(),
            b"Error".as_slice(),
            b"Throwable".as_slice(),
        ]
        .iter()
        .any(|suffix| class.ends_with(suffix))
    {
        return None;
    }
    NormalizedToken::from_bytes(class)
}

pub fn normalize_java_frame(input: &[u8]) -> Option<NormalizedToken> {
    validate_input(input)?;
    let value = trim_ascii(input).strip_prefix(b"at ")?;
    let open_paren = value.iter().position(|byte| *byte == b'(')?;
    let qualified = trim_ascii(&value[..open_paren]);
    let separator = qualified.iter().rposition(|byte| *byte == b'.')?;
    let class = &qualified[..separator];
    let method = &qualified[separator + 1..];
    if !is_java_class(class) || !is_java_method(method) {
        return None;
    }

    let mut writer = TokenWriter::new();
    writer.push_java_identifier(class)?;
    writer.push(b"#")?;
    writer.push_java_identifier(method)?;
    writer.finish()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnrReasonCategory {
    InputDispatchTimeout,
    BroadcastTimeout,
    ServiceTimeout,
    ContentProviderTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalizedAnrReason {
    pub category: AnrReasonCategory,
    pub canonical: NormalizedToken,
}

pub fn normalize_anr_reason(input: &[u8]) -> Option<NormalizedAnrReason> {
    validate_input(input)?;
    let reason = trim_ascii(input);

    if has_prefix_boundary(reason, b"Input dispatching timed out") {
        return Some(NormalizedAnrReason {
            category: AnrReasonCategory::InputDispatchTimeout,
            canonical: fixed_token(b"input-dispatch-timeout"),
        });
    }

    if let Some(payload) = reason.strip_prefix(b"Broadcast of Intent {") {
        let action_start = find_field(payload, b"act=")? + b"act=".len();
        let action = take_while(&payload[action_start..], is_action_byte);
        if action
            .first()
            .is_none_or(|byte| !byte.is_ascii_alphabetic())
        {
            return None;
        }
        let mut writer = TokenWriter::new();
        writer.push(b"broadcast:")?;
        writer.push(action)?;
        return Some(NormalizedAnrReason {
            category: AnrReasonCategory::BroadcastTimeout,
            canonical: writer.finish()?,
        });
    }

    if let Some(payload) = reason.strip_prefix(b"executing service ") {
        let component = take_while(payload, is_component_byte);
        if component.is_empty() || !component.contains(&b'/') {
            return None;
        }
        let mut writer = TokenWriter::new();
        writer.push(b"service:")?;
        writer.push(component)?;
        return Some(NormalizedAnrReason {
            category: AnrReasonCategory::ServiceTimeout,
            canonical: writer.finish()?,
        });
    }

    if has_prefix_boundary(
        reason,
        b"Context.startForegroundService() did not then call Service.startForeground()",
    ) {
        return Some(NormalizedAnrReason {
            category: AnrReasonCategory::ServiceTimeout,
            canonical: fixed_token(b"foreground-service-start-timeout"),
        });
    }

    if has_prefix_boundary(reason, b"ContentProvider not responding") {
        return Some(NormalizedAnrReason {
            category: AnrReasonCategory::ContentProviderTimeout,
            canonical: fixed_token(b"content-provider-timeout"),
        });
    }

    None
}

pub fn normalize_native_frame(input: &[u8]) -> Option<NormalizedToken> {
    validate_input(input)?;
    let value = trim_ascii(input);
    let mut cursor = 0;
    if value.get(cursor) != Some(&b'#') {
        return None;
    }
    cursor += 1;
    let frame_digits = take_while(&value[cursor..], |byte| byte.is_ascii_digit());
    if frame_digits.is_empty() {
        return None;
    }
    cursor += frame_digits.len();
    cursor = skip_ascii_whitespace(value, cursor);
    if value.get(cursor..cursor + 2)? != b"pc" {
        return None;
    }
    cursor += 2;
    if !value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    cursor = skip_ascii_whitespace(value, cursor);
    let relative_pc = take_while(&value[cursor..], |byte| byte.is_ascii_hexdigit());
    if relative_pc.is_empty() {
        return None;
    }
    cursor += relative_pc.len();
    if !value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        return None;
    }
    cursor = skip_ascii_whitespace(value, cursor);
    let module_path = take_while(&value[cursor..], |byte| !byte.is_ascii_whitespace());
    let module = module_basename(module_path)?;
    if !module.iter().copied().all(is_module_byte) {
        return None;
    }
    cursor += module_path.len();
    let remainder = trim_ascii(&value[cursor..]);

    if let Some(symbol) = native_symbol(remainder) {
        let mut writer = TokenWriter::new();
        writer.push(module)?;
        writer.push(b"#")?;
        writer.push(symbol)?;
        return writer.finish();
    }

    let build_id = build_id(remainder)?;
    let relative_pc = trim_leading_zeroes(relative_pc);
    let mut writer = TokenWriter::new();
    writer.push_ascii_lowercase(build_id)?;
    writer.push(b"+")?;
    writer.push(module)?;
    writer.push(b"+")?;
    writer.push_ascii_lowercase(relative_pc)?;
    writer.finish()
}

pub fn normalize_lmk_reason(input: &[u8]) -> Option<NormalizedToken> {
    validate_input(input)?;
    let message = trim_ascii(input);
    if message.starts_with(b"lowmemorykiller: Killing ") {
        return Some(fixed_token(b"legacy-lowmemorykiller"));
    }

    let marker = find_subslice(message, b"reason:")?;
    let reason = trim_ascii_start(&message[marker + b"reason:".len()..]);
    let token = if has_prefix_boundary(reason, b"low watermark") {
        b"lmkd:low-watermark".as_slice()
    } else if has_prefix_boundary(reason, b"process is thrashing")
        || has_prefix_boundary(reason, b"thrashing")
    {
        b"lmkd:thrashing".as_slice()
    } else if has_prefix_boundary(reason, b"memory pressure") || has_prefix_boundary(reason, b"psi")
    {
        b"lmkd:memory-pressure".as_slice()
    } else if has_prefix_boundary(reason, b"swap is low")
        || has_prefix_boundary(reason, b"swap low")
    {
        b"lmkd:swap-low".as_slice()
    } else if has_prefix_boundary(reason, b"direct reclaim") {
        b"lmkd:direct-reclaim".as_slice()
    } else {
        return None;
    };
    Some(fixed_token(token))
}

pub fn normalize_kernel_oom_mechanism(input: &[u8]) -> Option<NormalizedToken> {
    validate_input(input)?;
    let message = trim_ascii(input);
    if message.starts_with(b"Memory cgroup out of memory: Killed process ")
        || contains_field_value(message, b"constraint=CONSTRAINT_MEMCG")
    {
        return Some(fixed_token(b"memcg"));
    }
    if message.starts_with(b"Out of memory: Killed process ")
        || contains_field_value(message, b"constraint=CONSTRAINT_NONE")
    {
        return Some(fixed_token(b"global"));
    }
    None
}

fn native_symbol(remainder: &[u8]) -> Option<&[u8]> {
    if !remainder.starts_with(b"(") {
        return None;
    }
    let build_marker = find_subslice(remainder, b"(BuildId:").unwrap_or(remainder.len());
    let field = trim_ascii(&remainder[..build_marker]);
    if field.len() < 3 || field.first() != Some(&b'(') || field.last() != Some(&b')') {
        return None;
    }
    let mut symbol = trim_ascii(&field[1..field.len() - 1]);
    if symbol.starts_with(b"BuildId:")
        || symbol.starts_with(b"offset ")
        || symbol == b"??"
        || is_hex_address(symbol)
    {
        return None;
    }
    if let Some(plus) = symbol.iter().rposition(|byte| *byte == b'+') {
        let offset = &symbol[plus + 1..];
        if is_decimal(offset)
            || offset
                .strip_prefix(b"0x")
                .is_some_and(is_nonempty_ascii_hex)
        {
            symbol = trim_ascii(&symbol[..plus]);
        }
    }
    (!symbol.is_empty()
        && symbol
            .iter()
            .all(|byte| !byte.is_ascii_control() && *byte != 0))
    .then_some(symbol)
}

fn build_id(remainder: &[u8]) -> Option<&[u8]> {
    let marker = find_subslice(remainder, b"BuildId:")?;
    let after = trim_ascii_start(&remainder[marker + b"BuildId:".len()..]);
    let id = take_while(after, |byte| byte.is_ascii_hexdigit());
    if !is_nonempty_ascii_hex(id)
        || after
            .get(id.len())
            .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b')')
    {
        return None;
    }
    Some(id)
}

fn module_basename(path: &[u8]) -> Option<&[u8]> {
    let basename = path
        .iter()
        .rposition(|byte| *byte == b'/')
        .map_or(path, |separator| &path[separator + 1..]);
    (!basename.is_empty()).then_some(basename)
}

fn is_module_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
}

fn skip_ascii_whitespace(value: &[u8], mut cursor: usize) -> usize {
    while value.get(cursor).is_some_and(u8::is_ascii_whitespace) {
        cursor += 1;
    }
    cursor
}

fn trim_ascii_start(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    value
}

fn trim_leading_zeroes(value: &[u8]) -> &[u8] {
    let trimmed = value
        .iter()
        .position(|byte| *byte != b'0')
        .map_or(&value[value.len().saturating_sub(1)..], |start| {
            &value[start..]
        });
    if trimmed.is_empty() {
        b"0"
    } else {
        trimmed
    }
}

fn is_decimal(value: &[u8]) -> bool {
    !value.is_empty() && value.iter().all(u8::is_ascii_digit)
}

fn is_nonempty_ascii_hex(value: &[u8]) -> bool {
    !value.is_empty() && value.iter().all(u8::is_ascii_hexdigit)
}

fn is_hex_address(value: &[u8]) -> bool {
    value.strip_prefix(b"0x").is_some_and(is_nonempty_ascii_hex)
}

fn validate_input(input: &[u8]) -> Option<()> {
    if input.len() > MAX_NORMALIZATION_INPUT_BYTES || str::from_utf8(input).is_err() {
        return None;
    }
    Some(())
}

fn fixed_token(value: &'static [u8]) -> NormalizedToken {
    NormalizedToken::from_bytes(value).expect("built-in normalization token must fit")
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn find_field(haystack: &[u8], field: &[u8]) -> Option<usize> {
    let mut search_start = 0;
    while let Some(relative) = find_subslice(&haystack[search_start..], field) {
        let position = search_start + relative;
        if position == 0
            || haystack[position - 1].is_ascii_whitespace()
            || matches!(haystack[position - 1], b'{' | b',' | b';' | b':')
        {
            return Some(position);
        }
        search_start = position + 1;
    }
    None
}

fn contains_field_value(haystack: &[u8], field: &[u8]) -> bool {
    find_field(haystack, field).is_some_and(|position| {
        haystack
            .get(position + field.len())
            .is_none_or(|byte| byte.is_ascii_whitespace() || matches!(*byte, b',' | b';' | b')'))
    })
}

fn has_prefix_boundary(value: &[u8], prefix: &[u8]) -> bool {
    value.strip_prefix(prefix).is_some_and(|rest| {
        rest.first()
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
    })
}

fn take_while(value: &[u8], predicate: fn(u8) -> bool) -> &[u8] {
    let end = value
        .iter()
        .position(|byte| !predicate(*byte))
        .unwrap_or(value.len());
    &value[..end]
}

fn is_action_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'$')
}

fn is_component_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'$' | b'/' | b':' | b'-')
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

fn is_java_class(value: &[u8]) -> bool {
    let mut segment_start = true;
    let mut saw_separator = false;
    for byte in value {
        if *byte == b'.' {
            if segment_start {
                return false;
            }
            segment_start = true;
            saw_separator = true;
        } else if segment_start {
            if !byte.is_ascii_alphabetic() && !matches!(*byte, b'_' | b'$') {
                return false;
            }
            segment_start = false;
        } else if !byte.is_ascii_alphanumeric() && !matches!(*byte, b'_' | b'$') {
            return false;
        }
    }
    saw_separator && !segment_start
}

fn is_java_method(value: &[u8]) -> bool {
    if matches!(value, b"<init>" | b"<clinit>") {
        return true;
    }
    let Some(first) = value.first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || matches!(*first, b'_' | b'$'))
        && value[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'$'))
}

struct TokenWriter {
    bytes: [u8; MAX_NORMALIZED_TOKEN_BYTES],
    len: usize,
}

impl TokenWriter {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_NORMALIZED_TOKEN_BYTES],
            len: 0,
        }
    }

    fn push(&mut self, value: &[u8]) -> Option<()> {
        if value.contains(&0) {
            return None;
        }
        let end = self.len.checked_add(value.len())?;
        if end > MAX_NORMALIZED_TOKEN_BYTES {
            return None;
        }
        self.bytes[self.len..end].copy_from_slice(value);
        self.len = end;
        Some(())
    }

    fn push_java_identifier(&mut self, value: &[u8]) -> Option<()> {
        let mut cursor = 0;
        while cursor < value.len() {
            if value[cursor] == b'$' && value.get(cursor + 1).is_some_and(u8::is_ascii_digit) {
                self.push(b"$*")?;
                cursor += 2;
                while value.get(cursor).is_some_and(u8::is_ascii_digit) {
                    cursor += 1;
                }
            } else {
                self.push(&value[cursor..cursor + 1])?;
                cursor += 1;
            }
        }
        Some(())
    }

    fn push_ascii_lowercase(&mut self, value: &[u8]) -> Option<()> {
        for byte in value {
            self.push(&[byte.to_ascii_lowercase()])?;
        }
        Some(())
    }

    fn finish(self) -> Option<NormalizedToken> {
        if self.len == 0 || str::from_utf8(&self.bytes[..self.len]).is_err() {
            return None;
        }
        Some(NormalizedToken(self.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{needs_drop, size_of};

    #[test]
    fn normalized_token_is_stack_owned_and_bounded_to_256_bytes() {
        assert_eq!(size_of::<NormalizedToken>(), 256);
        assert!(!needs_drop::<NormalizedToken>());
        let exact = NormalizedToken::from_bytes(&[b'a'; MAX_NORMALIZED_TOKEN_BYTES]).unwrap();
        assert_eq!(exact.len(), MAX_NORMALIZED_TOKEN_BYTES);
        assert!(NormalizedToken::from_bytes(&[b'a'; MAX_NORMALIZED_TOKEN_BYTES + 1]).is_none());

        let mut oversized_throwable = b"com.example.".to_vec();
        oversized_throwable.extend([b'A'; MAX_NORMALIZED_TOKEN_BYTES]);
        oversized_throwable.extend(b"Exception");
        assert!(normalize_java_throwable(&oversized_throwable).is_none());
    }

    #[test]
    fn java_throwable_keeps_only_a_strict_throwable_class() {
        assert_eq!(
            normalize_java_throwable(b"  Caused by: java.lang.IllegalStateException: item 42  ")
                .unwrap()
                .as_str(),
            "java.lang.IllegalStateException"
        );
        assert_eq!(
            normalize_java_throwable(b"Suppressed: kotlin.KotlinNullPointerException").unwrap(),
            NormalizedToken::from_static("kotlin.KotlinNullPointerException")
        );
        assert!(normalize_java_throwable(b"FATAL EXCEPTION: main").is_none());
        assert!(normalize_java_throwable(b"java.lang.String: ordinary mention").is_none());
    }

    #[test]
    fn java_frame_removes_source_lines_and_synthetic_dynamic_ordinals() {
        let first = normalize_java_frame(b"    at com.example.Worker$1.run(Worker.kt:42)").unwrap();
        let second =
            normalize_java_frame(b"at com.example.Worker$987.run(Worker.kt:9001)").unwrap();
        assert_eq!(first.as_str(), "com.example.Worker$*#run");
        assert_eq!(first, second);
        assert_eq!(
            normalize_java_frame(
                b"at com.example.ScreenKt.access$1200(Screen.kt:12) [object@7ffdead]"
            )
            .unwrap()
            .as_str(),
            "com.example.ScreenKt#access$*"
        );
        assert!(normalize_java_frame(b"com.example.Worker.run(Worker.kt:42)").is_none());
        assert!(normalize_java_frame(b"at broken-frame").is_none());
    }

    #[test]
    fn java_normalizers_reject_invalid_utf8_and_excessive_input_without_panicking() {
        assert!(normalize_java_throwable(b"\xffjava.lang.Error").is_none());
        assert!(normalize_java_frame(b"at bad.\xff.method(File.java:1)").is_none());
        let oversized = vec![b'a'; MAX_NORMALIZATION_INPUT_BYTES + 1];
        assert!(normalize_java_throwable(&oversized).is_none());
        assert!(normalize_java_frame(&oversized).is_none());
    }

    #[test]
    fn anr_input_and_broadcast_reasons_have_separate_category_and_canonical_fields() {
        let first = normalize_anr_reason(
            b"Input dispatching timed out (Window 7f01 has not responded for 5003ms)",
        )
        .unwrap();
        let second = normalize_anr_reason(
            b"Input dispatching timed out (Waiting because the touched window is paused)",
        )
        .unwrap();
        assert_eq!(first.category, AnrReasonCategory::InputDispatchTimeout);
        assert_eq!(first, second);
        assert_eq!(first.canonical.as_str(), "input-dispatch-timeout");

        let broadcast = normalize_anr_reason(
            b"Broadcast of Intent { act=android.intent.action.BOOT_COMPLETED flg=0x10 cmp=x/.R }",
        )
        .unwrap();
        assert_eq!(broadcast.category, AnrReasonCategory::BroadcastTimeout);
        assert_eq!(
            broadcast.canonical.as_str(),
            "broadcast:android.intent.action.BOOT_COMPLETED"
        );
    }

    #[test]
    fn anr_service_categories_keep_bounded_stable_identity_only() {
        let service =
            normalize_anr_reason(b"executing service com.example.app/.SyncService, waited 20000ms")
                .unwrap();
        assert_eq!(service.category, AnrReasonCategory::ServiceTimeout);
        assert_eq!(
            service.canonical.as_str(),
            "service:com.example.app/.SyncService"
        );
        assert_eq!(
            normalize_anr_reason(
                b"Context.startForegroundService() did not then call Service.startForeground()"
            )
            .unwrap()
            .canonical
            .as_str(),
            "foreground-service-start-timeout"
        );
        assert_eq!(
            normalize_anr_reason(b"ContentProvider not responding: com.example.provider")
                .unwrap()
                .category,
            AnrReasonCategory::ContentProviderTimeout
        );
    }

    #[test]
    fn anr_unknown_near_matches_invalid_utf8_and_overlong_input_are_rejected() {
        assert!(normalize_anr_reason(b"InputDispatcher channel is unrecoverably broken").is_none());
        assert!(normalize_anr_reason(b"input dispatching timed out (case changed)").is_none());
        assert!(normalize_anr_reason(b"Input dispatching timed outright").is_none());
        assert!(
            normalize_anr_reason(b"Broadcast of Intent { xact=android.intent.action.BOOT }")
                .is_none()
        );
        assert!(normalize_anr_reason(b"executing service waited-without-component").is_none());
        assert!(normalize_anr_reason(b"OEM watchdog says app is frozen").is_none());
        assert!(normalize_anr_reason(b"Input dispatching timed out (\xff)").is_none());
        let oversized = vec![b'a'; MAX_NORMALIZATION_INPUT_BYTES + 1];
        assert!(normalize_anr_reason(&oversized).is_none());
    }

    #[test]
    fn symbolized_native_frames_keep_only_module_and_symbol() {
        let apex = normalize_native_frame(
            b"#00 pc 000000000004e1f4  /apex/com.android.runtime/lib64/bionic/libc.so (abort+164) (BuildId: AABBCCDD)",
        )
        .unwrap();
        let system = normalize_native_frame(
            b"#07 pc 0000000000099999  /system/lib64/libc.so (abort+4096) (BuildId: 11223344)",
        )
        .unwrap();
        assert_eq!(apex.as_str(), "libc.so#abort");
        assert_eq!(apex, system);
        assert_eq!(
            normalize_native_frame(
                b"#03 pc 0000000000000042 /data/app/libfoo.so (foo::Bar::run()+24)"
            )
            .unwrap()
            .as_str(),
            "libfoo.so#foo::Bar::run()"
        );
    }

    #[test]
    fn unsymbolized_native_frames_require_build_id_module_and_relative_pc() {
        let normalized = normalize_native_frame(
            b"#01 pc 0000000000001A20 /data/app/com.example/lib/arm64/libfoo.so (BuildId: A1B2C3D4)",
        )
        .unwrap();
        assert_eq!(normalized.as_str(), "a1b2c3d4+libfoo.so+1a20");
        assert_eq!(
            normalized,
            normalize_native_frame(
                b"#99 pc 00001a20 /another/location/libfoo.so (BuildId: a1b2c3d4)"
            )
            .unwrap()
        );
        assert_ne!(
            normalized,
            normalize_native_frame(b"#01 pc 0000000000001A21 /data/libfoo.so (BuildId: A1B2C3D4)")
                .unwrap()
        );
        assert!(normalize_native_frame(b"#01 pc 00001a20 /data/libfoo.so").is_none());
        assert!(
            normalize_native_frame(b"#01 pc 00001a20 /data/libfoo.so (BuildId: A1B2nothex)")
                .is_none()
        );
    }

    #[test]
    fn absolute_addresses_registers_invalid_utf8_and_overlong_native_lines_are_rejected() {
        assert!(normalize_native_frame(b"#00 0000007fa12bc000").is_none());
        assert!(normalize_native_frame(b"x0 0000000000000000 x1 0000007fa12bc000").is_none());
        assert!(normalize_native_frame(b"#00 pc 00001a20 /data/lib\xff.so (abort+1)").is_none());
        let oversized = vec![b'a'; MAX_NORMALIZATION_INPUT_BYTES + 1];
        assert!(normalize_native_frame(&oversized).is_none());
    }

    #[test]
    fn lmk_reason_tokens_drop_victim_ids_memory_counters_and_policy_numbers() {
        let first = normalize_lmk_reason(
            b"Killing 'com.example.app' (1234), uid 10123, oom_score_adj 900 to free 54321kB rss; reason: low watermark",
        )
        .unwrap();
        let second = normalize_lmk_reason(
            b"Killing 'another.app' (9999), uid 10555, oom_score_adj 800 to free 1kB rss; reason: low watermark",
        )
        .unwrap();
        assert_eq!(first.as_str(), "lmkd:low-watermark");
        assert_eq!(first, second);
        assert_eq!(
            normalize_lmk_reason(
                b"Killing 'x' (7), uid 8, oom_score_adj 0; reason: process is thrashing 99%"
            )
            .unwrap()
            .as_str(),
            "lmkd:thrashing"
        );
        assert_eq!(
            normalize_lmk_reason(b"lowmemorykiller: Killing 'legacy.app' (321), adj 906")
                .unwrap()
                .as_str(),
            "legacy-lowmemorykiller"
        );
    }

    #[test]
    fn lmk_selection_pressure_and_unknown_oem_text_do_not_become_reason_tokens() {
        assert!(normalize_lmk_reason(b"Selecting process 123 with oom_score_adj 900").is_none());
        assert!(
            normalize_lmk_reason(b"pressure 75, skip killing because swap is available").is_none()
        );
        assert!(normalize_lmk_reason(b"Killing app because OEM magic threshold 42").is_none());
        assert!(normalize_lmk_reason(b"reason: low watermarking").is_none());
        assert!(normalize_lmk_reason(b"reason: low water\xffmark").is_none());
    }

    #[test]
    fn kernel_oom_mechanism_is_only_global_or_memcg_and_ignores_dynamic_constraints() {
        assert_eq!(
            normalize_kernel_oom_mechanism(
                b"oom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null),cpuset=/uid_1000"
            )
            .unwrap()
            .as_str(),
            "memcg"
        );
        assert_eq!(
            normalize_kernel_oom_mechanism(
                b"Memory cgroup out of memory: Killed process 1234 (com.example)"
            )
            .unwrap()
            .as_str(),
            "memcg"
        );
        let global = normalize_kernel_oom_mechanism(
            b"oom-kill:constraint=CONSTRAINT_NONE,nodemask=(null),cpuset=/",
        )
        .unwrap();
        assert_eq!(global.as_str(), "global");
        assert_eq!(
            global,
            normalize_kernel_oom_mechanism(
                b"Out of memory: Killed process 9876 (another.app) total-vm:99999kB"
            )
            .unwrap()
        );
    }

    #[test]
    fn kernel_near_matches_invalid_utf8_and_overlong_inputs_are_rejected() {
        assert!(normalize_kernel_oom_mechanism(b"Killed process 1234 (app)").is_none());
        assert!(normalize_kernel_oom_mechanism(b"Out of memory pressure is rising").is_none());
        assert!(normalize_kernel_oom_mechanism(b"memcg monitor healthy").is_none());
        assert!(normalize_kernel_oom_mechanism(
            b"oom-kill:constraint=CONSTRAINT_MEMCG_FAKE,nodemask=(null)"
        )
        .is_none());
        assert!(normalize_kernel_oom_mechanism(b"Out of memory: Killed process \xff").is_none());
        let oversized = vec![b'a'; MAX_NORMALIZATION_INPUT_BYTES + 1];
        assert!(normalize_lmk_reason(&oversized).is_none());
        assert!(normalize_kernel_oom_mechanism(&oversized).is_none());
    }
}
