//! Where the shared region lives: POSIX shared memory, or a file in the
//! temporary folder when the system refuses it (a sandboxed host), or
//! plain memory of this library elsewhere.

use crate::Region;

pub(crate) struct Mapping {
    ptr: *const Region,
    #[cfg(not(unix))]
    _own: Box<Region>,
}

// The region is only ever touched through its atomics.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    pub(crate) fn region(&self) -> &Region {
        // SAFETY: `ptr` is a live mapping of at least one `Region`
        // (unmapped in `drop`), page-aligned, and every bit pattern is
        // a valid `Region` (atomics only).
        unsafe { &*self.ptr }
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    use super::{Mapping, Region};

    const SIZE: usize = std::mem::size_of::<Region>();

    /// Size `fd` if it is new and map it.
    fn map(fd: libc::c_int) -> Option<*const Region> {
        // SAFETY: plain calls on a descriptor we own; the mapping is
        // checked before use and outlives the descriptor.
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            let sized = libc::fstat(fd, &raw mut st) == 0
                && usize::try_from(st.st_size).is_ok_and(|s| s >= SIZE);
            #[allow(clippy::cast_possible_wrap)]
            let ok = sized || libc::ftruncate(fd, SIZE as libc::off_t) == 0;
            let p = if ok {
                libc::mmap(
                    std::ptr::null_mut(),
                    SIZE,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            } else {
                libc::MAP_FAILED
            };
            libc::close(fd);
            (p != libc::MAP_FAILED).then_some(p.cast::<Region>().cast_const())
        }
    }

    impl Mapping {
        pub(crate) fn open(name: &str) -> Option<Self> {
            let flags = libc::O_RDWR | libc::O_CREAT;
            let mode: libc::c_uint = 0o600;
            // macOS keeps shared-memory names under 31 bytes.
            let shm = CString::new(format!("/{name}")).ok()?;
            // SAFETY: a valid C string; the descriptor goes to `map`.
            let fd = unsafe { libc::shm_open(shm.as_ptr(), flags, mode) };
            let ptr = if fd >= 0 { map(fd) } else { None }.or_else(|| {
                let path = std::env::temp_dir().join(format!("{name}.link"));
                let path = CString::new(path.as_os_str().as_bytes()).ok()?;
                // SAFETY: as above.
                let fd = unsafe { libc::open(path.as_ptr(), flags, mode) };
                if fd >= 0 { map(fd) } else { None }
            })?;
            Some(Self { ptr })
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: the mapping made in `open`, unmapped once.
            unsafe {
                libc::munmap(self.ptr.cast_mut().cast(), SIZE);
            }
        }
    }
}

#[cfg(not(unix))]
impl Mapping {
    /// No sharing yet on this system: a region of its own, so that the
    /// plugin runs (alone).
    pub(crate) fn open(_name: &str) -> Option<Self> {
        // SAFETY: zeroed atomics are valid.
        let own: Box<Region> = unsafe { Box::new_zeroed().assume_init() };
        Some(Self {
            ptr: &raw const *own,
            _own: own,
        })
    }
}
