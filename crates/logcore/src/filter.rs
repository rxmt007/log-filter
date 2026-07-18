use crate::model::LogEntry;
use crate::parser::ParsedLine;
use regex::Regex;
use serde::{Deserialize, Serialize};

const LEVEL_VERBOSE: u8 = 1 << 0;
const LEVEL_DEBUG: u8 = 1 << 1;
const LEVEL_INFO: u8 = 1 << 2;
const LEVEL_WARN: u8 = 1 << 3;
const LEVEL_ERROR: u8 = 1 << 4;
const LEVEL_FATAL: u8 = 1 << 5;
const LEVEL_ALL: u8 =
    LEVEL_VERBOSE | LEVEL_DEBUG | LEVEL_INFO | LEVEL_WARN | LEVEL_ERROR | LEVEL_FATAL;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct LevelMask(u8);

impl LevelMask {
    pub fn all() -> Self {
        Self(LEVEL_ALL)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn from_bits(bits: u8) -> Self {
        Self(bits & LEVEL_ALL)
    }

    pub fn from_levels(levels: &[&str]) -> Self {
        let mut bits = 0;
        for level in levels {
            bits |= match *level {
                "V" | "VERBOSE" => LEVEL_VERBOSE,
                "D" | "DEBUG" => LEVEL_DEBUG,
                "I" | "INFO" => LEVEL_INFO,
                "W" | "WARN" => LEVEL_WARN,
                "E" | "ERROR" => LEVEL_ERROR,
                "F" | "FATAL" => LEVEL_FATAL,
                _ => 0,
            };
        }
        Self::from_bits(bits)
    }

    fn is_all(self) -> bool {
        self.0 == LEVEL_ALL
    }

    fn contains_level(self, level: &str) -> bool {
        let bit = match level {
            "V" => LEVEL_VERBOSE,
            "D" => LEVEL_DEBUG,
            "I" => LEVEL_INFO,
            "W" => LEVEL_WARN,
            "E" => LEVEL_ERROR,
            "F" => LEVEL_FATAL,
            _ => 0,
        };
        bit != 0 && (self.0 & bit) != 0
    }
}

impl Default for LevelMask {
    fn default() -> Self {
        Self::all()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterField {
    pub enabled: bool,
    pub pattern: String,
    pub regex: bool,
}

impl FilterField {
    pub fn plain(enabled: bool, pattern: impl Into<String>) -> Self {
        Self {
            enabled,
            pattern: pattern.into(),
            regex: false,
        }
    }

    pub fn regex(enabled: bool, pattern: impl Into<String>) -> Self {
        Self {
            enabled,
            pattern: pattern.into(),
            regex: true,
        }
    }

    pub fn is_active(&self) -> bool {
        self.enabled && !self.pattern.trim().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HighlightRule {
    pub enabled: bool,
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub color: String,
}

impl Default for HighlightRule {
    fn default() -> Self {
        Self {
            enabled: false,
            pattern: String::new(),
            regex: false,
            case_sensitive: false,
            color: "yellow".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FilterSpec {
    pub levels: LevelMask,
    pub marked_only: bool,
    pub pid: FilterField,
    pub tid: FilterField,
    pub tag_include: FilterField,
    pub tag_exclude: FilterField,
    pub word_include: FilterField,
    pub word_exclude: FilterField,
    #[serde(default)]
    pub highlights: Vec<HighlightRule>,
}

impl Default for FilterSpec {
    fn default() -> Self {
        Self {
            levels: LevelMask::all(),
            marked_only: false,
            pid: FilterField::default(),
            tid: FilterField::default(),
            tag_include: FilterField::default(),
            tag_exclude: FilterField::default(),
            word_include: FilterField::default(),
            word_exclude: FilterField::default(),
            highlights: vec![
                HighlightRule {
                    color: "yellow".to_string(),
                    ..Default::default()
                },
                HighlightRule {
                    color: "green".to_string(),
                    ..Default::default()
                },
                HighlightRule {
                    color: "blue".to_string(),
                    ..Default::default()
                },
            ],
        }
    }
}

impl FilterSpec {
    pub fn is_active(&self) -> bool {
        !self.levels.is_all()
            || self.marked_only
            || self.pid.is_active()
            || self.tid.is_active()
            || self.tag_include.is_active()
            || self.tag_exclude.is_active()
            || self.word_include.is_active()
            || self.word_exclude.is_active()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterError {
    pub message: String,
}

enum Matcher {
    Plain(String),
    Regex(Regex),
}

struct CompiledField {
    enabled: bool,
    matchers: Vec<Matcher>,
}

impl CompiledField {
    fn compile(field: &FilterField) -> Result<Self, FilterError> {
        let mut matchers = Vec::new();
        if field.enabled {
            for part in split_values(&field.pattern) {
                if field.regex {
                    matchers.push(Matcher::Regex(Regex::new(&part).map_err(|err| {
                        FilterError {
                            message: err.to_string(),
                        }
                    })?));
                } else {
                    matchers.push(Matcher::Plain(part));
                }
            }
        }
        Ok(Self {
            enabled: field.enabled,
            matchers,
        })
    }

    fn is_noop(&self) -> bool {
        !self.enabled || self.matchers.is_empty()
    }

    fn contains_any(&self, text: &str) -> bool {
        self.matchers.iter().any(|m| match m {
            Matcher::Plain(part) => text.contains(part),
            Matcher::Regex(re) => re.is_match(text),
        })
    }

    fn equals_any(&self, text: &str) -> bool {
        self.matchers.iter().any(|m| match m {
            Matcher::Plain(part) => text == part,
            Matcher::Regex(re) => re.is_match(text),
        })
    }
}

pub struct FilterMatcher {
    spec: FilterSpec,
    pid: CompiledField,
    tid: CompiledField,
    tag_include: CompiledField,
    tag_exclude: CompiledField,
    word_include: CompiledField,
    word_exclude: CompiledField,
}

impl FilterMatcher {
    pub fn new(spec: &FilterSpec) -> Result<Self, FilterError> {
        Ok(Self {
            spec: spec.clone(),
            pid: CompiledField::compile(&spec.pid)?,
            tid: CompiledField::compile(&spec.tid)?,
            tag_include: CompiledField::compile(&spec.tag_include)?,
            tag_exclude: CompiledField::compile(&spec.tag_exclude)?,
            word_include: CompiledField::compile(&spec.word_include)?,
            word_exclude: CompiledField::compile(&spec.word_exclude)?,
        })
    }

    /// 是否存在 marked-only 过滤:为 false 时 `filter_indexed_range` 可跳过书签查询。
    pub fn requires_mark(&self) -> bool {
        self.spec.marked_only
    }

    pub fn is_match(&self, entry: &ParsedLine<'_>) -> bool {
        self.is_match_with_mark(entry, false)
    }

    pub fn is_match_with_mark(&self, entry: &ParsedLine<'_>, marked: bool) -> bool {
        if self.spec.marked_only && !marked {
            return false;
        }
        if !self.spec.levels.is_all() && !self.spec.levels.contains_level(entry.level) {
            return false;
        }
        if !include_exact(&self.pid, entry.pid) || !include_exact(&self.tid, entry.tid) {
            return false;
        }
        if !include_contains(&self.tag_include, entry.tag)
            || exclude_contains(&self.tag_exclude, entry.tag)
        {
            return false;
        }
        if !include_contains(&self.word_include, entry.message)
            || exclude_contains(&self.word_exclude, entry.message)
        {
            return false;
        }
        true
    }
}

pub fn filter_entries(entries: &[LogEntry], spec: &FilterSpec) -> Result<Vec<u64>, FilterError> {
    let matcher = FilterMatcher::new(spec)?;
    Ok(entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| matcher.is_match(&entry.as_parsed()).then_some(idx as u64))
        .collect())
}

fn include_contains(field: &CompiledField, text: &str) -> bool {
    field.is_noop() || field.contains_any(text)
}

fn include_exact(field: &CompiledField, text: &str) -> bool {
    field.is_noop() || field.equals_any(text)
}

fn exclude_contains(field: &CompiledField, text: &str) -> bool {
    !field.is_noop() && field.contains_any(text)
}

fn split_values(pattern: &str) -> Vec<String> {
    pattern
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: &str, pid: &str, tid: &str, tag: &str, message: &str) -> LogEntry {
        LogEntry {
            level: level.to_string(),
            pid: pid.to_string(),
            tid: tid.to_string(),
            tag: tag.to_string(),
            message: message.to_string(),
            ..Default::default()
        }
    }

    fn sample() -> Vec<LogEntry> {
        vec![
            entry("D", "100", "10", "ActivityManager", "Start proc app"),
            entry("I", "200", "20", "Network", "GET /home ok"),
            entry("W", "200", "21", "Network", "slow request retry"),
            entry("E", "300", "30", "Payment", "SocketTimeoutException"),
            entry("", "", "", "", "--------- beginning of main"),
        ]
    }

    #[test]
    fn all_level_mask_preserves_raw_lines_without_levels() {
        let spec = FilterSpec::default();
        let matches = filter_entries(&sample(), &spec).expect("filter should compile");
        assert_eq!(matches, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn level_mask_filters_known_levels_when_not_all_selected() {
        let spec = FilterSpec {
            levels: LevelMask::from_levels(&["W", "E"]),
            ..Default::default()
        };
        let matches = filter_entries(&sample(), &spec).expect("filter should compile");
        assert_eq!(matches, vec![2, 3]);
    }

    #[test]
    fn empty_level_mask_matches_no_known_levels() {
        let entries = vec![
            entry("D", "100", "101", "Net", "debug"),
            entry("E", "100", "101", "Net", "error"),
        ];
        let spec = FilterSpec {
            levels: LevelMask::from_bits(0),
            ..Default::default()
        };

        assert_eq!(filter_entries(&entries, &spec).unwrap(), Vec::<u64>::new());
    }

    #[test]
    fn marked_only_requires_marked_row() {
        let matcher = FilterMatcher::new(&FilterSpec {
            marked_only: true,
            ..Default::default()
        })
        .unwrap();
        let entry = entry("I", "100", "101", "Tag", "message");

        assert!(!matcher.is_match_with_mark(&entry.as_parsed(), false));
        assert!(matcher.is_match_with_mark(&entry.as_parsed(), true));
    }

    #[test]
    fn pid_tid_and_tag_filters_are_conjunctive() {
        let spec = FilterSpec {
            pid: FilterField::plain(true, "200|300"),
            tid: FilterField::plain(true, "21"),
            tag_include: FilterField::plain(true, "Network|Payment"),
            ..Default::default()
        };
        let matches = filter_entries(&sample(), &spec).expect("filter should compile");
        assert_eq!(matches, vec![2]);
    }

    #[test]
    fn include_and_exclude_keyword_filters_work_together() {
        let spec = FilterSpec {
            word_include: FilterField::plain(true, "request|Timeout"),
            word_exclude: FilterField::plain(true, "slow"),
            ..Default::default()
        };
        let matches = filter_entries(&sample(), &spec).expect("filter should compile");
        assert_eq!(matches, vec![3]);
    }

    #[test]
    fn regex_filters_match_each_pipe_value() {
        let spec = FilterSpec {
            tag_include: FilterField::regex(true, r"Activity.*|Pay(ment)?"),
            word_include: FilterField::regex(true, r"Start\s+proc|SocketTimeout"),
            ..Default::default()
        };
        let matches = filter_entries(&sample(), &spec).expect("filter should compile");
        assert_eq!(matches, vec![0, 3]);
    }

    #[test]
    fn invalid_regex_returns_error() {
        let spec = FilterSpec {
            tag_include: FilterField::regex(true, "["),
            ..Default::default()
        };
        assert!(filter_entries(&sample(), &spec).is_err());
    }
}
