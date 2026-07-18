use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
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
    split_file_with_progress(path, out_dir, mode, &mut |_, _| {})
}

/// 切分文件;每关闭一个分片(轮换或收尾,writer 已 flush)回调一次
/// `on_part(已完成分片数, 已处理字节)`。切分永远按整行对齐,分片不会截断行。
pub fn split_file_with_progress(
    path: &Path,
    out_dir: &Path,
    mode: SplitMode,
    on_part: &mut dyn FnMut(usize, u64),
) -> io::Result<SplitSummary> {
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
        SplitMode::Bytes(limit) => split_by_bytes(path, out_dir, limit, on_part)?,
        SplitMode::Lines(limit) => split_by_lines(path, out_dir, limit, on_part)?,
    };
    Ok(SplitSummary { parts, total_bytes })
}

fn split_by_bytes(
    path: &Path,
    out_dir: &Path,
    limit: usize,
    on_part: &mut dyn FnMut(usize, u64),
) -> io::Result<Vec<PathBuf>> {
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, File::open(path)?);
    let mut line = Vec::new();
    let mut parts = Vec::new();
    let mut writer: Option<BufWriter<File>> = None;
    let mut current_bytes = 0usize;
    let mut bytes_processed = 0u64;

    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        // 轮换条件:已有非空分片且再写入本行会超过 limit。单行本身超过 limit
        // 时(current_bytes == 0)不轮换,让该行独占一个分片(可能超过 limit)。
        if writer.is_some() && current_bytes > 0 && current_bytes + read > limit {
            finish_part(writer.take(), parts.len(), bytes_processed, on_part)?;
            current_bytes = 0;
        }
        if writer.is_none() {
            let next_path = part_path(path, out_dir, parts.len() + 1);
            writer = Some(BufWriter::new(File::create(&next_path)?));
            parts.push(next_path);
        }
        writer
            .as_mut()
            .expect("writer exists after part creation")
            .write_all(&line)?;
        current_bytes += read;
        bytes_processed += read as u64;
    }

    finish_part(writer.take(), parts.len(), bytes_processed, on_part)?;
    Ok(parts)
}

fn split_by_lines(
    path: &Path,
    out_dir: &Path,
    limit: usize,
    on_part: &mut dyn FnMut(usize, u64),
) -> io::Result<Vec<PathBuf>> {
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, File::open(path)?);
    let mut line = Vec::new();
    let mut parts = Vec::new();
    let mut writer: Option<BufWriter<File>> = None;
    let mut current_lines = 0usize;
    let mut bytes_processed = 0u64;

    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if writer.is_some() && current_lines >= limit {
            finish_part(writer.take(), parts.len(), bytes_processed, on_part)?;
            current_lines = 0;
        }
        if writer.is_none() {
            let next_path = part_path(path, out_dir, parts.len() + 1);
            writer = Some(BufWriter::new(File::create(&next_path)?));
            parts.push(next_path);
        }
        writer
            .as_mut()
            .expect("writer exists after part creation")
            .write_all(&line)?;
        current_lines += 1;
        bytes_processed += read as u64;
    }

    finish_part(writer.take(), parts.len(), bytes_processed, on_part)?;
    Ok(parts)
}

/// 关闭一个分片:先 flush 让字节真正落盘,再回调进度(保证回调时字节数已在磁盘上)。
fn finish_part(
    writer: Option<BufWriter<File>>,
    parts: usize,
    bytes_processed: u64,
    on_part: &mut dyn FnMut(usize, u64),
) -> io::Result<()> {
    let Some(mut writer) = writer else {
        return Ok(());
    };
    writer.flush()?;
    on_part(parts, bytes_processed);
    Ok(())
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
    fn byte_split_aligns_to_line_boundaries() {
        let source = source_file(b"aaaa\nbb\ncccccc\ndd\n"); // 行字节数: 5,3,7,3
        let out_dir = tempfile::tempdir().unwrap();

        let summary = split_file(source.path(), out_dir.path(), SplitMode::Bytes(8)).unwrap();

        assert_eq!(joined_parts(&summary.parts), b"aaaa\nbb\ncccccc\ndd\n");
        assert_eq!(summary.parts.len(), 3); // "aaaa\nbb\n" / "cccccc\n" / "dd\n"
        for part in &summary.parts {
            let bytes = fs::read(part).unwrap();
            assert!(bytes.ends_with(b"\n"), "part must end at line boundary");
            assert!(bytes.len() <= 8);
        }
    }

    #[test]
    fn oversized_single_line_gets_its_own_part() {
        let source = source_file(b"abcdefghij\nx\n"); // 首行 11 字节 > limit 4
        let out_dir = tempfile::tempdir().unwrap();

        let summary = split_file(source.path(), out_dir.path(), SplitMode::Bytes(4)).unwrap();

        assert_eq!(summary.parts.len(), 2);
        assert_eq!(fs::read(&summary.parts[0]).unwrap(), b"abcdefghij\n");
        assert_eq!(fs::read(&summary.parts[1]).unwrap(), b"x\n");
    }

    #[test]
    fn split_reports_progress_per_part() {
        let source = source_file(b"a\nb\nc\nd\n");
        let out_dir = tempfile::tempdir().unwrap();
        let mut calls = Vec::new();

        split_file_with_progress(
            source.path(),
            out_dir.path(),
            SplitMode::Lines(2),
            &mut |parts, bytes| {
                calls.push((parts, bytes));
            },
        )
        .unwrap();

        assert_eq!(calls, vec![(1, 4), (2, 8)]);
    }

    #[test]
    fn zero_limits_are_rejected() {
        let source = source_file(b"a\n");
        let out_dir = tempfile::tempdir().unwrap();

        assert!(split_file(source.path(), out_dir.path(), SplitMode::Lines(0)).is_err());
        assert!(split_file(source.path(), out_dir.path(), SplitMode::Bytes(0)).is_err());
    }
}
