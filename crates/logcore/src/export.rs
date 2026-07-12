use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportSummary {
    pub written_lines: usize,
    pub written_bytes: u64,
}

pub fn write_raw_line<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<u64> {
    writer.write_all(bytes)?;
    Ok(bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_raw_line_preserves_bytes_and_reports_written_length() {
        let mut out = Vec::new();
        let bytes = b"raw line\xff\n";

        let written = write_raw_line(&mut out, bytes).unwrap();

        assert_eq!(written, bytes.len() as u64);
        assert_eq!(out, bytes);
    }

    #[test]
    fn write_raw_line_propagates_writer_errors() {
        let mut writer = FailingWriter;

        let err = write_raw_line(&mut writer, b"line").unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::Other);
    }
}
