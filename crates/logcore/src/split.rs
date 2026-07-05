use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitMode {
    Bytes(usize),
    Lines(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitSummary {
    pub parts: Vec<PathBuf>,
    pub total_bytes: u64,
}

pub fn split_file(path: &Path, out_dir: &Path, mode: SplitMode) -> io::Result<SplitSummary> {
    match mode {
        SplitMode::Bytes(0) | SplitMode::Lines(0) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "split limit must be greater than zero",
            ));
        }
        _ => {}
    }

    fs::create_dir_all(out_dir)?;
    let total_bytes = fs::metadata(path)?.len();
    let parts = match mode {
        SplitMode::Bytes(limit) => split_by_bytes(path, out_dir, limit)?,
        SplitMode::Lines(limit) => split_by_lines(path, out_dir, limit)?,
    };
    Ok(SplitSummary { parts, total_bytes })
}

fn split_by_bytes(path: &Path, out_dir: &Path, limit: usize) -> io::Result<Vec<PathBuf>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = vec![0; BUFFER_SIZE.min(limit.max(1))];
    let mut parts = Vec::new();
    let mut writer: Option<File> = None;
    let mut current_bytes = 0usize;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let mut offset = 0;
        while offset < read {
            if writer.is_none() || current_bytes >= limit {
                let next_path = part_path(path, out_dir, parts.len() + 1);
                writer = Some(File::create(&next_path)?);
                parts.push(next_path);
                current_bytes = 0;
            }
            let space = limit - current_bytes;
            let take = space.min(read - offset);
            writer
                .as_mut()
                .expect("writer exists after part creation")
                .write_all(&buffer[offset..offset + take])?;
            current_bytes += take;
            offset += take;
        }
    }

    Ok(parts)
}

fn split_by_lines(path: &Path, out_dir: &Path, limit: usize) -> io::Result<Vec<PathBuf>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut parts = Vec::new();
    let mut writer: Option<File> = None;
    let mut current_lines = 0usize;

    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if writer.is_none() || current_lines >= limit {
            let next_path = part_path(path, out_dir, parts.len() + 1);
            writer = Some(File::create(&next_path)?);
            parts.push(next_path);
            current_lines = 0;
        }
        writer
            .as_mut()
            .expect("writer exists after part creation")
            .write_all(&line)?;
        current_lines += 1;
    }

    Ok(parts)
}

fn part_path(path: &Path, out_dir: &Path, part_no: usize) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("log");
    out_dir.join(format!("{name}.part{part_no:03}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn source_file(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file
    }

    fn joined_parts(parts: &[PathBuf]) -> Vec<u8> {
        let mut out = Vec::new();
        for part in parts {
            out.extend(fs::read(part).unwrap());
        }
        out
    }

    #[test]
    fn line_split_preserves_bytes_and_names_parts() {
        let source = source_file(b"a\nbb\nccc\ndddd\n");
        let out_dir = tempfile::tempdir().unwrap();

        let summary = split_file(source.path(), out_dir.path(), SplitMode::Lines(2)).unwrap();

        assert_eq!(summary.parts.len(), 2);
        assert_eq!(joined_parts(&summary.parts), b"a\nbb\nccc\ndddd\n");
        assert!(summary.parts[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".part001"));
        assert!(summary.parts[1]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".part002"));
    }

    #[test]
    fn byte_split_preserves_bytes_and_limits_non_final_parts() {
        let source = source_file(b"abcdefghijkl");
        let out_dir = tempfile::tempdir().unwrap();

        let summary = split_file(source.path(), out_dir.path(), SplitMode::Bytes(5)).unwrap();

        assert_eq!(joined_parts(&summary.parts), b"abcdefghijkl");
        assert_eq!(summary.parts.len(), 3);
        for part in summary.parts.iter().take(2) {
            assert!(fs::metadata(part).unwrap().len() <= 5);
        }
    }

    #[test]
    fn zero_limits_are_rejected() {
        let source = source_file(b"a\n");
        let out_dir = tempfile::tempdir().unwrap();

        assert!(split_file(source.path(), out_dir.path(), SplitMode::Lines(0)).is_err());
        assert!(split_file(source.path(), out_dir.path(), SplitMode::Bytes(0)).is_err());
    }
}
