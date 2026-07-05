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
