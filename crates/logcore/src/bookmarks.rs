use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkDirection {
    Next,
    Previous,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BookmarkStore {
    lines: BTreeSet<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BookmarkSidecar {
    version: u32,
    source: String,
    lines: Vec<u64>,
}

impl BookmarkStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_lines(lines: impl IntoIterator<Item = u64>) -> Self {
        Self {
            lines: lines.into_iter().filter(|line| *line > 0).collect(),
        }
    }

    pub fn toggle(&mut self, line: u64) -> bool {
        if line == 0 {
            return false;
        }
        if self.lines.contains(&line) {
            self.lines.remove(&line);
            false
        } else {
            self.lines.insert(line);
            true
        }
    }

    pub fn contains(&self, line: u64) -> bool {
        self.lines.contains(&line)
    }

    pub fn list(&self) -> Vec<u64> {
        self.lines.iter().copied().collect()
    }

    pub fn next(&self, from: u64, direction: BookmarkDirection) -> Option<u64> {
        if self.lines.is_empty() {
            return None;
        }
        let lines = self.list();
        match direction {
            BookmarkDirection::Next => {
                let idx = match lines.binary_search(&from) {
                    Ok(i) => i + 1,
                    Err(i) => i,
                };
                Some(lines[idx % lines.len()])
            }
            BookmarkDirection::Previous => {
                let idx = match lines.binary_search(&from) {
                    Ok(0) | Err(0) => lines.len() - 1,
                    Ok(i) | Err(i) => i - 1,
                };
                Some(lines[idx])
            }
        }
    }

    pub fn load_for_source(path: &Path) -> io::Result<Self> {
        let sidecar_path = sidecar_path_for(path);
        if !sidecar_path.exists() {
            return Ok(Self::new());
        }
        let text = fs::read_to_string(sidecar_path)?;
        let sidecar: BookmarkSidecar = toml::from_str(&text).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid bookmark TOML: {err}"),
            )
        })?;
        Ok(Self::from_lines(sidecar.lines))
    }

    pub fn save_for_source(&self, path: &Path) -> io::Result<()> {
        let sidecar = BookmarkSidecar {
            version: 1,
            source: path.to_string_lossy().to_string(),
            lines: self.list(),
        };
        let text = toml::to_string_pretty(&sidecar)
            .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;
        fs::write(sidecar_path_for(path), text)
    }
}

pub fn sidecar_path_for(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(".lfbookmarks.toml");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn toggle_and_list_sorted_bookmarks() {
        let mut store = BookmarkStore::new();
        assert!(store.toggle(8));
        assert!(store.toggle(2));
        assert!(store.toggle(5));
        assert!(!store.toggle(2));
        assert_eq!(store.list(), vec![5, 8]);
        assert!(store.contains(5));
        assert!(!store.contains(2));
    }

    #[test]
    fn next_and_previous_wrap() {
        let mut store = BookmarkStore::new();
        for line in [3, 9, 12] {
            store.toggle(line);
        }
        assert_eq!(store.next(3, BookmarkDirection::Next), Some(9));
        assert_eq!(store.next(12, BookmarkDirection::Next), Some(3));
        assert_eq!(store.next(9, BookmarkDirection::Previous), Some(3));
        assert_eq!(store.next(3, BookmarkDirection::Previous), Some(12));
        assert_eq!(store.next(4, BookmarkDirection::Next), Some(9));
        assert_eq!(store.next(4, BookmarkDirection::Previous), Some(3));
    }

    #[test]
    fn sidecar_path_appends_bookmark_extension() {
        let path = std::path::Path::new("/tmp/device.log");
        assert_eq!(
            sidecar_path_for(path),
            std::path::PathBuf::from("/tmp/device.log.lfbookmarks.toml")
        );
    }

    #[test]
    fn save_and_load_sidecar_round_trip() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "04-20 12:00:00.000  1  1 E Tag: message").unwrap();

        let mut store = BookmarkStore::new();
        store.toggle(2);
        store.toggle(7);
        store.save_for_source(file.path()).unwrap();

        let loaded = BookmarkStore::load_for_source(file.path()).unwrap();
        assert_eq!(loaded.list(), vec![2, 7]);

        let sidecar = sidecar_path_for(file.path());
        assert!(sidecar.exists());
    }
}
