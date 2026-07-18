use crate::model::LogEntry;
use crate::parser::ParsedLine;
use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSpec {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

impl SearchSpec {
    pub fn plain(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            regex: false,
            case_sensitive: true,
        }
    }

    pub fn regex(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            regex: true,
            case_sensitive: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Next,
    Previous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSummary {
    pub count: usize,
    pub first: Option<u64>,
}

impl SearchSummary {
    pub fn from_matches(matches: &[u32]) -> Self {
        Self {
            count: matches.len(),
            first: matches.first().map(|idx| u64::from(*idx)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError {
    pub message: String,
}

enum CompiledSearch {
    Empty,
    Plain(String),
    Regex(Regex),
}

impl CompiledSearch {
    fn compile(spec: &SearchSpec) -> Result<Self, SearchError> {
        if spec.query.is_empty() {
            return Ok(Self::Empty);
        }
        if spec.regex {
            return RegexBuilder::new(&spec.query)
                .case_insensitive(!spec.case_sensitive)
                .build()
                .map(Self::Regex)
                .map_err(|err| SearchError {
                    message: err.to_string(),
                });
        }
        if !spec.case_sensitive {
            // 大小写不敏感明文:交给 regex 引擎(转义字面量 + case_insensitive),
            // 兼顾 ASCII 与 Unicode 折叠,避免朴素 O(n·m) 扫描与 lowercase 拷贝。
            return RegexBuilder::new(&regex::escape(&spec.query))
                .case_insensitive(true)
                .build()
                .map(Self::Regex)
                .map_err(|err| SearchError {
                    message: err.to_string(),
                });
        }
        Ok(Self::Plain(spec.query.clone()))
    }

    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Self::Empty => false,
            Self::Plain(needle) => text.contains(needle),
            Self::Regex(re) => re.is_match(text),
        }
    }
}

pub struct SearchMatcher(CompiledSearch);

impl SearchMatcher {
    pub fn new(spec: &SearchSpec) -> Result<Self, SearchError> {
        Ok(Self(CompiledSearch::compile(spec)?))
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text)
    }

    pub fn is_entry_match(&self, entry: &ParsedLine<'_>) -> bool {
        entry_matches(entry, &self.0)
    }
}

pub fn search_entries(entries: &[LogEntry], spec: &SearchSpec) -> Result<Vec<u64>, SearchError> {
    let matcher = SearchMatcher::new(spec)?;
    Ok(entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| matcher.is_entry_match(&entry.as_parsed()).then_some(idx as u64))
        .collect())
}

pub fn search_texts<'a, I>(texts: I, spec: &SearchSpec) -> Result<Vec<u64>, SearchError>
where
    I: IntoIterator<Item = (u64, &'a str)>,
{
    let matcher = SearchMatcher::new(spec)?;
    Ok(texts
        .into_iter()
        .filter_map(|(idx, text)| matcher.is_match(text).then_some(idx))
        .collect())
}

pub fn next_match(matches: &[u32], from: u32, direction: SearchDirection) -> Option<u32> {
    if matches.is_empty() {
        return None;
    }
    match direction {
        SearchDirection::Next => {
            let idx = match matches.binary_search(&from) {
                Ok(i) => i + 1,
                Err(i) => i,
            };
            Some(matches[idx % matches.len()])
        }
        SearchDirection::Previous => {
            let idx = match matches.binary_search(&from) {
                Ok(0) | Err(0) => matches.len() - 1,
                Ok(i) | Err(i) => i - 1,
            };
            Some(matches[idx])
        }
    }
}

fn entry_matches(entry: &ParsedLine<'_>, compiled: &CompiledSearch) -> bool {
    compiled.is_match(entry.date)
        || compiled.is_match(entry.time)
        || compiled.is_match(entry.level)
        || compiled.is_match(entry.pid)
        || compiled.is_match(entry.tid)
        || compiled.is_match(entry.tag)
        || compiled.is_match(entry.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tag: &str, message: &str) -> LogEntry {
        LogEntry {
            tag: tag.to_string(),
            message: message.to_string(),
            ..Default::default()
        }
    }

    fn sample() -> Vec<LogEntry> {
        vec![
            entry("ActivityManager", "Start proc app"),
            entry("Network", "GET /home ok"),
            entry("Network", "slow request retry"),
            entry("Payment", "SocketTimeoutException"),
        ]
    }

    #[test]
    fn substring_search_counts_and_returns_first() {
        let spec = SearchSpec::plain("Network");
        let matches = search_entries(&sample(), &spec).expect("search should compile");
        let matches32: Vec<u32> = matches.iter().map(|&idx| idx as u32).collect();
        let summary = SearchSummary::from_matches(&matches32);
        assert_eq!(summary.count, 2);
        assert_eq!(summary.first, Some(1));
        assert_eq!(matches, vec![1, 2]);
    }

    #[test]
    fn case_sensitive_search_can_be_disabled() {
        let spec = SearchSpec {
            query: "sockettimeout".to_string(),
            regex: false,
            case_sensitive: false,
        };
        let matches = search_entries(&sample(), &spec).expect("search should compile");
        assert_eq!(matches, vec![3]);
    }

    #[test]
    fn regex_search_matches_message() {
        let spec = SearchSpec::regex(r"GET\s+/home|SocketTimeout");
        let matches = search_entries(&sample(), &spec).expect("search should compile");
        assert_eq!(matches, vec![1, 3]);
    }

    #[test]
    fn invalid_regex_returns_error() {
        let spec = SearchSpec::regex("[");
        assert!(search_entries(&sample(), &spec).is_err());
    }

    #[test]
    fn next_and_previous_match_wrap() {
        let matches = vec![1, 3, 8];
        assert_eq!(next_match(&matches, 3, SearchDirection::Next), Some(8));
        assert_eq!(next_match(&matches, 8, SearchDirection::Next), Some(1));
        assert_eq!(next_match(&matches, 3, SearchDirection::Previous), Some(1));
        assert_eq!(next_match(&matches, 1, SearchDirection::Previous), Some(8));
        assert_eq!(next_match(&matches, 4, SearchDirection::Next), Some(8));
        assert_eq!(next_match(&matches, 4, SearchDirection::Previous), Some(3));
    }

    #[test]
    fn ascii_case_insensitive_plain_search_ignores_case() {
        let spec = SearchSpec {
            query: "network".to_string(),
            regex: false,
            case_sensitive: false,
        };
        let matcher = SearchMatcher::new(&spec).expect("search should compile");
        assert!(matcher.is_match("abc NETWORK xyz"));
        assert!(!matcher.is_match("Payment"));

        let spec = SearchSpec {
            query: "sockettimeout".to_string(),
            regex: false,
            case_sensitive: false,
        };
        let matcher = SearchMatcher::new(&spec).expect("search should compile");
        assert!(matcher.is_match("SocketTimeoutException"));
    }

    #[test]
    fn case_insensitive_plain_search_treats_metacharacters_literally() {
        // 明文查询即便走 regex 引擎也必须整体转义,`.` 不能当通配符。
        let spec = SearchSpec {
            query: "a.c".to_string(),
            regex: false,
            case_sensitive: false,
        };
        let matcher = SearchMatcher::new(&spec).expect("search should compile");
        assert!(matcher.is_match("xxA.Cxx"));
        assert!(!matcher.is_match("abc"));
    }

    #[test]
    fn case_insensitive_plain_search_keeps_unicode_behavior() {
        let spec = SearchSpec {
            query: "支付".to_string(),
            regex: false,
            case_sensitive: false,
        };
        let matches = search_entries(&[entry("Payment", "支付失败")], &spec)
            .expect("unicode plain search should compile");

        assert_eq!(matches, vec![0]);
    }
}
