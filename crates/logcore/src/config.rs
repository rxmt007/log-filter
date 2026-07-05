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
pub struct AppConfig {
    pub theme: ThemeMode,
    pub adb_path: Option<PathBuf>,
    pub storage_dir: Option<PathBuf>,
    pub encoding: String,
    pub font_size: u16,
    pub row_height: u16,
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
        }
    }
}

pub fn load_config(path: &Path) -> io::Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid config TOML: {err}"),
        )
    })
}

pub fn save_config(path: &Path, config: &AppConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(config)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
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
