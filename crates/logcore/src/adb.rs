use std::env;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
    pub product: Option<String>,
    pub transport_id: Option<String>,
}

impl AdbDevice {
    pub fn online(&self) -> bool {
        self.state == "device"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogcatBuffer {
    Main,
    System,
    Radio,
    Events,
    Crash,
}

impl LogcatBuffer {
    pub fn as_arg(self) -> &'static str {
        match self {
            LogcatBuffer::Main => "main",
            LogcatBuffer::System => "system",
            LogcatBuffer::Radio => "radio",
            LogcatBuffer::Events => "events",
            LogcatBuffer::Crash => "crash",
        }
    }
}

impl TryFrom<&str> for LogcatBuffer {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "main" => Ok(Self::Main),
            "system" => Ok(Self::System),
            "radio" => Ok(Self::Radio),
            "events" => Ok(Self::Events),
            "crash" => Ok(Self::Crash),
            other => Err(format!("unsupported logcat buffer: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogcatCommand {
    pub adb_path: PathBuf,
    pub args: Vec<String>,
}

pub fn parse_adb_devices(output: &str) -> Vec<AdbDevice> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("List of devices attached") {
                return None;
            }
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next().unwrap_or("unknown").to_string();
            let mut model = None;
            let mut product = None;
            let mut transport_id = None;
            for part in parts {
                if let Some(value) = part.strip_prefix("model:") {
                    model = Some(value.to_string());
                } else if let Some(value) = part.strip_prefix("product:") {
                    product = Some(value.to_string());
                } else if let Some(value) = part.strip_prefix("transport_id:") {
                    transport_id = Some(value.to_string());
                }
            }
            Some(AdbDevice {
                serial,
                state,
                model,
                product,
                transport_id,
            })
        })
        .collect()
}

pub fn select_online_device(
    devices: &[AdbDevice],
    requested_serial: Option<&str>,
) -> Result<AdbDevice, String> {
    if let Some(serial) = requested_serial.filter(|serial| !serial.trim().is_empty()) {
        return devices
            .iter()
            .find(|device| device.serial == serial && device.online())
            .cloned()
            .ok_or_else(|| format!("adb device is not online: {serial}"));
    }

    devices
        .iter()
        .find(|device| device.online())
        .cloned()
        .ok_or_else(|| "no online adb device found".to_string())
}

pub fn build_logcat_command(
    adb_path: PathBuf,
    serial: &str,
    buffers: &[LogcatBuffer],
) -> LogcatCommand {
    let mut args = vec![
        "-s".to_string(),
        serial.to_string(),
        "logcat".to_string(),
        "-v".to_string(),
        "threadtime".to_string(),
    ];
    for buffer in normalized_buffers(buffers) {
        args.push("-b".to_string());
        args.push(buffer.as_arg().to_string());
    }
    LogcatCommand { adb_path, args }
}

pub fn list_devices(adb_path: &Path) -> io::Result<Vec<AdbDevice>> {
    let output = Command::new(adb_path).arg("devices").arg("-l").output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(parse_adb_devices(&String::from_utf8_lossy(&output.stdout)))
}

pub fn resolve_adb_path(configured: Option<&Path>) -> Option<PathBuf> {
    configured
        .filter(|path| executable_candidate(path))
        .map(Path::to_path_buf)
        .or_else(|| find_adb_in_env_path(env::var_os("PATH")))
        .or_else(find_adb_in_common_locations)
}

fn normalized_buffers(buffers: &[LogcatBuffer]) -> Vec<LogcatBuffer> {
    if buffers.is_empty() {
        return vec![LogcatBuffer::Main];
    }
    let mut out = Vec::new();
    for buffer in buffers {
        if !out.contains(buffer) {
            out.push(*buffer);
        }
    }
    out
}

fn find_adb_in_env_path(path: Option<OsString>) -> Option<PathBuf> {
    let path = path?;
    env::split_paths(&path)
        .flat_map(|dir| {
            adb_binary_names()
                .into_iter()
                .map(move |name| dir.join(name))
        })
        .find(|candidate| executable_candidate(candidate))
}

fn find_adb_in_common_locations() -> Option<PathBuf> {
    common_adb_dirs()
        .into_iter()
        .flat_map(|dir| {
            adb_binary_names()
                .into_iter()
                .map(move |name| dir.join(name))
        })
        .find(|candidate| executable_candidate(candidate))
}

fn common_adb_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for key in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(root) = env::var_os(key) {
            dirs.push(PathBuf::from(root).join("platform-tools"));
        }
    }

    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(
            home.join("Library")
                .join("Android")
                .join("sdk")
                .join("platform-tools"),
        );
        dirs.push(home.join("Android").join("Sdk").join("platform-tools"));
    }
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        dirs.push(
            PathBuf::from(local_app_data)
                .join("Android")
                .join("Sdk")
                .join("platform-tools"),
        );
    }
    dirs
}

fn adb_binary_names() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["adb.exe", "adb"]
    } else {
        vec!["adb"]
    }
}

fn executable_candidate(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_adb_devices_with_models_and_offline_states() {
        let devices = parse_adb_devices(
            r#"
List of devices attached
192.0.2.12:5555 device product:oriole model:Pixel_6 device:oriole transport_id:3
USB123 offline transport_id:4
emulator-5554 unauthorized
"#,
        );

        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].serial, "192.0.2.12:5555");
        assert_eq!(devices[0].state, "device");
        assert_eq!(devices[0].model.as_deref(), Some("Pixel_6"));
        assert!(devices[0].online());
        assert_eq!(devices[1].state, "offline");
        assert_eq!(devices[2].state, "unauthorized");
    }

    #[test]
    fn selects_requested_online_device_or_first_online_device() {
        let devices = vec![
            AdbDevice {
                serial: "offline".to_string(),
                state: "offline".to_string(),
                model: None,
                product: None,
                transport_id: None,
            },
            AdbDevice {
                serial: "usb".to_string(),
                state: "device".to_string(),
                model: None,
                product: None,
                transport_id: None,
            },
        ];

        assert_eq!(select_online_device(&devices, None).unwrap().serial, "usb");
        assert_eq!(
            select_online_device(&devices, Some("usb")).unwrap().serial,
            "usb"
        );
        assert!(select_online_device(&devices, Some("offline")).is_err());
    }

    #[test]
    fn builds_threadtime_logcat_command_with_unique_buffers() {
        let command = build_logcat_command(
            PathBuf::from("adb"),
            "usb",
            &[LogcatBuffer::Main, LogcatBuffer::System, LogcatBuffer::Main],
        );

        assert_eq!(
            command.args,
            vec![
                "-s",
                "usb",
                "logcat",
                "-v",
                "threadtime",
                "-b",
                "main",
                "-b",
                "system"
            ]
        );
    }

    #[test]
    fn resolves_configured_adb_path_before_path_scan() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "fake adb").unwrap();

        assert_eq!(
            resolve_adb_path(Some(file.path())).unwrap(),
            file.path().to_path_buf()
        );
    }
}
