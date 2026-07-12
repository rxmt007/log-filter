use crate::encoding::TextEncoding;
use crate::filter::FilterSpec;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableColumnConfig {
    pub id: String,
    pub width: u16,
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableConfig {
    pub columns: Vec<TableColumnConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowConfig {
    pub width: u16,
    pub height: u16,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 1180,
            height: 720,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TableColumnSpec {
    id: &'static str,
    width: u16,
    min: u16,
    max: u16,
}

const TABLE_COLUMN_SPECS: [TableColumnSpec; 9] = [
    TableColumnSpec {
        id: "bookmark",
        width: 24,
        min: 22,
        max: 36,
    },
    TableColumnSpec {
        id: "lineNo",
        width: 58,
        min: 52,
        max: 120,
    },
    TableColumnSpec {
        id: "date",
        width: 50,
        min: 48,
        max: 90,
    },
    TableColumnSpec {
        id: "time",
        width: 98,
        min: 82,
        max: 160,
    },
    TableColumnSpec {
        id: "level",
        width: 40,
        min: 36,
        max: 60,
    },
    TableColumnSpec {
        id: "pid",
        width: 54,
        min: 48,
        max: 100,
    },
    TableColumnSpec {
        id: "tid",
        width: 54,
        min: 48,
        max: 100,
    },
    TableColumnSpec {
        id: "tag",
        width: 154,
        min: 110,
        max: 260,
    },
    TableColumnSpec {
        id: "message",
        width: 360,
        min: 220,
        max: 1200,
    },
];

impl Default for TableConfig {
    fn default() -> Self {
        Self {
            columns: TABLE_COLUMN_SPECS
                .iter()
                .map(|spec| TableColumnConfig {
                    id: spec.id.to_string(),
                    width: spec.width,
                    visible: true,
                })
                .collect(),
        }
    }
}

impl TableConfig {
    pub fn normalized(self) -> Self {
        let all_configured_columns_hidden = TABLE_COLUMN_SPECS.iter().all(|spec| {
            self.columns
                .iter()
                .find(|column| column.id == spec.id)
                .is_some_and(|column| !column.visible)
        });
        if all_configured_columns_hidden {
            return Self::default();
        }

        let columns = TABLE_COLUMN_SPECS
            .iter()
            .map(|spec| {
                let configured = self.columns.iter().find(|column| column.id == spec.id);
                let width = configured
                    .map(|column| column.width)
                    .unwrap_or(spec.width)
                    .clamp(spec.min, spec.max);
                let visible = if spec.id == "message" {
                    true
                } else {
                    configured.map(|column| column.visible).unwrap_or(true)
                };
                TableColumnConfig {
                    id: spec.id.to_string(),
                    width,
                    visible,
                }
            })
            .collect();
        Self { columns }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub theme: ThemeMode,
    pub adb_path: Option<PathBuf>,
    pub storage_dir: Option<PathBuf>,
    pub encoding: String,
    pub font_size: u16,
    pub row_height: u16,
    #[serde(default)]
    pub table: TableConfig,
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    #[serde(default)]
    pub last_filter: FilterSpec,
    #[serde(default)]
    pub command_buffers: Vec<String>,
    #[serde(default)]
    pub window: WindowConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Light,
            adb_path: None,
            storage_dir: None,
            encoding: "UTF-8".to_string(),
            font_size: 13,
            row_height: 20,
            table: TableConfig::default(),
            recent_files: Vec::new(),
            last_filter: FilterSpec::default(),
            command_buffers: vec!["main".to_string()],
            window: WindowConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn normalized(mut self) -> Self {
        self.encoding = TextEncoding::from_config(&self.encoding)
            .config_label()
            .to_string();
        self.font_size = self.font_size.clamp(10, 20);
        self.row_height = self.row_height.clamp(16, 32);
        self.table = self.table.normalized();
        self.recent_files = normalize_recent_files(self.recent_files);
        if self.last_filter.highlights.is_empty() {
            self.last_filter.highlights = FilterSpec::default().highlights;
        }
        if self.command_buffers.is_empty() {
            self.command_buffers = vec!["main".to_string()];
        }
        self.command_buffers.retain(|buffer| {
            matches!(
                buffer.as_str(),
                "main" | "system" | "radio" | "events" | "crash"
            )
        });
        if self.command_buffers.is_empty() {
            self.command_buffers = vec!["main".to_string()];
        }
        self.window.width = self.window.width.clamp(960, 3840);
        self.window.height = self.window.height.clamp(560, 2160);
        self
    }
}

fn normalize_recent_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for file in files {
        if file.as_os_str().is_empty() || out.contains(&file) {
            continue;
        }
        out.push(file);
        if out.len() == 10 {
            break;
        }
    }
    out
}

pub fn load_config(path: &Path) -> io::Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let text = fs::read_to_string(path)?;
    let config: AppConfig = toml::from_str(&text).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid config TOML: {err}"),
        )
    })?;
    Ok(config.normalized())
}

pub fn save_config(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(&config.clone().normalized())
        .map_err(|err| io::Error::other(err.to_string()))?;
    fs::write(path, text)
}

pub fn default_config_path() -> PathBuf {
    default_config_dir().join("config.toml")
}

pub fn default_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("LogFilter");
        }
        return home_dir().join("AppData").join("Roaming").join("LogFilter");
    }

    #[cfg(target_os = "macos")]
    {
        return home_dir()
            .join("Library")
            .join("Application Support")
            .join("LogFilter");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("logfilter");
        }
        return home_dir().join(".config").join("logfilter");
    }

    #[allow(unreachable_code)]
    home_dir().join(".config").join("logfilter")
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_file_friendly_and_light_theme() {
        let config = AppConfig::default();
        assert_eq!(config.theme, ThemeMode::Light);
        assert_eq!(config.encoding, "UTF-8");
        assert_eq!(config.font_size, 13);
        assert_eq!(config.row_height, 20);
        assert_eq!(config.adb_path, None);
        assert_eq!(config.storage_dir, None);
        assert_eq!(config.command_buffers, vec!["main"]);
        assert_eq!(config.window, WindowConfig::default());
    }

    #[test]
    fn default_config_includes_complete_table_columns() {
        let config = AppConfig::default();
        let ids: Vec<&str> = config
            .table
            .columns
            .iter()
            .map(|column| column.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["bookmark", "lineNo", "date", "time", "level", "pid", "tid", "tag", "message",]
        );
        assert!(config.table.columns.iter().all(|column| column.visible));
        assert!(config.table.columns.iter().all(|column| column.width > 0));
    }

    #[test]
    fn toml_round_trip_preserves_table_columns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = AppConfig::default();
        config.table.columns.iter_mut().for_each(|column| {
            if column.id == "tag" {
                column.width = 210;
            }
            if column.id == "pid" {
                column.visible = false;
            }
        });

        save_config(&path, &config).unwrap();
        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.table, config.table);
    }

    #[test]
    fn normalizes_recent_files_command_buffers_and_window_size() {
        let mut files = Vec::new();
        for i in 0..12 {
            files.push(PathBuf::from(format!("/tmp/{i}.log")));
        }
        files.push(PathBuf::from("/tmp/1.log"));

        let config = AppConfig {
            recent_files: files,
            command_buffers: vec!["kernel".to_string(), "crash".to_string()],
            window: WindowConfig {
                width: 10,
                height: 9999,
            },
            ..Default::default()
        }
        .normalized();

        assert_eq!(config.recent_files.len(), 10);
        assert_eq!(config.recent_files[1], PathBuf::from("/tmp/1.log"));
        assert_eq!(config.command_buffers, vec!["crash"]);
        assert_eq!(config.window.width, 960);
        assert_eq!(config.window.height, 2160);
    }

    #[test]
    fn normalized_config_restores_default_highlight_rules() {
        let config = AppConfig {
            last_filter: FilterSpec {
                highlights: Vec::new(),
                ..Default::default()
            },
            ..Default::default()
        }
        .normalized();

        assert_eq!(config.last_filter.highlights.len(), 3);
    }

    #[test]
    fn table_columns_are_normalized_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
theme = "light"
encoding = "UTF-8"
font_size = 13
row_height = 20

[[table.columns]]
id = "tag"
width = 999
visible = false

[[table.columns]]
id = "unknown"
width = 77
visible = true

[[table.columns]]
id = "message"
width = 1
visible = false
"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        let tag = config
            .table
            .columns
            .iter()
            .find(|column| column.id == "tag")
            .unwrap();
        let message = config
            .table
            .columns
            .iter()
            .find(|column| column.id == "message")
            .unwrap();
        assert_eq!(tag.width, 260);
        assert!(!tag.visible);
        assert_eq!(message.width, 220);
        assert!(message.visible);
        assert!(!config
            .table
            .columns
            .iter()
            .any(|column| column.id == "unknown"));
        assert_eq!(config.table.columns.len(), 9);
    }

    #[test]
    fn hiding_every_table_column_restores_default_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
theme = "light"
encoding = "UTF-8"
font_size = 13
row_height = 20

[[table.columns]]
id = "bookmark"
width = 24
visible = false

[[table.columns]]
id = "lineNo"
width = 58
visible = false

[[table.columns]]
id = "date"
width = 50
visible = false

[[table.columns]]
id = "time"
width = 98
visible = false

[[table.columns]]
id = "level"
width = 40
visible = false

[[table.columns]]
id = "pid"
width = 54
visible = false

[[table.columns]]
id = "tid"
width = 54
visible = false

[[table.columns]]
id = "tag"
width = 154
visible = false

[[table.columns]]
id = "message"
width = 360
visible = false
"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert!(config.table.columns.iter().all(|column| column.visible));
    }

    #[test]
    fn toml_round_trip_preserves_editable_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = AppConfig {
            theme: ThemeMode::Dark,
            adb_path: Some(PathBuf::from("/opt/android/adb")),
            storage_dir: Some(PathBuf::from("/tmp/logfilter")),
            encoding: "UTF-8".to_string(),
            font_size: 14,
            row_height: 22,
            table: TableConfig::default(),
            recent_files: Vec::new(),
            last_filter: FilterSpec::default(),
            command_buffers: vec!["main".to_string()],
            window: WindowConfig::default(),
        };

        save_config(&path, &config).unwrap();
        assert_eq!(load_config(&path).unwrap(), config);
    }

    #[test]
    fn missing_config_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.toml");
        assert_eq!(load_config(&path).unwrap(), AppConfig::default());
    }

    #[test]
    fn invalid_numeric_settings_are_normalized_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
theme = "dark"
encoding = ""
font_size = 0
row_height = 200
"#,
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.theme, ThemeMode::Dark);
        assert_eq!(config.encoding, "UTF-8");
        assert_eq!(config.font_size, 10);
        assert_eq!(config.row_height, 32);
    }

    #[test]
    fn default_path_points_at_logfilter_config_toml() {
        let path = default_config_path();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("config.toml")
        );
        assert!(path
            .components()
            .any(|component| component.as_os_str() == "LogFilter"
                || component.as_os_str() == "logfilter"));
    }
}
