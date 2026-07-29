use memmap2::Mmap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

#[cfg(windows)]
fn prefetch_virtual_memory(
    process: windows_sys::Win32::Foundation::HANDLE,
    range: &windows_sys::Win32::System::Memory::WIN32_MEMORY_RANGE_ENTRY,
) -> bool {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows_sys::Win32::System::Memory::WIN32_MEMORY_RANGE_ENTRY;

    type PrefetchVirtualMemoryFn =
        unsafe extern "system" fn(HANDLE, usize, *const WIN32_MEMORY_RANGE_ENTRY, u32) -> i32;

    static PREFETCH_VIRTUAL_MEMORY: OnceLock<Option<PrefetchVirtualMemoryFn>> = OnceLock::new();
    let function = PREFETCH_VIRTUAL_MEMORY.get_or_init(|| {
        // PrefetchVirtualMemory is a best-effort optimization introduced in
        // Windows 8. Resolve it lazily so an unavailable export cannot prevent
        // LogFilter (or its test binary) from starting.
        let module = unsafe { GetModuleHandleA(c"kernel32.dll".as_ptr().cast()) };
        if module.is_null() {
            return None;
        }
        let address = unsafe { GetProcAddress(module, c"PrefetchVirtualMemory".as_ptr().cast()) }?;
        // SAFETY: GetProcAddress returned the documented system-ABI export
        // with this exact signature.
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, PrefetchVirtualMemoryFn>(
                address,
            )
        })
    });

    function.is_some_and(|prefetch| unsafe { prefetch(process, 1, range, 0) != 0 })
}

#[derive(Clone)]
pub struct ReadAheadPlan {
    mmap: Arc<Mmap>,
    file: Arc<File>,
    offset: usize,
    len: usize,
}

impl ReadAheadPlan {
    pub const fn requested_bytes(&self) -> usize {
        self.len
    }

    /// Best-effort only: an unsupported or rejected hint must never change
    /// parsing, indexing, or Problems results.
    pub fn execute(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            let (Ok(ra_offset), Ok(ra_count)) = (
                libc::off_t::try_from(self.offset),
                libc::c_int::try_from(self.len),
            ) else {
                return false;
            };
            let advice = libc::radvisory {
                ra_offset,
                ra_count,
            };
            use std::os::fd::AsRawFd;
            // SAFETY: F_RDADVISE only reads the immutable radvisory during the
            // call. The retained File keeps the descriptor valid.
            let result = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_RDADVISE, &advice) };
            let _ = &self.mmap;
            result == 0
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = &self.file;
            self.mmap
                .advise_range(memmap2::Advice::WillNeed, self.offset, self.len)
                .is_ok()
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Memory::WIN32_MEMORY_RANGE_ENTRY;
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            let range = WIN32_MEMORY_RANGE_ENTRY {
                VirtualAddress: self
                    .mmap
                    .as_ptr()
                    .wrapping_add(self.offset)
                    .cast_mut()
                    .cast(),
                NumberOfBytes: self.len,
            };
            let _ = &self.file;
            // SAFETY: the Arc keeps this immutable mapping alive for the call,
            // and the planned range is clamped to that mapping.
            prefetch_virtual_memory(unsafe { GetCurrentProcess() }, &range)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (&self.mmap, &self.file, self.offset, self.len);
            false
        }
    }
}

/// 只读内存映射的文件源。空文件时 `mmap` 为 None(memmap2 无法映射 0 长度文件)。
pub struct MmapSource {
    mmap: Option<Arc<Mmap>>,
    file: Arc<File>,
}

impl MmapSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        let file = Arc::new(file);
        if len == 0 {
            return Ok(Self { mmap: None, file });
        }
        // Safety: 只读映射。外部截断由 `Session::remap_source`(重新 stat + 重映射 + 重建派生状态)侦测;
        // 但两次 remap 之间发生的截断仍可能在访问已消失页时触发 SIGBUS——此为已知残留风险。
        let mmap = unsafe { Mmap::map(file.as_ref())? };
        Ok(Self {
            mmap: Some(Arc::new(mmap)),
            file,
        })
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

    pub fn read_ahead_plan(&self, offset: usize, len: usize) -> Option<ReadAheadPlan> {
        let mmap = self.mmap.as_ref()?;
        let end = offset.saturating_add(len).min(mmap.len());
        (offset < end).then(|| ReadAheadPlan {
            mmap: mmap.clone(),
            file: self.file.clone(),
            offset,
            len: end - offset,
        })
    }
}

pub(crate) fn next_read_ahead_range(
    cursor: usize,
    advised_until: usize,
    total: usize,
    horizon: usize,
    low_water: usize,
) -> Option<(usize, usize, usize)> {
    if cursor >= total {
        return None;
    }
    let remaining_advised = advised_until.saturating_sub(cursor);
    if remaining_advised > low_water {
        return None;
    }
    let offset = cursor.max(advised_until.min(total));
    let end = offset.saturating_add(horizon).min(total);
    (offset < end).then_some((offset, end - offset, end))
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

    #[test]
    fn rolling_read_ahead_is_bounded_clamped_and_refilled_at_low_water() {
        assert_eq!(next_read_ahead_range(0, 0, 200, 64, 32), Some((0, 64, 64)));
        assert_eq!(next_read_ahead_range(31, 64, 200, 64, 32), None);
        assert_eq!(
            next_read_ahead_range(32, 64, 200, 64, 32),
            Some((64, 64, 128))
        );
        assert_eq!(
            next_read_ahead_range(160, 128, 200, 64, 32),
            Some((160, 40, 200))
        );
        assert_eq!(next_read_ahead_range(200, 200, 200, 64, 32), None);
        assert_eq!(
            next_read_ahead_range(usize::MAX - 4, 0, usize::MAX, 64, 32),
            Some((usize::MAX - 4, 4, usize::MAX))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_read_ahead_hint_is_safe_to_execute() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"hello\nworld").unwrap();
        let source = MmapSource::open(file.path()).unwrap();
        let plan = source.read_ahead_plan(0, source.len()).unwrap();

        let _ = plan.execute();
    }
}
