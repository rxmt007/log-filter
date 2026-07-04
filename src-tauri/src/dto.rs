use serde::{Deserialize, Serialize};

/// 传给前端的一行(camelCase 对齐 TS 类型)。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub line_no: u64,
    pub date: String,
    pub time: String,
    pub level: String,
    pub pid: String,
    pub tid: String,
    pub tag: String,
    pub message: String,
    pub marked: bool,
}

/// 会话状态快照。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub total_lines: usize,
    pub filtered_lines: usize,
    pub indexed_bytes: u64,
    pub total_bytes: u64,
    pub indexing: bool,
    pub generation: u64,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FilterFieldDto {
    pub enabled: bool,
    pub pattern: String,
    pub regex: bool,
}

impl From<FilterFieldDto> for logcore::filter::FilterField {
    fn from(value: FilterFieldDto) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FilterSpecDto {
    pub levels: u8,
    pub pid: FilterFieldDto,
    pub tid: FilterFieldDto,
    pub tag_include: FilterFieldDto,
    pub tag_exclude: FilterFieldDto,
    pub word_include: FilterFieldDto,
    pub word_exclude: FilterFieldDto,
}

impl From<FilterSpecDto> for logcore::filter::FilterSpec {
    fn from(value: FilterSpecDto) -> Self {
        Self {
            levels: logcore::filter::LevelMask::from_bits(value.levels),
            pid: value.pid.into(),
            tid: value.tid.into(),
            tag_include: value.tag_include.into(),
            tag_exclude: value.tag_exclude.into(),
            word_include: value.word_include.into(),
            word_exclude: value.word_exclude.into(),
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchSpecDto {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
}

impl From<SearchSpecDto> for logcore::search::SearchSpec {
    fn from(value: SearchSpecDto) -> Self {
        Self {
            query: value.query,
            regex: value.regex,
            case_sensitive: value.case_sensitive,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub count: usize,
    pub first_line: Option<u64>,
}
