use encoding_rs::{Encoding, BIG5, EUC_KR, GBK, SHIFT_JIS, UTF_8};
use std::borrow::Cow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Local,
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedTextEncoding {
    label: TextEncoding,
    encoding: &'static Encoding,
}

impl TextEncoding {
    pub fn from_config(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.eq_ignore_ascii_case("local") || trimmed == "本地" {
            TextEncoding::Local
        } else {
            TextEncoding::Utf8
        }
    }

    pub fn config_label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Local => "Local",
        }
    }

    pub fn resolve(self) -> ResolvedTextEncoding {
        let encoding = match self {
            TextEncoding::Utf8 => UTF_8,
            TextEncoding::Local => local_encoding(),
        };
        ResolvedTextEncoding {
            label: self,
            encoding,
        }
    }
}

impl ResolvedTextEncoding {
    pub fn config_label(self) -> &'static str {
        self.label.config_label()
    }

    pub fn decode<'a>(self, bytes: &'a [u8]) -> Cow<'a, str> {
        decode_with_encoding(self.encoding, bytes)
    }
}

fn decode_with_encoding<'a>(encoding: &'static Encoding, bytes: &'a [u8]) -> Cow<'a, str> {
    if encoding == UTF_8 {
        String::from_utf8_lossy(bytes)
    } else {
        let (decoded, _, _) = encoding.decode(bytes);
        decoded
    }
}

#[cfg(windows)]
fn local_encoding() -> &'static Encoding {
    let code_page = unsafe { GetACP() };
    encoding_for_windows_code_page(code_page).unwrap_or(UTF_8)
}

#[cfg(not(windows))]
fn local_encoding() -> &'static Encoding {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find_map(|value| encoding_from_locale_value(&value))
        .unwrap_or(UTF_8)
}

#[cfg(windows)]
unsafe extern "system" {
    fn GetACP() -> u32;
}

#[cfg(windows)]
fn encoding_for_windows_code_page(code_page: u32) -> Option<&'static Encoding> {
    match code_page {
        65001 => Some(UTF_8),
        936 | 54936 => Some(GBK),
        950 => Some(BIG5),
        932 => Some(SHIFT_JIS),
        949 => Some(EUC_KR),
        1250..=1258 => {
            let label = format!("windows-{code_page}");
            Encoding::for_label(label.as_bytes())
        }
        _ => None,
    }
}

#[cfg(not(windows))]
fn encoding_from_locale_value(value: &str) -> Option<&'static Encoding> {
    let label = value
        .rsplit_once('.')
        .map(|(_, encoding)| encoding)
        .unwrap_or(value)
        .split('@')
        .next()
        .unwrap_or(value)
        .trim()
        .replace('_', "-")
        .to_ascii_lowercase();

    if matches!(label.as_str(), "utf-8" | "utf8") {
        return Some(UTF_8);
    }
    match label.as_str() {
        "gbk" | "gb2312" | "gb18030" | "cp936" => Some(GBK),
        "big5" | "cp950" => Some(BIG5),
        "shift-jis" | "shift_jis" | "sjis" | "cp932" => Some(SHIFT_JIS),
        "euc-kr" | "euckr" | "cp949" => Some(EUC_KR),
        _ => Encoding::for_label(label.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_labels_accept_utf8_and_local() {
        assert_eq!(TextEncoding::from_config("UTF-8"), TextEncoding::Utf8);
        assert_eq!(TextEncoding::from_config("local"), TextEncoding::Local);
        assert_eq!(TextEncoding::from_config("本地"), TextEncoding::Local);
        assert_eq!(TextEncoding::from_config(""), TextEncoding::Utf8);
        assert_eq!(TextEncoding::Local.config_label(), "Local");
        assert_eq!(TextEncoding::Utf8.resolve().config_label(), "UTF-8");
    }

    #[cfg(not(windows))]
    #[test]
    fn locale_values_map_common_legacy_encodings() {
        assert_eq!(encoding_from_locale_value("zh_CN.GBK"), Some(GBK));
        assert_eq!(encoding_from_locale_value("ja_JP.SJIS"), Some(SHIFT_JIS));
        assert_eq!(encoding_from_locale_value("en_US.UTF-8"), Some(UTF_8));
    }

    #[test]
    fn decodes_legacy_chinese_bytes_with_selected_encoding() {
        let decoded = decode_with_encoding(GBK, &[0xc4, 0xe3, 0xba, 0xc3]);
        assert_eq!(decoded, "你好");
    }
}
