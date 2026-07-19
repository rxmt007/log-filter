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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AdbDeviceDto {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
    pub product: Option<String>,
    pub online: bool,
}

impl From<logcore::adb::AdbDevice> for AdbDeviceDto {
    fn from(value: logcore::adb::AdbDevice) -> Self {
        let online = value.online();
        Self {
            serial: value.serial,
            state: value.state,
            model: value.model,
            product: value.product,
            online,
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListDto {
    pub adb_path: Option<String>,
    pub devices: Vec<AdbDeviceDto>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StartLogcatRequest {
    pub device_serial: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub buffers: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamAppendDto {
    pub appended_bytes: u64,
    pub status: Status,
    pub device_serial: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StreamControlDto {
    pub status: Status,
    pub running: bool,
    pub paused: bool,
    pub device_serial: Option<String>,
    pub session_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HighlightRuleDto {
    pub enabled: bool,
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub color: String,
}

impl From<HighlightRuleDto> for logcore::filter::HighlightRule {
    fn from(value: HighlightRuleDto) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
            case_sensitive: value.case_sensitive,
            color: value.color,
        }
    }
}

impl From<logcore::filter::HighlightRule> for HighlightRuleDto {
    fn from(value: logcore::filter::HighlightRule) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
            case_sensitive: value.case_sensitive,
            color: value.color,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
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
    #[serde(default)]
    pub highlights: Vec<HighlightRuleDto>,
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
            highlights: value.highlights.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<logcore::filter::FilterSpec> for FilterSpecDto {
    fn from(value: logcore::filter::FilterSpec) -> Self {
        Self {
            levels: value.levels.bits(),
            marked_only: value.marked_only,
            pid: FilterFieldDto::from(value.pid),
            tid: FilterFieldDto::from(value.tid),
            tag_include: FilterFieldDto::from(value.tag_include),
            tag_exclude: FilterFieldDto::from(value.tag_exclude),
            word_include: FilterFieldDto::from(value.word_include),
            word_exclude: FilterFieldDto::from(value.word_exclude),
            highlights: value.highlights.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<logcore::filter::FilterField> for FilterFieldDto {
    fn from(value: logcore::filter::FilterField) -> Self {
        Self {
            enabled: value.enabled,
            pattern: value.pattern,
            regex: value.regex,
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
pub struct FilterDoneDto {
    pub filtered_lines: usize,
    pub generation: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgressDto {
    pub scanned: usize,
    pub matches: usize,
    pub first_line: Option<u64>,
    pub done: bool,
    pub generation: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MinimapBucketDto {
    pub bucket: usize,
    pub count: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MinimapDto {
    pub bookmarks: Vec<usize>,
    pub errors: Vec<MinimapBucketDto>,
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

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummaryDto {
    pub written_lines: usize,
    pub written_bytes: u64,
    pub cancelled: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgressDto {
    pub written_lines: usize,
    pub written_bytes: u64,
    pub done: bool,
    /// 输出文件路径:仅最终成功事件(done=true 且未取消)携带,进度中为 None。
    pub path: Option<String>,
    /// 取消标记:仅最终取消事件携带 true,进度中为 false。
    pub cancelled: bool,
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
pub struct SplitProgressDto {
    pub parts: usize,
    pub bytes_processed: u64,
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
pub struct TableColumnConfigDto {
    pub id: String,
    pub width: u16,
    pub visible: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TableConfigDto {
    pub columns: Vec<TableColumnConfigDto>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WindowConfigDto {
    pub width: u16,
    pub height: u16,
}

impl From<logcore::config::WindowConfig> for WindowConfigDto {
    fn from(value: logcore::config::WindowConfig) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

impl From<WindowConfigDto> for logcore::config::WindowConfig {
    fn from(value: WindowConfigDto) -> Self {
        Self {
            width: value.width,
            height: value.height,
        }
    }
}

impl Default for TableConfigDto {
    fn default() -> Self {
        logcore::config::TableConfig::default().into()
    }
}

impl From<logcore::config::TableConfig> for TableConfigDto {
    fn from(value: logcore::config::TableConfig) -> Self {
        Self {
            columns: value
                .columns
                .into_iter()
                .map(|column| TableColumnConfigDto {
                    id: column.id,
                    width: column.width,
                    visible: column.visible,
                })
                .collect(),
        }
    }
}

impl From<TableConfigDto> for logcore::config::TableConfig {
    fn from(value: TableConfigDto) -> Self {
        Self {
            columns: value
                .columns
                .into_iter()
                .map(|column| logcore::config::TableColumnConfig {
                    id: column.id,
                    width: column.width,
                    visible: column.visible,
                })
                .collect(),
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
    #[serde(default)]
    pub table: TableConfigDto,
    #[serde(default)]
    pub recent_files: Vec<String>,
    #[serde(default)]
    pub last_filter: Option<FilterSpecDto>,
    #[serde(default)]
    pub command_buffers: Vec<String>,
    #[serde(default)]
    pub current_command: String,
    #[serde(default)]
    pub command_presets: Vec<String>,
    #[serde(default = "default_window_config")]
    pub window: WindowConfigDto,
    pub config_path: String,
}

fn default_window_config() -> WindowConfigDto {
    logcore::config::WindowConfig::default().into()
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
            table: config.table.into(),
            recent_files: config
                .recent_files
                .into_iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
            last_filter: Some(config.last_filter.into()),
            command_buffers: config.command_buffers,
            current_command: config.current_command,
            command_presets: config.command_presets,
            window: config.window.into(),
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
            table: value.table.into(),
            recent_files: value
                .recent_files
                .into_iter()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect(),
            last_filter: value.last_filter.map(Into::into).unwrap_or_default(),
            command_buffers: value.command_buffers,
            current_command: value.current_command,
            command_presets: value.command_presets,
            window: value.window.into(),
        }
        .normalized())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filter_done_event_serializes_with_camel_case_fields() {
        let payload = FilterDoneDto {
            filtered_lines: 12,
            generation: 7,
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({"filteredLines": 12, "generation": 7})
        );
    }

    #[test]
    fn search_progress_event_serializes_with_first_line() {
        let payload = SearchProgressDto {
            scanned: 100,
            matches: 3,
            first_line: Some(42),
            done: true,
            generation: 9,
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "scanned": 100,
                "matches": 3,
                "firstLine": 42,
                "done": true,
                "generation": 9
            })
        );
    }

    #[test]
    fn stream_append_event_serializes_nested_status() {
        let payload = StreamAppendDto {
            appended_bytes: 128,
            status: Status {
                total_lines: 2,
                filtered_lines: 2,
                bookmark_lines: 0,
                error_lines: 1,
                indexed_bytes: 128,
                total_bytes: 128,
                indexing: false,
                generation: 4,
            },
            device_serial: "usb".to_string(),
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "appendedBytes": 128,
                "status": {
                    "totalLines": 2,
                    "filteredLines": 2,
                    "bookmarkLines": 0,
                    "errorLines": 1,
                    "indexedBytes": 128,
                    "totalBytes": 128,
                    "indexing": false,
                    "generation": 4
                },
                "deviceSerial": "usb"
            })
        );
    }

    #[test]
    fn app_config_dto_rejects_unknown_theme_and_normalizes_numbers() {
        let config = AppConfigDto {
            theme: "dark".to_string(),
            adb_path: Some(String::new()),
            storage_dir: Some(String::new()),
            encoding: String::new(),
            font_size: 99,
            row_height: 1,
            table: TableConfigDto::default(),
            recent_files: Vec::new(),
            last_filter: None,
            command_buffers: vec!["kernel".to_string(), "events".to_string()],
            current_command: String::new(),
            command_presets: vec![
                "logcat -v threadtime -b radio".to_string(),
                "logcat -v time".to_string(),
            ],
            window: WindowConfigDto {
                width: 1,
                height: 9999,
            },
            config_path: String::new(),
        };

        let converted = logcore::config::AppConfig::try_from(config).unwrap();
        assert_eq!(converted.theme, logcore::config::ThemeMode::Dark);
        assert_eq!(converted.adb_path, None);
        assert_eq!(converted.storage_dir, None);
        assert_eq!(converted.encoding, "UTF-8");
        assert_eq!(converted.font_size, 20);
        assert_eq!(converted.row_height, 16);
        assert_eq!(converted.command_buffers, vec!["events"]);
        assert_eq!(converted.current_command, "logcat -v threadtime -b events");
        assert!(converted
            .command_presets
            .contains(&"logcat -v threadtime -b radio".to_string()));
        assert_eq!(converted.window.width, 960);
        assert_eq!(converted.window.height, 2160);

        let bad = AppConfigDto {
            theme: "system".to_string(),
            ..AppConfigDto::from_config(logcore::config::AppConfig::default(), PathBuf::new())
        };
        assert!(logcore::config::AppConfig::try_from(bad).is_err());
    }

    #[test]
    fn app_config_dto_round_trips_command_presets() {
        let config = logcore::config::AppConfig {
            current_command: "logcat -v threadtime -b radio".to_string(),
            command_presets: vec!["logcat -v threadtime -b radio".to_string()],
            ..Default::default()
        }
        .normalized();
        let dto = AppConfigDto::from_config(config.clone(), PathBuf::new());
        assert_eq!(dto.current_command, "logcat -v threadtime -b radio");
        assert!(dto
            .command_presets
            .contains(&"logcat -v threadtime -b radio".to_string()));

        let converted = logcore::config::AppConfig::try_from(dto).unwrap();
        assert_eq!(converted.current_command, config.current_command);
    }
}
