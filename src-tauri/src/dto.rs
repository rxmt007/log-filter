use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub bookmark_lines: usize,
    pub error_lines: usize,
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
    pub marked_only: bool,
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
            marked_only: value.marked_only,
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MinimapDto {
    pub bookmarks: Vec<usize>,
    pub errors: Vec<usize>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NavigationTargetDto {
    pub line_no: u64,
    pub result_index: usize,
}

impl From<logcore::session::ResultTarget> for NavigationTargetDto {
    fn from(value: logcore::session::ResultTarget) -> Self {
        Self {
            line_no: value.line_no,
            result_index: value.result_index,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub mode: String,
    pub view: Option<String>,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub path: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummaryDto {
    pub written_lines: usize,
    pub written_bytes: u64,
}

impl From<logcore::export::ExportSummary> for ExportSummaryDto {
    fn from(value: logcore::export::ExportSummary) -> Self {
        Self {
            written_lines: value.written_lines,
            written_bytes: value.written_bytes,
        }
    }
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SplitRequest {
    pub path: String,
    pub out_dir: String,
    pub mode: String,
    pub value: usize,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SplitSummaryDto {
    pub parts: Vec<String>,
    pub total_bytes: u64,
}

impl From<logcore::split::SplitSummary> for SplitSummaryDto {
    fn from(value: logcore::split::SplitSummary) -> Self {
        Self {
            parts: value
                .parts
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            total_bytes: value.total_bytes,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigDto {
    pub theme: String,
    pub adb_path: Option<String>,
    pub storage_dir: Option<String>,
    pub encoding: String,
    pub font_size: u16,
    pub row_height: u16,
    pub config_path: String,
}

impl AppConfigDto {
    pub fn from_config(config: logcore::config::AppConfig, config_path: PathBuf) -> Self {
        Self {
            theme: match config.theme {
                logcore::config::ThemeMode::Light => "light".to_string(),
                logcore::config::ThemeMode::Dark => "dark".to_string(),
            },
            adb_path: config
                .adb_path
                .map(|path| path.to_string_lossy().to_string()),
            storage_dir: config
                .storage_dir
                .map(|path| path.to_string_lossy().to_string()),
            encoding: config.encoding,
            font_size: config.font_size,
            row_height: config.row_height,
            config_path: config_path.to_string_lossy().to_string(),
        }
    }
}

impl TryFrom<AppConfigDto> for logcore::config::AppConfig {
    type Error = String;

    fn try_from(value: AppConfigDto) -> Result<Self, Self::Error> {
        let theme = match value.theme.as_str() {
            "dark" => logcore::config::ThemeMode::Dark,
            "light" => logcore::config::ThemeMode::Light,
            other => return Err(format!("unsupported theme: {other}")),
        };
        Ok(Self {
            theme,
            adb_path: value
                .adb_path
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            storage_dir: value
                .storage_dir
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            encoding: value.encoding,
            font_size: value.font_size,
            row_height: value.row_height,
        }
        .normalized())
    }
}
