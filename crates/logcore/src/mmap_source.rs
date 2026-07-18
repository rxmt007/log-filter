use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

/// 只读内存映射的文件源。空文件时 `mmap` 为 None(memmap2 无法映射 0 长度文件)。
pub struct MmapSource {
    mmap: Option<Mmap>,
}

impl MmapSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        if len == 0 {
            return Ok(Self { mmap: None });
        }
        // Safety: 只读映射。外部截断由 `Session::remap_source`(重新 stat + 重映射 + 重建派生状态)侦测;
        // 但两次 remap 之间发生的截断仍可能在访问已消失页时触发 SIGBUS——此为已知残留风险。
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(Self { mmap: Some(mmap) })
    }

    pub fn len(&self) -> usize {
        self.mmap.as_ref().map_or(0, |m| m.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> &[u8] {
        self.mmap.as_ref().map_or(&[][..], |m| &m[..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn maps_and_slices() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello\nworld").unwrap();
        let src = MmapSource::open(f.path()).unwrap();
        assert_eq!(src.len(), 11);
        assert_eq!(&src.bytes()[0..5], b"hello");
    }

    #[test]
    fn empty_file_is_zero_len() {
        let f = tempfile::NamedTempFile::new().unwrap();
        let src = MmapSource::open(f.path()).unwrap();
        assert_eq!(src.len(), 0);
        assert_eq!(src.bytes(), b"");
    }
}
